//! sts2-core: deterministic combat engine for the sts2 Rust port.
//!
//! Mirrors `RUST_PORTING_PLAN_v2.1` §Architecture; types here are placeholders
//! refined in Phases 1–7. See `docs/plans/2026-05-06-rust-porting-plan-v2.1-refined.md`.
//!
//! Source pin: sts2-rl-agent@01c094d (see `PYTHON_SIM_PIN.md`).

/// Content hash of THIS crate's source, stamped at build time by `build.rs`
/// (via the `sts2-srchash` keystone). Compiled INTO the crate, so a stale
/// `sts2-core` rlib — cargo wrongly skipping a recompile — carries its *old*
/// hash, which is exactly how the parity harness detects a `.so` that does not
/// match the current source (issue #15, bead `sts2-rs-cg5ci`).
pub const SRC_HASH: &str = env!("STS2_CORE_SRC_HASH");

use std::num::NonZeroU32;

use smallvec::SmallVec;

// Bead sts2-rs-ung (P8-4): optional mimalloc reexport. Consumers opt in by
// activating the `mimalloc` cargo feature **and** registering the allocator
// at binary scope, e.g.
//
//     #[cfg(feature = "mimalloc")]
//     #[global_allocator]
//     static GLOBAL: sts2_core::alloc::MiMalloc = sts2_core::alloc::MiMalloc;
//
// See `docs/perf-notes-2026-05-12.md` for measured deltas.
#[cfg(feature = "mimalloc")]
pub mod alloc {
    pub use mimalloc::MiMalloc;
}

// Phase 1 (bead sts2-rs-yjt): card data + pile primitives.
//
// P6 (bead sts2-rs-y97 / vertical-slice expansion): retired the placeholder
// `Card { id }` previously defined further down in this file. The rich
// `cards::Card { uid, card_id, cost, card_type, target_type, upgraded,
// base_damage, base_block }` is now the canonical runtime card and is
// re-exported at crate root via the `pub use` below. `state.cards: CardStore`
// is finally populated at deck-build time (`sts2-py::build_encounter_state`)
// so `combat_loop::play_card` can dispatch on `card.card_id`.
pub mod cards;
/// Fixed-capacity `Copy` key→i32 map backing `Card::combat_vars` /
/// `Card::enchantments` (what makes `Card: Copy` — see the module docs).
pub mod inline_kv;
pub use cards::{Card, CardStore};

/// Root blindfold for honest-search combat MCTS (F0). See module docs
/// for the canonicalization argument. Bead `sts2-rs-vqckz`.
pub mod blindfold;

/// Per-card afflictions (Devoured, Bound, Entangled, …). Mirrors C#
/// `MegaCrit.Sts2.Core.Models.Afflictions/`; scope + decomp at
/// `docs/audits/2026-05-29-7n7tb-card-affliction-port-scope.md`.
/// Bead `sts2-rs-96546` (7n7tb sub-1).
pub mod card_afflictions;

// Phase 1 (bead sts2-rs-qai): 47 hook signatures + ListenerKind + HookId.
// Phase 2 dispatcher (bead sts2-rs-73b) wires iter_power_listeners +
// iter_relic_listeners through the per-hook match table.
pub mod hooks;

// Phase 0 (bead sts2-rs-ose): parity-trace serialization. Output shape
// mirrors `sts2_env.parity.bridge_replay.combat_state_to_bridge_state` from
// sts2-rl-agent@01c094d. Phase 0 emits an empty-state placeholder; later
// phases populate the body as the engine grows.
pub mod parity;

// Phase 2 (bead sts2-rs-5if VTHIN-2): `PowerKind` discriminant lives in its
// own module so the eventual ~10-variant enum (Vulnerable, Weak, Frail,
// Strength, Dexterity, ...) does not bloat lib.rs. VTHIN-2 ships exactly
// the `Vulnerable` variant; bead sts2-rs-6w8 (P2-2) adds the rest.
pub mod powers;
pub use powers::PowerKind;

// Phase 2 (bead sts2-rs-5if VTHIN-2): direct `apply_damage` primitive.
// Walks the modify_damage pipeline plumbed by sts2-rs-58w, then absorbs
// via block + applies the residual as HP loss. NO HP-loss-modifier chain
// (bead sts2-rs-nj8 lands that); NO block-debuff hooks (Phase 2). Just:
// pipeline → block → hp.
pub mod damage;

// Opt-in forensics for "which effect spent that HP?" — the question a mirror
// desync poses and cannot answer. Off by default; see the module docs.
pub mod damage_trace;

// The game's own combat log, ported: an opt-in, STATE-RESIDENT list of every
// hit / block / power / play, with the card or move that caused it. Unlike
// `damage_trace` above it is neither thread-local nor damage-only, so it can
// back a UI and survives a state moving between threads.
pub mod event_log;

// Phase 3 (bead sts2-rs-ypr P3-1): turn state machine entry points.
// Initial cut: `start_combat` resets scalars, fires `before_combat_start`,
// rolls initial enemy intents, advances Setup -> PlayerTurnStart. P3-2..P3-8
// extend with player turn-start, end-turn pipeline, enemy turn execution,
// and `check_combat_end`.
pub mod combat_loop;

// Phase 4 (bead sts2-rs-pg5 P4-3): stub monster system + 5 Act-1 enemies.
// Hand-coded `roll_intent(rng, history) -> Intent` per enemy; Phase 5's
// monster DSL generalizes this to data-driven state machines. The 5
// enemies (Cultist, JawWorm, AcidSlimeS, GreenLouse, Sentry) match the
// v2.1 plan §Phase-4 line 115 list.
pub mod ascension;
pub use ascension::{get_value_if_ascension, AscensionLevel};
pub mod monsters;
pub use monsters::{Intent, IntentHistory, IntentKindTag, MonsterId, MonsterKind};

// Bead sts2-rs-ov4 (P6-1): EffectResult ADT + reusable damage/block/power
// primitives used by the `PlayCard` dispatch registry in
// `sts2-content::cards::dispatch`. The registry routes per-CardId; the
// primitives live in core so future beads can call them directly without
// the registry round-trip.
pub mod card_effects;
pub use card_effects::EffectResult;
pub mod enemy_death_effects;

// Phase 5 (bead sts2-rs-gqy P5-1): Monster AI state machine DSL.
// Data-driven replacement for the hand-coded `MonsterKind::roll_intent`
// dispatch. Content sub-beads (sts2-rs-05f..sts2-rs-uzn) construct
// per-monster `MonsterAi` instances via struct literals.
pub mod monster_ai;
pub use monster_ai::MonsterAi;

// Phase 4 (bead sts2-rs-u4u P4-2): `RelicKind` discriminant lives in its
// own module so the eventual ~190-variant enum (BurningBlood, BlackBlood,
// CrackedCore, ...) does not bloat lib.rs. P4-2 ships exactly the
// `BurningBlood` variant; Phase 7 (umbrella bead sts2-rs-h9u) adds the
// rest of the pool plus the relic-effect interpreter that fires
// `RelicEffect::HealAtCombatEnd` and friends.
pub mod relics;
pub use relics::{RelicId, RelicKind};

// Phase 7 (bead sts2-rs-13q P7-6): potion discriminant + POD. Mirrors
// the `RelicKind`/`Relic` split established by P4-2. Per-potion
// factories + on-drink dispatch live in `sts2-content::potions`.
/// A search's value of the potion belt at its evaluations (sts2-agent fork).
pub mod belt_value;
pub mod potions;
pub use potions::{
    CharacterId, PotionKind, PotionRarity, PotionTargetType, PotionUsageType, MAX_POTIONS,
};

// Bead sts2-rs-13q (P7-6): potion on-drink dispatch registry. Mirrors
// `relic_effects` (P7-1). Per-potion handlers registered by
// `sts2-content::potions::dispatch`; consumers look up via
// [`potion_effects::dispatch`].
pub mod potion_effects;

// Bead sts2-rs-62t (P7-surf): RunState reward queue + Room / RoomType
// / Reward / CardReward / RelicReward PODs. Hosts the
// `pending_rewards` field that the `modify_rewards` /
// `modify_rest_site_heal_rewards` / `modify_card_reward_options_late`
// hook channels in `relic_effects` mutate.
pub mod runstate;
pub use runstate::{
    CardReward, PendingDeckEnchant, PendingDeckTransform, PotionReward, RelicRarity, RelicReward,
    Reward, Room, RoomType, RunState,
};

// Bead sts2-rs-6ynt.4.1 (P7 events framework): Run-side events POD
// set (EventId, EventOptionInfo, EventOutcome). Per-event static
// templates + handler dispatch live in `sts2-content::events`.
pub mod events;
pub use events::{
    apply_event_effects, EventEffect, EventId, EventOptionInfo, EventOutcome, EventVar,
    EventVarSpec, MultiPageEventState,
};

// Bead sts2-rs-l7l (P7-1): Relic dispatch registry. Mirrors the
// `card_effects` registry from bead sts2-rs-ov4 (P6-1). Keys per-relic
// on-trigger handlers by [`RelicId`]; the extension-point façade lives
// in `sts2-content::relics::dispatch::install_default_registry`.
pub mod relic_effects;

// Bead sts2-rs-hfpy1 (9l5c2 sub-1): per-PowerKind dispatch lanes for
// `AfterApplied` / `AfterCardEnteredCombat` / `AfterRemoved` on
// monster-owned creature powers. Parallel to the relic-side lanes in
// `relic_effects`, keyed by `PowerKind` instead of `RelicId`. Scope:
// docs/audits/2026-05-29-9l5c2-hungerpower-hook-dispatch-scope.md.
pub mod monster_power_effects;

// Bead sts2-rs-zq8 (j87-b): RunState-side relic hook bus. Symmetric
// sibling of `relic_effects`; hosts dispatchers whose firing path is
// out-of-combat (currently `after_obtained`; sub-bead `sts2-rs-ouv`
// will add `after_room_entered`).
pub mod run_effects;

// Bead sts2-rs-uiq (P8-1): deterministic structural hash of CombatState
// for MCTS transposition tables. See module docstring for the stability
// contract — stable within a build, not across pin bumps.
pub mod hashing;
pub use hashing::{chance_outcome_hash, run_hash, search_hash, state_hash};

// Bead sts2-rs (AutoSlay-inspired full-run fuzzer, B1): engine invariant
// checks shared between the `combat_step` libfuzzer target and the
// full-run fuzzer. Result-returning ports of the 21-point suite so the
// full-run sweep can record a failing seed instead of aborting.
pub mod invariants;

// Bead sts2-rs (AutoSlay-inspired full-run fuzzer, B3/B4): seeded
// full-run driver (the engine-level analog of the C# AutoSlay harness)
// with a pluggable RunPolicy and a run-hash livelock watchdog.
pub mod full_run;
pub use full_run::{
    drive_full_run, FullRunConfig, FullRunReport, RandomPolicy, RunOutcome, RunPolicy,
};

// Bead sts2-rs-qctu.1 (Wave-6 ExIt schema extension). Wide-format
// feature extractor (~80 floats) sister to the 12-feature
// `eval_features` surface used by the Texel HCE tuner. See module
// docstring for column layout.
pub mod eval_wide;
pub use eval_wide::{
    eval_features_wide, evaluate_in_progress_wide, wide_leaf_score, EVAL_FEATURE_NAMES_WIDE,
    EVAL_FEATURE_WEIGHTS_WIDE, WIDE_FEATURE_DIM, WIDE_IN_PROGRESS_DIVISOR,
};

// Phase 3 (combat-search-redesign Task 3.1): belief-state draw pile —
// information-set representation for the IS-search core. Not wired into the
// live engine; consumed only by the Phase-3 search. See
// `docs/plans/2026-06-09-combat-phase3-information-set-search-implementation.md`.
pub mod belief_pile;
pub use belief_pile::BeliefDrawPile;

// NOTE: the Stance state machine (bead sts2-rs-4t5, P6-6) lived here until
// GH #408. It modelled a Slay-the-Spire-*1* mechanic belonging to a class STS2
// does not have, so nothing ever entered a stance: no writer, no reader, and no
// card that could reach one. It was removed together with the
// `PlayerCombatState::stance` field, its `state_hash` contribution and the
// `$.player.stance` wire field. See GH #406 (Watcher removal) for the sibling
// cleanup.

// Bead sts2-rs-spy (P7-7): Defect orb mechanic (Channel / Evoke / passive
// tick + Focus synergy). The 5 orb kinds and `OrbSlots` collection live
// here; per-card retrofits (Zap, Dualcast, Glacier, Darkness, ...) call
// `orbs::channel_orb` / `evoke_front` / `evoke_all` from
// `sts2-content::cards::defect`.
pub mod orbs;
pub use orbs::{Orb, OrbKind, OrbSlots, DARK_ORB_BASE_EVOKE};

// Bead sts2-rs-6ynt.11: enchantment hook channels (6 surface
// dispatchers — `on_card_drawn`, `after_player_turn_start`,
// `before_flush`, `modify_shuffle_order`, `on_card_played`,
// `after_card_played`). Per-card stubs are no-ops at this pin; the
// listener bodies (Slither, Imbued, SlumberingEssence, PerfectFit,
// Adroit/Swift/Sown/Corrupted/Momentum, Vigorous/Glam/Goopy) land in
// follow-up sub-beads filed under `sts2-rs-6ynt.11.*`.
pub mod enchantment_hooks;

// Bead sts2-rs-5mfyd (P9.0.b): Encounter pools — thin-slice MVP. One
// normal + one boss spec for Act 1, deterministic `pool[0]` pick.
// Full per-act roster + RNG selection lands in P9.1.f.
pub mod encounters;
pub use encounters::{
    act1_boss_pool, act1_normal_pool, select_first, EncounterMember, EncounterSpec,
};

// Bead sts2-rs-d19 (P7-8): Affliction registry — classifies every
// `PowerKind` variant by polarity + duration semantics. Used by the
// turn-end cleanup pass, polarity-aware relic handlers (Artifact),
// and parity coverage tests.
pub mod afflictions;

// Bead sts2-rs-jjcb8 (P9.0.a): per-character starter pack registry
// (HP / gold / starter-relic / post-combat heal). Mirrors Python's
// `_CHARACTER_CONFIG` in `sts2_env/run/run_manager.py:74-105`. The
// run-loop's `enter_combat`/`initialize_run` bridge reads this when
// seeding `RunState` for a new run.
pub mod characters;
pub use characters::{character_starter, CharacterStarter};

// Bead sts2-rs-zregk (P9.1.a): act-map data PODs (`MapCoord`,
// `MapEdge`, `MapPoint`, `MapPointType`). The graph container
// (`ActMap`) lands in P9.1.b; categorical helpers on `MapPointType`
// land in P9.1.c. Port pin
// `sts2-rl-agent@01c094d:sts2_env/map/map_point.py`.
pub mod map;
pub use map::{
    ActMap, MapCoord, MapEdge, MapPoint, MapPointHistoryEntry, MapPointIdx, MapPointType,
};

// Bead sts2-rs-wn4wc (P9.2.b): .NET `System.Random`-compatible RNG
// primitive used by the act-map generator. Port pin
// `sts2-rl-agent@01c094d:sts2_env/core/rng.py`. The multi-stream
// `RunRngSet` registry lands separately in P9.1.g (`sts2-rs-ewm6b`)
// and will compose this primitive into named streams.
pub mod dotnet_rng;
pub use dotnet_rng::{canonicalize_seed, deterministic_hash_code, DotNetRandom, Rng as DotNetRng};

// Bead sts2-rs-ewm6b (P9.1.g): multi-stream RNG bundle composed on top
// of [`dotnet_rng::Rng`]. Mirrors Python `RunRngSet` at
// `sts2_env/run/run_state.py:1340-1370`. Lives on
// [`runstate::RunState::rng`]; downstream beads migrate the existing
// `rewards_rng_seed` Xoshiro call sites to per-stream determinism.
pub mod run_rng_set;
pub use run_rng_set::RunRngSet;

/// C#-faithful run-setup room generation off the shared `up_front` stream
/// (`RunManager.GenerateRooms`). Bead `sts2-rs-jenwp`.
pub mod run_room_gen;

/// Seed-driven act-variant selection (`ActModel.GetRandomList`, off the
/// dedicated `act_selection` stream). A PARITY INSTRUMENT ONLY: the engine
/// still hardcodes Act 1 = Overgrowth, because Underdocks content is not
/// ported. See the module docs for why this blind spot survived so long.
pub mod act_selection;
pub use act_selection::{act1_for_seed, ActSelectionUnlocks, ActVariant};

/// Boss-relic Ancient-event option rolls (acts 2/3). Bead `sts2-rs-40ikw`
/// (task #19). Per-event isolated RNG; faithful to each ancient's
/// `GenerateInitialOptions`.
pub mod boss_ancient_offers;

// Bead sts2-rs-qq9og (R3.1.1 under sts2-rs-nr8l4): Neow option-roll
// RNG algorithm. Pure-function port of C# `Neow.GenerateInitialOptions`
// (../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Events/Neow.cs:213-296).
// Lives in sts2-core (not sts2-content) so `runstate::RunState` can
// cache the rolled offers as a typed field (sub-bead `sts2-rs-14cid`).
pub mod neow_offers;
pub use neow_offers::{roll_neow_offers, NeowOffers, NeowRelic};

// Bead sts2-rs-zfmfz (P9.0.d): top-level run orchestrator skeleton.
// Phase enum + RunManager struct + new()/phase()/get_available_actions()
// /take_action() API surface, no phase behavior wired. Downstream P9.0.e
// (linear map driver) + P9.1+ (per-phase handlers) fill in the
// transitions. Port pin: `sts2_env/run/run_manager.py:182-450`.
pub mod run_manager;
pub use run_manager::{ActionResult, Phase, RunAction, RunManager};

// Bead sts2-rs-70anj.3.20: Pael cluster shared run-level trackers
// (pet level, sacrificed/fed counters). Lives on `RunState::pael_state`;
// per-relic listeners (`PaelsWing`, etc.) read/mutate via the run-state
// accessor methods so multiple Pael relics share one ledger.
pub mod pael_state;
pub use pael_state::PaelState;

// Bead sts2-rs-wn4wc (P9.2.b): standard act-map generator. Ports
// `_path_generate`, `_assign_room_types`, `_center_grid`,
// `generate_act_map`, plus the spoils variant + golden-path tutorial
// map. Composes [`ActMap`] from [`map`] with the [`dotnet_rng::Rng`]
// primitive. Port pin:
// `sts2-rl-agent@01c094d:sts2_env/map/generator.py`.
pub mod map_generator;
pub use map_generator::{
    generate_act_map, generate_golden_path_map, generate_spoils_act_map, PoolCounts,
};

// Bead `sts2-rs-l8hcp` (R3.2 sub-bead under `sts2-rs-5hsqp`):
// `MapPathPruning` port from C# v0.103.2
// (`MegaCrit.Sts2.Core.Map/MapPathPruning.cs`). Runs between
// `assign_room_types` and `center_grid` in [`generate_act_map`]:
// up to 3 iterations of duplicate-segment pruning + Monster→pool
// type repair. The Python sim pin `01c094d` had no pruning at all;
// this is a C#-only port.
pub mod map_path_pruning;
pub use map_path_pruning::prune_and_repair;

// Bead `sts2-rs-ed5nw` (R-Act1-Replay W2 — umbrella `sts2-rs-i6h9i`):
// path inference utility — recover a per-row column sequence from a
// scraped `(map_point_type)` walk through a generated `ActMap`. Pure
// structural DFS + room-type filter, with a "straightest path"
// tie-break when room_type alone leaves multiple candidates.
pub mod path_inference;
pub use path_inference::{infer_visited_cols, InferenceError, InferenceResult};

// Bead sts2-rs-7kptz (P9.1.d): per-act configuration + 3-act registry
// (ActConfig + act_1/act_2/act_3/all_acts). Port pin
// `sts2-rl-agent@01c094d:sts2_env/map/acts.py`.
pub mod acts;
pub use acts::{act_1, act_2, act_3, all_acts, ActConfig};

// Bead sts2-rs-jyfvu (P9.1.e thin slice): card-reward rarity roller +
// pity counter (`CardRarityOdds`) + per-context base-odds tables +
// `roll_for_upgrade` act-scaled upgrade roll. Concrete card-pool
// sampling lives in `sts2-content` and lands in a follow-up bead.
// Port pin `sts2-rl-agent@01c094d:sts2_env/run/odds.py::CardRarityOdds`
// + `sts2_env/run/rewards.py::{generate_card_reward, roll_for_upgrade}`.
pub mod card_rewards;
pub use card_rewards::{
    combat_card_upgrade_base_chance, roll_for_upgrade, CardRarityOdds, RarityContext, RewardRarity,
};

// Bead sts2-rs-o0x66 (P9.2.c.1 bounded slice): merchant inventory
// PODs (`ShopInventory` + `ShopCardEntry` / `ShopRelicEntry` /
// `ShopPotionEntry`) + the full Python price-table constants
// (`CARD_BASE_COST_*`, `POTION_BASE_COST_*`, `RELIC_BASE_COST_*`,
// removal pricing, variance bounds, sold-out sentinel). Pure POD +
// constants; the inventory-roll generator lands in `sts2-rs-64mq2`.
// Port pin `sts2-rl-agent@01c094d:sts2_env/run/shop.py`.
pub mod shop;
pub use shop::{
    ShopCardEntry, ShopInventory, ShopPotionEntry, ShopPotionRarity, ShopRelicEntry,
    ShopRelicRarity,
};

// Bead sts2-rs-khzyk (P9.2.f bounded slice): unknown ('?') map-point
// resolver — `UnknownMapPointOdds` rolling odds + the per-relic
// `modify_unknown_map_point_room_types` channel walk
// (`sts2-rs-vuxmm`). Port pin
// `sts2-rl-agent@01c094d:sts2_env/run/odds.py::UnknownMapPointOdds` +
// the `?`-resolver in `sts2_env/run/run_state.py` /
// `sts2_env/run/run_manager.py::_enter_room`.
pub mod unknown_room_odds;
pub use unknown_room_odds::{
    fallback_order as unknown_room_fallback_order, UnknownMapPointOdds,
    BASE_ELITE as UNKNOWN_BASE_ELITE, BASE_MONSTER as UNKNOWN_BASE_MONSTER,
    BASE_SHOP as UNKNOWN_BASE_SHOP, BASE_TREASURE as UNKNOWN_BASE_TREASURE,
};

// ---------------------------------------------------------------------------
// UID newtypes
// ---------------------------------------------------------------------------

/// Define a `NonZeroU32` newtype with the standard derive set used across the
/// engine. Keeps stores small (4 bytes per UID, 0 is the niche so
/// `Option<Uid>` is also 4 bytes) and prevents accidental cross-store mixing.
macro_rules! define_uid {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(transparent)]
        pub struct $name(pub NonZeroU32);

        impl $name {
            /// Construct from a raw `u32`. Returns `None` if `raw == 0`.
            #[inline]
            pub const fn new(raw: u32) -> Option<Self> {
                match NonZeroU32::new(raw) {
                    Some(n) => Some(Self(n)),
                    None => None,
                }
            }

            /// Raw underlying value (always non-zero).
            #[inline]
            pub const fn get(self) -> u32 {
                self.0.get()
            }
        }
    };
}

define_uid!(
    /// Stable handle into [`CreatureStore`].
    CreatureUid
);
define_uid!(
    /// Stable handle into [`CardStore`].
    CardUid
);
define_uid!(
    /// Stable handle into [`PowerStore`].
    PowerUid
);
define_uid!(
    /// Stable handle into [`RelicStore`].
    RelicUid
);
define_uid!(
    /// Stable handle into [`PotionStore`].
    PotionUid
);

// ---------------------------------------------------------------------------
// POD store payloads
//
// These are deliberately minimal scaffolding — Phase 1+ will flesh them out
// with HP, intent, card kind, power amount, etc. Keeping them as `id`-only POD
// today gives the rest of the engine a stable surface to compile against.
// ---------------------------------------------------------------------------

/// Creature record. Mirrors `sts2_env/core/creature.py::Creature`'s combat-
/// scoped scalar shape: hp, max hp, block, the powers attached to this
/// creature, and (bead sts2-rs-pg5 P4-3) `kind: MonsterKind` +
/// `intent: Option<Intent>`. The `id` newtype is the arena handle
/// (non-zero so `Option<Creature>` is niche-packed).
///
/// Field history:
///   * Bead sts2-rs-5if (VTHIN-2): grew from `{ id }` placeholder to add
///     `current_hp` / `max_hp` / `block` / `powers` so `apply_damage`
///     has somewhere to write hp loss + block absorption and so
///     `iter_power_listeners` can walk enemy powers.
///   * Bead sts2-rs-pg5 (P4-3): added `kind: MonsterKind` (discriminant
///     for the 5 stub enemies) and `intent: Option<Intent>` — the latter
///     is `None` for any creature without an active AI (e.g. allies, or
///     a Cultist constructed for a fixture that bypasses the Python
///     `combat.add_enemy(creature, ai)` path). The parity exporter only
///     emits `intent` / `intent_damage` / `intent_hits` when
///     `intent.is_some()`, matching Python's
///     `combat_state_to_bridge_state` lines 332-339 which only emit
///     when `ai is not None`.
///   * Bead sts2-rs-o7r (P3-stretch): added `intent_history:
///     IntentHistory`. `combat_loop::enemy_turn_exec` was previously
///     constructing a fresh `IntentHistory::default()` per roll, which
///     collapsed multi-state monsters back to their turn-0 intent every
///     turn (Cultist Incantation→Attack pattern degraded to Incantation
///     forever; `AcidSlimeS` Tackle/Lick "no two same in a row" never
///     fired; `JawWorm` caps were ignored). Persisting the history on the
///     creature lets `roll_intent` see the prior move and enforce the
///     repeat-limit / state-machine semantics across turns. The field
///     is NOT emitted by the parity exporter — the Python pin
///     (`sts2-rl-agent@01c094d`) keeps the equivalent state inside
///     `MonsterAI` (a non-serialized object), so adding it to the
///     bridge dict would be a parity divergence.
#[derive(Clone, Debug)]
pub struct Creature {
    pub id: NonZeroU32,
    pub current_hp: i32,
    pub max_hp: i32,
    pub block: i32,
    pub powers: SmallVec<[PowerUid; 8]>,
    /// Species discriminant for monster-Creatures, `None` for the
    /// player-Creature. Bead `sts2-rs-a19m` migrated this off the
    /// legacy STS1 sentinel that the player-Creature used pre-a19m;
    /// the STS1 stub variants were later removed in bead
    /// `sts2-rs-v1u5`. Callers that
    /// need to distinguish player/monster should use
    /// [`CombatState::creature_kind`]; `species` is the *what kind of
    /// monster* discriminant once you already know it's a monster.
    pub species: Option<MonsterKind>,
    pub monster_id: MonsterId,
    pub intent: Option<Intent>,
    pub intent_history: IntentHistory,
    pub ai: Option<MonsterAi>,
}

impl PartialEq for Creature {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.current_hp == other.current_hp
            && self.max_hp == other.max_hp
            && self.block == other.block
            && self.powers == other.powers
            && self.species == other.species
            && self.monster_id == other.monster_id
            && self.intent == other.intent
            && self.intent_history == other.intent_history
    }
}

impl Eq for Creature {}

impl Creature {
    /// Increase `max_hp` by `amount` and grow `current_hp` by the same
    /// amount (clamped at the new `max_hp`). Returns the **signed delta**
    /// applied to `max_hp` (always `>= 0`; negative / zero `amount` is a
    /// no-op).
    ///
    /// Mirrors `sts2-rl-agent@01c094d:sts2_env/core/creature.py::
    /// Creature.gain_max_hp`:
    /// ```text
    /// self.max_hp += amount
    /// self.current_hp = min(self.current_hp + amount, self.max_hp)
    /// ```
    ///
    /// Bead `sts2-rs-tcf` (P7-surf). The companion broadcast lives on
    /// [`CombatState::creature_gain_max_hp`] (which forwards to
    /// [`crate::relic_effects::dispatch_after_max_hp_changed`] after the
    /// mutation). The bare `&mut self` method is exposed for callers
    /// that already own a `&mut Creature` and don't need the bus event
    /// (e.g. `RunState` rest-site healing — `run_state.gain_max_hp(9)`
    /// in Python).
    #[inline]
    pub fn gain_max_hp(&mut self, amount: i32) -> i32 {
        if amount <= 0 {
            return 0;
        }
        // Bead `sts2-rs-73up8`: saturating add. The audit flagged the
        // pre-existing `self.max_hp += amount` as a panic-in-debug /
        // wrap-in-release hazard on extreme `amount`. Mirrors the
        // defensive arithmetic used elsewhere in the engine for
        // game-state scalars bounded above by player health rolls.
        self.max_hp = self.max_hp.saturating_add(amount);
        self.current_hp = self.current_hp.saturating_add(amount).min(self.max_hp);
        amount
    }

    /// Decrease `max_hp` by `amount`, flooring at 1, and clamp
    /// `current_hp` to the new `max_hp`. Returns the **signed delta**
    /// applied to `max_hp` (always `<= 0`; negative / zero `amount` is a
    /// no-op).
    ///
    /// Mirrors `sts2-rl-agent@01c094d:sts2_env/core/creature.py::
    /// Creature.lose_max_hp`:
    /// ```text
    /// self.max_hp = max(1, self.max_hp - amount)
    /// self.current_hp = min(self.current_hp, self.max_hp)
    /// ```
    ///
    /// Bead `sts2-rs-tcf` (P7-surf).
    #[inline]
    pub fn lose_max_hp(&mut self, amount: i32) -> i32 {
        if amount <= 0 {
            return 0;
        }
        let before = self.max_hp;
        self.max_hp = (self.max_hp - amount).max(1);
        if self.current_hp > self.max_hp {
            self.current_hp = self.max_hp;
        }
        self.max_hp - before
    }
}

/// Power record. Mirrors `sts2_env/powers/base.py::PowerInstance`'s
/// load-bearing scalars: a discriminant (`kind`, mapped 1-to-1 to Python's
/// `PowerId`) and `amount` (Python's `self.amount` — duration-in-turns for
/// stack-counted powers like Vulnerable, absolute value for stat-tracking
/// powers like Strength).
///
/// Bead sts2-rs-5if (VTHIN-2) grew this from `{ id }` placeholder to the
/// fields below so the multiplicative dispatcher can discriminate on
/// `kind` and emit `DamageMul::VULNERABLE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Power {
    pub id: NonZeroU32,
    pub kind: PowerKind,
    pub amount: i32,
    /// Per-instance counter for powers that need turn-scoped state
    /// (e.g. Unmovable's block-gains-this-turn count, CrimsonMantle /
    /// Inferno's self_damage accumulator). Default 0.
    ///
    /// Bead `sts2-rs-6ynt.6.11` adds this field as the precursor to the
    /// self-damage rituals (`6.3`), Unmovable (`6.5`), and Grapple
    /// (`6.7`) sibling beads. Stack policy: when stacking two instances
    /// of the same `kind`, the existing instance's counter is preserved
    /// (per-instance scratch — the new stack does NOT reset it).
    /// Powers that need a non-zero initial value MUST set `counter`
    /// explicitly at construction (mirrors the [`Relic::counter`]
    /// precedent landed in bead `sts2-rs-3nis`).
    pub counter: i32,
    /// Per-instance applier — set when a creature applies the power to
    /// another and the listener body needs to gate on `creature == applier`
    /// (e.g. Grapple). `None` for self-applied / system-applied powers.
    ///
    /// Bead `sts2-rs-6ynt.6.11`. Stack policy: latest applier wins —
    /// re-applying a power overwrites the stored applier reference,
    /// mirroring Python's `PowerInstance.applier` re-binding when an
    /// already-stacked power receives a fresh `apply_power` call.
    pub applier: Option<CreatureUid>,
    /// One-shot "skip the next decrement tick" flag. Mirrors Python's
    /// `power_listener.skip_next_tick` short-circuit. When the
    /// freshly-applied power's first tick would otherwise fire, the
    /// listener consumes (clears) this flag and skips the tick. Used
    /// by Vulnerable / Weak / Frail to defer the first decrement to
    /// the NEXT turn-end after application — without this flag, a
    /// curse like Doubt / Shame that applies Weak / Frail to the
    /// player at the player's own turn-end would have its applied
    /// power immediately decrement at the same turn-end's enemy-side
    /// tick (one extra tick than Python).
    ///
    /// Set by [`crate::card_effects::apply_power_to_creature`] when
    /// the target did not previously carry the power. Cleared by the
    /// Vulnerable / Weak / Frail tick listeners after their first
    /// skipped fire.
    ///
    /// Bead `sts2-rs-j0ek`. Python source:
    /// `sts2-rl-agent@01c094d:sts2_env/powers/common.py:75-129`
    /// (`Vulnerable`/`Weak`/`Frail` `at_end_of_turn` listener short-
    /// circuits on the equivalent flag) +
    /// `sts2_env/core/combat.py:2417-2423,2437-2442` (`DOUBT`/`SHAME`
    /// set the flag when freshly applying the power).
    pub skip_next_tick: bool,
}

impl Power {
    /// Construct a `Power` with default per-instance state
    /// (`counter = 0`, `applier = None`, `skip_next_tick = false`).
    /// Existing call sites use this shape; powers that need a
    /// non-zero `counter`, a non-`None` `applier`, or a freshly-set
    /// `skip_next_tick` should build the struct literally and set
    /// those fields.
    #[inline]
    #[must_use]
    pub const fn new(id: NonZeroU32, kind: PowerKind, amount: i32) -> Self {
        Self {
            id,
            kind,
            amount,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        }
    }
}

/// Relic record. Mirrors `sts2_env/relics/base.py::RelicInstance`'s
/// load-bearing scalar shape: an arena handle and a discriminant
/// (`kind`, mapped 1-to-1 to Python's `RelicId`). Per-instance counters
/// (`pulse_value`, charges-remaining for single-use relics, ...) land
/// in Phase 7 as the umbrella bead `sts2-rs-h9u` ports the per-relic
/// runtime state.
///
/// Field history:
///   * Bead sts2-rs-u4u (P4-2): grew from `{ id }` placeholder to add
///     `kind: RelicKind` so the Phase-7 dispatcher can discriminate
///     on `kind` and look up the matching `RelicEffect` from
///     `sts2_content::relics`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Relic {
    pub id: NonZeroU32,
    pub kind: RelicKind,
    /// Per-instance scratch field for relics with charge / stack /
    /// turn-count semantics. Mirrors the per-instance attributes that
    /// Python `RelicInstance` subclasses define in `__init__` (e.g.
    /// `EmberTea._combats_left`, `BoneTea._combats_left`,
    /// `BingBong._skip_next` — every such attribute is named in the
    /// per-relic doc comment in
    /// `crates/sts2-content/src/relics/event_reward.rs`).
    ///
    /// Default: `0`. Relics that need an initial value (e.g. `EmberTea`
    /// starts with `_combats_left = 5`) MUST set `counter` at
    /// construction time — there is no obtain-time hook that
    /// auto-initialises this field. The Python pin tracks each such
    /// counter as an `int` field, so `i32` is the natural Rust shape.
    ///
    /// Bead `sts2-rs-3nis` lands this POD; bead `sts2-rs-46ms` deferred
    /// the 14 event-reward relics whose only remaining blocker was
    /// counter wiring.
    pub counter: i32,
    /// C# `RelicModel.IsWax` — set on the 4 relics `ToyBox` offers when it
    /// is picked up (`MegaCrit.Sts2.Core.Models.Relics/ToyBox.cs:93`). Only
    /// wax relics are eligible to melt; nothing else reads this flag.
    pub is_wax: bool,
    /// C# `RelicModel.IsMelted` — set by `RelicCmd.Melt` (via
    /// `Player.MeltRelicInternal`, `Entities.Players/Player.cs:520`) once
    /// `ToyBox` has seen `DynamicVars["Combats"]` (=3) more combats.
    ///
    /// **A melted relic stays in the inventory.** C# never removes it from
    /// `Player.Relics`. What melting does is remove it from every hook
    /// dispatch: `CombatState.cs:130-137` and `RunState.cs:148` both filter
    /// `!r.IsMelted` when building the listener list. Mirror that by putting
    /// this flag inside every ownership *guard* and keeping it out of
    /// everything that *counts* or *serializes* relics.
    ///
    /// C# reads `Player.Relics` raw — melt-blind — in three places, all
    /// deliberate: `ILikeShiny.cs:24` (`Relics.Count >= 25` badge, not
    /// modelled here), `PaelsEye.cs:152`
    /// (`Relics.Any(r => r is WhisperingEarring)`, not modelled here) and
    /// `TouchOfOrobas.cs:156` (`Relics.First(r => r.Rarity == Starter)` —
    /// starters are never wax). The relic-trade events look melt-blind but
    /// are not: they filter on `RelicModel.IsTradable`, which itself returns
    /// false when `IsMelted` (`RelicModel.cs:188`).
    pub is_melted: bool,
}

impl Relic {
    /// Fresh, unmelted relic with a zero counter — the shape 95% of
    /// construction sites want.
    #[must_use]
    pub const fn new(id: NonZeroU32, kind: RelicKind) -> Self {
        Self {
            id,
            kind,
            counter: 0,
            is_wax: false,
            is_melted: false,
        }
    }
}

/// Placeholder potion record. Bodies fleshed out in Phase 7.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Potion {
    pub id: NonZeroU32,
}

/// Vector-backed arena over [`Creature`]. Indexing uses [`CreatureUid`] in
/// later phases; today it is a thin wrapper so callers can pre-bind the type.
pub type CreatureStore = Vec<Creature>;

// ---------------------------------------------------------------------------
// Dense id lookups (J07)
//
// Every production arena hands out ids densely: a creature gets
// `creatures.len() + 1` and a card `cards.len() + 1`, and neither arena drops
// a row during a combat, so the element with id `n` sits in slot `n - 1`.
// These helpers test that slot first and fall back to the linear scan the
// call sites used to do whenever the slot holds another id (a fixture arena
// that skips ids, or the creature arena after `teardown_player_creature`).
// Ids are unique, so both branches return the element the scan returned, and
// a site that asked `any(|x| x.id == id && pred(x))` gets the same answer from
// `helper(id).is_some_and(pred)`. Debug builds assert the uniqueness on every
// call; `invariants::check_combat_invariants` checks it per step.
// ---------------------------------------------------------------------------

/// The arena creature with id `id`: the dense `creatures[id - 1]` slot when it
/// holds `id`, else the first match of a linear scan.
#[inline]
#[must_use]
pub fn creature_by_id(creatures: &[Creature], id: NonZeroU32) -> Option<&Creature> {
    debug_assert_unique_creature_id(creatures, id);
    match creatures.get(id.get() as usize - 1) {
        Some(c) if c.id == id => Some(c),
        _ => creatures.iter().find(|c| c.id == id),
    }
}

/// Mutable [`creature_by_id`].
#[inline]
pub fn creature_by_id_mut(creatures: &mut [Creature], id: NonZeroU32) -> Option<&mut Creature> {
    debug_assert_unique_creature_id(creatures, id);
    let slot = id.get() as usize - 1;
    if creatures.get(slot).is_some_and(|c| c.id == id) {
        return creatures.get_mut(slot);
    }
    creatures.iter_mut().find(|c| c.id == id)
}

/// Arena index of the creature with id `id` (see [`creature_by_id`]).
#[inline]
#[must_use]
pub fn creature_pos(creatures: &[Creature], id: NonZeroU32) -> Option<usize> {
    debug_assert_unique_creature_id(creatures, id);
    let slot = id.get() as usize - 1;
    if creatures.get(slot).is_some_and(|c| c.id == id) {
        return Some(slot);
    }
    creatures.iter().position(|c| c.id == id)
}

/// The arena card with uid `uid`: the dense `cards[uid - 1]` slot when it
/// holds `uid`, else the first match of a linear scan.
#[inline]
#[must_use]
pub fn card_by_uid(cards: &[Card], uid: CardUid) -> Option<&Card> {
    debug_assert_unique_card_uid(cards, uid);
    match cards.get(uid.get() as usize - 1) {
        Some(c) if c.uid == uid => Some(c),
        _ => cards.iter().find(|c| c.uid == uid),
    }
}

/// Mutable [`card_by_uid`].
#[inline]
pub fn card_by_uid_mut(cards: &mut [Card], uid: CardUid) -> Option<&mut Card> {
    debug_assert_unique_card_uid(cards, uid);
    let slot = uid.get() as usize - 1;
    if cards.get(slot).is_some_and(|c| c.uid == uid) {
        return cards.get_mut(slot);
    }
    cards.iter_mut().find(|c| c.uid == uid)
}

/// Debug guard of the dense lookups: at most one creature answers to `id`.
/// With a duplicate, the dense slot and the first scan match could differ.
#[inline]
fn debug_assert_unique_creature_id(creatures: &[Creature], id: NonZeroU32) {
    debug_assert!(
        creatures.iter().filter(|c| c.id == id).count() <= 1,
        "duplicate creature id {id} in the arena: a dense lookup could pick a \
         different creature than the scan"
    );
}

/// Debug guard of the dense lookups: at most one card answers to `uid`.
#[inline]
fn debug_assert_unique_card_uid(cards: &[Card], uid: CardUid) {
    debug_assert!(
        cards.iter().filter(|c| c.uid == uid).count() <= 1,
        "duplicate card uid {uid:?} in the arena: a dense lookup could pick a \
         different card than the scan"
    );
}

// `CardStore` re-exported above via `pub use cards::{Card, CardStore};`.
/// Arena over [`Power`] carrying an O(1) `uid -> slot` side index.
///
/// Replaces the former `type PowerStore = Vec<Power>`. The arena is
/// **compacted on removal** (see [`PowerStore::retain`]) so a power's uid
/// is NOT its slot, and the engine resolves powers by uid in *hundreds* of
/// per-listener dispatch bodies (`modify_damage`, `modify_hp_lost`,
/// `modify_block`, the `iter_power_listeners` consumers, …). Those used to
/// be O(P) `powers.get_by_id(uid)` linear scans — the single
/// largest *distributed* cost in the engine (D1 in
/// `docs/plans/2026-06-07-mcts-single-threaded-perf-plan.md`). [`get_by_id`]
/// / [`get_mut_by_id`] make them O(1).
///
/// **Byte-identical by construction.** Power id *values* are unchanged
/// (still `max(existing)+1`, see `card_effects.rs`), so `state_hash` and
/// every pinned parity fixture are untouched. The index is a pure function
/// of `powers`, so [`PartialEq`]/[`Eq`] compare ONLY `powers` — a freshly
/// built (cache-cold) store and an engine-mutated (cache-warm) store with
/// equal `powers` still compare equal, preserving the derived `CombatState`
/// equality the parity harness relies on.
///
/// Read access (`iter`, `iter_mut`, indexing, `len`, `is_empty`) is via
/// `Deref`/`DerefMut` to `[Power]`; the slice target deliberately omits the
/// structural `Vec` ops (`push`/`retain`/`sort`/…) so every restructuring
/// goes through the inherent methods that keep the index in sync.
///
/// [`get_by_id`]: PowerStore::get_by_id
/// [`get_mut_by_id`]: PowerStore::get_mut_by_id
/// Number of `u64` words backing a [`PowerKindSet`]. 512 bits — comfortable
/// headroom over the ~274 [`PowerKind`] variants today. [`PowerKindSet::insert`]
/// debug-asserts the day a variant pushes past the capacity, and the const
/// [`PowerKindSet::from_kinds`] would fail const-eval (a compile error) if a
/// channel mask ever named an out-of-range kind — so the two can never drift
/// into an inconsistent, silently-wrong state.
const POWER_KIND_WORDS: usize = 8;

/// Plain (non-atomic) bitset over [`PowerKind`] discriminants.
///
/// Used as a *presence summary* so a dispatcher can skip the entire
/// `iter_power_listeners` walk — and its per-power `get_by_id` resolves — when
/// no power of a channel-relevant kind is live (the dominant dispatch-body cost
/// is the sheer volume of those resolves, most landing on `_ => 0` arms).
///
/// `PowerKind as usize` is the 0-based declaration-order discriminant (the enum
/// is fieldless with no `repr`), so it indexes the bitset directly.
///
/// **Safety / parity:** the presence set is a superset of the kinds that can
/// actually contribute on any channel — a power in the arena always has its bit
/// set, and a uid that resolves to no arena power contributes its default
/// anyway. So `present ∩ mask == ∅` provably means "no live power of a relevant
/// kind", and skipping the walk is bit-identical, never a silent drop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerKindSet {
    words: [u64; POWER_KIND_WORDS],
}

impl PowerKindSet {
    /// The empty set.
    pub const EMPTY: Self = Self {
        words: [0; POWER_KIND_WORDS],
    };

    /// Mark `kind` present. Fails *safe* on overflow (a kind past the bitset
    /// capacity is simply not recorded — but see [`POWER_KIND_WORDS`]: channel
    /// masks can't name such a kind, so no dispatcher would ever consult it).
    #[inline]
    fn insert(&mut self, kind: PowerKind) {
        let i = kind as usize;
        debug_assert!(
            i < POWER_KIND_WORDS * 64,
            "PowerKind discriminant {i} exceeds PowerKindSet capacity"
        );
        if let Some(w) = self.words.get_mut(i / 64) {
            *w |= 1u64 << (i % 64);
        }
    }

    /// Build a compile-time channel mask from the kinds a dispatcher's
    /// per-listener `match` discriminates on. Const-eval indexes `words`
    /// directly, so an out-of-range kind is a *compile error*, not a silent
    /// no-op.
    #[must_use]
    pub const fn from_kinds(kinds: &[PowerKind]) -> Self {
        let mut words = [0u64; POWER_KIND_WORDS];
        let mut n = 0;
        while n < kinds.len() {
            let i = kinds[n] as usize;
            words[i / 64] |= 1u64 << (i % 64);
            n += 1;
        }
        Self { words }
    }

    /// `self` minus `kinds`, in const context: derives a channel's arm mask
    /// from its mention mask (the `*_ARMS` consts in `hooks.rs`). Every
    /// removed kind must be in `self`, so a stale exclusion list is a
    /// compile error rather than dead text.
    #[must_use]
    pub const fn without(self, kinds: &[PowerKind]) -> Self {
        let mut words = self.words;
        let mut n = 0;
        while n < kinds.len() {
            let i = kinds[n] as usize;
            let bit = 1u64 << (i % 64);
            assert!(words[i / 64] & bit != 0, "removed kind is not in the set");
            words[i / 64] &= !bit;
            n += 1;
        }
        Self { words }
    }

    /// `self` plus every kind in `other`, in const context: builds the hook
    /// trigger sets in `hooks.rs` from the walk gates and the lane kinds.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        let mut words = self.words;
        let mut i = 0;
        while i < POWER_KIND_WORDS {
            words[i] |= other.words[i];
            i += 1;
        }
        Self { words }
    }

    /// Number of discriminants the set can hold.
    pub(crate) const CAPACITY: usize = POWER_KIND_WORDS * 64;

    /// True if the kind with discriminant `i` (`kind as usize`) is in the
    /// set, in const context, where a table is built per discriminant.
    #[must_use]
    pub(crate) const fn contains_index(&self, i: usize) -> bool {
        i < Self::CAPACITY && (self.words[i / 64] >> (i % 64)) & 1 != 0
    }

    /// True if every kind in `self` is also in `other`.
    #[must_use]
    pub const fn is_subset_of(&self, other: &PowerKindSet) -> bool {
        let mut i = 0;
        while i < POWER_KIND_WORDS {
            if self.words[i] & !other.words[i] != 0 {
                return false;
            }
            i += 1;
        }
        true
    }

    /// True if `kind` is in the set.
    #[inline]
    #[must_use]
    pub fn contains(&self, kind: PowerKind) -> bool {
        let i = kind as usize;
        match self.words.get(i / 64) {
            Some(w) => (*w >> (i % 64)) & 1 != 0,
            None => false,
        }
    }

    /// True if any kind is in both sets.
    #[inline]
    #[must_use]
    pub fn intersects(&self, other: &PowerKindSet) -> bool {
        let mut acc = 0u64;
        let mut i = 0;
        while i < POWER_KIND_WORDS {
            acc |= self.words[i] & other.words[i];
            i += 1;
        }
        acc != 0
    }
}

#[derive(Clone, Default)]
pub struct PowerStore {
    powers: Vec<Power>,
    /// `slot_of_id[id.get() - 1]` stores `slot + 1`; the `0` sentinel means
    /// "no live power with that id". Sized to the highest live id (ids are
    /// `max+1`-allocated, so this stays bounded by the peak concurrent
    /// power count — a few dozen entries at most).
    ///
    /// `SmallVec` inline (not `Vec`) so the per-simulation `CombatState`
    /// clone (1000×/MCTS search) copies the index by memcpy with **no heap
    /// allocation** in the common power-sparse case — a plain `Vec` here
    /// added an allocation per clone that erased the dispatch win on the
    /// clone-heavy search path. Spills to the heap only past 32 concurrent
    /// power ids, i.e. exactly the power-rich states where the O(1)
    /// dispatch saving dominates anyway.
    slot_of_id: SmallVec<[u32; 32]>,
    /// Presence summary of every live power's [`PowerKind`], maintained in
    /// `push` / `rebuild_index` / `clear`. Lets dispatchers early-out of the
    /// per-power listener walk when no relevant kind is live (see
    /// [`PowerKindSet`]). A derived view of `powers`, so — like `slot_of_id` —
    /// it is excluded from the `PartialEq` arena comparison.
    present_kinds: PowerKindSet,
    /// Hook-liveness word (J06): bit `h` is set when some kind in
    /// `present_kinds` is in the trigger set of [`crate::hooks::PowerHook`]
    /// `h`, i.e. the OR of [`crate::hooks::power_hook_bits`] over the present
    /// kinds. Maintained at the same three chokepoints, so a clear bit proves
    /// no power that could make that dispatcher act exists. Derived like
    /// `present_kinds`: excluded from `PartialEq` and from `Debug`.
    hook_live: u64,
}

/// The derived output of the fields before `hook_live` existed, so a state's
/// `Debug` text (which some parity harnesses hash) is unchanged by it.
#[allow(clippy::missing_fields_in_debug)]
impl core::fmt::Debug for PowerStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PowerStore")
            .field("powers", &self.powers)
            .field("slot_of_id", &self.slot_of_id)
            .field("present_kinds", &self.present_kinds)
            .finish()
    }
}

impl PartialEq for PowerStore {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        // Index is a derived view of `powers`; comparing it would make a
        // cache-cold store compare unequal to a logically identical
        // cache-warm one and break parity. Compare the arena only.
        self.powers == other.powers
    }
}
impl Eq for PowerStore {}

impl core::ops::Deref for PowerStore {
    type Target = [Power];
    #[inline]
    fn deref(&self) -> &[Power] {
        &self.powers
    }
}
impl core::ops::DerefMut for PowerStore {
    #[inline]
    fn deref_mut(&mut self) -> &mut [Power] {
        // `[Power]` exposes element mutation (`iter_mut`, index-assign) but
        // NOT structural ops, and no call site mutates `Power::id` in place,
        // so the index stays valid.
        &mut self.powers
    }
}

impl PowerStore {
    /// Empty store.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self {
            powers: Vec::new(),
            slot_of_id: SmallVec::new(),
            present_kinds: PowerKindSet::EMPTY,
            hook_live: 0,
        }
    }

    /// Empty store with arena capacity reserved.
    #[inline]
    #[must_use]
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            powers: Vec::with_capacity(cap),
            slot_of_id: SmallVec::new(),
            present_kinds: PowerKindSet::EMPTY,
            hook_live: 0,
        }
    }

    #[inline]
    fn id_index(id: NonZeroU32) -> usize {
        (id.get() - 1) as usize
    }

    /// O(1) resolve a power instance by its uid.
    #[inline]
    #[must_use]
    pub fn get_by_id(&self, id: NonZeroU32) -> Option<&Power> {
        let slot = *self.slot_of_id.get(Self::id_index(id))?;
        if slot == 0 {
            return None;
        }
        self.powers.get((slot - 1) as usize)
    }

    /// O(1) mutable resolve a power instance by its uid.
    #[inline]
    pub fn get_mut_by_id(&mut self, id: NonZeroU32) -> Option<&mut Power> {
        let slot = *self.slot_of_id.get(Self::id_index(id))?;
        if slot == 0 {
            return None;
        }
        self.powers.get_mut((slot - 1) as usize)
    }

    /// Append a power, recording its slot in the index. Debug-asserts the
    /// id is not already live (ids are unique instance handles).
    #[inline]
    pub fn push(&mut self, power: Power) {
        let slot = u32::try_from(self.powers.len()).expect("power arena slot fits u32");
        let i = Self::id_index(power.id);
        if i >= self.slot_of_id.len() {
            self.slot_of_id.resize(i + 1, 0);
        }
        debug_assert_eq!(
            self.slot_of_id[i], 0,
            "duplicate power id {} pushed into PowerStore",
            power.id
        );
        self.slot_of_id[i] = slot + 1;
        self.present_kinds.insert(power.kind);
        self.hook_live |= crate::hooks::power_hook_bits(power.kind);
        self.powers.push(power);
    }

    /// Retain powers matching `f`, then rebuild the slot index (slots shift
    /// on compaction). O(P) but removals are far rarer than lookups.
    pub fn retain<F>(&mut self, f: F)
    where
        F: FnMut(&Power) -> bool,
    {
        self.powers.retain(f);
        self.rebuild_index();
    }

    /// Drop all powers.
    #[inline]
    pub fn clear(&mut self) {
        self.powers.clear();
        self.slot_of_id.clear();
        self.present_kinds = PowerKindSet::EMPTY;
        self.hook_live = 0;
    }

    /// Borrow the underlying arena as a slice (explicit form of the `Deref`).
    #[inline]
    #[must_use]
    pub fn as_slice(&self) -> &[Power] {
        &self.powers
    }

    fn rebuild_index(&mut self) {
        for s in &mut self.slot_of_id {
            *s = 0;
        }
        self.present_kinds = PowerKindSet::EMPTY;
        self.hook_live = 0;
        for (slot, p) in self.powers.iter().enumerate() {
            let i = Self::id_index(p.id);
            if i >= self.slot_of_id.len() {
                self.slot_of_id.resize(i + 1, 0);
            }
            self.slot_of_id[i] = u32::try_from(slot).expect("power arena slot fits u32") + 1;
            self.present_kinds.insert(p.kind);
            self.hook_live |= crate::hooks::power_hook_bits(p.kind);
        }
    }

    /// Presence summary of every live power's [`PowerKind`]. Dispatchers
    /// intersect this with a per-channel [`PowerKindSet::from_kinds`] mask to
    /// skip the per-power listener walk when no relevant kind is live.
    #[inline]
    #[must_use]
    pub fn present_kinds(&self) -> &PowerKindSet {
        &self.present_kinds
    }

    /// The hook-liveness word (J06): see the `hook_live` field and
    /// [`crate::hooks::PowerHook`].
    #[inline]
    #[must_use]
    pub(crate) fn hook_live(&self) -> u64 {
        self.hook_live
    }
}

impl<'a> IntoIterator for &'a PowerStore {
    type Item = &'a Power;
    type IntoIter = core::slice::Iter<'a, Power>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.powers.iter()
    }
}

impl<'a> IntoIterator for &'a mut PowerStore {
    type Item = &'a mut Power;
    type IntoIter = core::slice::IterMut<'a, Power>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        // Element mutation only — cannot restructure, so the index holds.
        self.powers.iter_mut()
    }
}

impl From<Vec<Power>> for PowerStore {
    #[inline]
    fn from(powers: Vec<Power>) -> Self {
        let mut store = Self {
            powers,
            slot_of_id: SmallVec::new(),
            present_kinds: PowerKindSet::EMPTY,
            hook_live: 0,
        };
        store.rebuild_index();
        store
    }
}

impl FromIterator<Power> for PowerStore {
    fn from_iter<I: IntoIterator<Item = Power>>(iter: I) -> Self {
        Self::from(iter.into_iter().collect::<Vec<Power>>())
    }
}
/// Number of `u64` words backing a [`RelicKindSet`]: 320 bits over the 297
/// [`RelicKind`] variants today. The const assert below fails the build the
/// day a variant pushes past the capacity.
const RELIC_KIND_WORDS: usize = 5;
const _: () = assert!(RelicKind::ALL.len() <= RELIC_KIND_WORDS * 64);

/// Plain (non-atomic) bitset over [`RelicKind`] discriminants. `RelicKind` is
/// fieldless with no `repr`, so `kind as usize` is its 0-based declaration
/// index and indexes the bitset directly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RelicKindSet {
    words: [u64; RELIC_KIND_WORDS],
}

impl RelicKindSet {
    /// The empty set.
    pub const EMPTY: Self = Self {
        words: [0; RELIC_KIND_WORDS],
    };

    #[inline]
    fn insert(&mut self, kind: RelicKind) {
        let i = kind as usize;
        if let Some(w) = self.words.get_mut(i / 64) {
            *w |= 1u64 << (i % 64);
        }
    }

    /// True if `kind` is in the set.
    #[inline]
    #[must_use]
    pub fn contains(&self, kind: RelicKind) -> bool {
        let i = kind as usize;
        match self.words.get(i / 64) {
            Some(w) => (*w >> (i % 64)) & 1 != 0,
            None => false,
        }
    }

    /// The kinds of the non-melted relics in `relics` — exactly the kinds
    /// `relic_effects::helpers::player_owns_relic` answers `true` for.
    #[must_use]
    pub fn owned_by(relics: &[Relic]) -> Self {
        let mut set = Self::EMPTY;
        for r in relics.iter().filter(|r| !r.is_melted) {
            set.insert(r.kind);
        }
        set
    }
}

/// Vector-backed arena over [`Relic`], carrying two views derived from it:
/// the owned-kind set ([`RelicKindSet::owned_by`]) and the per-combat
/// relic-channel plan (`relic_effects::channels::RelicPlan`, M01) that the
/// owned / equip-order / obtain-order relic dispatchers walk instead of
/// re-scanning the relic list on every dispatch.
///
/// Replaces the former `type RelicStore = Vec<Relic>`. Both views are pure
/// functions of the relic sequence's `(kind, is_melted)` pairs (the plan also
/// of the frozen relic channels), and those pairs are constant for the whole
/// combat: production writes them only when a combat state is built. So every
/// structural write goes through a method here that rebuilds them (`push`,
/// `extend`, `set_melted`, `From<Vec<Relic>>`, …), and the read side derefs to
/// `[Relic]`, not `Vec`. `counter` stays freely mutable through `DerefMut`
/// (`iter_mut`, indexing). The same `&mut [Relic]` could also write `kind` /
/// `is_melted` around the rebuild; nothing does, and debug builds recompute
/// both views at every [`apply`] entry and assert they still match
/// (`RelicStore::debug_assert_derived_views`).
///
/// [`PartialEq`] and [`Debug`](std::fmt::Debug) look at the relics only (the
/// views are derived), so equality and the printed form are those of the old
/// `Vec<Relic>`.
#[derive(Clone)]
pub struct RelicStore {
    relics: Vec<Relic>,
    owned: RelicKindSet,
    plan: relic_effects::channels::RelicPlan,
}

impl PartialEq for RelicStore {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.relics == other.relics
    }
}
impl Eq for RelicStore {}

impl std::fmt::Debug for RelicStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.relics, f)
    }
}

impl Default for RelicStore {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl core::ops::Deref for RelicStore {
    type Target = [Relic];
    #[inline]
    fn deref(&self) -> &[Relic] {
        &self.relics
    }
}
impl core::ops::DerefMut for RelicStore {
    #[inline]
    fn deref_mut(&mut self) -> &mut [Relic] {
        // Element mutation only (counters). No structural op is reachable
        // through a slice; a `kind` / `is_melted` write through it is caught
        // by the debug drift assert at `apply` entry.
        &mut self.relics
    }
}

impl RelicStore {
    /// Empty store.
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            relics: Vec::new(),
            owned: RelicKindSet::EMPTY,
            plan: relic_effects::channels::RelicPlan::EMPTY_STORE,
        }
    }

    /// Recompute both derived views from the relic sequence.
    fn rebuild(&mut self) {
        self.owned = RelicKindSet::owned_by(&self.relics);
        self.plan = relic_effects::channels::RelicPlan::build(&self.relics, &self.owned);
    }

    /// Append a relic (obtain order).
    #[inline]
    pub fn push(&mut self, relic: Relic) {
        self.relics.push(relic);
        self.rebuild();
    }

    /// Insert a relic at `index`, shifting later relics.
    pub fn insert(&mut self, index: usize, relic: Relic) {
        self.relics.insert(index, relic);
        self.rebuild();
    }

    /// Remove and return the relic at `index`.
    pub fn remove(&mut self, index: usize) -> Relic {
        let relic = self.relics.remove(index);
        self.rebuild();
        relic
    }

    /// Remove and return the last relic.
    pub fn pop(&mut self) -> Option<Relic> {
        let relic = self.relics.pop();
        self.rebuild();
        relic
    }

    /// Keep only the relics matching `f`.
    pub fn retain<F: FnMut(&Relic) -> bool>(&mut self, f: F) {
        self.relics.retain(f);
        self.rebuild();
    }

    /// Drop every relic.
    pub fn clear(&mut self) {
        self.relics.clear();
        self.rebuild();
    }

    /// Set the ToyBox melt flag of the relic at `index` (C# `IsMelted`: a
    /// melted relic keeps its slot but leaves every hook dispatch).
    ///
    /// # Panics
    /// If `index` is out of bounds.
    pub fn set_melted(&mut self, index: usize, melted: bool) {
        self.relics[index].is_melted = melted;
        self.rebuild();
    }

    /// Kinds of the non-melted relics (the `player_owns_relic` predicate as
    /// a set).
    #[inline]
    #[must_use]
    pub fn owned_kinds(&self) -> &RelicKindSet {
        &self.owned
    }

    /// True iff a non-melted relic of `kind` is in the store: the
    /// `relics.iter().any(|r| !r.is_melted && r.kind == kind)` ownership scan,
    /// answered by one bit test on the stored owned-kind set (J01b). Debug
    /// builds also run the scan and assert the two agree.
    ///
    /// # Panics
    /// In debug builds, if the stored set disagrees with the scan (a `kind` /
    /// `is_melted` write bypassed the store's methods).
    #[inline]
    #[must_use]
    pub fn owns(&self, kind: RelicKind) -> bool {
        let owned = self.owned.contains(kind);
        #[cfg(debug_assertions)]
        assert_eq!(
            owned,
            self.relics.iter().any(|r| !r.is_melted && r.kind == kind),
            "RelicStore owned-kind set disagrees with the relic scan for {kind:?}"
        );
        owned
    }

    /// The per-combat relic-channel plan.
    #[inline]
    pub(crate) fn plan(&self) -> &relic_effects::channels::RelicPlan {
        &self.plan
    }

    /// Build the plan if this store was built before the relic channels were
    /// frozen (it then has none and every planned dispatcher takes its
    /// generic path). Called at every [`apply`] entry; a single flag test once
    /// the plan exists.
    #[inline]
    pub fn ensure_plan(&mut self) {
        if !self.plan.is_built() {
            self.plan = relic_effects::channels::RelicPlan::build(&self.relics, &self.owned);
        }
    }

    /// Debug drift guard: both derived views still equal a recompute from the
    /// relic sequence. Fires if a `kind` / `is_melted` write bypassed the
    /// store's methods through `&mut [Relic]`.
    ///
    /// # Panics
    /// If either view drifted.
    #[cfg(debug_assertions)]
    pub fn debug_assert_derived_views(&self) {
        let owned = RelicKindSet::owned_by(&self.relics);
        assert_eq!(
            self.owned, owned,
            "RelicStore owned-kind set drifted from the relics: a `kind` / `is_melted` \
             write went through `&mut [Relic]` instead of a RelicStore method"
        );
        if self.plan.is_built() {
            assert_eq!(
                self.plan,
                relic_effects::channels::RelicPlan::build(&self.relics, &owned),
                "RelicStore relic-channel plan drifted from the relics"
            );
        }
    }

    /// Test hook: discard the plan so every planned dispatcher takes its
    /// generic path (until the next `apply` rebuilds it).
    #[cfg(test)]
    pub(crate) fn drop_plan(&mut self) {
        self.plan = relic_effects::channels::RelicPlan::UNBUILT;
    }

    /// Borrow the relics as a slice (explicit form of the `Deref`).
    #[inline]
    #[must_use]
    pub fn as_slice(&self) -> &[Relic] {
        &self.relics
    }

    /// Unwrap into the relic `Vec`.
    #[inline]
    #[must_use]
    pub fn into_vec(self) -> Vec<Relic> {
        self.relics
    }
}

impl From<Vec<Relic>> for RelicStore {
    #[inline]
    fn from(relics: Vec<Relic>) -> Self {
        let mut store = Self {
            relics,
            owned: RelicKindSet::EMPTY,
            plan: relic_effects::channels::RelicPlan::EMPTY_STORE,
        };
        store.rebuild();
        store
    }
}

impl<const N: usize> From<[Relic; N]> for RelicStore {
    #[inline]
    fn from(relics: [Relic; N]) -> Self {
        Self::from(Vec::from(relics))
    }
}

impl FromIterator<Relic> for RelicStore {
    fn from_iter<I: IntoIterator<Item = Relic>>(iter: I) -> Self {
        Self::from(iter.into_iter().collect::<Vec<Relic>>())
    }
}

impl Extend<Relic> for RelicStore {
    fn extend<I: IntoIterator<Item = Relic>>(&mut self, iter: I) {
        self.relics.extend(iter);
        self.rebuild();
    }
}

impl IntoIterator for RelicStore {
    type Item = Relic;
    type IntoIter = std::vec::IntoIter<Relic>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.relics.into_iter()
    }
}

impl<'a> IntoIterator for &'a RelicStore {
    type Item = &'a Relic;
    type IntoIter = core::slice::Iter<'a, Relic>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.relics.iter()
    }
}

impl<'a> IntoIterator for &'a mut RelicStore {
    type Item = &'a mut Relic;
    type IntoIter = core::slice::IterMut<'a, Relic>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.relics.iter_mut()
    }
}

/// Vector-backed arena over [`Potion`].
pub type PotionStore = Vec<Potion>;

// ---------------------------------------------------------------------------
// CreatureKind — Player / Monster discriminator (bead `sts2-rs-d03` Phase 1).
//
// Mirrors the C# `Player? / Monster?` discriminator on
// `MegaCrit.Sts2.Core.Entities.Creatures.Creature` at pin v0.103.2
// (`sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Entities.Creatures/
// Creature.cs`). The Python pin (`sts2-rl-agent@01c094d:sts2_env/core/
// creature.py`) does not name the discriminator — it relies on the
// `combat_player_states` / `enemies` split on `CombatState`. The Rust
// engine grows a top-level `CreatureKind` so the future call-site
// migration (Phase 2 of bead `sts2-rs-d03`) can pattern-match without
// a back-reference to the [`CombatState`] arena.
//
// Phase 1 keeps this enum **descriptive only** — there is no struct
// field on [`Creature`] yet (adding one would churn ~75 literal
// construction sites which is decomposition-phase scope). The
// discriminator is computed via
// [`CombatState::creature_kind`] which compares against
// [`PlayerCombatState::creature_uid`].
// ---------------------------------------------------------------------------

/// Discriminator for the two creature flavors in a combat state.
///
/// Phase 1 (bead `sts2-rs-d03`) introduces the enum and the
/// [`CombatState::creature_kind`] lookup; Phase 2 will move the shared
/// combat fields (`current_hp`, `max_hp`, `block`, `powers`) off
/// [`PlayerCombatState`] onto [`Creature`] proper and migrate the 30-50
/// direct field accesses. See the bead description for the canonical
/// pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CreatureKind {
    /// The primary player (or an ally). Distinguished from monsters by
    /// the presence of a [`PlayerCombatState`] backing record.
    Player,
    /// A hostile creature (i.e. an enemy). Species discriminant lives
    /// in [`Creature::kind`] ([`MonsterKind`]).
    Monster,
}

// ---------------------------------------------------------------------------
// Player-side state
// ---------------------------------------------------------------------------

/// Per-player combat-scoped state (player or each ally).
///
/// Pile inline-sizes are tuned for typical mid-combat sizes per the plan
/// (<5μs clone target).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PlayerCombatState {
    pub hand: SmallVec<[CardUid; 16]>,
    pub draw: SmallVec<[CardUid; 32]>,
    pub discard: SmallVec<[CardUid; 32]>,
    pub exhaust: SmallVec<[CardUid; 32]>,
    pub play: SmallVec<[CardUid; 8]>,

    /// UIDs whose draw-pile position is "pinned" to the top by an effect
    /// (Headbutt-style: insert-at-top resolvers). Index `i` corresponds to
    /// `Top(i)` — `pinned_top[0]` is the very next card to be drawn.
    /// A subsequent pin pushes prior pins down (`insert(0, new)`), so
    /// the most recently pinned card always wins the top slot.
    ///
    /// Bead `sts2-rs-o6xw`. Consumed by [`crate::cards::shuffle`] to
    /// preserve known-position cards across MCTS PIMC determinization
    /// re-shuffles. Cleared per-uid when the card is drawn, and cleared
    /// wholesale when `discard_pile_into_draw_pile` reshuffles (a true
    /// reshuffle event scrambles all knowledge of position).
    pub pinned_top: SmallVec<[CardUid; 4]>,

    /// Symmetric to [`Self::pinned_top`] for "bottom-of-draw" pins
    /// (Forethought-style). Index `i` corresponds to `Bottom(i)` —
    /// `pinned_bottom[0]` is the very last card to be drawn.
    /// Bead `sts2-rs-o6xw`. Phase 1 ships the storage + shuffle logic;
    /// no STS2 card in the pinned `01c094d` Python source actually pins
    /// bottom-of-draw (Forethought is STS1-only and absent from the pin),
    /// so the field is exercised by unit tests only at this revision.
    pub pinned_bottom: SmallVec<[CardUid; 4]>,

    /// Search-only flag (bead `sts2-rs-09x5n`, F2 lightspeed-port
    /// `sts2_mcts::HiddenInfoMode::BeliefDraw`): when `true`, the unknown
    /// middle segment of [`Self::draw`] (see
    /// [`crate::cards::unknown_middle_uids`] — everything NOT covered by
    /// [`Self::pinned_top`]/[`Self::pinned_bottom`]) is a CANONICAL
    /// (uid-ascending sorted) belief-state multiset rather than a concrete
    /// materialized order. [`crate::cards::draw`] /
    /// [`crate::cards::draw_signaling`] defer the random CHOICE of which
    /// card is drawn next to draw time (consuming RNG then, instead of at
    /// the last shuffle), and every order-dissolving mutation
    /// (auto-reshuffle, [`crate::cards::shuffle`], the gh#181
    /// random-position `Shuffle`-stream insert family) re-sorts the
    /// segment instead of physically permuting it, consuming no RNG.
    ///
    /// Set ONLY on a private root-state clone inside
    /// `sts2_mcts::Mcts::with_policy` under `HiddenInfoMode::BeliefDraw`
    /// (see [`crate::blindfold::belief_root`]) and propagated through
    /// ordinary `Clone`s for the rest of that search. No env-visible engine
    /// path (parity harness, `run_arena`, workbench) ever sets it — every
    /// pre-bead / real-game state stays at the `#[derive(Default)]` value
    /// `false`, which is byte-identical to the historical
    /// materialized-order behavior in every pile function this flag gates.
    ///
    /// Deliberately excluded from every state/search hash
    /// (`sts2_core::hashing`) — it is search machinery, not observable game
    /// state, and folding it in would needlessly split the transposition
    /// table between otherwise-identical positions.
    pub deferred_draw_order: bool,

    pub energy: i8,
    pub max_energy: i8,
    pub stars: i8,

    pub current_hp: i32,
    pub max_hp: i32,
    pub block: i32,

    pub powers: SmallVec<[PowerUid; 8]>,

    /// Character identity for character-locked potion / card gating.
    /// `None` means "no gating" — the engine accepts every potion
    /// kind (used by fixtures / tests that don't care about
    /// character locks). Real runs set this at deck-build time.
    /// Bead sts2-rs-13q (P7-6).
    pub character: Option<CharacterId>,

    /// Potion-slot inventory. Bead sts2-rs-13q (P7-6) adds this field;
    /// inline capacity matches [`MAX_POTIONS`] = 5. Mirrors the
    /// `creature.potions: list[PotionInstance]` field on Python's
    /// `Creature` (`sts2_env/core/creature.py`) — but only `PotionKind`
    /// is stored here because there is no per-potion runtime state
    /// (charges, modifiers) at pin `01c094d`. The legacy
    /// `CombatState::potions: PotionStore` arena (carrying
    /// `Potion { id }` POD handles) remains untouched for parity
    /// hashing and is populated in lock-step by the runtime when
    /// integration plumbing lands.
    pub potions: SmallVec<[PotionKind; MAX_POTIONS]>,

    /// Physical belt-slot index of each entry in [`Self::potions`],
    /// parallel by position. C# `Player._potionSlots` is a fixed-size
    /// `List<PotionModel?>`: drinking a potion leaves a `null` HOLE
    /// (`RemovePotionInternal`), the next add fills the FIRST hole
    /// (`AddPotionInternal`: `slotIndex = _potionSlots.IndexOf(null)`),
    /// and the serialized `Potions` view yields non-null slots in SLOT
    /// order — so after drink-slot-0 + new drop, the new potion lists
    /// FIRST, not last (shadow census v8 seeds
    /// 1012/1013/1028/1035/1044/1051, belt pair reversed). Maintained by
    /// [`Self::add_potion_first_free_slot`] / [`Self::remove_potion_at`];
    /// fixtures that push `potions` directly are re-synced to the compact
    /// 0..len mapping on the next helper call.
    pub potion_slot_indices: SmallVec<[u8; MAX_POTIONS]>,

    /// Potion-slot *cap* (C# `Player.MaxPotionCount` = `_potionSlots.Count`).
    /// This is the number of belt slots — the ceiling that slot-filling
    /// potions (EntropicBrew, `Alchemize`, DelicateFrond) fill up to via
    /// [`crate::CombatState::fill_empty_potion_slots`]. C#
    /// `Player.initialMaxPotionSlotCount == 3`
    /// (`MegaCrit.Sts2.Core.Entities.Players/Player.cs:30`), and the
    /// headless harness `set_player` does NOT resize `_potionSlots`
    /// (`RunSimulator.cs:471-498` only clears + refills the existing
    /// slots), so a parity fixture's player has exactly 3 slots. `0` is
    /// a sentinel meaning "unset" and falls back to the hard
    /// [`MAX_POTIONS`](crate::potions::MAX_POTIONS) (= 5) for legacy
    /// fixtures that construct via `..Default::default()` without
    /// seeding a run-realistic cap; real-run / scenario builders set
    /// this to the run's `max_potion_slots` (default 3). Bead
    /// `sts2-rs-eysmt`: before this field, `fill_empty_potion_slots`
    /// collapsed to `MAX_POTIONS`, so EntropicBrew grew a 3-potion belt
    /// to 5 and surfaced `use_potion` slots 3/4 that C# never emits.
    pub potion_slots: u8,

    /// Defect's orb collection — Channel/Evoke/passive-tick state.
    /// Default is an empty 0-capacity belt; Defect decks call
    /// `OrbSlots::with_default_capacity()` at deck-build, and the
    /// `orbs::channel_orb` helper auto-grows a zero-capacity belt to
    /// the canonical 3 slots on first channel for fixture-friendly
    /// callers. Bead sts2-rs-spy (P7-7).
    pub orbs: OrbSlots,

    /// Handle into [`CombatState::creatures`] pointing to the player's
    /// [`Creature`] record, or `None` when the unified Creature has
    /// not yet been pushed for this combat (canonical default for
    /// fixtures that bypass [`CombatState::init_player_creature`]).
    ///
    /// Bead `sts2-rs-d03` — Creature unification. Phase 1 landed the
    /// accessor router (`player_hp` / `set_player_block` /
    /// `player_powers_mut` / …) which routes reads and writes through
    /// `state.creatures[uid]` when this is `Some`, falling back to the
    /// legacy `PlayerCombatState` fields otherwise. Phase 2 (bead
    /// `sts2-rs-daw`) was effectively subsumed by Phase 1's broader
    /// migration. Phase 3 (bead `sts2-rs-wo33`) proved player-targeting
    /// relic listeners fire via this uid through
    /// [`CombatState::creature_gain_max_hp`] /
    /// [`CombatState::creature_lose_max_hp`] (no player-skip guard
    /// remains — the panic on those methods is generic
    /// uid-not-found, not a player-side check).
    pub creature_uid: Option<CreatureUid>,

    /// Handle into [`CombatState::creatures`] pointing to this player's
    /// Osty pet, or `None` when no Osty has been summoned this combat
    /// (the canonical default for every non-Necrobinder run). Bead
    /// `sts2-rs-s87y` (BoneBrew + minimal `summon_osty` surface).
    ///
    /// Mirrors Python `CombatState.osty` /
    /// `CombatState.get_osty(player)` at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:1846-1860`. The
    /// Python pin tracks Osty via `Creature.is_osty` +
    /// `Creature.pet_owner` flags on the ally; the Rust port stores the
    /// pointer player-side so reads don't need to scan
    /// [`CombatState::creatures`]. The `is_osty` discriminant lives on
    /// the spawned Creature as `species == Some(MonsterKind::Osty)`.
    ///
    /// `None` invariants: (a) no `summon_osty` call has fired this
    /// combat, OR (b) the Osty was reaped between combats (the pet
    /// uid does not persist across `combat_setup`). The handle can
    /// remain `Some` while the pointed-to Creature has
    /// `current_hp == 0` — this is the "Osty died but can be revived"
    /// state Python's `summon_osty` recognises via the `is_reviving`
    /// branch.
    pub pet_uid: Option<CreatureUid>,

    /// Number of Shiv-tagged cards the player has played this turn.
    /// Reset to 0 at the start of the player's side-turn. Used by
    /// [`PowerKind::PhantomBlades`] to detect "first Shiv played this
    /// turn" (Python `_shiv_played_this_turn` instance flag at
    /// `sts2-rl-agent@01c094d:sts2_env/powers/card_play_effects.py:364-413`).
    /// Bead `sts2-rs-zlx.3`.
    pub shivs_played_this_turn: u32,

    /// Number of `CardType::Skill` cards the player has played this
    /// turn. Reset to 0 at the start of the player's side-turn.
    ///
    /// **Do not read this from card/power logic.** It is a forward-only
    /// duplicate of `cards_played_this_turn[CardType::Skill]` and, unlike
    /// that bucket, it does NOT cross `state_to_dict` / `state_from_dict` —
    /// both PyO3 constructors init it to 0. Any consumer that reads it sees
    /// 0 forever on a state rebuilt from a wire, which is exactly how the
    /// MakeItSo late-effect (bead `sts2-rs-j9x8`) stopped returning its card
    /// to hand on the live Regent arm. Use
    /// [`CombatState::count_player_cards_played_this_turn`] instead; it is
    /// the same quantity C# counts off `History.CardPlaysFinished`.
    /// Retained only because `invariants.rs` asserts its per-turn reset.
    pub skills_played_this_turn: u32,

    /// Number of card plays STARTED this turn — the analogue of C#'s
    /// `History.CardPlaysStarted … HappenedThisTurn` count. Differs from
    /// summing [`Self::cards_played_this_turn`] exactly while a play is in
    /// flight: the per-turn buckets tally at play END (after the effect
    /// dispatch), so an auto-play resolved INSIDE a parent card's effect
    /// (Cascade/Havoc gathering) does not yet see the parent there.
    /// `RingingPower.ShouldPlay` (CardPlaysStarted scan) and
    /// `SlothPower.ShouldPlay` (`BeforeCardPlayed` counter) both count
    /// started plays, so the in-flight parent must block its own
    /// auto-plays (sweep seed 3931: a Ringing-blocked Cascade gather
    /// actually played in Rust). Bumped at play start in `play_card`,
    /// `resolve_auto_played_card`, the replay loops, and the HistoryCourse
    /// dupe; reset with the per-turn buckets at player-turn start.
    pub card_plays_started_this_turn: u32,

    /// Remaining `EchoForm` card-doublings available this turn. Reset
    /// at the start of the player's side-turn to the sum of `EchoForm`
    /// power stacks (Python uses a per-instance
    /// Count of FIRST-IN-SERIES card plays the player has made this turn
    /// (reset to 0 at side-turn-start; incremented once per original
    /// [`crate::combat_loop::play_card`] — NOT per EchoForm/OneTwoPunch
    /// replay). Feeds the EchoForm echo decision: C#
    /// `EchoFormPower.ModifyCardPlayCount` (`EchoFormPower.cs:18-27`,
    /// v0.107.1) returns `playCount + 1` while the count of first-in-series
    /// plays this turn is `< Amount`, so the first `Amount` distinct cards
    /// each get exactly ONE extra play. Reading the LIVE EchoForm amount per
    /// play (captured at play-start, before the card's OnPlay) means an
    /// EchoForm gained mid-turn correctly echoes the cards played after it.
    /// (Was `echo_form_remaining`, a turn-start budget snapshot decremented
    /// per replay — bead `sts2-rs-8ett`. That model both missed mid-turn
    /// gains and let a single card drain the whole budget: Defect sweep
    /// `ORB_STATE|HOTFIX`/`ORB_STATE|CHILL`/`PLAYER_STATE|THE_BOMB`.)
    pub cards_first_in_series_this_turn: u32,

    /// `VoidForm`-style "end this turn after the current play finishes"
    /// rider. Set by `combat.request_end_turn_after_current_play()` in
    /// Python (`sts2-rl-agent@01c094d:sts2_env/core/combat.py:2091`);
    /// checked at the bottom of [`crate::combat_loop::play_card`] —
    /// once the played card has fully resolved (effect dispatch +
    /// replays + after-play buses + late-effects + Play→Discard/Exhaust
    /// move), the flag is cleared and the engine advances to
    /// `PlayerEndTurnPhaseOne` so the rest of the player's turn is
    /// skipped. Mirrors Python's `_end_turn_after_play` (combat.py:127)
    /// + checked at `combat.py:909-911`. Reset to `false` at the start
    /// of each player turn alongside the other per-turn counters. Bead
    /// `sts2-rs-3guy` (filed from `sts2-rs-ozy.4` VoidForm worktree).
    pub request_end_turn_after_play: bool,

    /// Cards in hand explicitly flagged for end-of-turn retain
    /// ("single-turn retain"). Mirrors Python's
    /// `CardInstance.single_turn_retain: bool` (per `sts2-rl-agent@01c094d:
    /// sts2_env/cards/base.py:36`) — a side-table here keeps the `Card`
    /// POD untouched (~25 constructor sites unchanged). Cleared at
    /// end-of-turn after the flush. Bead `sts2-rs-sah` (G3 plumbing).
    pub retain_this_turn: SmallVec<[CardUid; 8]>,

    /// Cards in hand explicitly flagged with "single-turn Sly" (C#
    /// `CardModel.GiveSingleTurnSly` / `HasSingleTurnSly`). HandTrick marks
    /// one Skill this way; the keyword is consulted (alongside the permanent
    /// `Sly` keyword) when a card is actively discarded — a Sly card
    /// auto-plays from wherever it lands. Side-table keeps the `Card` POD
    /// untouched, mirroring `retain_this_turn`. Cleared at end-of-turn after
    /// the flush. Bead `sts2-rs-handtrick-sly`.
    pub sly_this_turn: SmallVec<[CardUid; 8]>,

    /// Card instances permanently flagged `Sly` for the REST OF THE COMBAT
    /// (C# `CardCmd.ApplyKeyword(card, Sly)` → `CardModel.AddKeyword`, which
    /// adds to the card's `_keywords` set and is NOT reset per turn — distinct
    /// from [`Self::sly_this_turn`]'s single-turn `HasSingleTurnSly`).
    /// `MasterPlannerPower.AfterCardPlayed` marks every Skill the player plays
    /// this way, so a later active discard of that instance (Prepared /
    /// CalculatedGamble / GamblersBrew / …) auto-plays it. Side-table keyed by
    /// uid; cleared only at `start_combat`. Bead `sts2-rs-masterplanner-sly`.
    pub sly_this_combat: SmallVec<[CardUid; 8]>,

    /// Per-turn retain budget granted by stack-based powers
    /// (`WellLaidPlans`, `RetainHand`). Mirrors Python's
    /// `CombatState._pending_retain_count[player_id]` at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:1357-1360`. Multiple
    /// power instances accumulate additively. Consumed (and zeroed) by
    /// the end-of-turn retain-compute step. Bead `sts2-rs-sah`.
    pub pending_retain_count: u32,

    /// Per-turn cards-played counter, bucketed by [`CardType`] discriminant
    /// (`Attack=0`, `Skill=1`, `Power=2`, `Curse=3`, `Status=4` — see
    /// [`CardType::index`]). Reset to zero at the start of the player's
    /// side-turn. Mirrors Python's `CombatState._played_cards_this_turn:
    /// list[CardInstance]` at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:140` (appended once
    /// per `play_card_effect` invocation at lines 886/931, including each
    /// EchoForm replay; cleared at `_start_player_turn` line 678 and at
    /// combat start line 648). Queried by
    /// [`CombatState::count_player_cards_played_this_turn`] which is the
    /// Rust analogue of `combat.count_cards_played_this_turn(owner,
    /// card_type=...)` at `combat.py:1985`. Used by content like
    /// `LunarBlast` (hits = skills played this turn) and `Regent`'s
    /// skill-watch listeners. Bead `sts2-rs-r6l9`
    /// (zlx-countplayed: per-turn cards-played counter on `CombatState`).
    pub cards_played_this_turn: [u32; 5],

    /// Total Stars *gained* by the player this turn (sum of amounts —
    /// a single `gain_stars(3)` adds 3, not 1). Reset to zero at the
    /// start of the player's side-turn. Spending Stars (e.g. Stardust
    /// zeroing `player.stars`) does NOT decrement this counter — it is
    /// a one-way per-turn tally. Mirrors Python's
    /// `CombatState._stars_gained_this_turn: list[(Creature, int)]` at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:132` (appended
    /// inside `gain_stars` at `combat.py:1453`; cleared at combat
    /// start `combat.py:646` and at `_start_player_turn` `combat.py:677`),
    /// reduced over the player owner's entries by
    /// `combat.count_stars_gained_this_turn(owner)` at
    /// `combat.py:1550-1551`. Queried by
    /// [`CombatState::count_player_stars_gained_this_turn`] (the Rust
    /// analogue) and consumed by content like `Radiate`
    /// (hits = stars gained this turn — `cards/regent.py:L500-504`).
    /// Bead `sts2-rs-x646` (zlx-countstars: per-turn stars-gained
    /// counter on `CombatState`).
    pub stars_gained_this_turn: u32,

    /// Per-turn count of "powered" attack hits dealt by the player.
    /// A hit is "powered" when its [`hooks::ValueProps`] satisfy
    /// [`hooks::ValueProps::is_powered`] — i.e. the hit isn't tagged
    /// `UNPOWERED`, the same predicate Strength / Vulnerable / Weak
    /// listener arms gate on. Incremented in
    /// [`crate::damage::apply_damage`] when `dealer ==
    /// CreatureRef::Player` and `props.is_powered()`. Reset to zero
    /// at the start of the player's side-turn alongside the other
    /// per-turn counters. Mirrors Python's
    /// `CombatState._damage_events_this_turn` filter at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:1519-1531`
    /// (`count_powered_hits_this_turn` /
    /// `count_powered_hits_by_dealer_this_turn`); the Rust port
    /// keeps a player-totals scalar instead of replaying the full
    /// damage-event log because the only Phase-6 consumer —
    /// `BeatIntoShape` (`sts2-rl-agent@01c094d:sts2_env/cards/
    /// regent.py:599-607`) — only needs the player-as-dealer total.
    /// Read via
    /// [`CombatState::count_player_powered_hits_this_turn`].
    /// Bead `sts2-rs-jm6e` (ozy.4-rider: `count_powered_hits_this_turn`
    /// surface for BeatIntoShape).
    pub powered_hits_this_turn: u32,

    /// Per-target version of [`Self::powered_hits_this_turn`]: the count
    /// of powered player→enemy hits dealt **to each individual creature**
    /// this turn, keyed by receiver uid. C# `BeatIntoShape`'s
    /// `CalculatedForge` multiplier is strictly per-target —
    /// `History.Entries.OfType<DamageReceivedEntry>().Count(e =>
    /// e.Receiver == target && e.Dealer == card.Owner.Creature &&
    /// e.Result.Props.IsPoweredAttack() && e.HappenedThisTurn(...))`
    /// (`BeatIntoShape.cs:27-31`) — so the combat-wide scalar above
    /// over-counts in multi-enemy fights (hits the player landed on
    /// monster[0] inflate the forge when BeatIntoShape is played on
    /// monster[1]). This tally keys the count by receiver so the read is
    /// exact per target. Incremented alongside the scalar in
    /// [`crate::damage::apply_damage`] and reset at the start of the
    /// player's side-turn. Read via
    /// [`CombatState::count_player_powered_hits_on_target_this_turn`].
    /// Bead `sts2-rs-jm6e` (per-target follow-up for issue #177
    /// BeatIntoShape multi-enemy parity).
    pub powered_hits_by_target_this_turn: SmallVec<[(CreatureUid, u32); 4]>,

    /// Engine-private scratch: value of [`Self::powered_hits_this_turn`]
    /// captured at the START of the currently-resolving card play (set by
    /// `combat_loop::play_card` / `resolve_auto_played_card` before the on-play
    /// dispatch). The `after_card_played` Vigor / Gigantification consume gates
    /// on `powered_hits_this_turn > powered_hits_at_card_play_start` — i.e. the
    /// card ACTUALLY executed a powered attack — mirroring C#
    /// `VigorPower`/`GigantificationPower` latching on `BeforeAttack` rather
    /// than the card's type. Without it, an Attack-type card that deals zero
    /// hits (X-cost Whirlwind auto-played at X=0 via Cascade) wrongly consumed
    /// the buff (bead `sts2-rs-ph5mx`, seed 3057). Not serialized / not hashed
    /// (transient, always reset per card play). Bead `sts2-rs-ph5mx`.
    pub powered_hits_at_card_play_start: u32,

    /// Engine-private scratch: set true by an Attack-type card whose `OnPlay`
    /// conditionally issues **no** `AttackCommand` (currently only `PactsEnd`
    /// when its exhaust pile holds < 3 cards — `PactsEnd.cs:30-40` guards
    /// `DamageCmd.Attack(...).Execute()` behind `CanDealDamage`). C#
    /// `VigorPower`/`GigantificationPower` latch on `BeforeAttack` and consume
    /// on `AfterAttack`, both of which only fire when an `AttackCommand`
    /// actually executes — so a no-command Attack play must NOT consume the
    /// buff. The `after_card_played` consume gates on `!this` in addition to
    /// `is_player_attack`. Inverse of the reverted `ph5mx` powered-hit gate: a
    /// 0-HIT attack card (FlakCannon with no Status cards, X=0 Whirlwind) DOES
    /// still issue a command and still consumes, so those paths leave this flag
    /// false. Reset per card play (mirrors
    /// [`Self::powered_hits_at_card_play_start`]). Not serialized / not hashed.
    /// Bead `sts2-rs-pacts-no-command`.
    pub attack_command_suppressed_this_play: bool,

    /// Engine-private scratch: the card whose attack first consumed a live
    /// `VigorPower` this play — C# `VigorPower` latches `commandToModify` on
    /// the FIRST powered `AttackCommand` (`BeforeAttack`) and its
    /// `ModifyDamageAdditive` only boosts hits whose `cardSource` matches that
    /// command's card. We mirror that by recording the `play.last()` card after
    /// the first powered player hit; the Vigor additive arm then boosts only
    /// hits sourced from that latched card (`None` = not yet latched ⇒ apply,
    /// covering the first command and card-less attacks). Scopes Vigor to one
    /// attack command so a card that triggers a SECOND command in the same play
    /// (Uproar's auto-played follow-up attack) does NOT get the bonus. Cleared
    /// when Vigor is consumed (`after_turn`/`after_card_played`) and at
    /// side-turn start. Not serialized / not hashed (transient). Bead
    /// `sts2-rs-vigor-cmd`.
    pub vigor_latch_card: Option<CardUid>,

    /// Per-combat count of cards materialised into the player's hand via
    /// the "generated card" path ([`crate::card_effects::spawn_card_into_hand`]
    /// — colorless-modifier generation, `CollisionCourse` Debris,
    /// `Hotfix`, `Largesse`, etc). Reset to zero at the start of
    /// combat. Mirrors Python's
    /// `CombatState._generated_cards_combat: list[Creature]` at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:133` (appended
    /// inside `_add_generated_cards_to_hand` at `combat.py:2260` —
    /// unconditionally, *before* the hand-cap check, so overflow-to-discard
    /// still counts; cleared at `start_combat` `combat.py:647`).
    ///
    /// The Python list logs the owning [`Creature`] per generation so
    /// `count_generated_cards_this_combat(owner)` can per-owner filter;
    /// the Rust port keeps a player-only scalar because the only
    /// consumer in the pin — `Supermassive`
    /// (`sts2-rl-agent@01c094d:sts2_env/cards/regent.py:571-580`) —
    /// is always invoked with `owner == player`. Read via
    /// [`CombatState::count_player_generated_cards_this_combat`].
    /// Bead `sts2-rs-8h2q` (zlx-countgenerated).
    pub generated_cards_this_combat: u32,
}

impl PlayerCombatState {
    /// Effective potion-slot cap — the ceiling that slot-filling potions
    /// (EntropicBrew / `Alchemize` / DelicateFrond) fill up to. Resolves
    /// the [`Self::potion_slots`] field (C# `Player.MaxPotionCount`),
    /// treating the `0` sentinel ("unset", from `..Default::default()`
    /// fixtures) as the hard [`MAX_POTIONS`](crate::potions::MAX_POTIONS)
    /// fallback, and clamping any set value to that hard cap so the
    /// `SmallVec` inline storage is never exceeded. Bead `sts2-rs-eysmt`.
    #[must_use]
    pub fn effective_potion_slots(&self) -> usize {
        use crate::potions::MAX_POTIONS;
        if self.potion_slots == 0 {
            MAX_POTIONS
        } else {
            (self.potion_slots as usize).min(MAX_POTIONS)
        }
    }

    /// Re-derive [`Self::potion_slot_indices`] as the compact `0..len`
    /// mapping when it has fallen out of parallel with
    /// [`Self::potions`] — the fixture escape hatch for tests that push
    /// `potions` directly without slot bookkeeping.
    fn sync_potion_slot_indices(&mut self) {
        if self.potion_slot_indices.len() != self.potions.len() {
            self.potion_slot_indices = (0..self.potions.len() as u8).collect();
        }
    }

    /// Remove the potion at visible index `idx` from the belt, leaving
    /// its physical slot empty (C# `Player.RemovePotionInternal` nulls
    /// the slot — no compaction). The next
    /// [`Self::add_potion_first_free_slot`] refills the lowest hole.
    pub fn remove_potion_at(&mut self, idx: usize) -> PotionKind {
        self.sync_potion_slot_indices();
        self.potion_slot_indices.remove(idx);
        self.potions.remove(idx)
    }

    /// Add `kind` into the FIRST empty physical belt slot (C#
    /// `Player.AddPotionInternal`: `slotIndex = _potionSlots.IndexOf(null)`),
    /// keeping the visible `potions` list in slot order. Returns `false`
    /// on a full belt ([`Self::effective_potion_slots`] cap).
    pub fn add_potion_first_free_slot(&mut self, kind: PotionKind) -> bool {
        if self.potions.len() >= self.effective_potion_slots() {
            return false;
        }
        self.sync_potion_slot_indices();
        let mut slot: u8 = 0;
        while self.potion_slot_indices.contains(&slot) {
            slot += 1;
        }
        let pos = self
            .potion_slot_indices
            .iter()
            .position(|&s| s > slot)
            .unwrap_or(self.potions.len());
        self.potions.insert(pos, kind);
        self.potion_slot_indices.insert(pos, slot);
        true
    }
}

// ---------------------------------------------------------------------------
// Phase / side enums
// ---------------------------------------------------------------------------

/// Explicit combat phase enum (replaces Python's implicit boolean flags).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CombatPhase {
    Setup,
    PlayerTurnStart,
    PlayerPlayPhase,
    PlayerEndTurnPhaseOne,
    PlayerEndTurnPhaseTwo,
    EnemyTurnStart,
    EnemyTurnExec,
    EnemyTurnEnd,
    Resolved { player_won: bool },
}

/// Which side currently "has" the turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Player,
    Enemy,
}

// ---------------------------------------------------------------------------
// Pending state placeholders (Phase 6 fills these in)
// ---------------------------------------------------------------------------

/// Kind of pending combat choice. Mirrors the Python `STATE_TYPE_*`
/// dispatch in `bridge_replay.combat_state_to_bridge_state`
/// (`sts2_env/parity/bridge_replay.py:303-316`) — at this pin only the
/// `CardSelect` variant has bridge serialization wired (sts2-rs-cy8),
/// but we name the full set of pending-choice flavors so downstream
/// beads (sts2-rs-cqm wiring; OrbSlot/EnemyTarget surfaces) can pattern
/// match without churn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ChoiceKind {
    /// Pick N cards from a list (Apparition, Forethought, Gambling
    /// Chip, card-reward, etc.). Default variant — matches the
    /// `_placeholder` semantics of the prior shape.
    #[default]
    CardSelect,
    /// Put-back-into-deck flavor (e.g. Headbutt's "put a discard card
    /// on top of the draw pile").
    PutBack,
    /// Choose a card to exhaust (e.g. Exhume, Sanctity).
    ExhaustPile,
    /// Choose an orb slot to evoke / channel into.
    OrbSlot,
    /// Choose an enemy target (typically when a card needs an explicit
    /// target outside `Action::PlayCard`'s normal `target_index` slot).
    EnemyTarget,
    /// Pick one synthetic (generated, not in any pile) card and add it
    /// to the player's hand. Each option's `card_id` + `upgraded` flag
    /// describes a fresh card to allocate at resolve time. Used by
    /// Toolbox (3 colorless).
    AddGeneratedToHand,
    /// Same as [`Self::AddGeneratedToHand`] but the resolved card is
    /// tagged for retention this turn via `player.retain_this_turn`.
    /// Used by ChoicesParadox (5 character cards, retain).
    AddGeneratedRetainedToHand,
    /// Same as [`Self::AddGeneratedToHand`] but the resolved card is
    /// stamped free-this-turn (`VarKey::FreeThisTurn`), mirroring C#
    /// `CardModel.SetToFreeThisTurn()`. Used by AttackPotion /
    /// SkillPotion / PowerPotion / ColorlessPotion (generate 3 distinct,
    /// choose 1, the chosen card is free for the turn). Bead
    /// `sts2-rs-91tc1`.
    AddGeneratedFreeToHand,
    /// Pick exactly N master-deck cards to stash into
    /// [`crate::RunState::paels_tooth_stored`] for PaelsTooth's
    /// per-combat upgrade-and-return mechanic. Resolved via
    /// [`crate::RunState::resolve_paels_tooth_stash`]. Each option's
    /// `card_id` + `upgraded` pair identifies a master-deck entry
    /// (first-match semantics on duplicates). Bead `sts2-rs-n7tha`.
    PaelsToothStash,
    /// Pick one un-upgraded card in hand to upgrade for the rest of
    /// combat. Mirrors C# `Armaments.OnPlay` →
    /// `CardSelectCmd.FromHandForUpgrade` (base, un-upgraded Armaments).
    /// Each option's `card_uid` names the concrete hand card; resolution
    /// upgrades it in place via [`resolve_upgrade_in_hand`]. Bead
    /// `sts2-rs-8ukqd` (was the `sts2-rs-oac`-deferred pending-choice
    /// surface).
    UpgradeInHand,
    /// Pick one draw-pile card (pre-filtered to a single `CardType` at
    /// request time) and move it to hand. Mirrors C# `SecretTechnique`
    /// (Skill) / `SecretWeapon` (Attack) `CardSelectCmd.FromSimpleGrid`
    /// `prefs(1)`. Each option's `card_uid` names the concrete draw-pile
    /// card; resolution removes it from `draw` and pushes it onto `hand`
    /// via [`resolve_add_from_draw_by_type`]. Single-pick
    /// (`min=max=1`), so [`legal_actions`] enumerates one option per
    /// candidate and auto-resolve picks option 0. Bead `sts2-rs-1xmxp`
    /// (the four *multi*-select sibling stubs are blocked on the
    /// `sts2-rs-enwwr` multi-pick fan-out — see the audit doc).
    AddFromDrawByType,
    /// Pick one card from the *discard* pile, make it free this turn
    /// (or until played), and move it to hand. Mirrors C#
    /// `LiquidMemories.OnUse` (LiquidMemories.cs:25-30):
    /// `CardSelectCmd.FromSimpleGrid(PileType.Discard…, prefs(1))` →
    /// `SetToFreeThisTurn()` → `CardPileCmd.Add(.., Hand)`. Distinct
    /// from [`Self::AddFromDrawByType`] (which sources the *draw* pile and
    /// does NOT make the card free): the resolver
    /// [`resolve_add_from_discard_free`] removes the chosen `card_uid`
    /// from `discard`, stamps `VarKey::FreeThisTurn=1` (the canonical
    /// Rust analog of `SetToFreeThisTurn`, cleared by
    /// `combat_loop::clear_per_turn_card_vars`), and pushes it to hand
    /// honoring `HAND_LIMIT`. Single-pick (`min=max=1`), pile-order
    /// options (C# passes `…Cards` with no OrderBy). Bead
    /// `sts2-rs-gpsvo`.
    AddFromDiscardFree,
    /// Pick N cards in hand to discard. Mirrors C#
    /// `CardSelectCmd.FromHandForDiscard` — HiddenDaggers `prefs(Cards)`
    /// (discard exactly N) and GamblersBrew `prefs(0, ∞)` (discard any
    /// number). Each option's `card_uid` names a concrete hand card;
    /// resolution discards every selected card via
    /// [`resolve_discard_from_hand`]. Multi-select — the (now-closed)
    /// `sts2-rs-enwwr` fan-out enumerates the bounded selections.
    /// Bead `sts2-rs-1xmxp`.
    DiscardFromHand,
    /// Pick N cards in hand to exhaust. Mirrors C#
    /// `CardSelectCmd.FromHand` → `CardCmd.Exhaust` — Purity
    /// `prefs(0, Cards)` and Ashwater `prefs(0, ∞)` (exhaust any number).
    /// Each option's `card_uid` names a concrete hand card; resolution
    /// exhausts every selected card via
    /// [`crate::card_effects::exhaust_card_uid`] (fires the
    /// `on_card_exhausted` listeners) through
    /// [`resolve_exhaust_from_hand`]. Distinct from [`Self::ExhaustPile`]
    /// (which targets a non-hand pile, e.g. Exhume). Multi-select — the
    /// `sts2-rs-enwwr` fan-out enumerates the selections. Bead
    /// `sts2-rs-1xmxp`.
    ExhaustFromHand,
    /// Pick N cards in hand to discard, then draw N. Mirrors C#
    /// GamblersBrew `CardSelectCmd.FromHandForDiscard(prefs(0, ∞))` →
    /// `CardCmd.DiscardAndDraw(list, list.Count)`. Like
    /// [`Self::DiscardFromHand`] but the resolver
    /// ([`resolve_discard_from_hand_and_draw`]) additionally draws one
    /// card per discarded card (off the `combat_card`/shuffle RunRng
    /// stream). Multi-select — the `sts2-rs-enwwr` fan-out enumerates the
    /// selections. Bead `sts2-rs-1xmxp`.
    DiscardFromHandAndDraw,
    /// Pick one hand card and make it free (cost 0) for the rest of the
    /// combat. Mirrors C# TouchOfInsanity
    /// `CardSelectCmd.FromHand(prefs(1), filter: CostsEnergyOrStars)` →
    /// `SetToFreeThisCombat()`. Single-pick (`min=max=1`); options are
    /// pre-filtered to cost-bearing hand cards at request time.
    /// Resolution sets the chosen in-combat card's `cost` to 0 via
    /// [`resolve_make_free_in_hand`] — since `CombatState.cards` is
    /// per-combat scratch (rebuilt from the master deck each combat),
    /// a direct `cost = 0` IS "free this combat". Bead `sts2-rs-1xmxp`
    /// (closes the `sts2-rs-gbaft` follow-up — no new cost-override
    /// field was needed).
    MakeFreeInHand,
    /// KnowledgeDemon CURSE_OF_KNOWLEDGE: pick 1 of 2 curse cards, whose
    /// `OnChosen()` applies its *power* to the player (no card enters the
    /// deck). Option 0 is always `Disintegration`; option 1 is the
    /// counter-keyed alt (`MindRot`/`SlothStatus`/`WasteAway`). Single-pick
    /// (`min=max=1`, no skip). Resolved by [`resolve_monster_curse_select`],
    /// which maps the chosen `card_id` → `(PowerKind, amount)` via
    /// [`knowledge_demon_curse_power`] and applies it to the player. Mirrors
    /// C# `KnowledgeDemon.ChooseCurse` (the headless harness picks index 0).
    /// Bead `sts2-rs-ybtmu`.
    MonsterCurseSelect,
    /// Stratagem: on every shuffle, pick exactly `Amount` draw-pile
    /// cards and add them to hand. Mirrors C# `StratagemPower.AfterShuffle`
    /// (`StratagemPower.cs:20-33`): `CardSelectCmd.FromSimpleGrid` over the
    /// draw pile `orderby c.Rarity, c.Id` (C# `CardRarity` enum order, then
    /// `ModelId.Entry` ordinal — a STABLE sort, ties keep pile order) with
    /// `CardSelectorPrefs(prompt, Amount)` (`min = max = Amount`,
    /// `RequireManualConfirmation = false`), then `CardPileCmd.Add(item,
    /// Hand)` per pick (raw pile move — NO `AfterCardDrawn` riders; full
    /// hand redirects to discard). When the pile has `<= Amount` cards C#
    /// auto-takes all without opening a screen (`FromSimpleGrid`'s
    /// `cards.Count <= prefs.MinSelect` fast path) — mirrored at the
    /// dispatch site, so this kind only ever opens with
    /// `options.len() > Amount`. Multi-stack Stratagem merges (StackType
    /// Counter, NOT `IsInstanced`), so a single choice picks `Amount`
    /// cards. Resolved via [`resolve_add_from_draw_to_hand`]. Bead
    /// `sts2-rs-63mew`.
    AddFromDrawToHand,
    /// Pick `Amount` cards from the *draw* pile (sorted `orderby c.Rarity,
    /// c.Id`) and add them to hand, then REMOVE the `ForegoneConclusionPower`.
    /// Mirrors C# `ForegoneConclusionPower.BeforeHandDraw`
    /// (ForegoneConclusionPower.cs:18-30): `ShuffleIfNecessary`, then
    /// `CardSelectCmd.FromSimpleGrid(PileType.Draw sorted by Rarity,Id,
    /// CardSelectorPrefs(prompt, Amount))` (`min = max = Amount`,
    /// `RequireManualConfirmation = false`) → `CardPileCmd.Add(item, Hand)`
    /// per pick, then `PowerCmd.Remove(this)`. The select surfaces a player
    /// decision when the draw pile has `> Amount` cards (C# `FromSimpleGrid`
    /// `cards.Count <= prefs.MinSelect` auto-take fast path is mirrored at
    /// the dispatch site, so this kind only ever opens with
    /// `options.len() > Amount`). Distinct from [`Self::AddFromDrawToHand`]
    /// (Stratagem) because the resolver must ALSO strip the foregone power(s)
    /// — C# `PowerCmd.Remove(this)` runs AFTER the select resolves, so the
    /// removal moves into the resolver here. Resolved via
    /// [`resolve_foregone_conclusion_from_draw`].
    ForegoneConclusionFromDraw,
    /// Pick one card from the *discard* pile and return it to hand at its
    /// normal cost (NOT free — unlike [`Self::AddFromDiscardFree`]). Mirrors
    /// C# `Graveblast.OnPlay` (Graveblast.cs:30-35):
    /// `CardSelectCmd.FromSimpleGrid(PileType.Discard…, prefs(1))` →
    /// `CardPileCmd.Add(.., Hand)` with no `SetToFreeThisTurn`. Single-pick
    /// (`min=max=1`), pile-order options (C# passes `…Cards` with no OrderBy);
    /// when the discard has `<= 1` card C# auto-takes it (`FromSimpleGrid`'s
    /// `cards.Count <= MinSelect` fast path), so this kind only ever opens
    /// with `options.len() > 1`. Resolved via
    /// [`resolve_add_from_discard_to_hand`]. Parity bead `parity-necrobinder`.
    AddFromDiscardToHand,
    /// Pick one NON-Retain card in hand and grant it the Retain keyword.
    /// Mirrors C# `Snap.OnPlay` (Snap.cs:38-43):
    /// `CardSelectCmd.FromHand(prefs(1), filter: !Keywords.Contains(Retain))`
    /// → `CardCmd.ApplyKeyword(card, Retain)` — a PLAYER CHOICE with ZERO RNG.
    /// Options are pre-filtered to non-Retain hand cards at request time. When
    /// `<= 1` candidate exists C# auto-applies (`FromHand`'s
    /// `cards.Count <= MinSelect` fast path), so this kind only opens with
    /// `options.len() > 1`. Single-pick (`min=max=1`). Resolved via
    /// [`resolve_apply_retain_in_hand`], which stamps the chosen in-combat
    /// card's per-instance `keyword_override` with `RETAIN`. Parity bead
    /// `parity-necrobinder`.
    ApplyRetainInHand,
    /// Pick one Skill in hand that is not already Sly-this-turn and grant it
    /// single-turn Sly. Mirrors C# `HandTrick.OnPlay` (HandTrick.cs:30-34):
    /// `CardSelectCmd.FromHand(prefs(1), filter: Type == Skill && !IsSlyThisTurn)`
    /// → `CardCmd.ApplySingleTurnSly(card)` — a PLAYER CHOICE with ZERO RNG.
    /// Options are pre-filtered at request time; when `<= 1` candidate exists
    /// C# auto-applies (`FromHand`'s `cards.Count <= MinSelect` fast path), so
    /// this kind only opens with `options.len() > 1`. Single-pick
    /// (`min=max=1`). Resolved via [`resolve_apply_sly_in_hand`], which records
    /// the chosen card's uid in `player.sly_this_turn`. Bead
    /// `sts2-rs-handtrick-sly`.
    ApplySlyInHand,
    /// Pick one NON-Ethereal card in hand and grant it the Ethereal keyword.
    /// Mirrors C# `SculptingStrike.OnPlay` (SculptingStrike.cs):
    /// `CardSelectCmd.FromHand(prefs(1), filter: !Keywords.Contains(Ethereal))`
    /// → `CardCmd.ApplyKeyword(card, Ethereal)` — a PLAYER CHOICE with ZERO RNG.
    /// Options are pre-filtered to non-Ethereal hand cards; when `<= 1`
    /// candidate exists C# auto-applies, so this kind only opens with
    /// `options.len() > 1`. Single-pick (`min=max=1`). Resolved via
    /// [`resolve_apply_ethereal_in_hand`]. Parity bead `parity-necrobinder`.
    ApplyEtherealInHand,
    /// Pick one card in hand and transform it into `MinionStrike` (upgraded
    /// iff the source card — Begone — is upgraded). Mirrors C# `Begone.OnPlay`
    /// (Begone.cs:21-31):
    /// `CardSelectCmd.FromHand(prefs(TransformSelectionPrompt, 1), filter: null)`
    /// → `CreateCard<MinionStrike>` (`+ Upgrade` when `IsUpgraded`) →
    /// `CardCmd.Transform(chosen, minionStrike)` — a PLAYER CHOICE with ZERO RNG.
    /// `filter: null` means the whole hand is eligible (Begone itself is already
    /// in the Play pile during its OnPlay, so it is never an option). When
    /// `<= 1` hand card exists C# auto-applies (`FromHand`'s
    /// `cards.Count <= MinSelect` fast path), so the lone card is transformed
    /// directly with no prompt and this kind only opens with `options.len() > 1`.
    /// Single-pick (`min=max=1`). The prior port auto-picked the first hand card,
    /// which desynced whole-run parity vs the C# headless agent's pick (seed RP3:
    /// C# entered `card_select` at the Begone play, Rust stayed in `combat`).
    /// Resolved via [`resolve_transform_in_hand_to_minion_strike`]. Bead
    /// `sts2-rs-begone-select`.
    TransformInHandToMinionStrike,
    /// Pick ANY number of hand cards (0..=hand size) and transform each into
    /// `MinionSacrifice` (upgraded iff the resolving card is upgraded). Mirrors
    /// C# `Guards.OnPlay` (Guards.cs:24-35):
    /// `CardSelectCmd.FromHand(prefs(SelectionScreenPrompt, 0, 999999999),
    /// filter: null)` → for each selected card `CardCmd.Transform(item,
    /// CreateCard<MinionSacrifice>)` (`+ Upgrade` when `base.IsUpgraded`) — a
    /// PLAYER CHOICE with ZERO RNG. Unlike the single-pick
    /// [`ChoiceKind::TransformInHandToMinionStrike`] (Begone, `min=max=1`), this
    /// is a multi-select with `min=0` (skippable). `min=0` means C#'s
    /// `cards.Count <= MinSelect` auto-apply fast path fires only on an EMPTY
    /// hand, so any non-empty hand surfaces the prompt (it never auto-applies a
    /// lone card the way Begone does). The prior port hardcoded "transform the
    /// FIRST hand card" inline, which desynced whole-run parity vs the C#
    /// headless agent's pick (seed PARITY110 A5: C# entered `card_select` at the
    /// Guards play, Rust stayed in `combat`). Resolved via
    /// [`resolve_transform_in_hand_to_minion_sacrifice`]. Bead
    /// `sts2-rs-guards-select`.
    TransformInHandToMinionSacrifice,
    /// Pick one card in hand; bump its cost +1 this combat (unless X-cost or
    /// already free below 0) and increment its `BaseReplayCount`. Mirrors C#
    /// `Transfigure.OnPlay` (Transfigure.cs:29-38):
    /// `CardSelectCmd.FromHand(prefs(SelectionScreenPrompt, 1), filter: null)`
    /// → for the chosen card `if (!EnergyCost.CostsX &&
    /// EnergyCost.GetWithModifiers(None) >= 0) EnergyCost.AddThisCombat(1)` and
    /// `BaseReplayCount++` (unconditional) — a PLAYER CHOICE with ZERO RNG. Same
    /// single-pick (`min=max=1`) family as Begone
    /// ([`ChoiceKind::TransformInHandToMinionStrike`]) and Guards. `filter: null`
    /// means the whole hand is eligible (Transfigure itself is already in the
    /// Play pile during its OnPlay, so it is never an option). When `<= 1` hand
    /// card exists C# auto-applies (`FromHand`'s `cards.Count <= MinSelect` fast
    /// path), so the lone card is mutated directly with no prompt and this kind
    /// only opens with `options.len() > 1`. The prior port auto-picked the first
    /// hand card, which desynced whole-run parity vs the C# headless agent's pick
    /// (seed PARITY125 Necrobinder A0: C# entered `card_select` at the Transfigure
    /// play, Rust stayed in `combat`). Resolved via
    /// [`resolve_transfigure_card_in_hand`].
    TransfigureCardInHand,
    /// Pick one card in your DRAW pile and transform it into a `Soul` token.
    /// Mirrors C# `Seance.OnPlay` (Seance.cs): the draw pile is sorted
    /// `orderby c.Rarity, c.Id`, then
    /// `CardSelectCmd.FromSimpleGrid(ctx, cardsIn, owner,
    /// prefs(TransformSelectionPrompt, Cards.IntValue=1))` surfaces a forced
    /// single-pick (`min=max=1`) over that sorted draw-pile grid, then
    /// `CardCmd.TransformTo<Soul>(chosen)`. Same single-pick transform family as
    /// Begone ([`ChoiceKind::TransformInHandToMinionStrike`]), Guards
    /// ([`ChoiceKind::TransformInHandToMinionSacrifice`]), and Transfigure
    /// ([`ChoiceKind::TransfigureCardInHand`]) — but the grid is over the DRAW
    /// pile (not the hand) and the pick is transformed into a **Soul** token.
    /// `FromSimpleGrid` auto-applies (NO prompt) only when
    /// `cards.Count <= MinSelect` (i.e. ≤1 draw card), so this kind only opens
    /// with 2+ draw cards; a lone/empty draw pile is handled inline by the card
    /// handler. The minted Soul is fresh and UN-upgraded (C# `TransformTo<Soul>`
    /// does not carry the source card's upgrade flag). Resolved via
    /// [`resolve_transform_draw_card_to_soul`] — a PLAYER CHOICE with ZERO RNG.
    TransformDrawCardToSoul,
    /// Pick TWO cards in your DRAW pile and transform each into a
    /// `MinionDiveBomb` token. Mirrors C# `Charge.OnPlay` (Charge.cs): the draw
    /// pile is sorted `orderby c.Rarity, c.Id`, then
    /// `CardSelectCmd.FromSimpleGrid(ctx, cardsIn, owner,
    /// prefs(TransformSelectionPrompt, Cards.IntValue=2))` surfaces a forced
    /// two-pick (`min=max=2`) over that sorted draw-pile grid; for each chosen
    /// card `CardCmd.TransformTo<MinionDiveBomb>(item)` (and `Upgrade` it iff
    /// Charge is upgraded). Same draw-pile transform family as Seance
    /// ([`ChoiceKind::TransformDrawCardToSoul`]) but with `Cards=2` and a
    /// `MinionDiveBomb` target. `FromSimpleGrid` auto-applies inline (NO prompt)
    /// only when `cards.Count <= MinSelect` (i.e. ≤2 draw cards), so this kind
    /// only opens with 3+ draw cards; a ≤2-card draw pile is handled inline by
    /// the card handler. The minted `MinionDiveBomb` is upgraded iff the
    /// resolving CHARGE is upgraded — that flag is re-derived from the Play-pile
    /// tail (the suspended CHARGE) at resolution time, NOT carried on the
    /// option. Resolved via [`resolve_transform_draw_cards_to_minion_dive_bomb`]
    /// — a PLAYER CHOICE with ZERO RNG.
    TransformDrawCardsToMinionDiveBomb,
    /// Pick one card in hand whose clone(s) Nightmare will put into your hand
    /// NEXT turn. Mirrors C# `Nightmare.OnPlay` (Nightmare.cs:39-43):
    /// `CardSelectCmd.FromHand(prefs(SelectionScreenPrompt, 1), filter: null)`
    /// → `PowerCmd.Apply<NightmarePower>(owner, 3).SetSelectedCard(chosen)`.
    /// `filter: null` means the whole hand is eligible (Nightmare itself is
    /// already in the Play pile during its OnPlay, so it is never an option).
    /// When `<= 1` hand card exists C# auto-applies (`FromHand`'s
    /// `cards.Count <= MinSelect` fast path), so this kind only opens with
    /// `options.len() > 1`. Single-pick (`min=max=1`). Resolution stashes the
    /// chosen `card_uid` into the just-applied `PowerKind::Nightmare`
    /// instance's `counter` (the Rust analog of `SetSelectedCard`), where the
    /// turn-start [`crate::hooks::dispatch_before_hand_draw_powers`] Nightmare
    /// arm reads it to emit `Amount` clones into hand before the draw (C#
    /// `NightmarePower.BeforeHandDraw`, NightmarePower.cs:35-47). Resolved via
    /// [`resolve_clone_chosen_card_next_turn`]. Bead `sts2-rs-nightmare-select`.
    CloneChosenCardNextTurn,
    /// End-of-turn Well-Laid Plans retain: pick up to `Amount` hand cards
    /// (cards NOT already retaining this turn) to retain for this turn.
    /// Mirrors C# `WellLaidPlansPower.BeforeFlushLate`
    /// (`WellLaidPlansPower.cs:19-39`):
    /// `CardSelectCmd.FromHand(prefs(prompt, 0, Amount), filter: !card.ShouldRetainThisTurn)`
    /// → `card.GiveSingleTurnRetain()` per chosen card — a PLAYER CHOICE with
    /// ZERO RNG, `min_select = 0` (OPTIONAL: the player may retain nothing,
    /// so `can_skip = true`). `Amount` is the summed `WellLaidPlansPower`
    /// counter (base 1, +1 per upgrade). Options are pre-filtered to hand
    /// cards not already in the turn's retain set at request time; when no
    /// eligible card exists C#'s `FromHand` returns an empty list and
    /// `BeforeFlushLate` early-returns, so this kind never opens with empty
    /// options. Surfaced from the end-of-turn flush path
    /// (`combat_loop::end_player_turn_phase_two`, before the hand flush);
    /// resolution stamps each chosen `card_uid` into `player.retain_this_turn`
    /// (the Rust analog of `GiveSingleTurnRetain`) and then resumes the
    /// suspended flush tail + the remainder of the `end_turn_dispatch` chain
    /// via [`CombatState::pending_wlp_retain_resume`]. Resolved via
    /// [`resolve_retain_at_turn_end`]. Bead `sts2-rs-wlp-retain-select`.
    RetainAtTurnEnd,
    /// Pick one Skill in hand and AUTO-PLAY it `Repeat` times. Mirrors C#
    /// `DecisionsDecisions.OnPlay` (DecisionsDecisions.cs:29-44): after
    /// `Draw(Cards)`, `CardSelectCmd.FromHand(prefs(prompt, 1),
    /// filter: c.Type == Skill && !Keywords.Contains(Unplayable))` surfaces a
    /// single-pick (`min = max = 1`, `RequireManualConfirmation = false`) over
    /// the Skill cards in hand, then `for i in 0..Repeat: CardCmd.AutoPlay(card,
    /// null)`. `Repeat` is the card's `RepeatVar(3)` — a fixed 3, NOT
    /// upgrade-scaled (only `CardsVar(3)` is bumped to 5 by `OnUpgrade
    /// UpgradeValueBy(2)`; `RepeatVar` is untouched). Per C# `FromHand`
    /// (`CardSelectCmd.cs:471-481`): `Count == 0` → no-op (no card to replay);
    /// `Count <= MinSelect(1)` → silently auto-apply the lone Skill (no prompt);
    /// else → surface this `card_select` phase. The prior port (`yc2d` stub)
    /// only ran the draw and returned `Resolved` without ever opening the
    /// select or auto-playing — desyncing whole-run parity (seed A9P153 Regent
    /// A9: C# entered `card_select` at the DecisionsDecisions play, Rust stayed
    /// in `combat`), the same recurring pick-first/inline-resolve class as
    /// Begone / Guards / Transfigure / Seance / Charge. Options are pre-filtered
    /// to playable hand Skills at request time; single-pick, so
    /// [`legal_actions`] enumerates one option per candidate and auto-resolve
    /// picks option 0. Resolved via [`resolve_auto_play_skill_from_hand`], which
    /// re-locates the chosen card uid across piles each repeat (hand on the 1st
    /// AutoPlay, then discard on the 2nd/3rd for non-Exhaust Skills) and
    /// auto-plays it — mirroring C# `CardCmd.AutoPlay` taking the `CardModel`
    /// reference directly and replaying it regardless of pile.
    AutoPlaySkillFromHand,
}

impl ChoiceKind {
    /// Every variant, for callers that need to enumerate the enum —
    /// notably `sts2-py`'s string↔kind table, which derives its inverse
    /// direction by scanning this rather than hand-maintaining a second
    /// match that could drift.
    ///
    /// Kept in sync by [`Self::variant_index`], whose exhaustive match
    /// makes adding a variant without extending this list a compile
    /// error, plus a test asserting the two agree.
    pub const ALL: &'static [Self] = &[
        Self::CardSelect,
        Self::PutBack,
        Self::ExhaustPile,
        Self::OrbSlot,
        Self::EnemyTarget,
        Self::AddGeneratedToHand,
        Self::AddGeneratedRetainedToHand,
        Self::AddGeneratedFreeToHand,
        Self::PaelsToothStash,
        Self::UpgradeInHand,
        Self::AddFromDrawByType,
        Self::AddFromDiscardFree,
        Self::DiscardFromHand,
        Self::ExhaustFromHand,
        Self::DiscardFromHandAndDraw,
        Self::MakeFreeInHand,
        Self::MonsterCurseSelect,
        Self::AddFromDrawToHand,
        Self::ForegoneConclusionFromDraw,
        Self::AddFromDiscardToHand,
        Self::ApplyRetainInHand,
        Self::ApplySlyInHand,
        Self::ApplyEtherealInHand,
        Self::TransformInHandToMinionStrike,
        Self::TransformInHandToMinionSacrifice,
        Self::TransfigureCardInHand,
        Self::TransformDrawCardToSoul,
        Self::TransformDrawCardsToMinionDiveBomb,
        Self::CloneChosenCardNextTurn,
        Self::RetainAtTurnEnd,
        Self::AutoPlaySkillFromHand,
    ];

    /// This variant's position in [`Self::ALL`].
    ///
    /// The match is exhaustive on purpose: a new `ChoiceKind` variant
    /// fails to compile here, which is the reminder to extend `ALL`.
    #[must_use]
    pub const fn variant_index(self) -> usize {
        match self {
            Self::CardSelect => 0,
            Self::PutBack => 1,
            Self::ExhaustPile => 2,
            Self::OrbSlot => 3,
            Self::EnemyTarget => 4,
            Self::AddGeneratedToHand => 5,
            Self::AddGeneratedRetainedToHand => 6,
            Self::AddGeneratedFreeToHand => 7,
            Self::PaelsToothStash => 8,
            Self::UpgradeInHand => 9,
            Self::AddFromDrawByType => 10,
            Self::AddFromDiscardFree => 11,
            Self::DiscardFromHand => 12,
            Self::ExhaustFromHand => 13,
            Self::DiscardFromHandAndDraw => 14,
            Self::MakeFreeInHand => 15,
            Self::MonsterCurseSelect => 16,
            Self::AddFromDrawToHand => 17,
            Self::ForegoneConclusionFromDraw => 18,
            Self::AddFromDiscardToHand => 19,
            Self::ApplyRetainInHand => 20,
            Self::ApplySlyInHand => 21,
            Self::ApplyEtherealInHand => 22,
            Self::TransformInHandToMinionStrike => 23,
            Self::TransformInHandToMinionSacrifice => 24,
            Self::TransfigureCardInHand => 25,
            Self::TransformDrawCardToSoul => 26,
            Self::TransformDrawCardsToMinionDiveBomb => 27,
            Self::CloneChosenCardNextTurn => 28,
            Self::RetainAtTurnEnd => 29,
            Self::AutoPlaySkillFromHand => 30,
        }
    }
}

/// One selectable option presented in a [`PendingChoice`]. Mirrors
/// `sts2_env/core/selection.py::CardChoiceOption` (id + `source_pile`)
/// plus the `upgraded` flag emitted by
/// `bridge_replay.combat_state_to_bridge_state` lines 307-311.
///
/// `card_uid` is the canonical handle when the option refers to a
/// concrete in-combat [`CardInstance`]; it is left as `None` for choice
/// flavors that don't reference a card (e.g. `OrbSlot`, `EnemyTarget`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChoiceOption {
    /// Card identity (catalog id). For non-card choices, callers should
    /// pick a sentinel — at this pin all production callers are
    /// card-flavored.
    pub card_id: crate::cards::CardId,
    /// Upgrade flag mirroring Python `option.card.upgraded or None`.
    pub upgraded: bool,
    /// Concrete in-combat handle when the option references a specific
    /// [`CardInstance`]; `None` for synthetic options (e.g.
    /// card-reward, `OrbSlot`).
    pub card_uid: Option<crate::CardUid>,
    /// Pile the option was drawn from when applicable.
    pub source_pile: Option<crate::cards::PileKind>,
}

/// Pending card-selection prompt. Mirrors `PendingCardChoice` in
/// `sts2_env/core/selection.py` (prompt + options + min/max + allow
/// skip) and the bridge replay payload at
/// `sts2_env/parity/bridge_replay.py:303-316`.
///
/// At this pin the struct is a POD: no production code currently sets
/// `state.pending_choice = Some(..)` (sts2-rs-cqm will wire card
/// effects). The shape is provided so downstream beads can construct
/// real choices without further churn, and so the hash impl
/// (`hashing.rs`) can mix the payload deterministically — see
/// `hash_pending_choice`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PendingChoice {
    /// Discriminant — what *kind* of choice is open.
    pub kind: ChoiceKind,
    /// Ordered list of selectable options. Order is preserved (matches
    /// Python `selected_indices: set[int]` indexing into `options`).
    pub options: Vec<ChoiceOption>,
    /// Minimum number of options the player must select to confirm.
    pub min_select: u8,
    /// Maximum number of options the player may select before confirm.
    pub max_select: u8,
    /// Mirrors Python `PendingCardChoice.allow_skip`: when true the
    /// choice can be resolved by selecting zero options.
    pub can_skip: bool,
    /// Optional effect applied AFTER the selection resolves, used by
    /// cards whose C# `OnPlay` runs a follow-up command once the
    /// player has picked (e.g. Brand `→ Apply<StrengthPower>`,
    /// BurningPact `→ Draw`). The handler pre-computes the
    /// upgrade-dependent magnitude at choice-setup time and stuffs it
    /// here; the resolver applies it without needing to know which card
    /// opened the choice. `None` for every choice with no follow-up
    /// (the common case). Currently honored by
    /// [`ChoiceKind::ExhaustFromHand`] (see [`resolve_exhaust_from_hand`]).
    /// Bead `sts2-rs-pe5zo`.
    pub post_effect: Option<PostChoiceEffect>,
    /// Original-option indices picked so far during a SEQUENTIAL multi-pick
    /// (`max_select > 1`) resolution. Empty for single-pick choices and for a
    /// freshly-opened multi-pick. Bead `sts2-rs-sopgf`.
    ///
    /// MCTS can choose *which* cards a multi-pick adds (e.g. Necrobinder's
    /// Dredge "add up to N discard cards") via single-pick decomposition:
    /// [`legal_actions`] fans a `max_select > 1` choice out into one
    /// incremental `ResolveChoice { [i] }` per still-unpicked option plus a
    /// FINALIZE `ResolveChoice { [] }`. Each incremental pick appends `i` here
    /// and RE-OPENS the same choice; the per-`ChoiceKind` resolver runs ONCE,
    /// at FINALIZE, over the full accumulated `picked` set. Option *indices*
    /// stay stable (`options` is never mutated) — progress is tracked solely
    /// through `picked`.
    ///
    /// **Parity invariant.** Intermediate re-opens fire NO effects, so the
    /// kind-resolver receives the IDENTICAL full index set it always has — the
    /// resulting state is byte-identical to a single atomic resolve. The
    /// replay/oracle paths (`RunManager::resolve_combat_choice_selection`,
    /// `CombatResolveChoiceTakeFirst`) bypass accumulation entirely via
    /// [`resolve_choice_atomic`], driving the full selection in one shot.
    pub picked: SmallVec<[u8; 4]>,
}

/// A follow-up effect a [`PendingChoice`] applies once its selection
/// resolves. Lets a card defer part of its `OnPlay` past a
/// player-selection prompt (C# `CardSelectCmd.From*` mid-`OnPlay`)
/// without the engine having to remember which card opened the choice.
/// The magnitude is captured at choice-setup time (already folded
/// through the card's upgrade state). Bead `sts2-rs-pe5zo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PostChoiceEffect {
    /// Apply `StrengthPower(amount)` to the player. Brand
    /// (`→ Apply<StrengthPower>(Strength.BaseValue)`).
    GainStrength(i32),
    /// Draw `count` cards. BurningPact (`→ Draw(Cards.BaseValue)`).
    DrawCards(i32),
    /// Apply `amount` stacks of `kind` to the player AFTER the selection
    /// resolves. Scavenge (`→ Apply<EnergyNextTurnPower>(Energy)` once the
    /// exhaust choice resolves; the power applies regardless of whether a
    /// card was exhausted, so callers fire it inline on an empty hand).
    ApplyPlayerPower(PowerKind, i32),
}

/// A potion's `Hook.AfterPotionUsed` dispatch deferred because the potion's
/// own effect opened a blocking player choice. C# `PotionModel.OnUseWrapper`
/// (`MegaCrit.Sts2.Core.Models/PotionModel.cs:291-342`) `await`s `OnUse`
/// (`:327`) — and with it any card-select the potion surfaces (the
/// Attack/Skill/Power/Colorless Potion choose-a-card screen) — BEFORE firing
/// `Hook.AfterPotionUsed` (`:338`), so AfterPotionUsed listeners
/// (`ReptileTrinket.AfterPotionUsed`'s +3 temporary Strength, `BeltBuckle`)
/// see the post-choice state. Rust's `Action::DrinkPotion` arm parks the
/// dispatch here when the potion effect leaves a live
/// [`CombatState::pending_choice`]; the [`resolve_choice_atomic`] tail
/// consumes it once the pick resolves without re-opening a nested choice.
/// Always `None` at rest (not hashed, like
/// [`CombatState::pending_draw_resume`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingPotionHookResume {
    /// The potion that was drunk (already removed from the belt).
    pub kind: crate::potions::PotionKind,
    /// The resolved enemy-slot target the drink used (`None` for
    /// self / all-enemies potions).
    pub target: Option<u8>,
}

/// A hand-draw suspended mid-reshuffle by an `after_shuffle` player
/// choice (Stratagem). Mirrors C# `CardPileCmd.Draw`'s per-card loop
/// awaiting `Hook.AfterShuffle` (and thus the Stratagem grid-select)
/// BEFORE the next card is taken — the not-yet-drawn remainder only
/// resolves after the pick. Stored on
/// [`CombatState::pending_draw_resume`]; consumed by
/// `apply(Action::ResolveChoice)`. Bead `sts2-rs-63mew`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingDrawResume {
    /// Cards still to draw once the pending choice resolves (the card
    /// whose reshuffle opened the choice has NOT been drawn yet).
    pub remaining: i32,
    /// Whether the suspended draw was a combat-loop hand draw
    /// ([`CombatState::draw_into_hand`] — fires the enchantment
    /// `on_card_drawn` channel with `from_hand_draw = true`) or a
    /// card-effect draw ([`cards::draw_via_state`] — does not).
    pub from_hand_draw: bool,
    /// Set by `combat_loop::continue_player_turn_setup` when the
    /// suspended draw was the turn-start hand draw: after the resumed
    /// draw completes, the remaining turn-setup steps
    /// (`combat_loop::finish_player_turn_setup`) must run.
    pub resume_turn_setup: bool,
}

/// In-flight state of a turn-start Mayhem `AutoPlayFromDrawPile` collect loop
/// suspended mid-reshuffle by a `StratagemPower.AfterShuffle` grid-select.
/// Phase 1 (collect `count` top cards to Play) had drawn `collected` so far and
/// still needs `remaining` more once the choice resolves; phase 2 (play the
/// collected cards) runs after. Bead `sts2-rs-o0rz3`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PendingAutoplay {
    /// Cards still to collect once the choice resolves.
    pub remaining: i32,
    /// Cards already moved to the Play pile (to play in phase 2).
    pub collected: smallvec::SmallVec<[CardUid; 8]>,
    /// C# `card.ExhaustOnNextPlay` for the collected cards.
    pub force_exhaust: bool,
}

/// The turn-start auto-play cluster suspended by a forced card-select. Three
/// distinct suspend sites share this resume record:
///
/// 1. **`AfterPlayerTurnStart` walk** (`mayhem_phase == false`): a Tyranny /
///    Tools-of-the-Trade exhaust/discard select opens during the pre-`after_side`
///    snapshot walk. Resume continues the walk at `idx` (its `autoplay` is a
///    no-op), then fires `after_side_turn_start`, then the Mayhem auto-play.
/// 2. **Mayhem AutoPrePlay** (`mayhem_phase == true`): a `StratagemPower`
///    `AfterShuffle` grid-select opens during Mayhem's auto-play. In v0.107.1
///    Mayhem is `AfterAutoPrePlayPhaseEntered`, which C# `CombatManager` runs
///    AFTER `Hook.AfterSideTurnStart` (`CombatManager.cs:524` precedes the
///    `RunAutoPrePlayPhase` at `:570`), so `after_side_turn_start` has ALREADY
///    fired by this point. Resume finishes the in-flight `autoplay` and runs the
///    turn-start tail — it does NOT re-walk the snapshot or re-fire
///    `after_side_turn_start`.
/// 3. **Relic `AfterPlayerTurnStart` bus** (`relic_start == Some(idx)`):
///    GamblingChip's round-1 mulligan opens a `CardSelectCmd` that C#
///    `Hook.AfterPlayerTurnStart` AWAITS, so every later relic on that pass
///    (MercuryHourglass) plus the whole `AfterSideTurnStart` walk (Akabeko,
///    Lantern, BloodVial) waits for the pick. Resume re-enters the relic bus at
///    equip index `idx` and then runs the rest of the tail. `autoplay` /
///    `idx` / `mayhem_phase` are unused in this case.
///
/// Stored on [`CombatState::pending_cluster_resume`]; consumed by
/// `apply(Action::ResolveChoice)`. Beads `sts2-rs-o0rz3` / `sts2-rs-181`;
/// case 3 is mirror-fidelity round-4 defect #13.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PendingClusterResume {
    /// The pre-auto-play power snapshot, in cluster-dispatch order.
    pub snapshot: smallvec::SmallVec<[PowerUid; 8]>,
    /// Index in `snapshot` of the power that suspended (its `autoplay` must be
    /// finished first); the walk continues from `idx + 1`.
    pub idx: usize,
    /// The suspended auto-play to finish before continuing.
    pub autoplay: PendingAutoplay,
    /// `after_side_turn_start` already fired at the suspend point — do not
    /// re-fire it when the cluster resumes.
    pub side_turn_start_fired: bool,
    /// `true` when the suspend happened in Mayhem's post-`after_side`
    /// AutoPrePlay dispatch (case 2 above): resume the Mayhem auto-play + tail
    /// directly rather than the `AfterPlayerTurnStart` snapshot walk.
    pub mayhem_phase: bool,
    /// `Some(equip_idx)` when the suspend happened inside the RELIC
    /// `AfterPlayerTurnStart` bus (case 3 above — GamblingChip's mulligan):
    /// resume that bus from the relic at `equip_idx`, then run the rest of the
    /// turn-start tail. `None` for cases 1 and 2.
    pub relic_start: Option<u8>,
}

/// Pending mid-play state for a card that resumes after a choice.
///
/// Bead `sts2-rs-mcts-surface-onplay-select`: when a top-level player card's
/// `OnPlay` opens an interactive `pending_choice` we SURFACE (so an MCTS agent
/// can pick) rather than drain to first-legal, the rest of the play — the
/// `AfterCardPlayed` buses, per-turn tallies, OneTwoPunch/EchoForm replay
/// loops, and Play→Discard/Exhaust disposal — is stashed here and re-entered by
/// `resolve_choice_action` once the pick lands (via
/// `combat_loop::finish_card_play`). This matches C#'s `OnPlayWrapper`, which
/// `await`s the selector INSIDE `OnPlay`, so the selection's consequences (the
/// discard + its `AfterCardDiscarded` reaction) run before `AfterCardPlayed`.
/// Set together with `pending_choice` and always cleared before the choice
/// closes, so it never persists across a turn boundary (see `invariants.rs`).
/// Realizes the context the former empty placeholder stubbed (Python's
/// `_resume_pending_play`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingPlay {
    pub(crate) finish: crate::combat_loop::CardPlayFinish,
}

/// An auto-play (Havoc / Cascade / Catastrophe / Mayhem / DistilledChaos /
/// Stampede) suspended because the card it just dispatched opened a SURFACING
/// `pending_choice` from its `OnPlay`.
///
/// **Why this exists (parity bug `combat_select_ironclad`).** C#'s
/// `OnPlayWrapper` `await`s the `ICardSelector` INSIDE `OnPlay`
/// (`CardModel.cs:1498`). The headless selector
/// (`HeadlessCardSelector.GetSelectedCards`, `RunSimulator.cs:4804-4836`)
/// auto-resolves INLINE only when `optList.Count == 0`, or
/// `optList.Count == 1 && minSelect >= 1`, or the `AutoResolveInline` flag is
/// set — and `AutoResolveInline` is set ONLY while a combat play is being
/// RESUMED (`DoSelectCards`, `RunSimulator.cs:2165`). So the FIRST card-select
/// an auto-play's `OnPlay` opens has `AutoResolveInline == false` and SURFACES
/// (`HasPending == true`); only FURTHER choices opened by the resumed play
/// drain inline. The Rust port used to blanket-drain every such choice
/// (`resolve_inline_auto_play_choices`), silently picking option-0 and staying
/// in `combat` instead of surfacing — e.g. Havoc auto-playing un-upgraded
/// Armaments with 2+ upgradable hand cards auto-upgraded the first card rather
/// than surfacing the Strike/Defend `UpgradeInHand` select.
///
/// This captures everything needed to resume `resolve_auto_played_card`'s tail
/// (after the dispatch + surfaced choice) AND the remaining collected
/// auto-play cards, so `apply(Action::ResolveChoice)` can finish the play
/// faithfully. Mirrors the [`PendingPlay`] machinery for top-level plays.
/// `auto_resolve_inline` is raised on resume so any further nested choice
/// drains inline (the C# `AutoResolveInline` mirror — preserving the `y1fic`
/// chained-auto-play deadlock fix). Set together with `pending_choice`; always
/// cleared before the choice closes, so it never persists across a turn
/// boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingAutoplayResume {
    /// The post-dispatch tail of the suspended card's `resolve_auto_played_card`.
    pub(crate) finish: crate::combat_loop::AutoPlayFinish,
    /// The not-yet-played remaining collected cards (Phase-2 of
    /// `auto_play_n_from_draw_top`), played in order once the tail finishes.
    pub remaining: smallvec::SmallVec<[CardUid; 8]>,
    /// `card.ExhaustOnNextPlay` for the remaining collected cards.
    pub force_exhaust: bool,
    /// Re-selection auto-play-loop continuation (Catastrophe / BeatDown). A
    /// card that re-picks from a *live* pile each iteration (Catastrophe
    /// re-shuffles the draw pile every step; BeatDown plays a fixed captured
    /// discard list) cannot express its remainder as the pre-collected
    /// `remaining` UID list above: a surfaced nested select (SecretTechnique
    /// fetching a Skill, Headbutt relocating a discard card) MUST resolve —
    /// mutating the pile — before the next iteration's pick. So such a loop
    /// suspends mid-iteration and records how to re-enter itself here; the
    /// resolver runs it (under `autoplay_inline_resolve`) once the surfaced
    /// card's tail + `remaining` finish. `None` for the pre-collected
    /// draw-top variants (Cascade / Havoc / Mayhem / DistilledChaos), which
    /// only need `remaining`. Bead `sts2-rs-cata-suspend`.
    pub continuation: Option<AutoPlayLoopCont>,
    /// Outer auto-plays whose post-dispatch tail is still owed because an
    /// auto-play they nested INTO surfaced first. Stored inner→outer (push
    /// order): the card recorded in `finish` is the one that surfaced the live
    /// `pending_choice`; each chain entry is an enclosing auto-play that
    /// unwound up to the surface gate while that select was still pending.
    ///
    /// Parity bug seed 2752 (`DISTILLED_CHAOS` auto-plays `HAVOC`, which
    /// auto-plays `BURNING_PACT`, which surfaces an `ExhaustFromHand` select):
    /// HAVOC's tail must run AFTER BURNING_PACT's, but a single `finish` slot
    /// can hold only one — the previous code overwrote BURNING_PACT's finish
    /// with HAVOC's, so BURNING_PACT's Play→Exhaust disposal never ran and the
    /// card was lost from every pile. The resolver finishes `finish` (+ its
    /// `remaining`/`continuation`) and then drains this chain in order. Empty
    /// for the common single-level auto-play. Follow-up to bead
    /// `sts2-rs-cata-suspend`.
    pub parent_chain: smallvec::SmallVec<[crate::combat_loop::AutoPlayParentFrame; 4]>,
}

/// Where the player's end-turn dispatch chain suspended when a listener
/// auto-play surfaced an interactive card-select.
///
/// C# never has a "stage" here: `EndPlayerTurn` is one `async` method and the
/// `await CardCmd.AutoPlay` inside `StampedePower.AfterAutoPostPlayPhaseEntered`
/// simply blocks where it stands, so everything after that point — the rest of
/// the Stampede batch, the `HowlFromBeyond` exhaust replay, `BeforeTurnEnd`,
/// the flush, the enemy turn — runs only once the selector returns. The Rust
/// port has to unwind to the caller and come back, so it records the resume
/// point explicitly. Bead `sts2-rs-eirvo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndTurnResume {
    /// Suspended INSIDE `AutoPostPlay`, mid-Stampede-batch. `stampede_remaining`
    /// iterations of C# `StampedePower`'s `for (i < Amount)` loop are still
    /// owed, and none of what follows the batch has run yet: the Howl replay,
    /// the unspent-energy / zero-block snapshots, and the `BeforeTurnEnd`
    /// dispatch are all still ahead.
    ///
    /// Each remaining iteration re-reads the hand, exactly as C# does after its
    /// `await` — so a card the resolved select ADDED to hand (SeekerStrike's
    /// draw-pile pick) is a legal target for the next Stampede attack, which it
    /// was not while the batch ran ahead of the selection.
    AutoPostPlay {
        /// Stampede-batch iterations not yet played.
        stampede_remaining: u32,
    },
    /// Suspended after the `BeforeTurnEnd` dispatch: only the phase-one tail
    /// (Chains clear, orb passives, phase advance) and the rest of the chain
    /// are owed.
    AfterBeforeTurnEnd,
}

/// Continuation that re-enters a re-selection auto-play loop for its
/// not-yet-run iterations after a surfaced nested card-select resolves.
///
/// Mirrors C#: `Catastrophe.OnPlay` / `BeatDown.OnPlay` `await
/// CardCmd.AutoPlay(...)` inside their loop, so an auto-played card that
/// opens a surfacing select fully resolves (the fetched / relocated card
/// leaves the pile) BEFORE the loop's next pick reads that pile. The Rust
/// auto-play primitives surface the first such select to the harness / agent
/// and suspend; this struct carries the loop's resume point so
/// [`resolve_choice_action`] can replay the remaining iterations once the
/// pick lands.
///
/// `resume_fn` lives in `sts2-content` (the card module that owns the loop);
/// core only stores and calls the plain function pointer. Catastrophe
/// re-selects from the live draw each step, so its remaining work is a COUNT
/// (`iters`); BeatDown plays a fixed captured discard list, so its remaining
/// work is the not-yet-played `uids`. Whichever the loop does not use stays
/// empty / zero.
#[derive(Clone, Debug)]
pub struct AutoPlayLoopCont {
    /// Re-entry point: `(state, remaining_iters, remaining_uids, force_exhaust)`.
    pub resume_fn: fn(&mut CombatState, u32, &[CardUid], bool),
    /// Remaining loop iterations for a count-driven loop (Catastrophe).
    pub iters: u32,
    /// Not-yet-played captured cards for a fixed-list loop (BeatDown).
    pub uids: smallvec::SmallVec<[CardUid; 8]>,
    /// `force_exhaust` threaded through the remaining iterations.
    pub force_exhaust: bool,
}

// `PartialEq`/`Eq` are hand-written rather than derived: deriving `==` over the
// `resume_fn` field trips the `unpredictable_function_pointer_comparisons` lint
// (fn addresses are not guaranteed unique across codegen units). Comparing the
// pointers' addresses as `usize` is good enough for the transient resume marker
// (within one process the same fn has a stable address) and is lint-clean +
// MSRV-1.78-safe (`core::ptr::fn_addr_eq` only stabilized in 1.85).
impl PartialEq for AutoPlayLoopCont {
    fn eq(&self, other: &Self) -> bool {
        self.resume_fn as usize == other.resume_fn as usize
            && self.iters == other.iters
            && self.uids == other.uids
            && self.force_exhaust == other.force_exhaust
    }
}
impl Eq for AutoPlayLoopCont {}

/// The enemy-turn perform walk suspended mid-flight because a monster move
/// opened a BLOCKING player choice.
///
/// **Why this exists (KnowledgeDemon CURSE_OF_KNOWLEDGE).** C#
/// `KnowledgeDemon.CurseOfKnowledge` (`KnowledgeDemon.cs:149-189`) `await`s
/// `CardSelectCmd.FromChooseACardScreen(new BlockingPlayerChoiceContext(), …)`
/// INSIDE the move body: the whole enemy turn — the rest of that move, the
/// remaining monsters, the turn-end pipeline, and the next player turn-start —
/// blocks on the player's pick. Rust previously deferred the pick to the next
/// `start_player_turn`, which put the engine a full turn AHEAD of the live game
/// at the same decision (measured on the v27wr sweep: ~81 intent, 95 `$.round`
/// and 37 `$.phase` divergences plus 35 unanswerable `card_select` injections).
///
/// This captures the already-snapshotted acting roster plus the index of the
/// next monster to act, so `apply(Action::ResolveChoice)` can re-enter
/// [`crate::combat_loop::resume_enemy_turn_exec`] and finish the walk exactly
/// where it stopped. Set together with `pending_choice`; always cleared before
/// the choice closes, so it never persists across a turn boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingEnemyExecResume {
    /// The `(uid, intent)` roster snapshot `enemy_turn_exec` took at entry, in
    /// `enemy_uids` order. Kept whole (not truncated to the remainder) because
    /// the post-loop `on_move_performed` walk iterates every enemy that acted
    /// this turn, including the ones that acted before the suspend.
    pub acting: SmallVec<[(CreatureUid, Option<Intent>); 8]>,
    /// Index into [`Self::acting`] of the next monster to perform. Everything
    /// below it has already performed (and recorded its intent history).
    pub next_index: usize,
}

/// Per-turn / per-combat event counters consumed by C#
/// `History.Entries` queries for `WasCardExhaustedThisTurn` /
/// `LostHpThisTurn` / `DamageReceivedEntry` predicates.
///
/// Bead `sts2-rs-xsvp9`: replaces the original placeholder POD with
/// the minimum-viable subset needed by EvilEye / Spite / TearAsunder
/// (the three Ironclad conditional handlers deferred from
/// `sts2-rs-1d56i`). Per-turn fields reset at `start_player_turn`
/// via [`Self::reset_per_turn`]; combat-wide fields reset only at
/// `start_combat` (where the whole struct is `Default::default`).
/// One card resolution recorded for display (workbench event log). Ordered,
/// combat-scoped. Append-only; never read by engine logic — purely a UI
/// side-channel. Recording is gated by [`TurnEvents::record_card_play_log`]
/// (default OFF) so headless self-play / parity / MCTS rollouts allocate
/// nothing. NOT emitted by `state_to_dict` — the parity surface is unaffected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayLogEntry {
    /// The card that resolved.
    pub card_id: crate::cards::CardId,
    /// Upgrade level of the played instance.
    pub upgraded: bool,
    /// Energy actually paid (the X value for X-cost cards; 0 for free
    /// auto-plays).
    pub energy_spent: i32,
    /// `true` for engine auto-plays (Cascade / Havoc / Stampede / Mayhem /
    /// DistilledChaos); `false` for a manual `play_card`.
    pub auto_played: bool,
    /// Player-chosen enemy target index, if any (manual single-target plays).
    pub target: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TurnEvents {
    /// Count of cards the player has drawn THIS TURN via card effects —
    /// i.e. `CardDrawnEntry`s with `!FromHandDraw` (every draw that is NOT
    /// the turn-start hand draw). C# Necrobinder `DeathMarch.cs`'s
    /// `CalculatedDamageVar` multiplier counts exactly these
    /// (`History.Entries.OfType<CardDrawnEntry>().Count(e =>
    /// e.HappenedThisTurn(cs) && e.Actor == owner && !e.FromHandDraw)`).
    /// Incremented on the single card-effect draw chokepoint
    /// ([`crate::cards::draw_via_state`]): per-card inside
    /// [`CombatState::draw_suspending`] when `from_hand_draw == false`
    /// (covers the suspend/resume per-card loop), plus the batch path's
    /// `moved` count. The turn-start hand draw routes through
    /// [`CombatState::draw_into_hand`] (`from_hand_draw == true`) and is
    /// NOT counted. Cleared each player turn in [`Self::reset_per_turn`].
    pub cards_drawn_by_effect_this_turn: u32,
    /// Number of `CardType::Status` cards the player has drawn this turn.
    /// Mirrors C# `IterationPower.AfterCardDrawn`'s history count
    /// (`History.Entries.OfType<CardDrawnEntry>().Count(e =>
    /// e.HappenedThisTurn(cs) && e.Actor == owner && e.Card.Type == Status)`).
    /// Iteration draws `Amount` extra cards only when this count is `<= 1`
    /// (i.e. on the FIRST status card drawn each turn — including the
    /// turn-start hand draw, since C# ignores `fromHandDraw`). Incremented
    /// per drawn Status card in [`Self::invoke_card_drawn_for_newly_drawn`];
    /// cleared each player turn in [`Self::reset_per_turn`].
    pub status_cards_drawn_this_turn: u32,
    /// `true` once the PLAYER has applied `DoomPower` to any creature during
    /// the current player turn. C# Necrobinder `DeathsDoor.cs`'s
    /// `WasDoomAppliedThisTurn` scans `History.Entries.OfType<PowerReceivedEntry>()`
    /// for a `DoomPower` whose `Applier == Owner.Creature` that
    /// `HappenedThisTurn` — when set, DeathsDoor gains its Block 3× instead of
    /// 1×. Set in two places: [`crate::card_effects::apply_power_to_enemy`]
    /// (the card-play player→enemy path) when `kind == Doom`, and
    /// [`crate::card_effects::apply_power_to_creature_with_applier`] when a
    /// Doom lands with `applier == player` — the latter covers the
    /// applier-threaded grants (Neurosurge/Countdown and the
    /// OblivionPower/ReaperForm/PotionOfDoom grants, all `Applier == player`
    /// in C#). Enemy-applied Doom does NOT arm it. Cleared each player turn
    /// in [`Self::reset_per_turn`].
    pub doom_applied_by_player_this_turn: bool,
    /// Number of times the Osty (the Necrobinder's pet) has attacked this
    /// player turn. Mirrors C# `History` `CreatureAttackedEntry` count where
    /// `Actor == Owner.Osty && HappenedThisTurn`. Incremented at the damage
    /// chokepoint ([`crate::damage`]) when the dealer is the pet
    /// (`CreatureRef::Enemy(pet_uid)`), cleared each player turn in
    /// [`Self::reset_per_turn`], and read by `Rattle`'s `1 + count` hit-count
    /// scaling. Bead `sts2-rs-osty-attacks-this-turn`.
    pub osty_attacks_this_turn: i32,
    /// Count of cards exhausted since the start of the current
    /// player turn. C# `EvilEye.WasCardExhaustedThisTurn` queries
    /// `CardExhaustedEntry.Owner == Owner && HappenedThisTurn`;
    /// our engine has only the player as a card-owning side, so a
    /// scalar counter is exact. Incremented inside
    /// [`crate::hooks::dispatch_after_card_exhausted`] (the single
    /// dispatch site every exhaust path funnels through).
    pub cards_exhausted_this_turn: u32,
    /// Count of `DamageReceivedEntry`s vs the player this turn
    /// with `Result.UnblockedDamage > 0`. C# `Spite.LostHpThisTurn`
    /// reads this as a bool (`> 0`); modelled as a counter for
    /// symmetry with the combat-wide field. Incremented in
    /// [`crate::combat_loop::apply_damage_to_player`] when
    /// post-block damage is non-zero.
    pub hp_lost_this_turn: u32,
    /// Combat-wide count of `DamageReceivedEntry`s vs the player
    /// with `Result.UnblockedDamage > 0`. C# `TearAsunder` reads
    /// the count directly: `CalculatedHits = 1 + this`.
    /// Incremented at the same site as
    /// [`Self::hp_lost_this_turn`]; reset only at combat start.
    pub unblocked_damage_received_this_combat: u32,
    /// Total energy the player has spent this turn (summed across
    /// every card's `SpendEnergy`). The C# analog is
    /// `History.Entries.OfType<EnergySpentEntry>().Where(
    /// HappenedThisTurn).Sum(Amount)`; bumped in
    /// [`crate::combat_loop::play_card`] at the point C# records an
    /// `EnergySpentEntry`, reset each turn in [`Self::reset_per_turn`].
    /// Read by `HelixDrill`'s `CalculatedHits` (hits = energy spent this
    /// turn − the card's own effective cost). HelixDrill.cs:30-43.
    pub energy_spent_this_turn: i32,
    /// Count of 0-energy **Attack** cards the player has played this turn
    /// (incl. auto-plays), reset at the player's side-turn start via
    /// [`Self::reset_per_turn`]. Drives
    /// `FeralPower.ModifyCardPlayResultPileTypeAndPosition` (C# `FeralPower.cs`):
    /// the first `Amount` such attacks return to Hand/Top instead of
    /// exhaust/discard. C# tracks this via a per-power `zeroCostAttacksPlayed`
    /// seeded (AfterApplied) from the turn's history and reset
    /// (AfterSideTurnStart); a plain per-turn tally is the equivalent — a
    /// pre-Feral 0-cost attack still counts against the budget (it incremented
    /// the tally) even though it wasn't redirected. Reads the energy ACTUALLY
    /// spent (`resolved_cost`/`ResourceInfo.EnergyValue`), not printed cost.
    pub zero_cost_attacks_played_this_turn: u32,
    /// Count of player card plays FINISHED this player-turn (incl. auto-plays),
    /// bumped at the same sites as [`Self::player_card_plays_this_combat`].
    /// Snapshotted into [`Self::card_plays_finished_last_player_turn`] at the
    /// start of the next player turn (in [`Self::reset_per_turn`]) so
    /// `PaleBlueDotPower.ModifyHandDraw` can read the PREVIOUS turn's total
    /// (C# `History.CardPlaysFinished.Count(HappenedLastPlayerTurn)`).
    pub card_plays_finished_this_turn: u32,
    /// Snapshot of [`Self::card_plays_finished_this_turn`] as of the end of the
    /// LAST player turn (captured at the next turn's `reset_per_turn`, before the
    /// hand draw). Read by the `PaleBlueDot` arm of
    /// [`crate::hooks::modify_hand_draw`]: +Amount draw iff `>= 5`.
    pub card_plays_finished_last_player_turn: u32,
    /// Count of `CardDiscardedEntry`s @ Owner == player this turn.
    /// C# `MementoMori.CalculatedDamage` multiplies `ExtraDamage`
    /// by this count. Bumped in
    /// [`crate::hooks::dispatch_after_card_discarded`] (the single
    /// dispatch site every discard path funnels through); cleared
    /// at `start_player_turn` via [`Self::reset_per_turn`].
    /// Bead `sts2-rs-cdwi3` (restored under `sts2-rs-76kjk` — the
    /// original cdwi3 lib.rs delta dropped out of an upstream
    /// rebase, leaving hooks.rs referencing a phantom field).
    pub cards_discarded_this_turn: u32,
    /// Creatures that transitioned from `current_hp > 0` to
    /// `current_hp <= 0` since the start of the current turn (player
    /// or enemy turn — same buffer). Drives the cross-creature
    /// AfterDeath dispatch surface (`sts2-rs-79tv9`); a Queen-side
    /// reactor flips per-AI counters when sibling TorchHeadAmalgam
    /// dies (`sts2-rs-6g4v4`). Populated at every site that mutates
    /// `current_hp`; consumed + drained by the dispatch loop after
    /// each `perform_intent`. Cleared in [`Self::reset_per_turn`]
    /// for defense-in-depth; the dispatch loop drains it eagerly so
    /// the reset is normally a no-op.
    ///
    /// Bead `sts2-rs-ar9ll` (dvqa1.1). See
    /// `docs/audits/2026-05-29-dvqa1-after-death-dispatch-scope.md`.
    pub newly_dead_this_turn: smallvec::SmallVec<[crate::CreatureUid; 4]>,
    /// Count of player cards afflicted with `Bound` by
    /// `ChainsOfBindingPower` since the start of the current player
    /// turn. C# `ChainsOfBindingPower.AfterCardDrawn` (v0.103.2,
    /// `MegaCrit.Sts2.Core.Models.Powers/ChainsOfBindingPower.cs`)
    /// caps per-turn `Bound` applications at the power's stack count
    /// by reading `CombatManager.History.Entries.OfType<CardAfflictedEntry>()
    /// .Count(... Affliction is Bound && Actor == base.Owner &&
    /// HappenedThisTurn(...))`. Rust does not maintain a
    /// `CardAfflictedEntry` history layer; this scalar counter is the
    /// equivalent per-turn cap-check budget. Incremented by the
    /// future `ChainsOfBinding` `AfterCardDrawn`-equivalent block
    /// inside [`Self::invoke_card_drawn_for_newly_drawn`] (sub-1,
    /// `sts2-rs-kese5`); cleared in [`Self::reset_per_turn`]. Bead
    /// `sts2-rs-hmxmc` (sub-2 of `sts2-rs-f933s` Queen
    /// PUPPET_STRINGS_MOVE).
    pub bound_cards_afflicted_by_chains_this_turn: u32,

    /// C# `ChainsOfBindingPower.internalData.boundCardPlayed`. Set true
    /// inside `play_card` after a Bound-afflicted card commits to play;
    /// gates further Bound plays this turn via `ShouldPlay`-equivalent
    /// guard at the top of `play_card`. Cleared in
    /// [`Self::reset_per_turn`]. Bead `sts2-rs-f0own` (f933s follow-up).
    pub chains_bound_card_played_this_turn: bool,

    /// C# `Voltaic.cs` `CalculatedVar` reads
    /// `History.Entries.OfType<OrbChanneledEntry>().Count(e =>
    /// e.Actor.Player == this.Owner && e.Orb is LightningOrb)` —
    /// i.e. the running total of Lightning orbs the player has
    /// channeled in this combat. Incremented by
    /// `orbs::channel_orb` whenever the channeled orb is
    /// `Orb::Lightning`. Combat-wide (NOT cleared in
    /// [`Self::reset_per_turn`]); resets via `TurnEvents::default()`
    /// at the start of each combat. Bead `sts2-rs-7y9vg`.
    pub lightning_orbs_channeled_this_combat: u32,

    /// C# `GoldAxe.cs` `CalculatedDamageVar.WithMultiplier(_ =>
    /// History.CardPlaysFinished.Count(e => e.CardPlay.Card.Owner ==
    /// owner))` — running total of player card plays in this
    /// combat. Incremented in `combat_loop::play_card` after the
    /// Hand→Play commit (once for the initial play; OneTwoPunch /
    /// EchoForm replays each bump it AGAIN inside their replay loops,
    /// mirroring C# `OnPlayWrapper` (CardModel.cs:1472-1523) which calls
    /// `History.CardPlayFinished` once per `i < playCount2` so each replay
    /// adds its own `CardPlayFinishedEntry`). The counter therefore equals
    /// `History.CardPlaysFinished.Count`. Combat-wide (NOT cleared in
    /// [`Self::reset_per_turn`]); resets via `TurnEvents::default()` at the
    /// start of each combat. Beads `sts2-rs-rnyme`, `sts2-rs-wvpjv`.
    pub player_card_plays_this_combat: u32,

    /// C# `History.Entries.OfType<CardPlayFinishedEntry>().Count(e =>
    /// e.CardPlay.Card.Owner == owner && e.WasEthereal)` — running
    /// total of ETHEREAL cards the player has finished playing this
    /// combat. Feeds Necrobinder `PullFromBelow.cs`'s
    /// `CalculatedVar` hit count (Base 0 + Extra 1 × this count).
    /// Incremented in `combat_loop::play_card` AFTER the card body
    /// resolves (so a card does not count itself — its
    /// `CardPlayFinishedEntry` is recorded post-`OnPlay`), once per
    /// resolved play including EchoForm replays (mirroring the per-turn
    /// `cards_played_this_turn` tally). Combat-wide (NOT cleared in
    /// [`Self::reset_per_turn`]); resets via `TurnEvents::default()` at
    /// the start of each combat.
    pub ethereal_cards_played_this_combat: u32,

    /// `true` only while the end-of-turn ETHEREAL sweep is draining the
    /// relic `after_card_exhausted` bus — the relic-side analogue of the
    /// hooks bus's `caused_by_ethereal` argument (the shared relic
    /// `CardEventCtx` cannot carry it without touching every constructor).
    /// JossPaper reads it to defer ethereal-caused exhaust credit to
    /// `AfterTurnEnd` (`JossPaper.cs:96-126`); a mid-sweep draw would pull
    /// a card into hand BEFORE the flush and feed it into the reshuffle
    /// (sweep seed 5329: one extra Shuffle read, DRUM_OF_BATTLE+1
    /// relegated). Bead `sts2-rs-adsld` (resolved root).
    pub exhaust_sweep_ethereal: bool,

    /// JossPaper's deferred ethereal-exhaust tally (C# `EtherealCount`,
    /// `JossPaper.cs:74-85`): ethereal-caused exhausts bank here during the
    /// sweep and fold into the relic counter at `AfterTurnEnd(Player)`,
    /// where the threshold draw fires post-flush.
    pub joss_paper_ethereal_count: u32,

    /// `CardId` of the last Attack/Skill the player played during the
    /// round that is currently in progress. Read at the start of the
    /// next round's play phase by `HistoryCourse`
    /// (`before_play_phase_start`), which auto-plays a free dupe of "the
    /// last Attack/Skill you played last round" (C# `HistoryCourse.cs`
    /// scans `History.CardPlaysFinished` for the last non-dupe
    /// Attack/Skill with `RoundNumber == round - 1`). A single field
    /// suffices because the value is read-then-reset at each
    /// play-phase-start, so between two play phases it holds exactly the
    /// just-finished round's last Attack/Skill. Set in
    /// `combat_loop::play_card` on each top-level Attack/Skill play (NOT
    /// on EchoForm/OneTwoPunch replays or `HistoryCourse`'s own dupe,
    /// which dispatch the effect directly — mirroring C#'s `IsDupe`
    /// exclusion). Combat-wide field; resets via `TurnEvents::default()`
    /// at combat start. Carries `(card_id, upgraded, instance uid)` so
    /// `HistoryCourse`'s dupe is replayed at the right upgrade level AND can
    /// clone the live instance — C# `CreateDupe()` copies the card MODEL,
    /// so accumulated per-instance state (Rampage's `+5/play` var, Thrash's
    /// exhaust fold) carries into the dupe (sweep seeds 2963/3087/4902).
    /// Also stamped by `resolve_auto_played_card`: a Stampede/Mayhem
    /// auto-play is a REAL card play (`!IsDupe`) and C#'s
    /// `History.CardPlaysFinished` scan sees it (sweep seeds 5645/4095).
    /// Bead `sts2-rs-ummci`.
    pub last_attack_skill_played: Option<(crate::cards::CardId, bool, Option<crate::CardUid>)>,

    /// The PREVIOUS round's final Attack/Skill play — what C#'s
    /// `HistoryCourse.BeforePlayPhaseStart` scan (`CardPlaysFinished` with
    /// `RoundNumber == round - 1`) actually reads. A single slot is not
    /// enough once auto-plays stamp too: the turn-start Mayhem cluster runs
    /// BEFORE the play-phase-start relic dispatch, so its play would
    /// overwrite round N-1's entry right before HistoryCourse reads it
    /// (sweep seed 4688: Rust duped this round's Mayhem DARK_SHACKLES where
    /// C# duped last round's MOLTEN_FIST). Shifted from
    /// [`Self::last_attack_skill_played`] at player-turn start, before the
    /// auto-play cluster.
    pub prev_round_last_attack_skill_played:
        Option<(crate::cards::CardId, bool, Option<crate::CardUid>)>,

    /// Snapshot of "the player had 0 block at the START of the turn-end
    /// sequence" (before ANY `before_turn_end` block grant). Mirrors C#
    /// `Orichalcum`/`FakeOrichalcum.BeforeTurnEndVeryEarly`, which arm their
    /// `ShouldTrigger` flag by reading `Block == 0` in the FIRST of C#'s
    /// three ordered turn-end passes, then grant in the Normal pass. Rust
    /// collapses the passes into one collect-then-apply walk where the relic
    /// handlers run AFTER the power block grants (Plating/RippleBasin), so a
    /// live `block != 0` read defeats the guard. Captured in
    /// `end_player_turn_phase_one` before the dispatches; read by the relic
    /// handlers. Reset in [`Self::reset_per_turn`]. Bead `sts2-rs-xk9yo`.
    pub block_zero_at_turn_end: bool,
    /// Player `Block` snapshotted at the start of the `after_turn_end`
    /// dispatch, BEFORE any late (`AfterSideTurnEndLate`) power consumes it.
    /// C# `Hook.AfterTurnEnd` runs all `AfterSideTurnEnd` (early — incl. the
    /// Parrying Shield relic) to completion, then all `AfterSideTurnEndLate`
    /// (late — incl. `DisintegrationPower`, which deals UNPOWERED damage the
    /// player's block absorbs). Rust collapses both phases into one walk and
    /// runs the late powers (`hooks::dispatch_after_turn_end`) before the
    /// relics (`relic_effects::dispatch_after_turn_end`), so Parrying Shield's
    /// `Block >= 10` test would see the already-decremented value. The relic
    /// handler reads this pre-late snapshot instead. Captured in
    /// `end_player_turn_phase_two`; reset in [`Self::reset_per_turn`].
    /// (sweep seed 5003: FISTICUFFS block 10 + Disintegration 6.)
    pub block_before_late_turn_end: i32,
    /// Bead `sts2-rs-xk9yo` (Unmovable): the play-sequence tag
    /// ([`Self::player_card_plays_this_combat`] at gain time) of each
    /// card-sourced (powered) player block-gain THIS turn, in order, one entry
    /// per gain. C# `UnmovablePower.ModifyBlockMultiplicative` doubles a card's
    /// block while the number of this-turn card/monster-move block-gain history
    /// entries *from a different `CardPlay`* is below the power's `Amount`. The
    /// play tag is the `CardPlay` identity: constant across a single card's
    /// multiple block-gains (so they double together) but distinct between
    /// separate plays — including two plays of the SAME card (which a card-uid
    /// key wrongly merged, over-doubling the replay; fuzz seed 2898). When
    /// block is gained, `num = count(tags where tag != current play tag)`;
    /// double if `num < Amount`. Populated once per gain (post-walk) in
    /// `hooks::dispatch_after_block_gained`, only when the player owns
    /// Unmovable. Cleared each player turn start. Power-sourced block
    /// (FeelNoPain / CrimsonMantle, `card_source == None`) is neither doubled
    /// nor recorded — matching C#'s `props.IsCardOrMonsterMove()` gate.
    pub unmovable_card_block_plays: smallvec::SmallVec<[u32; 8]>,
    /// Ordered, combat-scoped record of cards played, for the workbench
    /// event log (see [`PlayLogEntry`]). Appended by `combat_loop::play_card`
    /// and `combat_loop::resolve_auto_played_card` ONLY when
    /// [`Self::record_card_play_log`] is set. Persists for the whole combat
    /// (NOT cleared by `reset_per_turn`); cleared only at combat start via the
    /// full `TurnEvents::default()`.
    pub card_play_log: Vec<PlayLogEntry>,
    /// Opt-in flag: when `true`, the two play chokepoints append to
    /// [`Self::card_play_log`]. Default OFF. Only the workbench sets it (via
    /// `RunManager`); headless paths leave it false and pay zero cost.
    pub record_card_play_log: bool,
    /// The full game-style combat log — every hit, block, power, play, draw,
    /// orb and potion, with the card or move that caused it. Superset of
    /// [`Self::card_play_log`], which stays for its existing consumers.
    ///
    /// Opt-in via [`crate::event_log::EventLog::set_recording`] and OFF by
    /// default, so a headless run pays one `bool` read per would-be event.
    /// Excluded from equality (see the `PartialEq` impl on
    /// [`crate::event_log::EventLog`]) and never emitted by `state_to_dict`:
    /// a display side-channel, not a parity surface. Cleared at combat start
    /// with the rest of `TurnEvents`; recording must therefore be re-asserted
    /// per combat, exactly like `record_card_play_log`.
    pub event_log: crate::event_log::EventLog,
}

impl TurnEvents {
    /// Clear per-turn counters. Combat-wide counters (e.g.
    /// `unblocked_damage_received_this_combat`) are preserved.
    /// Called from `start_player_turn` instead of
    /// `Self::default()` so a fresh enemy-turn hit landing in the
    /// previous round still contributes to the combat-wide tally.
    pub fn reset_per_turn(&mut self) {
        self.cards_drawn_by_effect_this_turn = 0;
        self.status_cards_drawn_this_turn = 0;
        self.doom_applied_by_player_this_turn = false;
        self.osty_attacks_this_turn = 0;
        self.cards_exhausted_this_turn = 0;
        self.hp_lost_this_turn = 0;
        self.energy_spent_this_turn = 0;
        // FeralPower.AfterSideTurnStart resets zeroCostAttacksPlayed each turn.
        self.zero_cost_attacks_played_this_turn = 0;
        // PaleBlueDot: snapshot the just-ended player turn's finished-play count
        // BEFORE resetting, so this turn's ModifyHandDraw reads "last player
        // turn" (C# HappenedLastPlayerTurn). reset_per_turn runs at player
        // turn-start, before the hand draw.
        self.card_plays_finished_last_player_turn = self.card_plays_finished_this_turn;
        self.card_plays_finished_this_turn = 0;
        self.cards_discarded_this_turn = 0;
        self.newly_dead_this_turn.clear();
        // Bead `sts2-rs-hmxmc` (f933s sub-2): per-turn cap-check
        // budget for ChainsOfBindingPower's AfterCardDrawn affliction
        // limiter.
        self.bound_cards_afflicted_by_chains_this_turn = 0;
        // Bead `sts2-rs-f0own` (f933s follow-up): C# ChainsOfBindingPower
        // resets `boundCardPlayed` on BeforeTurnEnd; clearing here also
        // covers the start-of-turn invariant.
        self.chains_bound_card_played_this_turn = false;
        // Bead `sts2-rs-xk9yo`: Orichalcum/FakeOrichalcum turn-end block==0
        // snapshot — recomputed each turn-end in `end_player_turn_phase_one`,
        // cleared here for determinism.
        self.block_zero_at_turn_end = false;
        // Pre-late turn-end block snapshot for Parrying Shield — recomputed
        // each turn-end before the after_turn_end dispatch, cleared here.
        self.block_before_late_turn_end = 0;
        // Bead `sts2-rs-xk9yo` (Unmovable): C# resets the per-turn block-gain
        // history that drives the doubling at the owner's side-turn start.
        self.unmovable_card_block_plays.clear();
    }
}

// ---------------------------------------------------------------------------
// Action / StepResult
// ---------------------------------------------------------------------------

/// Top-level action surface for MCTS / scripted play.
///
/// Mirrors `RUST_PORTING_PLAN_v2` §MCTS API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Play card at `hand_index`, optionally targeted at enemy slot `target`.
    PlayCard { hand_index: u8, target: Option<u8> },
    /// End the player's turn (advance through end-of-turn pipeline).
    EndTurn,
    /// Drink potion in `slot`, optionally targeted.
    ///
    /// Bead sts2-rs-13q (P7-6) introduces [`Self::DrinkPotion`] as the
    /// canonical name; `UsePotion` is retained for back-compat with
    /// callers in `sts2-py` / `sts2-cli` / the parity shrinker that
    /// still emit the old variant. New code should emit
    /// [`Self::DrinkPotion`].
    ///
    /// GH #303: [`apply`] now *delegates* this variant to
    /// [`Self::DrinkPotion`], so it is a true alias. It previously hit a
    /// Phase-7 stub that returned `Advanced` without touching the belt —
    /// a silent no-op that told the caller the drink had happened.
    UsePotion { slot: u8, target: Option<u8> },
    /// Drink potion in `potion_slot`, optionally targeted at an enemy
    /// (`AnyEnemy` target) or a player (`AnyPlayer` target — multiplayer
    /// is single-player today, so the index is always `0` when present).
    /// Mirrors `Action::PlayCard`'s `Option<u8>` target shape.
    ///
    /// Bead sts2-rs-13q (P7-6) adds this variant. `legal_actions`
    /// emits one [`Self::DrinkPotion`] per occupied slot in
    /// [`PlayerCombatState::potions`]; character-locked potions held
    /// by the wrong character do not appear in the legal-action set
    /// (the gating predicate consumes
    /// `sts2_content::potions::PotionSpec::character_lock`), and
    /// `PotionUsage.Automatic` potions (Fairy in a Bottle) neither
    /// appear nor resolve — the apply arm rejects them as a
    /// non-consuming no-op (A8 audit §7a).
    DrinkPotion { potion_slot: u8, target: Option<u8> },
    /// Channel a specific [`Orb`] onto the player. Bead sts2-rs-spy
    /// (P7-7). This variant is NOT emitted by `legal_actions` — orb
    /// channels are driven by card effects (Zap / Coolheaded / ...),
    /// not by direct user input. The variant exists at the action
    /// surface so that scripted-play / replay fixtures and the future
    /// MCTS rollout harness can record / replay orb channels as
    /// first-class steps if needed.
    Channel { orb: Orb, target: Option<u8> },
    /// Evoke the front orb `times` times (consumed at end). Bead
    /// sts2-rs-spy (P7-7). Like [`Self::Channel`], this is not
    /// emitted by `legal_actions` — Dualcast / Multi-Cast / Quadcast
    /// route through `orbs::evoke_front_n` directly.
    Evoke { times: u32 },
    /// Resolve an open [`PendingChoice`] with the given selection indices.
    ResolveChoice { selection: SmallVec<[u8; 4]> },
}

/// Outcome of a single [`apply`] call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepResult {
    /// State advanced normally.
    Advanced,
    /// State now has a pending choice that must be resolved before further
    /// progress can be made.
    PendingChoice,
    /// Combat ended in a player victory.
    CombatWon,
    /// Combat ended in a player loss.
    CombatLost,
    /// The action referenced a target slot that does not name an alive
    /// enemy (bead `sts2-rs-vaq`). The state was NOT mutated — energy,
    /// hand, discard, and combat log are unchanged. Surfaced by
    /// [`combat_loop::play_card`] when an `Enemy`-targeted card carries
    /// a `target_index` that is out of range or names a dead creature,
    /// and by [`apply`] for a manual `DrinkPotion` aimed at a
    /// `PotionUsage.Automatic` slot (A8 audit §7a — never emitted by
    /// [`legal_actions`], so only external drivers can see it there).
    /// Callers (the CLI REPL, the Python bridge, MCTS rollouts) should
    /// treat this as a legality reject and re-prompt / re-sample.
    InvalidTarget,
}

// ---------------------------------------------------------------------------
// CombatState
// ---------------------------------------------------------------------------

/// One card stolen from the run deck by a `SwipePower` thief (the
/// `ThievingHopper`). Mirrors C# `SwipePower.Steal` →
/// `CardPileCmd.RemoveFromDeck(card.DeckVersion)`: the steal removes the
/// card from BOTH the combat pile AND the persistent run deck. C#
/// `SwipePower.BeforeDeath` restores the card to the deck (and adds a
/// `SpecialCardReward`) IFF the thief is killed; an escaped/surviving thief
/// keeps the card permanently. We record the (thief, card-identity) here at
/// steal time and flip [`Self::reclaimed`] when the thief dies; at
/// `exit_combat` only the un-reclaimed entries are removed from the run
/// `master_deck`. Bead `sts2-rs-RP13`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwipeStolen {
    /// The thief creature that stole the card (the `SwipePower` owner).
    pub thief: CreatureUid,
    /// Master-deck identity of the stolen card (paired by `(card_id,
    /// upgraded)` against `master_deck` at `exit_combat`).
    pub card_id: crate::cards::CardId,
    /// Upgrade flag of the stolen card, part of the master-deck pairing key.
    pub upgraded: bool,
    /// `true` once the thief died (C# `SwipePower.BeforeDeath` restored the
    /// card); such entries are NOT removed from the run deck.
    pub reclaimed: bool,
}

/// Top-level combat state. Plain data over arena stores — no trait objects,
/// no `Arc`, no interior mutability (plan §State).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CombatState {
    pub phase: CombatPhase,
    pub round: u16,
    pub side: Side,
    /// Combat-wide RNG. .NET-faithful (`System.Random`) — matches the
    /// game's `Rng` wrapper byte-for-byte so deck shuffles, monster HP
    /// rolls, intent rolls, and random target picks align with the C#
    /// decomp at pin v0.103.2. Migrated from `Xoshiro256PlusPlus`
    /// under bead `sts2-rs-ykmk9` (Stage 2 of the RNG audit at
    /// `docs/audits/2026-05-24-rng-audit-where-xoshiro-should-be-dotnet.md`).
    ///
    /// **Legacy / DET-only.** C# fans combat rolls out across the named
    /// streams on [`Self::run_rng`] (Shuffle / Niche / MonsterAi /
    /// CombatTargets / etc.). Parity-critical callsites are being
    /// migrated to the matching `state.run_rng.<stream>` under bead
    /// `sts2-rs-943w7`; remaining `state.rng` usages are either still
    /// unmigrated (will move) or DET-only sub-seeding sites.
    pub rng: crate::dotnet_rng::Rng,

    /// Per-stream RNG bundle mirroring C# `RunState.Rng: RunRngSet`.
    /// Each combat callsite that needs C# parity reads from the
    /// dedicated named stream (`run_rng.shuffle`, `run_rng.niche`,
    /// `run_rng.combat_targets`, `run_rng.monster_ai`, etc.) so a
    /// shuffle doesn't perturb the next intent roll and vice versa.
    /// Bead `sts2-rs-943w7`; structural blocker for ykmk9.
    pub run_rng: crate::run_rng_set::RunRngSet,

    pub creatures: CreatureStore,
    pub cards: CardStore,
    pub powers: PowerStore,
    pub relics: RelicStore,
    pub potions: PotionStore,

    pub player: PlayerCombatState,
    pub allies: SmallVec<[PlayerCombatState; 1]>,
    pub enemy_uids: SmallVec<[CreatureUid; 8]>,

    /// The card whose `OnPlay` body is executing RIGHT NOW, or `None`
    /// outside a card-effect dispatch.
    ///
    /// GH #422. Card handlers read "the card being played" through
    /// `state.player.play.last()` — an *inference* that only holds while the
    /// resolving card is still sitting on top of the Play pile. C# does not
    /// infer: `CardModel.OnPlayWrapper` runs every play iteration on `this`
    /// and reads `this.Damage` / `this.Block` directly, and its result-pile
    /// disposal is explicitly guarded on `Pile?.Type == PileType.Play`
    /// (`CardModel.cs:1975`) precisely because the card CAN leave Play
    /// mid-resolve. In the port, the suspend/resume machinery makes that
    /// reachable: a Stampede batch whose first auto-play surfaces a
    /// card-select parks its tail and keeps auto-playing, so a second
    /// auto-played card is still on the Play pile when the turn rolls over —
    /// and the turn-start Bolas / ThrummingHatchet return-to-hand sweep
    /// (which deliberately scans EVERY non-Hand pile, Play included) pulls it
    /// off. Its parked replay then re-dispatched against an EMPTY Play pile
    /// and `current_damage`'s `expect` took the whole run down.
    ///
    /// So the engine now states the identity instead of inferring it: every
    /// dispatch site sets this to the card it is playing (see
    /// [`crate::card_effects::dispatch_for_card`]) and restores the previous
    /// value on the way out, so nested auto-plays stack correctly. Readers go
    /// through [`crate::card_effects::resolving_card_uid`], which falls back
    /// to `play.last()` when unset — so every read is byte-identical whenever
    /// the invariant holds, and merely CORRECT when it doesn't.
    ///
    /// Always `None` between `apply()` calls (each dispatch restores it), so
    /// it never perturbs state hashing / equality / snapshots.
    pub resolving_card: Option<CardUid>,
    pub pending_choice: Option<PendingChoice>,
    pub pending_play: Option<PendingPlay>,
    /// Bead `sts2-rs-l2nz`: deferred-turn-setup resume flag. Mirrors
    /// Python's `_pending_turn_setup` callable at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:126,682,1672-1675`.
    /// When `start_player_turn` fires `before_side_turn_start` and a
    /// listener opens a `pending_choice`, the rest of turn-setup
    /// (block clear / energy reset / hand draw / etc.) defers and
    /// this flag is set. `apply(Action::ResolveChoice)` re-runs the
    /// remainder once the choice clears, mirroring Python's
    /// `pending_setup()` call. Python stores a callable; the Rust
    /// port stores a single boolean because the only deferred
    /// continuation at this pin is `_continue_player_turn_setup`
    /// (the second half of `start_player_turn`).
    pub pending_turn_setup_resume: bool,
    /// Bead `sts2-rs-ybtmu` (retimed): the pre-increment KnowledgeDemon curse
    /// counter (0/1/2) for the CURSE_OF_KNOWLEDGE card-select currently on
    /// screen. Stamped by the content effect_fn as the move resolves;
    /// [`crate::combat_loop::open_monster_curse_select`] reads it to build the
    /// 2-option choice (`Disintegration` + the counter's alt curse) and
    /// [`resolve_monster_curse_select`] consumes it for the Disintegration
    /// magnitude lookup (6/7/8). Set and raised in the SAME enemy-turn
    /// `perform_intent`, and consumed by the resolution that un-suspends the
    /// enemy turn, so it is always `None` at rest (not hashed — like
    /// [`Self::pending_turn_setup_resume`]).
    pub pending_curse_choice: Option<u8>,
    /// `Hook.AfterPotionUsed` deferred past the potion's own surfaced
    /// card-select. See [`PendingPotionHookResume`] (C#
    /// `PotionModel.OnUseWrapper`, `PotionModel.cs:291-342`: `OnUse` — and any
    /// choice it awaits — completes before `Hook.AfterPotionUsed` at `:338`).
    /// Set by the `Action::DrinkPotion` arm, consumed by
    /// [`resolve_choice_atomic`]. `None` at rest (not hashed, like
    /// [`Self::pending_draw_resume`]).
    pub pending_potion_hook_resume: Option<PendingPotionHookResume>,
    /// The enemy-turn perform walk suspended because a monster move opened a
    /// blocking player choice (KnowledgeDemon CURSE_OF_KNOWLEDGE). C# `await`s
    /// that card screen INSIDE the move body, so the rest of the enemy turn —
    /// the remaining monsters, `enemy_turn_end`, and the next
    /// `start_player_turn` — must not run until the player answers.
    /// `apply(Action::ResolveChoice)` consumes this via
    /// [`crate::combat_loop::resume_enemy_turn_exec`]. `None` at rest (not
    /// hashed, like [`Self::pending_turn_setup_resume`]). See
    /// [`PendingEnemyExecResume`].
    pub pending_enemy_exec_resume: Option<PendingEnemyExecResume>,
    /// Bead `sts2-rs-63mew`: a draw suspended mid-reshuffle by an
    /// `after_shuffle` player choice (Stratagem). C# awaits the
    /// `StratagemPower.AfterShuffle` grid-select INSIDE
    /// `CardPileCmd.Draw`'s per-card loop (`ShuffleIfNecessary` runs
    /// before each card is taken), so the remaining draws happen only
    /// after the pick resolves. `Some` carries the not-yet-drawn
    /// remainder; `apply(Action::ResolveChoice)` resumes the draw once
    /// the choice clears. Always `None` at rest (not hashed — like
    /// [`Self::pending_turn_setup_resume`]).
    pub pending_draw_resume: Option<PendingDrawResume>,
    /// Bead `sts2-rs-snecko-suspend` (#180): a `SneckoOil` cost-randomize
    /// deferred because its 7-card draw suspended mid-reshuffle on a Stratagem
    /// `AfterShuffle` grid-select. C# `SneckoOil.OnUse` awaits the full draw
    /// BEFORE the cost-randomize `foreach`, so when the Rust draw suspends the
    /// randomize must wait for the complete hand. Set by `drink_snecko_oil`,
    /// drained by `apply(Action::ResolveChoice)` once `pending_draw_resume`
    /// fully clears. Always `false` at rest (not hashed, like
    /// [`Self::pending_draw_resume`]).
    pub pending_snecko_cost_randomize: bool,
    /// Turn-start `AfterPlayerTurnStart` cluster suspended mid-Mayhem-auto-play
    /// by a Stratagem `AfterShuffle` grid-select (bead `sts2-rs-o0rz3`).
    /// Consumed by `apply(Action::ResolveChoice)`. `None` at rest (not hashed,
    /// like [`Self::pending_draw_resume`]).
    pub pending_cluster_resume: Option<PendingClusterResume>,
    /// Turn-start hand draw suspended by a `ForegoneConclusionPower.BeforeHandDraw`
    /// surfacing card-select. C# `ForegoneConclusionPower.BeforeHandDraw` is
    /// awaited by the turn-start sequence: the FromSimpleGrid select runs +
    /// `CardPileCmd.Add(selected, Hand)` + `PowerCmd.Remove(this)` all complete
    /// BEFORE the natural hand-draw of 5 (so hand = 2 FC cards + 5 drawn = 7).
    /// Rust's `dispatch_before_hand_draw_powers` surfaces the FC select and
    /// returns without moving the cards (deferred to the resolver), so the
    /// natural draw must be SUSPENDED until the select resolves — otherwise it
    /// over-drains the draw pile (the FC cards are still in it) and the FC tail
    /// then yields hand=5. `Some(draw_count)` carries the stashed natural-draw
    /// count; `apply(Action::ResolveChoice)` runs the remaining turn-setup tail
    /// (relic before_hand_draw, card returns, the natural draw,
    /// `finish_player_turn_setup`) once the FC select resolves. Always
    /// `None` at rest (not hashed, like [`Self::pending_draw_resume`]).
    /// Bead `sts2-rs-fc-handdraw-suspend`.
    pub pending_hand_draw_resume: Option<i32>,
    /// Turn-start hand draw suspended one stage LATER than
    /// [`Self::pending_hand_draw_resume`] — by a *relic* whose
    /// `before_hand_draw` opens a card-select. Toolbox is the only one today
    /// (`Toolbox.cs:22-33`: `await CardSelectCmd.FromChooseACardScreen` then
    /// `AddGeneratedCardToCombat(.., Hand)`, all inside the awaited
    /// `Hook.BeforeHandDraw` at `CombatManager.cs:654`), so the chosen card
    /// reaches hand BEFORE the natural draw of 5.
    ///
    /// Two markers rather than one because the resume points differ: the FC
    /// marker re-enters at the relic channel, this one re-enters *after* it.
    /// Collapsing them would re-fire `dispatch_before_hand_draw` — harmless
    /// for Toolbox itself (its per-combat `Relic.counter` latch absorbs the
    /// second call) but not for the Bolas/ThrummingHatchet returns and the
    /// natural draw that follow. They are mutually exclusive in practice: the
    /// FC leg returns before the relic channel runs.
    ///
    /// `Some(draw_count)` carries the stashed natural-draw count. Always
    /// `None` at rest (not hashed, like [`Self::pending_hand_draw_resume`]).
    /// GH #369.
    pub pending_relic_hand_draw_resume: Option<i32>,
    /// End-of-turn flush suspended by a Well-Laid Plans retain card-select
    /// (bead `sts2-rs-wlp-retain-select`). C# `WellLaidPlansPower.BeforeFlushLate`
    /// awaits an interactive `CardSelectCmd.FromHand(0, Amount)` BEFORE the hand
    /// flush; the chosen cards each get single-turn retain. Rust surfaces that
    /// select as a [`ChoiceKind::RetainAtTurnEnd`] `pending_choice` and parks the
    /// rest of `end_player_turn_phase_two` (the hand flush, per-turn cleanup,
    /// `after_turn_end` hooks, extra-turn check, phase advance) plus the
    /// remaining `end_turn_dispatch` chain steps (enemy turns + next player
    /// turn-start) behind this flag. `apply(Action::ResolveChoice)` consumes it
    /// after the retain select resolves, running
    /// [`crate::combat_loop::resume_wlp_retain_flush`]. Always `false` at rest
    /// (not hashed, like [`Self::pending_turn_setup_resume`]).
    pub pending_wlp_retain_resume: bool,
    /// `end_player_turn_phase_one` suspended because a listener auto-play
    /// surfaced a card-select — and WHERE it stopped (parity bug
    /// `combat_select_ironclad` follow-up; bead `sts2-rs-eirvo`).
    ///
    /// C# `StampedePower.AfterAutoPostPlayPhaseEntered` `await CardCmd.AutoPlay`s
    /// a random hand Attack at end of turn; an auto-played `SeekerStrike` then
    /// awaits an interactive `CardSelectCmd.FromCombatPile` draw-select INSIDE
    /// its `OnPlay`, blocking the whole turn transition (the rest of the
    /// Stampede batch, the Howl replay, `BeforeTurnEnd`, the hand discard, the
    /// enemy turn, the next turn-start draw) until the player answers. Rust
    /// surfaces that select as a `pending_choice` + parks the auto-play tail on
    /// [`Self::pending_autoplay_resume`]; without this marker the end-turn
    /// dispatch chain ran to completion WHILE the choice was pending, stranding
    /// the auto-played card in the Play pile (it never reached discard) and
    /// drawing the next turn's hand before the select resolved — Ironclad seed
    /// 3859 lost `SEEKER_STRIKE` (card non-conservation 12→11).
    ///
    /// When set, `apply(Action::ResolveChoice)` finishes the suspended auto-play
    /// and then re-enters the chain at the recorded point. `None` at rest (not
    /// hashed, like [`Self::pending_turn_setup_resume`]).
    pub pending_end_turn_resume: Option<EndTurnResume>,
    /// An auto-play (Havoc / Cascade / Catastrophe / Mayhem / DistilledChaos /
    /// Stampede) suspended because the card it dispatched opened a SURFACING
    /// card-select from its `OnPlay` (parity bug `combat_select_ironclad`).
    /// `apply(Action::ResolveChoice)` consumes it once the surfaced choice
    /// resolves, finishing the suspended card's tail then the remaining
    /// collected cards. `None` at rest (not hashed, like
    /// [`Self::pending_cluster_resume`]). See [`PendingAutoplayResume`].
    pub pending_autoplay_resume: Option<PendingAutoplayResume>,
    /// Mirror of the headless `HeadlessCardSelector.AutoResolveInline`
    /// (`RunSimulator.cs:4803`): while a combat auto-play is being RESUMED
    /// after a surfaced card-select, any FURTHER nested choice opened by the
    /// resumed play resolves INLINE at the option-0 prefix rather than
    /// surfacing again (C# `DoSelectCards`, `RunSimulator.cs:2165`). Raised by
    /// the resume path, lowered when it returns. `false` at rest (not hashed).
    pub autoplay_inline_resolve: bool,
    pub combat_log: TurnEvents,

    /// Cards stolen from the run deck this combat by `SwipePower` thieves
    /// (`ThievingHopper`). Drained at `exit_combat` to remove the
    /// un-reclaimed cards from the run `master_deck`. See [`SwipeStolen`].
    /// Bead `sts2-rs-RP13`.
    pub swipe_stolen: Vec<SwipeStolen>,

    /// Count of enemy creatures that FLED via `Intent::Escape` this combat.
    /// Mirrors C# `CombatState.EscapedCreatures.Count`; the combat gold reward
    /// is scaled by `GoldProportion = 1 - escaped / total_monsters`
    /// (`CombatRoom.cs:216`) at `exit_combat`. Bead `sts2-rs-RP15`.
    pub escaped_creatures: u32,

    /// Gold recovered this combat by KILLING a `HeistPower` carrier (C#
    /// `HeistPower.BeforeDeath` → `CombatRoom.AddExtraReward(player, new
    /// GoldReward(Amount, …, wasGoldStolenBack: true))`). The GremlinMerc's
    /// `ThieveryPower` takes gold off the player mid-combat and, when the merc
    /// dies, `SurprisePower.AfterDeath` hands the running total to the spawned
    /// FatGremlin as `HeistPower`; killing THAT gremlin before it flees hands
    /// the gold back. Accumulated at the damage-death chokepoint (so escapes,
    /// which never reach it, keep the gold) and settled by `exit_combat` on a
    /// won combat — the reward screen a lost combat never shows. GH #604.
    pub heist_recovered_gold: i32,

    /// Set true when a `BattlewornDummyTimeLimit` timer expires and the
    /// dummy flees instead of being killed (C#
    /// `BattlewornDummyTimeLimitPower.AfterTurnEnd` → set
    /// `BattlewornDummyEventEncounter.RanOutOfTime = true` +
    /// `CreatureCmd.Escape`). The event→combat resume path reads this back
    /// (via `exit_combat`) to suppress the victory reward — a timed-out
    /// dummy fight yields nothing. Bead `sts2-rs-ck9z6` (GH #166).
    pub event_combat_timed_out: bool,

    /// Combat-time mirror of the run's gold balance. C# gold lives on the
    /// Player and is read/spent DURING combat by relics like `SealOfGold`
    /// (`SealOfGold.cs` `AfterSideTurnStart`: while gold ≥ 5, spend 5 for +1
    /// energy). The combat engine has no `&mut RunState` back-edge, so
    /// `enter_combat` snapshots `RunState.gold` here, combat-side effects mutate
    /// it, the in-combat gold view reads it, and `exit_combat` writes it back to
    /// `RunState.gold`. For combats with no gold-touching effect it stays equal
    /// to the run gold (no-op write-back). Bead `sts2-rs-RP17`.
    pub gold: i32,

    pub is_over: bool,
    pub player_won: bool,

    /// Ascension level (0–20). Bead `sts2-rs-l7l` (P7-1) introduces the
    /// field; downstream P7-* and Act-4 beads consume it for HP scaling,
    /// reward gating, and modifier toggles. Defaults to `0` (A0). Mirrors
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_state.py::RunState.ascension_level`.
    /// Python keeps the value on `RunState`; for the Rust engine we land it
    /// directly on `CombatState` so the combat-loop dispatchers don't need
    /// a back-reference to a run object.
    pub ascension: u8,

    /// The player's master deck — the run-scoped card collection that
    /// persists across combats. Bead `sts2-rs-anx` (P7-surf): deck-add
    /// event-bus surface required by the `BingBong` / `DarkstonePeriapt`
    /// event-reward relics (audit doc
    /// `docs/reality-checks/2026-05-12-event-reward-audit.md` §2).
    ///
    /// Mirrors Python's `RunState.deck: list[CardInstance]` at
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_state.py`. Python keeps
    /// the deck on `RunState`; Rust's `RunState` POD has not landed yet
    /// (bead `sts2-rs-62t` will plumb it), so the field lives on
    /// [`CombatState`] alongside [`Self::ascension`] following the same
    /// precedent. When `RunState` lands the field migrates without a
    /// public API change — callers go through
    /// [`Self::add_to_master_deck`].
    ///
    /// Storage is `Vec<CardId>` rather than `Vec<Card>` because
    /// run-scope identity is the `CardId` discriminant; per-instance
    /// state (`combat_vars` / `enchantments`) lives in [`Self::cards`]
    /// when a card is in a combat pile. `BingBong`'s
    /// `duplicate_last_added_card` only needs the id; richer
    /// `CardInstance.clone(seed)` semantics land with bead
    /// `sts2-rs-3f8`.
    pub master_deck: Vec<crate::cards::CardId>,

    /// Currently-resolving attack frame, or `None` when no attack is
    /// in flight. Mirrors Python's `combat._attack_context_stack[-1]`
    /// (`sts2-rl-agent@01c094d:sts2_env/core/combat.py::CombatState
    /// .active_attack`) but stored as a single frame rather than a
    /// stack — nesting is not exercised by any pinned fixture at this
    /// pin.
    ///
    /// Read-only access through [`Self::active_attack()`]; mutators are
    /// [`Self::begin_attack`] / [`Self::end_attack`] /
    /// [`Self::record_attack_hit`]. Bead `sts2-rs-c7ak` (P7-surf).
    /// Combat-loop auto-push/pop wiring is deferred to a follow-up
    /// bead; until then the field is only set by tests + direct
    /// callers, so `HandDrill`'s listener is observationally inert in
    /// production combat.
    pub active_attack: Option<ActiveAttack>,

    /// The most-recently-appended entry in [`Self::master_deck`].
    /// `None` when no card has been added yet. Bead `sts2-rs-anx`.
    ///
    /// Mirrors the implicit `self.deck[-1]` access at
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_state.py::
    /// duplicate_last_added_card` (lines 463-467). Python doesn't
    /// materialize a `last_added_card` slot — it just reads
    /// `self.deck[-1]` directly. Rust caches the id eagerly so
    /// listeners inspecting "what was just added?" can read it without
    /// re-indexing the deck and so the value is well-defined inside
    /// re-entrant duplicate-into-the-bus listeners (see `BingBong`'s
    /// `_skip_next` re-entry guard at `shop_event.py:828-836`).
    pub last_added_card: Option<crate::cards::CardId>,

    /// Whether this combat takes place in an Elite room. Mirrors
    /// Python's `combat.is_elite` attribute
    /// (`sts2-rl-agent@01c094d:sts2_env/core/combat.py`), set by the
    /// run/map layer at combat setup from
    /// `RoomType::Elite`. Defaults to `false`. Surface lifted by bead
    /// `sts2-rs-jvk3` for `SwordOfStone`, which reads it on
    /// `after_combat_victory` to gate the elite-defeat counter
    /// (`shop_event.py:2755`).
    pub is_elite: bool,

    /// Whether this combat takes place in a Boss room. Mirrors Python's
    /// `combat.room.is_boss` / `room.room_type == RoomType.BOSS` check,
    /// set by the run/map layer at combat setup from
    /// [`crate::runstate::RoomType::Boss`]. Defaults to `false`. Read
    /// on `before_combat_start` by `StoneCracker` to gate the
    /// draw-pile-upgrade behavior (Python `uncommon.py::StoneCracker`).
    pub is_boss: bool,

    /// Act index (1, 2, or 3) snapshotted from
    /// [`crate::runstate::RunState::act_index`] at combat setup, so
    /// combat-side hooks can act-gate without threading the run state
    /// through every listener invocation. Defaults to `1` (Act 1).
    /// Until the run/map layer lands, this is populated by test
    /// fixtures and by the eventual `enter_combat(rs, encounter)`
    /// bridge (bead `sts2-rs-tjn0v`). Read by `PumpkinCandle` and
    /// other act-gated relic arms in
    /// [`crate::hooks::modify_max_energy`]. Bead `sts2-rs-mi67e`.
    pub act_index: u8,

    /// Cumulative count of unspent energy at end-of-turn, summed across
    /// every player turn in this combat. Bead `sts2-rs-ob7h` (Wave 2 of
    /// `docs/plans/2026-05-17-mcts-leaf-eval-and-search-improvements.md`
    /// §10): prereq for `sts2-rs-95o9` (eval-energy-cards-port) which
    /// re-introduces the lightspeed `energy_wasted * -0.2` term in
    /// [`evaluate_in_progress`]. Reset to `0` in
    /// [`crate::combat_loop::start_combat`]; incremented in
    /// [`crate::combat_loop::end_player_turn_phase_one`] by the current
    /// `player.energy` value before the end-turn dispatch chain runs.
    /// No Python source-of-truth — this is a Rust-side measurement
    /// counter for the MCTS leaf-eval heuristic.
    pub energy_wasted: i32,

    /// Cumulative count of cards drawn into hand across every draw
    /// event in this combat. Bead `sts2-rs-o05r` (sister of
    /// `sts2-rs-ob7h`, prereq for `sts2-rs-95o9`
    /// eval-energy-cards-port): re-introduces the lightspeed
    /// `cards_drawn * 0.1` term in [`evaluate_in_progress`]. Reset
    /// to `0` in [`crate::combat_loop::start_combat`]; incremented
    /// in [`CombatState::draw_into_hand`] by the actual number of
    /// cards moved into hand (post reshuffle, pre any on-draw side
    /// effects that might burn them). No Python source-of-truth.
    /// `u16` is plenty: STS combats rarely cross ~200 draws even in
    /// degenerate draw-infinites; the counter saturates rather
    /// than wraps on overflow so a runaway loop reads as `u16::MAX`
    /// instead of an aliased small number.
    pub cards_drawn: u16,

    /// Bead `sts2-rs-3bie`: combat→run gold-loss intent queue.
    ///
    /// Python at pin `01c094d` mutates `combat.gold` inline during the
    /// turn-end-in-hand handler for DEBT (`combat.py:2403-2405`:
    /// `self.gold = max(0, self.gold - min(card.effect_vars["gold"], self.gold))`).
    /// DEAD / obsolete (TODO: remove). This accumulator predated the
    /// live combat [`Self::gold`] mirror and was never wired to any
    /// production drain — [`crate::runstate::RunState::apply_pending_combat_gold_loss`]
    /// has no caller other than its own unit tests, and `exit_combat`
    /// reconciles via `rs.gold = combat.gold` without draining it. DEBT's
    /// turn-end gold loss now mutates the live `state.gold` directly (see
    /// the `CardId::Debt` arm of `execute_turn_end_in_hand_effect`,
    /// mirroring C# `Debt.OnTurnEndInHand` -> `PlayerCmd.LoseGold`), so
    /// nothing writes a non-zero value here anymore. Kept only to avoid
    /// touching ~40 struct-literal initializers; safe to delete in a
    /// mechanical follow-up pass.
    ///
    /// Defaults to `0`.
    pub pending_run_gold_loss: i32,

    /// The uid of the single most-recently-drawn card, captured at the
    /// instant it entered the hand by the single-card draw paths
    /// ([`Self::draw_suspending`] / [`crate::cards::draw_via_state`]).
    ///
    /// C# `CardPileCmd.Draw(ctx, owner)` RETURNS the drawn `CardModel`,
    /// so a caller learns the card's identity/type even when an
    /// `AfterCardDrawnEarly` rider (HellraiserPower auto-playing a drawn
    /// Strike) immediately yanks it back out of the hand. Rust's draw
    /// primitives return only a *count*, so a caller that inferred
    /// "did I draw?" from a hand-length delta was fooled by that
    /// removal (Pillage's do/while attack-draw loop broke one card early
    /// under Hellraiser — bead `sts2-rs-pillage-hellraiser`). This slot
    /// preserves the C# return contract for the single-card path. It is
    /// transient (set on each draw, read immediately by the caller) and
    /// `None` when the last draw moved no card. Not in the serialized
    /// parity surface nor the MCTS `state_hash`.
    pub last_drawn_card: Option<crate::CardUid>,

    /// The search's value of the potion belt (sts2-agent fork): when set,
    /// every evaluation of this state and of the states derived from it (the
    /// win terminal, both depth-cap leaves) prices the held potions by it
    /// instead of the process-wide `λ` ([`belt_hp_equiv`]). Set on a search's
    /// root copy; shared, read-only. Not in the serialized surface nor the
    /// MCTS `state_hash` (constant within a search).
    pub belt_value: Option<std::sync::Arc<crate::belt_value::BeltValue>>,
}

impl Default for CombatState {
    fn default() -> Self {
        Self {
            phase: CombatPhase::Setup,
            round: 0,
            side: Side::Player,
            rng: crate::dotnet_rng::Rng::new(0),
            run_rng: crate::run_rng_set::RunRngSet::new(0),

            creatures: CreatureStore::new(),
            cards: CardStore::new(),
            powers: PowerStore::new(),
            relics: RelicStore::new(),
            potions: PotionStore::new(),

            player: PlayerCombatState::default(),
            allies: SmallVec::new(),
            enemy_uids: SmallVec::new(),

            resolving_card: None,
            pending_choice: None,
            pending_play: None,
            pending_turn_setup_resume: false,
            pending_curse_choice: None,
            pending_potion_hook_resume: None,
            pending_enemy_exec_resume: None,
            pending_draw_resume: None,
            pending_snecko_cost_randomize: false,
            pending_cluster_resume: None,
            pending_hand_draw_resume: None,
            pending_relic_hand_draw_resume: None,
            pending_wlp_retain_resume: false,
            pending_end_turn_resume: None,
            pending_autoplay_resume: None,
            autoplay_inline_resolve: false,
            combat_log: TurnEvents::default(),
            swipe_stolen: Vec::new(),
            escaped_creatures: 0,
            heist_recovered_gold: 0,
            event_combat_timed_out: false,
            gold: 0,

            is_over: false,
            player_won: false,

            ascension: 0,
            master_deck: Vec::new(),
            last_added_card: None,
            is_elite: false,
            is_boss: false,
            act_index: 1,
            active_attack: None,
            energy_wasted: 0,
            cards_drawn: 0,
            pending_run_gold_loss: 0,
            last_drawn_card: None,
            belt_value: None,
        }
    }
}

impl CombatState {
    /// Grow the targeted creature's `max_hp` by `amount` and broadcast
    /// the [`AfterMaxHpChangedFn`](crate::relic_effects::AfterMaxHpChangedFn)
    /// event to every relic listener registered on the channel.
    ///
    /// Mirrors `sts2-rl-agent@01c094d:sts2_env/core/combat.py::
    /// CombatState.gain_max_hp`, which calls `owner.gain_max_hp(amount)`
    /// and `state.player_state.gain_max_hp(amount)` in lock-step. The
    /// broadcast itself is a Rust-side affordance: Python derives the
    /// equivalent fan-out by calling `owner.gain_max_hp` directly from
    /// relic handlers, but the Rust port routes through a typed bus so
    /// downstream relics (`BigMushroom`, `ChosenCheese`,
    /// `DarkstonePeriapt`, `FakeMango`) can register handlers without
    /// each one re-reading the creature store.
    ///
    /// Returns the delta actually applied (always `>= 0`; negative /
    /// zero `amount` is a no-op and no event is broadcast). Bead
    /// `sts2-rs-tcf` (P7-surf).
    pub fn creature_gain_max_hp(&mut self, owner: CreatureUid, amount: i32) -> i32 {
        if amount <= 0 {
            return 0;
        }
        let Some(idx) = self.creatures.iter().position(|c| c.id == owner.0) else {
            panic!(
                "creature_gain_max_hp: creature uid {} not found",
                owner.get()
            )
        };
        let delta = self.creatures[idx].gain_max_hp(amount);
        // Bead `sts2-rs-73up8` (d03 Phase-2 prereq): mirror the
        // post-state back to `state.player.{current_hp, max_hp}` when
        // the owner is the player's mirror uid — `Creature::gain_max_hp`
        // bumps BOTH max_hp and current_hp, so both legacy fields must
        // re-sync. Symmetric with the read-side fallback path in
        // `player_hp` / `player_max_hp` and with `creature_heal`'s
        // existing mirror.
        let is_player = self.player.creature_uid == Some(owner);
        if is_player {
            self.player.max_hp = self.creatures[idx].max_hp;
            self.player.current_hp = self.creatures[idx].current_hp;
        }
        if delta != 0 {
            let ctx = crate::relic_effects::MaxHpChangeCtx { owner, delta };
            let _ = crate::relic_effects::dispatch_after_max_hp_changed(self, &ctx);
            // Bead `sts2-rs-jlc` (n3w-b): gain-only sibling channel.
            // Always fires here because `gain_max_hp` short-circuits on
            // non-positive `amount`, so `delta > 0` when we reach this.
            let _ = crate::relic_effects::dispatch_after_max_hp_gained(self, &ctx);
            // C# `RedSkull.AfterCurrentHpChanged` fires on a CURRENT-hp change;
            // `Creature::gain_max_hp` bumps current HP alongside max, so the
            // player's current HP moved (`delta > 0`) and the 50% threshold
            // must be re-probed (mirror of the `creature_heal` re-probe).
            if is_player
                && crate::relic_effects::helpers::player_owns_relic(
                    self,
                    crate::RelicKind::RedSkull,
                )
            {
                crate::relic_effects::helpers::red_skull_check_hp(self);
            }
        }
        delta
    }

    /// Shrink the targeted creature's `max_hp` by `amount` (floored at 1)
    /// and broadcast the
    /// [`AfterMaxHpChangedFn`](crate::relic_effects::AfterMaxHpChangedFn)
    /// event with the negative delta.
    ///
    /// Returns the delta actually applied (always `<= 0`; negative /
    /// zero `amount` is a no-op and no event is broadcast). Bead
    /// `sts2-rs-tcf` (P7-surf).
    pub fn creature_lose_max_hp(&mut self, owner: CreatureUid, amount: i32) -> i32 {
        if amount <= 0 {
            return 0;
        }
        let Some(idx) = self.creatures.iter().position(|c| c.id == owner.0) else {
            panic!(
                "creature_lose_max_hp: creature uid {} not found",
                owner.get()
            )
        };
        // Capture the pre-shrink current HP so a max-hp clamp that pulls
        // current HP down can be detected (C# `AfterCurrentHpChanged` fires
        // only on a CURRENT-hp move, so a non-clamping max loss must NOT
        // re-probe RedSkull — otherwise we over-fire vs the decomp).
        let before_current_hp = self.creatures[idx].current_hp;
        let delta = self.creatures[idx].lose_max_hp(amount);
        let clamped_current_hp = self.creatures[idx].current_hp;
        // Bead `sts2-rs-73up8` (d03 Phase-2 prereq): mirror
        // `Creature::lose_max_hp`'s post-state — max_hp shrinks and
        // current_hp may be clamped down — back into the player legacy
        // fields when this uid is the player's mirror. Symmetric with
        // `creature_gain_max_hp` and `creature_heal`.
        let is_player = self.player.creature_uid == Some(owner);
        if is_player {
            self.player.max_hp = self.creatures[idx].max_hp;
            self.player.current_hp = self.creatures[idx].current_hp;
        }
        if delta != 0 {
            let ctx = crate::relic_effects::MaxHpChangeCtx { owner, delta };
            let _ = crate::relic_effects::dispatch_after_max_hp_changed(self, &ctx);
            // Only re-probe RedSkull if the max-hp shrink actually clamped the
            // player's CURRENT hp down (PaperCuts LoseMaxHp below current HP) —
            // matching C#'s current-hp-change trigger, not a max-hp trigger.
            if is_player
                && clamped_current_hp != before_current_hp
                && crate::relic_effects::helpers::player_owns_relic(
                    self,
                    crate::RelicKind::RedSkull,
                )
            {
                crate::relic_effects::helpers::red_skull_check_hp(self);
            }
        }
        delta
    }

    /// Copy `state.player.current_hp` into the player's mirror
    /// [`Creature`] row when [`PlayerCombatState::creature_uid`] is set.
    /// No-op when the mirror isn't installed (pre-d03 fixtures) or the
    /// mirror Creature isn't in `state.creatures` (test fixtures that
    /// build a partial player slot).
    ///
    /// Bead `sts2-rs-73up8` (d03 Phase-2 prereq). Call after every
    /// direct write to `state.player.current_hp` so the Phase-1 mirror
    /// stays coherent. The sibling [`Self::sync_creature_block_from_player`]
    /// and [`Self::sync_creature_max_hp_from_player`] handle the
    /// block / max_hp fields the same way — each is per-field so that
    /// a write site that only touched one field doesn't accidentally
    /// stomp a freshly-set value on another field in the mirror
    /// Creature (an issue that surfaced in the colorless card test
    /// fixtures, which set Creature HP independently of
    /// `state.player.current_hp`).
    pub fn sync_creature_current_hp_from_player(&mut self) {
        let hp = self.player.current_hp;
        self.with_player_mirror(|c| c.current_hp = hp);
    }

    /// Copy `state.player.block` into the player's mirror
    /// [`Creature`] row. See [`Self::sync_creature_current_hp_from_player`]
    /// for the design rationale (per-field micro-sync).
    pub fn sync_creature_block_from_player(&mut self) {
        let block = self.player.block;
        self.with_player_mirror(|c| c.block = block);
    }

    /// Copy `state.player.max_hp` into the player's mirror
    /// [`Creature`] row. See [`Self::sync_creature_current_hp_from_player`]
    /// for the design rationale (per-field micro-sync).
    pub fn sync_creature_max_hp_from_player(&mut self) {
        let max_hp = self.player.max_hp;
        self.with_player_mirror(|c| c.max_hp = max_hp);
    }

    /// Run `f` on the player's mirror [`Creature`] row, if the player has
    /// one (`player.creature_uid` set AND present in `creatures`) — the
    /// shared guard of the three `sync_creature_*_from_player` micro-syncs
    /// above. No-op otherwise (legacy states without a mirror).
    fn with_player_mirror(&mut self, f: impl FnOnce(&mut Creature)) {
        let Some(uid) = self.player.creature_uid else {
            return;
        };
        let Some(idx) = creature_pos(&self.creatures, uid.0) else {
            return;
        };
        f(&mut self.creatures[idx]);
    }

    /// Heal the [`Creature`] at `uid` by `amount`, clamped to `max_hp`.
    /// Returns the signed delta actually applied (always `>= 0`; zero /
    /// negative `amount` is a no-op, dead targets are a no-op).
    ///
    /// Mirrors `sts2-rl-agent@01c094d:sts2_env/core/creature.py::Creature.heal`:
    /// ```text
    /// if amount <= 0 or self.is_dead: return 0
    /// before = self.current_hp
    /// self.current_hp = min(self.current_hp + amount, self.max_hp)
    /// return self.current_hp - before
    /// ```
    ///
    /// **Player-slot mirror.** When `uid == self.player.creature_uid`,
    /// the legacy [`PlayerCombatState::current_hp`] field is also
    /// updated so the Phase-1 d03 read-side source of truth stays
    /// coherent with the Creature row. No broadcast — Python's
    /// `Creature.heal` is a plain mutation with no event hook.
    ///
    /// Returns `0` if the uid is missing. Bead `sts2-rs-v7mp`
    /// (combat-side helper for `FakeBloodVial`).
    pub fn creature_heal(&mut self, uid: CreatureUid, amount: i32) -> i32 {
        if amount <= 0 {
            return 0;
        }
        let Some(idx) = self.creatures.iter().position(|c| c.id == uid.0) else {
            return 0;
        };
        let creature = &mut self.creatures[idx];
        if creature.current_hp <= 0 {
            return 0;
        }
        let before = creature.current_hp;
        creature.current_hp = (creature.current_hp + amount).min(creature.max_hp);
        let applied = creature.current_hp - before;
        // Mirror the player-slot legacy field so accessors that still
        // read `state.player.current_hp` see the heal.
        if self.player.creature_uid == Some(uid) {
            self.player.current_hp = creature.current_hp;
            // C# `RedSkull.AfterCurrentHpChanged` fires on EVERY player HP
            // change — including a heal that lifts HP back above the 50%
            // threshold, which removes the +3 Strength. Rust previously only
            // re-probed on `after_damage_received` (HP drops), so a heal
            // (FairyInABottle, BloodVial, Regen, rest, …) left the bonus
            // Strength stuck (sweep seed 1763: FAIRY_IN_A_BOTTLE healed past
            // 50% but Rust kept RedSkull's STRENGTH 3). Re-probe here, gated
            // on ownership — `red_skull_check_hp` applies Strength on the
            // low-HP transition unconditionally, so it must NOT run without
            // the relic.
            if applied > 0
                && crate::relic_effects::helpers::player_owns_relic(
                    self,
                    crate::RelicKind::RedSkull,
                )
            {
                crate::relic_effects::helpers::red_skull_check_hp(self);
            }
        }
        applied
    }

    /// Pick a random alive enemy via `self.rng` (uniform over
    /// `enemy_uids` filtered to `current_hp > 0`). Returns `None` if
    /// no enemy is alive.
    ///
    /// Mirrors `combat.random_enemy_of(primary_player)` semantics from
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:1578-1580`:
    /// `self.rng.choice(get_enemies_of(player))`. The player branch of
    /// `get_enemies_of` returns `alive_enemies`, which is the iteration
    /// shape this helper mirrors.
    ///
    /// Bead `sts2-rs-v7mp` (combat-side helper for `ForgottenSoul`).
    pub fn get_random_enemy(&mut self) -> Option<CreatureUid> {
        let alive: smallvec::SmallVec<[CreatureUid; 8]> = self
            .enemy_uids
            .iter()
            .copied()
            .filter(|uid| {
                self.creatures
                    .iter()
                    .any(|c| c.id == uid.0 && c.current_hp > 0)
            })
            .collect();
        if alive.is_empty() {
            return None;
        }
        // C# random-target picks read the dedicated `CombatTargets`
        // stream via `Rng.CombatTargets.NextItem(HittableEnemies)`
        // (e.g. `CardCmd.cs:60`, `BeatDown.cs:34`, `LightningOrb.cs:52`).
        // `NextItem(list)` indexes via `NextInt(0, count)`, matching
        // `next_int_exclusive`. Bead `sts2-rs-943w7`.
        let idx = self
            .run_rng
            .combat_targets
            .next_int_exclusive(0, alive.len() as i32) as usize;
        Some(alive[idx])
    }

    /// C# `Hook.ShouldCreatureBeRemovedFromCombatAfterDeath` analog
    /// (`Hook.cs:1696`): a dead enemy leaves the active `Enemies` list
    /// (`CombatState.RemoveCreature`, `CombatState.cs:419`) *unless* it
    /// owns a power whose override returns `false` for its own owner.
    ///
    /// In v0.103.2 the six overrides — `IllusionPower` (`:96`),
    /// `AdaptablePower` (`:55`), `SteamEruptionPower` (`:28`),
    /// `ReattachPower` (`:84`), `DieForYouPower` (`:37`), and
    /// `PainfulStabsPower` (`:28`) — are all `return creature != Owner`,
    /// i.e. they retain *only their own owner's* corpse; another
    /// creature's such power does not retain this one. The hook OR-folds
    /// across all listeners, so a single retaining power on the dying
    /// creature flips the result to "retain".
    ///
    /// Callers gate on death themselves (`current_hp <= 0`); alive
    /// creatures are never removed. The default (`AbstractModel.cs:984`)
    /// is `true` (remove). Returns `true` (remove) for an unknown uid.
    ///
    /// Bead `sts2-rs-c8why`. This is the predicate behind
    /// [`Self::active_enemy_uids`]; the canonical `enemy_uids` substrate
    /// is **not** mutated — see that method's docs for the rationale.
    #[must_use]
    pub fn should_remove_from_combat_on_death(&self, uid: CreatureUid) -> bool {
        /// Powers whose owner is retained in the encounter array after
        /// death (C# overrides that return `false` for their own owner).
        const RETAIN_CORPSE_POWERS: [PowerKind; 6] = [
            PowerKind::Illusion,
            PowerKind::Adaptable,
            PowerKind::SteamEruption,
            PowerKind::Reattach,
            PowerKind::DieForYou,
            PowerKind::PainfulStabs,
        ];
        let Some(c) = self.creatures.iter().find(|c| c.id == uid.0) else {
            return true;
        };
        let retained = c.powers.iter().any(|puid| {
            self.powers
                .iter()
                .any(|p| p.id == puid.0 && p.amount != 0 && RETAIN_CORPSE_POWERS.contains(&p.kind))
        });
        !retained
    }

    /// Derived **C#-faithful "Enemies" projection** — the active roster
    /// after `RemoveCreature` has dropped dead-and-removable corpses.
    /// Equals `enemy_uids` minus any creature that is both dead
    /// (`current_hp <= 0`) and removable per
    /// [`Self::should_remove_from_combat_on_death`].
    ///
    /// **Design (bead `sts2-rs-c8why`, Option I).** The canonical
    /// `enemy_uids` list is left *stable* — corpses keep their slot for
    /// the whole combat. That stable order is the substrate the NN
    /// feature encoder (`eval_wide`) and MCTS target ordinals key off, so
    /// not mutating it means an enemy never reshuffles its NN slot when a
    /// sibling dies (which would otherwise silently invalidate trained
    /// value/policy nets). This accessor is the "flexible pointer view":
    /// a *derived* compacted projection for consumers that need to match
    /// C#'s `Enemies` semantics (parity emission, C#-ordinal targeting),
    /// computed on demand rather than stored. The two index spaces are
    /// bridged by [`Self::active_ordinal_of_enemy`].
    #[must_use]
    pub fn active_enemy_uids(&self) -> smallvec::SmallVec<[CreatureUid; 8]> {
        self.enemy_uids
            .iter()
            .copied()
            .filter(|uid| !self.is_removed_corpse(*uid))
            .collect()
    }

    /// Whether `uid` is a dead enemy that C# would have removed from the
    /// `Enemies` list (dead **and** removable). Alive enemies and
    /// dead-but-retained corpses (e.g. `IllusionPower` owners like a slain
    /// EyeWithTeeth) return `false`. Bead `sts2-rs-c8why`.
    #[must_use]
    pub fn is_removed_corpse(&self, uid: CreatureUid) -> bool {
        let dead = self
            .creatures
            .iter()
            .any(|c| c.id == uid.0 && c.current_hp <= 0);
        dead && self.should_remove_from_combat_on_death(uid)
    }

    /// Bridge from a **stable** `enemy_uids` index (the NN-slot space) to
    /// the **compacted** ordinal in [`Self::active_enemy_uids`] (the C#
    /// `Enemies` space). Returns `None` when the stable slot is empty or
    /// holds a removed corpse (which has no compacted ordinal).
    ///
    /// This is the explicit slot↔ordinal decouple: NN features index by
    /// the stable slot, C#-faithful targeting indexes by the compacted
    /// ordinal, and this maps between them. Bead `sts2-rs-c8why`.
    #[must_use]
    pub fn active_ordinal_of_enemy(&self, stable_index: usize) -> Option<usize> {
        let uid = *self.enemy_uids.get(stable_index)?;
        if self.is_removed_corpse(uid) {
            return None;
        }
        // Compacted ordinal = count of non-removed enemies before this slot.
        let ordinal = self.enemy_uids[..stable_index]
            .iter()
            .filter(|u| !self.is_removed_corpse(**u))
            .count();
        Some(ordinal)
    }

    /// Current 1-indexed round number. Mirrors Python's
    /// `combat.round_number` (`sts2-rl-agent@01c094d:sts2_env/core/combat.py:118`,
    /// init `1`, post-increment in `_execute_enemy_turn` /
    /// `end_player_turn`). Surface lifted by bead `sts2-rs-v7mp` for
    /// `MrStruggles` (which reads `combat.round_no` as the damage payload).
    /// The Python attribute is `round_number`; the Rust accessor uses
    /// the C#-side spelling `round_no` (consistent with the bead
    /// description and the `MrStruggles` doc-comment).
    #[must_use]
    pub fn round_no(&self) -> u16 {
        self.round
    }

    /// Whether this combat is taking place in an Elite room. Mirrors
    /// the `combat.is_elite` getter at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py`. Read on
    /// `after_combat_victory` by `SwordOfStone`
    /// (`shop_event.py:2755`). The field is populated by the run/map
    /// layer at combat setup from
    /// [`crate::runstate::RoomType::Elite`]; default is `false`. Bead
    /// `sts2-rs-jvk3`.
    #[must_use]
    pub fn is_elite(&self) -> bool {
        self.is_elite
    }

    /// Current act index (1, 2, or 3) for this combat. Snapshotted from
    /// [`crate::runstate::RunState::act_index`] at combat setup so
    /// combat-side hooks can act-gate without threading `RunState`.
    /// Bead `sts2-rs-mi67e`; mirrors the [`Self::is_elite`] /
    /// [`Self::is_boss`] surface idiom.
    #[must_use]
    pub fn act_index(&self) -> u8 {
        self.act_index
    }

    /// Grow the player's [`PlayerCombatState::energy`] by `amount`,
    /// saturating to `i8::MAX` and ignoring non-positive deltas.
    ///
    /// Mirrors `sts2-rl-agent@01c094d:sts2_env/core/combat.py::
    /// CombatState.gain_energy` (lines 1385-1388):
    ///
    /// ```python
    /// def gain_energy(self, owner: Creature, amount: int) -> None:
    ///     state = self.combat_player_state_for(owner)
    ///     if state is not None and amount > 0:
    ///         state.energy += amount
    /// ```
    ///
    /// The Python signature takes `owner: Creature` because allies
    /// have their own energy pools. The Rust port at this pin only
    /// supports the primary player (`self.player`) since `allies` is
    /// the canonical zero-element `SmallVec` for every pinned fixture;
    /// the signature can grow to `(owner: CreatureUid, amount: i32)`
    /// when ally-energy fixtures land.
    ///
    /// Suppresses all energy gain if the player has `PowerKind::NoEnergyGain`
    /// active, matching C# `NoEnergyGainPower.ModifyEnergyGain → 0m`.
    /// Bead sts2-rs-yb8m: mirrors C# `PlayerCmd.GainEnergy` hook dispatch
    /// for `ModifyEnergyGain` power listeners.
    ///
    /// Surface lifted by bead `sts2-rs-c7ak` for the
    /// `FakeHappyFlower` event-reward relic, which calls
    /// `combat.gain_energy(owner, 1)` every 5 player turns.
    pub fn gain_energy(&mut self, amount: i32) {
        if amount <= 0 {
            return;
        }
        // C# ModifyEnergyGain hook: NoEnergyGainPower returns 0m when
        // applied to the player.
        if self.player_power_amount(PowerKind::NoEnergyGain) > 0 {
            // C# `NoEnergyGainPower.cs:30` flashes in `AfterModifyingEnergyGain`,
            // which `Hook.AfterModifyingEnergyGain` (Hook.cs:853) fires ONLY on
            // the models that actually CHANGED the value (`Hook.ModifyEnergyGain`
            // collects them at Hook.cs:1757). So the flash's real guard is
            // "a positive energy gain was zeroed by this power" — exactly this
            // branch, not every `GainEnergy` call.
            crate::event_log::flash_player_power(self, PowerKind::NoEnergyGain);
            return;
        }
        let new_energy = i32::from(self.player.energy).saturating_add(amount);
        self.player.energy = i8::try_from(new_energy.min(i32::from(i8::MAX))).unwrap_or(i8::MAX);
    }

    /// Grant the player `amount` Stars, tallying the per-turn
    /// stars-gained counter. Core-side mirror of the content-crate
    /// `cards::regent::helpers::gain_stars` so listener-walk effects
    /// (e.g. `StarNextTurnPower.AfterEnergyReset`) can gain stars
    /// without reaching into the content crate. `player.stars` is an
    /// `i8` so we saturate; non-positive amounts are no-ops (matches the
    /// Python pin's no-negative-grant semantics).
    pub fn gain_stars(&mut self, amount: i32) {
        if amount <= 0 {
            return;
        }
        let amount_i8 = i8::try_from(amount).unwrap_or(i8::MAX);
        self.player.stars = self.player.stars.saturating_add(amount_i8);
        let amount_u32 = u32::try_from(amount).unwrap_or(u32::MAX);
        self.player.stars_gained_this_turn = self
            .player
            .stars_gained_this_turn
            .saturating_add(amount_u32);
        // `BlackHolePower.AfterStarsGained` (C# `BlackHolePower.cs`): a
        // positive stars GAIN deals `Amount` damage to every enemy. This is
        // the central core-side gain site (`Effect::GainStars` from
        // StarNextTurn / Genesis routes here); content's `gain_stars` helper
        // delegates here too, so every gain path fires the burst once. Parity
        // RP34: StarNextTurn's +1 at turn-start finished the last enemy.
        self.apply_black_hole_on_stars_event(amount);
    }

    /// Insert a freshly-built [`Card`] POD at the front of the
    /// player's draw pile, registering it in [`Self::cards`] with a
    /// freshly-allocated [`CardUid`].
    ///
    /// The caller supplies a `Card` whose `uid` field is ignored — the
    /// accessor assigns the next sequential `CardUid` (1-based,
    /// `state.cards.len() + 1`) so the arena handle is unique. Returns
    /// the assigned [`CardUid`] for caller bookkeeping (e.g. tests).
    ///
    /// Mirrors `sts2-rl-agent@01c094d:sts2_env/core/combat.py::
    /// CombatState.add_card_to_draw_pile` (lines 1266-1269):
    ///
    /// ```python
    /// def add_card_to_draw_pile(self, owner: Creature, card_name: str) -> None:
    ///     card = self._make_named_card(card_name)
    ///     if card is not None:
    ///         self._zones_for_creature(owner)["draw"].insert(0, card)
    /// ```
    ///
    /// The Python path constructs a fresh `CardInstance` from a
    /// `card_name: str` lookup. The Rust port accepts a
    /// fully-constructed [`Card`] POD because `CardId`-to-`Card`
    /// spec-resolution lives in `sts2-content` (the per-character
    /// spec factories), which `sts2-core` cannot depend on. Callers
    /// (e.g. the `TeaOfDiscourtesy` retrofit) build the POD from the
    /// content-side `CardSpec` factory and pass it here.
    ///
    /// Bead `sts2-rs-c7ak`. Insertion order matches Python's
    /// `insert(0, …)` — the new card lands at index 0 of the draw
    /// pile (i.e. on top, drawn next).
    pub fn add_card_to_draw_pile(&mut self, card: Card) -> CardUid {
        let raw = u32::try_from(self.cards.len() + 1).expect("cards arena fits in u32");
        let uid = CardUid::new(raw).expect("non-zero uid");
        let mut card = card;
        card.uid = uid;
        self.cards.push(card);
        self.player.draw.insert(0, uid);
        uid
    }

    /// Replace the card identified by `old_uid` (in any of the
    /// player's piles) with `new_card`, allocating a fresh
    /// [`CardUid`] for the replacement and rewriting the slot
    /// in-place at the same pile index.
    ///
    /// Returns `Some(new_uid)` on success, or `None` if `old_uid`
    /// is not present in any pile.
    ///
    /// Mirrors `sts2-rl-agent@01c094d:sts2_env/core/combat.py::
    /// CombatState.transform_card` (lines 1801-1820):
    ///
    /// ```python
    /// def transform_card(self, old_card, new_card):
    ///     if old_card is None or new_card is None:
    ///         return None
    ///     # locate (pile, index) of old_card across all_piles
    ///     ...
    ///     target_pile[target_index] = new_card
    ///     return new_card
    /// ```
    ///
    /// Like [`Self::add_card_to_draw_pile`], the `uid` field of the
    /// supplied `new_card` is ignored — the accessor assigns the
    /// next sequential [`CardUid`]. The old card's slot in the
    /// arena is left in place (the uid is now orphan, matching the
    /// Python sim where the old `CardInstance` simply drops out of
    /// every pile while remaining a referenceable object). Pile
    /// order is preserved (the new uid lands at the same index the
    /// old one occupied).
    ///
    /// Used by Charge (transform 2 draw-pile cards into
    /// MinionStrike), PrimalForce (→ GiantRock), various Regent
    /// uncommons, and Necrobinder transformations. Bead
    /// `sts2-rs-ryot`.
    pub fn transform_card(&mut self, old_uid: CardUid, new_card: Card) -> Option<CardUid> {
        // Locate the slot. Order matches Python's `all_piles`
        // iteration: hand, draw, discard, exhaust, play. (The
        // exact iteration order doesn't matter functionally — a
        // given `CardUid` lives in at most one pile — but we keep
        // the canonical sequence for anyone reading alongside the
        // Python source.)
        let pile = [
            crate::cards::PileKind::Hand,
            crate::cards::PileKind::Draw,
            crate::cards::PileKind::Discard,
            crate::cards::PileKind::Exhaust,
            crate::cards::PileKind::Play,
        ]
        .into_iter()
        .find_map(|p| {
            crate::cards::pile_iter(&self.player, p)
                .position(|u| u == old_uid)
                .map(|idx| (p, idx))
        });
        let (pile, idx) = pile?;

        let raw = u32::try_from(self.cards.len() + 1).expect("cards arena fits in u32");
        let new_uid = CardUid::new(raw).expect("non-zero uid");
        let mut new_card = new_card;
        new_card.uid = new_uid;
        self.cards.push(new_card);
        // C# `CardCmd.Transform` records a `CardGeneratedEntry` for every
        // non-Deck destination pile (`CardCmd.cs:444`), creator = the
        // player — drives Supermassive's damage (`Supermassive.cs:22-27`).
        // Unconditional (fires for Exhaust/Play too); this path never
        // transforms into the run deck, so no `PileType.Deck` guard needed.
        self.player.generated_cards_this_combat =
            self.player.generated_cards_this_combat.saturating_add(1);

        let slot: &mut CardUid = match pile {
            crate::cards::PileKind::Hand => &mut self.player.hand[idx],
            crate::cards::PileKind::Draw => &mut self.player.draw[idx],
            crate::cards::PileKind::Discard => &mut self.player.discard[idx],
            crate::cards::PileKind::Exhaust => &mut self.player.exhaust[idx],
            crate::cards::PileKind::Play => &mut self.player.play[idx],
        };
        *slot = new_uid;
        // Bead `sts2-rs-xk9yo`: a transform creates a NEW card that enters
        // combat (C# `CardFactory.Create*` adds it), so fire the
        // `after_card_entered_combat` bus — Regalite (+2 Block per owner card
        // entering combat) and HungerPower were skipped for cards created via
        // this path (PrimalForce→GiantRock, Charge→MinionStrike, Regent /
        // Necrobinder transforms). Mirrors `card_effects::transform_card_in_place`
        // and `fire_after_card_entered_combat`; Exhaust is not a card-entered
        // event.
        if !matches!(pile, crate::cards::PileKind::Exhaust) {
            let _ = crate::relic_effects::dispatch_after_card_entered_combat(self, new_uid, true);
            let _ =
                crate::monster_power_effects::dispatch_after_card_entered_combat_on_monster_powers(
                    self, new_uid,
                );
            // C# `CardCmd.Transform` creates the new card via `CreateCard`, which
            // fires `AfterCardGeneratedForCombat(addedByPlayer: true)` — the same
            // hook a fresh `spawn_card_into_pile` raises. The player-power
            // listeners on that hook (Smokestack damage, Arsenal Strength,
            // PillarOfCreation block) must fire here too; this lower-level
            // transform path (BEGONE → MinionStrike, choice-resolved transforms)
            // previously raised only the entered-combat relic/monster buses, so a
            // Regent's PillarOfCreation granted no block on a transform-generated
            // card (parity fuzz seeds 142/309). Exhaust transforms are excluded
            // (the generation gate matches the entered-combat gate above).
            // C# `CardCmd.TransformTo` passes `addedByPlayer: true`
            // unconditionally (CardCmd.cs:395-401), so pass `true` here too —
            // a turn-start transform (empty Play pile) still grants these.
            crate::card_effects::fire_card_generated_for_combat_powers(self, new_uid, true);
        }
        Some(new_uid)
    }

    /// Borrow the currently-resolving attack context, or `None` when
    /// no attack is in flight.
    ///
    /// Mirrors `sts2-rl-agent@01c094d:sts2_env/core/combat.py::
    /// CombatState.active_attack` (lines 312-316), which returns the
    /// top of `combat._attack_context_stack` if any.
    ///
    /// The Rust port stores at most one in-flight attack
    /// ([`Self::active_attack`] field), which is sufficient at the
    /// current pin because every Python fixture that consults
    /// `active_attack` is single-frame. Nested attack contexts
    /// (e.g. damage reflection landing inside `before_damage_received`)
    /// would need a `Vec<ActiveAttack>` stack; deferred until a
    /// fixture actually requires it.
    ///
    /// Bead `sts2-rs-c7ak`. Surface required by the `HandDrill`
    /// event-reward relic, which checks `combat.active_attack is not
    /// None` + on-block-break before applying Vulnerable on
    /// [`crate::relic_effects::after_damage_given`].
    ///
    /// **Wiring status:** the field defaults to `None` and is
    /// currently only mutated by callers that explicitly invoke
    /// [`Self::begin_attack`] / [`Self::end_attack`]. Combat-loop
    /// integration (auto-push on card-play attack frames, auto-pop on
    /// resolution) is deferred to a follow-up bead — until then
    /// `HandDrill`'s listener is a no-op in production combat. Tests
    /// can drive the field directly via the helpers.
    #[must_use]
    pub fn active_attack(&self) -> Option<&ActiveAttack> {
        self.active_attack.as_ref()
    }

    /// Push a fresh [`ActiveAttack`] frame onto [`Self::active_attack`].
    /// Mirrors Python's `_push_attack_context`
    /// (`sts2-rl-agent@01c094d:sts2_env/core/combat.py:356-360`).
    /// Bead `sts2-rs-c7ak`.
    pub fn begin_attack(&mut self, attacker: Option<CreatureUid>, target: Option<CreatureUid>) {
        self.active_attack = Some(ActiveAttack {
            attacker,
            target,
            results: Vec::new(),
        });
    }

    /// Drop the in-flight [`ActiveAttack`] frame. Mirrors Python's
    /// `_pop_attack_context`
    /// (`sts2-rl-agent@01c094d:sts2_env/core/combat.py:362-372`).
    /// Bead `sts2-rs-c7ak`.
    pub fn end_attack(&mut self) {
        self.active_attack = None;
    }

    /// Append a per-target [`AttackResult`] to the in-flight attack
    /// frame. No-op when no attack is in flight (i.e. when
    /// [`Self::active_attack`] is `None`). Bead `sts2-rs-c7ak`.
    ///
    /// Mirrors Python's `AttackContext.results.append(…)` from
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py::AttackContext`.
    /// Called by [`crate::damage::apply_damage`] right after the
    /// block/HP write so the latest `AttackResult` reflects the
    /// post-hit creature state — `HandDrill` reads
    /// `results[-1].blocked > 0 && target.block == 0` to detect
    /// block-break.
    pub fn record_attack_hit(&mut self, target: CreatureUid, blocked: i32, hp_loss: i32) {
        if let Some(attack) = self.active_attack.as_mut() {
            attack.results.push(AttackResult {
                target,
                blocked,
                hp_loss,
            });
        }
    }

    /// Total Stars *gained* by the player this turn (sum of amounts —
    /// e.g. `gain_stars(3)` contributes 3, not 1). Mirrors Python's
    /// `combat.count_stars_gained_this_turn(owner)` at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:1550-1551`,
    /// which sums the per-owner entries in
    /// `_stars_gained_this_turn` (a list of `(owner, amount)` tuples
    /// appended inside `gain_stars` at `combat.py:1453`).
    ///
    /// The Rust port collapses the list to a single `u32` on
    /// [`PlayerCombatState::stars_gained_this_turn`] (only the player
    /// owns Stars at this pin) — see that field's doc-comment for
    /// reset/lifecycle. Spending Stars does NOT decrement this
    /// counter; it tallies gains only.
    ///
    /// Bead `sts2-rs-x646`. Used by content like `Radiate`
    /// (hits = stars gained this turn — `cards/regent.py:L500-504`).
    #[must_use]
    pub fn count_player_stars_gained_this_turn(&self) -> u32 {
        self.player.stars_gained_this_turn
    }

    /// Spend up to `amount` stars from the player's pool and fire the
    /// `on_stars_spent` channel on both buses with the post-cap value.
    ///
    /// Mirrors Python `combat.spend_stars(owner, amount)` at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:1460-1474`: the
    /// effective spend is `min(amount, player.stars)`; the channel
    /// fires only when `spent > 0`. Returns the actual spend so card
    /// bodies can scale follow-on effects (e.g. `cards/regent.py:548`
    /// `spend_stars(owner, hits)`).
    ///
    /// Bead `sts2-rs-nrjb` (1h8q-tail ch.6). No card/relic callers
    /// are wired at this pin — the function lands as the central
    /// dispatch site so follow-up bodies (`ChildOfTheStars`,
    /// `BlackHole`, `MiniRegent`, `GalacticDust`) can route through
    /// it without re-shaping the call chain.
    pub fn spend_stars(&mut self, amount: i32) -> i32 {
        if amount <= 0 {
            return 0;
        }
        let available = i32::from(self.player.stars);
        let spent = amount.min(available);
        if spent <= 0 {
            return 0;
        }
        self.player.stars = self
            .player
            .stars
            .saturating_sub(i8::try_from(spent).unwrap_or(i8::MAX));
        let ctx = crate::hooks::StarsSpentCtx { amount: spent };
        crate::hooks::dispatch_on_stars_spent(self, &ctx);
        let _ = crate::relic_effects::dispatch_on_stars_spent(self, &ctx);
        // Power-side reactive: `BlackHolePower.AfterStarsSpent` deals
        // `Amount` damage to every enemy on a positive spend (the hooks-bus
        // `on_stars_spent` power channel is a read-only `noop_hook!`, so the
        // mutating body lives here at the central spend site, mirroring how
        // the relic bus carries `&mut`). C# `BlackHolePower.cs:AfterStarsSpent`
        // / `AfterStarsGained` both call `DealDamageToAllEnemies`.
        self.apply_black_hole_on_stars_event(spent);
        self.apply_child_of_the_stars_on_stars_spent(spent);
        spent
    }

    /// `ChildOfTheStarsPower` reactive: on a positive stars SPEND, gain
    /// `Amount * stars_spent` `Unpowered` block. C#
    /// `ChildOfTheStarsPower.AfterStarsSpent` (ChildOfTheStarsPower.cs):
    /// `CreatureCmd.GainBlock(owner, base.Amount * amount, ValueProp.Unpowered,
    /// null)`. Like `BlackHolePower`, the power-side `on_stars_spent` hooks
    /// channel is a read-only `noop_hook!`, so the mutating grant lives here at
    /// the central spend site. The `child_of_the_stars_block_on_spend` helper
    /// existed and was unit-tested but was NEVER wired into the spend path, so a
    /// star-cost card (e.g. CrescentSpear) granted no block under ChildOfTheStars
    /// (Regent fuzz seeds 22/241: player.block low). Spend-only (NOT on
    /// stars-gained), matching C# `AfterStarsSpent`. No-op without the power.
    pub fn apply_child_of_the_stars_on_stars_spent(&mut self, stars_spent: i32) {
        if stars_spent <= 0 {
            return;
        }
        let amount = self.player_power_amount(crate::PowerKind::ChildOfTheStars);
        let block = crate::powers::regent::child_of_the_stars_block_on_spend(amount, stars_spent);
        if block <= 0 {
            return;
        }
        // C# `ChildOfTheStarsPower.cs:19` calls `Flash()` inside the same
        // `amount > 0 && spender == Owner.Player` guard, immediately before
        // `CreatureCmd.GainBlock(Owner, Amount * amount, Unpowered, null)`.
        // The `block <= 0` return above is the WraithForm under-fire deviation
        // (see `hooks::after_side_turn_start`'s WraithForm arm).
        crate::event_log::flash_player_power(self, PowerKind::ChildOfTheStars);
        let _ = crate::card_effects::gain_block_player_unpowered(self, block);
    }

    /// `BlackHolePower` reactive: on a positive stars delta (spent OR gained),
    /// deal `BlackHole.Amount` `Unpowered` damage to every living enemy. C#
    /// `BlackHolePower.DealDamageToAllEnemies`
    /// (`BlackHolePower.cs`): `CreatureCmd.Damage(HittableEnemies, base.Amount,
    /// ValueProp.Unpowered, base.Owner, null)`. The power-side `on_stars_*`
    /// channels are read-only `noop_hook!`s, so the central
    /// [`Self::spend_stars`] (and content's `gain_stars`) call this mutating
    /// site directly. No-op when the delta is non-positive or the player holds
    /// no BlackHole stacks. Bead `sts2-rs-nrjb` follow-up (parity RP34).
    pub fn apply_black_hole_on_stars_event(&mut self, stars_delta: i32) {
        if stars_delta <= 0 {
            return;
        }
        let amount = self.player_power_amount(crate::PowerKind::BlackHole);
        if amount <= 0 {
            return;
        }
        // C# `BlackHolePower.cs:39` calls `Flash()` at the head of the private
        // `DealDamageToAllEnemies()`, which BOTH `AfterCardPlayed` (stars spent
        // on the play) and `AfterStarsGained` funnel into — so the flash rides
        // the damage, not either entry point. This method is Rust's single
        // funnel for both, so the flash goes here, not in the shared
        // `deal_unpowered_damage_to_all_enemies` (Speedster uses that too).
        // C# flashes above the target scan, so a dead arena still flashes.
        crate::event_log::flash_player_power(self, PowerKind::BlackHole);
        self.deal_unpowered_damage_to_all_enemies(amount);
    }

    /// Deal `amount` `Unpowered` damage to every living enemy once, the
    /// C# `CreatureCmd.Damage(HittableEnemies, amount, ValueProp.Unpowered,
    /// base.Owner, null)` shape. `Unpowered` props mean Strength /
    /// Vulnerable NO-op on the damage multipliers. Shared by
    /// [`Self::apply_black_hole_on_stars_event`] (BlackHolePower) and the
    /// Silent `SpeedsterPower.AfterCardDrawn` body in
    /// [`Self::invoke_card_drawn_for_newly_drawn`]. No-op when `amount <= 0`.
    pub(crate) fn deal_unpowered_damage_to_all_enemies(&mut self, amount: i32) {
        if amount <= 0 {
            return;
        }
        // Snapshot the alive-enemy targets before dealing (a kill mid-loop
        // mutates `enemy_uids`); C# captures `HittableEnemies` up front.
        let targets: smallvec::SmallVec<[CreatureUid; 8]> = self
            .enemy_uids
            .iter()
            .copied()
            .filter(|uid| {
                self.creatures
                    .iter()
                    .any(|c| c.id == uid.0 && c.current_hp > 0)
            })
            .collect();
        for target in targets {
            // Re-check alive (a prior hit could not have killed a different
            // enemy, but a future rider might — keep it defensive).
            if !self
                .creatures
                .iter()
                .any(|c| c.id == target.0 && c.current_hp > 0)
            {
                continue;
            }
            crate::damage::apply_damage(
                self,
                crate::hooks::CreatureRef::Player,
                target,
                amount,
                crate::hooks::ValueProps::UNPOWERED,
                None,
            );
        }
    }
}

/// In-flight attack context. Mirrors Python
/// `sts2-rl-agent@01c094d:sts2_env/core/combat.py::AttackContext`
/// (load-bearing fields only).
///
/// Bead `sts2-rs-c7ak`. Stored as `Option<ActiveAttack>` on
/// [`CombatState::active_attack`] (single-frame; see
/// [`CombatState::active_attack()`] for nesting rationale).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ActiveAttack {
    /// Originating creature, or `None` for environmental attacks.
    pub attacker: Option<CreatureUid>,
    /// Declared single-target, or `None` for multi-target attacks
    /// (`AllEnemies` / `RandomEnemy`).
    pub target: Option<CreatureUid>,
    /// Per-target hit results in deal order. The most-recent entry
    /// (`results.last()`) is the one `HandDrill` inspects.
    pub results: Vec<AttackResult>,
}

/// Per-target outcome of one hit inside an [`ActiveAttack`] frame.
/// Mirrors Python `AttackResult { target, blocked, hp_loss }`.
/// Bead `sts2-rs-c7ak`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttackResult {
    /// Hit target (the creature that the damage was applied to).
    pub target: CreatureUid,
    /// Amount of damage absorbed by the target's block bar before
    /// HP loss. `> 0` AND the target's post-hit `block == 0` is the
    /// `HandDrill` block-break detection gate.
    pub blocked: i32,
    /// HP loss actually inflicted (post-block, post-modify_hp_lost).
    pub hp_loss: i32,
}

impl CombatState {
    /// Discriminate the [`Creature`] at `uid` as either [`CreatureKind::Player`]
    /// or [`CreatureKind::Monster`] (bead `sts2-rs-d03` Phase 1).
    ///
    /// Returns [`CreatureKind::Player`] iff `Some(uid) ==
    /// self.player.creature_uid`; otherwise [`CreatureKind::Monster`].
    /// Mirrors the C# `Creature.IsPlayer` getter at
    /// `sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Entities.Creatures/
    /// Creature.cs`. Phase 2 will replace this method with a direct
    /// struct-field read once the call-site migration lands.
    #[must_use]
    pub fn creature_kind(&self, uid: CreatureUid) -> CreatureKind {
        if self.player.creature_uid == Some(uid) {
            CreatureKind::Player
        } else {
            CreatureKind::Monster
        }
    }

    /// Allocate a [`Creature`] record for the player in
    /// [`Self::creatures`] (conventionally slot 0 for the primary
    /// player) and capture its uid in [`PlayerCombatState::creature_uid`].
    ///
    /// The Creature's combat scalars (`current_hp`, `max_hp`, `block`,
    /// `powers`) are mirrored from [`Self::player`] at the moment of
    /// call. Phase 1 of bead `sts2-rs-d03` keeps the legacy
    /// [`PlayerCombatState`] fields as the read-side source of truth;
    /// the Creature is populated for forward-compat so Phase 2 can
    /// migrate every access site to read from
    /// `state.creatures[state.player.creature_uid]` without changing
    /// observable behavior.
    ///
    /// Returns the [`CreatureUid`] of the freshly-pushed Creature.
    /// If [`PlayerCombatState::creature_uid`] is already `Some`, this
    /// method panics — the player-Creature should be created exactly
    /// once per combat (start) and torn down at combat end via
    /// [`Self::teardown_player_creature`].
    ///
    /// Mirrors the canonical-pin C# `Combat.SetupPlayerCreature` (see
    /// `Creature.cs` at v0.103.2) and Python's lock-step
    /// `combat.player_state.creature = primary_creature` assignment in
    /// `sts2_env/core/combat.py::CombatState.__init__`.
    pub fn init_player_creature(&mut self) -> CreatureUid {
        assert!(
            self.player.creature_uid.is_none(),
            "init_player_creature: player.creature_uid already set; call teardown_player_creature first"
        );
        // Use slot 0 by convention when the arena is fresh; otherwise
        // append. The id is `creatures.len() + 1` (NonZeroU32) so it
        // is distinct from any existing creature ids in the arena.
        let raw =
            u32::try_from(self.creatures.len() + 1).expect("creature arena length fits in u32");
        let id = NonZeroU32::new(raw).expect("non-zero");
        let creature = Creature {
            id,
            current_hp: self.player.current_hp,
            max_hp: self.player.max_hp,
            block: self.player.block,
            powers: self.player.powers.clone(),
            // Bead `sts2-rs-a19m`: the player-Creature has no
            // species — `species: None` is the canonical sentinel.
            // Use [`CombatState::creature_kind`] for player/monster
            // discrimination; `species` is *only* the monster-species
            // discriminant once you already know you're looking at a
            // monster.
            species: None,
            monster_id: MonsterId("Player"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        };
        self.creatures.push(creature);
        let uid = CreatureUid(id);
        self.player.creature_uid = Some(uid);
        uid
    }

    /// Yield the player's power [`PowerUid`]s as the UNION of the mirror
    /// Creature's `powers` and the legacy `state.player.powers`, deduped
    /// by uid (mirror first, then legacy uids not already seen).
    ///
    /// Card-driven `apply_power_to_player` (post-au4l2) routes through
    /// `state.creatures[player_uid].powers`, while legacy paths and many
    /// unit tests still push to `state.player.powers`. Reactive hooks that
    /// react to player-owned powers (Juggernaut on block-gain, FeelNoPain /
    /// DarkEmbrace on exhaust, CrimsonMantle's self-damage counter) MUST
    /// walk this union — a raw `state.player.powers` scan silently no-ops
    /// any mirror-applied instance (see
    /// `feedback_player_consume_must_walk_mirror_union`). This is the
    /// single-side analogue of [`crate::hooks::iter_power_listeners`]'s
    /// player segment. Bead `sts2-rs-q6bix` (F-C).
    #[must_use]
    pub fn player_power_uids(&self) -> SmallVec<[PowerUid; 8]> {
        let mut out: SmallVec<[PowerUid; 8]> = self
            .player
            .creature_uid
            .and_then(|uid| creature_by_id(&self.creatures, uid.0))
            .map(|c| c.powers.clone())
            .unwrap_or_default();
        // Fast path (perf 2026-07-04): no legacy powers to union in — the
        // mirror clone above IS the result. This is the common case on the
        // MCTS hot path (post-au4l2 combat routes powers through the
        // mirror creature), and it skips the O(mirror × legacy) dedup loop.
        if self.player.powers.is_empty() {
            return out;
        }
        for uid in &self.player.powers {
            if !out.contains(uid) {
                out.push(*uid);
            }
        }
        out
    }

    /// Count living player-side **ally** creatures (NOT counting the
    /// player itself): summoned allies in [`Self::allies`] plus a living
    /// Osty pet (`player.pet_uid`). Used for DieForYou / pet-aware logic.
    #[must_use]
    pub fn living_ally_count(&self) -> usize {
        let allies = self.allies.iter().filter(|a| a.current_hp > 0).count();
        let pet_alive = self
            .player
            .pet_uid
            .and_then(|uid| self.creatures.iter().find(|c| c.id == uid.0))
            .is_some_and(|c| c.current_hp > 0);
        allies + usize::from(pet_alive)
    }

    /// Whether an `AnyAlly`-target card has a legal target, mirroring C#
    /// `CardModel.CanPlay` → `NoLivingAllies` when
    /// `PlayerCreatures.Count(IsAlive) <= 1` (`CardModel.cs:1342`).
    /// `PlayerCreatures` is `Creatures.Where(IsPlayer)` (`CombatState.cs:292`)
    /// — i.e. the player plus any co-op **player** allies. It does NOT include
    /// the Osty **pet** (a pet has `IsPlayer == false`), so a DieForYou Osty
    /// summoned by BoundPhylactery/PhylacteryUnbound must NOT make an AnyAlly
    /// card playable. `living_ally_count` counts the pet (it is pet-aware for
    /// DieForYou), so the AnyAlly gate uses this pet-EXCLUDING count instead.
    /// In single-player Ironclad `allies` is always empty, so AnyAlly cards
    /// (Coordinate/BelieveInYou/DemonicShield/…) are always unplayable — exactly
    /// like C#. Bead `sts2-rs-6zsju`.
    #[must_use]
    pub fn has_any_ally_card_target(&self) -> bool {
        self.allies.iter().any(|a| a.current_hp > 0)
    }

    /// Sum the player's instances of `kind` across the mirror+legacy
    /// union (see [`Self::player_power_uids`]). Returns 0 when absent.
    /// Bead `sts2-rs-q6bix` (F-C).
    #[must_use]
    pub fn player_power_amount(&self, kind: PowerKind) -> i32 {
        // O(1) absence gate: `present_kinds` is a conservative superset of
        // every live power kind in the arena (any creature), so a miss here
        // proves the player can't have one either. The expert rollout
        // heuristics probe many kinds that are absent in most states — this
        // skips the full union walk for all of them.
        if !self.powers.present_kinds().contains(kind) {
            return 0;
        }
        // Allocation-free walk of the mirror+legacy power union (the
        // owning-side analogue of [`crate::hooks::iter_power_listeners`]).
        // This is called extremely hot (~hundreds of thousands of times
        // per MCTS search via the expert heuristics + per-card draw
        // gates), so it borrows the creature power slice directly
        // instead of cloning it into a `SmallVec` the way
        // [`Self::player_power_uids`] does. Perf-investigation 2026-06-07.
        let creature_powers: &[PowerUid] = match self
            .player
            .creature_uid
            .and_then(|uid| creature_by_id(&self.creatures, uid.0))
        {
            Some(c) => c.powers.as_slice(),
            None => &[],
        };
        let uids = creature_powers.iter().copied().chain(
            self.player
                .powers
                .iter()
                .copied()
                .filter(|uid| !creature_powers.contains(uid)),
        );
        let mut acc = 0_i32;
        for uid in uids {
            if let Some(p) = self.powers.get_by_id(uid.0) {
                if p.kind == kind {
                    acc = acc.saturating_add(p.amount);
                }
            }
        }
        acc
    }

    /// Accumulate `delta` Strength onto the player's `Rupture` power's
    /// per-instance `counter` (the deferred-Strength bucket), mirroring C#
    /// `RupturePower.AfterDamageReceived`'s `playedCards[cardSource] += Amount`
    /// branch (`MegaCrit.Sts2.Core.Models.Powers/RupturePower.cs:55`). Used
    /// when the HP-loss that triggered Rupture was inflicted by the
    /// currently-playing card, so the Strength must land only *after* the
    /// card finishes resolving (and thus not buff the card's own hits).
    ///
    /// The `counter` field is engine-private scratch (never serialized — the
    /// parity exporter emits only `amount`) and is drained back to `0` by
    /// [`Self::drain_rupture_deferred_strength`] at the `after_card_played`
    /// boundary, so it is always `0` at rest. No-op if the player has no
    /// Rupture instance. Bead `sts2-rs-22ttw`.
    pub fn accumulate_rupture_deferred_strength(&mut self, delta: i32) {
        if delta == 0 {
            return;
        }
        // Perf 2026-07-04: `present_kinds` is a superset of live kinds, so
        // "no Rupture anywhere" provably means the union walk below would
        // find nothing — skip it (the common case on the search hot path).
        if !self.powers.present_kinds().contains(PowerKind::Rupture) {
            return;
        }
        let Some(uid) = self.player_power_uids().into_iter().find(|uid| {
            self.powers
                .get_by_id(uid.0)
                .is_some_and(|p| p.kind == PowerKind::Rupture)
        }) else {
            return;
        };
        if let Some(pow) = self.powers.get_mut_by_id(uid.0) {
            pow.counter = pow.counter.saturating_add(delta);
        }
    }

    /// Drain and return the player's `Rupture` deferred-Strength accumulator
    /// (the per-instance `counter`), resetting it to `0`. Returns `0` when no
    /// Rupture instance exists or nothing was deferred. Called once by
    /// [`crate::combat_loop::play_card`] right after the `after_card_played`
    /// dispatch to apply C#'s deferred `PowerCmd.Apply<StrengthPower>` from
    /// `RupturePower.AfterCardPlayed`. Bead `sts2-rs-22ttw`.
    pub fn drain_rupture_deferred_strength(&mut self) -> i32 {
        // Perf 2026-07-04: same `present_kinds` early-out as
        // `accumulate_rupture_deferred_strength` — this runs once per card
        // play, and Rupture is almost never live.
        if !self.powers.present_kinds().contains(PowerKind::Rupture) {
            return 0;
        }
        let Some(uid) = self.player_power_uids().into_iter().find(|uid| {
            self.powers
                .get_by_id(uid.0)
                .is_some_and(|p| p.kind == PowerKind::Rupture)
        }) else {
            return 0;
        };
        if let Some(pow) = self.powers.get_mut_by_id(uid.0) {
            let owed = pow.counter;
            pow.counter = 0;
            owed
        } else {
            0
        }
    }

    /// Summon (or refresh) the player's Osty pet — Necrobinder
    /// signature mechanic. Bead `sts2-rs-s87y` (BoneBrew + minimal
    /// surface).
    ///
    /// Mirrors Python `CombatState.summon_osty(owner, amount, source)`
    /// at `sts2-rl-agent@01c094d:sts2_env/core/combat.py:1861-1916`.
    /// The shape:
    ///
    /// 1. If no Osty exists for this player ([`PlayerCombatState::pet_uid`]
    ///    is `None`), spawn one with `max_hp = current_hp = amount`,
    ///    apply a single stack of [`PowerKind::DieForYou`], record the
    ///    new [`CreatureUid`] on `pet_uid`, and return `Some(uid)`.
    /// 2. If an Osty record exists and is alive, the call refreshes
    ///    *grows* its `max_hp` by `amount` (matches Python
    ///    `osty.gain_max_hp(amount)`).
    /// 3. If an Osty record exists but is dead (`current_hp == 0`),
    ///    the call revives it with `max_hp = current_hp = amount` and
    ///    re-applies `DieForYou` if missing (Python `is_reviving`
    ///    branch).
    /// 4. `amount <= 0` is a no-op that returns the current `pet_uid`.
    ///
    /// Deferred surfaces (vs the Python pin — these will retrofit when
    /// the corresponding listeners land):
    ///
    /// - `modify_summon_amount` listener bus (Python amplifies
    ///   `amount` via Necrobinder relics like RingOfTheDrake before
    ///   the gain).
    /// - `after_summon` / `after_creature_added_to_combat` /
    ///   `after_osty_revived` event broadcasts (no Necrobinder
    ///   listeners exist yet on the Rust side; the dispatch buses
    ///   themselves are sub-beads of the Necrobinder runtime epic).
    /// - Multi-player `pet_owner` resolution: this method only handles
    ///   the primary player (`self.player`); `allies` slots will gain
    ///   per-player `pet_uid` lookups when the multi-player runtime
    ///   lands.
    ///
    /// Returns the Osty's [`CreatureUid`] (always `Some` after a
    /// non-trivial call), or the existing handle on a zero-amount
    /// no-op.
    /// Rebuild a serialized Osty onto a freshly reconstructed state.
    ///
    /// [`Self::summon_osty`] is the wrong tool for this: it *is* a summon, so
    /// it grows an existing pet rather than setting it, and it fires the
    /// `on_pet_summoned` relic bus — an event that did not happen in the world
    /// being reconstructed. This restores the recorded creature and nothing
    /// else, then re-establishes the `DieForYou` link the redirect needs.
    ///
    /// Without it the wire's `$.player.osty` was parsed and then dropped on
    /// the floor: `state_from_dict` left `pet_uid` at `None`, so a rebuilt
    /// Necrobinder had NO pet, and BoundPhylactery re-summoned a fresh 1-HP
    /// one at the next turn start. Measured on the oracle: the mirror's osty
    /// sat at `hp 1 / max_hp 1` against a world at `2/8`, `3/7`, `5/11`,
    /// `5/8` — a pet that absorbs nothing, so the player ate every hit the
    /// real Osty would have soaked. That was the largest oracle class, 127
    /// single-field `$.player.hp` records, mirror always LOWER.
    pub fn restore_osty(&mut self, hp: i32, max_hp: i32, block: i32) -> CreatureUid {
        let max_hp = max_hp.max(1);
        let raw =
            u32::try_from(self.creatures.len() + 1).expect("creature arena length fits in u32");
        let id = NonZeroU32::new(raw).expect("non-zero");
        self.creatures.push(Creature {
            id,
            current_hp: hp.clamp(0, max_hp),
            max_hp,
            block: block.max(0),
            powers: SmallVec::new(),
            species: Some(MonsterKind::Osty),
            monster_id: MonsterId("Osty"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        let pet_uid = CreatureUid(id);
        self.player.pet_uid = Some(pet_uid);
        self.ensure_die_for_you_on(pet_uid);
        pet_uid
    }

    pub fn summon_osty(&mut self, amount: i32) -> Option<CreatureUid> {
        if amount <= 0 {
            return self.player.pet_uid;
        }
        // C# `OstyCmd.Summon` (`OstyCmd.cs`) fires `Hook.AfterSummon` on
        // EVERY non-zero-amount path — including the already-alive
        // `IsOstyAlive → GainMaxHp` grow branch — at the single call site
        // after the if/else. So the `on_pet_summoned` relic bus must fire on
        // the alive-grow branch too, not only on fresh-spawn / revive (the
        // earlier bead `sts2-rs-cqml` treated the grow path as "not a summon",
        // which diverges from the C# source of truth). The `amount <= 0`
        // early-return above mirrors C#'s `amount == 0` guard.
        // Branch 1: existing Osty record — refresh/grow or revive.
        if let Some(pet_uid) = self.player.pet_uid {
            if let Some(idx) = self.creatures.iter().position(|c| c.id == pet_uid.0) {
                let was_alive = self.creatures[idx].current_hp > 0;
                if was_alive {
                    // Grow Max HP without broadcasting the relic
                    // `after_max_hp_changed` bus (Python's `gain_max_hp`
                    // on the bare `Creature` does not fire the relic
                    // hook — the bus is owner-of-creature-keyed and
                    // the Osty has no owner-relic listeners at this
                    // pin). The `AfterSummon` broadcast still fires below.
                    self.creatures[idx].gain_max_hp(amount);
                } else {
                    // Reviving branch — full reset to `amount`.
                    self.creatures[idx].max_hp = amount;
                    self.creatures[idx].current_hp = amount;
                    self.creatures[idx].block = 0;
                }
                self.ensure_die_for_you_on(pet_uid);
                // C# `OstyCmd.Summon` fires `Hook.AfterSummon` after BOTH the
                // alive-grow and revive paths — fire unconditionally here.
                let ctx = crate::relic_effects::PetSummonCtx {
                    pet_kind: crate::relic_effects::PetKind::Osty,
                };
                let _ = crate::relic_effects::dispatch_on_pet_summoned(self, &ctx);
                return Some(pet_uid);
            }
            // Dangling pet_uid (creature reaped) — fall through to
            // spawn a fresh Osty.
            self.player.pet_uid = None;
        }
        // Branch 2: fresh spawn.
        let raw =
            u32::try_from(self.creatures.len() + 1).expect("creature arena length fits in u32");
        let id = NonZeroU32::new(raw).expect("non-zero");
        let osty = Creature {
            id,
            current_hp: amount,
            max_hp: amount,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Osty),
            monster_id: MonsterId("Osty"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        };
        self.creatures.push(osty);
        let pet_uid = CreatureUid(id);
        self.player.pet_uid = Some(pet_uid);
        self.ensure_die_for_you_on(pet_uid);
        // Fresh spawn — always notify (bead `sts2-rs-cqml`).
        let ctx = crate::relic_effects::PetSummonCtx {
            pet_kind: crate::relic_effects::PetKind::Osty,
        };
        let _ = crate::relic_effects::dispatch_on_pet_summoned(self, &ctx);
        Some(pet_uid)
    }

    /// Necrobinder `Melancholy.AfterDeath` cost-reduction rider — C#
    /// `Melancholy.cs::AfterDeath` (`MegaCrit.Sts2.Core.Models.Cards/
    /// Melancholy.cs:35-48`). C# fires `CardModel.AfterDeath` on EVERY card
    /// model whenever ANY creature dies during combat (the
    /// `CreatureCmd.KillWithoutCheckingWinCondition` death sweep calls
    /// `Hook.AfterDeath(.., wasRemovalPrevented: false, ..)` unconditionally —
    /// `CreatureCmd.cs:371`). Melancholy's body subtracts `Energy.IntValue`
    /// (=1) from its own this-combat energy cost
    /// (`EnergyCost.AddThisCombat(-base.DynamicVars.Energy.IntValue)`), but
    /// ONLY when the copy sits in a COMBAT pile (`Pile.IsCombatPile`) — i.e.
    /// Draw / Hand / Discard / Exhaust / Play (`PileTypeExtensions.cs:24-30`,
    /// enum values 1..=5), never the master Deck pile. The Necrobinder Osty
    /// resurrect/die loop racks up deaths fast, so a Melancholy in hand drops
    /// from cost 3 to 0 after three deaths.
    ///
    /// This mirrors the `KinglyKick` per-draw cost-decrement precedent
    /// (`invoke_card_drawn_for_newly_drawn`, keyed off `CardId::KinglyKick`):
    /// a core-side, `CardId`-keyed flat mutation of `Card.cost`, clamped at 0.
    /// `wasRemovalPrevented` is modeled as always-false here, matching the
    /// unconditional C# `Hook.AfterDeath` call and the existing power-side
    /// witness dispatch ([`crate::hooks::dispatch_after_death`]).
    ///
    /// Fired from every creature-death chokepoint that the Necrobinder loop
    /// reaches: the damage-kill witness dispatch ([`crate::hooks::
    /// dispatch_after_death`]), the direct [`Self::kill_osty`] (SACRIFICE /
    /// BONE_SHARDS), and the inline DieForYou Osty-redirect kill in
    /// `apply_damage_to_player`. Bead `sts2-rs` RP85.
    pub fn reduce_melancholy_cost_on_death(&mut self) {
        // Fast absence gate: this fires on EVERY creature death, but
        // Melancholy is in almost no deck. The O(piles × cards) uid-by-uid
        // `find` walk below showed up at ~1.3% of MCTS-search instructions on
        // the 2026-07-03 realistic-deck callgrind; the arena's presence bit
        // (J02) answers "no Melancholy anywhere" in one test.
        if !self.cards.holds_id(crate::cards::CardId::Melancholy) {
            #[cfg(debug_assertions)]
            self.shadow_skipped_card_sweep(
                "reduce_melancholy_cost_on_death",
                CombatState::reduce_melancholy_cost_walk,
            );
            return;
        }
        self.reduce_melancholy_cost_walk();
    }

    /// The ungated pile walk of [`Self::reduce_melancholy_cost_on_death`].
    fn reduce_melancholy_cost_walk(&mut self) {
        // Collect every combat-pile uid (C# `Pile.IsCombatPile`): Draw, Hand,
        // Discard, Exhaust, Play. NOT the master Deck (out-of-combat pile).
        let combat_pile_uids: smallvec::SmallVec<[CardUid; 64]> = self
            .player
            .hand
            .iter()
            .chain(self.player.draw.iter())
            .chain(self.player.discard.iter())
            .chain(self.player.exhaust.iter())
            .chain(self.player.play.iter())
            .copied()
            .collect();
        for uid in combat_pile_uids {
            if let Some(c) = self.cards.iter_mut().find(|c| c.uid == uid) {
                if c.card_id == crate::cards::CardId::Melancholy {
                    // C# `EnergyCost.AddThisCombat(-1)`, clamped at 0.
                    c.cost = c.cost.saturating_sub(1).max(0);
                }
            }
        }
    }

    /// Kill the player's Osty pet — C# `CreatureCmd.Kill(Owner.Osty)`, used by
    /// Bone Shards' sacrifice. Sets the pet's HP/block to 0 but RETAINS the
    /// corpse in `state.creatures` and keeps `player.pet_uid` pointing at it:
    /// `DieForYouPower` overrides `ShouldCreatureBeRemovedFromCombatAfterDeath`
    /// to `false`, so the dead Osty persists and a later [`Self::summon_osty`]
    /// revives it in place (the `current_hp == 0` revive branch).
    ///
    /// Mirrors C# `RemoveAllPowersAfterDeath`: every power whose
    /// `ShouldPowerBeRemovedAfterOwnerDeath()` is `true` is dropped; only
    /// `DieForYou` (the lone override returning `false`) survives — which is
    /// exactly what lets the revive path re-find the pet. No-op when no Osty
    /// exists or it is already dead.
    ///
    /// There is no pet-death dispatch bus at this pin (only `on_pet_summoned`),
    /// so no listener broadcast is owed here.
    pub fn kill_osty(&mut self) {
        let Some(pet_uid) = self.player.pet_uid else {
            return;
        };
        let Some(idx) = self.creatures.iter().position(|c| c.id == pet_uid.0) else {
            return;
        };
        let prev_hp = self.creatures[idx].current_hp;
        if prev_hp <= 0 {
            return;
        }
        self.creatures[idx].current_hp = 0;
        self.creatures[idx].block = 0;
        // C# `CreatureCmd.KillWithoutCheckingWinCondition` (CreatureCmd.cs:349-353)
        // fires `Hook.AfterCurrentHpChanged(creature, -currentHp)` immediately
        // after `LoseHpInternal` and BEFORE `RemoveAllPowersAfterDeath`
        // (CreatureCmd.cs:385). The Osty losing all its HP reflects through
        // `NecroMasteryPower.AfterCurrentHpChanged` (deal `lostHp * Amount`
        // Unblockable|Unpowered to all hittable enemies). The damage-redirect
        // path in `combat_loop` already fires this hook, but a DIRECT kill via
        // this method (SACRIFICE, BONE_SHARDS) bypassed it — so killing the Osty
        // while NecroMastery was up dealt no reflected damage (Necrobinder fuzz
        // seed 443: monsters[0].hp +8 in Rust).
        crate::hooks::dispatch_after_current_hp_changed(self, pet_uid, -prev_hp);
        // The hook may mutate `self.creatures` (enemies dying to the reflected
        // damage can be removed), so re-locate the Osty by uid before the power
        // sweep rather than reusing the stale `idx`.
        let Some(idx) = self.creatures.iter().position(|c| c.id == pet_uid.0) else {
            return;
        };
        // RemoveAllPowersAfterDeath: retain only DieForYou.
        let removed: SmallVec<[NonZeroU32; 8]> = self.creatures[idx]
            .powers
            .iter()
            .filter(|u| {
                self.powers
                    .iter()
                    .find(|p| p.id == u.0)
                    .map_or(true, |p| p.kind != PowerKind::DieForYou)
            })
            .map(|u| u.0)
            .collect();
        self.creatures[idx]
            .powers
            .retain(|u| !removed.contains(&u.0));
        self.powers.retain(|p| !removed.contains(&p.id));
        // C# `CreatureCmd.Kill` death sweep fires `Hook.AfterDeath` for this
        // direct Osty kill too (CreatureCmd.cs:371), so reduce each in-pile
        // Melancholy's cost by 1. The damage-kill path goes through
        // `hooks::dispatch_after_death`; this direct kill bypasses it.
        self.reduce_melancholy_cost_on_death();
    }

    /// Immediately doom-kill every alive enemy whose current HP is at or below
    /// its total Doom stacks. Mirrors C#
    /// `DoomPower.DoomKill(DoomPower.GetDoomedCreatures(HittableEnemies))` —
    /// the on-play kill `EndOfDays` runs right after applying its Doom (a card
    /// at/under its Doom threshold dies the same turn, not one turn later).
    ///
    /// Each kill routes through [`crate::hooks::doom_kill`], which reuses the
    /// proven self-lethal HP-loss tick and fires the `after_died_to_doom`
    /// relic bus (BookRepairKnife heal) per victim — the same path the
    /// turn-end `DoomPower.before_turn_end` tick reaches via
    /// [`crate::hooks::Effect::DoomKill`]. Bead `sts2-rs-z1z5t`.
    pub fn doom_kill_doomed_enemies(&mut self) {
        let victims: Vec<CreatureUid> = self
            .enemy_uids
            .iter()
            .copied()
            .filter(|uid| {
                self.creatures
                    .iter()
                    .find(|c| c.id == uid.0)
                    .is_some_and(|c| {
                        if c.current_hp <= 0 {
                            return false;
                        }
                        let doom: i32 = c
                            .powers
                            .iter()
                            .filter_map(|pu| {
                                self.powers
                                    .iter()
                                    .find(|p| p.id == pu.0 && p.kind == PowerKind::Doom)
                            })
                            .map(|p| p.amount)
                            .sum();
                        doom > 0 && c.current_hp <= doom
                    })
            })
            .collect();
        for uid in victims {
            crate::hooks::doom_kill(self, crate::hooks::CreatureRef::Enemy(uid));
        }
    }

    /// Summon a cosmetic, behaviourless pet creature of `species` with
    /// `hp` max/current HP. Mirrors C# `PlayerCmd.AddPet<T>` for the
    /// "display pet" relics whose pet monster is a `NOTHING_MOVE`,
    /// no-health-bar companion (`Byrdpip`, `PaelsLegion`): the pet's
    /// gameplay lives entirely on the owning relic, so the spawned
    /// Creature carries no AI, no intent, and is intentionally NOT pushed
    /// onto [`Self::enemy_uids`] — it never takes a turn, is never
    /// targeted, and its (empty) power list is never walked. It exists so
    /// pet-count / display consumers observe it.
    ///
    /// Distinct from [`Self::summon_osty`], which owns the single
    /// Necrobinder `pet_uid` slot and fires the `on_pet_summoned` bus;
    /// display pets do not touch `pet_uid`. Idempotent within a combat:
    /// reuses an existing live pet of the same `species` rather than
    /// stacking duplicates (mirrors `AddPet<T>` returning the existing
    /// typed pet). Returns the pet's [`CreatureUid`]. Bead
    /// `sts2-rs-4x616` / `sts2-rs-ppabi`.
    pub fn summon_pet(&mut self, species: MonsterKind, hp: i32) -> CreatureUid {
        if let Some(c) = self
            .creatures
            .iter()
            .find(|c| c.species == Some(species) && c.current_hp > 0)
        {
            return CreatureUid(c.id);
        }
        let raw =
            u32::try_from(self.creatures.len() + 1).expect("creature arena length fits in u32");
        let id = NonZeroU32::new(raw).expect("non-zero");
        self.creatures.push(Creature {
            id,
            current_hp: hp,
            max_hp: hp,
            block: 0,
            powers: SmallVec::new(),
            species: Some(species),
            monster_id: MonsterId(species.python_name()),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        CreatureUid(id)
    }

    /// Spawn a fresh enemy creature mid-combat. Mirrors Python's
    /// `combat.add_enemy(creature, ai)` at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py` — picks the next
    /// available [`CreatureUid`], rolls starting HP via
    /// [`MonsterKind::starting_hp`] off the combat RNG cursor, and
    /// appends to both [`Self::creatures`] and [`Self::enemy_uids`].
    /// Returns the new uid so the caller can attach an AI via
    /// `self.creatures[idx].ai = Some(...)`.
    ///
    /// Bead `sts2-rs-vn0m`: the Phase-5 Summon dispatcher consumes this
    /// from the per-move `effect_fn` of summoner factories (Fogmog
    /// ILLUSION, Ovicopter LAY_EGGS, Vantom CONSUME, etc.). AI attachment
    /// is the caller's responsibility —
    /// content-side `effect_fn` bodies pair this with the matching
    /// `create_*_ai()` factory. Spawned enemies without an AI fall back
    /// to the legacy [`MonsterKind::roll_intent`] path
    /// (`combat_loop::enemy_turn_exec` checks `c.ai.is_some()` before
    /// routing).
    pub fn spawn_enemy(&mut self, species: MonsterKind) -> CreatureUid {
        let at = self.enemy_uids.len();
        self.spawn_enemy_at(species, at)
    }

    /// Attach a freshly-built [`MonsterAi`] to an already-spawned
    /// creature. Mid-combat summoner `effect_fn`s pair
    /// [`Self::spawn_enemy`] / [`Self::spawn_enemy_at_front`] with the
    /// matching content `create_*_ai()` factory so the summon acts on
    /// its real moveset instead of the legacy
    /// [`MonsterKind::roll_intent`] fallback (`enemy_turn_exec` routes
    /// on `c.ai.is_some()`). No-op if `uid` names no live creature.
    /// Bead `sts2-rs-cuocf` (generalizes the `sts2-rs-vn0m` summon
    /// dispatcher's "AI attachment is the caller's responsibility"
    /// contract into a public setter; previously content had no way to
    /// reach the private `Creature::ai` field).
    pub fn set_creature_ai(&mut self, uid: CreatureUid, ai: crate::monster_ai::MonsterAi) {
        if let Some(c) = self.creatures.iter_mut().find(|c| c.id == uid.0) {
            c.ai = Some(ai);
        }
    }

    /// Like [`Self::spawn_enemy`] but inserts the new enemy at the
    /// FRONT of `enemy_uids` (turn / display order index 0).
    ///
    /// Bead `sts2-rs-o1mwv` (C.1). C# orders enemies by their slot's
    /// position in the encounter's `Slots` list
    /// (`CombatState.SortEnemiesBySlotName`). The one act-1 mid-combat
    /// summoner — Fogmog — adds its EyeWithTeeth into slot `"illusion"`
    /// (`Fogmog.cs:59` `CreatureCmd.Add<EyeWithTeeth>(.., "illusion")`),
    /// which precedes the `"fogmog"` slot in
    /// `FogmogNormal.Slots = ["illusion", "fogmog"]` — so the illusion
    /// renders/acts ahead of its summoner. Rust has no slot model yet
    /// (`enemy_uids` is raw push order), so this primitive reproduces
    /// the observed ordering by front-insertion.
    ///
    /// TODO(o1mwv): a general per-encounter slot-ordering model
    /// (Creature slot names + `Encounter::slots()` + sort) would
    /// subsume this; filed as a follow-up. Front-insert is exact for
    /// FogmogNormal (single leading summon slot) and is the only act-1
    /// case today.
    pub fn spawn_enemy_at_front(&mut self, species: MonsterKind) -> CreatureUid {
        self.spawn_enemy_at(species, 0)
    }

    /// Like [`Self::spawn_enemy`] but inserts the new enemy at an
    /// explicit `enemy_uids` index (clamped to the current length, so
    /// an out-of-range index appends). Bead `sts2-rs-z4jsg`: the
    /// Fabricator's bots fill C# encounter slots
    /// `[bot1, bot2, fabricator, bot3, bot4]` via `GetNextSlot` (first
    /// unused slot) + `SortEnemiesBySlotName`, so the first two bots
    /// land BEFORE the Fabricator and the rest after — a position
    /// neither [`Self::spawn_enemy`] (append) nor
    /// [`Self::spawn_enemy_at_front`] can express. Same slot-model
    /// TODO as `spawn_enemy_at_front`.
    pub fn spawn_enemy_at_index(&mut self, species: MonsterKind, index: usize) -> CreatureUid {
        self.spawn_enemy_at(species, index)
    }

    /// Record the encounter **slot index** for a freshly-spawned enemy
    /// (a position in the encounter's `Slots` list — C#
    /// `Creature.SlotName` → `Encounter.Slots.IndexOf`). Stored on the
    /// creature's [`crate::monster_ai::MonsterAi`] (excluded from
    /// `Creature` equality / parity). No-op if the uid names no live
    /// creature or it has no AI attached. Used by dynamic slot-filling
    /// summoners (Ovicopter) to reproduce C#
    /// `CombatState.SortEnemiesBySlotName`. Bead `sts2-rs-c8why`.
    pub fn set_enemy_slot(&mut self, uid: CreatureUid, slot: u8) {
        if let Some(c) = self.creatures.iter_mut().find(|c| c.id == uid.0) {
            if let Some(ai) = c.ai.as_mut() {
                ai.set_slot(slot);
            }
        }
    }

    /// Stamp the encounter's slot model onto a freshly-built combat:
    /// `member_slots[i]` is the slot the i-th spawned enemy occupies, C#
    /// `GenerateMonsters` returning `(model, slotName)` pairs with the name
    /// resolved to its `Slots` index.
    ///
    /// Call once at combat entry, after the roster is in `enemy_uids`. An
    /// encounter with no `Slots` passes an empty slice and this is a no-op.
    ///
    /// Does NOT reorder anything: every shipped `GenerateMonsters` already
    /// returns its members in slot-ascending order, so the spawn order IS the
    /// sorted order and stamping is pure bookkeeping. What it buys is the
    /// ability to place a LATER summon correctly — see
    /// [`Self::spawn_enemy_in_slot`]. GH #552.
    ///
    /// The slot COUNT deliberately lives with each summoner rather than on
    /// `CombatState`: it is a constant of the encounter the monster belongs to
    /// (`TWO_TAILED_RAT_SLOTS`, `EGG_SLOT_COUNT`, …), and putting it on the
    /// state would add a field to every one of the ~60 `CombatState` literals
    /// in the workspace and to its `PartialEq`.
    pub fn assign_encounter_slots(&mut self, member_slots: &[u8]) {
        for (uid, &slot) in self.enemy_uids.clone().iter().zip(member_slots.iter()) {
            self.set_enemy_slot(*uid, slot);
            // `set_enemy_slot` stores on the creature's `MonsterAi` (kept off
            // `Creature` so it stays out of equality / parity), so it silently
            // no-ops on an AI-less creature — and a half-stamped encounter
            // degrades to append-on-summon, which is the bug this whole model
            // exists to fix. Every real combat-entry path attaches AIs first;
            // fail loudly in debug rather than lose the ordering quietly.
            debug_assert_eq!(
                self.enemy_slot(*uid),
                Some(slot),
                "slot {slot} did not stick on {uid:?} — attach the MonsterAi before stamping slots"
            );
        }
    }

    /// Slot indices held by LIVING enemies.
    ///
    /// Liveness is the whole subtlety. C# removes a corpse from `_enemies`
    /// (`ShouldCreatureBeRemovedFromCombatAfterDeath` defaults to `true`), so a
    /// dead monster's slot frees up and a later summon can take it. Rust
    /// retains corpses in `enemy_uids` (bead `sts2-rs-c8why`), so this filter
    /// does by hand what C# gets from the removal.
    ///
    /// # The exception, and why not every summoner should use this
    ///
    /// `CreatureCmd`'s removal is guarded by `!monster.IsPerformingMove`
    /// (`CreatureCmd.cs:525-529`): a monster that kills ITSELF during its own
    /// move stays in `_enemies`, so its slot is NOT recycled. LivingFog's
    /// GasBombs die that way — inside their own EXPLODE — which is why
    /// `living_fog_bloat_effect` counts bombs (dead included) instead of
    /// calling into these helpers, and why the fight is capped at five bombs
    /// total rather than five at a time.
    ///
    /// So: before putting a summoner on the slot model, check how its spawns
    /// die. Killed by the player (Two-Tailed Rats, Ovicopter eggs) → slot
    /// recycles, use these helpers. Self-killed mid-move → it does not.
    fn occupied_enemy_slots(&self) -> SmallVec<[u8; 8]> {
        self.enemy_uids
            .iter()
            .filter_map(|u| {
                let c = self.creatures.iter().find(|c| c.id == u.0)?;
                if c.current_hp > 0 {
                    self.enemy_slot(*u)
                } else {
                    None
                }
            })
            .collect()
    }

    /// C# `EncounterModel.GetNextSlot` — `Slots.FirstOrDefault(free)`, the
    /// LOWEST-index slot no living enemy holds. `None` when the encounter has
    /// no slot model or every slot is taken.
    ///
    /// Note C# has TWO different rules and they are not interchangeable:
    /// `GetNextSlot` is first-free, while the summoners that actually place a
    /// creature (`TwoTailedRat.CallForBackup`, `Ovicopter.LayEggsMove`) inline
    /// their own `Slots.LastOrDefault(free)`. `GetNextSlot` is mostly used as
    /// an "is there room?" test, where the two agree; when you are choosing
    /// where a summon GOES, check which one the C# call site uses.
    #[must_use]
    pub fn first_free_enemy_slot(&self, slot_count: u8) -> Option<u8> {
        let occupied = self.occupied_enemy_slots();
        (0..slot_count).find(|s| !occupied.contains(s))
    }

    /// `Slots.LastOrDefault(free)` — the HIGHEST-index slot no living enemy
    /// holds. See [`Self::first_free_enemy_slot`] for why both exist.
    #[must_use]
    pub fn last_free_enemy_slot(&self, slot_count: u8) -> Option<u8> {
        let occupied = self.occupied_enemy_slots();
        (0..slot_count).rev().find(|s| !occupied.contains(s))
    }

    /// The `enemy_uids` index at which inserting a creature in `slot` keeps
    /// the LIVING enemies slot-ascending — the effect of C#
    /// `SortEnemiesBySlotName` running on the add.
    ///
    /// An insert rather than a re-sort on purpose: every add goes through
    /// here, so the list is already ordered and a global sort would be a
    /// no-op with more ways to go wrong. Living enemies with no recorded slot
    /// never displace the newcomer (they sort last), so a half-stamped
    /// encounter degrades to append instead of scrambling.
    #[must_use]
    pub fn slot_insert_index(&self, slot: u8) -> usize {
        self.enemy_uids
            .iter()
            .position(|u| {
                let Some(c) = self.creatures.iter().find(|c| c.id == u.0) else {
                    return false;
                };
                if c.current_hp <= 0 {
                    return false;
                }
                self.enemy_slot(*u).is_some_and(|key| key > slot)
            })
            .unwrap_or(self.enemy_uids.len())
    }

    /// Spawn a summoned enemy into `slot`, inserted so the living enemies stay
    /// slot-ascending, with `ai` attached and the slot recorded.
    ///
    /// This is the general form of what the Fogmog front-insert and the
    /// Ovicopter's hand-rolled scan were doing per-encounter. Use it for any
    /// mid-combat summon in a slotted encounter: `enemy_uids` order is turn
    /// order and the positional `target_index`, so getting it wrong is
    /// observable even when the fight is otherwise identical (GH #552).
    ///
    /// The AI is attached BEFORE the slot is recorded because
    /// [`Self::set_enemy_slot`] stores on the `MonsterAi` (kept off `Creature`
    /// so it stays out of equality / parity comparisons).
    pub fn spawn_enemy_in_slot(
        &mut self,
        species: MonsterKind,
        slot: u8,
        ai: crate::monster_ai::MonsterAi,
    ) -> CreatureUid {
        let at = self.slot_insert_index(slot);
        let uid = self.spawn_enemy_at_index(species, at);
        self.set_creature_ai(uid, ai);
        self.set_enemy_slot(uid, slot);
        uid
    }

    /// Read the encounter slot index recorded for `uid`, if any. Bead
    /// `sts2-rs-c8why`.
    #[must_use]
    pub fn enemy_slot(&self, uid: CreatureUid) -> Option<u8> {
        self.creatures
            .iter()
            .find(|c| c.id == uid.0)
            .and_then(|c| c.ai.as_ref())
            .and_then(crate::monster_ai::MonsterAi::slot)
    }

    /// Shared spawn body: push the Creature into the arena and insert
    /// its uid into `enemy_uids` at `enemy_index` (clamped to the
    /// current length, so an out-of-range index appends).
    fn spawn_enemy_at(&mut self, species: MonsterKind, enemy_index: usize) -> CreatureUid {
        // C# `CombatState.CreateCreature` calls `SetUniqueMonsterHpValue`
        // before the new creature joins `_enemies`, passing the enemies
        // already on side; it removes their MaxHp from the candidate range
        // so same-kind monsters get distinct HP (bead `sts2-rs-t6sh2`).
        //
        // DEFAULT `taken`: the MaxHp of every LIVING same-side enemy. C#
        // `SetUniqueMonsterHpValue` passes `_enemies`, which never contains a
        // dead creature — `RemoveCreature` strips a corpse from the array on
        // death (`ShouldCreatureBeRemovedFromCombatAfterDeath`). Rust retains
        // corpses in `enemy_uids` (bead `sts2-rs-c8why`), so the candidate
        // exclusion must filter `current_hp > 0` to match — otherwise a dead
        // egg's MaxHp wrongly shrinks the candidate set for a LATER summon,
        // shifting its `NextItem` value off C# (coverage seed 1334: a 2nd
        // OVICOPTER lay rolled {15,16,18} vs C#'s {14,15,17} because Rust's
        // dead first-batch egg lingered in `taken`; the Niche cursor stays in
        // lockstep — candidate COUNT never changes the 1-read-per-egg). The
        // DEATH-triggered respawn path still curates its own set — see
        // [`Self::spawn_enemy_at_index_with_taken`]. Bead `sts2-rs-egg-taken`.
        let taken: std::collections::HashSet<i32> = self
            .enemy_uids
            .iter()
            .filter_map(|u| {
                self.creatures
                    .iter()
                    .find(|c| c.id == u.0 && c.current_hp > 0)
                    .map(|c| c.max_hp)
            })
            .collect();
        self.spawn_enemy_at_with_taken(species, enemy_index, &taken)
    }

    /// Spawn variant accepting an explicit `taken` MaxHp exclusion set for the
    /// unique-HP roll, mirroring C# `CreateCreature` passing `_enemies` (minus
    /// the new creature) to `SetUniqueMonsterHpValue`. The death-triggered
    /// Axebot respawn (`StockPower.AfterDeath`) passes {alive enemies} ∪ {the
    /// dying owner} — EXCLUDING stale corpses from PRIOR deaths that C# already
    /// removed (`RemoveCreature` after `AfterDeath`) but Rust retains
    /// (`sts2-rs-c8why`). Without this, an Axebot fight with two+ respawns
    /// pollutes the respawn's candidate range with dead bots' old MaxHp and
    /// the Niche `NextItem` index drifts vs C# (bead `sts2-rs-etqsw`).
    pub fn spawn_enemy_at_index_with_taken(
        &mut self,
        species: MonsterKind,
        enemy_index: usize,
        taken: &std::collections::HashSet<i32>,
    ) -> CreatureUid {
        self.spawn_enemy_at_with_taken(species, enemy_index, taken)
    }

    fn spawn_enemy_at_with_taken(
        &mut self,
        species: MonsterKind,
        enemy_index: usize,
        taken: &std::collections::HashSet<i32>,
    ) -> CreatureUid {
        let raw =
            u32::try_from(self.creatures.len() + 1).expect("creature arena length fits in u32");
        let id = NonZeroU32::new(raw).expect("non-zero");
        let hp = species.starting_hp_unique(&mut self.run_rng.niche, taken, self.ascension);
        // Decimillipede segments run an extra C# `AfterAddedToRoom` maxHp
        // fix-up (round up to even + dedup vs already-spawned teammates).
        // `taken` is exactly the set of prior same-side maxHp values C#
        // compares against, and the roll above already excluded it, so the
        // Niche cursor is unaffected. Identity for every other species
        // (bead `sts2-rs-8mpo1`).
        let hp = species.finalize_spawn_max_hp(hp, taken, self.ascension);
        self.creatures.push(Creature {
            id,
            current_hp: hp,
            max_hp: hp,
            block: 0,
            powers: SmallVec::new(),
            species: Some(species),
            monster_id: MonsterId(species.python_name()),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        let uid = CreatureUid(id);
        let at = enemy_index.min(self.enemy_uids.len());
        self.enemy_uids.insert(at, uid);
        // Bead `sts2-rs-3jo9`: apply the per-species static on-spawn
        // self-buffs that Python's `create_<monster>` factories
        // perform via `creature.apply_power(...)` at construction
        // time. Empty for most species; non-empty for 5 act1/act2
        // monsters (see [`MonsterKind::on_spawn_self_buffs`]).
        self.apply_creature_spawn_buffs(uid);
        uid
    }

    /// Apply the static on-spawn self-buffs from
    /// [`MonsterKind::on_spawn_self_buffs`] to a freshly-pushed
    /// creature. Idempotent on the buff *set*: re-calling stacks the
    /// amounts (matches the existing
    /// [`crate::card_effects::apply_power_to_creature`] stack policy);
    /// callers should invoke once per spawn.
    ///
    /// `spawn_enemy` calls this automatically. Other entry paths that
    /// push creatures directly (scenario builders, parity-fixture
    /// setup, test harnesses) should call this immediately after the
    /// push to maintain spawn-time-buff parity with the Python pin.
    /// Bead `sts2-rs-3jo9`.
    pub fn apply_creature_spawn_buffs(&mut self, uid: CreatureUid) {
        let Some(species) = self
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .and_then(|c| c.species)
        else {
            return;
        };
        // C# resolves a few `AfterAddedToRoom` amounts through
        // `GetValueIfAscension(DeadlyEnemies, …)` (CorpseSlug's Ravenous,
        // TestSubject's Enrage); the static table above is the A0 value.
        // `ToughEnemies` (A8) scales others (Vantom's Slippery, the Platings,
        // Skittish, Shriek, Curl Up).
        let deadly = self.ascension >= crate::ascension::AscensionLevel::DeadlyEnemies as u8;
        let tough = self.ascension >= crate::ascension::AscensionLevel::ToughEnemies as u8;
        for &(kind, amount) in species.on_spawn_self_buffs() {
            let amount = if deadly {
                species.on_spawn_self_buff_deadly(kind).unwrap_or(amount)
            } else {
                amount
            };
            let amount = if tough {
                species.on_spawn_self_buff_tough(kind).unwrap_or(amount)
            } else {
                amount
            };
            crate::card_effects::apply_power_to_creature(self, uid, kind, amount);
        }
        // Bead `sts2-rs-ybtmu`: per-species static on-spawn powers the
        // monster lands on its opponents (the player) — C# `AfterAddedToRoom`
        // applies these to `GetOpponentsOf(Creature)`. Only Rocket's
        // `SurroundedPower(1)` at this pin. Routed to the player's mirror
        // Creature uid; a no-op if the player creature is not yet wired.
        let opponent_debuffs = species.on_spawn_opponent_debuffs();
        if !opponent_debuffs.is_empty() {
            if let Some(player_uid) = self.player.creature_uid {
                for &(kind, amount) in opponent_debuffs {
                    crate::card_effects::apply_power_to_creature(self, player_uid, kind, amount);
                }
            }
        }
        // Bead `sts2-rs-caz92`: per-species static on-spawn block grant
        // mirrors C# `CreatureCmd.GainBlock` inside `AfterAddedToRoom`.
        // Only CubexConstruct uses this at the current pin.
        let block = species.on_spawn_block();
        if block > 0 {
            if let Some(c) = self.creatures.iter_mut().find(|c| c.id == uid.0) {
                c.block = c.block.saturating_add(block);
            }
        }
    }

    /// Apply a single stack of [`PowerKind::DieForYou`] to the targeted
    /// creature if it doesn't already carry the power. Idempotent
    /// helper used by [`Self::summon_osty`]; not a general-purpose
    /// surface (Necrobinder pets are the only consumer at this pin).
    fn ensure_die_for_you_on(&mut self, uid: CreatureUid) {
        let Some(idx) = self.creatures.iter().position(|c| c.id == uid.0) else {
            return;
        };
        let already_has = self.creatures[idx].powers.iter().any(|puid| {
            self.powers
                .iter()
                .find(|p| p.id == puid.0)
                .is_some_and(|p| p.kind == PowerKind::DieForYou)
        });
        if already_has {
            return;
        }
        crate::card_effects::apply_power_to_creature(self, uid, PowerKind::DieForYou, 1);
    }

    /// Sync the player's [`Creature`] record from the live
    /// [`PlayerCombatState`] scalar fields. Used at combat teardown
    /// (and between Phase 2 partial migrations) to keep the two
    /// stores coherent while both exist.
    ///
    /// No-op when [`PlayerCombatState::creature_uid`] is `None`.
    /// Bead `sts2-rs-d03` Phase 1.
    pub fn sync_player_creature(&mut self) {
        let Some(uid) = self.player.creature_uid else {
            return;
        };
        let Some(idx) = self.creatures.iter().position(|c| c.id == uid.0) else {
            // creature_uid pointed at a missing record — treat as
            // out-of-sync state, clear the dangling handle.
            self.player.creature_uid = None;
            return;
        };
        let c = &mut self.creatures[idx];
        c.current_hp = self.player.current_hp;
        c.max_hp = self.player.max_hp;
        c.block = self.player.block;
        c.powers.clone_from(&self.player.powers);
    }

    /// Tear down the player's [`Creature`] record at combat end:
    /// flush the in-Creature scalars back to
    /// [`PlayerCombatState`] (Phase 2 callers will already have the
    /// Creature as the source of truth — for Phase 1 this is a
    /// no-op-equivalent sync), remove the record from
    /// [`Self::creatures`], and clear
    /// [`PlayerCombatState::creature_uid`].
    ///
    /// Mirrors the C# `Combat.OnCombatEnd` cleanup that drops the
    /// per-combat `Creature` reference on the run's `Player` object.
    /// Bead `sts2-rs-d03` Phase 1.
    pub fn teardown_player_creature(&mut self) {
        let Some(uid) = self.player.creature_uid else {
            return;
        };
        if let Some(idx) = self.creatures.iter().position(|c| c.id == uid.0) {
            // Flush Creature → PlayerCombatState. Phase 1: the
            // PlayerCombatState fields are already the live source
            // of truth so this is observably a no-op when called
            // after [`Self::sync_player_creature`]; the explicit
            // copy is here so Phase 2 can rely on this teardown
            // even after the directionality flips.
            let c = &self.creatures[idx];
            self.player.current_hp = c.current_hp;
            self.player.max_hp = c.max_hp;
            self.player.block = c.block;
            self.player.powers.clone_from(&c.powers);
            self.creatures.remove(idx);
        }
        self.player.creature_uid = None;
    }

    /// Summon a freshly-built monster into combat mid-fight.
    ///
    /// Allocates a new `CreatureUid`, pushes a [`Creature`] record onto
    /// [`Self::creatures`], registers the uid in [`Self::enemy_uids`],
    /// and fires the `after_creature_added_to_combat` hook so listeners
    /// (Bound Phylactery, etc.) observe the spawn. Returns the new uid.
    ///
    /// This is the engine-side primitive cards (e.g. `SummonForth`-style
    /// future content) and relics (e.g. `BoundPhylactery`'s Osty spawn)
    /// reach for to add hostile creatures during combat. It mirrors
    /// Python's `combat.add_enemy(creature, ai)` at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:549-559` — the
    /// canonical mid-combat add path used by act3's Soul Nexus
    /// (`SUMMON` move spawns an Osty) and the Queen boss
    /// (`summon_guard` spawns a Royal Guard).
    ///
    /// Monster-AI surfaces live in `sts2-content::monster_dispatch::build_ai`;
    /// callers there own the `kind → MonsterAi` resolution and pass the
    /// built `MonsterAi` in. `hp` is the spawn HP (used as both
    /// `current_hp` and `max_hp` — matches the Python
    /// `Creature(max_hp=hp)` constructor pattern at the pin).
    ///
    /// **Player-summon semantics** (Osty pets) are out of scope here —
    /// those use a separate `summon_osty` path because they push to
    /// `state.allies` and apply `DieForYou`. This primitive covers the
    /// general "add a monster on the enemy side" surface used by
    /// content-side card and relic effects.
    ///
    /// Bead `sts2-rs-d7j` (G3 plumbing under `sts2-rs-kng`).
    // Port-pin: sts2-rl-agent@01c094d:sts2_env/core/combat.py:549
    pub fn summon_monster(&mut self, kind: MonsterKind, hp: i32, ai: MonsterAi) -> CreatureUid {
        // Allocate a fresh non-zero id distinct from any existing
        // creature in the arena. Mirrors `init_player_creature`'s
        // sequential-id scheme.
        let raw =
            u32::try_from(self.creatures.len() + 1).expect("creature arena length fits in u32");
        let id = NonZeroU32::new(raw).expect("non-zero");
        let uid = CreatureUid(id);
        self.creatures.push(Creature {
            id,
            current_hp: hp,
            max_hp: hp,
            block: 0,
            powers: SmallVec::new(),
            species: Some(kind),
            monster_id: MonsterId(kind.python_name()),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: Some(ai),
        });
        self.enemy_uids.push(uid);
        // Fire the spawn hook so listeners (e.g. relics / powers that
        // gate on creature additions) observe the new enemy. Python's
        // `add_enemy` only fires this when the combat has already
        // started (`if self._combat_started`); the Rust port uses the
        // primitive exclusively from mid-combat call sites (cards /
        // relics react to in-combat events), so the pre-start gate is
        // not needed here.
        let ctx = crate::hooks::CreatureAddedCtx::default();
        crate::hooks::dispatch_after_creature_added_to_combat(self, &ctx);
        uid
    }

    /// Lightweight structural sanity check for state-injection
    /// constructors (bead `sts2-rs-osyf` hint-T2).
    ///
    /// Returns `Ok(())` when every internal invariant holds, else an
    /// `Err(String)` describing the first violation. Cheap (O(n) over
    /// the creature / card / power arenas) so callers can run it after
    /// every [`crate::apply`] call in fuzzers / parity harnesses
    /// without measurable overhead. Production combat does not call
    /// this — the engine maintains the invariants by construction.
    ///
    /// Checked properties (non-exhaustive — add new entries here when
    /// new structural rules are introduced):
    ///
    /// 1. **Player creature link.** If `player.creature_uid` is
    ///    `Some(uid)`, then `creatures` must contain a record with
    ///    matching `id`.
    /// 2. **Enemy uid resolution.** Every `uid` in `enemy_uids` must
    ///    resolve to an entry in `creatures`.
    /// 3. **HP bounds.** For every creature, `0 <= current_hp <= max_hp`
    ///    and `max_hp > 0`. Block is non-negative.
    /// 4. **Player scalar bounds.** `0 <= player.current_hp <=
    ///    player.max_hp`, `player.energy >= 0`, `player.block >= 0`,
    ///    `player.max_energy >= 0`.
    /// 5. **Power uid resolution.** Every `PowerUid` referenced from a
    ///    creature's `powers` SmallVec or from the player must resolve
    ///    to an entry in `powers`.
    /// 6. **Card uid resolution.** Every `CardUid` in any player pile
    ///    (`hand` / `draw` / `discard` / `exhaust` / `play`) must
    ///    resolve to an entry in `cards`.
    /// 7. **No pile aliasing.** A single `CardUid` must not appear in
    ///    more than one player pile simultaneously.
    /// 8. **Active-attack handle resolution.** Both the attacker and
    ///    target uids on [`Self::active_attack`] (when `Some`) must
    ///    resolve to entries in `creatures` (`None` legs skipped).
    ///
    /// # Errors
    ///
    /// Returns a one-line `String` description of the first invariant
    /// violation encountered. The message format is intentionally
    /// loose — callers should treat it as a diagnostic, not a stable
    /// API.
    pub fn assert_invariants(&self) -> Result<(), String> {
        // Player creature link.
        if let Some(uid) = self.player.creature_uid {
            if !self.creatures.iter().any(|c| c.id == uid.0) {
                return Err(format!(
                    "player.creature_uid {} not found in creatures arena",
                    uid.get()
                ));
            }
        }

        // Enemy uid resolution.
        for euid in &self.enemy_uids {
            if !self.creatures.iter().any(|c| c.id == euid.0) {
                return Err(format!(
                    "enemy uid {} not found in creatures arena",
                    euid.get()
                ));
            }
        }

        // Creature HP / block bounds.
        for c in &self.creatures {
            if c.max_hp <= 0 {
                return Err(format!("creature {}: max_hp <= 0", c.id.get()));
            }
            if c.current_hp < 0 {
                return Err(format!("creature {}: current_hp < 0", c.id.get()));
            }
            if c.current_hp > c.max_hp {
                return Err(format!(
                    "creature {}: current_hp ({}) > max_hp ({})",
                    c.id.get(),
                    c.current_hp,
                    c.max_hp
                ));
            }
            if c.block < 0 {
                return Err(format!("creature {}: block < 0", c.id.get()));
            }
            // Power uid resolution for this creature.
            for puid in &c.powers {
                if !self.powers.iter().any(|p| p.id == puid.0) {
                    return Err(format!(
                        "creature {}: power uid {} not in powers arena",
                        c.id.get(),
                        puid.get()
                    ));
                }
            }
        }

        // Player scalar bounds (legacy fields — the d03 accessors
        // route through `creatures[creature_uid]` when set, but the
        // legacy fields must still be sane on their own).
        if self.player.max_hp <= 0 {
            return Err("player.max_hp <= 0".to_string());
        }
        if self.player.current_hp < 0 {
            return Err("player.current_hp < 0".to_string());
        }
        if self.player.current_hp > self.player.max_hp {
            return Err(format!(
                "player.current_hp ({}) > player.max_hp ({})",
                self.player.current_hp, self.player.max_hp
            ));
        }
        if self.player.block < 0 {
            return Err("player.block < 0".to_string());
        }
        if self.player.energy < 0 {
            return Err("player.energy < 0".to_string());
        }
        if self.player.max_energy < 0 {
            return Err("player.max_energy < 0".to_string());
        }

        // Player power uid resolution.
        for puid in &self.player.powers {
            if !self.powers.iter().any(|p| p.id == puid.0) {
                return Err(format!(
                    "player.powers: power uid {} not in powers arena",
                    puid.get()
                ));
            }
        }

        // Card uid resolution per pile + no aliasing across piles.
        let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for (label, pile) in [
            ("hand", self.player.hand.as_slice()),
            ("draw", self.player.draw.as_slice()),
            ("discard", self.player.discard.as_slice()),
            ("exhaust", self.player.exhaust.as_slice()),
            ("play", self.player.play.as_slice()),
        ] {
            for cuid in pile {
                if !self.cards.iter().any(|c| c.uid == *cuid) {
                    return Err(format!(
                        "player.{}: card uid {} not in cards arena",
                        label,
                        cuid.get()
                    ));
                }
                if !seen.insert(cuid.get()) {
                    return Err(format!(
                        "card uid {} appears in multiple piles (last seen in `{}`)",
                        cuid.get(),
                        label
                    ));
                }
            }
        }

        // Active-attack handle resolution.
        if let Some(attack) = &self.active_attack {
            if let Some(a) = attack.attacker {
                if !self.creatures.iter().any(|c| c.id == a.0) {
                    return Err(format!(
                        "active_attack.attacker uid {} not in creatures arena",
                        a.get()
                    ));
                }
            }
            if let Some(t) = attack.target {
                if !self.creatures.iter().any(|c| c.id == t.0) {
                    return Err(format!(
                        "active_attack.target uid {} not in creatures arena",
                        t.get()
                    ));
                }
            }
        }

        Ok(())
    }
}

impl CombatState {
    /// Append `card_id` to the player's [`Self::master_deck`], cache it
    /// in [`Self::last_added_card`], and broadcast the
    /// [`OnCardAddedToDeckFn`](crate::relic_effects::OnCardAddedToDeckFn)
    /// event to every relic listener registered on the channel.
    ///
    /// Bead `sts2-rs-anx` (P7-surf). This is the canonical "add card to
    /// master deck" call-site — every site that needs to grow the run's
    /// deck must route through here so the bus fan-out is uniform.
    ///
    /// Mirrors `sts2-rl-agent@01c094d:sts2_env/run/run_state.py::
    /// RunState.add_card_instance_to_deck` (lines 297-306). The Python
    /// body:
    ///   1. appends to `self.deck`,
    ///   2. walks `self.get_relic_objects()` and calls each relic's
    ///      `on_card_added_to_deck(self)` if defined,
    ///   3. special-cases `DARKSTONE_PERIAPT` for `CardType::CURSE`
    ///      additions (`self.gain_max_hp(6)`).
    ///
    /// The Rust port (this pin) implements step 1 + 2; step 3
    /// (curse-type-conditional max-HP bonus) lands with bead
    /// `sts2-rs-3f8` (which plumbs `CardTag::Curse` queries on
    /// `CardInstance`). See audit doc §2 entries for `BingBong` and
    /// `DarkstonePeriapt`.
    ///
    /// **Re-entry note:** the bus is dispatched *after* the push +
    /// `last_added_card` cache update, so a listener that calls
    /// [`Self::duplicate_last_added_card`] from inside its handler
    /// observes the freshly-added card as the source for the
    /// duplicate. `BingBong`'s Python body adds an `_skip_next` guard
    /// on the relic body (`shop_event.py:828-836`) to break the
    /// infinite recursion that would otherwise occur — the guard is
    /// the relic body's responsibility, not the dispatcher's. The bus
    /// itself is plain (no implicit re-entry counter on
    /// [`CombatState`]). The umbrella retrofit `sts2-rs-01o` ports the
    /// per-relic `_skip_next` field when `BingBong`'s body is filled
    /// in; until then [`Self::duplicate_last_added_card`] called from
    /// a listener body would recurse. The convention `BingBong` relies
    /// on at the umbrella-retrofit pin is
    /// `ctx.source != AddSource::BingBongDuplicate`; see the
    /// `re_entry_guard_stable` test in `relic_effects` for the pin.
    pub fn add_to_master_deck(
        &mut self,
        card_id: crate::cards::CardId,
        source: crate::relic_effects::AddSource,
    ) {
        self.master_deck.push(card_id);
        self.last_added_card = Some(card_id);
        let ctx = crate::relic_effects::CardAddedCtx { card_id, source };
        let _ = crate::relic_effects::dispatch_on_card_added_to_deck(self, &ctx);
    }

    /// Clone the last-added [`Self::master_deck`] entry by re-adding the
    /// same [`CardId`](crate::cards::CardId) through
    /// [`Self::add_to_master_deck`] with
    /// [`AddSource::BingBongDuplicate`](crate::relic_effects::AddSource::BingBongDuplicate).
    ///
    /// Returns `true` iff a duplicate was actually appended; returns
    /// `false` when [`Self::last_added_card`] is `None` (no prior add).
    ///
    /// Bead `sts2-rs-anx` (P7-surf). Mirrors
    /// `sts2-rl-agent@01c094d:sts2_env/run/run_state.py::
    /// RunState.duplicate_last_added_card` at lines 463-467:
    ///
    /// ```python
    /// def duplicate_last_added_card(self) -> bool:
    ///     if not self.deck:
    ///         return False
    ///     self.add_card_instance_to_deck(
    ///         self.deck[-1].clone(20_000_000 + len(self.deck))
    ///     )
    ///     return True
    /// ```
    ///
    /// The Python `clone(seed)` produces a distinct `CardInstance` with
    /// a fresh `uid_seed`. The Rust port at this pin clones only the
    /// `CardId` (the discriminant) — per-instance state lives in
    /// [`Self::cards`] for in-combat cards, and the run-scope
    /// `Vec<CardId>` is sufficient for the listener semantics this
    /// bead targets. Richer `CardInstance.clone(seed)` plumbing lands
    /// with bead `sts2-rs-3f8`; the umbrella retrofit `sts2-rs-01o`
    /// will swap the inner call here without a public-API break.
    pub fn duplicate_last_added_card(&mut self) -> bool {
        let Some(card_id) = self.last_added_card else {
            return false;
        };
        self.add_to_master_deck(card_id, crate::relic_effects::AddSource::BingBongDuplicate);
        true
    }
}

impl CombatState {
    /// Open a pending card-selection prompt. Mirrors Python
    /// `combat.request_card_choice(prompt, cards, source_pile, resolver,
    /// allow_skip=False)` at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py` (called by Regent
    /// uncommons such as `Glimmer`, `CosmicIndifference`, `PhotonCut`,
    /// `Quasar`, `HeirloomHammer`, `DecisionsDecisions`, `Begone`).
    ///
    /// Bead `sts2-rs-yc2d`. Builds a [`PendingChoice`] from the supplied
    /// option list and stores it on `self.pending_choice`. The handler
    /// returns control to the combat loop; the caller (combat_loop)
    /// observes the open choice and surfaces it through
    /// [`legal_actions`], whose [`Action::ResolveChoice`] handler then
    /// dispatches per [`ChoiceKind`]. The `resolver` closure on the
    /// Python side is *not* stored on the choice — instead, the
    /// `ChoiceKind` discriminant selects the resolver in the dispatch
    /// trampoline (see `apply` / `resolve_putback_choice`).
    ///
    /// Behavior notes:
    /// * Empty `options` is a no-op — Python short-circuits at the
    ///   call-site (`if not combat.hand: return`); we mirror that here
    ///   defensively so callers don't need to repeat the guard. No
    ///   pending choice is opened.
    /// * `min_select` and `max_select` are clamped so that
    ///   `max_select >= min_select` and `max_select <= options.len()`.
    /// * If a pending choice is already open, this call **overwrites**
    ///   it. Python's `request_card_choice` asserts no nested choice;
    ///   the Rust port logs nothing today (no log surface) but the
    ///   parity harness should never trigger nesting at the pin.
    /// C# `CardSelectCmd.FromHand` (CardSelectCmd.cs:475) and
    /// `FromHandForDiscard` (which routes through `FromHand`) auto-resolve —
    /// selecting every candidate without surfacing a player prompt — when
    /// the selectable count is `<= MinSelect` for a forced (non-skippable)
    /// hand selection. These are the Rust [`ChoiceKind`]s backed by those
    /// commands. Kinds NOT listed here keep surfacing even when forced,
    /// matching their C# commands.
    const fn kind_autoresolves_when_forced(kind: ChoiceKind) -> bool {
        matches!(
            kind,
            ChoiceKind::ExhaustFromHand
                | ChoiceKind::DiscardFromHand
                | ChoiceKind::DiscardFromHandAndDraw
        )
    }

    /// Open a combat hand-selection [`PendingChoice`], or AUTO-RESOLVE it
    /// inline when C# `FromHand` would not surface a choice — a forced
    /// (`!can_skip`, `min_select >= 1`) selection whose candidate count is
    /// `<= min_select`. Rust previously always surfaced the choice, so a
    /// forced 1-card exhaust showed up as a spurious `card_select` decision
    /// where C# resolved it inline (RP101/RP119: cs=combat rs=card_select).
    /// Auto-resolution still fires the choice's `post_effect` (BurningPact's
    /// draw, Brand's Strength) via [`resolve_exhaust_from_hand`].
    pub fn open_hand_choice(&mut self, pc: PendingChoice) {
        let auto = Self::kind_autoresolves_when_forced(pc.kind)
            && !pc.can_skip
            && pc.min_select >= 1
            && (pc.options.len() as u8) <= pc.min_select;
        self.pending_choice = Some(pc);
        if auto {
            let all: Vec<u8> = self
                .pending_choice
                .as_ref()
                .map(|p| (0..p.options.len() as u8).collect())
                .unwrap_or_default();
            let _ = resolve_choice_action(self, &all, false);
        }
    }

    pub fn request_card_choice(
        &mut self,
        kind: ChoiceKind,
        mut options: Vec<ChoiceOption>,
        min_select: u8,
        max_select: u8,
        can_skip: bool,
    ) {
        if options.is_empty() {
            return;
        }
        // C# guards every `CardSelectCmd` variant on combat end —
        // `FromCombatPile` returns empty on `IsEnding`
        // (CardSelectCmd.cs:383) and `FromHand`/`FromHandForUpgrade` on
        // `IsOverOrEnding` (CardSelectCmd.cs:699/778) — so a select
        // requested after a lethal hit never surfaces: a killing
        // NeowsFury goes straight to the reward screen with no
        // discard-pick (shadow census v8 seed 1048, cs=card_reward
        // rs=card_select). Mirror the guard at the single request site.
        if self.is_over || crate::combat_loop::combat_is_ending(self) {
            return;
        }
        // Bead `sts2-rs-ve0oq`: PendingChoice indexes options with a
        // `u8`, so anything past slot 255 is unreachable. Truncate the
        // options vec to match before storing so the visible-options
        // length and the index space agree (the prior code silently
        // capped only `max_select` via `u8::try_from(...).unwrap_or(u8::MAX)`,
        // leaving the trailing entries in the vec but unselectable).
        // No production caller offers >255 options today; the
        // debug_assert flags any regression in test builds.
        debug_assert!(
            u8::try_from(options.len()).is_ok(),
            "PendingChoice options exceed u8 index space ({} > 255) — truncating",
            options.len(),
        );
        if options.len() > usize::from(u8::MAX) {
            options.truncate(usize::from(u8::MAX));
        }
        let len_u8 = u8::try_from(options.len()).expect("len capped at 255 above");
        let max_clamped = max_select.min(len_u8).max(1);
        let min_clamped = min_select.min(max_clamped);
        // Pile tracer (STS2_TRACE_PILES): emit the option list in PRESENTATION
        // ORDER so it diffs against the C# `SELECTTRACE` (the headless
        // `GetSelectedCards` order). For a draw-pile select C# re-sorts by
        // (Rarity, Id); Rust currently presents them however the card built
        // them — this line exposes the gap (#184).
        if crate::cards::pile_trace_enabled() {
            let rendered = options
                .iter()
                .map(|o| {
                    format!(
                        "{}{}",
                        o.card_id.to_python_name(),
                        if o.upgraded { "+" } else { "" }
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            eprintln!(
                "SELECTTRACE kind={kind:?} options=[{rendered}] min={min_clamped} max={max_clamped}"
            );
        }
        // Route through `open_hand_choice` so a forced FromHand-family
        // selection auto-resolves like C# instead of surfacing (RP101/119).
        self.open_hand_choice(PendingChoice {
            kind,
            options,
            min_select: min_clamped,
            max_select: max_clamped,
            can_skip,
            // Generic helper: post-effect choices (Brand/BurningPact)
            // construct PendingChoice directly. Bead `sts2-rs-pe5zo`.
            post_effect: None,
            // Fresh choice: nothing accumulated yet (bead `sts2-rs-sopgf`).
            picked: SmallVec::new(),
        });
    }

    /// Push one uniformly-sampled non-event, character-compatible potion
    /// onto `self.player.potions`. Returns `true` when a potion was added,
    /// `false` when the belt is at the player's
    /// [`PlayerCombatState::effective_potion_slots`] cap (C#
    /// `Player.MaxPotionCount`) or the eligible pool is empty.
    ///
    /// Mirrors Python `CombatState.procure_random_potion` at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:1835` in the
    /// `in_combat=True` branch (event-only filter + character-lock).
    /// Used by [`crate::CombatState::fill_empty_potion_slots`] and the
    /// `Alchemize` colorless card. Bead `sts2-rs-70anj.3.12`.
    pub fn procure_random_potion(&mut self) -> bool {
        use crate::potions::PotionKind;

        if self.player.potions.len() >= self.player.effective_potion_slots() {
            return false;
        }
        let player_char = self.player.character;
        let pool: smallvec::SmallVec<[PotionKind; 64]> = PotionKind::ALL
            .iter()
            .copied()
            .filter(|p| !p.is_event_only())
            .filter(|p| match (p.character_lock(), player_char) {
                (None, _) => true,
                (Some(_), None) => false,
                (Some(locked), Some(owned)) => locked == owned,
            })
            .collect();
        if pool.is_empty() {
            return false;
        }
        let pick = pool[self.rng.next_int_exclusive(0, pool.len() as i32) as usize];
        // First-free-slot insert (C# AddPotionInternal), not append — keeps
        // the visible belt in physical-slot order after mid-combat drinks.
        self.player.add_potion_first_free_slot(pick)
    }

    /// Fill empty potion slots with random potions until the belt hits
    /// the player's [`PlayerCombatState::effective_potion_slots`] cap or
    /// the eligible pool empties. Returns the number actually filled.
    ///
    /// Mirrors C# `EntropicBrew.OnUse` (`while Owner.HasOpenPotionSlots`,
    /// `MegaCrit.Sts2.Core.Models.Potions/EntropicBrew.cs:21`), where
    /// `HasOpenPotionSlots` is `_potionSlots.Any(p => p == null)` and
    /// `_potionSlots.Count == Player.MaxPotionCount` (default
    /// `initialMaxPotionSlotCount == 3`,
    /// `MegaCrit.Sts2.Core.Entities.Players/Player.cs:30`). Bead
    /// `sts2-rs-eysmt`: previously this collapsed to the hard
    /// [`MAX_POTIONS`](crate::potions::MAX_POTIONS) (= 5) because the
    /// combat-side carried no slot cap, so EntropicBrew over-filled a
    /// 3-slot belt to 5 and surfaced `use_potion` slots 3/4 C# never
    /// emits. Original port pin: `sts2-rs-70anj.3.12`.
    pub fn fill_empty_potion_slots(&mut self) -> i32 {
        let cap = self.player.effective_potion_slots();
        let mut filled = 0i32;
        while self.player.potions.len() < cap {
            if !self.procure_random_potion() {
                break;
            }
            filled += 1;
        }
        filled
    }
}

impl CombatState {
    /// Per-draw permanent `base_damage` increase for Regent
    /// `KinglyPunch` (`KinglyPunch.cs` `DynamicVar("Increase", 4)`).
    /// Bead `sts2-rs-8z4u4`.
    pub const KINGLY_PUNCH_DRAW_INCREASE: i32 = 4;

    /// Upgraded variant of [`Self::KINGLY_PUNCH_DRAW_INCREASE`]; C#
    /// `KinglyPunch.cs` OnUpgrade `Increase.UpgradeValueBy(2m)` →
    /// upgraded Increase = 6. Bead `sts2-rs-om4tt`.
    pub const KINGLY_PUNCH_DRAW_INCREASE_UPGRADED: i32 = 6;

    /// Draw up to `n` cards into the player's hand, firing the
    /// [`AfterShuffleFn`](crate::relic_effects::AfterShuffleFn) relic-bus
    /// channel once per auto-reshuffle that occurs during the draw.
    ///
    /// Returns the number of cards actually moved into the hand. Wraps
    /// [`crate::cards::draw_signaling`] — the canonical "draw N cards
    /// and broadcast shuffle events" surface for combat-loop callers.
    ///
    /// Bead `sts2-rs-jlc` (n3w-b). Mirrors Python's
    /// `_draw_cards_for_creature` + `fire_after_shuffle` pair at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:1102-1141`.
    pub fn draw_into_hand(&mut self, n: i32) -> usize {
        // Turn-start hand draw + relic draws credit `from_hand_draw = true`
        // (C# `CombatManager` hand draw). Card-EFFECT draws route through
        // [`Self::draw_into_hand_card_effect`], which passes `false` so
        // Speedster fires (`SpeedsterPower.AfterCardDrawn` gates on
        // `!fromHandDraw`). potion-autoplay fix 3.
        self.draw_into_hand_with_flag(n, true)
    }

    /// Shared body of [`Self::draw_into_hand`] with an explicit
    /// `from_hand_draw` flag (C# `CardPileCmd.Draw(.., fromHandDraw)`). The
    /// turn-start hand draw passes `true`; card-effect draws pass `false`.
    fn draw_into_hand_with_flag(&mut self, n: i32, from_hand_draw: bool) -> usize {
        // Bead `sts2-rs-63mew`: when a power-side `after_shuffle` choice
        // listener (Stratagem) is active, route through the C#-faithful
        // per-card loop that fires the shuffle hooks BEFORE each card is
        // taken and suspends the draw while the grid-select is open.
        // The common no-Stratagem combat keeps the batch path below
        // byte-for-byte (riders fire in the same aggregate order).
        if self.per_card_draw_loop_required() {
            return self.draw_suspending(n, from_hand_draw);
        }
        // C# `CardPileCmd.Draw` / `CardPileCmd.Shuffle` (CardPileCmd.cs:749,806)
        // early-return when `CombatManager.IsOverOrEnding`, so a turn-start hand
        // draw issued AFTER a `BeforeHandDraw` auto-play (Bombardment /
        // HowlFromBeyond) downs the last enemy performs NO `ShuffleIfNecessary`.
        // Cap the batch draw to the cards physically present when combat is
        // ending so the natural draw never reshuffles an exhausted pile — that
        // phantom reshuffle over-reads the run-level `shuffle` stream by `n-1`
        // and desyncs the NEXT combat's opening shuffle (whole-run parity: Regent
        // RP5 A5 combat-1 Bombardment kill → +4 shuffle reads → combat-2
        // opening-hand divergence → constant enemies_hp skew). The master deck is
        // tracked separately from the combat piles, so the capped draw cannot
        // change the next combat's pre-shuffle order. Mirrors the card-effect
        // draw gate (`cards::draw_via_state`) + the `draw_suspending`
        // skip-reshuffle-when-ending idiom (PARITY86); reads no RNG either way.
        // While combat is LIVE `combat_is_ending` is false → normal reshuffle.
        let n = if crate::combat_loop::combat_is_ending(self) {
            n.min(i32::try_from(self.player.draw.len()).unwrap_or(i32::MAX))
        } else {
            n
        };
        let hand_len_before = self.player.hand.len();
        let (moved, shuffles) = crate::cards::draw_signaling(
            &mut self.player,
            n,
            &self.cards,
            &mut self.run_rng.shuffle,
        );
        // bead sts2-rs-o05r: per-combat cards-drawn counter. Saturate
        // at `u16::MAX` so degenerate infinite-draw scenarios can't alias
        // small numbers; the eval term that consumes this (sts2-rs-95o9)
        // converts to f32 so saturation is observationally fine.
        if let Ok(moved_u16) = u16::try_from(moved) {
            self.cards_drawn = self.cards_drawn.saturating_add(moved_u16);
        } else {
            self.cards_drawn = u16::MAX;
        }
        if shuffles > 0 {
            let ctx = crate::relic_effects::ShuffleCtx {
                from_reshuffle: true,
            };
            for _ in 0..shuffles {
                let _ = crate::relic_effects::dispatch_after_shuffle_bus(self, &ctx);
            }
            // Bead `sts2-rs-6ynt.11`: enchantment
            // `modify_shuffle_order` channel. Mid-combat reshuffle —
            // PerfectFit-marked cards bubble to the top. Splits the
            // borrow with a temporary draw buffer so the dispatcher
            // can mutate the order while reading from `self.cards`.
            //
            // Empty-bus early-out (perf-investigation 2026-06-07):
            // `dispatch_modify_shuffle_order` is a no-op unless a card
            // in the draw pile carries an enchantment, but the two
            // clones below (the whole card store + the draw buffer) ran
            // unconditionally on every reshuffle — and reshuffles are a
            // top engine cost in MCTS rollouts (35k+ per search on the
            // starter-deck fixture, none of which carry enchantments).
            // Hoisting the same check the dispatcher does in front of
            // the clones keeps this byte-identical while skipping the
            // allocations whenever no card is enchanted.
            let any_enchanted = self.player.draw.iter().any(|uid| {
                self.cards
                    .get((uid.get() as usize).saturating_sub(1))
                    .is_some_and(|c| !c.enchantments.is_empty())
            });
            if any_enchanted {
                let cards_snapshot: Vec<crate::cards::Card> = self.cards.to_vec();
                let mut draw_buf: smallvec::SmallVec<[crate::CardUid; 32]> =
                    self.player.draw.clone();
                crate::enchantment_hooks::dispatch_modify_shuffle_order(
                    &mut self.player,
                    &mut draw_buf,
                    false,
                    &cards_snapshot,
                );
                self.player.draw = draw_buf;
            }
        }
        // Bead `sts2-rs-6ynt.11`: enchantment `on_card_drawn` channel.
        // Fires once per card actually moved into hand with
        // `from_hand_draw = true` (this is the canonical hand-draw site
        // — `try_add_to_hand` callers do NOT pass `from_hand_draw`).
        // Empty-bus early-out lives inside the dispatcher.
        if moved > 0 {
            let drawn: smallvec::SmallVec<[crate::CardUid; 8]> = self.player.hand
                [hand_len_before..]
                .iter()
                .copied()
                .collect();
            for uid in drawn {
                crate::enchantment_hooks::dispatch_on_card_drawn(self, uid, from_hand_draw);
            }
        }
        // Bead `sts2-rs-6ynt.3.2`: per-card on-draw effect for `CardId::Void`.
        // Mirrors Python `CombatState._invoke_card_drawn` at
        // `sts2-rl-agent@01c094d:sts2_env/core/combat.py:2322-2330`, which
        // drains 1 energy (clamped to >=0) for every Void that lands in hand
        // during a draw. The full `on_card_drawn` power-bus dispatch is
        // deferred under `sts2-rs-h9u-8` (placeholder context); this special
        // case restores Python parity in the meantime so MCTS can't exploit
        // a cost-free Void.
        self.invoke_card_drawn_for_newly_drawn(hand_len_before, from_hand_draw);
        moved
    }

    /// [`Self::draw_into_hand`] for CARD-EFFECT draws (hook bodies, potion
    /// resolvers, relic listeners — anything that is NOT the turn-start hand
    /// draw). C# routes EVERY draw through `Hook.ShouldDraw`
    /// (`CardPileCmd.cs:753`); `NoDrawPower`/`ScrutinyPower` veto all
    /// `fromHandDraw == false` draws, so a vetoed effect draws nothing —
    /// crucially it also never triggers a reshuffle (sweep seeds 2335/2563:
    /// a GamblersBrew / DarkEmbrace draw under Battle Trance's NO_DRAW
    /// spuriously reshuffled the discard pile and desynced the shuffle
    /// stream). Same gate as [`cards::draw_via_state`]; this variant keeps
    /// `draw_into_hand`'s full rider/shuffle-signaling behavior.
    pub fn draw_into_hand_card_effect(&mut self, n: i32) -> usize {
        if crate::cards::card_effect_draw_blocked(self) {
            // C# `NoDrawPower.cs:29` flashes inside `ShouldDraw` on the path
            // that returns false — `!fromHandDraw && player == Owner.Player`.
            // `Hook.ShouldDraw` (Hook.cs:2413) short-circuits on the FIRST
            // listener to veto, and `card_effect_draw_blocked` checks NoDraw
            // before Fiddle, so NoDraw is the preventer exactly when it is
            // present. The turn-start hand draw never reaches here.
            if crate::event_log::armed(self) && self.player_power_amount(PowerKind::NoDraw) > 0 {
                crate::event_log::flash_player_power(self, PowerKind::NoDraw);
            }
            return 0;
        }
        // Card-effect draws are `fromHandDraw = false` in C# — so Speedster
        // (`SpeedsterPower.AfterCardDrawn`, gated on `!fromHandDraw`) deals its
        // damage on these draws, unlike the turn-start hand draw. potion-autoplay
        // fix 3 (seed threading through the shared body).
        self.draw_into_hand_with_flag(n, false)
    }

    /// `true` when a power-side `after_shuffle` listener that can open a
    /// player choice is active — currently only Stratagem
    /// (C# `StratagemPower.AfterShuffle` grid-select). Read off the
    /// mirror+legacy power union via [`Self::player_power_amount`].
    /// Gates the C#-faithful suspending draw loop
    /// ([`Self::draw_suspending`]); the common no-Stratagem combat keeps
    /// the batch draw paths untouched. Bead `sts2-rs-63mew`.
    #[must_use]
    pub(crate) fn after_shuffle_choice_listener_active(&self) -> bool {
        self.player_power_amount(PowerKind::Stratagem) > 0
    }

    /// `true` when the draw must run the C#-faithful PER-CARD loop
    /// ([`Self::draw_suspending`]) instead of the batch path:
    ///
    /// * an `after_shuffle` choice listener (Stratagem) is active — the
    ///   grid-select must see the post-reshuffle pile mid-draw (bead
    ///   `sts2-rs-63mew`); or
    /// * HellraiserPower is active — C# `AfterCardDrawnEarly`
    ///   (`HellraiserPower.cs:34-42`) auto-plays each drawn Strike
    ///   *inside* the draw loop, so the played Strike lands in discard
    ///   BEFORE the next `ShuffleIfNecessary` check and can be
    ///   reshuffled back and drawn (and auto-played) again within the
    ///   SAME draw. The batch path defers the auto-play to after all
    ///   draws, missing that cascade (HELLRAISER+1 quick-sweep
    ///   divergence, seed 40 turn 4; bead `sts2-rs-bsj93`).
    ///
    /// Read off the mirror+legacy power union via
    /// [`Self::player_power_amount`].
    #[must_use]
    pub(crate) fn per_card_draw_loop_required(&self) -> bool {
        self.after_shuffle_choice_listener_active()
            || self.player_power_amount(PowerKind::Hellraiser) > 0
    }

    /// C#-faithful per-card draw loop, used when an `after_shuffle`
    /// choice listener (Stratagem) is active. Port of
    /// `CardPileCmd.Draw` (`CardPileCmd.cs:747-802`): each iteration
    /// checks the hand cap, runs `ShuffleIfNecessary` (auto-reshuffle +
    /// `Hook.ModifyShuffleOrder` + relic `after_shuffle` bus + the
    /// power-side `after_shuffle` walk) and only THEN takes the next
    /// card — so a Stratagem grid-select sees the full post-reshuffle
    /// pile before any of the remaining draws. If the shuffle walk
    /// opens a `pending_choice`, the not-yet-drawn remainder is stashed
    /// on [`Self::pending_draw_resume`] and the loop returns; the
    /// `apply(Action::ResolveChoice)` arm resumes it (mirroring C#'s
    /// `await` inside the draw loop).
    ///
    /// Per-card riders fire as each card lands (C# `Hook.AfterCardDrawn`
    /// per draw): the enchantment `on_card_drawn` channel (hand-draw
    /// path only, matching the batch paths' coverage), the
    /// Void/Confused/Pagestorm walk, and the `cards_drawn` counter.
    /// `from_hand_draw` keeps the per-path rider sets identical to
    /// [`Self::draw_into_hand`] / [`cards::draw_via_state`] so the gate
    /// flips ONLY the hook timing + suspension, not rider coverage.
    /// Bead `sts2-rs-63mew`.
    pub(crate) fn draw_suspending(&mut self, n: i32, from_hand_draw: bool) -> usize {
        let mut remaining = n.max(0);
        let mut moved = 0usize;
        while remaining > 0 {
            if crate::cards::is_hand_full(&self.player) {
                break;
            }
            // C# `CardPileCmd.Shuffle` (`CardPileCmd.cs:806`) early-returns on
            // `IsOverOrEnding`: when an on-draw auto-play (Hellraiser) downs the
            // last enemy MID-DRAW, C# does NOT reshuffle the discard pile for
            // the remaining requested cards. Rust defers the `is_over` flip to
            // the turn-start tail (`finish_player_turn_setup` Step 10), so
            // `is_over` is still false here even though the enemy just died —
            // mirror C# with the pure `combat_is_ending` read so we skip the
            // phantom reshuffle. Without it the over-read desyncs the run-level
            // `shuffle` stream into every later combat (PARITY86 Ironclad: +12
            // RNG in combat 1 round 5 → combat 2 opening-hand divergence). The
            // empty draw pile then `break`s the loop below (matching C#, whose
            // post-no-shuffle `drawPile.FirstOrDefault()` is null). Bead
            // `sts2-rs-draw-end-reshuffle`.
            if self.player.draw.is_empty() && !crate::combat_loop::combat_is_ending(self) {
                let shuffled = crate::cards::discard_pile_into_draw_pile_signaling(
                    &mut self.player,
                    &self.cards,
                    &mut self.run_rng.shuffle,
                );
                if shuffled {
                    // C# `CardPileCmd.Shuffle` order: StableShuffle →
                    // `Hook.ModifyShuffleOrder` → cards land in pile →
                    // `Hook.AfterShuffle` (relics + powers).
                    let cards_snapshot: Vec<crate::cards::Card> = self.cards.to_vec();
                    let mut draw_buf: smallvec::SmallVec<[crate::CardUid; 32]> =
                        self.player.draw.clone();
                    crate::enchantment_hooks::dispatch_modify_shuffle_order(
                        &mut self.player,
                        &mut draw_buf,
                        false,
                        &cards_snapshot,
                    );
                    self.player.draw = draw_buf;
                    let ctx = crate::relic_effects::ShuffleCtx {
                        from_reshuffle: true,
                    };
                    let _ = crate::relic_effects::dispatch_after_shuffle_bus(self, &ctx);
                    crate::hooks::dispatch_after_shuffle(
                        self,
                        &crate::hooks::CombatLifecycleCtx::default(),
                    );
                    if self.pending_choice.is_some() {
                        // Suspend: the current card has NOT been drawn.
                        self.pending_draw_resume = Some(PendingDrawResume {
                            remaining,
                            from_hand_draw,
                            resume_turn_setup: false,
                        });
                        return moved;
                    }
                }
            }
            if self.player.draw.is_empty() {
                break;
            }
            // Bead `sts2-rs-09x5n` (F2): defers the random choice to here
            // under `deferred_draw_order` (byte-identical `draw.remove(0)`
            // otherwise) — keeps the Stratagem-suspend draw loop consistent
            // with `cards::draw`/`cards::draw_signaling`.
            let uid = crate::cards::pop_next_card(&mut self.player, &mut self.run_rng.shuffle);
            crate::cards::unpin(&mut self.player, uid);
            let hand_len_before = self.player.hand.len();
            self.player.hand.push(uid);
            // Record the drawn card BEFORE `invoke_card_drawn_for_newly_drawn`
            // runs — a Hellraiser auto-play rider can immediately remove it
            // from the hand, but C#'s `CardPileCmd.Draw` still reports it to
            // the caller (Pillage's attack-draw loop). Bead
            // `sts2-rs-pillage-hellraiser`.
            self.last_drawn_card = Some(uid);
            moved += 1;
            remaining -= 1;
            self.cards_drawn = self.cards_drawn.saturating_add(1);
            if !from_hand_draw {
                // C# `CardDrawnEntry` with `!FromHandDraw`: a card-effect draw.
                // Counted here per-card so the suspend/resume loop (Stratagem /
                // Hellraiser) tallies resumed cards too. Feeds Necrobinder
                // DeathMarch's `CalculatedDamageVar` multiplier.
                self.combat_log.cards_drawn_by_effect_this_turn = self
                    .combat_log
                    .cards_drawn_by_effect_this_turn
                    .saturating_add(1);
            }
            // C# `Slither.AfterCardDrawn` fires on every draw-pile draw
            // regardless of `fromHandDraw` — effect draws through the
            // per-card loop must reroll Slither costs too (the batch paths
            // already dispatch unconditionally; census v8 seeds
            // 1003/1006/1042/1049).
            crate::enchantment_hooks::dispatch_on_card_drawn(self, uid, from_hand_draw);
            self.invoke_card_drawn_for_newly_drawn(hand_len_before, from_hand_draw);
            // Bead `sts2-rs-hellraiser-draw-suspend` (DRAW_ORDER|end_turn seeds
            // 1586/3453/4081 + PILE_COUNT seed 5754): an on-draw auto-play
            // (HellraiserPower auto-playing a drawn Strike) whose OnPlay opens a
            // SURFACING card-select (SeekerStrike's draw-pile pick) parks its
            // post-dispatch tail on `pending_autoplay_resume` — the auto-played
            // card is still on the Play pile and has NOT reached discard. C#'s
            // `CardPileCmd.Draw` per-card loop `await`s the full
            // `Hook.AfterCardDrawnEarly` → `CardCmd.AutoPlay` (including the
            // OnPlay selector AND the card's move to its result pile) BEFORE
            // drawing the next card. We must mirror that: suspend the draw HERE
            // so the next draw/reshuffle does not run while the auto-played card
            // is stranded on Play — otherwise a mid-draw reshuffle sees the
            // discard pile one card short (cs reshuffles N, rs reshuffles N-1),
            // cascading the whole turn's draw order + hand size. The resume in
            // `apply(Action::ResolveChoice)` finishes the parked auto-play (card
            // → discard) BEFORE continuing this draw via `pending_draw_resume`.
            if self.pending_choice.is_some() && self.pending_draw_resume.is_none() {
                self.pending_draw_resume = Some(PendingDrawResume {
                    remaining,
                    from_hand_draw,
                    resume_turn_setup: false,
                });
                return moved;
            }
        }
        moved
    }

    /// Resume a hand/card-effect draw that suspended mid-loop — either a
    /// Stratagem `after_shuffle` grid-select (bead `sts2-rs-63mew`) or an
    /// on-draw Hellraiser auto-play whose OnPlay surfaced a card-select
    /// (bead `sts2-rs-hellraiser-draw-suspend`). Drives the remaining
    /// [`Self::draw_suspending`] and returns `Some(StepResult)` when the
    /// resume owns the return (it re-suspended on a fresh choice, or
    /// completed the turn-setup tail); `None` to fall through to the
    /// remaining resume blocks in [`crate::combat_loop::resolve_choice_action`].
    /// Shared by the `pending_draw_resume` block and the
    /// `pending_autoplay_resume` tail so both produce the identical draw
    /// continuation.
    pub(crate) fn resume_suspended_draw(&mut self, pdr: PendingDrawResume) -> Option<StepResult> {
        let _ = self.draw_suspending(pdr.remaining, pdr.from_hand_draw);
        if let Some(again) = self.pending_draw_resume.as_mut() {
            again.resume_turn_setup |= pdr.resume_turn_setup;
            return Some(StepResult::PendingChoice);
        }
        if pdr.resume_turn_setup {
            // The suspended draw was the turn-start hand draw: run the
            // remaining setup steps (after-energy hooks, side-turn-start, …).
            return Some(crate::combat_loop::finish_player_turn_setup(self));
        }
        if self.pending_snecko_cost_randomize {
            self.pending_snecko_cost_randomize = false;
            self.snecko_randomize_hand_costs();
        }
        None
    }

    /// Reshuffle the discard pile into an empty draw pile, firing the same
    /// C# `CardPileCmd.Shuffle` hook chain as the turn-start hand draw —
    /// `Hook.ModifyShuffleOrder` (enchantments) → relic `after_shuffle` bus →
    /// power-side `Hook.AfterShuffle` (the `StratagemPower.AfterShuffle`
    /// grid-select). Returns `true` if a shuffle happened. The caller inspects
    /// `pending_choice` afterward to detect a Stratagem suspend. Mirrors the
    /// reshuffle block in [`Self::draw_suspending`]; used by the turn-start
    /// Mayhem auto-play so a reshuffle it triggers fires Stratagem (and can
    /// suspend) exactly like C# `CardPileCmd.AutoPlayFromDrawPile`'s
    /// `ShuffleIfNecessary`. Bead `sts2-rs-o0rz3`.
    pub(crate) fn reshuffle_into_draw_with_after_shuffle(&mut self) -> bool {
        if !self.player.draw.is_empty() || self.player.discard.is_empty() {
            return false;
        }
        let shuffled = crate::cards::discard_pile_into_draw_pile_signaling(
            &mut self.player,
            &self.cards,
            &mut self.run_rng.shuffle,
        );
        if shuffled {
            let cards_snapshot: Vec<crate::cards::Card> = self.cards.to_vec();
            let mut draw_buf: smallvec::SmallVec<[crate::CardUid; 32]> = self.player.draw.clone();
            crate::enchantment_hooks::dispatch_modify_shuffle_order(
                &mut self.player,
                &mut draw_buf,
                false,
                &cards_snapshot,
            );
            self.player.draw = draw_buf;
            let ctx = crate::relic_effects::ShuffleCtx {
                from_reshuffle: true,
            };
            let _ = crate::relic_effects::dispatch_after_shuffle_bus(self, &ctx);
            crate::hooks::dispatch_after_shuffle(
                self,
                &crate::hooks::CombatLifecycleCtx::default(),
            );
        }
        shuffled
    }

    /// Walk the suffix of the player's hand newly populated by the most
    /// recent `draw_signaling` and invoke per-card on-draw effects.
    ///
    /// Currently only the `CardId::Void` energy-drain branch from Python's
    /// `_invoke_card_drawn` is implemented; the broader `on_card_drawn`
    /// power-bus dispatch (`hooks::dispatch_on_card_drawn`) remains a
    /// no-op until bead `sts2-rs-h9u-8` lands the per-card uid context.
    /// Bead `sts2-rs-6ynt.3.2`.
    ///
    /// Per-card on-draw riders also handled here (bead `sts2-rs-8z4u4`):
    /// Regent `KinglyPunch` permanently gains
    /// [`Self::KINGLY_PUNCH_DRAW_INCREASE`] base_damage per draw, and
    /// `KinglyKick` loses 1 energy cost per draw (clamped at 0).
    pub(crate) fn invoke_card_drawn_for_newly_drawn(
        &mut self,
        hand_len_before: usize,
        from_hand_draw: bool,
    ) {
        let hand_len_after = self.player.hand.len();
        if hand_len_after <= hand_len_before {
            return;
        }
        let drawn: smallvec::SmallVec<[CardUid; 8]> = self.player.hand
            [hand_len_before..hand_len_after]
            .iter()
            .copied()
            .collect();

        // Event log (C# `CardDrawnEntry`), one row per card in draw order.
        // This is the single funnel for cards landing in hand, and it already
        // carries C#'s `FromHandDraw` discriminator.
        if crate::event_log::armed(self) {
            for &uid in &drawn {
                let Some(card) = self.cards.get(uid.0.get() as usize - 1) else {
                    continue;
                };
                let (card_id, upgraded) = (card.card_id, card.upgraded);
                crate::event_log::record(
                    self,
                    crate::event_log::EventLogEntry::CardDrawn {
                        card: card_id,
                        upgraded,
                        from_hand_draw,
                    },
                );
            }
        }

        // Silent `SpeedsterPower.AfterCardDrawn` (C# `SpeedsterPower.cs:18-27`):
        // for each card drawn NOT via the turn-start hand draw
        // (`fromHandDraw == false` — the C# default for every `CardPileCmd.Draw`
        // except `CombatManager`'s hand draw), deal `Amount` Unpowered damage to
        // every hittable enemy. The C# owner/side gate is always satisfied here
        // (this walks the player's own hand during the player's turn). Read the
        // stack once off the mirror+legacy union; one hit per drawn card is
        // dealt after the per-card rider loop (Speedster consumes no RNG and
        // deals to ALL enemies, so the deferral is observationally identical to
        // C#'s per-card `AfterCardDrawn` firing).
        let speedster_amount = if from_hand_draw {
            0
        } else {
            self.player_power_amount(PowerKind::Speedster).max(0)
        };
        let speedster_hits = if speedster_amount > 0 {
            i32::try_from(drawn.len()).unwrap_or(i32::MAX)
        } else {
            0
        };

        // Bead `sts2-rs-bsj93`: HellraiserPower.AfterCardDrawnEarly (C#
        // `HellraiserPower.cs:34-42`). Must walk the mirror+legacy power
        // UNION via `player_power_amount` — the card-driven
        // `apply_power_to_player` routes through
        // `apply_power_to_creature` when `player.creature_uid` is set,
        // so the power lands on `state.creatures[uid].powers`, NOT on
        // `state.player.powers`. A raw `self.player.powers` scan silently
        // misses the Hellraiser instance in real combat (same class of
        // bug fixed for Vigor in `feedback_player_consume_must_walk_mirror_union`).
        let has_hellraiser = self.player_power_amount(PowerKind::Hellraiser) > 0;
        // C# `HellraiserPower.AfterCardDrawnEarly` (`HellraiserPower.cs:34-42`)
        // only auto-plays a drawn Strike when
        // `!base.Owner.CombatState.HittableEnemies.All(c => c.ShowsInfiniteHp)`
        // — i.e. at least one alive, hittable, non-infinite-HP enemy remains.
        // Once a mid-draw auto-play (or any earlier kill) clears the board,
        // `HittableEnemies` is empty, `All(..)` is vacuously true, and the
        // negation is false ⇒ the drawn Strike is NOT auto-played; it stays
        // in hand. Without this guard the auto-play fires into an empty board
        // and Pillage's attack-draw loop (`Pillage.cs:28-33`) redraws and
        // re-auto-plays the same reshuffled Strike forever (a hang). Rust does
        // not model `ShowsInfiniteHp` (only WaterfallGiant/Doormaker boss
        // phases set it; never reachable mid-Strike-spam), so the faithful
        // predicate here is "an alive enemy remains" — matching the alive
        // filter in [`Self::get_random_enemy`], the same set the auto-play's
        // random target is rolled from. Evaluated once: under Hellraiser the
        // draw routes through the per-card [`Self::draw_suspending`] loop,
        // which calls this fn once per single drawn card, so this reflects
        // every prior auto-play's kills.
        let has_alive_enemy = has_hellraiser
            && self.enemy_uids.iter().any(|uid| {
                self.creatures
                    .iter()
                    .any(|c| c.id == uid.0 && c.current_hp > 0)
            });

        // Bead `sts2-rs-lj36b`: ConfusedPower.AfterCardDrawn (Snecko Eye /
        // FAKE_SNECKO_EYE). C# `ConfusedPower.cs::AfterCardDrawn` re-rolls
        // every player card's energy cost as it is drawn, *unless* its
        // canonical cost is negative (X-cost or unplayable Status/Curse,
        // `card.EnergyCost.Canonical < 0`). The roll is
        // `RunState.Rng.CombatEnergyCosts.NextInt(4)` ([0,3]) and is set as
        // an absolute per-combat cost (`SetThisCombat`). Read the
        // mirror+legacy power union via `player_power_amount` so a
        // mirror-applied Confused (FAKE_SNECKO_EYE wires it through
        // `apply_power_to_creature`) is seen. Confused is the only on-draw
        // RNG consumer in this loop, so its position relative to the other
        // per-card riders does not perturb the `combat_energy_costs` cursor.
        let confused = self.player_power_amount(PowerKind::Confused) > 0;

        // Bead `sts2-rs-4wppb`: PagestormPower.AfterCardDrawn (Necrobinder).
        // C# `PagestormPower.cs`: when the owning player draws a card that has
        // the `Ethereal` keyword, draw `Amount` additional cards. Read the
        // player's stack count once (mirror+legacy union via
        // `player_power_amount`); the per-card accumulation + the actual draw
        // (which recurses through `draw_into_hand` → here, so a cascade of
        // Ethereal-into-Ethereal draws fires faithfully) live below.
        let pagestorm_amount = self.player_power_amount(PowerKind::Pagestorm).max(0);

        // Defect `IterationPower.AfterCardDrawn` (C# `IterationPower.cs`): the
        // FIRST `CardType::Status` card the owner draws each turn draws
        // `Amount` extra cards. Read the stack once (mirror+legacy union); the
        // per-drawn-card "first status this turn" gate + the deferred draw
        // (which recurses through `draw_via_state` → here) live below.
        // Iteration is a Defect power and Pagestorm/Speedster/CorrosiveWave are
        // Necrobinder/Silent, so the deferred draw can never reorder against
        // them in a single-character combat.
        let iteration_amount = self.player_power_amount(PowerKind::Iteration).max(0);

        // Silent `CorrosiveWavePower.AfterCardDrawn` (C#
        // `CorrosiveWavePower.cs:25-42`): for each card the owning player
        // draws, apply `Amount` Poison to every hittable enemy. The C# gate
        // `card.Owner.Creature == base.Owner` is always true here (this walks
        // the player's own hand). Read the stack once (mirror+legacy union);
        // accumulate per-drawn-card below and apply after the loop (deferred
        // past the `self.cards` borrow). Poison applies no RNG, so the
        // post-loop placement is order-neutral.
        let corrosive_wave_amount = self.player_power_amount(PowerKind::CorrosiveWave).max(0);

        // Bead `sts2-rs-kese5` (f933s sub-1) — hoisted out of the per-card
        // loop (perf 2026-07-04): the ChainsOfBinding stack count is
        // loop-invariant (the loop body mutates card fields, player energy,
        // combat-log counters, and Automation `counter`s — never a power's
        // kind/amount, and it neither adds nor removes powers), so one scan
        // replaces the per-drawn-card `player.powers` walk. Gated on the
        // `present_kinds` superset first: no live ChainsOfBinding anywhere ⇒
        // the walk would find nothing ⇒ 0, bit-identically.
        let chains_stacks: u32 = if self
            .powers
            .present_kinds()
            .contains(crate::powers::PowerKind::ChainsOfBinding)
        {
            let mut amt: i32 = 0;
            for puid in &self.player.powers {
                if let Some(p) = self.powers.get_by_id(puid.0) {
                    if p.kind == crate::powers::PowerKind::ChainsOfBinding {
                        amt = p.amount;
                        break;
                    }
                }
            }
            u32::try_from(amt.max(0)).unwrap_or(0)
        } else {
            0
        };

        // Perf 2026-07-04: `present_kinds` is a conservative superset of every
        // live power kind, so a miss proves the per-drawn-card Automation walk
        // over the whole power arena below could never match — skip it.
        let automation_present = self.powers.present_kinds().contains(PowerKind::Automation);

        let mut auto_play_uids: smallvec::SmallVec<[CardUid; 4]> = smallvec::SmallVec::new();
        // Bead `sts2-rs-q6bix` (F-S): accumulated AutomationPower energy
        // gain, applied once after the per-card loop (deferred past the
        // `self.powers` borrow).
        let mut automation_energy: i32 = 0;
        // C# `AutomationPower.cs:45` flashes once per instance that resets
        // (`cardsLeft <= 0`) — same accumulate-then-apply deferral as
        // `automation_energy` (the power is always player-owned, so a count
        // suffices; no per-owner tracking needed).
        let mut automation_flashes: i32 = 0;
        // Bead `sts2-rs-4wppb`: accumulated Pagestorm extra-draw count, drawn
        // once after the per-card loop (deferred past the `self.cards` borrow;
        // the draw recurses into this fn so the cascade is handled there).
        let mut pagestorm_draws: i32 = 0;
        // C# `PagestormPower.cs:23` flashes once per qualifying `AfterCardDrawn`
        // call (one per Ethereal card drawn), same accumulate-then-apply
        // deferral as `pagestorm_draws` — applied after the loop, ahead of the
        // deferred draw.
        let mut pagestorm_flashes: i32 = 0;
        // CorrosiveWave Poison-on-draw: count the number of drawn cards that
        // trigger `CorrosiveWavePower.AfterCardDrawn`. C# applies a SEPARATE
        // `PowerCmd.Apply<PoisonPower>(base.Amount)` per drawn card, and each
        // such application re-enters the `ModifyPowerAmountGivenAdditive`
        // chain — so a flat per-application Poison additive (e.g. the Silent
        // relic SneckoSkull, +1 per applied Poison) fires ONCE PER CARD, not
        // once for an accumulated sum. Accumulating into a single
        // `apply_power_to_enemy(Poison, N*Amount)` would apply that additive
        // only once, under-counting by `(cards-1) * additive` (seed 1131:
        // Adrenaline draws 2 cards under SneckoSkull → C# 6, accumulate-once 5).
        // Apply per drawn card after the loop instead (deferred past the
        // `self.cards` borrow; Poison applies no RNG, so the count is exact).
        let mut corrosive_wave_cards: i32 = 0;
        // Accumulated Defect Iteration extra-draw count, drawn once after the
        // per-card loop (deferred past the `self.cards` borrow; the draw
        // recurses into this fn so a status-into-status cascade is bounded and
        // does not re-fire — the per-turn counter is already past 1).
        let mut iteration_draws: i32 = 0;

        for uid in drawn {
            let idx = (uid.get() as usize).saturating_sub(1);
            let Some(card) = self.cards.get(idx) else {
                continue;
            };
            let card_id = card.card_id;
            // Captured under the same immutable `card` borrow; consumed by the
            // Iteration first-status-of-turn gate below.
            let card_type = card.card_type;
            // Short-circuit order (perf 2026-07-04): the gate flag is checked
            // BEFORE the per-card predicate so the predicate (a pure read) is
            // only evaluated when its consumer below could act on it.
            let is_strike = has_alive_enemy && card.has_tag(crate::cards::CardTag::Strike);
            // Captured under the same immutable `card` borrow as `card_id`;
            // consumed by the Pagestorm accumulation below (bead `sts2-rs-4wppb`).
            let is_ethereal = pagestorm_amount > 0 && crate::card_effects::card_ethereal_inst(card);
            // Canonical (base) cost captured before any per-card cost rider
            // mutates `self.cards[idx].cost`. For the Confused skip check:
            // C# `ConfusedPower.AfterCardDrawn` skips only `Canonical < 0`
            // (Status/Curse with canonicalCost=-1). X-cost cards have
            // `Canonical = 0` in C# (`CardEnergyCost` ctor: `CostsX ? 0 : canonicalCost`),
            // so they ARE rolled — consuming one RNG read — even though the modifier
            // is never applied (C# `GetWithModifiers` short-circuits on `CostsX`).
            // In Rust, X-cost cards use cost=-1 with `is_x_cost()=true`; we must
            // consume the read without overwriting the sentinel. Bead `sts2-rs-ktqmg`.
            let canonical_cost = card.cost;
            let is_x_cost_card = card.is_x_cost();
            if card_id == crate::cards::CardId::Void {
                let new_energy = i32::from(self.player.energy).saturating_sub(1).max(0);
                self.player.energy = i8::try_from(new_energy).unwrap_or(0);
            }
            // Regent KinglyPunch (`KinglyPunch.cs::AfterCardDrawn`, bead
            // `sts2-rs-8z4u4`): every time this card is drawn into hand it
            // permanently bumps its own `base_damage` by `Increase` (4 base,
            // 6 upgraded — C# `KinglyPunch.cs` OnUpgrade
            // `Increase.UpgradeValueBy(2m)`). Bead `sts2-rs-om4tt` restores
            // the upgrade-scaled rider; previously hardcoded to 4.
            // C# gates on `card == this`, which here is the per-uid match —
            // we mutate exactly the drawn card instance.
            if card_id == crate::cards::CardId::KinglyPunch {
                if let Some(c) = self.cards.get_mut(idx) {
                    let inc = if c.upgraded {
                        Self::KINGLY_PUNCH_DRAW_INCREASE_UPGRADED
                    } else {
                        Self::KINGLY_PUNCH_DRAW_INCREASE
                    };
                    c.base_damage = c.base_damage.saturating_add(inc);
                }
            }
            // Bead `sts2-rs-lj36b` + `sts2-rs-ktqmg`: ConfusedPower per-draw
            // cost re-roll. C# `ConfusedPower.AfterCardDrawn` skips ONLY cards
            // where `card.EnergyCost.Canonical < 0` — i.e. Status/Curse
            // (`canonicalCost = -1`). X-cost cards have `Canonical = 0` (not
            // negative), so C# DOES consume one `CombatEnergyCosts.NextInt(4)`
            // read for them (even though the resulting modifier is ignored by
            // `GetWithModifiers` which short-circuits on `CostsX`).
            // In Rust: skip only unplayable cards (`canonical_cost < 0 AND NOT
            // is_x_cost`); for X-cost cards consume the read but leave
            // `c.cost = -1` intact (overwriting the sentinel would break
            // `is_x_cost()` and drain player energy on play).
            // C# ref: `ConfusedPower.cs::AfterCardDrawn` (v0.103.2).
            if confused && (canonical_cost >= 0 || is_x_cost_card) {
                let rolled = self.run_rng.combat_energy_costs.next_int(0, 3) as i8;
                if let Some(c) = self.cards.get_mut(idx) {
                    // Only apply the rolled cost for non-X-cost cards.
                    // X-cost cards just consume the RNG read (the modifier
                    // would be ignored by C#'s GetWithModifiers anyway).
                    if !is_x_cost_card {
                        c.cost = rolled;
                        // Bead `sts2-rs-xk9yo`: Confused's `AfterCardDrawn`
                        // sets an ABSOLUTE this-turn cost — it supersedes any
                        // additive this-turn modifier already banked on this
                        // card (Stomp's `BeforeCardPlayed` -1 shave). Because
                        // this is a flat `c.cost` overwrite (not a modifier
                        // stack like C#'s `EnergyCost`), the banked Stomp
                        // reduction would otherwise be re-added by
                        // `restore_stomp_costs_for_turn_start` at the NEXT
                        // turn start, leaving the card wrongly +K over its
                        // base (e.g. a Stomp shaved on turn 0 by a
                        // DistilledChaos auto-played Attack, then drawn under
                        // Confused, resurfaced at cost 3+K on turn 1 and was
                        // wrongly excluded from `legal_actions` — seed 1754).
                        // Clear the bank so the overwrite is authoritative.
                        c.set_combat_var(crate::cards::VarKey::StompCostReductionThisTurn, 0);
                        // C# `SetThisCombat(roll)` is an ABSOLUTE local cost
                        // modifier — flag it so an in-combat upgrade clamps
                        // it per `CardEnergyCost.UpgradeBy` (sweep seeds
                        // 1170/4720). Bead `sts2-rs-i77h0`.
                        c.set_combat_var(crate::cards::VarKey::CostAbsolutelySet, 1);
                    }
                }
            }
            // Regent KinglyKick (`KinglyKick.cs::AfterCardDrawn`, bead
            // `sts2-rs-8z4u4`): every draw reduces this card's energy cost
            // by 1 for the rest of combat (`EnergyCost.AddThisCombat(-1)`),
            // clamped at 0. MUST run AFTER the Confused re-roll above: C#
            // `CombatState.IterateHookListeners` fires powers (Confused) BEFORE
            // pile cards (KinglyKick), and `CardEnergyCost` applies its local
            // modifiers chronologically. Confused's `SetThisCombat(roll)` is an
            // absolute set; KinglyKick's `AddThisCombat(-1)` then subtracts from
            // it, so a card drawn while Confused lands at `roll - 1`, not `roll`.
            // Running KinglyKick first (the old order) let Confused's absolute
            // overwrite clobber the -1 (regent-hand-cost part B, seeds
            // 4113/4597/5402). When not Confused the block above is skipped, so
            // ordering is behavior-neutral.
            if card_id == crate::cards::CardId::KinglyKick {
                if let Some(c) = self.cards.get_mut(idx) {
                    c.cost = c.cost.saturating_sub(1).max(0);
                }
            }
            // Bead `sts2-rs-kese5` (f933s sub-1): ChainsOfBindingPower
            // AfterCardDrawn. C# `ChainsOfBindingPower.cs` (v0.103.2)
            // afflicts each newly-drawn player card with `Bound` up to
            // the power's stack-count per turn; further draws this
            // turn after the cap are not afflicted. The per-turn
            // cap-check budget lives on
            // [`TurnEvents::bound_cards_afflicted_by_chains_this_turn`]
            // (bead `sts2-rs-hmxmc`) and clears at turn start.
            //
            // The player's ChainsOfBinding stack count (`chains_stacks`) is
            // read ONCE above the loop — it is loop-invariant (see the hoist
            // comment). If the card already carries an affliction we skip —
            // the flat single-slot affliction model matches C#
            // `CardModel.Affliction` semantics, and `Bound`'s `CanAfflict`
            // returns false on already-afflicted cards.
            if chains_stacks > 0
                && self.combat_log.bound_cards_afflicted_by_chains_this_turn < chains_stacks
            {
                if let Some(c) = self.cards.get_mut(idx) {
                    if c.affliction.is_none()
                        && crate::card_afflictions::CardAffliction::can_afflict_card_type(
                            crate::card_afflictions::CardAfflictionKind::Bound,
                            c.card_type,
                        )
                        && c.set_affliction(crate::card_afflictions::CardAffliction::new(
                            crate::card_afflictions::CardAfflictionKind::Bound,
                            1,
                        ))
                    {
                        self.combat_log.bound_cards_afflicted_by_chains_this_turn = self
                            .combat_log
                            .bound_cards_afflicted_by_chains_this_turn
                            .saturating_add(1);
                    }
                }
            }
            // Bead `sts2-rs-q6bix` (F-S): AutomationPower.AfterCardDrawn.
            // C# `AutomationPower` keeps an internal `cardsLeft` counter
            // starting at 10; every player card drawn decrements it, and at
            // 0 it fires `PlayerCmd.GainEnergy(Amount)` and resets to 10. We
            // model the count-up on the power's per-instance `counter` (0 →
            // 10): bump per drawn card, and on reaching 10 gain `amount`
            // energy and reset. Read the mirror+legacy union so a
            // card-applied Automation (mirror-only) is seen. The energy gain
            // is deferred to a (uid, amount) list applied after the borrow
            // of `self.powers` ends. With `PowerKind::Automation` instanced,
            // each separate Automation power increments its counter
            // independently (C# AutomationPower.IsInstanced=true).
            // Skipped entirely when no Automation is live (`present_kinds`
            // superset gate hoisted above — the walk could never match).
            if automation_present {
                for p in &mut self.powers {
                    if p.kind == PowerKind::Automation {
                        let new_counter = p.counter.saturating_add(1);
                        if new_counter >= 10 {
                            p.counter = 0;
                            automation_energy = automation_energy.saturating_add(p.amount);
                            automation_flashes = automation_flashes.saturating_add(1);
                        } else {
                            p.counter = new_counter;
                        }
                    }
                }
            }
            // Bead `sts2-rs-4wppb`: PagestormPower.AfterCardDrawn. C# gates on
            // `card.Owner.Creature == base.Owner` (the card is mine) — always
            // true here since this walks the player's own hand — and
            // `card.Keywords.Contains(Ethereal)`. Defer the draw so the
            // `self.cards` borrow is released first; the count is exact
            // regardless of interleaving because each Ethereal card
            // contributes exactly `Amount` draws.
            if pagestorm_amount > 0 && is_ethereal {
                pagestorm_draws = pagestorm_draws.saturating_add(pagestorm_amount);
                pagestorm_flashes = pagestorm_flashes.saturating_add(1);
            }
            if corrosive_wave_amount > 0 {
                corrosive_wave_cards = corrosive_wave_cards.saturating_add(1);
            }
            // Defect IterationPower.AfterCardDrawn (C# `IterationPower.cs`):
            // count this drawn Status card; only the FIRST status card drawn
            // this turn (C# `num <= 1`, the count INCLUDING the current draw)
            // fires the +`Amount` draw. The counter persists across draw
            // batches (turn-start hand draw + effect draws) and clears at
            // turn start, matching C#'s per-turn `CardDrawnEntry` history scan.
            if card_type == crate::cards::CardType::Status {
                self.combat_log.status_cards_drawn_this_turn = self
                    .combat_log
                    .status_cards_drawn_this_turn
                    .saturating_add(1);
                if iteration_amount > 0 && self.combat_log.status_cards_drawn_this_turn == 1 {
                    iteration_draws = iteration_draws.saturating_add(iteration_amount);
                }
            }
            if has_alive_enemy && is_strike {
                auto_play_uids.push(uid);
            }
        }
        for _ in 0..automation_flashes {
            crate::event_log::flash_player_power(self, PowerKind::Automation);
        }
        if automation_energy > 0 {
            self.gain_energy(automation_energy);
        }
        // CorrosiveWave Poison-on-draw (deferred past the per-card `self.cards`
        // borrow): C# `CorrosiveWavePower.AfterCardDrawn` applies a separate
        // `PowerCmd.Apply<PoisonPower>(base.Amount)` per drawn card. Replay one
        // apply per drawn card so each application independently re-enters the
        // `modify_power_amount_given` chain — matching SneckoSkull's per-apply
        // +1 additive (seed 1131). Recompute the alive set each card so a kill
        // mid-batch is reflected, mirroring C#'s per-card `HittableEnemies`.
        for _ in 0..corrosive_wave_cards {
            // C# `CorrosiveWavePower.cs:32` calls `Flash()` inside
            // `AfterCardDrawn`'s `card.Owner.Creature == Owner` guard — once per
            // DRAWN CARD, before that card's Poison application, and regardless
            // of whether any enemy is hittable. The per-card replay below is the
            // same batch C# fires one hook per, so one flash per iteration.
            crate::event_log::flash_player_power(self, PowerKind::CorrosiveWave);
            for enemy_uid in self.enemy_uids.clone() {
                let alive = self
                    .creatures
                    .iter()
                    .any(|c| c.id == enemy_uid.0 && c.current_hp > 0);
                if alive {
                    crate::card_effects::apply_power_to_enemy(
                        self,
                        enemy_uid,
                        PowerKind::Poison,
                        corrosive_wave_amount,
                    );
                }
            }
        }
        // Silent SpeedsterPower damage-on-draw (deferred past the per-card
        // `self.cards` borrow): one hit of `speedster_amount` to all living
        // enemies per drawn card. Recomputes the target set each hit so a kill
        // mid-batch matches C#'s per-card `HittableEnemies` recompute.
        for _ in 0..speedster_hits {
            self.deal_unpowered_damage_to_all_enemies(speedster_amount);
        }
        // Pagestorm extra draws (bead `sts2-rs-4wppb`). Route through the
        // card-effect draw chokepoint `draw_via_state` — NOT `draw_into_hand`:
        // C# `PagestormPower.AfterCardDrawn` calls `CardPileCmd.Draw(.., Amount,
        // player)` with the default `fromHandDraw = false`, so each Pagestorm
        // draw is a non-hand-draw and is counted by Necrobinder DeathMarch's
        // `CalculatedDamageVar` multiplier (C# `CardDrawnEntry` with
        // `!FromHandDraw`). `draw_into_hand` is the turn-start hand-draw path
        // (`from_hand_draw = true`) and silently bypassed
        // `cards_drawn_by_effect_this_turn`, undercounting DeathMarch's damage
        // when Pagestorm fired (sweep seeds 53/412). `draw_via_state` recurses
        // into this fn for the newly-drawn suffix, so a chain of Ethereal draws
        // still cascades correctly (bounded by draw+discard size).
        if pagestorm_flashes > 0 {
            for _ in 0..pagestorm_flashes {
                crate::event_log::flash_player_power(self, PowerKind::Pagestorm);
            }
        }
        if pagestorm_draws > 0 {
            crate::cards::draw_via_state(self, pagestorm_draws);
        }
        // Defect Iteration extra draws (deferred past the per-card `self.cards`
        // borrow). Routed through `draw_via_state` (the card-effect draw
        // chokepoint) to match C# `IterationPower.AfterCardDrawn`'s
        // `CardPileCmd.Draw(.., Amount, player)` (default `fromHandDraw=false`).
        // The recursion re-enters this fn for the newly-drawn suffix; the
        // status counter is already >= 1, so an extra-drawn status card does
        // not re-trigger Iteration (matches C#'s `num <= 1` gate).
        if iteration_draws > 0 {
            // C# `IterationPower.cs:25` calls `Flash()` inside
            // `card.Owner.Creature == Owner && card.Type == Status` AND
            // `num <= 1` (status cards drawn this turn, counting this one),
            // immediately before `CardPileCmd.Draw(Amount)`. `iteration_draws`
            // is set only under exactly that gate, once per turn, so this is
            // the same clause — deferred past the per-card `self.cards` borrow
            // the same way the CorrosiveWave flash above is.
            crate::event_log::flash_player_power(self, PowerKind::Iteration);
            crate::cards::draw_via_state(self, iteration_draws);
        }

        // Bead `sts2-rs-bsj93`: Hellraiser auto-plays Strike cards as drawn.
        // C# `CardCmd.AutoPlay(choiceContext, card, null)` (`CardCmd.cs:33-109`)
        // is a FREE play (`ResourceInfo.EnergySpent = 0`, :104) that — for an
        // `AnyEnemy`/`Enemy`-targeted card with a null target — rolls
        //   `card.Owner.RunState.Rng.CombatTargets.NextItem(combatState.HittableEnemies)`
        // (:56-66) at auto-play time, *before* the play resolves. The roll
        // consumes a `CombatTargets` read even when only one enemy is hittable
        // (C# `NextInt` on a 1-element range still advances the counter).
        //
        // Route through the shared FREE auto-play dispatcher
        // [`crate::combat_loop::resolve_auto_played_card`] (the Stampede
        // hand-side pattern, bead `sts2-rs-lmfsm`), which resolves an Enemy
        // target via [`Self::get_random_enemy`] off the `combat_targets`
        // stream — NOT through `play_card`: the latter (a) no-ops outside
        // `PlayerPlayPhase`, silently skipping every TURN-START draw (the
        // hand draw runs in `PlayerTurnStart`, before the step-11 phase flip
        // — the HELLRAISER+1 quick-sweep divergence, seed 40), and (b)
        // spends energy, while C# AutoPlay is free.
        for uid in auto_play_uids {
            if let Some(pos) = self.player.hand.iter().position(|u| *u == uid) {
                self.player.hand.remove(pos);
                self.player.play.push(uid);
                crate::combat_loop::resolve_auto_played_card(
                    self, uid, /*force_exhaust=*/ false,
                );
            }
        }
    }

    /// Shuffle the player's draw pile in place and fire the
    /// [`AfterShuffleFn`](crate::relic_effects::AfterShuffleFn) relic-bus
    /// channel exactly once.
    ///
    /// Intended for cards / relics that explicitly "shuffle the draw
    /// pile" (as opposed to the auto-reshuffle inside [`crate::cards::draw`],
    /// which routes through [`Self::draw_into_hand`]). The
    /// `from_reshuffle` flag on the dispatched [`ShuffleCtx`](crate::relic_effects::ShuffleCtx)
    /// is `false` for this entry point.
    ///
    /// Faithful to C# `CardPileCmd.Shuffle` (`CardPileCmd.cs:804`): this
    /// **merges the discard pile into the draw pile** and `StableShuffle`s
    /// the combined list (pre-sort by `(Entry, upgrade-level)`), leaving
    /// discard empty — not a draw-pile-only shuffle. The two callers
    /// (Reboot, BottledPotential) move the hand into draw first, so the
    /// net effect is hand+draw+discard shuffled into draw. Bead
    /// `sts2-rs-s2pr8` (pre-sort + discard-merge); `sts2-rs-jlc` (n3w-b)
    /// for the `after_shuffle` surface.
    pub fn shuffle_draw_pile(&mut self) {
        crate::cards::shuffle_draw_and_discard(
            &mut self.player,
            &self.cards,
            &mut self.run_rng.shuffle,
        );
        let ctx = crate::relic_effects::ShuffleCtx {
            from_reshuffle: false,
        };
        let _ = crate::relic_effects::dispatch_after_shuffle_bus(self, &ctx);
        // Bead `sts2-rs-6ynt.11`: enchantment `modify_shuffle_order`
        // channel. Explicit shuffle path (not an auto-reshuffle) —
        // still `is_initial_shuffle = false` per Python semantics
        // (only the combat-start shuffle skips the enchantment hook).
        let cards_snapshot: Vec<crate::cards::Card> = self.cards.to_vec();
        let mut draw_buf: smallvec::SmallVec<[crate::CardUid; 32]> = self.player.draw.clone();
        crate::enchantment_hooks::dispatch_modify_shuffle_order(
            &mut self.player,
            &mut draw_buf,
            false,
            &cards_snapshot,
        );
        self.player.draw = draw_buf;
        // Bead `sts2-rs-63mew`: power-side `after_shuffle` walk — C#'s
        // `CardPileCmd.Shuffle` ends with `Hook.AfterShuffle`
        // (CardPileCmd.cs:857) for explicit shuffles (Reboot,
        // BottledPotential) too, so a Stratagem stack grid-selects here
        // as well. No draw is in progress at this surface, so the
        // opened choice needs no suspension plumbing.
        crate::hooks::dispatch_after_shuffle(self, &crate::hooks::CombatLifecycleCtx::default());
    }

    /// Pop up to `count` cards from the top of the player's draw pile
    /// and route each one into the exhaust pile, dual-firing
    /// `after_card_exhausted` on both the hooks bus (powers — e.g.
    /// `FeelNoPainPower`, `DarkEmbracePower`) and the relic-effects bus
    /// (relics — e.g. `CharonsAshes`) for every card exhausted.
    ///
    /// If the draw pile is empty at the top of an iteration and the
    /// discard pile is non-empty, the discard pile is reshuffled back
    /// into draw via [`crate::cards::discard_pile_into_draw_pile_signaling`]
    /// and the `after_shuffle` relic-bus is fired exactly once per
    /// reshuffle (mirroring `CombatState::draw_into_hand`). If both
    /// piles are empty the loop exits early — no spurious dispatch.
    ///
    /// Mirrors `sts2-rl-agent@01c094d:sts2_env/core/combat.py::
    /// CombatState.exhaust_from_draw_pile` (lines 1274-1284):
    ///
    /// ```python
    /// def exhaust_from_draw_pile(self, owner: Creature, count: int) -> None:
    ///     from sts2_env.core.hooks import fire_after_card_exhausted
    ///
    ///     zones = self._zones_for_creature(owner)
    ///     for _ in range(max(0, count)):
    ///         self._shuffle_if_needed(owner)
    ///         if not zones["draw"]:
    ///             break
    ///         card = zones["draw"].pop(0)
    ///         zones["exhaust"].append(card)
    ///         fire_after_card_exhausted(card, self)
    /// ```
    ///
    /// Python's `fire_after_card_exhausted` walks power-listeners
    /// BEFORE relic-listeners (`hooks.py:351-355`); the Rust dual-fire
    /// preserves that order (hooks bus, then relic-effects bus).
    ///
    /// `count <= 0` is a no-op.
    ///
    /// Bead `sts2-rs-6ynt.6.8.1`. Required by `DrumOfBattle` (bead
    /// `6ynt.6.8`) and likely several other Phase-6 cards/powers that
    /// surgically exhaust off the top of draw.
    /// Record that `thief` stole the master-deck card identified by
    /// `(card_id, upgraded)` from the run deck (C# `SwipePower.Steal` →
    /// `RemoveFromDeck(card.DeckVersion)`). `exit_combat` removes the
    /// un-reclaimed entries from `master_deck`. Bead `sts2-rs-RP13`.
    pub fn record_swipe_steal(
        &mut self,
        thief: CreatureUid,
        card_id: crate::cards::CardId,
        upgraded: bool,
    ) {
        self.swipe_stolen.push(SwipeStolen {
            thief,
            card_id,
            upgraded,
            reclaimed: false,
        });
    }

    /// Mark every card stolen by `dead` as reclaimed (C#
    /// `SwipePower.BeforeDeath` restores the card to the deck when the thief
    /// dies). Reclaimed entries are NOT removed from the run deck at
    /// `exit_combat`. Called from the damage-death chokepoint, so it fires
    /// on real deaths but NOT on escapes. Bead `sts2-rs-RP13`.
    pub fn reclaim_swipe_on_death(&mut self, dead: CreatureUid) {
        if self.swipe_stolen.is_empty() {
            return;
        }
        for s in &mut self.swipe_stolen {
            if s.thief == dead {
                s.reclaimed = true;
            }
        }
    }

    /// Bank every `HeistPower` stack `dead` was carrying into
    /// [`Self::heist_recovered_gold`] (C# `HeistPower.BeforeDeath` →
    /// `AddExtraReward(GoldReward(Amount, wasGoldStolenBack: true))`).
    /// Called from the damage-death chokepoint alongside
    /// [`Self::reclaim_swipe_on_death`], so — exactly like the stolen-card
    /// ledger — it fires on real deaths but NOT on `Intent::Escape`: a
    /// FatGremlin that flees keeps the gold it is holding.
    ///
    /// C# reads `HeistPower.Amount` (the power's stack value, which
    /// `SurprisePower.AfterDeath` seeded from the merc's
    /// `ThieveryPower.DynamicVars.Gold` running total), so the Rust mirror
    /// sums `Power::amount` over the corpse's Heist instances. GH #604.
    pub fn recover_heist_on_death(&mut self, dead: CreatureUid) {
        let Some(c) = self.creatures.iter().find(|c| c.id == dead.0) else {
            return;
        };
        let uids: SmallVec<[PowerUid; 8]> = c.powers.clone();
        let mut recovered = 0_i32;
        for puid in uids {
            if let Some(p) = self
                .powers
                .get_by_id(puid.0)
                .filter(|p| p.kind == PowerKind::Heist)
            {
                recovered = recovered.saturating_add(p.amount.max(0));
            }
        }
        if recovered > 0 {
            self.heist_recovered_gold = self.heist_recovered_gold.saturating_add(recovered);
        }
    }

    pub fn exhaust_from_draw_pile(&mut self, count: i32) {
        let iters = usize::try_from(count).unwrap_or(0);
        for _ in 0..iters {
            // An already-ending combat neither reshuffles nor draws Shuffle (in
            // addition to the admission check at the move,
            // `exhaust_one_from_draw_at`).
            if !crate::combat_loop::exhaust_may_begin(self) {
                break;
            }
            // Reshuffle if draw is empty and discard is non-empty.
            // Mirrors Python's `_shuffle_if_needed` at the top of each
            // iteration (so a single call can drain across the boundary).
            //
            // Power-side `after_shuffle` walk (Stratagem grid-select): C#
            // awaits the choice before the next exhaust; this non-suspending
            // path instead finishes the exhausts with the choice held open
            // (the resolver defensively skips uids no longer in the draw
            // pile). Accepted approximation for the mid-combat callers (e.g.
            // PaelsEye) where no resume machinery exists.
            self.exhaust_from_draw_reshuffle_step();
            if self.player.draw.is_empty() {
                break;
            }
            self.exhaust_one_from_draw_top();
        }
    }

    /// Reshuffle the discard pile into an empty draw pile (if draw is empty and
    /// discard is non-empty) and fire the `after_shuffle` relic bus + power-side
    /// walk (the `StratagemPower.AfterShuffle` grid-select). Returns `true` iff a
    /// reshuffle actually happened. Used by [`Self::exhaust_from_draw_pile`].
    fn exhaust_from_draw_reshuffle_step(&mut self) -> bool {
        if self.player.draw.is_empty()
            && crate::cards::discard_pile_into_draw_pile_signaling(
                &mut self.player,
                &self.cards,
                &mut self.run_rng.shuffle,
            )
        {
            let ctx = crate::relic_effects::ShuffleCtx {
                from_reshuffle: true,
            };
            let _ = crate::relic_effects::dispatch_after_shuffle_bus(self, &ctx);
            crate::hooks::dispatch_after_shuffle(
                self,
                &crate::hooks::CombatLifecycleCtx::default(),
            );
            true
        } else {
            false
        }
    }

    /// Exhaust one card from the top of the draw pile, optionally skipping
    /// Innate cards. Mirrors C# `ToastyMittens.BeforeHandDraw`
    /// (`ToastyMittens.cs`): on round 1 the relic exhausts the first
    /// NON-Innate draw card (`cards.FirstOrDefault(!Innate)`), falling back to
    /// the top card when every draw card is Innate; on later rounds it just
    /// exhausts the top card. Reshuffles the discard pile in first if the draw
    /// pile is empty (C# `await ShuffleIfNecessary`), then no-ops if both piles
    /// are empty. Bead `sts2-rs-70anj.1` (ToastyMittens before-draw timing fix).
    pub fn exhaust_top_from_draw_skipping_innate(&mut self, skip_innate: bool) {
        // An already-ending combat doesn't reshuffle (in addition to the
        // admission check at the move, `exhaust_one_from_draw_at`).
        if !crate::combat_loop::exhaust_may_begin(self) {
            return;
        }
        self.exhaust_from_draw_reshuffle_step();
        if self.player.draw.is_empty() {
            return;
        }
        let idx = if skip_innate {
            self.player
                .draw
                .iter()
                .position(|uid| {
                    self.cards
                        .get(uid.0.get() as usize - 1)
                        .map_or(true, |c| !crate::card_effects::card_innate_inst(c))
                })
                .unwrap_or(0)
        } else {
            0
        };
        self.exhaust_one_from_draw_at(idx);
    }

    /// Pop the top card of the draw pile into the exhaust pile and dual-fire
    /// `after_card_exhausted` (powers bus, then relics bus — Python order).
    /// Caller must ensure the draw pile is non-empty. Shared exhaust-one body.
    fn exhaust_one_from_draw_top(&mut self) {
        self.exhaust_one_from_draw_at(0);
    }

    /// Pop the card at `idx` of the draw pile into the exhaust pile and
    /// dual-fire `after_card_exhausted` (powers bus, then relics bus — Python
    /// order). Caller must ensure `idx` is in bounds. Shared exhaust-one body
    /// for both the top-card path and the skip-Innate path
    /// ([`Self::exhaust_top_from_draw_skipping_innate`]).
    fn exhaust_one_from_draw_at(&mut self, idx: usize) {
        // The draw-pile exhaust's admission check, immediately before the card
        // moves: after the prerequisite reshuffle and its reactions (The
        // Abacus' block firing Juggernaut can end the combat there), as C#
        // `CardCmd.Exhaust` checks only once it is called (`CardCmd.cs:239`).
        // The entry points' earlier check only spares an already-ending combat
        // the reshuffle and its Shuffle draws. Once admitted, the move and both
        // buses always run.
        if !crate::combat_loop::exhaust_may_begin(self) {
            return;
        }
        let uid = self.player.draw.remove(idx);
        self.player.exhaust.push(uid);

        // Resolve typed contexts for both buses from the arena
        // entry (mirrors the dual-fire pattern in
        // `combat_loop::end_player_turn_phase_two` /
        // `auto_play_card_from_draw_top`).
        let exhaust_idx = uid.0.get() as usize - 1;
        let exhausted_card = &self.cards[exhaust_idx];
        let card_id = exhausted_card.card_id;
        let card_tags = exhausted_card.tags;
        let card_type = exhausted_card.card_type;

        // Order: hooks bus (powers) BEFORE relic-effects bus
        // (relics) — matches Python's `fire_after_card_exhausted`.
        let hook_ctx = crate::hooks::CardCtx {
            card_id: Some(card_id),
            card_uid: Some(uid),
            card_tags,
            card_type: Some(card_type),
        };
        // Exhaust-from-draw (count-based, e.g. Stratagem) — not the
        // end-of-turn ethereal sweep, so `caused_by_ethereal = false`.
        // Bead `sts2-rs-umouk`.
        crate::hooks::dispatch_after_card_exhausted(self, &hook_ctx, false);

        let exhaust_ctx = crate::relic_effects::CardEventCtx {
            card_id,
            card_uid: Some(uid),
            target: None,
            card_tags,
            energy_spent: 0,
        };
        let _ = crate::relic_effects::dispatch_after_card_exhausted(self, &exhaust_ctx);
    }

    /// Exhaust every card currently in the player's hand. Mirrors
    /// Python's `for card in list(combat.hand): combat.exhaust_card(card)`
    /// at `sts2-rl-agent@01c094d:sts2_env/relics/shop_event.py::PaelsEye`
    /// (`before_turn_end` body). Fires `after_card_exhausted` on both
    /// the hooks bus (powers) and the relic-effects bus (relics) per
    /// exhausted card, mirroring the dual-fire pattern in
    /// [`Self::exhaust_from_draw_pile`].
    ///
    /// A snapshot of the hand uids is taken first so listeners that
    /// re-enter `exhaust_card_uid` mid-loop cannot induce iterator
    /// invalidation.
    ///
    /// Bead `sts2-rs-vfzgs`.
    pub fn exhaust_hand(&mut self) {
        let uids: smallvec::SmallVec<[crate::CardUid; 10]> =
            self.player.hand.iter().copied().collect();
        for uid in uids {
            // exhaust_card_uid handles pile removal + dual-fire of
            // `after_card_exhausted`. Returns false for already-exhausted
            // uids (defensive — should not happen since we snapshot from
            // the hand pile).
            let _ = crate::card_effects::exhaust_card_uid(self, uid);
        }
    }

    /// Count cards the player has played this turn, optionally filtered
    /// by [`CardType`]. Mirrors Python's
    /// `combat.count_cards_played_this_turn(owner, card_type=...)` at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:1985-1996`.
    ///
    /// The Rust port stores per-turn play counts as the
    /// [`PlayerCombatState::cards_played_this_turn`] bucket array
    /// (indexed by [`CardType::index`]); this accessor reads it. With
    /// `card_type = None`, returns the sum across all buckets.
    ///
    /// Bead `sts2-rs-r6l9`. Used by content like `LunarBlast` (hits =
    /// skills played this turn).
    #[must_use]
    pub fn count_player_cards_played_this_turn(
        &self,
        card_type: Option<crate::cards::CardType>,
    ) -> u32 {
        let buckets = &self.player.cards_played_this_turn;
        match card_type {
            Some(ct) => buckets[ct.index()],
            None => buckets.iter().copied().fold(0u32, u32::saturating_add),
        }
    }

    /// Apply Stomp's dynamic discount when the player plays an Attack.
    ///
    /// Stomp costs 1 less energy for each Attack its owner plays this turn
    /// (C# `Stomp.BeforeCardPlayed` → `EnergyCost.AddThisTurn(-1)`,
    /// clamped at 0). Every Stomp copy in combat is shaved by 1 (clamped),
    /// and the shaved amount is banked per-card in
    /// [`VarKey::StompCostReductionThisTurn`] so
    /// [`Self::restore_stomp_costs_for_turn_start`] can undo it on the
    /// owner's next turn — matching C#'s `AddThisTurn` auto-reset.
    ///
    /// Because the discount is applied to *every* Stomp (not just those in
    /// hand), a Stomp drawn later this turn already carries the reduced
    /// cost — subsuming C# `Stomp.AfterCardEnteredCombat` (which back-fills
    /// the discount for attacks played before the card entered combat).
    /// The caller invokes this once per Attack play (and per Echo replay).
    /// Bead `sts2-rs-stomp`.
    pub fn discount_stomp_on_attack_played(&mut self) {
        self.discount_card_cost_on_play(
            crate::cards::CardId::Stomp,
            crate::cards::VarKey::StompCostReductionThisTurn,
        );
    }

    /// Shave 1 energy off every in-combat copy of `card_id` (clamped at 0)
    /// and bank the shaved amount per-card in `bank` so the matching
    /// `restore_*_costs_for_turn_start` can undo it on the owner's next
    /// turn — the shared mechanism behind the Stomp / Pinpoint dynamic
    /// discounts (C# `EnergyCost.AddThisTurn(-1)`).
    ///
    /// Runs on every Attack (Stomp) and every Skill (Pinpoint) play, so it
    /// opens with the arena's presence bit for `card_id` (J02): the loop
    /// only touches cards with that id, so when none is in the arena it is a
    /// no-op and skipping it is bit-identical.
    fn discount_card_cost_on_play(&mut self, card_id: crate::cards::CardId, bank: cards::VarKey) {
        if !self.cards.holds_id(card_id) {
            #[cfg(debug_assertions)]
            self.shadow_skipped_card_sweep("discount_card_cost_on_play", |s| {
                s.discount_card_cost_on_play_walk(card_id, bank);
            });
            return;
        }
        self.discount_card_cost_on_play_walk(card_id, bank);
    }

    /// The ungated arena walk of [`Self::discount_card_cost_on_play`].
    fn discount_card_cost_on_play_walk(
        &mut self,
        card_id: crate::cards::CardId,
        bank: cards::VarKey,
    ) {
        for c in &mut self.cards {
            if c.card_id == card_id && c.cost > 0 {
                c.cost -= 1;
                let banked = c.combat_var(bank);
                c.set_combat_var(bank, banked.saturating_add(1));
            }
        }
    }

    /// Debug shadow of a card sweep a J02 presence gate skipped: run the
    /// ungated `walk` on a clone and assert it changed nothing (RNG cursors
    /// and all, and the event log, which `CombatState` equality ignores),
    /// i.e. that skipping it was bit-identical. Sampled like the
    /// relic-channel shadows ([`relic_effects::channels::shadow_this_call`]).
    ///
    /// # Panics
    /// If the skipped walk would have changed the state or written the log.
    #[cfg(debug_assertions)]
    fn shadow_skipped_card_sweep(&self, sweep: &str, walk: impl FnOnce(&mut CombatState)) {
        if !relic_effects::channels::shadow_this_call() {
            return;
        }
        let mut probe = self.clone();
        walk(&mut probe);
        assert!(
            probe == *self,
            "J02 presence gate skipped {sweep}, but its ungated walk changes the state"
        );
        assert!(
            probe
                .combat_log
                .event_log
                .same_contents(&self.combat_log.event_log),
            "J02 presence gate skipped {sweep}, but its ungated walk writes the event log"
        );
    }

    /// Undo the per-turn Stomp discount banked by
    /// [`Self::discount_stomp_on_attack_played`]. Called at player-turn
    /// start (mirrors C# `EnergyCost.AddThisTurn`'s reset-each-turn
    /// semantics): restore each Stomp's cost by the banked amount and
    /// clear the slot. Bead `sts2-rs-stomp`.
    pub fn restore_stomp_costs_for_turn_start(&mut self) {
        self.restore_banked_costs_for_turn_start(
            crate::cards::CardId::Stomp,
            crate::cards::VarKey::StompCostReductionThisTurn,
        );
    }

    /// Undo a per-turn discount banked in `bank` by
    /// [`Self::discount_card_cost_on_play`]: restore each card's cost by
    /// the banked amount and clear the slot.
    ///
    /// Fast absence gate (perf 2026-07-04, Melancholy pattern): the banked
    /// var is only ever set non-zero on cards whose `card_id` matched at
    /// banking time (`discount_card_cost_on_play` and
    /// `stamp_entered_combat_cost_backfill` both gate on the id), the card
    /// arena is append-only for the whole combat, and no in-combat path
    /// mutates `card_id` in place (transforms allocate a fresh arena row,
    /// leaving the old row — and its banked var — behind). So "no such card
    /// anywhere in the arena" proves every banked value is 0 and the
    /// mutable restore walk below is a no-op; skipping it is bit-identical.
    /// The arena answers that from its presence bit for `card_id` (J02).
    fn restore_banked_costs_for_turn_start(
        &mut self,
        card_id: crate::cards::CardId,
        bank: cards::VarKey,
    ) {
        if !self.cards.holds_id(card_id) {
            #[cfg(debug_assertions)]
            self.shadow_skipped_card_sweep("restore_banked_costs_for_turn_start", |s| {
                s.restore_banked_costs_walk(bank);
            });
            return;
        }
        self.restore_banked_costs_walk(bank);
    }

    /// The ungated arena walk of [`Self::restore_banked_costs_for_turn_start`].
    fn restore_banked_costs_walk(&mut self, bank: cards::VarKey) {
        for c in &mut self.cards {
            let banked = c.combat_var(bank);
            if banked != 0 {
                c.cost = c.cost.saturating_add(banked as i8);
                c.set_combat_var(bank, 0);
            }
        }
    }

    /// Apply Pinpoint's dynamic discount when the player plays a Skill.
    ///
    /// Pinpoint costs 1 less energy for each Skill its owner plays this turn
    /// (C# `Pinpoint.AfterCardPlayed` → `EnergyCost.AddThisTurn(-1)`, floored
    /// at 0). This is the Skill-triggered twin of
    /// [`Self::discount_stomp_on_attack_played`]; see that method for the
    /// banked-per-card / discount-every-copy rationale (which subsumes C#
    /// `Pinpoint.AfterCardEnteredCombat`'s back-fill). Without it, Pinpoint
    /// stayed at cost 3 and was wrongly dropped from `legal_actions` once the
    /// player spent enough energy on Skills (Silent sweep LEGAL_ACTION diff).
    pub fn discount_pinpoint_on_skill_played(&mut self) {
        self.discount_card_cost_on_play(
            crate::cards::CardId::Pinpoint,
            crate::cards::VarKey::PinpointCostReductionThisTurn,
        );
    }

    /// Undo the per-turn Pinpoint discount banked by
    /// [`Self::discount_pinpoint_on_skill_played`]. Called at player-turn
    /// start (mirrors C# `EnergyCost.AddThisTurn`'s reset-each-turn
    /// semantics). Twin of [`Self::restore_stomp_costs_for_turn_start`].
    pub fn restore_pinpoint_costs_for_turn_start(&mut self) {
        self.restore_banked_costs_for_turn_start(
            crate::cards::CardId::Pinpoint,
            crate::cards::VarKey::PinpointCostReductionThisTurn,
        );
    }

    /// Undo Snecko Oil's per-turn cost randomization banked by
    /// [`crate::cards::VarKey::SneckoCostDeltaThisTurn`]. Called at
    /// player-turn start, mirroring C# `EnergyCost.SetThisTurnOrUntilPlayed`'s
    /// auto-revert at the owner's next turn start: subtract the banked
    /// delta to restore each card's pre-randomization cost and clear the
    /// slot. Bead `sts2-rs-2e9q2` follow-up.
    pub fn restore_snecko_costs_for_turn_start(&mut self) {
        // Fast absence gate (perf 2026-07-04, J02): the Snecko delta has no
        // card-id key (any hand card can carry it). A card whose "may be
        // nonzero" bit for it is clear holds a 0 delta, so when no card has
        // the bit every banked delta reads 0 and the walk below is a pure
        // no-op — skip it. (The old gate, "every `combat_vars` empty", stayed
        // open for the whole combat once any card var was written, zeros
        // included.)
        if !self.cards.iter().any(|c| {
            c.combat_vars
                .maybe_nonzero(crate::cards::VarKey::SneckoCostDeltaThisTurn)
        }) {
            #[cfg(debug_assertions)]
            self.shadow_skipped_card_sweep(
                "restore_snecko_costs_for_turn_start",
                CombatState::restore_snecko_costs_walk,
            );
            return;
        }
        self.restore_snecko_costs_walk();
    }

    /// The ungated arena walk of [`Self::restore_snecko_costs_for_turn_start`].
    fn restore_snecko_costs_walk(&mut self) {
        for c in &mut self.cards {
            let banked = c.combat_var(crate::cards::VarKey::SneckoCostDeltaThisTurn);
            if banked != 0 {
                c.cost = c.cost.saturating_sub(banked as i8);
                c.set_combat_var(crate::cards::VarKey::SneckoCostDeltaThisTurn, 0);
                // The expiring Snecko roll was the absolute modifier the
                // upgrade-clamp flag tracks; the cost is base-owned again.
                c.set_combat_var(crate::cards::VarKey::CostAbsolutelySet, 0);
            }
        }
    }

    /// Undo the this-turn cost discounts a foreign producer had already
    /// applied when this state was reconstructed, banked in
    /// [`crate::cards::VarKey::WireTransientCostThisTurn`].
    ///
    /// The per-effect restores ([`Self::restore_stomp_costs_for_turn_start`]
    /// and friends) only cover discounts *this* engine applied and banked. A
    /// state rebuilt mid-turn from a C# wire inherits an already-shaved cost
    /// with no bank behind it, so its discount would never expire — the
    /// reconstruction records the shave here instead, and this restores it at
    /// the next player-turn start.
    ///
    /// Not literally C#'s boundary: `CardEnergyCost.EndOfTurnCleanup()` drops
    /// the `EndOfTurn` modifiers when the turn ENDS, so between end-of-player-
    /// turn and the next player-turn start this engine still holds the discount.
    /// Deliberate, and the same convention as
    /// [`Self::restore_snecko_costs_for_turn_start`] /
    /// [`Self::restore_stomp_costs_for_turn_start`], which is why it is here and
    /// not in the end-turn path — nothing between those two points can read a
    /// card's cost (the player is not deciding, and the monster turn does not
    /// price player cards), so the window is unobservable. Moving it would mean
    /// moving all three.
    ///
    /// Disjoint from the per-effect banks by construction: this one covers
    /// what happened BEFORE the reconstruction, they cover what this engine
    /// did AFTER it, so both firing at turn start restores the whole shave
    /// exactly once.
    pub fn restore_wire_transient_costs_for_turn_start(&mut self) {
        // Same fast absence gate as the Snecko restore: the bank has no
        // card-id key, but a card can only hold a nonzero one if its
        // "may be nonzero" bit for it is set.
        if !self.cards.iter().any(|c| {
            c.combat_vars
                .maybe_nonzero(crate::cards::VarKey::WireTransientCostThisTurn)
        }) {
            #[cfg(debug_assertions)]
            self.shadow_skipped_card_sweep(
                "restore_wire_transient_costs_for_turn_start",
                CombatState::restore_wire_transient_costs_walk,
            );
            return;
        }
        self.restore_wire_transient_costs_walk();
    }

    /// The ungated arena walk of
    /// [`Self::restore_wire_transient_costs_for_turn_start`].
    fn restore_wire_transient_costs_walk(&mut self) {
        for c in &mut self.cards {
            let banked = c.combat_var(crate::cards::VarKey::WireTransientCostThisTurn);
            if banked != 0 {
                c.cost = c.cost.saturating_add(banked as i8);
                c.set_combat_var(crate::cards::VarKey::WireTransientCostThisTurn, 0);
                rebank_wire_cost_after_edge(c, crate::cards::VarKey::WireCostExpiringOnPlay);
            }
        }
    }

    /// The WHEN-PLAYED half of the same story: undo the part of a foreign
    /// producer's discount that the game retires the moment the card is
    /// played, banked in [`crate::cards::VarKey::WireCostExpiringOnPlay`].
    ///
    /// Mirrors C# `CardEnergyCost.AfterCardPlayedCleanup()`, which drops every
    /// modifier flagged `WhenPlayed` — and the "costs N this turn" helpers are
    /// `EndOfTurn | WhenPlayed`, whichever comes first, so this edge fires far
    /// more often than the rare pure-`SetUntilPlayed`.
    pub fn restore_wire_cost_expiring_on_play(&mut self, card_uid: crate::CardUid) {
        let Some(c) = card_by_uid_mut(&mut self.cards, card_uid) else {
            return;
        };
        // The free riders are `WhenPlayed`-flagged in C# too: `SetToFreeThisTurn`
        // is `EnergyCost.SetThisTurnOrUntilPlayed(0)` (CardModel.cs:1267-1271)
        // and `FreeUntilPlayed` is `SetUntilPlayed(0)` — both dropped by
        // `AfterCardPlayedCleanup` the moment the card is played. The Rust
        // riders only died at end-of-turn cleanup, so a generated-free card
        // still read cost 0 in the exhaust/discard pile after its play — masked
        // for as long as reconstruction wiped `combat_vars` every reconcile,
        // and 71 `$.deck.exhaust_pile` cost_override desyncs on the v28wr sweep
        // once the transplant started carrying them.
        c.set_combat_var(crate::cards::VarKey::FreeThisTurn, 0);
        c.set_combat_var(crate::cards::VarKey::FreeUntilPlayed, 0);
        let banked = c.combat_var(crate::cards::VarKey::WireCostExpiringOnPlay);
        if banked == 0 {
            return;
        }
        c.cost = c.cost.saturating_add(banked as i8);
        c.set_combat_var(crate::cards::VarKey::WireCostExpiringOnPlay, 0);
        rebank_wire_cost_after_edge(c, crate::cards::VarKey::WireTransientCostThisTurn);
    }

    /// Randomize the cost of every non-X-cost card in hand to a uniform value
    /// in `0..=3` — C# `SneckoOil.OnUse`'s `foreach` after the draw. For each
    /// hand card with `cost >= 0` (`!CostsX`), roll `Rng.CombatEnergyCosts
    /// .NextInt(4)` and bank `roll - prior_cost` in `SneckoCostDeltaThisTurn` so
    /// the temporary `SetThisTurnOrUntilPlayed` override reverts at the next
    /// player-turn start (see [`Self::restore_snecko_costs_for_turn_start`]). The
    /// `combat_energy_costs` roll is consumed ONLY for cards that pass the guard,
    /// matching C#'s `Where(!CostsX)` before `NextInt`.
    ///
    /// MUST run only after the SneckoOil draw FULLY completes: C# `await`s the
    /// 7-card `CardPileCmd.Draw` (including any Stratagem `AfterShuffle` pick that
    /// suspends it) BEFORE the `foreach`. When the Rust draw suspends mid-reshuffle,
    /// `drink_snecko_oil` arms [`Self::pending_snecko_cost_randomize`] and the
    /// `ResolveChoice` resume site calls this once the complete hand is present, so
    /// every drawn card is rolled in one contiguous block — keeping the
    /// `combat_energy_costs` stream in lockstep with C#. Bead `sts2-rs-snecko-suspend`.
    pub fn snecko_randomize_hand_costs(&mut self) {
        use crate::cards::VarKey;
        let hand_uids: smallvec::SmallVec<[crate::CardUid; 16]> =
            self.player.hand.iter().copied().collect();
        for uid in hand_uids {
            let idx = uid.0.get() as usize - 1;
            // Read the current cost first; skip X-cost cards WITHOUT drawing a
            // roll (C# excludes `CostsX` from the `Where` before `NextInt`).
            let Some(cur_cost) = self.cards.get(idx).map(|c| c.cost) else {
                continue;
            };
            if cur_cost < 0 {
                continue;
            }
            let roll = self.run_rng.combat_energy_costs.next_int(0, 3) as i8;
            if let Some(card) = self.cards.get_mut(idx) {
                let delta = i32::from(roll) - i32::from(cur_cost);
                card.cost = roll;
                let banked = card.combat_var(VarKey::SneckoCostDeltaThisTurn);
                card.set_combat_var(
                    VarKey::SneckoCostDeltaThisTurn,
                    banked.saturating_add(delta),
                );
                // Clear any `FreeThisTurn` override so the Snecko roll wins (C#
                // appends the Absolute roll modifier AFTER the free-this-turn
                // Absolute(0) in the ordered walk).
                card.set_combat_var(VarKey::FreeThisTurn, 0);
                // The roll is an ABSOLUTE local modifier — flag it so an in-combat
                // upgrade clamps it per C# `CardEnergyCost.UpgradeBy`.
                card.set_combat_var(VarKey::CostAbsolutelySet, 1);
            }
        }
    }

    /// Remove `uid` from whichever player pile currently owns it
    /// (`hand` / `draw` / `discard` / `exhaust` / `play`). Returns
    /// `true` if the card was found and removed, `false` otherwise.
    ///
    /// Mirrors `sts2-rl-agent@01c094d:sts2_env/core/combat.py::
    /// CombatState._remove_card_from_piles` (lines 1755-1760), which
    /// scans each pile for the card and removes the first match.
    ///
    /// Used by [`Self::move_card_to_hand`] and
    /// [`Self::insert_card_into_draw_pile`] (bead `sts2-rs-grne`) to
    /// reroute a currently-resolving card out of the play pile back
    /// into hand (`ParticleWall`) or onto the top of draw
    /// (`ShiningStrike`, `Begone`). Public for the run-AI's
    /// arrival-state counterfactual probe (`fork_diagnosis`
    /// entry mutations — spec
    /// `2026-07-18-arrival-state-counterfactual-probe-design.md`),
    /// which strips Curse cards from a snapshotted fight entry.
    pub fn remove_card_from_piles(&mut self, uid: CardUid) -> bool {
        if let Some(pos) = self.player.hand.iter().position(|u| *u == uid) {
            self.player.hand.remove(pos);
            return true;
        }
        if let Some(pos) = self.player.draw.iter().position(|u| *u == uid) {
            self.player.draw.remove(pos);
            return true;
        }
        if let Some(pos) = self.player.discard.iter().position(|u| *u == uid) {
            self.player.discard.remove(pos);
            return true;
        }
        if let Some(pos) = self.player.exhaust.iter().position(|u| *u == uid) {
            self.player.exhaust.remove(pos);
            return true;
        }
        if let Some(pos) = self.player.play.iter().position(|u| *u == uid) {
            self.player.play.remove(pos);
            return true;
        }
        false
    }

    /// Move the card identified by `uid` from its current pile into
    /// the player's hand. If the hand is at [`crate::cards::HAND_LIMIT`]
    /// (Python `MAX_HAND_SIZE = 10`), the card is routed to the
    /// discard pile instead — mirroring Python's overflow behaviour.
    ///
    /// Mirrors `sts2-rl-agent@01c094d:sts2_env/core/combat.py::
    /// CombatState.move_card_to_hand` / `move_card_to_creature_hand`
    /// (lines 1679-1691):
    ///
    /// ```python
    /// def move_card_to_creature_hand(self, creature, card):
    ///     self._remove_card_from_piles(card)
    ///     ...
    ///     if len(zones["hand"]) < MAX_HAND_SIZE:
    ///         zones["hand"].append(card)
    ///     else:
    ///         zones["discard"].append(card)
    /// ```
    ///
    /// Bead `sts2-rs-grne`. Required by `ParticleWall`'s
    /// resolve→hand reroute (the currently-resolving card lives in
    /// `play` while the card-play pipeline is mid-resolution; this
    /// surface lifts it back into `hand`).
    ///
    /// Defensive no-op if `uid` is not present in any pile.
    pub fn move_card_to_hand(&mut self, uid: CardUid) {
        if !self.remove_card_from_piles(uid) {
            return;
        }
        if self.player.hand.len() < crate::cards::HAND_LIMIT {
            self.player.hand.push(uid);
        } else {
            self.player.discard.push(uid);
        }
    }

    /// Insert the card identified by `uid` into the player's draw
    /// pile after removing it from any pile that currently holds it.
    /// When `random_position` is `false`, the card lands at index 0
    /// (top of draw, drawn next). When `true`, an insertion index in
    /// `0..=draw.len()` is sampled from [`Self::run_rng`]'s shuffle stream
    /// (C#'s `CardPilePosition.Random`) and the card is inserted there.
    ///
    /// Mirrors `sts2-rl-agent@01c094d:sts2_env/core/combat.py::
    /// CombatState.insert_card_into_draw_pile` /
    /// `insert_card_into_creature_draw_pile` (lines 1734-1753):
    ///
    /// ```python
    /// def insert_card_into_creature_draw_pile(
    ///     self, creature, card, *, random_position=False,
    /// ) -> None:
    ///     self._remove_card_from_piles(card)
    ///     ...
    ///     if random_position:
    ///         insert_at = self.rng.next_int(0, len(zones["draw"]))
    ///         zones["draw"].insert(insert_at, card)
    ///     else:
    ///         zones["draw"].insert(0, card)
    /// ```
    ///
    /// Bead `sts2-rs-grne`. NOTE (final-review finding, bead
    /// `sts2-rs-ztl78`): despite the doc history above, `ShiningStrike`'s
    /// return-to-top epilogues route through their OWN inline pinned insert
    /// in `combat_loop.rs` (not this function), and `Begone`
    /// (`sts2-content/src/cards/regent/begone.rs`) transforms a hand card and
    /// never calls this function at all — so the `random_position: false`
    /// branch below is currently dead in live content. The real live
    /// callers are `sts2-content/src/monsters/act2_hive.rs:384,926`, both
    /// with `random_position: true`. Kept for any future
    /// definite-position-insert caller of the same "player watched it land
    /// on top" class.
    ///
    /// Defensive no-op if `uid` is not present in any pile.
    pub fn insert_card_into_draw_pile(&mut self, uid: CardUid, random_position: bool) {
        if !self.remove_card_from_piles(uid) {
            return;
        }
        if random_position {
            // Bead `sts2-rs-09x5n` (F2 `HiddenInfoMode::BeliefDraw`,
            // lightspeed `CardPile` port): under a deferred-draw-order pile
            // there is no physical position for a random index to land in
            // — fold `uid` into the canonical (uid-sorted) unknown segment
            // instead, consuming no RNG.
            if self.player.deferred_draw_order {
                crate::cards::insert_into_unknown_sorted(&mut self.player, uid);
                return;
            }
            let len = self.player.draw.len();
            // C# `CardPilePosition.Random` samples `RunState.Rng.Shuffle
            // .NextInt(Cards.Count + 1)` (CardPileCmd.cs:433) — the SHUFFLE
            // stream, not the legacy combat `rng`. `next_int(a, b)` is
            // inclusive on both ends, so `0..=len` gives `len + 1` slots
            // (matching `NextInt(count + 1)`, incl. a true append). Bead
            // `sts2-rs-ybtmu`: corrected from `self.rng` (a Python-pin
            // mirror; the Python sim drifts from C# here) so the random-
            // position callers (TheInsatiable LIQUIFY, Entomancer Dazed
            // flood) advance the Shuffle stream the harness reads.
            let insert_at = self.run_rng.shuffle.next_int(0, len as i32) as usize;
            self.player.draw.insert(insert_at, uid);
        } else {
            self.player.draw.insert(0, uid);
            // Definite-position insert at draw-top — same "player watched
            // it land on top" class as Headbutt/CosmicIndifference/
            // ShiningStrike/Nostalgia, all of which pin. Future-proofs this
            // currently-dead branch so it doesn't silently reopen the
            // stale-pin bug class (final-review finding, bead
            // `sts2-rs-ztl78`) if it gains a live caller later. Called
            // unconditionally — same as the ShiningStrike inline pinned
            // insert in `combat_loop.rs` — since `pinned_top` also serves
            // real (non-deferred) gameplay: `cards::shuffle`'s non-deferred
            // branch preserves pinned-card order across a physical shuffle.
            crate::cards::pin_top(&mut self.player, uid);
        }
    }

    /// Discard-pile analogue of [`Self::insert_card_into_draw_pile`]. C#
    /// `CardPilePosition.Random` (CardPileCmd.cs:433) samples
    /// `RunState.Rng.Shuffle.NextInt(Cards.Count + 1)` and inserts the card
    /// at that index; `random_position: false` appends to the back. Used by
    /// `TheInsatiable` LIQUIFY_GROUND's discard-pile FranticEscape inserts so
    /// the Shuffle-RNG consumption matches C# — the discard ORDER is not
    /// draw-observable, but the RNG draws desync every later draw/reshuffle in
    /// the combat if skipped. Defensive no-op if `uid` is absent from all
    /// piles. Bead `sts2-rs-ybtmu`.
    pub fn insert_card_into_discard_pile(&mut self, uid: CardUid, random_position: bool) {
        if !self.remove_card_from_piles(uid) {
            return;
        }
        if random_position {
            let len = self.player.discard.len();
            let insert_at = self.run_rng.shuffle.next_int(0, len as i32) as usize;
            self.player.discard.insert(insert_at, uid);
        } else {
            self.player.discard.push(uid);
        }
    }

    /// Count powered attack hits the player has dealt this turn.
    /// Mirrors Python's
    /// `combat.count_powered_hits_by_dealer_this_turn(owner)` at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:1526-1531` for
    /// `owner == player`. Reads the
    /// [`PlayerCombatState::powered_hits_this_turn`] scalar, which
    /// `crate::damage::apply_damage` increments whenever the player
    /// is the dealer and the hit's [`hooks::ValueProps::is_powered`]
    /// returns true (not tagged `UNPOWERED`).
    ///
    /// Used by `BeatIntoShape` (`sts2-rl-agent@01c094d:sts2_env/
    /// cards/regent.py:599-607`) to scale `forge_per_hit`. The
    /// player-uid view (no per-target keying) is sufficient because
    /// `BeatIntoShape`'s caller threads its own target separately
    /// and the only on-target consumer in Python at this pin uses
    /// `count_powered_hits_this_turn(owner, target)` solely with
    /// `owner == player` — collapsing the per-(dealer,target) map
    /// to a player-totals scalar drops a hot-path allocation
    /// without changing observable behavior for the Phase-6
    /// content set. Bead `sts2-rs-jm6e`.
    #[must_use]
    pub fn count_player_powered_hits_this_turn(&self) -> u32 {
        self.player.powered_hits_this_turn
    }

    /// Per-target version of
    /// [`Self::count_player_powered_hits_this_turn`]: the number of
    /// powered attack hits the player has dealt **to `target`** this turn.
    /// Mirrors C# `BeatIntoShape.cs:27-31`'s `CalculatedForge` multiplier,
    /// which counts `DamageReceivedEntry`s strictly per-target
    /// (`e.Receiver == target`). Reads the
    /// [`PlayerCombatState::powered_hits_by_target_this_turn`] tally that
    /// [`crate::damage::apply_damage`] keys by receiver uid.
    ///
    /// Used by `BeatIntoShape` to scale its forge so that, in a
    /// multi-enemy fight, prior powered hits the player landed on a
    /// *different* enemy this turn do NOT inflate the forge played on
    /// this target. Returns 0 for a target with no recorded hits. Bead
    /// `sts2-rs-jm6e` (per-target follow-up, issue #177).
    #[must_use]
    pub fn count_player_powered_hits_on_target_this_turn(&self, target: CreatureUid) -> u32 {
        self.player
            .powered_hits_by_target_this_turn
            .iter()
            .find(|(uid, _)| *uid == target)
            .map_or(0, |(_, n)| *n)
    }

    /// Rust analogue of Python's
    /// `combat.count_generated_cards_this_combat(owner)` at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:1547-1548`,
    /// specialised to `owner == player` (the only call site in the
    /// pin — `Supermassive`'s damage scaling at
    /// `sts2-rl-agent@01c094d:sts2_env/cards/regent.py:578`).
    ///
    /// Reads [`PlayerCombatState::generated_cards_this_combat`], which
    /// is incremented by [`crate::card_effects::spawn_card_into_hand`]
    /// and reset by [`crate::combat_loop::start_combat`]. Bead
    /// `sts2-rs-8h2q`.
    #[must_use]
    pub fn count_player_generated_cards_this_combat(&self) -> u32 {
        self.player.generated_cards_this_combat
    }
}

/// After one transient edge has restored its shave, recompute the OTHER
/// bank against [`crate::cards::VarKey::WireCostStable`] instead of leaving it
/// as the producer sent it.
///
/// This is what makes the two banks composable. The two banks alone cannot say
/// how much of the shave SURVIVES the first edge: for a single shared
/// `EndOfTurn | WhenPlayed` modifier the whole shave is already restored and
/// the remaining bank must recompute to zero, while for two independent
/// modifiers the half that has not expired yet must stay. Only the
/// after-both-edges cost separates those. Leaving the other bank alone would
/// restore a shared shave twice.
///
/// A card with no wire bank set has `other == 0` and this is a no-op. A card
/// WITH a bank always has `WireCostStable` too — the parser refuses a wire that
/// sends part of the trio, precisely because this reads the var back through a
/// default-0 accessor and could not otherwise tell an absent `stable_cost` from
/// a real zero.
fn rebank_wire_cost_after_edge(c: &mut crate::cards::Card, other: crate::cards::VarKey) {
    if c.combat_var(other) == 0 {
        return;
    }
    let stable = c.combat_var(crate::cards::VarKey::WireCostStable);
    c.set_combat_var(other, stable - i32::from(c.cost));
}

// ---------------------------------------------------------------------------
// apply
// ---------------------------------------------------------------------------

/// Single-step transition function — the public entry point for MCTS /
/// scripted play. Routes the [`Action`] to the appropriate sub-pipeline:
///
///   * [`Action::EndTurn`] dispatches through
///     [`combat_loop::end_turn_dispatch`], walking the 6-step chain:
///     `end_player_turn_phase_one -> end_player_turn_phase_two ->
///     enemy_turn_start -> enemy_turn_exec -> enemy_turn_end ->
///     start_player_turn`. Returns the first non-`Advanced`
///     [`StepResult`] from any sub-step (`CombatWon` / `CombatLost`
///     short-circuit; `PendingChoice` propagates to the caller).
///     `Action::EndTurn` is only valid in [`CombatPhase::PlayerPlayPhase`];
///     out-of-phase invocations no-op per the bead spec.
///   * [`Action::UsePotion`]: delegates to [`Action::DrinkPotion`]
///     (GH #303 — it used to be a silent no-op stub).
//
// `needless_pass_by_value` is preserved as an allow because the match
// consumes `action` by move on some arms (e.g. the SmallVec inside
// `ResolveChoice`) while others only read scalars out of it.
//
// `match_same_arms` is allowed because the explicit per-variant arms
// force a compile error when `Action` gains a new variant, which is the
// safety property we want — collapsing look-alike arms to a wildcard
// would silently swallow new variants.
#[allow(clippy::needless_pass_by_value, clippy::match_same_arms)]
pub fn apply(state: &mut CombatState, action: Action) -> StepResult {
    // A state built before the relic channels froze carries no relic plan
    // (its planned dispatchers take their generic path); build it now.
    state.relics.ensure_plan();
    #[cfg(debug_assertions)]
    state.relics.debug_assert_derived_views();
    #[cfg(debug_assertions)]
    state.cards.debug_assert_gate_bits();
    match action {
        Action::PlayCard { hand_index, target } => {
            // P6 (beads sts2-rs-y97 + sts2-rs-06w): Ironclad starter deck
            // complete — Strike + Defend + Bash wired end-to-end. The
            // remaining 70+ Ironclad cards (and ~500 cross-character cards)
            // remain no-op stubs inside `combat_loop::play_card`'s effect
            // match; they fall through `AscendersBane` exhaustively until
            // each is ported under the P6 umbrella.
            combat_loop::play_card(state, hand_index, target)
        }
        Action::EndTurn => {
            // Action::EndTurn is only valid mid-play. Out-of-phase
            // invocations (e.g., during Setup, mid-EnemyTurnExec, or
            // post-resolution) are observable no-ops at this pin
            // (return Advanced without mutation). The bead spec
            // initially proposed a `debug_assert_eq!` here, but the
            // existing `apply_returns_advanced_for_all_actions` test
            // exercises Action::EndTurn against a Setup-phase state
            // (defensive-shape coverage, not an end-turn validity
            // check), so a debug_assert would panic in unit tests.
            // The runtime `if` guard alone is sufficient — Phase 6
            // (bead sts2-rs-2un) lifts both this no-op and tightens
            // the validation by routing invalid actions through a
            // proper `Action::is_legal(state)` predicate.
            if state.phase != CombatPhase::PlayerPlayPhase {
                return StepResult::Advanced;
            }
            combat_loop::end_turn_dispatch(state)
        }
        Action::UsePotion { slot, target } => {
            // GH #303. This arm was the Phase-7 stub (`=> Advanced`), i.e.
            // a SILENT no-op: the potion stayed in the belt, no effect
            // fired, and the caller was told the step succeeded. Every
            // in-engine producer emits `DrinkPotion`, so the trap was
            // reserved for external callers using the documented
            // back-compat verb — it cost a mis-attributed Touch of
            // Insanity investigation (the card_select replay in
            // `headless_eval.py` → `combat_mirror.apply_step` drank a
            // potion that never happened, then "diverged"). The
            // decision-stability probe is NOT a victim: it converts to
            // the run verb `drink_potion` and goes through
            // `RunManager.take_action`, never this bridge shape.
            //
            // `UsePotion` is now what its doc-comment always claimed:
            // an alias. Delegating (rather than erroring) is the fix the
            // issue prefers — the verb is legitimate, it was only ever
            // the implementation that was missing. A no-op that reports
            // success is exactly the landmine that desyncs a driven
            // mirror, so the one outcome ruled out is the old one.
            apply(
                state,
                Action::DrinkPotion {
                    potion_slot: slot,
                    target,
                },
            )
        }
        Action::DrinkPotion {
            potion_slot,
            target,
        } => {
            // Phase 7 (sts2-rs-mvsu): wire per-potion effect dispatch
            // through the `potion_effects` registry installed by
            // `sts2-content::potions::dispatch::install_default_registry`.
            //
            // The slot consumption below was the partial fix from
            // `sts2-rs-czfg` (DrinkPotion no-op infinite-loops MCTS
            // heuristic rollouts: heuristic weights EndTurn priority
            // 10_000 → near-zero softmax weight, so a no-op DrinkPotion
            // is re-picked indefinitely and depth-cuts the rollout at
            // wins=0/losses=0/ties=N). We still consume first — even
            // when no body is registered (`EffectResult::NotImplemented`
            // / marker `Resolved`) — so legal_actions stops re-emitting
            // the same action regardless of dispatch outcome. Bodies
            // for the 63 potions land per the `sts2-rs-6ynt.9` activation
            // waves; until then dispatch returns `Resolved` from the
            // marker handlers in content's `install_default_registry`.
            //
            // Hook integration: the `after_potion_used` relic-side
            // channel fires AFTER the potion body resolves (bead
            // `sts2-rs-drisx`). Listeners (e.g. `ReptileTrinket`'s
            // +3-Strength) see the post-effect state. The bead
            // intentionally does NOT add a pre-resolution
            // `before_potion_used` channel — no Python relic at the
            // pin observes that timing.
            let slot = usize::from(potion_slot);
            if slot >= state.player.potions.len() {
                return StepResult::Advanced;
            }
            // `PotionUsage.Automatic` (Fairy in a Bottle): C# doc-comments
            // `FairyInABottle.OnUse` as never executed directly by
            // `UsePotionAction` — only its `AfterPreventingDeath` trigger
            // (`try_fairy_in_a_bottle_prevent_death`) may consume it.
            // Reject the manual drink as `InvalidTarget` (state unmutated,
            // the documented legality-reject) rather than a
            // success-reporting no-op: an external driver (Python bridge,
            // workbench mirror, pre-fix-corpus replay) that sees `Advanced`
            // marks the potion consumed on its side and its belt model
            // desyncs — exactly the GH #303 landmine above. No in-engine
            // producer can hit this: `legal_actions` no longer emits
            // DrinkPotion for Automatic slots, so MCTS/rollouts never send
            // it, and the `sts2-rs-czfg` no-op livelock stays impossible.
            // A8 audit §7a.
            if state.player.potions[slot].is_automatic() {
                return StepResult::InvalidTarget;
            }
            // Resolve a default target for targeted (`AnyEnemy`) potions
            // when none was supplied. `legal_actions` emits DrinkPotion
            // with `target: None` for every slot (per-enemy fan-out is
            // deferred, sts2-rs-h9u), so a targeted potion such as
            // FirePotion would otherwise no-op (drink_fire_potion needs a
            // target). The C# game resolves a null potion target to the
            // first alive enemy (`RunSimulator.DoUsePotion`), so mirror
            // that here. Self / no-target potions ignore the argument, so
            // defaulting it is harmless for them.
            let resolved_target = target.or_else(|| {
                state.enemy_uids.iter().enumerate().find_map(|(i, uid)| {
                    let alive = state
                        .creatures
                        .iter()
                        .any(|c| c.id == uid.0 && c.current_hp > 0);
                    #[allow(clippy::cast_possible_truncation)]
                    alive.then_some(i as u8)
                })
            });
            // Bead `sts2-rs-ybtmu`: `SurroundedPower.BeforePotionUsed` — a
            // single-enemy-targeted potion re-faces the surrounded player
            // toward its victim before the potion resolves (exposing the
            // OTHER KaiserCrab arm at the player's back). C# fires
            // `UpdateDirection(target)` with the potion's resolved enemy
            // target; Self / AllEnemies potions carry no single target and
            // never flip. We key off the potion's `AnyEnemy` target type
            // (not the harness-supplied explicit `target`, which
            // `legal_actions` emits as `None` for every slot — the
            // per-enemy fan-out is deferred, sts2-rs-h9u) applied to the
            // same `resolved_target` the effect uses, so a `VULNERABLE_POTION`
            // aimed at the first alive enemy flips facing exactly as C# does.
            let kind_peek = state.player.potions[slot];
            if matches!(
                crate::potion_effects::lookup_target_type(kind_peek),
                Some(crate::potions::PotionTargetType::AnyEnemy)
            ) {
                if let Some(t_slot) = resolved_target {
                    if let Some(t_uid) = state.enemy_uids.get(t_slot as usize).copied() {
                        crate::hooks::update_surrounded_facing(state, t_uid);
                    }
                }
            }
            // Leaves the physical belt slot empty (C# RemovePotionInternal)
            // so a later drop refills THIS slot, listing before later slots.
            let kind = state.player.remove_potion_at(slot);
            let _ = potion_effects::dispatch(state, kind, resolved_target);
            // C# `PotionModel.OnUseWrapper` (PotionModel.cs:291-342) fires
            // `Hook.AfterPotionUsed` (`:338`) only after `await OnUse(...)`
            // (`:327`) fully completes — INCLUDING any card-select the potion
            // surfaces (the Attack/Skill/Power/Colorless Potion choose-a-card
            // screen). When the effect left a live `pending_choice`, park the
            // dispatch on `pending_potion_hook_resume`; the
            // `resolve_choice_atomic` tail fires it once the pick resolves.
            // Dispatching here instead landed `ReptileTrinket`'s +3 temporary
            // Strength on the choice-screen decision point — one decision
            // EARLY vs the DLL (mirror corpus `$.player.powers`
            // REPTILE_TRINKET class, seeds 1002/1022/1041).
            if state.pending_choice.is_some() {
                state.pending_potion_hook_resume = Some(PendingPotionHookResume {
                    kind,
                    target: resolved_target,
                });
            } else {
                let _ = relic_effects::dispatch_after_potion_used(state, kind, resolved_target);
            }
            // A potion that opened a choose-a-card screen (AttackPotion,
            // SkillPotion, PowerPotion, ColorlessPotion — all four route
            // through `open_generated_free_choice`) leaves a live
            // `pending_choice` behind, and the caller has to be TOLD, exactly
            // as the card-play path tells it (`combat_loop.rs`'s
            // `pending_choice.is_some()` gate before `finish_card_play`).
            //
            // Returning a bare `Advanced` here made the screen invisible to
            // every driver that steps this engine off the result: the DLL
            // mirror saw `advanced`, dropped the parked choice it never
            // received, and answered the game's screen on a state that had no
            // idea a card was being granted — 102 of 107 unparked injections
            // in a 4-seed probe were exactly this, one per card-generating
            // potion drunk. In-process rollouts never noticed because they
            // read `pending_choice` directly instead of the `StepResult`.
            //
            // A lethal potion (PotionShapedRock, FirePotion, ExplosiveAmpoule,
            // …) can kill the last enemy. C# routes the drink through a
            // `UsePotionAction` whose damage triggers normal combat-end
            // detection, so the killing drink resolves straight to the rewards
            // screen. `play_card` runs `check_combat_end` after every effect;
            // the DrinkPotion arm used to skip it and return a bare `Advanced`,
            // leaving `is_over` unset — so the run layer kept Rust in the play
            // phase while C# moved on (PARITY11: a PotionShapedRock killed the
            // lone enemy → `cs=card_reward rs=combat`). Mirror `play_card` and
            // run the check, guarded on a non-empty enemy roster so degenerate
            // / setup-phase fixtures (empty `enemy_uids`) don't resolve as a
            // vacuous victory. Bead `sts2-rs-potion-lethal-combat-end`.
            //
            // FIRST, deliberately. `check_combat_end` is what SETS `is_over`, so
            // the two guards below have to run after it or they read a stale
            // flag: a potion that both opens a card screen and kills the last
            // enemy would surface a live `PendingChoice` — or fire an end-turn
            // rider — on a combat that is already over. No potion does both
            // today, which is why this was latent rather than a bug.
            let ended = if state.enemy_uids.is_empty() {
                StepResult::Advanced
            } else {
                combat_loop::check_combat_end(state)
            };
            // Guarded on `is_over` for the same reason the play path is: a
            // finished combat has no live decision to surface, and the choice
            // dies with it.
            if state.pending_choice.is_some() && !state.is_over {
                return StepResult::PendingChoice;
            }
            // VoidForm-style end-turn rider: a potion whose effect requested an
            // end-turn (e.g. a potion that auto-plays a card raising
            // `request_end_turn_after_play`) must advance the turn now — mirroring
            // `finish_card_play` (combat_loop.rs:6394). C# `PlayerCmd
            // .SetReadyToEndTurn` fires after the whole `UsePotionAction`, so the
            // deferred end-turn runs once the potion resolves. potion-autoplay
            // fix 1.
            if state.player.request_end_turn_after_play {
                state.player.request_end_turn_after_play = false;
                if state.phase == CombatPhase::PlayerPlayPhase && !state.is_over {
                    return combat_loop::end_turn_dispatch(state);
                }
            }
            ended
        }
        Action::Channel { orb, .. } => {
            // Bead sts2-rs-spy (P7-7). Direct channel for scripted-play
            // and replay fixtures. The card-effect path (Zap, Coolheaded,
            // ...) calls `orbs::channel_orb` directly without going
            // through `apply`.
            orbs::channel_orb(state, orb);
            StepResult::Advanced
        }
        Action::Evoke { times } => {
            orbs::evoke_front_n(state, times);
            StepResult::Advanced
        }
        Action::ResolveChoice { selection } => {
            // The top-level `apply` path resolves the choice AND flushes any
            // played card that deferred its Play→Discard move while the choice
            // was open (the normal single-card `play_card` deferral). The
            // shared body lives in `resolve_choice_action`; the auto-play
            // inline resolver (`combat_loop::resolve_inline_auto_play_choices`,
            // bead `sts2-rs-y1fic`) calls it with `flush_played = false`
            // because multiple collected sibling cards sit on the Play pile at
            // once and must NOT be swept by a mid-loop choice resolution.
            resolve_choice_action(state, &selection, /*flush_played=*/ true)
        }
    }
}

/// Resolve the open [`CombatState::pending_choice`] with `selection` ATOMICALLY,
/// BYPASSING the sequential multi-pick accumulation in
/// [`resolve_choice_action`]. Bead `sts2-rs-sopgf`.
///
/// The C#-oracle parity replay and the headless take-first driver supply the
/// COMPLETE multi-select index set in one shot (e.g. Dredge's `[1, 2, 4]`), so
/// they must resolve in a single step — never via the per-pick re-open loop
/// MCTS uses. Routing them here keeps the per-`ChoiceKind` resolver receiving
/// the identical full selection it always has, so the resulting state stays
/// byte-identical to the pre-`sopgf` behavior (the parity invariant). `flush_played`
/// matches [`apply`]'s top-level `Action::ResolveChoice` path (the normal
/// `play_card` deferral flush).
pub fn resolve_choice_atomic_apply(state: &mut CombatState, selection: &[u8]) -> StepResult {
    let Some(pc) = state.pending_choice.take() else {
        return StepResult::Advanced;
    };
    resolve_choice_atomic(state, pc, selection, /*flush_played=*/ true)
}

/// Resolve the currently-open [`CombatState::pending_choice`] with `selection`,
/// running the per-[`ChoiceKind`] effect, the suspended-draw resume, and the
/// deferred-turn-setup resume. When `flush_played` is `true` it also flushes
/// any played card still sitting on the Play pile (the normal `play_card`
/// deferral). Auto-play inline resolution passes `flush_played = false` so it
/// does not sweep sibling collected cards (bead `sts2-rs-y1fic`).
///
/// Phase 6 (sts2-rs-419n): the `.take()` simultaneously consumes the pending
/// choice — so *every* path clears `pending_choice`, which subsumes the prior
/// `sts2-rs-v2oh` hotfix (a bare `state.pending_choice = None`) that masked
/// silent resolution failures by clearing-without-effecting. Per-variant
/// handlers below run the chosen effect.
///
/// Bead `sts2-rs-sopgf`: for a SEQUENTIAL multi-pick (`max_select > 1`) this
/// function ACCUMULATES single-index picks into `pc.picked` and re-opens the
/// choice without firing any effect; the per-`ChoiceKind` resolver runs ONCE,
/// at FINALIZE (empty `selection`, or the incremental pick that reaches
/// `max_select`), over the full accumulated set. Three cases for an open
/// multi-pick:
///   * `selection == []` → FINALIZE with the accumulated `pc.picked` (possibly
///     empty = a true skip).
///   * `selection == [i]` with `picked.len() + 1 < max_select` → INCREMENTAL:
///     append `i`, re-open the SAME choice, fire NO effect.
///   * `selection == [i]` that reaches `max_select` → append, then FINALIZE.
///   * `selection.len() >= 2` (a full set passed directly via
///     `apply(Action::ResolveChoice)`) → treated as ATOMIC for back-compat.
/// Single-pick (`max_select == 1`) and empty-options choices keep their exact
/// prior net behavior (`[i]` finalizes immediately with `[i]`). The actual
/// dispatch + resume tail lives in [`resolve_choice_atomic`]; the parity
/// invariant is that the kind-resolver always sees the IDENTICAL full index
/// set, so the resulting state is byte-identical to a one-shot resolve.
/// Re-enter the player's end-turn dispatch chain at the point a listener
/// auto-play's surfaced card-select suspended it. Consumes
/// [`CombatState::pending_end_turn_resume`]; returns `None` when nothing is
/// owed. Bead `sts2-rs-eirvo`.
fn resume_suspended_end_turn(state: &mut CombatState) -> Option<StepResult> {
    match state.pending_end_turn_resume.take()? {
        EndTurnResume::AutoPostPlay { stampede_remaining } => Some(
            crate::combat_loop::resume_end_turn_after_auto_post_play(state, stampede_remaining),
        ),
        EndTurnResume::AfterBeforeTurnEnd => Some(
            crate::combat_loop::resume_end_turn_after_before_turn_end(state),
        ),
    }
}

pub(crate) fn resolve_choice_action(
    state: &mut CombatState,
    selection: &[u8],
    flush_played: bool,
) -> StepResult {
    let Some(mut pc) = state.pending_choice.take() else {
        // No open choice: legal_actions wouldn't emit this, but
        // a malformed scripted input could. Treat as a no-op so
        // the engine remains crash-free.
        return StepResult::Advanced;
    };
    // Bead `sts2-rs-sopgf`: SEQUENTIAL multi-pick accumulation. Only a
    // single-index incremental pick on a `max_select > 1` choice re-opens; a
    // full set (len >= 2) is atomic for back-compat, and the single-pick /
    // empty-options paths fall straight through to the atomic dispatch below.
    if pc.max_select > 1 && selection.len() == 1 {
        let i = selection[0];
        let max_n = usize::from(pc.max_select);
        // Ignore an already-picked or out-of-range index defensively (the
        // legal-action surface never emits these, but a scripted input could).
        if usize::from(i) < pc.options.len() && !pc.picked.contains(&i) {
            pc.picked.push(i);
        }
        if pc.picked.len() < max_n {
            // Not yet at the cap: re-open the same choice (no effect fires)
            // and let the next pick / FINALIZE continue accumulation.
            state.pending_choice = Some(pc);
            return StepResult::Advanced;
        }
        // Reached `max_select`: FINALIZE with the full accumulated set.
        let full: SmallVec<[u8; 4]> = pc.picked.clone();
        return resolve_choice_atomic(state, pc, &full, flush_played);
    }
    if pc.max_select > 1 && selection.is_empty() {
        // FINALIZE a multi-pick: run the kind-resolver ONCE over everything
        // accumulated so far (an empty `picked` is a true skip).
        let full: SmallVec<[u8; 4]> = pc.picked.clone();
        return resolve_choice_atomic(state, pc, &full, flush_played);
    }
    // Single-pick, empty-options, or a directly-supplied full set (len >= 2):
    // resolve atomically with the given selection — unchanged net behavior.
    let owned: SmallVec<[u8; 4]> = selection.iter().copied().collect();
    resolve_choice_atomic(state, pc, &owned, flush_played)
}

/// Atomic per-[`ChoiceKind`] resolution of an ALREADY-TAKEN [`PendingChoice`].
///
/// This is the ONE place the kind-resolvers run for a choice. It receives the
/// full `selection` (skip = empty, single = `[i]`, multi = `[i, j, …]`) and
/// runs the per-kind effect followed by the suspended-draw / deferred-turn-
/// setup / played-card-flush resume tail, exactly as before bead
/// `sts2-rs-sopgf` split it out of `resolve_choice_action`.
///
/// **Parity invariant (bead `sts2-rs-sopgf`).** The sequential multi-pick
/// accumulation in [`resolve_choice_action`] and the replay/oracle drivers
/// (`RunManager::resolve_combat_choice_selection`, `CombatResolveChoiceTakeFirst`,
/// and the `apply(Action::ResolveChoice)` len >= 2 back-compat path) BOTH funnel
/// here with the IDENTICAL full index set the C# multi-select produced.
/// Intermediate re-opens fire no effects, so the end state is byte-identical to
/// a single atomic resolve — which is what keeps C#-vs-Rust parity intact.
fn resolve_choice_atomic(
    state: &mut CombatState,
    pc: PendingChoice,
    selection: &[u8],
    flush_played: bool,
) -> StepResult {
    match pc.kind {
        ChoiceKind::PutBack => resolve_putback_choice(state, &pc, selection),
                ChoiceKind::AddGeneratedToHand => {
                    resolve_add_generated_choice(state, &pc, selection, false, false);
                }
                ChoiceKind::AddGeneratedRetainedToHand => {
                    resolve_add_generated_choice(state, &pc, selection, true, false);
                }
                ChoiceKind::AddGeneratedFreeToHand => {
                    resolve_add_generated_choice(state, &pc, selection, false, true);
                }
                ChoiceKind::UpgradeInHand => resolve_upgrade_in_hand(state, &pc, selection),
                ChoiceKind::AddFromDrawByType => {
                    resolve_add_from_draw_by_type(state, &pc, selection);
                }
                ChoiceKind::AddFromDiscardFree => {
                    resolve_add_from_discard_free(state, &pc, selection);
                }
                ChoiceKind::AddFromDiscardToHand => {
                    resolve_add_from_discard_to_hand(state, &pc, selection);
                }
                ChoiceKind::ApplyRetainInHand => {
                    resolve_apply_retain_in_hand(state, &pc, selection);
                }
                ChoiceKind::ApplySlyInHand => {
                    resolve_apply_sly_in_hand(state, &pc, selection);
                }
                ChoiceKind::ApplyEtherealInHand => {
                    resolve_apply_ethereal_in_hand(state, &pc, selection);
                }
                ChoiceKind::TransformInHandToMinionStrike => {
                    resolve_transform_in_hand_to_minion_strike(state, &pc, selection);
                }
                ChoiceKind::TransformInHandToMinionSacrifice => {
                    resolve_transform_in_hand_to_minion_sacrifice(state, &pc, selection);
                }
                ChoiceKind::TransfigureCardInHand => {
                    resolve_transfigure_card_in_hand(state, &pc, selection);
                }
                ChoiceKind::TransformDrawCardToSoul => {
                    resolve_transform_draw_card_to_soul(state, &pc, selection);
                }
                ChoiceKind::TransformDrawCardsToMinionDiveBomb => {
                    resolve_transform_draw_cards_to_minion_dive_bomb(state, &pc, selection);
                }
                ChoiceKind::CloneChosenCardNextTurn => {
                    resolve_clone_chosen_card_next_turn(state, &pc, selection);
                }
                ChoiceKind::DiscardFromHand => {
                    resolve_discard_from_hand(state, &pc, selection);
                }
                ChoiceKind::ExhaustFromHand => {
                    resolve_exhaust_from_hand(state, &pc, selection);
                }
                ChoiceKind::DiscardFromHandAndDraw => {
                    resolve_discard_from_hand_and_draw(state, &pc, selection);
                }
                ChoiceKind::MakeFreeInHand => {
                    resolve_make_free_in_hand(state, &pc, selection);
                }
                ChoiceKind::MonsterCurseSelect => {
                    resolve_monster_curse_select(state, &pc, selection);
                }
                ChoiceKind::AddFromDrawToHand => {
                    resolve_add_from_draw_to_hand(state, &pc, selection);
                }
                ChoiceKind::ForegoneConclusionFromDraw => {
                    resolve_foregone_conclusion_from_draw(state, &pc, selection);
                }
                ChoiceKind::RetainAtTurnEnd => {
                    resolve_retain_at_turn_end(state, &pc, selection);
                }
                ChoiceKind::AutoPlaySkillFromHand => {
                    resolve_auto_play_skill_from_hand(state, &pc, selection);
                }
                ChoiceKind::CardSelect
                | ChoiceKind::ExhaustPile
                | ChoiceKind::OrbSlot
                | ChoiceKind::EnemyTarget
                // `PaelsToothStash` is a run-side choice resolved
                // via `RunState::resolve_paels_tooth_stash`; if it
                // ever surfaces in the combat-side resolver it's a
                // misroute, so the no-op arm here matches the
                // same swallow-and-continue policy as the other
                // unimplemented variants (bead `sts2-rs-n7tha`).
                | ChoiceKind::PaelsToothStash => {
                    // Sub-beads will land per-kind dispatch (Forethought,
                    // Armaments, Exhume, Discovery, ...). Keeping the
                    // pending_choice cleared (already done by `.take()`)
                    // so the rollout doesn't loop, but logging would be
                    // ideal — the assertion below is a hard `unimplemented!`
                    // when invoked from production code paths. Disabled in
                    // favor of a no-op so existing tests using the default
                    // `CardSelect` placeholder still terminate; the bead
                    // body documents that real handlers replace this arm.
                }
            }
    // Bead `sts2-rs-wlp-retain-select`: resume the end-of-turn flush
    // suspended by a Well-Laid Plans retain select. The chosen cards have
    // just been stamped into `retain_this_turn` (the resolved choice); now
    // run the flush tail (rebuild the retain set, flush the rest of the
    // hand, per-turn cleanup, `after_turn_end` hooks, extra-turn check, phase
    // advance) AND the remainder of the `end_turn_dispatch` chain (enemy
    // turns + next player turn-start) so the whole `Action::EndTurn` settles
    // back to the resting `PlayerPlayPhase`. Ordered FIRST among the resume
    // blocks: it is an end-turn-flow continuation, mutually exclusive with the
    // turn-setup resume markers below (those only arm during
    // `start_player_turn`). Only fires when the retain resolution didn't
    // re-open a nested choice (a chained select keeps the continuation parked
    // and re-surfaces).
    // Resume the enemy turn suspended by a monster move's BLOCKING player card
    // screen (KnowledgeDemon CURSE_OF_KNOWLEDGE — C# awaits
    // `CardSelectCmd.FromChooseACardScreen` inside `CurseOfKnowledge`). The
    // per-kind resolver above just applied the chosen curse power; now finish
    // the parked perform walk, the enemy turn-end pipeline and the next
    // player turn-start, settling the original `Action::EndTurn` back to the
    // resting `PlayerPlayPhase`. Ordered FIRST among the resume blocks: it is
    // an end-turn-flow continuation and is mutually exclusive with every marker
    // below (those only arm during the player's own turn). Only fires when the
    // curse resolution didn't re-open a nested choice.
    if state.pending_enemy_exec_resume.is_some() && state.pending_choice.is_none() {
        return crate::combat_loop::resume_enemy_turn_exec(state);
    }
    if state.pending_wlp_retain_resume && state.pending_choice.is_none() {
        state.pending_wlp_retain_resume = false;
        return crate::combat_loop::resume_wlp_retain_flush(state);
    }
    // Bead `sts2-rs-fc-handdraw-suspend`: resume the turn-start hand draw
    // suspended by a `ForegoneConclusionPower.BeforeHandDraw` surfacing
    // card-select. The FC cards have just been moved to hand + the power
    // stripped (the resolved choice); now run the remaining turn-setup tail
    // (relic before_hand_draw, Bolas/ThrummingHatchet returns, Howl/Bombardment
    // auto-play, the natural draw, `finish_player_turn_setup`)
    // with the FC cards already in hand — matching C#'s awaited ordering (FC 2
    // cards → hand, THEN draw 5 → hand = 7). Mutually exclusive with the
    // turn-setup resume markers below: this marker only arms during
    // `continue_player_turn_setup`, and the tail it re-enters is what later sets
    // `pending_draw_resume` (so that is never set
    // while this is). Only fires when the FC resolution didn't re-open a nested
    // choice (a chained select keeps the continuation parked and re-surfaces).
    if state.pending_choice.is_none() {
        if let Some(draw_count) = state.pending_hand_draw_resume.take() {
            return crate::combat_loop::resume_after_foregone_select(state, draw_count);
        }
        // GH #369: the same suspension one stage later — a relic's
        // `before_hand_draw` select (Toolbox). The chosen card has just been
        // added to hand by the resolver; run the remaining tail
        // (Bolas/ThrummingHatchet returns, Howl/Bombardment auto-play,
        // DrumOfBattle-late, the natural draw, `finish_player_turn_setup`)
        // with it already there, matching C#'s awaited ordering. Re-enters
        // AFTER the relic channel so it does not re-fire.
        if let Some(draw_count) = state.pending_relic_hand_draw_resume.take() {
            return crate::combat_loop::resume_after_relic_hand_draw_select(state, draw_count);
        }
    }
    // Bead `sts2-rs-63mew`: resume a draw suspended mid-reshuffle
    // by an `after_shuffle` choice (Stratagem). C# awaits the
    // grid-select INSIDE `CardPileCmd.Draw`'s per-card loop, so
    // the not-yet-drawn remainder resolves here, BEFORE any
    // deferred turn-setup tail or played-card flush. If the
    // resumed draw suspends again (a second reshuffle — requires
    // something to refill the discard pile mid-draw), the fresh
    // marker inherits the turn-setup obligation.
    // Bead `sts2-rs-hellraiser-draw-suspend`: when a parked auto-play tail
    // (`pending_autoplay_resume`) coexists with a suspended draw, the auto-play
    // MUST finish first (its card → discard) before the draw continues — the
    // `pending_autoplay_resume` arm below owns that ordering and resumes the
    // draw itself. So skip this block while an auto-play is parked. The lone-
    // draw-resume case (Stratagem `after_shuffle`, no auto-play) keeps running
    // here unchanged.
    if state.pending_choice.is_none() && state.pending_autoplay_resume.is_none() {
        if let Some(pdr) = state.pending_draw_resume.take() {
            if let Some(r) = state.resume_suspended_draw(pdr) {
                return r;
            }
        }
    }
    // Bead `sts2-rs-o0rz3`: resume the turn-start `AfterPlayerTurnStart` cluster
    // suspended by a `StratagemPower.AfterShuffle` grid-select during a Mayhem
    // auto-play. Finishes the suspended Mayhem auto-play and continues the
    // snapshot walk; `after_side_turn_start` already fired at the pause, so it
    // is not re-fired. Only when the resolution didn't re-open a fresh choice.
    // Bead `sts2-rs-2itwm`: skipped while an auto-play is parked, for the same
    // reason as the suspended-draw arm above — a Mayhem batch that suspended
    // because a collected card's `OnPlay` surfaced a select owes that card's
    // tail (and its un-played siblings) BEFORE the cluster's turn-start tail.
    // The `pending_autoplay_resume` arm below re-enters the cluster itself once
    // it has drained both.
    //
    // GH #500: gated on `flush_played` for the same reason the `pending_play`
    // arm below is — it is the top-level-vs-nested discriminator. The cluster's
    // turn-start tail is an OUTER continuation: it may only run once the
    // in-flight auto-play that suspended the cluster has fully finished. A card
    // played from the resume's `remaining` (a Mayhem sibling) drains its own
    // `OnPlay` select INLINE, which re-enters here with `flush_played = false`
    // while its `pending_autoplay_resume` is already taken — and this arm then
    // ran the whole turn-start tail from inside that card's `OnPlay`. The
    // HistoryCourse dupe therefore resolved BEFORE the sibling's
    // `AfterCardPlayed`, so a Slow-scaled dupe attack read one card-play too few
    // (Ironclad seed 29: BygoneEffigy hp cs=15 rs=16 — the dupe's SeekerStrike
    // hit at SlowAmount 1 instead of 2, 12x1.1 = 13 vs 12x1.2 = 14). Leaving the
    // marker parked hands it to the arm at the end of the `pending_autoplay_resume`
    // block, which is where C#'s ordering puts it.
    if flush_played && state.pending_choice.is_none() && state.pending_autoplay_resume.is_none() {
        if let Some(r) = state.pending_cluster_resume.take() {
            return crate::combat_loop::resume_player_turn_start_cluster(state, r);
        }
    }
    // Bead `sts2-rs-l2nz`: resume deferred turn-setup if the
    // resolved choice was the one that paused
    // `start_player_turn` (mirrors Python's
    // `_pending_turn_setup` invocation at
    // `combat.py:1672-1675`). Only fires when no nested
    // listener re-opened a fresh `pending_choice` during
    // resolution.
    if state.pending_turn_setup_resume && state.pending_choice.is_none() {
        state.pending_turn_setup_resume = false;
        return crate::combat_loop::continue_player_turn_setup(state);
    }
    // Deferred `Hook.AfterPotionUsed` (C# `PotionModel.OnUseWrapper`,
    // PotionModel.cs:291-342): the potion whose `OnUse` surfaced this choice
    // finishes NOW — the awaited select has resolved — so its AfterPotionUsed
    // listeners (`ReptileTrinket`'s +3 temporary Strength, `BeltBuckle`) fire
    // here, seeing the post-choice state exactly as C# does (`:338`, after
    // `await OnUse` at `:327`). Ordered AFTER the suspended-draw resume above
    // — a draw continuation (e.g. a GamblersBrew redraw suspended by a
    // Stratagem reshuffle select) is still part of the potion's awaited
    // `OnUse`, so on a resolve that completes such a draw (the resume falls
    // through with `None`) the hook fires after it, exactly like C#. Only
    // fires when the resolution didn't re-open a nested choice and didn't
    // leave a draw / auto-play continuation armed; a continuation that ends
    // this resolve via an early return keeps the dispatch parked for the next
    // one — the hook can at worst fire late (never early), and the marker
    // dies with the combat state. The turn-flow resumes above are mutually
    // exclusive with a potion drink (only drinkable from the resting
    // `PlayerPlayPhase`).
    if state.pending_choice.is_none()
        && state.pending_draw_resume.is_none()
        && state.pending_autoplay_resume.is_none()
    {
        if let Some(resume) = state.pending_potion_hook_resume.take() {
            let _ =
                crate::relic_effects::dispatch_after_potion_used(state, resume.kind, resume.target);
            // Mirror the `Action::DrinkPotion` arm's post-hook combat-end
            // check (no shipped AfterPotionUsed listener is lethal today,
            // but the arm runs it and staying in step is cheap).
            if !state.enemy_uids.is_empty() {
                let ended = crate::combat_loop::check_combat_end(state);
                if !matches!(ended, StepResult::Advanced) {
                    return ended;
                }
            }
        }
    }
    // Bead `sts2-rs-8ukqd` (bug D): flush any played card whose
    // OnPlay deferred its Play→Discard move because it opened
    // this choice (see `combat_loop::play_card` cleanup). C#
    // holds the played card out of all piles until the awaited
    // choice completes, then moves it to discard — we mirror
    // that final flush here. Only fires when the resolution
    // didn't re-open a nested `pending_choice`.
    //
    // Skipped on the auto-play inline path (`flush_played = false`):
    // there, several collected sibling cards sit on the Play pile at
    // once and the per-card auto-play resolver routes each itself, so
    // flushing the whole Play pile here would sweep the not-yet-played
    // siblings into discard (bead `sts2-rs-y1fic`).
    // Bead `sts2-rs-mcts-surface-onplay-select`: re-enter the deferred play
    // tail for a top-level card whose OnPlay select we surfaced (rather than
    // drained). The per-kind resolver above just applied the pick (the discard
    // + its `AfterCardDiscarded` reaction), so running `finish_card_play` now
    // fires `AfterCardPlayed` AFTER it — the exact order C#'s `OnPlayWrapper`
    // produces by awaiting the selector inside `OnPlay`. `finish_card_play`
    // performs its own Play→Discard/Exhaust disposal, so it supersedes (and we
    // skip) the `flush_played_cards_after_choice` fallback below. Only when the
    // resolution didn't re-open a nested choice (a chained select keeps the
    // continuation parked and re-surfaces). The auto-play inline drive
    // (`flush_played = false`) never sets `pending_play_finish`, so this is
    // inert there.
    // Parity bug `combat_select_ironclad`: re-enter an auto-play (Havoc /
    // Cascade / Catastrophe / Mayhem / DistilledChaos / Stampede) suspended
    // because the card it dispatched opened a SURFACING card-select from its
    // `OnPlay`. The per-kind resolver above just applied the surfaced pick (the
    // upgrade / exhaust / add-to-hand); now finish that card's post-dispatch
    // tail (`AfterCardPlayed` buses, replay loop, Play→Discard/Exhaust
    // disposal) and then play any remaining collected cards — exactly the order
    // C#'s `OnPlayWrapper` produces by awaiting the selector inside `OnPlay`
    // (`CardModel.cs:1498`), then continuing the auto-play loop. `autoplay_inline_resolve`
    // is raised for the whole resume so any FURTHER nested choice (a chained
    // auto-play's follow-on screen) drains inline at the option-0 prefix —
    // mirroring the headless `AutoResolveInline` set during `DoSelectCards`
    // (`RunSimulator.cs:2165`), which preserves the `y1fic` chained-auto-play
    // deadlock fix. Ordered BEFORE the generic `pending_play` block.
    //
    // The two CAN both be set at once: a HAND-played auto-play card (e.g. Havoc
    // via `play_card`) whose nested auto-play surfaced sets BOTH
    // `pending_autoplay_resume` (the nested Armaments tail) AND `pending_play`
    // (Havoc's own tail, parked by `play_card`). C# resumes the whole stack in
    // one `DoSelectCards` call with `AutoResolveInline` held true throughout —
    // the nested auto-play finishes, then the parent play's tail runs, all
    // inline. We mirror that: finish the auto-play resume, then (still under the
    // raised flag) finish the parent `pending_play` if present, then restore the
    // flag. Only when the surfaced pick didn't re-open a nested choice (a
    // chained select keeps the resume parked and re-surfaces).
    if state.pending_choice.is_none() {
        if let Some(resume) = state.pending_autoplay_resume.take() {
            let crate::PendingAutoplayResume {
                finish,
                remaining,
                force_exhaust,
                continuation,
                parent_chain,
            } = resume;
            let prev_inline = state.autoplay_inline_resolve;
            state.autoplay_inline_resolve = true;
            crate::combat_loop::finish_auto_played_card(state, finish);
            // Play the remaining collected cards (Cascade-for-N where the
            // surfacing card was not the last). Each runs with inline-resolve on
            // so its own choices drain rather than re-surface.
            for uid in remaining {
                if state.is_over {
                    break;
                }
                crate::combat_loop::resolve_auto_played_card(state, uid, force_exhaust);
            }
            // Nested auto-play chain (seed 2752 DISTILLED_CHAOS → HAVOC →
            // BURNING_PACT): now that the INNER surfacing card's tail has run,
            // unwind each enclosing auto-play in inner→outer order — finish its
            // own post-dispatch tail, then play any collected siblings it still
            // owed. This mirrors C#'s nested `await CardCmd.AutoPlay` returning
            // up the stack inside one `DoSelectCards`, all under the raised
            // inline flag so no further select re-surfaces. Runs BEFORE the
            // re-selection `continuation` (the outermost loop) and the parent
            // hand-play tail, matching the C# resume order. Bead `sts2-rs`
            // (#181), follow-up to `sts2-rs-cata-suspend`.
            for frame in parent_chain {
                if state.is_over {
                    break;
                }
                // GH #500: this frame's own auto-play LOOP first. C#
                // `Catastrophe.OnPlay` awaits every `CardCmd.AutoPlay` in its
                // `for` loop, so the loop drains completely before `OnPlay`
                // returns — i.e. before `OnPlayWrapper` fires `AfterCardPlayed`
                // (`frame.finish`) and before the enclosing batch moves on to
                // the siblings it attached as `frame.remaining`.
                if let Some(cont) = frame.continuation {
                    (cont.resume_fn)(state, cont.iters, &cont.uids, cont.force_exhaust);
                    if state.is_over {
                        break;
                    }
                }
                crate::combat_loop::finish_auto_played_card(state, frame.finish);
                for uid in frame.remaining {
                    if state.is_over {
                        break;
                    }
                    crate::combat_loop::resolve_auto_played_card(state, uid, frame.force_exhaust);
                }
            }
            // Re-selection auto-play-loop continuation (Catastrophe / BeatDown):
            // now that the surfaced card's tail has run (the fetched / relocated
            // card has left the pile), replay the loop's not-yet-run iterations
            // against the UPDATED pile — mirroring C# `await CardCmd.AutoPlay`
            // resuming the `OnPlay` loop. Still under the raised inline flag, so
            // any further nested select in those iterations drains inline rather
            // than re-surfacing (the loop runs to completion here). Bead
            // `sts2-rs-cata-suspend`.
            if let Some(cont) = continuation {
                if !state.is_over {
                    (cont.resume_fn)(state, cont.iters, &cont.uids, cont.force_exhaust);
                }
            }
            // Restore inline-resolve BEFORE the draw continuation below so a
            // further on-draw auto-play in the continued draw surfaces its own
            // select (rather than draining inline). The parent hand-play tail
            // (if any) likewise finishes under the restored flag, matching C#.
            state.autoplay_inline_resolve = prev_inline;
            // Bead `sts2-rs-hellraiser-draw-suspend` (DRAW_ORDER / PILE_COUNT
            // seeds 1586/3453/4081/5754): the auto-play just finished
            // was an on-draw HellraiserPower play whose surfaced select had
            // suspended a draw (`pending_draw_resume`). Now that the auto-played
            // card has reached its result pile (discard) above, CONTINUE the draw
            // — exactly C#'s `await CardCmd.AutoPlay` returning into
            // `CardPileCmd.Draw`'s per-card loop, which then draws the next card
            // against the updated pile. Running this AFTER the auto-play finish
            // (not before, as a lone `pending_draw_resume` would) is the whole
            // point: a mid-draw reshuffle now sees the discard pile at full size.
            // It MUST also run BEFORE the parent hand-play tail below: when the
            // suspended draw is a card's OnPlay draw (Scrawl drawing until the
            // hand is full, seed 5754), C# completes the whole awaited draw — all
            // its further Hellraiser auto-plays + reshuffles — before that card's
            // OnPlay returns and it moves to discard. If the draw re-suspends on
            // a later select, return and keep the parent parked for the next
            // resolve.
            if state.pending_choice.is_none() {
                if let Some(pdr) = state.pending_draw_resume.take() {
                    if let Some(r) = state.resume_suspended_draw(pdr) {
                        return r;
                    }
                }
            }
            // Finish the parent hand-play's tail (Havoc/Cascade/Scrawl/... played
            // from hand) if `play_card` parked it on the same surfaced choice,
            // matching C#'s single resume call.
            if state.pending_choice.is_none() {
                if let Some(pp) = state.pending_play.take() {
                    return crate::combat_loop::finish_card_play(state, pp.finish);
                }
            }
            // Bead `sts2-rs-2itwm`: the auto-play just finished was a card from
            // a turn-start Mayhem `AutoPlayFromDrawPile` batch, whose surfaced
            // select suspended the `AfterPlayerTurnStart` cluster walk. Its
            // siblings have just replayed above; run the cluster's turn-start
            // tail now — unless finishing them re-opened a nested choice, in
            // which case the marker keeps for the next resolve.
            if state.pending_choice.is_none() {
                if let Some(r) = state.pending_cluster_resume.take() {
                    return crate::combat_loop::resume_player_turn_start_cluster(state, r);
                }
            }
            // Parity bug `combat_select_ironclad` (Ironclad seed 3859): the
            // auto-play just finished was a StampedePower end-of-turn auto-play
            // whose surfaced select suspended the end-turn dispatch chain. Now
            // that the card has reached its result pile, resume the chain from
            // wherever it stopped — unless finishing the auto-play re-opened a
            // nested choice, in which case keep the marker for the next resolve.
            if state.pending_choice.is_none() {
                if let Some(r) = resume_suspended_end_turn(state) {
                    return r;
                }
            }
            return StepResult::Advanced;
        }
    }
    // Parity bug `combat_select_ironclad`: resume the end-turn dispatch chain
    // suspended by a `before_turn_end` auto-play's surfaced select when the
    // parked auto-play tail was already drained on an earlier resolve (a chain
    // of nested selects), so `pending_autoplay_resume` is gone but the chain
    // still owes its remaining steps. Mutually exclusive with the
    // `pending_autoplay_resume` arm above (which resumes inline when it owns the
    // finish). Only fires once the surfaced pick didn't re-open a nested choice.
    if state.pending_choice.is_none() && state.pending_autoplay_resume.is_none() {
        if let Some(r) = resume_suspended_end_turn(state) {
            return r;
        }
    }
    // GH #500: gated on `flush_played`, i.e. only the TOP-LEVEL resolve drives
    // the parked hand-play tail. `flush_played == false` marks a NESTED
    // resolution running inside an in-flight play — `resolve_inline_auto_play_choices`
    // draining an auto-played card's own select, or `open_hand_choice`
    // auto-resolving a forced one. During a RESUME the outer
    // `pending_autoplay_resume` arm above re-enters an auto-play loop
    // (Catastrophe's `continuation`) while the hand-played card's tail is still
    // parked in `pending_play`; a select opened by that continuation drained here
    // would finish the OUTER card mid-flight, routing it to discard BEFORE the
    // auto-play it is still running (C# `OnPlayWrapper` moves the card to its
    // result pile only after `OnPlay` returns, `CardModel.cs:1538-1552`). Seed 55
    // of the auto-play/replay-heavy Ironclad oracle campaign: CATASTROPHE
    // auto-plays HEADBUTT (surfacing put-back select) then HAVOC; RS discarded
    // `[… HEADBUTT, CATASTROPHE, HAVOC]` where C# has `[… HEADBUTT, HAVOC,
    // CATASTROPHE]`. Same pile CONTENTS, so the count-based divergence check
    // stayed clean until the next reshuffle StableShuffle read the differently
    // ordered discard pile and dealt a different hand. The parked tail is picked
    // up by the `pending_autoplay_resume` arm above (after the continuation) or
    // by the next top-level resolve.
    if flush_played && state.pending_choice.is_none() {
        if let Some(pp) = state.pending_play.take() {
            return crate::combat_loop::finish_card_play(state, pp.finish);
        }
    }
    // Bead `sts2-rs-8ukqd` (bug D): flush any played card whose
    // OnPlay deferred its Play→Discard move because it opened
    // this choice (see `combat_loop::play_card` cleanup). C#
    // holds the played card out of all piles until the awaited
    // choice completes, then moves it to discard — we mirror
    // that final flush here. Only fires when the resolution
    // didn't re-open a nested `pending_choice`.
    if flush_played && state.pending_choice.is_none() {
        crate::combat_loop::flush_played_cards_after_choice(state);
    }
    StepResult::Advanced
}

/// Resolve a [`ChoiceKind::PutBack`] pending choice (Headbutt-style:
/// "pick a card from a source pile and put it on top of the draw pile").
///
/// Selection is a length-0 or length-1 slice of indices into
/// `pc.options` (length 0 = no-op skip when allowed). The chosen
/// `ChoiceOption` must carry a `card_uid` that currently lives in the
/// declared source pile; if either invariant is violated the call is
/// a defensive no-op (combat-state remains valid; pending_choice has
/// already been cleared by the caller's `.take()`).
///
/// Mirrors the `resolver=lambda selected: combat.insert_card_into_draw_pile(selected, random_position=False)`
/// closure in `sts2-rl-agent@01c094d:sts2_env/cards/ironclad.py:369` —
/// the chosen card is removed from its source pile and inserted at
/// index 0 of `draw` (top-of-pile / next-drawn).
/// First selected option of a pending choice — the shared prelude of the
/// single-select `resolve_*` handlers below. `None` for an empty or
/// out-of-range selection; every resolver treats a malformed selection as
/// a defensive no-op rather than a panic.
fn selected_option<'a>(pc: &'a PendingChoice, selection: &[u8]) -> Option<&'a ChoiceOption> {
    pc.options.get(usize::from(*selection.first()?))
}

/// `card_uid` of the first selected option (see [`selected_option`]).
/// `None` when the selection is malformed or the chosen option carries no
/// store-backed card instance.
fn selected_card_uid(pc: &PendingChoice, selection: &[u8]) -> Option<CardUid> {
    selected_option(pc, selection)?.card_uid
}

/// Resolve a [`ChoiceKind::AddGeneratedToHand`] or
/// [`ChoiceKind::AddGeneratedRetainedToHand`] pending choice. Builds a
/// fresh `Card` POD from the chosen option's `(card_id, upgraded)` via
/// the registered card factory, then routes it through
/// [`crate::card_effects::spawn_card_into_hand`] (overflow → discard).
/// When `retained` is true, the new uid is also pushed into
/// `state.player.retain_this_turn` so the end-of-turn flush keeps it
/// in hand for the current turn (ChoicesParadox semantics — pin
/// `01c094d` Python uses the persistent `retain` keyword, the Rust
/// port approximates with the one-turn retain flag).
///
/// Selection is a length-1 slice of indices into `pc.options`. Empty
/// selection is a defensive no-op (Toolbox / ChoicesParadox both pass
/// `can_skip=false` but a malformed input shouldn't crash).
fn resolve_add_generated_choice(
    state: &mut CombatState,
    pc: &PendingChoice,
    selection: &[u8],
    retained: bool,
    free_this_turn: bool,
) {
    let Some(opt) = selected_option(pc, selection) else {
        return;
    };
    // C# `HeirloomHammer.OnPlay` does `selection.CreateClone()` — a DEEP
    // copy of the chosen instance that preserves in-combat mutations (a
    // forged SovereignBlade's raised `base_damage`, in particular). When the
    // option carries a live source `card_uid` (HeirloomHammer's
    // ≥2-colorless branch picks an existing HAND card), clone that
    // instance's POD; only fall back to a fresh spec-built card when there is
    // no source instance — Quasar / Toolbox / ChoicesParadox all generate
    // brand-new cards with `card_uid == None`. Without the clone a forged
    // blade re-spawns at its spec base damage (parity Regent seed 955:
    // cloned blade 19 vs C# 29 → enemy hp rust=22 vs cs=12).
    let card = if let Some(template) = opt
        .card_uid
        .and_then(|uid| state.cards.get(uid.0.get() as usize - 1).copied())
    {
        template
    } else {
        let Some(factory) = lookup_card_factory() else {
            return;
        };
        let Some(card) = factory(opt.card_id, opt.upgraded) else {
            return;
        };
        card
    };
    // C# always passes `addedByPlayer: true` for every effect that opens a
    // generated-card choose-a-card screen — cards (Discovery / Splash, whose
    // play pile is non-empty anyway) AND potions (SkillPotion / AttackPotion /
    // ColorlessPotion, whose `OnUse` leaves the play pile EMPTY). The play-pile
    // heuristic in plain `spawn_card_into_hand` would read `false` for the
    // potion case and silently drop the `AfterCardGeneratedForCombat` reactions
    // (ArsenalPower Strength, PillarOfCreationPower Block) — Regent fuzz seed 42:
    // a SkillPotion-generated Skill failed to grant Arsenal(1) Strength
    // (player.powers STRENGTH cs=1 rs=absent). Force `added_by_player = true`.
    // Bead `sts2-rs-potion-gen-arsenal`.
    let uid = crate::card_effects::spawn_card_into_hand_with_origin(state, card, true);
    if retained && state.player.hand.contains(&uid) {
        state.player.retain_this_turn.push(uid);
    }
    // C# `CardModel.SetToFreeThisTurn()` — stamp the FreeThisTurn combat
    // var so `effective_card_cost` charges 0 energy for the rest of the
    // turn (Attack/Skill/Power/ColorlessPotion). Bead `sts2-rs-91tc1`.
    if free_this_turn {
        let idx = uid.0.get() as usize - 1;
        if let Some(c) = state.cards.get_mut(idx) {
            c.set_combat_var(crate::cards::VarKey::FreeThisTurn, 1);
        }
    }
}

/// Resolve a [`ChoiceKind::UpgradeInHand`] pending choice (base Armaments:
/// "pick one un-upgraded card in hand and upgrade it for the rest of
/// combat"). Selection is a length-0 or length-1 slice of indices into
/// `pc.options`; the chosen option's `card_uid` is upgraded in place via
/// [`Card::upgrade`]. Empty/invalid selection is a defensive no-op (the
/// played Armaments has already been deferred onto the Play pile by
/// `play_card` and will flush to discard once this returns). Mirrors C#
/// `Armaments.OnPlay` → `CardCmd.Upgrade(cardModel)`. Bead `sts2-rs-8ukqd`.
fn resolve_upgrade_in_hand(state: &mut CombatState, pc: &PendingChoice, selection: &[u8]) {
    let Some(uid) = selected_card_uid(pc, selection) else {
        return;
    };
    // Only upgrade if the card is still in hand (defensive — a nested
    // effect could in principle have moved it).
    if !state.player.hand.contains(&uid) {
        return;
    }
    let idx = uid.0.get() as usize - 1;
    if let Some(c) = state.cards.get_mut(idx) {
        let _ = c.upgrade();
    }
}

/// Resolve a [`ChoiceKind::AddFromDrawByType`] pending choice
/// (SecretTechnique / SecretWeapon: "pick one Skill / Attack from the
/// draw pile and add it to hand"). Selection is a length-1 slice of
/// indices into `pc.options`; the chosen option's `card_uid` is removed
/// from `draw` and pushed onto `hand`. Empty/invalid selection, or a
/// uid no longer in the draw pile, is a defensive no-op. Mirrors C#
/// `SecretTechnique`/`SecretWeapon` `CardSelectCmd.FromSimpleGrid` →
/// `CardPileCmd.Move(.., Hand)`. Bead `sts2-rs-1xmxp`.
fn resolve_add_from_draw_by_type(state: &mut CombatState, pc: &PendingChoice, selection: &[u8]) {
    let Some(uid) = selected_card_uid(pc, selection) else {
        return;
    };
    if let Some(pos) = state.player.draw.iter().position(|&u| u == uid) {
        let card = state.player.draw.remove(pos);
        // Bead `sts2-rs-09x5n`: raw index removal from draw that bypasses
        // `cards::pop_next_card` (SecretTechnique/SecretWeapon/
        // DropletOfPrecognition/Stratagem/ForegoneConclusion pick-from-pile).
        // Clear any stale pin on the removed card so a later
        // `pop_next_card` doesn't short-circuit to a deterministic pop for
        // a now-possibly-heterogeneous draw and skip a chance node.
        crate::cards::unpin(&mut state.player, card);
        state.player.hand.push(card);
    }
}

/// Resolve a [`ChoiceKind::AddFromDiscardFree`] pending choice
/// (LiquidMemories: "pick one card from the discard pile, make it free
/// this turn, and add it to hand"). `selection` is a length-1 slice of
/// indices into `pc.options`; the chosen option's `card_uid` is removed
/// from `discard`, stamped `VarKey::FreeThisTurn=1`, and pushed onto
/// `hand` (subject to `HAND_LIMIT` — past the cap the card is left in
/// the discard pile, matching the same edge in
/// `route_generated_into_hand`). Empty/invalid selection, or a uid no
/// longer in the discard pile, is a defensive no-op. Mirrors C#
/// `LiquidMemories.OnUse` (LiquidMemories.cs:25-30):
/// `CardSelectCmd.FromSimpleGrid(PileType.Discard…, prefs(1))` →
/// `cardModel.SetToFreeThisTurn()` → `CardPileCmd.Add(.., Hand)` — a
/// pure player choice with ZERO RNG. Bead `sts2-rs-gpsvo`.
fn resolve_add_from_discard_free(state: &mut CombatState, pc: &PendingChoice, selection: &[u8]) {
    let Some(uid) = selected_card_uid(pc, selection) else {
        return;
    };
    let Some(pos) = state.player.discard.iter().position(|&u| u == uid) else {
        return;
    };
    let card = state.player.discard.remove(pos);
    if let Some(c) = state.cards.iter_mut().find(|c| c.uid == card) {
        c.set_combat_var(crate::cards::VarKey::FreeThisTurn, 1);
    }
    if state.player.hand.len() < crate::cards::HAND_LIMIT {
        state.player.hand.push(card);
    } else {
        state.player.discard.push(card);
    }
}

/// Resolve a [`ChoiceKind::AddFromDiscardToHand`] pending choice. Two C#
/// callers share this kind, both `CardSelectCmd.FromSimpleGrid(PileType.Discard
/// …)` → `CardPileCmd.Add(.., Hand)` with NO `SetToFreeThisTurn` (the cards
/// return at their normal cost, unlike [`resolve_add_from_discard_free`]):
///   * Graveblast (`Graveblast.cs:34`) — single-pick (`prefs(1)`).
///   * Dredge (`Dredge.cs:29`) — multi-pick (`prefs(num)`, `min = max = num`).
/// `selection` is the full list of indices into `pc.options`; every selected
/// option's `card_uid` is removed from `discard` (in selection order) and
/// pushed onto `hand`, subject to `HAND_LIMIT` (past the cap the card is left
/// in the discard pile, mirroring C# `CardPileCmd.Add`'s full-hand redirect).
/// Invalid indices, or a uid no longer in the discard pile, are skipped
/// defensively. Pure player choice with ZERO RNG.
fn resolve_add_from_discard_to_hand(state: &mut CombatState, pc: &PendingChoice, selection: &[u8]) {
    for &sel_idx in selection {
        let Some(opt) = pc.options.get(usize::from(sel_idx)) else {
            continue;
        };
        let Some(uid) = opt.card_uid else {
            continue;
        };
        let Some(pos) = state.player.discard.iter().position(|&u| u == uid) else {
            continue;
        };
        let card = state.player.discard.remove(pos);
        if state.player.hand.len() < crate::cards::HAND_LIMIT {
            state.player.hand.push(card);
        } else {
            state.player.discard.push(card);
        }
    }
}

/// Resolve a [`ChoiceKind::ApplyRetainInHand`] pending choice (Snap: "pick a
/// non-Retain hand card and give it Retain"). `selection` is a length-1 slice
/// of indices into `pc.options`; the chosen option's `card_uid` gets its
/// per-instance `keyword_override` unioned with `RETAIN` (mirroring
/// `card_effects::apply_ethereal_keyword` / the prior Snap auto-pick). Empty/
/// invalid selection, or a uid with no backing card, is a defensive no-op.
/// Pure player choice with ZERO RNG. Mirrors C# `CardCmd.ApplyKeyword(card,
/// Retain)` (Snap.cs:42).
fn resolve_apply_retain_in_hand(state: &mut CombatState, pc: &PendingChoice, selection: &[u8]) {
    let Some(uid) = selected_card_uid(pc, selection) else {
        return;
    };
    let idx = uid.0.get() as usize - 1;
    if let Some(card) = state.cards.get_mut(idx) {
        crate::card_effects::apply_retain_keyword(card);
    }
}

/// Resolve a [`ChoiceKind::RetainAtTurnEnd`] pending choice (Well-Laid Plans'
/// end-of-turn retain). `selection` is a length-0..=`Amount` slice indexing
/// `pc.options`; each chosen option's `card_uid` is stamped into
/// `player.retain_this_turn` so the suspended flush keeps it in hand THIS turn
/// only (the per-turn set is cleared after the flush in
/// [`crate::combat_loop::end_player_turn_phase_two_flush_tail`]). This is the
/// Rust analog of C# `CardModel.GiveSingleTurnRetain()`
/// (`WellLaidPlansPower.BeforeFlushLate`, WellLaidPlansPower.cs:30-33). An empty
/// selection (the player retained nothing — `min_select == 0`) is a no-op,
/// matching C#'s empty `FromHand` result. ZERO RNG. Bead
/// `sts2-rs-wlp-retain-select`.
fn resolve_retain_at_turn_end(state: &mut CombatState, pc: &PendingChoice, selection: &[u8]) {
    for &sel_idx in selection {
        let Some(opt) = pc.options.get(usize::from(sel_idx)) else {
            continue;
        };
        let Some(uid) = opt.card_uid else {
            continue;
        };
        // Defensive: only retain a card that is still in hand and not already
        // flagged. C# `GiveSingleTurnRetain` is idempotent per card.
        if state.player.hand.contains(&uid) && !state.player.retain_this_turn.contains(&uid) {
            state.player.retain_this_turn.push(uid);
        }
    }
}

/// Resolve a [`ChoiceKind::ApplyEtherealInHand`] pending choice (SculptingStrike:
/// "pick a non-Ethereal hand card and give it Ethereal"). `selection` is a
/// length-1 slice indexing `pc.options`; the chosen option's `card_uid` gets
/// Ethereal via [`crate::card_effects::apply_ethereal_keyword`]. Empty/invalid
/// selection or a uid with no backing card is a defensive no-op. Pure player
/// choice with ZERO RNG. Mirrors C# `CardCmd.ApplyKeyword(card, Ethereal)`
/// (SculptingStrike.cs).
/// Resolve a [`ChoiceKind::ApplySlyInHand`] choice (HandTrick): record the
/// chosen hand card's uid in `player.sly_this_turn` so it counts as Sly for
/// the rest of the turn (auto-plays if actively discarded). Mirrors C#
/// `CardCmd.ApplySingleTurnSly` → `CardModel.GiveSingleTurnSly`.
fn resolve_apply_sly_in_hand(state: &mut CombatState, pc: &PendingChoice, selection: &[u8]) {
    let Some(uid) = selected_card_uid(pc, selection) else {
        return;
    };
    if !state.player.sly_this_turn.contains(&uid) {
        state.player.sly_this_turn.push(uid);
    }
}

fn resolve_apply_ethereal_in_hand(state: &mut CombatState, pc: &PendingChoice, selection: &[u8]) {
    let Some(uid) = selected_card_uid(pc, selection) else {
        return;
    };
    let idx = uid.0.get() as usize - 1;
    if let Some(card) = state.cards.get_mut(idx) {
        crate::card_effects::apply_ethereal_keyword(card);
    }
}

/// Resolve a [`ChoiceKind::TransformInHandToMinionStrike`] pending choice
/// (Begone: "pick a hand card and transform it into MinionStrike"). `selection`
/// is a length-1 slice indexing `pc.options`; the chosen option's `card_uid` is
/// transformed in place into a freshly-minted `MinionStrike` via
/// [`CombatState::transform_card`]. The MinionStrike is upgraded iff the
/// resolving source card (the Begone held in the Play pile while this choice is
/// awaited — `player.play` tail) is itself upgraded, mirroring C# `Begone.OnPlay`'s
/// `if (base.IsUpgraded) CardCmd.Upgrade(minionStrike)`. The MinionStrike POD is
/// minted through the content-registered card factory ([`lookup_card_factory`]),
/// so this stays in core without a content dep. Empty/invalid selection, a uid
/// with no backing card, or an unregistered factory is a defensive no-op. Pure
/// player choice with ZERO RNG. Bead `sts2-rs-begone-select`.
fn resolve_transform_in_hand_to_minion_strike(
    state: &mut CombatState,
    pc: &PendingChoice,
    selection: &[u8],
) {
    let Some(uid) = selected_card_uid(pc, selection) else {
        return;
    };
    // C# upgrades the created MinionStrike iff Begone is upgraded. The Begone
    // that opened this choice is held in the Play pile (its OnPlay is suspended
    // awaiting the select; the deferred Play→Discard flush runs only after the
    // choice resolves), so the Play-pile tail is that very card.
    let upgraded = state
        .player
        .play
        .last()
        .map(|u| u.0.get() as usize - 1)
        .and_then(|i| state.cards.get(i))
        .is_some_and(|c| c.upgraded);
    let Some(factory) = lookup_card_factory() else {
        return;
    };
    let Some(minion) = factory(crate::cards::CardId::MinionStrike, upgraded) else {
        return;
    };
    let _ = state.transform_card(uid, minion);
}

/// Resolve a [`ChoiceKind::TransformInHandToMinionSacrifice`] pending choice
/// (Guards: "pick ANY number of hand cards and transform each into
/// MinionSacrifice"). `selection` is a slice of indices into `pc.options`
/// (0..=hand size, possibly empty — `min=0` is skippable); each chosen option's
/// `card_uid` is transformed in place into a freshly-minted `MinionSacrifice`
/// via [`CombatState::transform_card`], mirroring C# `Guards.OnPlay`'s
/// `foreach (item in selected) Transform(item, CreateCard<MinionSacrifice>)`.
/// Each MinionSacrifice is upgraded iff the resolving source card (the Guards
/// held in the Play pile while the choice is awaited — `player.play` tail) is
/// itself upgraded, mirroring `if (base.IsUpgraded) CardCmd.Upgrade(...)`. The
/// MinionSacrifice POD is minted through the content-registered card factory
/// ([`lookup_card_factory`]), so this stays in core without a content dep. An
/// empty selection (the player declined), a uid with no backing card, or an
/// unregistered factory is a defensive no-op. Pure player choice with ZERO RNG.
/// Bead `sts2-rs-guards-select`.
fn resolve_transform_in_hand_to_minion_sacrifice(
    state: &mut CombatState,
    pc: &PendingChoice,
    selection: &[u8],
) {
    // C# upgrades every created MinionSacrifice iff Guards is upgraded. The
    // Guards that opened this choice is held in the Play pile (its OnPlay is
    // suspended awaiting the select; the deferred Play→Discard flush runs only
    // after the choice resolves), so the Play-pile tail is that very card.
    let upgraded = state
        .player
        .play
        .last()
        .map(|u| u.0.get() as usize - 1)
        .and_then(|i| state.cards.get(i))
        .is_some_and(|c| c.upgraded);
    let Some(factory) = lookup_card_factory() else {
        return;
    };
    // Resolve each selected option to its hand-card uid FIRST, before any
    // transform mutates `state.cards` — `transform_card` swaps the POD in place
    // and keeps the same `CardUid`, so collecting uids up front is robust
    // regardless of resolution order. Duplicate/out-of-range indices are skipped.
    let uids: Vec<CardUid> = selection
        .iter()
        .filter_map(|&sel_idx| pc.options.get(usize::from(sel_idx)))
        .filter_map(|opt| opt.card_uid)
        .collect();
    for uid in uids {
        let Some(minion) = factory(crate::cards::CardId::MinionSacrifice, upgraded) else {
            return;
        };
        let _ = state.transform_card(uid, minion);
    }
}

/// Resolve a [`ChoiceKind::TransformDrawCardsToMinionDiveBomb`] pending choice
/// (Charge: "pick TWO draw-pile cards; transform each into a `MinionDiveBomb`").
/// `selection` indexes `pc.options` (the (rarity, id)-sorted draw grid); each
/// selected option's `card_uid` is transformed in place into a freshly-minted
/// `MinionDiveBomb` via [`CombatState::transform_card`], mirroring C#
/// `Charge.OnPlay`'s `foreach (item) { TransformTo<MinionDiveBomb>(item); if
/// (IsUpgraded) Upgrade(added); }`.
///
/// The minted `MinionDiveBomb` is upgraded iff the resolving CHARGE is upgraded.
/// CHARGE is held in the Play pile (its OnPlay is suspended awaiting the select;
/// the deferred Play→Discard flush runs only after the choice resolves), so the
/// Play-pile tail is that very card — identical to Guards
/// ([`resolve_transform_in_hand_to_minion_sacrifice`]). The factory mints
/// `MinionDiveBomb` already-upgraded via its bool, the right analog of C#
/// `TransformTo<MinionDiveBomb>` + `Upgrade`. Empty/invalid selection, a uid
/// with no backing card, or an unregistered factory is a defensive no-op. Pure
/// player choice with ZERO RNG.
fn resolve_transform_draw_cards_to_minion_dive_bomb(
    state: &mut CombatState,
    pc: &PendingChoice,
    selection: &[u8],
) {
    let upgraded = state
        .player
        .play
        .last()
        .map(|u| u.0.get() as usize - 1)
        .and_then(|i| state.cards.get(i))
        .is_some_and(|c| c.upgraded);
    let Some(factory) = lookup_card_factory() else {
        return;
    };
    // Collect the selected option uids FIRST, before any transform mutates
    // `state.cards` — `transform_card` rebinds the slot but keeps the same
    // `CardUid`, so collecting uids up front is robust regardless of order.
    // Duplicate/out-of-range indices are skipped.
    let uids: Vec<CardUid> = selection
        .iter()
        .filter_map(|&sel_idx| pc.options.get(usize::from(sel_idx)))
        .filter_map(|opt| opt.card_uid)
        .collect();
    for uid in uids {
        let Some(minion) = factory(crate::cards::CardId::MinionDiveBomb, upgraded) else {
            return;
        };
        let _ = state.transform_card(uid, minion);
    }
}

/// Resolve a [`ChoiceKind::TransfigureCardInHand`] pending choice (Transfigure:
/// "pick a hand card; bump its cost +1 this combat and make it Replay once
/// more"). `selection` is a length-1 slice indexing `pc.options`; the chosen
/// option's `card_uid` is mutated in place, mirroring C# `Transfigure.OnPlay`:
/// `if (!EnergyCost.CostsX && EnergyCost.GetWithModifiers(None) >= 0)
/// EnergyCost.AddThisCombat(1)` and `BaseReplayCount++` (the replay bump fires
/// unconditionally, even when the cost bump is skipped). `is_x_cost()` covers
/// `CostsX` (cost `-1` for non-curse/status); `cost >= 0` mirrors the
/// modified-cost-non-negative check. Empty/invalid selection or a uid with no
/// backing card is a defensive no-op. Pure player choice with ZERO RNG.
fn resolve_transfigure_card_in_hand(state: &mut CombatState, pc: &PendingChoice, selection: &[u8]) {
    let Some(uid) = selected_card_uid(pc, selection) else {
        return;
    };
    let idx = uid.0.get() as usize - 1;
    if let Some(card) = state.cards.get_mut(idx) {
        if !card.is_x_cost() && card.cost >= 0 {
            card.cost = card.cost.saturating_add(1);
        }
        card.base_replay_count = card.base_replay_count.saturating_add(1);
    }
}

/// Resolve a [`ChoiceKind::AutoPlaySkillFromHand`] pending choice
/// (DecisionsDecisions: "pick a Skill in hand and auto-play it 3 times").
/// `selection` is a length-1 slice indexing `pc.options`; the chosen option's
/// `card_uid` is auto-played `REPEAT` times, mirroring C#
/// `DecisionsDecisions.OnPlay`'s `for i in 0..Repeat: CardCmd.AutoPlay(card,
/// null)` (DecisionsDecisions.cs:39-43).
///
/// `REPEAT` is the card's `RepeatVar(3)` — a fixed 3, NOT upgrade-scaled (only
/// `CardsVar` is bumped by `OnUpgrade`; the draw was already applied when this
/// choice was requested). C# captures the `CardModel` *reference* once
/// (`card`), then replays it by reference each iteration regardless of which
/// pile it now sits in: the first AutoPlay finds it in HAND, plays it, and a
/// non-Exhaust Skill routes to DISCARD; the 2nd/3rd AutoPlay then replay that
/// same uid out of the discard pile. [`crate::combat_loop::auto_play_specific_card_from_discard`]
/// re-locates the uid across `Discard`/`Draw`/`Hand` each call (the exact
/// pile-agnostic semantics of C# `CardCmd.AutoPlay(CardModel, ...)`,
/// CardCmd.cs:33), so it handles both the hand-source 1st play and the
/// discard-source replays — unlike [`crate::combat_loop::auto_play_specific_card`],
/// which only searches the DRAW pile and would no-op here. Each
/// `auto_play_specific_card_from_discard` internally honors the
/// `IsOverOrEnding`/owner-dead breaks via [`resolve_auto_played_card`]. Empty /
/// invalid selection or a uid with no backing card is a defensive no-op. Pure
/// player choice with ZERO RNG on the select itself (the auto-plays may roll
/// per-card targeting RNG inside `resolve_auto_played_card`, matching C#).
fn resolve_auto_play_skill_from_hand(
    state: &mut CombatState,
    pc: &PendingChoice,
    selection: &[u8],
) {
    // C# `DecisionsDecisions` `RepeatVar(3)` — fixed, not upgrade-scaled.
    const REPEAT: usize = 3;
    let Some(uid) = selected_card_uid(pc, selection) else {
        return;
    };
    for _ in 0..REPEAT {
        // C# `CardCmd.AutoPlay` replays the SAME card by reference regardless of
        // pile; `auto_play_specific_card_from_discard` re-locates `uid` across
        // Discard/Draw/Hand each iteration (hand on the 1st play, discard on the
        // 2nd/3rd for non-Exhaust Skills). It also short-circuits when combat is
        // over (`resolve_auto_played_card`'s `IsOverOrEnding` analog); if the
        // card is gone (e.g. an Exhaust Skill that left no replayable copy) the
        // helper no-ops and the loop simply continues.
        crate::combat_loop::auto_play_specific_card_from_discard(state, uid, false);
    }
}

/// Resolve a [`ChoiceKind::TransformDrawCardToSoul`] pending choice (Seance:
/// "pick a draw-pile card and transform it into a Soul"). `selection` is a
/// length-1 slice indexing `pc.options`; the chosen option's `card_uid` is
/// transformed in place into a freshly-minted, UN-upgraded `Soul` via
/// [`CombatState::transform_card`], mirroring C# `Seance.OnPlay`'s
/// `CardCmd.TransformTo<Soul>(chosen)` (the transform does NOT carry the source
/// card's upgrade flag). The Soul POD is minted through the content-registered
/// card factory ([`lookup_card_factory`]), so this stays in core without a
/// content dep. Empty/invalid selection, a uid with no backing card, or an
/// unregistered factory is a defensive no-op. Pure player choice with ZERO RNG.
fn resolve_transform_draw_card_to_soul(
    state: &mut CombatState,
    pc: &PendingChoice,
    selection: &[u8],
) {
    let Some(uid) = selected_card_uid(pc, selection) else {
        return;
    };
    let Some(factory) = lookup_card_factory() else {
        return;
    };
    let Some(soul) = factory(crate::cards::CardId::Soul, false) else {
        return;
    };
    let _ = state.transform_card(uid, soul);
}

/// Resolve a [`ChoiceKind::CloneChosenCardNextTurn`] pending choice (Nightmare:
/// "pick a hand card; next turn put `Amount` clones of it into your hand").
/// `selection` is a length-1 slice indexing `pc.options`; the chosen option's
/// `card_uid` is stashed into the most-recently-applied `PowerKind::Nightmare`
/// instance's `counter` (the Rust analog of C# `NightmarePower.SetSelectedCard`
/// — `CardUid` is `NonZeroU32`, so `counter == 0` ⇒ "not set"). The turn-start
/// [`crate::hooks::dispatch_before_hand_draw_powers`] Nightmare arm reads the
/// counter and clones the card `Amount` times into hand before the draw,
/// mirroring C# `NightmarePower.BeforeHandDraw` (NightmarePower.cs:35-47).
///
/// GH #364 A1: the power is applied HERE, not by `play_nightmare` before the
/// choice opens. C# `Nightmare.OnPlay` awaits the select at `Nightmare.cs:32`
/// and only then runs
/// `(await PowerCmd.Apply<NightmarePower>(..., 3m, ...)).SetSelectedCard(card)`
/// at `Nightmare.cs:44`, gated on `selectedCard != null` — so a mid-play
/// snapshot must show NO Nightmare power. `play_nightmare` rides the apply in
/// on [`PendingChoice::post_effect`] (the Scavenge idiom); this resolver fires
/// that post-effect first and then stashes into the instance it just created,
/// reproducing C#'s apply-then-`SetSelectedCard` order exactly.
///
/// The stash walks the player's powers in reverse to find the freshest
/// instance. Empty/invalid selection or a uid with no backing card is a
/// defensive no-op — and, matching C#'s null gate, applies no power either.
/// Pure player choice with ZERO RNG. Bead `sts2-rs-nightmare-select`.
fn resolve_clone_chosen_card_next_turn(
    state: &mut CombatState,
    pc: &PendingChoice,
    selection: &[u8],
) {
    let Some(uid) = selected_card_uid(pc, selection) else {
        return;
    };
    // C# `Nightmare.cs:44`: Apply trails the select and precedes SetSelectedCard.
    apply_post_choice_effect(state, pc.post_effect);
    let stash = i32::try_from(uid.0.get()).unwrap_or(i32::MAX);
    for power in state.powers.iter_mut().rev() {
        if power.kind == PowerKind::Nightmare {
            power.counter = stash;
            break;
        }
    }
}

/// Resolve a [`ChoiceKind::AddFromDrawToHand`] pending choice
/// (Stratagem: "pick `Amount` draw-pile cards and add them to hand").
/// `selection` indexes into `pc.options` (the (rarity, id)-sorted grid);
/// every selected option's `card_uid` is removed from `draw` and pushed
/// onto `hand` — subject to [`crate::cards::HAND_LIMIT`], past which the
/// card is redirected to discard, mirroring C# `CardPileCmd.Add(..,
/// Hand)`'s `isFullHandAdd → Discard` branch (`CardPileCmd.cs:363-366`).
/// A raw pile move: NO `AfterCardDrawn` riders fire (C# `Add` ≠ `Draw`).
/// Invalid indices / uids no longer in the draw pile are skipped
/// defensively. Mirrors C# `StratagemPower.AfterShuffle`'s
/// `foreach (item in await FromSimpleGrid(..)) CardPileCmd.Add(item,
/// Hand)`. Bead `sts2-rs-63mew`.
fn resolve_add_from_draw_to_hand(state: &mut CombatState, pc: &PendingChoice, selection: &[u8]) {
    for &sel_idx in selection {
        let Some(opt) = pc.options.get(usize::from(sel_idx)) else {
            continue;
        };
        let Some(uid) = opt.card_uid else {
            continue;
        };
        if let Some(pos) = state.player.draw.iter().position(|&u| u == uid) {
            let card = state.player.draw.remove(pos);
            let _ = crate::cards::try_add_to_hand(&mut state.player, card);
        }
    }
}

/// Resolve a [`ChoiceKind::ForegoneConclusionFromDraw`] pending choice
/// (Foregone Conclusion: "pick `Amount` draw-pile cards, add them to hand,
/// then remove `ForegoneConclusionPower`"). `selection` indexes into
/// `pc.options` (the (rarity, id)-sorted draw grid); every selected option's
/// `card_uid` is moved from `draw` to `hand` (full hand redirects to discard
/// per [`crate::cards::try_add_to_hand`]), then EVERY remaining
/// `ForegoneConclusionPower` is stripped from `state.powers`, the player's
/// power list, and every creature's power list. Mirrors C#
/// `ForegoneConclusionPower.BeforeHandDraw` (ForegoneConclusionPower.cs:18-30):
/// `await CardSelectCmd.FromSimpleGrid(...)` → `CardPileCmd.Add(item, Hand)`
/// per pick → `PowerCmd.Remove(this)`. The power removal lives HERE (not at
/// the dispatch hook) because C# `PowerCmd.Remove(this)` runs AFTER the select
/// awaits — removing it at surface time would let the power re-fire next turn.
fn resolve_foregone_conclusion_from_draw(
    state: &mut CombatState,
    pc: &PendingChoice,
    selection: &[u8],
) {
    for &sel_idx in selection {
        let Some(opt) = pc.options.get(usize::from(sel_idx)) else {
            continue;
        };
        let Some(uid) = opt.card_uid else {
            continue;
        };
        if let Some(pos) = state.player.draw.iter().position(|&u| u == uid) {
            let card = state.player.draw.remove(pos);
            // Bead `sts2-rs-09x5n`/`sts2-rs-ztl78`: raw index removal that
            // bypasses `cards::pop_next_card` — clear any stale pin on the
            // removed card so a later `pop_next_card` doesn't short-circuit
            // to a deterministic pop and skip a chance node (matches
            // `resolve_add_from_draw_by_type`).
            crate::cards::unpin(&mut state.player, card);
            let _ = crate::cards::try_add_to_hand(&mut state.player, card);
        }
    }
    // C# `PowerCmd.Remove(this)`: strip the foregone power(s) now that the
    // (deferred) select has resolved. Re-scan by kind — the surfacing hook
    // intentionally did NOT remove them.
    let foregone_ids: Vec<_> = state
        .powers
        .iter()
        .filter(|p| p.kind == crate::powers::PowerKind::ForegoneConclusion)
        .map(|p| p.id)
        .collect();
    for pid in foregone_ids {
        state.powers.retain(|p| p.id != pid);
        state.player.powers.retain(|u| u.0 != pid);
        for c in &mut state.creatures {
            c.powers.retain(|u| u.0 != pid);
        }
    }
}

/// Resolve a [`ChoiceKind::ExhaustFromHand`] pending choice (Purity /
/// Ashwater: "exhaust the selected hand cards"). `selection` is the full
/// list of indices into `pc.options`; every selected option's `card_uid`
/// is exhausted via [`crate::card_effects::exhaust_card_uid`] (which
/// removes it from hand and fires the `on_card_exhausted` listeners).
/// Invalid indices / cards no longer in hand are skipped. Mirrors C#
/// `CardSelectCmd.FromHand` → `CardCmd.Exhaust`. Bead `sts2-rs-1xmxp`.
fn resolve_exhaust_from_hand(state: &mut CombatState, pc: &PendingChoice, selection: &[u8]) {
    for &sel_idx in selection {
        let Some(opt) = pc.options.get(usize::from(sel_idx)) else {
            continue;
        };
        if let Some(uid) = opt.card_uid {
            let _ = crate::card_effects::exhaust_card_uid(state, uid);
        }
    }
    apply_post_choice_effect(state, pc.post_effect);
}

/// Apply a [`PendingChoice::post_effect`] after its selection resolves.
/// Used by cards (Brand, BurningPact) whose C# `OnPlay` runs a follow-up
/// command once the player has picked a card to exhaust. A `None`
/// post-effect (the common case) is a no-op. Bead `sts2-rs-pe5zo`.
/// Map a KnowledgeDemon curse card to the `(PowerKind, amount)` its
/// `OnChosen()` applies to the player. Mirrors C# `Disintegration` /
/// `MindRot` / `Sloth` / `WasteAway` `.OnChosen()` (each
/// `PowerCmd.Apply<…Power>(player, …)`), with `Disintegration`'s magnitude
/// read from `KnowledgeDemon._disintegrationDamageValues[counter] = {6,7,8}`
/// (the pre-increment curse counter). Returns `None` for any other card id
/// (defensive — only these four are ever offered). Bead `sts2-rs-ybtmu`.
#[must_use]
fn knowledge_demon_curse_power(card_id: cards::CardId, counter: u8) -> Option<(PowerKind, i32)> {
    const DISINTEGRATION_DAMAGE: [i32; 3] = [6, 7, 8];
    match card_id {
        cards::CardId::Disintegration => {
            let amt = DISINTEGRATION_DAMAGE
                .get(counter as usize)
                .copied()
                .unwrap_or(8);
            Some((PowerKind::Disintegration, amt))
        }
        cards::CardId::MindRot => Some((PowerKind::MindRot, 1)),
        cards::CardId::SlothStatus => Some((PowerKind::Sloth, 3)),
        cards::CardId::WasteAway => Some((PowerKind::WasteAway, 1)),
        _ => None,
    }
}

/// Resolve a [`ChoiceKind::MonsterCurseSelect`] (KnowledgeDemon
/// CURSE_OF_KNOWLEDGE). The single selected option names a curse card; its
/// power is applied to the player via the legacy `state.player.powers`
/// surface (the wire-side observer the parity harness reads — matching the
/// deterministic-Disintegration path proven clean in the prior commit).
/// `Disintegration`'s magnitude is keyed by the pre-increment curse counter,
/// which rides on [`CombatState::pending_curse_choice`] from the enemy-turn
/// curse move until this resolver consumes it. An empty selection (only
/// possible via malformed input — the choice is `min=max=1`, no skip) is a
/// no-op. Bead `sts2-rs-ybtmu`.
fn resolve_monster_curse_select(state: &mut CombatState, pc: &PendingChoice, selection: &[u8]) {
    let counter = state.pending_curse_choice.unwrap_or(0);
    state.pending_curse_choice = None;
    let Some(&sel) = selection.first() else {
        return;
    };
    let Some(opt) = pc.options.get(sel as usize) else {
        return;
    };
    if let Some((kind, amount)) = knowledge_demon_curse_power(opt.card_id, counter) {
        apply_player_power_legacy(state, kind, amount);
    }
}

fn apply_post_choice_effect(state: &mut CombatState, effect: Option<PostChoiceEffect>) {
    match effect {
        Some(PostChoiceEffect::GainStrength(amount)) if amount > 0 => {
            if let Some(uid) = state.player.creature_uid {
                // Mirror creature present (always true in real combat —
                // run_manager calls init_player_creature at combat start).
                crate::card_effects::apply_power_to_creature(
                    state,
                    uid,
                    PowerKind::Strength,
                    amount,
                );
            } else {
                apply_player_power_legacy(state, PowerKind::Strength, amount);
            }
        }
        Some(PostChoiceEffect::DrawCards(count)) if count > 0 => {
            // NoDraw-aware (mirrors C# `CardPileCmd.Draw`), matching the
            // direct-draw path BurningPact used before the choice surface.
            let _ = crate::cards::draw_via_state(state, count);
        }
        Some(PostChoiceEffect::ApplyPlayerPower(kind, amount)) if amount > 0 => {
            if let Some(uid) = state.player.creature_uid {
                crate::card_effects::apply_power_to_creature(state, uid, kind, amount);
            } else {
                apply_player_power_legacy(state, kind, amount);
            }
        }
        // Non-positive magnitudes and `None` are no-ops.
        Some(_) | None => {}
    }
}

/// Apply a player power via the legacy `state.player.powers` surface,
/// used only when no mirror creature is present (test fixtures that
/// bypass [`CombatState::init_player_creature`]). Mirrors the no-mirror
/// fallback in the content `apply_power_to_player` helpers: stack into an
/// existing instance of `kind`, else allocate a fresh [`Power`]. Real
/// combat always has the mirror, so this branch is exercised only by
/// fixtures. Bead `sts2-rs-pe5zo`.
fn apply_player_power_legacy(state: &mut CombatState, kind: PowerKind, amount: i32) {
    if amount <= 0 {
        return;
    }
    let existing = state
        .player
        .powers
        .iter()
        .copied()
        .find(|uid| state.powers.iter().any(|p| p.id == uid.0 && p.kind == kind));
    if let Some(uid) = existing {
        if let Some(pow) = state.powers.get_mut_by_id(uid.0) {
            pow.amount = pow.amount.saturating_add(amount);
        }
        return;
    }
    let next_id_raw = state
        .powers
        .iter()
        .map(|p| p.id.get())
        .max()
        .map_or(1, |m| m.saturating_add(1));
    let new_id = NonZeroU32::new(next_id_raw).expect("power id non-zero");
    state.powers.push(Power::new(new_id, kind, amount));
    state.player.powers.push(PowerUid(new_id));
}

/// Resolve a [`ChoiceKind::DiscardFromHand`] pending choice
/// (HiddenDaggers / GamblersBrew: "discard the selected hand cards").
/// `selection` is the full list of indices into `pc.options`; every
/// selected option's `card_uid` still in hand is moved to the discard
/// pile via [`crate::cards::discard`]. Invalid indices / uids no longer
/// in hand are skipped (defensive). Mirrors C#
/// `CardSelectCmd.FromHandForDiscard` → `CardCmd.Discard`. Bead
/// `sts2-rs-1xmxp`.
fn resolve_discard_from_hand(state: &mut CombatState, pc: &PendingChoice, selection: &[u8]) {
    // Collect the selected hand uids in selection order, then route through the
    // shared C# `CardCmd.Discard` path so each fires `AfterCardDiscarded` AND any
    // discarded `Sly` card auto-plays afterwards (Ricochet/Sneaky/Reflex/…).
    // The prior raw `cards::discard` move dropped both — most visibly the Sly
    // auto-play (run-parity Silent RP42: discarding Ricochet via Survivor's
    // FromHandForDiscard dealt 0 in Rust vs Ricochet's 4 hits in C#).
    let uids: smallvec::SmallVec<[crate::CardUid; 16]> = selection
        .iter()
        .filter_map(|&sel_idx| {
            pc.options
                .get(usize::from(sel_idx))
                .and_then(|o| o.card_uid)
        })
        .collect();
    let _ = crate::card_effects::discard_hand_cards_with_hooks(state, &uids);
}

/// Resolve a [`ChoiceKind::DiscardFromHandAndDraw`] pending choice
/// (GamblersBrew). Discards every selected hand card, firing
/// `after_card_discarded` per card, then draws one card per card
/// actually discarded.
///
/// Mirrors C# `CardCmd.DiscardAndDraw(list, list.Count)`
/// (`MegaCrit.Sts2.Core.Commands/CardCmd.cs:128-161`):
///
/// [decompiled C# omitted — see the cited path]
///
/// Bead `sts2-rs-9sgbe`: the prior port raw-moved each card via
/// [`crate::cards::discard`] and drew via [`crate::cards::draw`],
/// bypassing BOTH the per-card `after_card_discarded` dispatch (an
/// *active* discard fires the hook — Reflex/Sly/ToughBandages/
/// MementoMori, the discard-counter, and any on-discard relic) AND the
/// on-draw riders + reshuffle hooks the canonical
/// [`CombatState::draw_into_hand`] runs. That silent bypass desynced
/// the RNG cursor against C# whenever a discard- or draw-listening
/// power/relic was present (the TEA_OF_DISCOURTESY RNG_DESYNC residual
/// noted on `sts2-rs-ktqmg`). The draw must happen AFTER every discard
/// has fired its hook, exactly as C# orders it. Bead `sts2-rs-1xmxp`
/// established the player-choice shape.
fn resolve_discard_from_hand_and_draw(
    state: &mut CombatState,
    pc: &PendingChoice,
    selection: &[u8],
) {
    // Collect the selected hand uids in selection order. C# `DiscardAndDraw`
    // (CardCmd.cs:128-161) discards each (firing `AfterCardDiscarded`),
    // collects `Sly` cards, draws AFTER all discards, THEN auto-plays the Sly
    // cards. Split the discard+hook from the Sly auto-play so the draw lands
    // between them, exactly as C# orders it.
    let uids: smallvec::SmallVec<[crate::CardUid; 16]> = selection
        .iter()
        .filter_map(|&sel_idx| {
            pc.options
                .get(usize::from(sel_idx))
                .and_then(|o| o.card_uid)
        })
        .collect();
    let sly: smallvec::SmallVec<[crate::CardUid; 8]> = uids
        .iter()
        .copied()
        .filter(|uid| crate::card_effects::card_is_sly(state, *uid))
        .collect();
    let mut discarded: i32 = 0;
    for &uid in &uids {
        if !state.player.hand.contains(&uid) {
            continue;
        }
        // Capture the discarded card's identity for the hook ctx BEFORE the
        // pile move (read it up front to mirror the exhaust-side pattern).
        let idx = uid.0.get() as usize - 1;
        let (card_id, card_tags, card_type) = state.cards.get(idx).map_or_else(
            || (None, crate::cards::CardTags::NONE, None),
            |c| (Some(c.card_id), c.tags, Some(c.card_type)),
        );
        let _ = crate::cards::discard(&mut state.player, uid);
        discarded += 1;
        // C# fires `Hook.AfterCardDiscarded` per discarded card (CardCmd.cs:150)
        // — an active discard, distinct from the end-of-turn flush which fires
        // no hook. Order: powers bus then relic bus, handled inside the
        // dispatcher.
        let hook_ctx = crate::hooks::CardCtx {
            card_id,
            card_uid: Some(uid),
            card_tags,
            card_type,
        };
        crate::hooks::dispatch_after_card_discarded(state, &hook_ctx);
    }
    if discarded > 0 {
        // Draw AFTER all discards (CardCmd.cs:153-156), via the canonical
        // hand-draw site so on-draw riders, reshuffle hooks, and the
        // cards-drawn counter all fire — but through the C# `ShouldDraw`
        // gate: a GamblersBrew "draw that many" under Battle Trance's
        // NO_DRAW draws nothing in C# (`NoDrawPower.ShouldDraw`, sweep
        // seed 2335 — the ungated draw spuriously reshuffled the discard).
        let _ = state.draw_into_hand_card_effect(discarded);
    }
    // Auto-play each discarded `Sly` card AFTER the draw (CardCmd.cs:157-160).
    for uid in sly {
        let _ = crate::combat_loop::auto_play_specific_card_from_discard(state, uid, false);
    }
}

/// Resolve a [`ChoiceKind::MakeFreeInHand`] pending choice
/// (TouchOfInsanity: "make one hand card free this combat"). Selection
/// is a length-1 slice of indices into `pc.options`; the chosen option's
/// in-combat card has its `cost` set to 0. `CombatState.cards` is
/// per-combat scratch, so this persists only for the current combat
/// (C# `SetToFreeThisCombat`). Empty/invalid selection, or a uid no
/// longer present, is a defensive no-op. Bead `sts2-rs-1xmxp`.
fn resolve_make_free_in_hand(state: &mut CombatState, pc: &PendingChoice, selection: &[u8]) {
    let Some(uid) = selected_card_uid(pc, selection) else {
        return;
    };
    let idx = uid.0.get() as usize - 1;
    if let Some(c) = state.cards.get_mut(idx) {
        c.cost = 0;
        // C# `SetToFreeThisCombat` zeros the star cost too — mark it so the
        // legal-action gate and play-time spend treat this star card as free.
        c.set_combat_var(crate::cards::VarKey::StarCostFreeThisCombat, 1);
        // C# `SetThisCombat(0)` (CardModel.cs:1275 → CardEnergyCost.cs:238)
        // installs an *Absolute* local cost modifier that owns the evaluated
        // cost for the rest of combat. Mark it so a later in-combat upgrade
        // (e.g. ARMAMENTS+1 on a freed CALAMITY, EnergyCost.UpgradeBy(-1))
        // takes `Card::upgrade`'s absolute-modifier clamp branch
        // (cards.rs:819-838) — which keeps the evaluated cost at 0 — instead
        // of the additive else-branch that would underflow 0 + (2-3) = -1.
        // A -1 cost reads as an X-cost (is_x_cost), draining all energy on
        // play. The collapsed base+modifier model otherwise has no way to
        // distinguish a freed card from a genuinely 0-base one. Bead
        // `sts2-rs-2e9q2` (gh#180 colorless/potion slice).
        c.set_combat_var(crate::cards::VarKey::CostAbsolutelySet, 1);
    }
}

fn resolve_putback_choice(state: &mut CombatState, pc: &PendingChoice, selection: &[u8]) {
    let Some(opt) = selected_option(pc, selection) else {
        return;
    };
    let Some(uid) = opt.card_uid else {
        return;
    };
    let source = opt.source_pile.unwrap_or(crate::cards::PileKind::Discard);
    let removed = match source {
        crate::cards::PileKind::Discard => {
            if let Some(pos) = state.player.discard.iter().position(|u| *u == uid) {
                state.player.discard.remove(pos);
                true
            } else {
                false
            }
        }
        crate::cards::PileKind::Draw => {
            if let Some(pos) = state.player.draw.iter().position(|u| *u == uid) {
                state.player.draw.remove(pos);
                true
            } else {
                false
            }
        }
        crate::cards::PileKind::Hand => {
            if let Some(pos) = state.player.hand.iter().position(|u| *u == uid) {
                state.player.hand.remove(pos);
                true
            } else {
                false
            }
        }
        crate::cards::PileKind::Exhaust => {
            if let Some(pos) = state.player.exhaust.iter().position(|u| *u == uid) {
                state.player.exhaust.remove(pos);
                true
            } else {
                false
            }
        }
        crate::cards::PileKind::Play => false,
    };
    if removed {
        state.player.draw.insert(0, uid);
        // Bead `sts2-rs-o6xw`: Headbutt-style resolvers know the
        // exact destination position. Pin the inserted card so that
        // MCTS PIMC determinization re-shuffles preserve the
        // top-of-draw guarantee. A subsequent Headbutt push pins
        // the new card at `Top(0)` and demotes the prior pin to
        // `Top(1)` (handled by `pin_top`'s push-to-front).
        crate::cards::pin_top(&mut state.player, uid);
    }
}

/// Returns the crate version string (kept for use by the eventual `PyO3`
/// surface).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

// ---------------------------------------------------------------------------
// Card-factory hook — registered by `sts2-content` so `sts2-core` can build
// a `Card` POD from a `CardId` without depending on the content crate.
//
// Used by `ChoiceKind::AddGeneratedToHand` resolution (Toolbox /
// ChoicesParadox): the listener stores `(card_id, upgraded)` in the
// `PendingChoice` options; the resolver synthesizes the chosen card via
// this factory and routes it through `spawn_card_into_hand`. If the
// factory is unregistered (or returns `None` for the id), the resolver
// is a defensive no-op.
// ---------------------------------------------------------------------------

/// Signature of the card-factory hook: produce a `Card` POD for the given
/// `(CardId, upgraded)`. Returns `None` if no spec is known.
pub type CardFactoryFn = fn(crate::cards::CardId, bool) -> Option<crate::cards::Card>;

static CARD_FACTORY: std::sync::OnceLock<std::sync::RwLock<Option<CardFactoryFn>>> =
    std::sync::OnceLock::new();

fn card_factory_cell() -> &'static std::sync::RwLock<Option<CardFactoryFn>> {
    CARD_FACTORY.get_or_init(|| std::sync::RwLock::new(None))
}

/// Register the card-factory hook. Called once at content-crate
/// initialization. Overwrites any previously-registered factory.
pub fn register_card_factory(f: CardFactoryFn) {
    *card_factory_cell()
        .write()
        .expect("card factory registry poisoned") = Some(f);
}

/// Look up the registered card factory, if any.
#[must_use]
pub fn lookup_card_factory() -> Option<CardFactoryFn> {
    *card_factory_cell()
        .read()
        .expect("card factory registry poisoned")
}

// ---------------------------------------------------------------------------
// Card-rarity-rank hook — registered by `sts2-content` so `sts2-core` can
// sort cards by the C# `CardRarity` enum discriminant without depending on
// the content crate. Used by the Stratagem `after_shuffle` grid-select
// (`hooks::dispatch_after_shuffle`), whose option order mirrors C#
// `StratagemPower.AfterShuffle`'s `orderby c.Rarity, c.Id`. Bead
// `sts2-rs-63mew`.
// ---------------------------------------------------------------------------

/// Signature of the card-rarity-rank hook: map a `CardId` to the C#
/// `CardRarity` enum discriminant (`CardRarity.cs`: None=0, Basic=1,
/// Common=2, Uncommon=3, Rare=4, Ancient=5, Event=6, Token=7, Status=8,
/// Curse=9, Quest=10) so core-side sorts replicate C# `orderby c.Rarity`.
pub type CardRarityRankFn = fn(crate::cards::CardId) -> u8;

static CARD_RARITY_RANK: std::sync::OnceLock<std::sync::RwLock<Option<CardRarityRankFn>>> =
    std::sync::OnceLock::new();

fn card_rarity_rank_cell() -> &'static std::sync::RwLock<Option<CardRarityRankFn>> {
    CARD_RARITY_RANK.get_or_init(|| std::sync::RwLock::new(None))
}

/// Register the card-rarity-rank hook. Called once at content-crate
/// initialization. Overwrites any previously-registered fn.
pub fn register_card_rarity_rank(f: CardRarityRankFn) {
    *card_rarity_rank_cell()
        .write()
        .expect("card rarity-rank registry poisoned") = Some(f);
}

/// Look up the registered card-rarity-rank fn, if any. Callers should
/// treat an unregistered hook as "all ranks equal" (sort by the
/// secondary key only).
#[must_use]
pub fn lookup_card_rarity_rank() -> Option<CardRarityRankFn> {
    *card_rarity_rank_cell()
        .read()
        .expect("card rarity-rank registry poisoned")
}

/// Content-registered predicate: `true` when a master-deck [`crate::Card`] is
/// removable (C# `CardModel.IsRemovable => !Keywords.Contains(
/// CardKeyword.Eternal)`, `CardModel.cs:738`). This is the filter
/// `CardSelectCmd.FromDeckForRemoval` (`CardSelectCmd.cs:629`) applies before
/// building the generic removal offer, and the one EVERY other C# consumer of
/// `IsRemovable` shares: Cook eligibility (`CookRestSiteOption.cs:64`), Pael's
/// PaelsTooth offer (`Pael.cs:166`), Amalgamator, SlipperyBridge, PandorasBox,
/// WoodCarvings. There is exactly one removability rule in v0.107.1 and this
/// is it — a caller that wants a different one is a bug.
///
/// Eternal cards (Greed, AscendersBane, …) are EXCLUDED; basic Strike/Defend
/// stay, since they carry no Eternal keyword and ARE removable at a shop as
/// well as via Neow/events.
///
/// Takes the whole `Card`, not its `CardId`, because C# reads the card's LIVE
/// keyword set: `TezcatarasEmber` adds Eternal on enchant
/// (`TezcatarasEmber.cs:18`), and NutritiousSoup hands it to every basic
/// Strike. An id-only predicate cannot see that, and since removal-offer
/// indices are positional, a wrongly-offered card shifts every later pick.
/// Bead `sts2-rs-RP22`.
pub type CardRemovableFn = fn(&crate::Card) -> bool;

static CARD_REMOVABLE: std::sync::OnceLock<std::sync::RwLock<Option<CardRemovableFn>>> =
    std::sync::OnceLock::new();

fn card_removable_cell() -> &'static std::sync::RwLock<Option<CardRemovableFn>> {
    CARD_REMOVABLE.get_or_init(|| std::sync::RwLock::new(None))
}

/// Register the generic-removal-removability hook. Called once at content-crate
/// initialization. Overwrites any previously-registered fn.
pub fn register_card_removable(f: CardRemovableFn) {
    *card_removable_cell()
        .write()
        .expect("card removable registry poisoned") = Some(f);
}

/// Look up the registered removability fn, if any. Callers should treat an
/// unregistered hook as "every card removable" (pure-core tests that never
/// install the content registry keep the legacy unfiltered offer order).
#[must_use]
pub fn lookup_card_removable() -> Option<CardRemovableFn> {
    *card_removable_cell()
        .read()
        .expect("card removable registry poisoned")
}

// ---------------------------------------------------------------------------
// Bead sts2-rs-uiq (P8-1): terminal / legal-actions helpers used by the
// MCTS rollout harness and the Python rollout bridge.
// ---------------------------------------------------------------------------

/// `true` when combat has resolved (win or loss).
#[must_use]
pub fn is_terminal(state: &CombatState) -> bool {
    state.is_over
}

/// Sum of non-minion enemy `current_hp / max_hp` for the in-progress
/// evaluation. Minions don't count — they're spawned as soak/distraction
/// and ignoring them mirrors sts_lightspeed's `getNonMinionMonsterCurHpRatio`
/// (`BattleScumSearcher2.cpp:388-405`). A creature counts as a minion when
/// it carries the [`crate::PowerKind::Minion`] power.
///
/// Returns `(sum_current_hp, sum_max_hp, alive_count)` — the caller decides
/// how to fold them. `alive_count` only includes non-minion enemies with
/// `current_hp > 0`.
///
/// Bead `sts2-rs-infQb` (issue #29): when every non-minion enemy is
/// dead but minions are still alive, fall back to the minion totals.
/// STS2's victory predicate (`combat_loop.rs:2153`) requires ALL
/// `enemy_uids` to have `hp ≤ 0` including minions, so the
/// PersonalHive / PhrogParasite / Decimillipede-split families keep
/// combat going after the named slot drops. The lightspeed mirror
/// despawns minions on boss death; STS2 doesn't, so the straight
/// mirror reported `enemy_hp_ratio = 0` (i.e. "decisive win") while
/// the engine kept the fight live — driving MCTS to End-turn-stall
/// to the round cap with Q ≈ +5.8.
fn nonminion_enemy_hp_totals(state: &CombatState) -> (i32, i32, u32) {
    let mut cur_total = 0i32;
    let mut max_total = 0i32;
    let mut alive = 0u32;
    let mut minion_cur = 0i32;
    let mut minion_max = 0i32;
    let mut minion_alive = 0u32;
    for uid in &state.enemy_uids {
        let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) else {
            continue;
        };
        let is_minion = c.powers.iter().any(|puid| {
            state
                .powers
                .iter()
                .any(|p| p.id == puid.0 && p.kind == PowerKind::Minion)
        });
        if is_minion {
            minion_cur += c.current_hp.max(0);
            minion_max += c.max_hp.max(0);
            if c.current_hp > 0 {
                minion_alive += 1;
            }
            continue;
        }
        cur_total += c.current_hp.max(0);
        max_total += c.max_hp.max(0);
        if c.current_hp > 0 {
            alive += 1;
        }
    }
    if alive == 0 && minion_alive > 0 {
        return (minion_cur, minion_max, minion_alive);
    }
    (cur_total, max_total, alive)
}

/// Victory eval upper bound used to normalize [`evaluate_victory`] into
/// `[0.5, 1.0]`. Picked so a clean kill at full HP with five potions
/// in pocket maps near `1.0` while a 1-HP win with no potions still
/// clears the `0.5` win-bucket threshold.
///
/// Formula shape matches sts_lightspeed (`BattleScumSearcher2.cpp:411`)
/// `100 * (35 + curHp + potionCount * k - turn * 0.01)` for the raw
/// score; we then map `[VICTORY_MIN, VICTORY_MAX]` → `[0.5, 1.0]`. The
/// constants assume a player `max_hp` ceiling near 100 (Ironclad
/// caps at ~95 with Stone Humidifier stacking; higher would clamp
/// at the cap from the final `.clamp(0.5, 1.0)`).
const VICTORY_RAW_MIN: f32 = 100.0 * 35.0; // ~ 1 HP, 0 potions, ~0 turns
const VICTORY_RAW_MAX: f32 = 100.0 * (35.0 + 100.0 + 5.0 * VICTORY_POTION_HP_EQUIV); // 100 HP, 5 potions

/// HP-equivalents a held potion is worth in the victory eval.
/// Lightspeed shipped `4`; bumped to `8` (2026-07-12, GH #253 decision:
/// skip the sweep, pick the mid value) because the PPO-combat gate-B
/// post-mortem (PR #250) showed 4 underprices potion thrift relative to
/// the run-level compounding of held potions — the search happily
/// spends potions in safely-won fights.
///
/// The skipped sweep has since been run (GH #447,
/// `logs/2026-08-19-potion-weight-447/`): λ ∈ {4, 8, 12, 16} on the A8
/// mechanism panel left every potion metric flat — tie-vs-EndTurn drink
/// share 34–37%, empty-belt boss entries 21–24/77, belt mean not even
/// monotone in λ. The whole 4→16 lever is worth 0.043 in the victory
/// band against a ±0.075 per-child noise floor at the run budget, so
/// the price cannot reach the drink-vs-EndTurn plies at any plausible
/// value. `8` therefore stays — now as the measured verdict that the
/// knob is insensitive, not as an unswept guess.
pub const VICTORY_POTION_HP_EQUIV: f32 = 8.0;

/// In-progress eval scale. Refit empirically against the joint
/// starter + realistic-floor-40+ corpus in bead `sts2-rs-tidt`
/// (`crates/sts2-bench/examples/eval_rescale.rs --joint
/// --realistic-floor-min 40`, 20 000 samples total):
/// - starter regime: `p1 ≈ +19.8`, `p99 ≈ +52.7`
/// - realistic floor-40+ regime: `p1 ≈ +3.0`, `p99 ≈ +7.2`
/// - **joint** (equal-weight pool): `p1 ≈ +3.0`, `p50 ≈ +19.8`,
///   `p99 ≈ +51.6`
///
/// Joint `p99 / 0.45 ≈ 114.56` is the new divisor. The 4.9× bump
/// from the original `sts2-rs-zzpb` value of `23.3` reflects two
/// structural shifts since that refit:
/// 1. The leaf-eval added 7 new positive terms across Wave-1+2+3
///    (`9lrx` strength_tanh, `mkn5` net_incoming_threat,
///    `n1ak` enemy_vulnerable/weak, `0962` player_hp,
///    `9nzs` low_hp_cliff, `95o9` energy/cards, plus
///    `ns1s`/`5l4y`/`gzbh`/`ot78`). Raw magnitudes scale ~5× up.
/// 2. The `sts2-rs-d58e959` Texel writeback bumped most weights
///    (PLAYER_HP_WEIGHT 4.0 → 5.12, ENEMY_VULNERABLE_WEIGHT
///    0.3 → 1.42, ENERGY_WASTED_WEIGHT -0.2 → +0.63, etc.).
///
/// With `IN_PROGRESS_DIVISOR = 114.56`, the 99th-percentile raw
/// value lands at `0.45` — comfortably below the widened `±0.9`
/// clamp and the `[0.5, 1.0]` victory range. The starter regime
/// dominates the joint p99 because its raw-score spread is ~9×
/// wider than the realistic-floor-40+ regime; this is acceptable
/// (the value-net training step sees both regimes, and clamping
/// starter positions too aggressively would re-introduce the
/// Wave-3 saturation pathology that the bump fixes).
///
/// Pre-`tidt` historical values, for context:
/// - `25.0` (pre-`zzpb`) clamped at `±0.49`, pinning everything
///   into the `Tie` bucket.
/// - `23.3` (`zzpb`) was empirical-refit against the post-Wave-1+2
///   shape (raw `[-5, +14]`, `p99 ≈ +10.5`). Stale once the
///   Wave-3 terms + Texel writeback landed.
const IN_PROGRESS_DIVISOR: f32 = 114.56;

/// Linear weights for columns 0–3 of [`EVAL_FEATURE_NAMES`] —
/// `damage_dealt_raw`, `alive_raw`, `potions_raw`, `round_raw`.
/// Bead `sts2-rs-dhvr`: parameterized so `scripts/texel_writeback.py`
/// (vsqc.4) can rewrite them in lockstep with the
/// [`EVAL_FEATURE_WEIGHTS`] array. Before this bead the four columns
/// used inline literals in the formula body (`* 10.0`, `* 1.0`,
/// `* 2.0`, `* 0.2`) that drifted out of sync with the Texel-tuned
/// weights — the `eval_features_dot_weights_matches_evaluate_in_progress_raw`
/// parity test failed with Δ ≈ 8.0 pre-fix, dominated by the
/// round_raw mismatch (`1.195` table vs `0.2` formula).
const DAMAGE_DEALT_WEIGHT: f32 = 11.042_911;
const ALIVE_WEIGHT: f32 = -0.110_006_18;
const POTIONS_WEIGHT: f32 = 2.0;
const ROUND_WEIGHT: f32 = 1.195_362_8;

/// Runtime override for the victory-eval potion price λ (issue #253): "a held
/// potion is worth λ HP-equivalents at a win terminal". Drives the `potions·λ`
/// term in [`evaluate_victory`] (the deployed `RichBoth` win terminal) and,
/// proportionally via [`in_progress_potion_weight`], the depth-cap leaves
/// (narrow [`evaluate_in_progress_raw`] and wide `evaluate_in_progress_wide`).
///
/// Set `STS2_RUNAI_POTION_WEIGHT=<f32>` to sweep λ (the #447 sweep ran
/// {4, 8, 12, 16} and found every potion mechanism metric flat — see
/// `logs/2026-08-19-potion-weight-447/FINDINGS.md`; the deployed default is
/// [`VICTORY_POTION_HP_EQUIV`] = 8). Read once and cached —
/// an env change after the first eval does not take effect.
///
/// Unset ⇒ [`VICTORY_POTION_HP_EQUIV`] ⇒ identical to the deployed victory
/// eval, and (via the ratio in [`in_progress_potion_weight`]) preserves the
/// tuned `POTIONS_WEIGHT = 2.0` in-progress term so the `EVAL_FEATURE_WEIGHTS`
/// dot-product invariant holds untouched.
///
/// An invalid value **panics** rather than silently falling back to the
/// default: a typo must not quietly poison an A/B arm (mirrors the
/// `STS2_RUNAI_HONEST` / `STS2_RUNAI_LEAF` refuse-to-fall-back precedent).
fn potion_weight() -> f32 {
    static POTION_WEIGHT: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *POTION_WEIGHT.get_or_init(|| match std::env::var("STS2_RUNAI_POTION_WEIGHT") {
        Err(_) => VICTORY_POTION_HP_EQUIV,
        Ok(s) => {
            let w: f32 = s.parse().unwrap_or_else(|_| {
                panic!(
                    "STS2_RUNAI_POTION_WEIGHT={s:?} is not a valid f32 — refusing to \
                     silently fall back to {VICTORY_POTION_HP_EQUIV} and poison the A/B arm"
                )
            });
            assert!(
                w.is_finite() && w >= 0.0,
                "STS2_RUNAI_POTION_WEIGHT={w} must be finite and non-negative"
            );
            w
        }
    })
}

/// The victory eval's potion price λ in effect for this process (see
/// [`potion_weight`]): reading it latches the env override, as the first
/// evaluation would. Lets a caller check the λ-pairing rule (a root potion
/// charge wants the leaf term at 0) before it searches (sts2-agent fork).
#[must_use]
pub fn victory_potion_weight() -> f32 {
    potion_weight()
}

/// HP-equivalents of the potions `state` holds, as the victory eval prices
/// them: by the search's [`belt_value::BeltValue`] when the state carries one,
/// else `potions · λ` ([`potion_weight`]; byte-identical to the deployed eval).
#[must_use]
pub fn belt_hp_equiv(state: &CombatState) -> f32 {
    match &state.belt_value {
        Some(b) => b.hp_equiv(&state.player.potions),
        None => state.player.potions.len() as f32 * potion_weight(),
    }
}

/// The depth-cap leaves' potion term (tanh scale): the belt's HP-equivalents
/// at the in-progress weight's ratio to the victory price, so a search's belt
/// value reaches both leaves in proportion; without one, `potions ·`
/// [`in_progress_potion_weight`] as deployed.
#[must_use]
pub fn in_progress_belt_term(state: &CombatState) -> f32 {
    match &state.belt_value {
        Some(b) => b.hp_equiv(&state.player.potions) * (POTIONS_WEIGHT / VICTORY_POTION_HP_EQUIV),
        None => state.player.potions.len() as f32 * in_progress_potion_weight(),
    }
}

/// In-progress (depth-cap leaf) potion price, tanh-scale — used by the narrow
/// [`evaluate_in_progress_raw`] and the wide `evaluate_in_progress_wide`. The
/// tuned base [`POTIONS_WEIGHT`] (`= 2.0`, from the `tidt` empirical refit,
/// lives on the `tanh` scale — a different scale from the linear victory eval)
/// is scaled by the sweep ratio `λ / VICTORY_POTION_HP_EQUIV` so it tracks the
/// swept victory knob proportionally while the deployed default
/// (`λ = VICTORY_POTION_HP_EQUIV`) leaves it at exactly `2.0` — keeping the
/// `EVAL_FEATURE_WEIGHTS` dot-product invariant intact (issue #253).
fn in_progress_potion_weight() -> f32 {
    POTIONS_WEIGHT * (potion_weight() / VICTORY_POTION_HP_EQUIV)
}

/// Runtime override for the victory-eval HP price w (2026-07-18
/// HP-repricing campaign): remaining player HP at a win terminal is worth
/// `w` HP-equivalents per point. Drives the `hp·w` term in
/// [`evaluate_victory`] and, proportionally via [`in_progress_hp_weight`]
/// and the wide leaf's extra HP term, the depth-cap leaves.
///
/// Motivation: the arrival-state counterfactual probe showed entry-HP
/// deficit at killer fights is the dominant causal loss axis, and the
/// deficit is accumulated inside upstream fights the search *wins* —
/// sloppily, because the default prices HP at ~1/6 of its measured
/// run-level marginal value. `STS2_RUNAI_HP_WEIGHT=<f32>` sweeps w.
///
/// Unset ⇒ `1.0` ⇒ bit-identical to the deployed evals everywhere. An
/// invalid value **panics** rather than silently falling back (mirrors
/// [`potion_weight`] — a typo must not quietly poison an A/B arm). Read
/// once and cached.
/// GH #375: bind audited intent payloads in the wide leaf's enemy-intent
/// one-hots instead of matching the "unspecified" sentinels.
///
/// `STS2_RUNAI_INTENT_ONEHOT_FIX=1` enables. **Unset ⇒ off ⇒ bit-identical
/// to the deployed leaf**, because this is a feature-distribution change,
/// not a pure bug fix in the eval's output: the four current-turn
/// `intent_{buff,debuff,defend}` weights are all exactly `0.0` (Texel fitted
/// them against columns that effectively never fire) while `intent_other`
/// carries real weight. Enabling therefore moves defending enemies out of a
/// weighted column into a zeroed one. Correct the encoding, re-tune the
/// Texel table against it, then flip the default.
///
/// The training-corpus path (`corpus_gen`) is the one place where enabling
/// is unambiguously right today — a re-tune must be fitted against the
/// corrected encoding.
#[must_use]
pub fn intent_one_hot_fix() -> bool {
    static FIX: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FIX.get_or_init(|| {
        std::env::var("STS2_RUNAI_INTENT_ONEHOT_FIX").is_ok_and(|s| s == "1" || s == "true")
    })
}

pub(crate) fn hp_weight() -> f32 {
    static HP_WEIGHT: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *HP_WEIGHT.get_or_init(|| match std::env::var("STS2_RUNAI_HP_WEIGHT") {
        Err(_) => 1.0,
        Ok(s) => {
            let w: f32 = s.parse().unwrap_or_else(|_| {
                panic!(
                    "STS2_RUNAI_HP_WEIGHT={s:?} is not a valid f32 — refusing to \
                     silently fall back to 1.0 and poison the A/B arm"
                )
            });
            assert!(
                w.is_finite() && w >= 0.0,
                "STS2_RUNAI_HP_WEIGHT={w} must be finite and non-negative"
            );
            w
        }
    })
}

/// The per-HP value gradient of the DEFAULT (clamped, in-band) victory
/// eval: `0.5 · 100 / (VICTORY_RAW_MAX − VICTORY_RAW_MIN) ≈ 0.00357`
/// value-units per HP point.
const VICTORY_HP_UNIT: f32 = 0.5 * 100.0 / (VICTORY_RAW_MAX - VICTORY_RAW_MIN);

/// The HP-repricing surplus added OUTSIDE [`evaluate_victory`]'s clamped
/// `[0.5, 1.0]` band at MCTS win terminals:
/// `(w − 1) · curHp · VICTORY_HP_UNIT`, i.e. remaining HP is priced at
/// exactly `w ×` the default in-band gradient. Zero when `w = 1` (unset)
/// — byte-identical to the deployed terminals.
///
/// **Why outside the band:** the pilot-1 in-band version (hp·w with a
/// w-scaled normalization ceiling) is dose-compressed — the win band has
/// fixed width 0.5, so rescaling the ceiling caps the effective per-HP
/// gradient at 1.4× no matter how large w gets (w=4 delivered only
/// 1.27×). True repricing must let HP-rich wins escape the band:
/// `w = 4`, 80 HP ⇒ surplus ≈ 0.86, win values reach ≈ 2.36 instead of
/// 1.5. That deliberately lets the search trade win-probability for HP
/// at up to `w ×` the deployed rate — the campaign's whole point — and
/// widens the Q spread the UCT constant sees (noted in the spec; the
/// A/B is the arbiter of whether the trade-off nets out).
#[must_use]
pub fn victory_hp_surplus(state: &CombatState) -> f32 {
    (hp_weight() - 1.0) * state.player.current_hp.max(0) as f32 * VICTORY_HP_UNIT
}

/// HP-equivalent price of ONE run-deck card a thief walks off with
/// (`SwipePower`, i.e. the ThievingHopper). Anchored to
/// [`VICTORY_POTION_HP_EQUIV`] (a potion = 8 HP): a deck card the Hopper
/// picked is worth appreciably more than a potion — its steal priorities
/// (`ThievingHopper.cs:31-65`) take an Uncommon first, then
/// Common/Rare/Event, and only fall back to Basic when nothing else is
/// left, so the loss is a real card, not a spare Strike. Priced at 2×
/// a potion. GH #604.
pub const LOOT_CARD_HP_EQUIV: f32 = 2.0 * VICTORY_POTION_HP_EQUIV;

/// HP-equivalent price of one gold. A potion runs ~60-80 gold in a shop
/// and [`VICTORY_POTION_HP_EQUIV`] prices a potion at 8 HP, so one gold
/// is ~0.1 HP. GH #604.
pub const LOOT_GOLD_HP_EQUIV: f32 = 0.1;

/// Expected Monster-room gold reward, used to price the escape-driven
/// `GoldProportion` shrink without a `RunState` in hand. The midpoint of
/// [`crate::runstate::ENCOUNTER_GOLD_LOW_MONSTER`] ..=
/// [`crate::runstate::ENCOUNTER_GOLD_HIGH_MONSTER`] (10..=20). Elite and
/// Boss rooms never scale their gold by `GoldProportion` at all (C#
/// `RewardsSet.cs:167-182`), and no Elite/Boss monster has an
/// `Intent::Escape`, so the Monster-room band is the only one that can
/// pay out here. GH #604.
pub const LOOT_ESCAPE_GOLD_BASE: f32 = 15.0;

/// Gold this combat has taken off the player that is still UNRECOVERED —
/// the `ThieveryPower` running totals (C# `DynamicVars.Gold`, kept on
/// [`Power::counter`]) minus whatever killing a `HeistPower` carrier has
/// already handed back ([`CombatState::heist_recovered_gold`]).
///
/// The two are a matched pair by construction: `SurprisePower.AfterDeath`
/// moves each Thievery counter onto the spawned FatGremlin as a
/// `HeistPower` of the same size, and `HeistPower.BeforeDeath` refunds
/// exactly that amount. So the difference is the gold that is still
/// riding on a living (or fled) gremlin. Never negative. GH #604.
#[must_use]
pub fn stolen_gold_outstanding(state: &CombatState) -> i32 {
    let stolen: i32 = state
        .powers
        .iter()
        .filter(|p| p.kind == PowerKind::Thievery)
        .map(|p| p.counter.max(0))
        .sum();
    (stolen - state.heist_recovered_gold).max(0)
}

/// HP-equivalent value of the run-level loot this combat is currently on
/// course to LOSE, as a non-negative magnitude. Three terms, all of them
/// things the C# engine settles at `exit_combat` and none of them visible
/// to the HP/potion/turn combat eval:
///
/// * **Stolen cards** — every un-reclaimed [`SwipeStolen`] entry, at
///   [`LOOT_CARD_HP_EQUIV`] each. Killing the thief flips `reclaimed`
///   (C# `SwipePower.BeforeDeath`) and the term drops to zero; letting it
///   escape keeps the card gone for the rest of the run.
/// * **Stolen gold** — [`stolen_gold_outstanding`] at
///   [`LOOT_GOLD_HP_EQUIV`] per gold. Killing the `HeistPower` carrier
///   refunds it (C# `HeistPower.BeforeDeath`); a flee keeps it.
/// * **Escape-shrunk reward gold** — `escaped / total` of
///   [`LOOT_ESCAPE_GOLD_BASE`], mirroring C# `CombatRoom.GoldProportion
///   = 1 - EscapedCreatures.Count / MonstersWithSlots.Count`.
///
/// **Why this is not free-standing eval polish:** an `Intent::Escape`
/// zeroes the fleeing creature's HP (`combat_loop.rs`, bead
/// `sts2-rs-8f336`), so to every HP-shaped term in the combat eval — and
/// to the win terminal itself, since a solo ThievingHopper fleeing ENDS
/// the combat as a win — a flee is indistinguishable from a kill. Without
/// this term the search is genuinely indifferent between killing the
/// Hopper and watching it leave with a card, which is what GH #604
/// observed.
#[must_use]
pub fn loot_at_risk_hp_equiv(state: &CombatState) -> f32 {
    let cards_lost = state.swipe_stolen.iter().filter(|s| !s.reclaimed).count();
    #[allow(clippy::cast_precision_loss)]
    let cards = cards_lost as f32 * LOOT_CARD_HP_EQUIV;

    #[allow(clippy::cast_precision_loss)]
    let gold = stolen_gold_outstanding(state) as f32 * LOOT_GOLD_HP_EQUIV;

    let total_monsters = state.enemy_uids.len();
    #[allow(clippy::cast_precision_loss)]
    let escape = if total_monsters == 0 {
        0.0
    } else {
        (f32::from(u16::try_from(state.escaped_creatures).unwrap_or(u16::MAX))
            / total_monsters as f32)
            .clamp(0.0, 1.0)
            * LOOT_ESCAPE_GOLD_BASE
            * LOOT_GOLD_HP_EQUIV
    };

    cards + gold + escape
}

/// In-progress (depth-cap leaf) player-HP weight — the tuned base
/// [`PLAYER_HP_WEIGHT`] scaled by the swept victory knob [`hp_weight`],
/// mirroring [`in_progress_potion_weight`]'s ratio idiom. The deployed
/// default (`w = 1`) leaves it at exactly `PLAYER_HP_WEIGHT`, keeping the
/// `EVAL_FEATURE_WEIGHTS` dot-product invariant intact. Consistency-only:
/// the 2026-07-18 hard-example campaign showed depth-200 rollouts make
/// the win terminal the load-bearing objective, not the leaf.
fn in_progress_hp_weight() -> f32 {
    PLAYER_HP_WEIGHT * hp_weight()
}

/// Linear weight applied to `player.current_hp / max_hp` in
/// [`evaluate_in_progress`]. From the 2026-05-17 MCTS leaf-eval plan
/// §4.1.4 (bead `sts2-rs-0962`); Texel-tuning is a sibling bead.
const PLAYER_HP_WEIGHT: f32 = 5.121_131;
/// Linear weight applied to the net-incoming-threat term in
/// [`evaluate_in_progress`]. Net incoming = sum over alive non-minion
/// enemies of `intent.damage() * intent.hits()`; the term is
/// `-((net_incoming - block).max(0) / max_hp) * weight`, i.e. only the
/// unblocked portion counts and the worse-case is `-weight` (player
/// at low HP facing full unblocked telegraphed damage). From the
/// 2026-05-17 MCTS leaf-eval plan Wave-1 (bead `sts2-rs-mkn5`); Texel
/// tuning is a sibling bead.
const NET_INCOMING_THREAT_WEIGHT: f32 = 3.949_739_2;
/// Per-stack weights on enemy Vulnerable / Weak in [`evaluate_in_progress`].
/// Vulnerable on enemies is good for the player (their incoming damage
/// gets amplified); the Weak coefficient is negative per the Wave-1
/// bead spec (untuned at this pin — Texel will reconcile sign and
/// magnitude against measured win-rate in a sibling bead). From the
/// 2026-05-17 MCTS leaf-eval plan Wave-1 (bead `sts2-rs-n1ak`).
const ENEMY_VULNERABLE_WEIGHT: f32 = 1.420_992_3;
const ENEMY_WEAK_WEIGHT: f32 = -0.2;
/// Per-stack penalty for enemy Artifact charges in [`evaluate_in_progress`].
/// Artifact absorbs the next incoming debuff (one charge per stack), so
/// the existing `vuln_stacks` / `weak_stacks` features over-credit
/// effective debuff pressure against an Artifacted enemy. This term
/// pulls the eval back toward "wasted debuff" when Artifact is up.
/// bottled_ai audit B-4 (`docs/perf/2026-05-18-bottled-ai-feature-
/// audit.md`). Wave-1 untuned; Texel will reconcile in a sibling
/// tuning pass. Bead `sts2-rs-w022`.
const ENEMY_ARTIFACT_WEIGHT: f32 = -0.5;
/// Per-stack penalty for remaining enemy `Slippery` charges in
/// [`evaluate_in_progress`]. SlipperyPower caps each incoming hit at
/// 1 damage and consumes one stack per hit, so `Vantom HP 181 Slippery
/// 9` and `Vantom HP 181 Slippery 0` are NOT equal positions — the
/// second is strictly better (the next attack lands at full damage).
/// `damage_dealt_raw = (1 - enemy_hp_ratio)` is blind to this (HP is
/// unchanged while the wall holds), so without this term the search
/// sees no progress from methodically chipping the counter down and
/// Q-saturates near the loss floor (potion-spam / tie-break play).
/// Negative-signed: more remaining stacks = worse; clearing them
/// raises the eval monotonically, independent of hand composition —
/// the clean complement to the `time_to_lethal` Slippery term (which
/// is entangled with hand contents). Follow-up to bead `sts2-rs-ldd1c`
/// (which shipped only the `time_to_lethal` half); Wave-1 untuned,
/// Texel reconciles in a sibling pass.
const ENEMY_SLIPPERY_WEIGHT: f32 = -0.5;
/// Per-stack weights on **player** Vulnerable / Weak / Frail in
/// [`evaluate_in_progress`]. Mirror of the enemy-side terms with
/// negative signs: being Vulnerable yourself amplifies incoming
/// damage; being Weak reduces outgoing; Frail halves block gain.
/// bottled_ai audit B-7 (`docs/perf/2026-05-18-bottled-ai-feature-
/// audit.md`) flagged the asymmetry — eval rewards debuffing enemies
/// but is blind to being debuffed. Wave-1 untuned magnitudes; Texel
/// will reconcile in a sibling tuning pass. Bead `sts2-rs-p0zk`.
const PLAYER_VULNERABLE_WEIGHT: f32 = -1.0;
const PLAYER_WEAK_WEIGHT: f32 = -0.5;
const PLAYER_FRAIL_WEIGHT: f32 = -0.5;
/// Weight applied to `block / max_hp` in [`evaluate_in_progress`] **when
/// the player has [`PowerKind::Barricade`] up**. Default eval treats
/// end-of-turn block as turn-local (the threat term only credits block
/// up to the incoming-damage absorption); Barricade carries the full
/// block stack into the next turn, so the surplus has positive
/// expected value that the bare threat term cannot express. Half of
/// `PLAYER_HP_WEIGHT` (untuned) since one block carried is roughly
/// half an HP carried (block decays into HP only if next-turn intent
/// lands and is absorbed). bottled_ai audit B-2; bead `sts2-rs-ymvk`.
const BARRICADE_BLOCK_WEIGHT: f32 = 2.5;
/// Weight applied to `tanh(player_strength / PLAYER_STRENGTH_SCALE)` in
/// [`evaluate_in_progress`]. Bead `sts2-rs-9lrx` (Wave-1 leaf-eval).
/// `tanh` saturates so DemonForm / runaway Strength stacks can't
/// dominate the eval (term bounded in
/// `(-PLAYER_STRENGTH_WEIGHT, +PLAYER_STRENGTH_WEIGHT)`); the `/5`
/// scale keeps typical mid-combat Strength values (3–8) in the steep
/// part of the curve. Texel tuning is a sibling bead.
const PLAYER_STRENGTH_WEIGHT: f32 = 2.0;
const PLAYER_STRENGTH_SCALE: f32 = 5.0;

/// Low-HP cliff threshold + weight for [`evaluate_in_progress`]. Adds
/// `max(0, LOW_HP_CLIFF_THRESHOLD - hp_ratio)^2 * LOW_HP_CLIFF_WEIGHT`
/// — a quadratic ramp that fires only when `hp_ratio` drops below
/// 20%. Steeper than chess-endgame analogues because STS deaths are
/// one-shot: a 5%-HP position is much worse than 20% by more than
/// the linear `PLAYER_HP_WEIGHT * hp_ratio` term can express. From
/// the 2026-05-17 MCTS leaf-eval plan Wave-1 (bead `sts2-rs-9nzs`,
/// §4.1.4); Texel will reconcile the threshold and magnitude.
const LOW_HP_CLIFF_THRESHOLD: f32 = 0.2;
const LOW_HP_CLIFF_WEIGHT: f32 = -20.544_304;
/// Per-unit weights for the lightspeed-ported `energy_wasted` and
/// `cards_drawn` terms in [`evaluate_in_progress`]. Bead `sts2-rs-95o9`
/// (Wave-2 final piece of `docs/plans/2026-05-17-mcts-leaf-eval-and-search-improvements.md`
/// §10): re-introduces the two `evaluateEndState` terms dropped
/// during the original v2oh port because the counters didn't exist
/// yet — `ob7h` landed `energy_wasted`, `o05r` landed `cards_drawn`,
/// this bead wires them in. Magnitudes mirror sts_lightspeed's
/// `BattleScumSearcher2.cpp:431-432` raw scoring (`-0.2` per wasted
/// energy point, `+0.03` per card drawn).
const ENERGY_WASTED_WEIGHT: f32 = 0.631_019_6;
const CARDS_DRAWN_WEIGHT: f32 = 1.025_363_2;
/// Weight applied to the count of `Action::PlayCard` entries returned
/// by [`legal_actions`] at the current state. Higher mobility = more
/// branches the search can explore from this leaf, and empirically
/// correlates with positional flexibility (turn-1 hands with 5
/// playable cards beat hands with 2 playable cards on the same
/// encounter). Magnitude is small (0.05) because the count is
/// unbounded — a 10-card hand at 3 energy can yield ~30+
/// PlayCard actions (per-enemy targeting), and we don't want the
/// term to overwhelm the HP-ratio / threat signals.
///
/// Bead `sts2-rs-ns1s`. Texel will reconcile the weight against the
/// fitted yuyc-corpus optimum in a sibling tuning pass.
const PLAYABLE_CARDS_WEIGHT: f32 = 1.111_439_3;
/// Weight applied to the `time_pressure` leaf-eval term in
/// [`evaluate_in_progress_raw`]. Bead `sts2-rs-ot78` —
/// `time_pressure = -softplus(round - monster_pressure_threshold)`
/// (smoothed by GitHub issue #12 from the original
/// `-max(0, round - monster_pressure_threshold)` step),
/// where the threshold is keyed on the alive enemy species via
/// [`monster_pressure_threshold`] (default `99` ⇒ effectively no
/// pressure; `5` for `MonsterKind::Vantom` so the search learns to
/// finish Vantom before turn 5 escalation). Magnitude `0.6` keeps a
/// turn-8 vs-Vantom hand (`-3 × 0.6 = -1.8` raw) in scale with the
/// HP/threat band after `IN_PROGRESS_DIVISOR` normalization while
/// still moving the policy needle enough to clear the +3pp aggregate
/// gate in `docs/perf/2026-05-17-boss-vantom-ceiling.md`. Texel will
/// reconcile in a sibling tuning pass.
const TIME_PRESSURE_WEIGHT: f32 = 0.6;
/// Weight applied to the differential time-to-lethal term in
/// [`evaluate_in_progress_raw`]. Bead `sts2-rs-m0f2`.
///
/// Pair-companion to `TIME_PRESSURE_WEIGHT`: the bare time-pressure
/// term is *non-differential* (every root-action's rollout reaches
/// the same horizon, so it penalizes uniformly — see
/// `docs/perf/2026-05-17-boss-vantom-ceiling.md` 'Post-ot78 rerun').
/// This term creates root-action spread by reading hand contents:
///
/// ```text
/// attack_potential = Σ_hand attack_card.base_damage * (1 + tanh(strength/5))
/// // Slippery awareness (sts2-rs-ldd1c): cap one hit at 1 damage per
/// // remaining enemy Slippery stack, up to attack_count.
/// avg_hit = attack_potential / attack_count
/// slippery_loss = min(slippery_total, attack_count) * max(0, avg_hit - 1)
/// effective_potential = max(0, attack_potential - slippery_loss)
/// time_to_lethal = total_alive_enemy_hp / max(1.0, effective_potential)
/// time_to_lethal_score = -clamp(time_to_lethal, 0, TIME_TO_LETHAL_MAX) * WEIGHT
/// ```
///
/// PlayCard actions that swap an Attack out of hand for a non-Attack
/// (e.g. Defend) reduce `attack_potential`, raising `time_to_lethal`
/// → larger negative penalty. PlayCard actions that apply Strength
/// or Vulnerable raise `attack_potential` (Strength via the `tanh`
/// multiplier) → smaller penalty. This produces an action-dependent
/// gradient that the bare round-clock term cannot.
///
/// Negative-signed: longer time-to-lethal = worse. Magnitude `0.15`
/// keeps the saturated contribution (`20.0 × 0.15 = 3.0` raw,
/// `≈ 0.10` post-IN_PROGRESS_DIVISOR) on the same scale as the
/// `damage_dealt_raw` and `threat_score` bands. Texel will reconcile.
const TIME_TO_LETHAL_WEIGHT: f32 = 0.15;
/// Clip on the raw `time_to_lethal` ratio before applying the weight.
/// Prevents a low-attack hand against a high-HP boss (e.g. starter
/// deck vs. Vantom 300 HP) from saturating the term and dwarfing the
/// rest of the eval. 20.0 was chosen so a typical 5-Strike hand
/// against Cultist HP=50 (potential ~30, T2L ~1.7) reads as a small
/// signal, while a 0-attack hand against Vantom (potential ~0, T2L
/// clamped to 20) reads as a near-saturated penalty.
const TIME_TO_LETHAL_MAX: f32 = 20.0;

/// Round threshold past which the leaf evaluator's `time_pressure`
/// term goes negative, keyed by the alive monster species. Mirrors
/// the per-encounter `EncounterDef::pressure_threshold` table in
/// `sts2-content` — both surfaces are driven by the same
/// Vantom-aggressive-after-turn-5 finding from
/// `docs/perf/2026-05-17-boss-vantom-ceiling.md`.
///
/// Eval can only see `CombatState` (no encounter id), so the
/// threshold is recomputed each call as the *minimum* across alive
/// non-minion enemy species (tightest pressure dominates). Default
/// `99` is effectively "no pressure". Bead `sts2-rs-ot78`.
#[must_use]
pub const fn monster_pressure_threshold(kind: MonsterKind) -> u16 {
    match kind {
        MonsterKind::Vantom => 5,
        _ => 99,
    }
}

/// Smoothed `time_pressure` signal — replacement for the original
/// `-max(0, round - threshold)` step function (GitHub issue #12).
///
/// The step form returned exactly zero sub-threshold, so MCTS got no
/// gradient that the deadline was approaching until `round` crossed
/// the cliff. This version uses softplus
/// (`softplus(x) = ln(1 + exp(x))`), which:
///   * asymptotes to the original ramp `-(round - threshold)` for
///     `round ≫ threshold` (post-cliff shape preserved, magnitude
///     differs by less than `ln(2) ≈ 0.693` at the threshold itself),
///   * decays smoothly toward zero for `round ≪ threshold`
///     (e.g. at default threshold `99` the sub-threshold contribution
///     is numerically zero for any realistic combat length),
///   * produces a non-zero gradient *before* the cliff on tight
///     thresholds (e.g. Vantom = 5), so a turn-3 leaf already sees
///     a small penalty that grows monotonically into the cliff.
///
/// Numerically-stable evaluation: `softplus(x) = max(x, 0) + ln(1 + exp(-|x|))`
/// to avoid `exp` overflow on large positive `x`.
#[inline]
fn time_pressure_signal(round: u16, threshold: u16) -> f32 {
    let x = f32::from(round) - f32::from(threshold);
    let softplus = x.max(0.0) + (-x.abs()).exp().ln_1p();
    -softplus
}

/// Weight applied to the survival-horizon term in [`evaluate_in_progress_raw`].
/// Bead `sts2-rs-5l4y` — captures "how many turns of unblocked incoming
/// damage can the player absorb right now", computed as
/// `(player.current_hp + player.block) / max(1.0, net_incoming_threat)`
/// and clipped to [`SURVIVAL_HORIZON_MAX`]. The clip prevents a
/// no-intent or low-intent enemy from saturating the term and dwarfing
/// the HP/threat band; weight 0.1 keeps the saturated value
/// (`5.0 × 0.1 = 0.5`) in scale with `LOW_HP_CLIFF_WEIGHT`'s magnitude
/// after `IN_PROGRESS_DIVISOR` normalization. Texel will reconcile both
/// magnitude and clip in a sibling tuning pass.
const SURVIVAL_HORIZON_WEIGHT: f32 = 1.208_790_5;
const SURVIVAL_HORIZON_MAX: f32 = 5.0;

/// Weight applied to the count of curse / status cards currently in
/// `state.player.hand`. The predicate mirrors
/// `sts2_mcts::expert::play_ordering(card_id) >= 60` — the bucket that
/// contains every cycle-only / cycle-and-cost card in the deck
/// (Burn/Decay/Clumsy/Wound/Dazed/Void at priority 60, Regret/Shame/
/// Doubt at 65, and Slimed/Writhe/Parasite/AscendersBane/CurseOfTheBell/
/// Injury/Normality/Pain at 70). Implemented here via
/// [`crate::cards::CardType`] (`Curse | Status`) to avoid an sts2-core →
/// sts2-mcts crate dep — the two predicates are bijective on the
/// classified card set, and any future curse/status card added to the
/// content side picks up this term automatically without an expert.rs
/// edit.
///
/// Negative-signed because each held curse/status occupies a hand slot
/// that *could* have been a playable card. Magnitude (-0.3 per card)
/// keeps the term in scale with the HP / threat band without dominating
/// it. Bead `sts2-rs-gzbh`. Texel will reconcile the magnitude in a
/// sibling tuning pass.
const CURSE_IN_HAND_WEIGHT: f32 = -0.3;
/// Per-card reward for permanent deck purification — Curse / Status
/// cards in [`PlayerCombatState::exhaust`]. Mirrors
/// [`CURSE_IN_HAND_WEIGHT`] for the exhaust pile so the eval
/// differentiates "Wound currently clogging hand" (transient, already
/// penalized via `CURSE_IN_HAND_WEIGHT`) from "Wound exhausted out of
/// the deck rotation" (permanent benefit). bottled_ai audit B-8
/// (`docs/perf/2026-05-18-bottled-ai-feature-audit.md` ranking #7);
/// bead `sts2-rs-mbj0`. Magnitude 0.30 — same scale as the in-hand
/// penalty so an Apotheosis play that exhausts a Wound nets to ~+0.30
/// total (cancelling the hand penalty *and* awarding the permanent
/// removal). Wave-1 untuned; Texel will reconcile.
const EXHAUST_CURSE_REWARD_WEIGHT: f32 = 0.3;
/// Per-tier weights on player-side Power-class buffs in
/// [`evaluate_in_progress`]. Mirrors bottled_ai's `powers_we_love`
/// / `_like` / `_like_less` / `_dislike` partition so the eval sees
/// DemonForm / Barricade / Juggernaut plays — `strength_tanh` alone
/// covers only the Strength axis. Wave-1 untuned; Texel will reconcile
/// once `acbj` rebalances the corpus to include Power-play positions.
/// Slot for `_dislike` is reserved (currently empty in curated table)
/// so the column-order pin stays stable. bottled_ai audit B-6; bead
/// `sts2-rs-kpk5`.
const PLAYER_BUFF_LOVE_WEIGHT: f32 = 3.0;
const PLAYER_BUFF_LIKE_WEIGHT: f32 = 1.5;
const PLAYER_BUFF_MEH_WEIGHT: f32 = 0.5;
const PLAYER_BUFF_DISLIKE_WEIGHT: f32 = -1.0;
/// Penalty for `player.block` accumulated on turns where no alive
/// non-minion enemy telegraphs an Attack/MultiAttack intent **and** the
/// player holds no block-preserving Power (see
/// [`player_has_block_preserving_power`]). Block resets at turn-end in
/// that case, so the points have zero expected value — modeled by
/// 12 strict `defend_no_attack_intent` blunders flagged 2026-05-21.
///
/// Base value is normalized by [`WASTED_BLOCK_NORM`] (= 50) so a
/// 15-block whiff scores `-0.3 * 0.3 = -0.09` raw, in scale with the
/// `curse_in_hand` / `barricade_block` band. Texel will reconcile in
/// a sibling tuning pass (`feedback_texel_train_loss_doesnt_predict_winrate`
/// — A/B via `mcts_ab_value_net` is required before any writeback).
///
/// Block-preserving Powers per the bead body:
/// `{Barricade, Calipers, Juggernaut, Metallicize, Entrench, Plated,
/// BodyBuilder}`. Only `Barricade` / `Juggernaut` / `Metallicize`
/// exist as [`PowerKind`] variants at pin `01c094d`; the rest are
/// content gaps (Calipers/Entrench/PlatedArmor under `bay` Wave-2/3;
/// BodyBuilder under Necrobinder content). Once those land, extend
/// the match arm in [`player_has_block_preserving_power`].
///
/// Bead `sts2-rs-adulv`.
const WASTED_BLOCK_WEIGHT: f32 = -0.3;
const WASTED_BLOCK_NORM: f32 = 50.0;
/// Continuous victory eval in `[0.5, 1.0]`. Ported from sts_lightspeed
/// `evaluateEndState` (`BattleScumSearcher2.cpp:411`), potion price
/// raised to [`VICTORY_POTION_HP_EQUIV`] (= 8; lightspeed used 4):
/// ```text
/// raw = 100 * (35 + player.curHp + potion_count * 8 - turn * 0.01)
/// out = clamp(0.5 + 0.5 * (raw - VICTORY_RAW_MIN) /
///                       (VICTORY_RAW_MAX - VICTORY_RAW_MIN), 0.5, 1.0)
/// ```
/// HP repricing (`STS2_RUNAI_HP_WEIGHT`) does NOT touch this band — the
/// surplus lives in [`victory_hp_surplus`], added by the MCTS win
/// terminals on top of this value.
///
/// Higher remaining HP, more potions held, and earlier wins all push
/// the score up. Energy waste / cards drawn (lightspeed's lesser terms)
/// are not yet tracked on STS2 [`CombatState`] — they default to zero
/// and can be folded in once the per-turn counters land.
#[must_use]
pub fn evaluate_victory(state: &CombatState) -> f32 {
    let hp = state.player.current_hp.max(0) as f32;
    let round = f32::from(state.round);
    let raw = 100.0 * (35.0 + hp + belt_hp_equiv(state) - round * 0.01);
    let norm = 0.5 + 0.5 * (raw - VICTORY_RAW_MIN) / (VICTORY_RAW_MAX - VICTORY_RAW_MIN);
    norm.clamp(0.5, 1.0)
}

/// HP-equivalents → root-Q units at the victory band's exchange rate.
///
/// [`evaluate_victory`] maps `raw = 100·(35 + hp + potions·λ − …)` onto
/// `[0.5, 1.0]`, so one HP-equivalent is worth
/// `0.5·100 / (VICTORY_RAW_MAX − VICTORY_RAW_MIN)` ≈ `0.00357` Q. The
/// root-level potion opportunity-cost charge (GH #447 follow-up) uses
/// this so its tables can be written in interpretable HP units while the
/// MCTS root pick subtracts plain Q.
#[must_use]
pub fn victory_hp_equiv_to_q(hp_equiv: f32) -> f32 {
    hp_equiv * (0.5 * 100.0 / (VICTORY_RAW_MAX - VICTORY_RAW_MIN))
}

/// Continuous in-progress / depth-cap eval in `(-0.5, 0.5)`. Ported
/// from sts_lightspeed `evaluateEndState`'s loss branch
/// (`BattleScumSearcher2.cpp:421`):
/// ```text
/// raw = (1 - enemy_hp_ratio) * 10
///     + alive_score (= -alive_nonminion_enemies)
///     + potion_count * 2
///     + turn * 0.2
/// out = tanh(raw / IN_PROGRESS_DIVISOR)   // soft-saturated, bounded in (-1, 1)
/// ```
///
/// Differences from lightspeed: `energyPenalty` (energy wasted) and
/// `drawBonus` (cards drawn) are not yet tracked on STS2 `CombatState`
/// (deferred to a follow-up bead). The `turn` term keeps its lightspeed
/// sign — surviving longer in a losing fight is a mild positive
/// signal, the opposite of the victory branch's stall penalty.
///
/// Soft-saturated via `tanh` so the Q-gradient survives at the
/// extremes (a near-dead enemy and a fully-dead enemy must not both
/// project to the same +0.9 plateau — GH issue #11). `tanh` is
/// asymptotic to `±1.0` and never reaches it, so the `±1.0` reservation
/// for confirmed terminal outcomes in [`terminal_value`] still holds.
#[must_use]
pub fn evaluate_in_progress(state: &CombatState) -> f32 {
    (evaluate_in_progress_raw(state) / IN_PROGRESS_DIVISOR).tanh()
}

/// Canonical column order for [`eval_features`]. The Python Texel
/// tuning harness (bead `sts2-rs-vsqc.3`) dot-products its trained
/// weight vector against this exact layout, so any reorder MUST be
/// rolled back through the harness header and the corpus generator
/// (`sts2-rs-vsqc.1`) in lockstep. The mapping bead is
/// `sts2-rs-uxyf` (vsqc.2).
///
/// Each entry is the *pre-weight* base value the evaluator multiplies
/// by its named `const f32` constant in `evaluate_in_progress_raw`.
/// Index `i` aligns 1:1 with [`EVAL_FEATURE_WEIGHTS`].
pub const EVAL_FEATURE_NAMES: [&str; 29] = [
    "damage_dealt_raw",      // (1 - enemy_hp_ratio)
    "alive_raw",             // -alive_nonminion_count
    "potions_raw",           // potions.len()
    "round_raw",             // round
    "hp_ratio",              // player.current_hp / max_hp
    "threat_raw",            // -(unblocked / max_hp)
    "vuln_stacks",           // sum enemy Vulnerable amounts
    "weak_stacks",           // sum enemy Weak amounts
    "strength_tanh",         // tanh(player_strength / SCALE)
    "cliff_excess_sq",       // max(0, THRESHOLD - hp_ratio)^2
    "energy_wasted_raw",     // state.energy_wasted (i32 → f32)
    "cards_drawn_raw",       // state.cards_drawn (u16 → f32)
    "playable_cards",        // bead sts2-rs-ns1s: count of Action::PlayCard in legal_actions
    "survival_horizon",      // bead sts2-rs-5l4y: (hp+block)/max(1,net_incoming), clipped [0, 5]
    "curse_in_hand",         // bead sts2-rs-gzbh: count of Curse|Status cards in state.player.hand
    "time_pressure",         // bead sts2-rs-ot78 + GH #12: -softplus(round - threshold)
    "time_to_lethal",        // bead sts2-rs-m0f2: -clamp(enemy_hp/attack_potential, 0, 20)
    "player_vuln_stacks",    // bead sts2-rs-p0zk: sum player Vulnerable amounts
    "player_weak_stacks",    // bead sts2-rs-p0zk: sum player Weak amounts
    "player_frail_stacks",   // bead sts2-rs-p0zk: sum player Frail amounts
    "barricade_block",       // bead sts2-rs-ymvk: block/max_hp when player has Barricade
    "enemy_artifact_stacks", // bead sts2-rs-w022: sum enemy Artifact amounts (penalty for wasted debuff-targeting)
    "exhaust_curse_count", // bead sts2-rs-mbj0: count of Curse|Status cards in state.player.exhaust (permanent deck purification)
    "player_buff_love", // bead sts2-rs-kpk5: sum of player-side LOVE-tier buff amounts (DemonForm/Barricade/Juggernaut/Envenom/EchoForm)
    "player_buff_like", // bead sts2-rs-kpk5: sum of player-side LIKE-tier buff amounts (Metallicize/Thorns/DarkEmbrace/NoxiousFumes/Afterimage/Corruption)
    "player_buff_meh", // bead sts2-rs-kpk5: sum of player-side MEH-tier buff amounts (Blur/Vigor/Buffer/TemporaryStrength/FlameBarrier)
    "player_buff_dislike", // bead sts2-rs-kpk5: sum of player-side DISLIKE-tier buff amounts (slot reserved; empty in Wave-1 curated table)
    "wasted_block", // bead sts2-rs-adulv: player.block / 50 on no-incoming-attack turns when no block-preserving Power held (Barricade/Juggernaut/Metallicize)
    "enemy_slippery_stacks", // ldd1c follow-up: sum remaining enemy Slippery amounts (negative weight; clearing the wall is progress damage_dealt_raw can't see)
];

/// Per-feature `const f32` weights that `evaluate_in_progress_raw`
/// multiplies the [`EVAL_FEATURE_NAMES`] base values by. Exposed so
/// the Texel harness (bead `sts2-rs-vsqc.3`) can sanity-check its
/// numpy re-implementation of the score against this exact layout
/// before any weight write-back (bead `sts2-rs-vsqc.4`).
pub const EVAL_FEATURE_WEIGHTS: [f32; 29] = [
    DAMAGE_DEALT_WEIGHT,   // damage_dealt_raw
    ALIVE_WEIGHT,          // alive_raw
    POTIONS_WEIGHT,        // potions_raw
    ROUND_WEIGHT,          // round_raw
    5.121_131,             // hp_ratio
    3.949_739_2,           // threat_raw
    1.420_992_3,           // vuln_stacks
    -0.2,                  // weak_stacks
    2.0,                   // strength_tanh
    -20.544_304,           // cliff_excess_sq
    0.631_019_6,           // energy_wasted_raw
    1.025_363_2,           // cards_drawn_raw
    1.111_439_3,           // playable_cards
    1.208_790_5,           // survival_horizon
    -0.3,                  // curse_in_hand
    0.6,                   // time_pressure
    0.15,                  // time_to_lethal (bead sts2-rs-m0f2)
    -1.0,                  // player_vuln_stacks (bead sts2-rs-p0zk; Wave-1 untuned)
    -0.5,                  // player_weak_stacks (bead sts2-rs-p0zk; Wave-1 untuned)
    -0.5,                  // player_frail_stacks (bead sts2-rs-p0zk; Wave-1 untuned)
    2.5,                   // barricade_block (bead sts2-rs-ymvk; Wave-1 untuned)
    -0.5,                  // enemy_artifact_stacks (bead sts2-rs-w022; Wave-1 untuned)
    0.3,  // exhaust_curse_count (bead sts2-rs-mbj0; mirrors -CURSE_IN_HAND_WEIGHT magnitude)
    3.0,  // player_buff_love (bead sts2-rs-kpk5; Wave-1 untuned)
    1.5,  // player_buff_like (bead sts2-rs-kpk5; Wave-1 untuned)
    0.5,  // player_buff_meh (bead sts2-rs-kpk5; Wave-1 untuned)
    -1.0, // player_buff_dislike (bead sts2-rs-kpk5; Wave-1 untuned)
    WASTED_BLOCK_WEIGHT, // wasted_block (bead sts2-rs-adulv; Wave-1 untuned)
    ENEMY_SLIPPERY_WEIGHT, // enemy_slippery_stacks (ldd1c follow-up; Wave-1 untuned)
];

/// Pre-weight base values for every term in [`evaluate_in_progress_raw`],
/// in the canonical [`EVAL_FEATURE_NAMES`] column order. Texel-tuning
/// surface (bead `sts2-rs-uxyf`/`vsqc.2`) — the Python harness fits
/// per-feature weights `w` such that
/// `sigmoid(K · features.dot(w)) ≈ eventual_outcome`.
///
/// `evaluate_in_progress_raw` is the same as
/// `eval_features(state).iter().zip(EVAL_FEATURE_WEIGHTS).map(|(f, w)| f * w).sum()`
/// up to f32 rounding; the harness asserts this within 1e-6 on a
/// sanity grid before writing fitted weights back.
///
/// Bead `sts2-rs-4wie`: is the player currently `Intangible`? Used by
/// both eval entry points to clamp telegraphed `net_incoming` to
/// `total_hits` (1 dmg/hit) while the power is up. See B-1 in
/// `docs/perf/2026-05-18-bottled-ai-feature-audit.md`.
fn player_has_intangible(state: &CombatState) -> bool {
    let Some(uid) = state.player.creature_uid else {
        return false;
    };
    let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) else {
        return false;
    };
    c.powers.iter().any(|puid| {
        state
            .powers
            .iter()
            .any(|p| p.id == puid.0 && p.kind == PowerKind::Intangible && p.amount > 0)
    })
}

/// Bead `sts2-rs-ymvk` (bottled_ai audit B-2): is the player currently
/// holding `Barricade` (block carries between turns)? Used to credit
/// end-of-turn block as positive eval rather than turn-local. See B-2
/// in `docs/perf/2026-05-18-bottled-ai-feature-audit.md`.
fn player_has_barricade(state: &CombatState) -> bool {
    let Some(uid) = state.player.creature_uid else {
        return false;
    };
    let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) else {
        return false;
    };
    c.powers.iter().any(|puid| {
        state
            .powers
            .iter()
            .any(|p| p.id == puid.0 && p.kind == PowerKind::Barricade && p.amount > 0)
    })
}

/// Sum the per-instance `amount` for every player Power matching
/// `kind`. Two stacks of Weak 1 + 2 report as `3`, matching the
/// engine's effective-debuff arithmetic. Mirror of the eval_wide
/// helper but reachable from this module for the leaf-eval surface.
/// Bead `sts2-rs-p0zk`.
fn player_debuff_amount(state: &CombatState, kind: PowerKind) -> i32 {
    let Some(uid) = state.player.creature_uid else {
        return 0;
    };
    let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) else {
        return 0;
    };
    c.powers
        .iter()
        .filter_map(|puid| state.powers.get_by_id(puid.0))
        .filter(|p| p.kind == kind)
        .map(|p| p.amount)
        .sum()
}

/// Curated Power-class tier table for bead `sts2-rs-kpk5` (bottled_ai
/// audit B-6). Buckets player-side buff Powers into 4 tiers mirroring
/// bottled_ai's `powers_we_love` / `_like` / `_like_less` / `_dislike`.
/// Debuffs are scored separately via `p0zk` debuff features and are
/// intentionally excluded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BuffTier {
    None,
    Love,
    Like,
    Meh,
    // Reserved per `sts2-rs-kpk5`: no player buff currently maps here, but the
    // feature column + match arm exist so Texel can fit a non-zero weight once
    // a Dislike-tier player buff lands.
    #[allow(dead_code)]
    Dislike,
}

const fn player_buff_tier(kind: PowerKind) -> BuffTier {
    match kind {
        PowerKind::DemonForm
        | PowerKind::Barricade
        | PowerKind::Juggernaut
        | PowerKind::Envenom
        | PowerKind::EchoForm => BuffTier::Love,
        PowerKind::Metallicize
        | PowerKind::Thorns
        | PowerKind::DarkEmbrace
        | PowerKind::NoxiousFumes
        | PowerKind::Afterimage
        | PowerKind::Corruption => BuffTier::Like,
        PowerKind::Blur
        | PowerKind::Vigor
        | PowerKind::Buffer
        | PowerKind::TemporaryStrength
        | PowerKind::FlameBarrier => BuffTier::Meh,
        _ => BuffTier::None,
    }
}

/// Sum the per-instance `amount` for every player-Creature Power
/// whose kind falls into each `BuffTier`. Returns `[love, like, meh,
/// dislike]` summed amounts (not counts) so `DemonForm 3` reads as
/// `+3` to the LOVE tier, matching the additive nature of stack
/// counts on these Powers. Bead `sts2-rs-kpk5`.
fn player_buff_tier_amounts(state: &CombatState) -> [i32; 4] {
    let mut out = [0i32; 4];
    let Some(uid) = state.player.creature_uid else {
        return out;
    };
    let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) else {
        return out;
    };
    for puid in &c.powers {
        let Some(p) = state.powers.get_by_id(puid.0) else {
            continue;
        };
        match player_buff_tier(p.kind) {
            BuffTier::Love => out[0] = out[0].saturating_add(p.amount),
            BuffTier::Like => out[1] = out[1].saturating_add(p.amount),
            BuffTier::Meh => out[2] = out[2].saturating_add(p.amount),
            BuffTier::Dislike => out[3] = out[3].saturating_add(p.amount),
            BuffTier::None => {}
        }
    }
    out
}

/// Bead `sts2-rs-adulv`: does any alive non-minion enemy telegraph an
/// Attack or MultiAttack intent? Drives the `wasted_block` leaf-eval
/// feature — block applied on a no-attack-intent turn is pure waste
/// unless a block-preserving Power carries it forward.
fn any_enemy_attack_intent(state: &CombatState) -> bool {
    for uid in &state.enemy_uids {
        let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) else {
            continue;
        };
        if c.current_hp <= 0 {
            continue;
        }
        let is_minion = c.powers.iter().any(|puid| {
            state
                .powers
                .iter()
                .any(|p| p.id == puid.0 && p.kind == PowerKind::Minion)
        });
        if is_minion {
            continue;
        }
        if let Some(intent) = c.intent {
            if intent.damage() > 0 && intent.hits() > 0 {
                return true;
            }
        }
    }
    false
}

/// Bead `sts2-rs-adulv`: does the player hold any Power that preserves
/// block across the turn boundary? See `WASTED_BLOCK_WEIGHT` for the
/// canonical bead-body list; only `Barricade` / `Juggernaut` /
/// `Metallicize` exist as `PowerKind` variants at pin `01c094d`.
fn player_has_block_preserving_power(state: &CombatState) -> bool {
    let Some(uid) = state.player.creature_uid else {
        return false;
    };
    let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) else {
        return false;
    };
    c.powers.iter().any(|puid| {
        state.powers.iter().any(|p| {
            p.id == puid.0
                && p.amount > 0
                && matches!(
                    p.kind,
                    PowerKind::Barricade | PowerKind::Juggernaut | PowerKind::Metallicize
                )
        })
    })
}

#[must_use]
pub fn eval_features(state: &CombatState) -> [f32; 29] {
    let (enemy_cur, enemy_max, alive) = nonminion_enemy_hp_totals(state);
    let enemy_hp_ratio = if enemy_max > 0 {
        enemy_cur as f32 / enemy_max as f32
    } else {
        0.0
    };
    let max_hp = state.player.max_hp.max(1) as f32;
    let hp_ratio = state.player.current_hp.max(0) as f32 / max_hp;

    let mut net_incoming: i32 = 0;
    let mut total_hits: i32 = 0;
    let mut enemy_vuln_stacks: i32 = 0;
    let mut enemy_weak_stacks: i32 = 0;
    let mut enemy_artifact_stacks: i32 = 0;
    let mut enemy_slippery_stacks: i32 = 0;
    for uid in &state.enemy_uids {
        let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) else {
            continue;
        };
        if c.current_hp <= 0 {
            continue;
        }
        let mut is_minion = false;
        for puid in &c.powers {
            if let Some(p) = state.powers.get_by_id(puid.0) {
                match p.kind {
                    PowerKind::Minion => is_minion = true,
                    PowerKind::Vulnerable => {
                        enemy_vuln_stacks = enemy_vuln_stacks.saturating_add(p.amount);
                    }
                    PowerKind::Weak => {
                        enemy_weak_stacks = enemy_weak_stacks.saturating_add(p.amount);
                    }
                    // bead sts2-rs-w022 (bottled_ai audit B-4): Artifact
                    // absorbs incoming debuffs one charge per stack. The
                    // term is negative-weighted so the eval discounts
                    // positions where the existing vuln_stacks/weak_stacks
                    // credit would be wasted against an Artifacted enemy.
                    PowerKind::Artifact => {
                        enemy_artifact_stacks = enemy_artifact_stacks.saturating_add(p.amount);
                    }
                    // ldd1c follow-up: remaining Slippery stacks each cap
                    // an incoming hit at 1 dmg. Negative-weighted so the
                    // eval rewards chipping the counter down — progress
                    // `damage_dealt_raw` is blind to while the wall holds.
                    PowerKind::Slippery => {
                        enemy_slippery_stacks = enemy_slippery_stacks.saturating_add(p.amount);
                    }
                    _ => {}
                }
            }
        }
        if !is_minion {
            if let Some(intent) = c.intent {
                // Bead `sts2-rs-2u5fq`: saturating_mul guards against
                // pathological intent values (damage * hits > i32::MAX)
                // wrapping to a negative number that would then
                // saturating_add into a smaller net_incoming than
                // truthful. Realistic STS2 intents stay well under
                // the wrap threshold (damage ~5-60, hits 1-3), but
                // fuzz-corrupted fixtures or future high-magnitude
                // intent variants could trigger the silent wrap.
                let dmg = intent.damage().saturating_mul(i32::from(intent.hits()));
                net_incoming = net_incoming.saturating_add(dmg);
                total_hits = total_hits.saturating_add(i32::from(intent.hits()));
            }
        }
    }
    // Bead `sts2-rs-4wie` (bottled_ai audit B-1): cap incoming damage
    // at `total_hits` (1 dmg/hit) while player Intangible is up.
    // bottled_ai's `incoming_damage` accessor models Intangible the
    // same way (see `bottled_ai/rs/common/comparators/core/
    // assessment.py::incoming_damage` and `docs/perf/2026-05-18-
    // bottled-ai-feature-audit.md` §3 B-1). Without this cap,
    // `threat_raw = -(unblocked / max_hp)` over-penalizes telegraphed
    // damage by 5-30x while the player has bought a turn of
    // Intangible — every leaf eval downstream of a `PlayCard:
    // Apparition` / `PlayCard: Touch of Insanity` reads the same
    // pessimism as if the player had no defense at all.
    let net_incoming_after_intangible = if player_has_intangible(state) {
        net_incoming.min(total_hits)
    } else {
        net_incoming
    };
    let unblocked = (net_incoming_after_intangible - state.player.block).max(0) as f32;

    let mut player_strength: i32 = 0;
    if let Some(uid) = state.player.creature_uid {
        if let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) {
            for puid in &c.powers {
                if let Some(p) = state.powers.get_by_id(puid.0) {
                    if p.kind == PowerKind::Strength {
                        player_strength = player_strength.saturating_add(p.amount);
                    }
                }
            }
        }
    }
    let strength_tanh = (player_strength as f32 / PLAYER_STRENGTH_SCALE).tanh();

    let cliff_excess = (LOW_HP_CLIFF_THRESHOLD - hp_ratio).max(0.0);
    let cliff_excess_sq = cliff_excess * cliff_excess;

    // Bead `sts2-rs-ns1s`: mobility — count of `Action::PlayCard`
    // entries from `legal_actions` at this state. Targeted cards
    // contribute one entry per alive enemy, so a high-mobility hand
    // (e.g. 4 cards × 3 alive enemies = 12) reads as 12 here; the
    // 0.05 weight keeps the term in scale with the HP/threat band.
    let playable_cards = legal_actions(state)
        .iter()
        .filter(|a| matches!(a, Action::PlayCard { .. }))
        .count() as f32;
    // Bead `sts2-rs-5l4y`: survival horizon — turns of unblocked
    // incoming damage the player can absorb. Reuses the `net_incoming`
    // / `unblocked` computation above for parity with
    // `evaluate_in_progress_raw`.
    let effective_hp = (state.player.current_hp.max(0) + state.player.block.max(0)) as f32;
    let survival_horizon =
        (effective_hp / (net_incoming.max(1) as f32)).clamp(0.0, SURVIVAL_HORIZON_MAX);
    // Bead `sts2-rs-gzbh`: curses / status cards held in hand. Predicate
    // is `card_type ∈ {Curse, Status}` against the arena lookup, which is
    // bijective with `sts2_mcts::expert::play_ordering(card_id) >= 60`
    // on the classified card set.
    let curse_in_hand = state
        .player
        .hand
        .iter()
        .filter(|uid| {
            state
                .cards
                .iter()
                .find(|c| c.uid == **uid)
                .is_some_and(|c| {
                    matches!(
                        c.card_type,
                        crate::cards::CardType::Curse | crate::cards::CardType::Status
                    )
                })
        })
        .count() as f32;
    // Bead `sts2-rs-ot78`: time-pressure term. Look up the tightest
    // pressure threshold across alive non-minion enemies via
    // [`monster_pressure_threshold`] and emit a negative ramp once
    // `round` exceeds it. Default `99` ⇒ zero contribution on
    // pressure-free encounters.
    let pressure_threshold = state_pressure_threshold(state);
    let time_pressure = time_pressure_signal(state.round, pressure_threshold);
    // Bead `sts2-rs-m0f2`: differential time-to-lethal estimate.
    // Negative-signed so longer T2L → lower eval. Hand-based, so
    // PlayCard actions that change hand composition (and the
    // strength/vulnerable contexts) produce action-dependent spread
    // at the depth-10 leaf — the differential signal the bare
    // time_pressure term lacks.
    let time_to_lethal = -time_to_lethal_estimate(state);
    // Bead `sts2-rs-p0zk` (bottled_ai audit B-7): player-side debuff
    // stacks. Mirrors the enemy `vuln_stacks` / `weak_stacks` features
    // onto the player creature, plus Frail (no enemy-side analogue
    // since enemies don't block). Negative-signed weights live in
    // `PLAYER_VULNERABLE_WEIGHT` / `PLAYER_WEAK_WEIGHT` /
    // `PLAYER_FRAIL_WEIGHT`.
    let player_vuln_stacks = player_debuff_amount(state, PowerKind::Vulnerable);
    let player_weak_stacks = player_debuff_amount(state, PowerKind::Weak);
    let player_frail_stacks = player_debuff_amount(state, PowerKind::Frail);
    // Bead `sts2-rs-ymvk` (bottled_ai audit B-2): when player has
    // Barricade, end-of-turn block carries over and is positive eval.
    // Without Barricade the threat term already prices block against
    // current-turn incoming damage; this term only fires when the
    // surplus has next-turn value.
    let barricade_block = if player_has_barricade(state) {
        state.player.block.max(0) as f32 / max_hp
    } else {
        0.0
    };
    // Bead `sts2-rs-mbj0` (bottled_ai audit B-8): count Curse / Status
    // cards in the player's exhaust pile. Mirrors `curse_in_hand` but
    // for the permanently-removed-from-rotation half of the
    // purification picture.
    let exhaust_curse_count = state
        .player
        .exhaust
        .iter()
        .filter(|uid| {
            state
                .cards
                .iter()
                .find(|c| c.uid == **uid)
                .is_some_and(|c| {
                    matches!(
                        c.card_type,
                        crate::cards::CardType::Curse | crate::cards::CardType::Status
                    )
                })
        })
        .count() as f32;
    // Bead `sts2-rs-kpk5` (bottled_ai audit B-6): tiered player-side
    // buff counters. Mirrors `eval_features` for the
    // `evaluate_in_progress_raw` side.
    let buff_tier_amounts = player_buff_tier_amounts(state);
    // Bead `sts2-rs-adulv`: wasted-block penalty. Block normalised by
    // `WASTED_BLOCK_NORM` (= 50) and gated on no enemy attack intent
    // **and** no block-preserving Power. Zero on either guard so the
    // term doesn't leak into "defend prophylactically before a buff
    // turn while Barricade is up" positions where carried block has
    // real next-turn value (already credited via `barricade_block`).
    let wasted_block =
        if !any_enemy_attack_intent(state) && !player_has_block_preserving_power(state) {
            state.player.block.max(0) as f32 / WASTED_BLOCK_NORM
        } else {
            0.0
        };
    [
        1.0 - enemy_hp_ratio,
        -(alive as f32),
        state.player.potions.len() as f32,
        f32::from(state.round),
        hp_ratio,
        -(unblocked / max_hp),
        enemy_vuln_stacks as f32,
        enemy_weak_stacks as f32,
        strength_tanh,
        cliff_excess_sq,
        state.energy_wasted as f32,
        f32::from(state.cards_drawn),
        playable_cards,
        survival_horizon,
        curse_in_hand,
        time_pressure,
        time_to_lethal,
        player_vuln_stacks as f32,
        player_weak_stacks as f32,
        player_frail_stacks as f32,
        barricade_block,
        enemy_artifact_stacks as f32,
        exhaust_curse_count,
        buff_tier_amounts[0] as f32,
        buff_tier_amounts[1] as f32,
        buff_tier_amounts[2] as f32,
        buff_tier_amounts[3] as f32,
        wasted_block,
        enemy_slippery_stacks as f32,
    ]
}

/// Differential time-to-lethal estimate consumed by the
/// `time_to_lethal` leaf-eval term. Bead `sts2-rs-m0f2`,
/// Slippery-awareness extension `sts2-rs-ldd1c`.
///
/// Returns `enemy_hp_total / max(1.0, effective_attack_potential)`,
/// clipped to `[0, TIME_TO_LETHAL_MAX]`. `attack_potential` sums
/// `base_damage` across hand Attack cards, scaled by
/// `(1 + tanh(strength/5))` — the same saturating Strength
/// multiplier the `strength_score` term uses. Vulnerable on any
/// alive enemy amplifies the potential by a flat 1.5× factor (the
/// term is a cheap heuristic, not a precise damage simulator).
/// Hits-multiplier on multi-hit cards is approximated as 1 — the
/// small undercount is acceptable for the gradient signal this
/// term provides.
///
/// Slippery awareness (`sts2-rs-ldd1c`): each remaining
/// `PowerKind::Slippery` stack on an alive non-minion enemy caps
/// one incoming hit at 1 damage (per `SlipperyPower.cs`). We sum
/// `slippery_total` across all such enemies and subtract
/// `min(slippery_total, attack_count) * max(0, avg_hit - 1)` from
/// `attack_potential`, where `avg_hit = attack_potential /
/// attack_count`. This makes the eval prefer states that have
/// chipped the Slippery counter down even when raw HP is unchanged
/// (Vantom's 9-stack opener is the motivating case).
///
/// Returns 0.0 when no enemies are alive (no time pressure).
fn time_to_lethal_estimate(state: &CombatState) -> f32 {
    let mut enemy_hp_total: i32 = 0;
    let mut any_vulnerable = false;
    let mut slippery_total: i32 = 0;
    for uid in &state.enemy_uids {
        let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) else {
            continue;
        };
        if c.current_hp <= 0 {
            continue;
        }
        let mut is_minion = false;
        let mut creature_slippery: i32 = 0;
        for puid in &c.powers {
            if let Some(p) = state.powers.get_by_id(puid.0) {
                match p.kind {
                    PowerKind::Minion => is_minion = true,
                    PowerKind::Vulnerable => any_vulnerable = true,
                    PowerKind::Slippery => {
                        creature_slippery = creature_slippery.saturating_add(p.amount.max(0));
                    }
                    _ => {}
                }
            }
        }
        if !is_minion {
            enemy_hp_total = enemy_hp_total.saturating_add(c.current_hp);
            slippery_total = slippery_total.saturating_add(creature_slippery);
        }
    }
    if enemy_hp_total <= 0 {
        return 0.0;
    }
    let mut player_strength: i32 = 0;
    if let Some(uid) = state.player.creature_uid {
        if let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) {
            for puid in &c.powers {
                if let Some(p) = state.powers.get_by_id(puid.0) {
                    if p.kind == PowerKind::Strength {
                        player_strength = player_strength.saturating_add(p.amount);
                    }
                }
            }
        }
    }
    let strength_mult = 1.0 + (player_strength as f32 / PLAYER_STRENGTH_SCALE).tanh();
    let vuln_mult = if any_vulnerable { 1.5_f32 } else { 1.0 };
    let mut attack_potential: f32 = 0.0;
    let mut attack_count: i32 = 0;
    for uid in &state.player.hand {
        let Some(card) = state.cards.iter().find(|c| c.uid == *uid) else {
            continue;
        };
        if card.card_type != crate::cards::CardType::Attack {
            continue;
        }
        if card.base_damage <= 0 {
            continue;
        }
        attack_potential += card.base_damage as f32 * strength_mult * vuln_mult;
        attack_count = attack_count.saturating_add(1);
    }
    if slippery_total > 0 && attack_count > 0 {
        let avg_hit = attack_potential / attack_count as f32;
        let effective_slippery = slippery_total.min(attack_count) as f32;
        let slippery_loss = effective_slippery * (avg_hit - 1.0).max(0.0);
        attack_potential = (attack_potential - slippery_loss).max(0.0);
    }
    let t2l = (enemy_hp_total as f32) / attack_potential.max(1.0);
    t2l.clamp(0.0, TIME_TO_LETHAL_MAX)
}

/// Tightest [`monster_pressure_threshold`] across alive non-minion
/// enemies on `state`. Default `99` when no enemy carries a custom
/// threshold (or all enemies are dead / minions). Bead `sts2-rs-ot78`.
fn state_pressure_threshold(state: &CombatState) -> u16 {
    let mut threshold: u16 = 99;
    for uid in &state.enemy_uids {
        let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) else {
            continue;
        };
        if c.current_hp <= 0 {
            continue;
        }
        let is_minion = c.powers.iter().any(|puid| {
            state
                .powers
                .iter()
                .any(|p| p.id == puid.0 && p.kind == PowerKind::Minion)
        });
        if is_minion {
            continue;
        }
        if let Some(species) = c.species {
            let t = monster_pressure_threshold(species);
            if t < threshold {
                threshold = t;
            }
        }
    }
    threshold
}

/// Raw, undivided, unclamped pre-normalization score consumed by
/// [`evaluate_in_progress`]. Exposed for bead `sts2-rs-zzpb` empirical
/// rescaling (`crates/sts2-bench/examples/eval_rescale.rs`) — the
/// example samples this across a synthetic mid-combat corpus and
/// reports `p1` / `p99` so `IN_PROGRESS_DIVISOR` can be refit against
/// the post-Wave-1/Wave-2 leaf-eval shape.
#[must_use]
pub fn evaluate_in_progress_raw(state: &CombatState) -> f32 {
    let (enemy_cur, enemy_max, alive) = nonminion_enemy_hp_totals(state);
    let enemy_hp_ratio = if enemy_max > 0 {
        enemy_cur as f32 / enemy_max as f32
    } else {
        0.0
    };
    let damage_dealt = (1.0 - enemy_hp_ratio) * DAMAGE_DEALT_WEIGHT;
    // eval_features[1] = -(alive as f32); dot = -(alive) * ALIVE_WEIGHT.
    let alive_score = -(alive as f32) * ALIVE_WEIGHT;
    let round = f32::from(state.round);
    // bead sts2-rs-0962: player_hp_ratio term closes the "5 HP and 80 HP
    // look the same" leaf-eval compression. Weight 4.0 from plan §4.1.4
    // (`docs/plans/2026-05-17-mcts-leaf-eval-and-search-improvements.md`);
    // Texel tuning is a separate sibling bead.
    let max_hp = state.player.max_hp.max(1) as f32;
    let hp_ratio = state.player.current_hp.max(0) as f32 / max_hp;
    let player_hp_score = hp_ratio * in_progress_hp_weight();
    // bead sts2-rs-mkn5 (Wave-1 leaf-eval): penalize unblocked
    // telegraphed damage. `net_incoming` sums `intent.damage() *
    // intent.hits()` across alive non-minion enemies (matches the
    // `nonminion_enemy_hp_totals` predicate above so the same set of
    // enemies that contribute to `enemy_hp_ratio` also contributes to
    // threat). Subtracting `player.block` clamps at zero so block can
    // fully absorb the telegraph but never grants positive eval.
    let mut net_incoming: i32 = 0;
    let mut total_hits: i32 = 0;
    for uid in &state.enemy_uids {
        let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) else {
            continue;
        };
        if c.current_hp <= 0 {
            continue;
        }
        let is_minion = c.powers.iter().any(|puid| {
            state
                .powers
                .iter()
                .any(|p| p.id == puid.0 && p.kind == PowerKind::Minion)
        });
        if is_minion {
            continue;
        }
        if let Some(intent) = c.intent {
            // Bead `sts2-rs-2u5fq`: saturating_mul mirrors the same
            // hardening applied in the `eval_features` walk above
            // (see that block's comment for rationale).
            let dmg = intent.damage().saturating_mul(i32::from(intent.hits()));
            net_incoming = net_incoming.saturating_add(dmg);
            total_hits = total_hits.saturating_add(i32::from(intent.hits()));
        }
    }
    // Bead `sts2-rs-4wie` (bottled_ai audit B-1): see the
    // `eval_features` mirror above for the rationale. Both functions
    // share the same `unblocked` semantics by construction (verified
    // by `eval_features_dot_weights_matches_evaluate_in_progress_raw`)
    // so the cap must land in lockstep.
    let net_incoming_after_intangible = if player_has_intangible(state) {
        net_incoming.min(total_hits)
    } else {
        net_incoming
    };
    let unblocked = (net_incoming_after_intangible - state.player.block).max(0) as f32;
    let threat_score = -(unblocked / max_hp) * NET_INCOMING_THREAT_WEIGHT;
    // bead sts2-rs-n1ak (Wave-1 leaf-eval): enemy debuff stacks. Sums
    // Vulnerable and Weak amounts across alive enemies (minions
    // included — Vulnerable on a minion still alters its damage-taken
    // arithmetic). Weights are Wave-1 untuned; Texel will reconcile.
    let mut enemy_vuln_stacks: i32 = 0;
    let mut enemy_weak_stacks: i32 = 0;
    let mut enemy_artifact_stacks: i32 = 0;
    let mut enemy_slippery_stacks: i32 = 0;
    for uid in &state.enemy_uids {
        let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) else {
            continue;
        };
        if c.current_hp <= 0 {
            continue;
        }
        for puid in &c.powers {
            if let Some(p) = state.powers.get_by_id(puid.0) {
                match p.kind {
                    PowerKind::Vulnerable => {
                        enemy_vuln_stacks = enemy_vuln_stacks.saturating_add(p.amount);
                    }
                    PowerKind::Weak => {
                        enemy_weak_stacks = enemy_weak_stacks.saturating_add(p.amount);
                    }
                    // bead sts2-rs-w022 (bottled_ai audit B-4): mirror
                    // the Artifact accumulator from `eval_features` so
                    // the raw evaluator and the column-dot match.
                    PowerKind::Artifact => {
                        enemy_artifact_stacks = enemy_artifact_stacks.saturating_add(p.amount);
                    }
                    // ldd1c follow-up: mirror the Slippery accumulator
                    // from `eval_features` so raw == column-dot.
                    PowerKind::Slippery => {
                        enemy_slippery_stacks = enemy_slippery_stacks.saturating_add(p.amount);
                    }
                    _ => {}
                }
            }
        }
    }
    let vuln_score = enemy_vuln_stacks as f32 * ENEMY_VULNERABLE_WEIGHT;
    let weak_score = enemy_weak_stacks as f32 * ENEMY_WEAK_WEIGHT;
    let enemy_artifact_score = enemy_artifact_stacks as f32 * ENEMY_ARTIFACT_WEIGHT;
    let enemy_slippery_score = enemy_slippery_stacks as f32 * ENEMY_SLIPPERY_WEIGHT;
    // bead sts2-rs-9nzs (Wave-1 leaf-eval): low-HP cliff nonlinearity.
    // Quadratic ramp fires only below the threshold so high-HP states
    // are unaffected; magnitude steep because STS deaths are one-shot.
    let cliff_excess = (LOW_HP_CLIFF_THRESHOLD - hp_ratio).max(0.0);
    let cliff_score = cliff_excess * cliff_excess * LOW_HP_CLIFF_WEIGHT;
    // bead sts2-rs-9lrx (Wave-1 leaf-eval): player Strength buff,
    // tanh-saturated. Sum every Power on the player's mirror Creature
    // whose kind == Strength. `tanh(x/5)` hits ~0.76 at Strength=5,
    // ~0.96 at Strength=10, ~0.999 at Strength=20 — bounded so
    // DemonForm runaway can't dominate the eval the way a linear term
    // would.
    let mut player_strength: i32 = 0;
    if let Some(uid) = state.player.creature_uid {
        if let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) {
            for puid in &c.powers {
                if let Some(p) = state.powers.get_by_id(puid.0) {
                    if p.kind == PowerKind::Strength {
                        player_strength = player_strength.saturating_add(p.amount);
                    }
                }
            }
        }
    }
    let strength_score =
        (player_strength as f32 / PLAYER_STRENGTH_SCALE).tanh() * PLAYER_STRENGTH_WEIGHT;
    // bead sts2-rs-95o9 (Wave-2 final piece): lightspeed-ported
    // energy-wasted / cards-drawn terms. Both counters are populated
    // by sister beads (`ob7h` resets + increments `energy_wasted` at
    // end-of-turn; `o05r` increments `cards_drawn` in
    // `draw_into_hand`); this is the consuming surface.
    let energy_wasted_score = state.energy_wasted as f32 * ENERGY_WASTED_WEIGHT;
    let cards_drawn_score = f32::from(state.cards_drawn) * CARDS_DRAWN_WEIGHT;
    // bead sts2-rs-ns1s: mobility term. Count of `Action::PlayCard`
    // entries from `legal_actions` — higher mobility correlates with
    // positional flexibility (turn-1 hands with 5 playable cards
    // outperform 2-playable hands on the same encounter). The 0.05
    // weight keeps the term in scale with HP/threat (a 10-action
    // root contributes only +0.5 to the raw score; ~+0.02 after
    // IN_PROGRESS_DIVISOR).
    let playable_cards = legal_actions(state)
        .iter()
        .filter(|a| matches!(a, Action::PlayCard { .. }))
        .count() as f32;
    let playable_cards_score = playable_cards * PLAYABLE_CARDS_WEIGHT;
    // bead sts2-rs-5l4y: survival horizon. Effective HP divided by
    // net_incoming gives "turns of unblocked incoming the player can
    // absorb right now"; clipped to [0, SURVIVAL_HORIZON_MAX=5] so a
    // no-intent / low-intent enemy doesn't saturate the term and
    // overwhelm the HP/threat band. `net_incoming.max(1)` keeps the
    // ratio finite when no enemy is telegraphing damage.
    let effective_hp = (state.player.current_hp.max(0) + state.player.block.max(0)) as f32;
    let survival_horizon =
        (effective_hp / (net_incoming.max(1) as f32)).clamp(0.0, SURVIVAL_HORIZON_MAX);
    let survival_horizon_score = survival_horizon * SURVIVAL_HORIZON_WEIGHT;
    // Bead `sts2-rs-gzbh`: penalty for curses / status cards currently
    // in the player's hand. Walks `state.player.hand`, looks up each
    // CardUid in the arena, and counts entries whose `card_type` is
    // `Curse` or `Status` (bijective with `expert::play_ordering >= 60`
    // — see `CURSE_IN_HAND_WEIGHT` doc).
    let curse_in_hand = state
        .player
        .hand
        .iter()
        .filter(|uid| {
            state
                .cards
                .iter()
                .find(|c| c.uid == **uid)
                .is_some_and(|c| {
                    matches!(
                        c.card_type,
                        crate::cards::CardType::Curse | crate::cards::CardType::Status
                    )
                })
        })
        .count() as f32;
    let curse_in_hand_score = curse_in_hand * CURSE_IN_HAND_WEIGHT;
    // bead sts2-rs-ot78 + GitHub issue #12: smoothed (softplus) time-
    // pressure penalty keyed on a per-encounter round threshold. Default
    // 99 = effectively zero contribution at realistic combat lengths;
    // Vantom sets 5 so a 7-round Vantom fight scores about
    // `-softplus(2) * TIME_PRESSURE_WEIGHT ≈ -2.13 * 0.6` here, and
    // sub-threshold rounds get a small non-zero gradient toward the
    // cliff (which the original step-function form did not provide).
    let pressure_threshold = state_pressure_threshold(state);
    let time_pressure = time_pressure_signal(state.round, pressure_threshold);
    let time_pressure_score = time_pressure * TIME_PRESSURE_WEIGHT;
    // bead sts2-rs-m0f2: differential time-to-lethal estimate.
    // Hand-content driven: PlayCard actions that drop attack cards
    // from hand (or add Strength/Vulnerable) shift T2L, producing
    // root-action Q-gap spread that the bare time_pressure term
    // cannot. Negative-signed: longer T2L is worse for the player.
    let time_to_lethal_score = -time_to_lethal_estimate(state) * TIME_TO_LETHAL_WEIGHT;
    // bead sts2-rs-p0zk (bottled_ai audit B-7): player-side debuff
    // stacks. Mirror of `vuln_score`/`weak_score` onto the player
    // creature, plus Frail (block-side). Weights live in
    // `PLAYER_VULNERABLE_WEIGHT` / `PLAYER_WEAK_WEIGHT` /
    // `PLAYER_FRAIL_WEIGHT` — all negative-signed so being debuffed
    // lowers the eval.
    let player_vuln_stacks = player_debuff_amount(state, PowerKind::Vulnerable);
    let player_weak_stacks = player_debuff_amount(state, PowerKind::Weak);
    let player_frail_stacks = player_debuff_amount(state, PowerKind::Frail);
    let player_vuln_score = player_vuln_stacks as f32 * PLAYER_VULNERABLE_WEIGHT;
    let player_weak_score = player_weak_stacks as f32 * PLAYER_WEAK_WEIGHT;
    let player_frail_score = player_frail_stacks as f32 * PLAYER_FRAIL_WEIGHT;
    // bead sts2-rs-ymvk (bottled_ai audit B-2): Barricade carries
    // block between turns, so end-of-turn block has next-turn value
    // not captured by the threat term. Mirror of `eval_features`.
    let barricade_block_score = if player_has_barricade(state) {
        (state.player.block.max(0) as f32 / max_hp) * BARRICADE_BLOCK_WEIGHT
    } else {
        0.0
    };
    // bead sts2-rs-mbj0 (bottled_ai audit B-8): permanent deck-
    // purification reward. Counts Curse / Status cards in the
    // player's exhaust pile — the mirror of `curse_in_hand` for the
    // permanently-removed-from-rotation half of the picture. Plays
    // that exhaust a curse net to ~+0.30 (cancelling the hand
    // penalty *and* awarding the permanent removal).
    let exhaust_curse_count = state
        .player
        .exhaust
        .iter()
        .filter(|uid| {
            state
                .cards
                .iter()
                .find(|c| c.uid == **uid)
                .is_some_and(|c| {
                    matches!(
                        c.card_type,
                        crate::cards::CardType::Curse | crate::cards::CardType::Status
                    )
                })
        })
        .count() as f32;
    let exhaust_curse_score = exhaust_curse_count * EXHAUST_CURSE_REWARD_WEIGHT;
    // bead sts2-rs-kpk5 (bottled_ai audit B-6): tiered player-side buff
    // counters. Mirror of `eval_features` so the dot-product invariant
    // (`features.dot(EVAL_FEATURE_WEIGHTS) == evaluate_in_progress_raw`)
    // holds.
    let buff_tier_amounts = player_buff_tier_amounts(state);
    let buff_love_score = buff_tier_amounts[0] as f32 * PLAYER_BUFF_LOVE_WEIGHT;
    let buff_like_score = buff_tier_amounts[1] as f32 * PLAYER_BUFF_LIKE_WEIGHT;
    let buff_meh_score = buff_tier_amounts[2] as f32 * PLAYER_BUFF_MEH_WEIGHT;
    let buff_dislike_score = buff_tier_amounts[3] as f32 * PLAYER_BUFF_DISLIKE_WEIGHT;
    // bead sts2-rs-adulv: wasted-block penalty mirroring the
    // `eval_features` branch so `features.dot(EVAL_FEATURE_WEIGHTS) ==
    // evaluate_in_progress_raw` (pinned by
    // `eval_features_dot_weights_matches_evaluate_in_progress_raw`).
    let wasted_block =
        if !any_enemy_attack_intent(state) && !player_has_block_preserving_power(state) {
            state.player.block.max(0) as f32 / WASTED_BLOCK_NORM
        } else {
            0.0
        };
    let wasted_block_score = wasted_block * WASTED_BLOCK_WEIGHT;
    damage_dealt
        + alive_score
        // issue #253: potion term scaled off the swept victory knob via
        // in_progress_potion_weight() (default == POTIONS_WEIGHT = 2.0, so the
        // EVAL_FEATURE_WEIGHTS dot-product invariant holds when the env var is
        // unset). Narrow leaf is live only under `eval-strong` / `STS2_RUNAI_LEAF=texel`.
        + in_progress_belt_term(state)
        + round * ROUND_WEIGHT
        + player_hp_score
        + threat_score
        + vuln_score
        + weak_score
        + strength_score
        + cliff_score
        + energy_wasted_score
        + cards_drawn_score
        + playable_cards_score
        + survival_horizon_score
        + curse_in_hand_score
        + time_pressure_score
        + time_to_lethal_score
        + player_vuln_score
        + player_weak_score
        + player_frail_score
        + barricade_block_score
        + enemy_artifact_score
        + enemy_slippery_score
        + exhaust_curse_score
        + buff_love_score
        + buff_like_score
        + buff_meh_score
        + buff_dislike_score
        + wasted_block_score
}

/// MCTS reward from the **player's** perspective.
///   * `(true,  v)` — combat over, player won. `v ∈ [0.5, 1.0]` per
///     [`evaluate_victory`]: higher HP + potions kept + faster win
///     score higher.
///   * `(true, -1.0)` — combat over, player lost. Flat — there is no
///     gradient on losses (lightspeed's loss eval is only used for
///     in-progress positions; once the player is dead, the loss is
///     total).
///   * `(false, 0.0)` — combat still in progress.
///
/// Bead `sts2-rs-v2oh`: replaced the binary `±1.0` reward with the
/// sts_lightspeed-derived continuous formula so MCTS can distinguish
/// "win at 1 HP" from "win at 80 HP" in the value backup, fixing the
/// "Blood Wall ranks above Strike on free turns" pathology where
/// the old reward gave both lines identical Q.
#[must_use]
pub fn terminal_value(state: &CombatState) -> (bool, f32) {
    if state.is_over {
        if state.player_won {
            (true, evaluate_victory(state))
        } else {
            (true, -1.0)
        }
    } else {
        (false, 0.0)
    }
}

/// Bead `sts2-rs-722vx-A`: cards whose entire value is "buff every
/// subsequent attack this turn" AND that cost 0 energy. Playing them
/// first when attacks are queued is unconditionally correct (delay
/// leaves block / damage on the table; playing first costs nothing).
/// Keep this list narrow — only add when the play-first rule is
/// risk-free (cost == 0 AND effect applies to *subsequent* actions
/// in the SAME turn). Energy-cost buffs (DemonForm 3, Inflame 1) are
/// situational and intentionally excluded.
#[must_use]
fn is_zero_cost_buff_prior(card_id: cards::CardId) -> bool {
    matches!(
        card_id,
        // Rage: +3 block per attack played this turn. Apex of the
        // "play me first" pattern.
        cards::CardId::RageCard
    )
    // v0.107.1: DrumOfBattle was reworked from a 0-cost Power into a 1-cost
    // Skill (draw 2; gain energy on exhaust), so it no longer fits the
    // zero-cost-buff-prior pattern and was dropped from this list.
}

/// Enumerate every action that is legal in the current state.
///
/// Branches on `pending_choice` first: when a choice is open, **only**
/// `Action::ResolveChoice` variants are legal (mirrors the Python
/// `select_card` short-circuit in `combat.legal_actions`).
///
/// Otherwise, in `PlayerPlayPhase`:
///   * `Action::PlayCard` for every hand index whose card cost ≤ energy
///     and whose target requirements can be satisfied. Targeted enemy
///     cards generate one action per alive enemy slot.
///   * `Action::DrinkPotion` for every potion slot except
///     character-locked (wrong character) and `PotionUsage.Automatic`
///     slots (target is `None` for non-targeted potions — Phase 7 will
///     refine the target predicate).
///     Never the legacy `Action::UsePotion` alias, which no producer in
///     this crate emits (asserted by
///     `legal_actions_emits_drink_potion_per_slot`).
///   * `Action::EndTurn` is always legal.
///
/// Outside `PlayerPlayPhase`, only `Action::EndTurn` is returned as the
/// trivial fallback; in practice MCTS callers should not be querying
/// non-play phases, but a non-empty list keeps rollout loops well-formed.
/// Base star cost per card — C# `CardModel.CanonicalStarCost` (default
/// `-1` → effective `max(0, -1) == 0`, i.e. star-payable by anyone). Only
/// the 21 Regent star cards override it (`MegaCrit.Sts2.Core.Models.Cards/
/// {SevenStars,Alignment,…}.cs`, v0.103.2). Consumed by the
/// [`legal_actions`] star-cost gate, mirroring C#
/// `PlayerCombatState.HasEnoughResourcesFor`'s `StarCostTooHigh`. Star-cost
/// *modifiers* (`GetStarCostWithModifiers`) are Regent-relic-specific and
/// not modelled here; the base cost suffices to gate a star-less Ironclad
/// (the only star-card surface in the IC sweep — generated Regent Attacks).
/// Bead `sts2-rs-starcost`.
const fn card_base_star_cost(id: crate::cards::CardId) -> i32 {
    use crate::cards::CardId;
    match id {
        CardId::SevenStars => 7,
        CardId::DecisionsDecisions => 6,
        CardId::Comet | CardId::NeutronAegis | CardId::RoyalGamble => 5,
        CardId::Devastate | CardId::TheSmith => 4,
        CardId::Alignment
        | CardId::AstralPulse
        | CardId::DyingStar
        | CardId::GammaBlast
        | CardId::ReflectCard
        | CardId::Resonance
        | CardId::TheSealedThrone => 3,
        CardId::FallingStar
        | CardId::GuidingStar
        | CardId::MeteorShower
        | CardId::ParticleWall
        | CardId::Quasar => 2,
        CardId::CloakOfStars | CardId::CrescentSpear => 1,
        _ => 0,
    }
}

/// Effective star cost of a specific card instance — `card_base_star_cost`
/// unless the card has been made free. C# `CardModel.SetToFreeThisTurn()`
/// zeros BOTH energy and star cost (`SetStarCostThisTurn(0)`), and
/// `SetToFreeThisCombat()` does the same with combat lifetime
/// (`SetStarCostThisCombat(0)`, `CardModel.cs:978`/`984`). The Rust port
/// represents the energy side by stamping `cost = 0` directly, but the star
/// side is computed from the static `card_base_star_cost`, so it must consult
/// the per-instance free markers to match: `FreeThisTurn` (Splash /
/// InfernalBlade generated cards) and `StarCostFreeThisCombat` (TouchOfInsanity's
/// `SetToFreeThisCombat`). Without this, a TouchOfInsanity-freed Regent star
/// card (e.g. DyingStar) stays gated at its base star cost — Rust lists it
/// unplayable while C# offers it (parity-fuzz Regent seeds 230/320/405).
#[inline]
pub fn effective_star_cost(card: &crate::cards::Card) -> i32 {
    // Perf 2026-07-04: resolve the const base cost FIRST. All but the 21
    // Regent star cards have base 0, where the free-marker branch is
    // irrelevant (0 either way) — skipping the two `combat_vars` scans on
    // this per-hand-card `legal_actions` path. Return values are identical
    // for every input: base 0 → 0 regardless of markers; base > 0 → 0 iff
    // a free marker is set, else base. Same pure function, reordered.
    let base = card_base_star_cost(card.card_id);
    if base == 0 {
        return 0;
    }
    if card.combat_var(crate::cards::VarKey::FreeThisTurn) != 0
        || card.combat_var(crate::cards::VarKey::StarCostFreeThisCombat) != 0
    {
        return 0;
    }
    base
}

/// Star cost of `card` folding in DYNAMIC in-combat modifiers the static
/// [`effective_star_cost`] can't see.
///
/// At this pin the only dynamic modifier is `VoidFormPower`. C#
/// `VoidFormPower.TryModifyStarCost` zeroes the star cost of the first
/// `Amount` cards the owner plays each turn — the EXACT mirror of its
/// `TryModifyEnergyCostInCombat` (which the energy path already honors via
/// `hooks::…VoidForm → void_form_cost_override`). The static
/// `effective_star_cost` only knows the per-card free markers, so a
/// VoidForm-freed star card (ASTRAL_PULSE / RESONANCE) stayed gated at its
/// base star cost: Rust listed it unplayable + would have over-spent stars
/// while C# offers it for 0 stars (Regent fuzz seeds 12/15/44/382/419/432,
/// surfaced once the VoidForm end-turn fix advanced play into the next turn).
///
/// Counter timing matches the energy path and C#: the tally counts only PRIOR
/// plays at both the legal gate and the play-time spend (C# increments
/// `Data.cardsPlayedThisTurn` in `AfterCardPlayed`, after the cost is
/// computed), so the card currently being evaluated is the `played`-th free
/// card. GH #363 A2: that tally is VoidForm's OWN instance counter, not
/// `count_player_cards_played_this_turn` — see
/// [`crate::combat_loop::void_form_cards_played_this_turn`]. Keeping the two
/// cost paths on the same source is what stops the star gate and the energy
/// gate from disagreeing after an auto-play.
#[must_use]
pub fn effective_star_cost_in_combat(state: &CombatState, card: &crate::cards::Card) -> i32 {
    let base = effective_star_cost(card);
    if base == 0 {
        return 0;
    }
    // C# `BrilliantScarf.TryModifyStarCost`: the fifth card is free in stars too.
    if crate::hooks::brilliant_scarf_makes_free(state, card, card.uid) {
        return 0;
    }
    let vf_amount = state.player_power_amount(crate::powers::PowerKind::VoidForm);
    if vf_amount > 0 {
        let played = crate::combat_loop::void_form_cards_played_this_turn(state);
        if crate::powers::regent::void_form_cost_override(vf_amount, played, true).is_some() {
            return 0;
        }
    }
    base
}

#[must_use]
pub fn legal_actions(state: &CombatState) -> SmallVec<[Action; 16]> {
    let mut out: SmallVec<[Action; 16]> = SmallVec::new();
    if state.is_over {
        return out;
    }
    if let Some(pc) = state.pending_choice.as_ref() {
        // Bead sts2-rs-419n: enumerate one `ResolveChoice` per legal
        // selection. For single-pick choices (Headbutt's `PutBack`),
        // that's one entry per option. For variable-pick choices with
        // `can_skip`, an empty selection is also legal. Multi-pick
        // (max_select > 1) combinatorial enumeration is gated to single-
        // pick at this pin to avoid a 2^N explosion — sub-beads will
        // land Forethought / Ashwater fan-outs when their resolvers are
        // wired. If the option list is empty (e.g. discard-empty
        // Headbutt), emit a single empty selection so the rollout can
        // advance past the choice (Python auto-resolves empty choices
        // by short-circuiting before opening one; we mirror that here).
        if pc.options.is_empty() || pc.max_select == 0 {
            out.push(Action::ResolveChoice {
                selection: SmallVec::new(),
            });
        } else if pc.max_select == 1 {
            #[allow(clippy::cast_possible_truncation)]
            for i in 0..pc.options.len() {
                let mut sel: SmallVec<[u8; 4]> = SmallVec::new();
                sel.push(i as u8);
                out.push(Action::ResolveChoice { selection: sel });
            }
            if pc.can_skip || pc.min_select == 0 {
                out.push(Action::ResolveChoice {
                    selection: SmallVec::new(),
                });
            }
        } else {
            // Multi-pick (`max_select > 1`). Bead `sts2-rs-sopgf`:
            // SEQUENTIAL single-pick decomposition so MCTS can choose
            // *which* cards a multi-pick adds (Dredge "add up to N discard
            // cards"), without a `C(n,k)` subset enumeration. Each node
            // branches at most N ways (one incremental pick per still-
            // unpicked option) and the tree is at most K deep. Progress is
            // carried in `pc.picked` (the ORIGINAL option indices chosen so
            // far); `options` is never mutated, so indices stay stable.
            //
            // Each emitted incremental `ResolveChoice { [i] }` appends `i`
            // to `picked` and RE-OPENS the same choice (no effect fires); a
            // FINALIZE `ResolveChoice { [] }` runs the per-`ChoiceKind`
            // resolver ONCE over the full accumulated set. See
            // [`resolve_choice_action`] for the accumulation/finalize split
            // and [`resolve_choice_atomic`] for the parity-safe one-shot
            // path the replay/oracle drivers use.
            let picked_n = pc.picked.len();
            let max_n = usize::from(pc.max_select);
            if picked_n >= max_n {
                // Reached the cap: FINALIZE is the ONLY legal action —
                // no further picks (the `+1 == max_select` incremental
                // path already auto-finalized, so this is the defensive
                // floor for a choice re-opened exactly at `max_select`).
                out.push(Action::ResolveChoice {
                    selection: SmallVec::new(),
                });
            } else {
                // FINALIZE is legal once the minimum is satisfied (the
                // player may stop early), OR when zero picks already
                // satisfy a skippable choice and nothing is picked yet.
                if picked_n >= usize::from(pc.min_select)
                    || ((pc.can_skip || pc.min_select == 0) && picked_n == 0)
                {
                    out.push(Action::ResolveChoice {
                        selection: SmallVec::new(),
                    });
                }
                // One incremental pick per option NOT already in `picked`,
                // in ascending option order (so the deterministic drainers
                // accumulate `[0, 1, …, min-1]` — byte-identical to the
                // prior "first-K" selection).
                #[allow(clippy::cast_possible_truncation)]
                for i in 0..pc.options.len() {
                    let idx = i as u8;
                    if pc.picked.contains(&idx) {
                        continue;
                    }
                    let mut sel: SmallVec<[u8; 4]> = SmallVec::new();
                    sel.push(idx);
                    out.push(Action::ResolveChoice { selection: sel });
                }
            }
        }
        return out;
    }
    if state.phase != CombatPhase::PlayerPlayPhase {
        out.push(Action::EndTurn);
        return out;
    }

    // Player card plays.
    let alive_enemies: SmallVec<[u8; 8]> = state
        .enemy_uids
        .iter()
        .enumerate()
        .filter_map(|(i, uid)| {
            let alive = creature_by_id(&state.creatures, uid.0).is_some_and(|c| c.current_hp > 0);
            #[allow(clippy::cast_possible_truncation)]
            alive.then_some(i as u8)
        })
        .collect();

    // Bead `sts2-rs-fa3w`: curse-bodies-umbrella scope reduced from
    // "20 missing bodies" to two play-gating ports — Enthralled +
    // Normality. Audit-of-audits matrix at the bead close-out shows
    // 6 curses already wired via the turn-end-in-hand dispatcher
    // (`execute_turn_end_in_hand_effect` — sts2-rs-6ynt.3.1) and 12
    // curses with no combat-side trigger in the Python pin at
    // `01c094d` (dead-weight / run-side / keyword-handled /
    // upstream-sim TODO). The remaining two are play-gating, not
    // card effects — mirror Python `combat.py:755-764`:
    //   * Enthralled in hand → blocks playing any other card (the
    //     Enthralled card itself is still playable, costing 2 energy
    //     to clear the gate).
    //   * Normality in hand + ≥3 cards already played this turn →
    //     blocks further plays.
    // Bead `sts2-rs-722vx-A` + perf 2026-07-04: single fused hand walk.
    // One pass resolves each hand card once and simultaneously (a) splits
    // the hand into the 0-cost-buff-prior "early" prefix and the rest —
    // preserving the original emission order exactly — and (b) detects
    // Enthralled / Normality in hand (previously two extra hand walks).
    // The resolved `&Card` is carried alongside so the emission loop
    // below doesn't re-index `state.cards` per card. Rationale for the
    // early/rest split: 0-cost cards that buff *all subsequent attacks
    // this turn* are strictly correct to lead with when attacks are
    // queued — playing them later leaves block / damage on the table.
    // Scope is deliberately narrow: only unconditionally-correct-first
    // 0-cost buffs, never energy-cost buffs (DemonForm 3, Inflame 1 are
    // situational).
    let mut enthralled_in_hand = false;
    let mut normality_in_hand = false;
    // Perf 2026-07-04: carry the hand index (as the `u8` the emitted action
    // needs anyway) + `CardUid` + resolved `&Card` so the emission loop
    // below neither re-indexes `state.player.hand` nor `state.cards` per
    // card. Unresolvable uids (defensive `state.cards.get` miss) are
    // dropped here — the old emission loop `continue`d on them, emitting
    // nothing, so the output is identical. The two partitions stay
    // SEPARATE buffers walked early-then-rest via `chain` — the previous
    // concat-and-move shape memcpy'd a ~400-byte inline SmallVec up to
    // twice per call (block-move + extend), the single largest self-cost
    // in `legal_actions` per callgrind.
    type HandEntry<'a> = (u8, CardUid, &'a cards::Card);
    let mut hand_early: SmallVec<[HandEntry<'_>; 16]> = SmallVec::new();
    let mut hand_rest: SmallVec<[HandEntry<'_>; 16]> = SmallVec::new();
    let mut enemy_target_n = 0usize;
    for (hi, card_uid) in state.player.hand.iter().enumerate() {
        let idx = card_uid.0.get() as usize - 1;
        let Some(card) = state.cards.get(idx) else {
            continue;
        };
        enthralled_in_hand |= card.card_id == cards::CardId::Enthralled;
        normality_in_hand |= card.card_id == cards::CardId::Normality;
        enemy_target_n += usize::from(card.target_type == cards::TargetType::Enemy);
        #[allow(clippy::cast_possible_truncation)]
        let hi_u8 = hi as u8;
        if is_zero_cost_buff_prior(card.card_id) {
            hand_early.push((hi_u8, *card_uid, card));
        } else {
            hand_rest.push((hi_u8, *card_uid, card));
        }
    }
    let normality_threshold_reached =
        normality_in_hand && state.count_player_cards_played_this_turn(None) >= 3;
    // Bead `sts2-rs-ybtmu`: mirror the `play_card` SlothPower gate
    // (C# `SlothPower.ShouldPlay`) so the legal-action surface agrees with
    // play resolution — once the player has played `Amount` cards this turn,
    // no card is playable. Same shape as Normality but the cap is the power's
    // amount (KnowledgeDemon curse option 1, counter 1) instead of a fixed 3.
    // Perf 2026-07-04: the `present_kinds` bitset test in front of each
    // power-gate scan below is a pure early-out — the presence set is a
    // superset of live kinds (see `PowerKindSet`), so "not present"
    // provably means the scan would find nothing. `legal_actions` runs
    // once per rollout step, and Sloth / ChainsOfBinding / Ringing are
    // almost never live.
    // (Single-kind gates use `contains` — one word test — rather than an
    // 8-word `intersects` against a one-kind mask; same absence proof.)
    let sloth_cap_reached = state.powers.present_kinds().contains(PowerKind::Sloth)
        && state
            .player
            .powers
            .iter()
            .find_map(|puid| {
                state
                    .powers
                    .get_by_id(puid.0)
                    .filter(|p| p.kind == crate::PowerKind::Sloth)
                    .map(|p| p.amount)
            })
            .is_some_and(|amt| {
                state.count_player_cards_played_this_turn(None) >= amt.max(0) as u32
            });

    // Bead `sts2-rs-ybtmu`: mirror the `play_card` ChainsOfBindingPower
    // gate (C# `ChainsOfBindingPower.ShouldPlay`,
    // `MegaCrit.Sts2.Core.Models.Powers/ChainsOfBindingPower.cs` v0.103.2)
    // so the legal-action surface agrees with play resolution. Once a
    // `Bound`-afflicted card has been played this turn
    // (`internalData.boundCardPlayed == true`, tracked here as
    // `chains_bound_card_played_this_turn`), every further `Bound` card
    // is unplayable — `ShouldPlay` returns false for them. Queen's
    // PUPPET_STRINGS_MOVE applies ChainsOfBinding(3), which afflicts
    // drawn cards with `Bound`, so without this filter `legal_actions`
    // would list a second Bound card as playable while `play_card`
    // (combat_loop.rs:3068-3092) rejects it — a LEGAL_ACTION divergence.
    // Gated on the player actually owning the power so a stale flag can't
    // strand plays after Chains expires mid-turn, matching `play_card`.
    let chains_bound_blocks = state.combat_log.chains_bound_card_played_this_turn
        && state
            .powers
            .present_kinds()
            .contains(PowerKind::ChainsOfBinding)
        && state.player.powers.iter().any(|puid| {
            state
                .powers
                .iter()
                .any(|p| p.id == puid.0 && p.kind == crate::PowerKind::ChainsOfBinding)
        });

    // Bead `sts2-rs-xk9yo`: mirror the `play_card` RingingPower gate
    // (C# `RingingPower.ShouldPlay`) so the legal-action surface agrees with
    // play resolution. Ringing afflicts every owned card; once the player has
    // started any card play this turn, no further card is playable — net, one
    // card per turn. Gated on the player actually owning the power so a stale
    // marker can't strand plays after Ringing expires at turn end.
    let ringing_blocks = state.powers.present_kinds().contains(PowerKind::Ringing)
        && state.count_player_cards_played_this_turn(None) >= 1
        && state.player.powers.iter().any(|puid| {
            state
                .powers
                .iter()
                .any(|p| p.id == puid.0 && p.kind == crate::PowerKind::Ringing)
        });

    // Mirror the `play_card` SmoggyPower gate (C# `SmoggyPower.ShouldPlay`,
    // `MegaCrit.Sts2.Core.Models.Powers/SmoggyPower.cs:64-71` v0.107.1) so the
    // legal-action surface agrees with play resolution. `SmoggyPower
    // .AfterCardPlayed` smogs every unafflicted player Skill the moment a Skill
    // is played, so from the second Skill onward each is unplayable — the live
    // game reports them `playable:false` / `unplayable_reason:"BlockedByHook"`.
    // Without this filter MCTS plans multi-Skill turns the wire then rejects
    // (LivingFog ADVANCED_GAS; live capture 2026-08-03). Read through
    // `player_power_amount` (the mirror+legacy union, with its own O(1)
    // `present_kinds` early-out) rather than the legacy-only scans above,
    // because both Smoggy appliers — ADVANCED_GAS and the wire reconstruction —
    // write the legacy surface while cards/relics would write the mirror.
    let smoggy_blocks = state.player_power_amount(PowerKind::Smoggy) > 0;

    // GH #742: mirror the `play_card` relic `ShouldPlay` gate (C#
    // `VelvetChoker.ShouldPlay`, six card plays per turn). Card-independent,
    // so it blocks every card once reached.
    let relic_blocks = hooks::relic_should_play_blocked(state);

    // Perf 2026-07-04: reserve the emission upper bound up front so the
    // SmallVec never regrows mid-loop (a full hand of Enemy-target cards ×
    // several alive enemies spills past the 16-slot inline buffer and used
    // to reallocate more than once). Only `TargetType::Enemy` cards fan out
    // per enemy (the Shiv+FanOfKnives override only shrinks that to one),
    // so bound those by the enemy count and everything else by one action
    // each — a hand-of-16 all-cards bound would heap-spill the 16-slot
    // inline buffer on multi-enemy fights whose actual output fits inline.
    // Capacity is unobservable — the produced actions and their order are
    // unchanged.
    let hand_n = hand_early.len() + hand_rest.len();
    out.reserve(
        enemy_target_n * alive_enemies.len().max(1)
            + (hand_n - enemy_target_n)
            + state.player.potions.len()
            + 1,
    );

    // Perf 2026-08-18: the modify_card_cost listener-presence mask cannot
    // change between hand cards, so evaluate it once per enumeration and
    // hand it to the per-card cost calls below.
    let cost_mods_present = hooks::modify_card_cost_listeners_present(state);

    for (hi_u8, card_uid, card) in hand_early.iter().copied().chain(hand_rest.iter().copied()) {
        // Bead `sts2-rs-8ukqd` (parity-fuzz R4): gate affordability on the
        // EFFECTIVE cost (after cost-modifier listeners), not the raw stored
        // `card.cost`. Otherwise TANGLED's `+1`-on-Attacks surcharge (and any
        // other `modify_card_cost` rider) is ignored by the legal-action
        // gate, so Rust lists attacks as playable that C# correctly rejects
        // on energy. `effective_card_cost` folds in FreeThisTurn + the
        // modify_card_cost walk, matching the energy actually spent on play.
        if i32::from(state.player.energy)
            < hooks::effective_card_cost_inst_with_mask(state, card, card_uid, cost_mods_present)
        {
            continue;
        }
        // Bead `sts2-rs-starcost`: star-cost gate. C#
        // `PlayerCombatState.HasEnoughResourcesFor` computes
        // `starCost = max(0, card.GetStarCostWithModifiers())` and sets
        // `StarCostTooHigh` when `starCost > Stars` (PlayerCombatState.cs:154,
        // 165). The 21 Regent cards with a `CanonicalStarCost` override are
        // therefore unplayable by a player who lacks the Stars — e.g. a
        // 0-star Ironclad who generated a Regent Attack via AttackPotion
        // (coverage sweep: SEVEN_STARS seed 1171, ASTRAL_PULSE seed 3123).
        // All other cards have `CanonicalStarCost == -1` → effective 0,
        // always star-payable.
        //
        // Free exemptions: C# `CardModel.SetToFreeThisTurn()` does BOTH
        // `EnergyCost.SetThisTurnOrUntilPlayed(0)` AND `SetStarCostThisTurn(0)`
        // (CardModel.cs), so a Splash-generated star card (stamped
        // `VarKey::FreeThisTurn`) costs 0 stars THIS turn and IS playable.
        // `SetToFreeThisCombat()` (TouchOfInsanity) zeros the star cost for the
        // whole combat (`VarKey::StarCostFreeThisCombat`). `effective_star_cost`
        // folds in both markers (else seed 1171's turn-1 Splash SEVEN_STARS, or
        // a TouchOfInsanity-freed DyingStar in Regent seeds 230/320/405, would
        // be wrongly suppressed).
        if effective_star_cost_in_combat(state, card) > i32::from(state.player.stars) {
            continue;
        }
        // Bead `sts2-rs-vir3o`: skip UNPLAYABLE-keyword cards
        // (AscendersBane and other curses/statuses with cost=-1). The
        // energy<cost gate above lets cost=-1 cards pass since energy
        // is non-negative, so we need an explicit playability check.
        // Bead `sts2-rs-59atd`: `card_unplayable_inst` honors an
        // upgrade-aware `keyword_override` before the CardId registry.
        if card_effects::card_unplayable_inst(card) {
            continue;
        }
        // q6bix F-E: `AnyAlly`-target cards (spec-collapsed to `Self_`)
        // are unplayable when the player has no living ally — C#
        // `CardModel.CanPlay` → `NoLivingAllies` when
        // `PlayerCreatures.Count(alive) <= 1`.
        if card_effects::card_is_any_ally(card.card_id) && !state.has_any_ally_card_target() {
            continue;
        }
        // v0.107.1: PactsEnd is ALWAYS playable. The v0.103.2 `IsPlayable`
        // requirement (≥3 cards in exhaust) was removed; the 3-exhaust gate now
        // only governs whether the play DEALS damage (`CanDealDamage`, handled
        // in `play_pacts_end`), not whether it is a legal play (`PactsEnd.cs`).
        // Bead `sts2-rs-bombardment-handraw`-adjacent.
        // Bead `sts2-rs-6diaj`: Clash's C# `IsPlayable` requires EVERY
        // card in hand to be an Attack (`Clash.cs:20`,
        // `GetCards(Owner, Hand).All(c => c.Type == CardType.Attack)`,
        // v0.103.2). Clash itself is an Attack, so it never disqualifies
        // itself; any non-Attack in hand (Skill / Power / Status / Curse)
        // blocks the play. Without this filter Rust lists Clash as
        // playable while C# rejects it whenever a non-Attack shares the
        // hand — a `play_card`-gating LEGAL_ACTION divergence. The same
        // gate lands in `play_card` (combat_loop.rs) so legal-action
        // enumeration and play resolution agree.
        if card.card_id == cards::CardId::Clash
            && !state.player.hand.iter().all(|uid| {
                state
                    .cards
                    .get(uid.0.get() as usize - 1)
                    .is_some_and(|c| c.card_type == cards::CardType::Attack)
            })
        {
            continue;
        }
        // Bead `sts2-rs-6diaj`: GrandFinale's C# `IsPlayable` requires the
        // draw pile to be EMPTY (`GrandFinale.cs:17`,
        // `PileType.Draw.GetPile(Owner).Cards.Count == 0`, v0.103.2).
        // With any card left in the draw pile it is unplayable. Mirrors
        // `play_card` so the surfaces agree.
        if card.card_id == cards::CardId::GrandFinale && !state.player.draw.is_empty() {
            continue;
        }
        // HighFive's C# `IsPlayable => !base.Owner.IsOstyMissing`
        // (HighFive.cs:18) — the OstyAttack is unplayable while the
        // Necrobinder's Osty is missing or dead (C# `IsOstyMissing =>
        // !(Osty?.IsAlive ?? false)`, Player.cs:91-93). `osty_dealer_ref`
        // returns `Some` only for a present, alive Osty, so its `None` is
        // exactly `IsOstyMissing`. Other OstyAttack cards only glow red without
        // an Osty (`ShouldGlowRedInternal`) but stay playable — HighFive is the
        // sole card that overrides `IsPlayable`. Without this gate Rust offered
        // HighFive with no Osty while C# rejected it (Necrobinder sweep
        // `LEGAL_ACTION` cluster).
        if card.card_id == cards::CardId::HighFive && card_effects::osty_dealer_ref(state).is_none()
        {
            continue;
        }
        // Bead `sts2-rs-fa3w`: skip every non-Enthralled card while
        // Enthralled sits in hand. Enthralled itself must remain
        // playable so the player can clear the gate.
        if enthralled_in_hand && card.card_id != cards::CardId::Enthralled {
            continue;
        }
        // Bead `sts2-rs-fa3w`: skip every card once Normality is in
        // hand and the 3-plays-per-turn cap is reached.
        if normality_threshold_reached {
            continue;
        }
        // Bead `sts2-rs-ybtmu`: skip every card once the SlothPower
        // per-turn play cap is reached (mirrors the `play_card` gate).
        if sloth_cap_reached || relic_blocks {
            continue;
        }
        // Bead `sts2-rs-xk9yo`: skip every card once the player owns Ringing
        // and has already played a card this turn (mirrors `play_card`).
        if ringing_blocks {
            continue;
        }
        // Bead `sts2-rs-ybtmu`: skip a `Bound`-afflicted card once a
        // Bound card has already been played this turn while the player
        // owns ChainsOfBinding (mirrors `play_card`'s ShouldPlay gate).
        if chains_bound_blocks
            && matches!(
                card.affliction,
                Some(crate::card_afflictions::CardAffliction {
                    kind: crate::card_afflictions::CardAfflictionKind::Bound,
                    ..
                })
            )
        {
            continue;
        }
        // Skip a `Smog`-afflicted card while the player owns SmoggyPower
        // (mirrors `play_card`'s ShouldPlay gate — see `smoggy_blocks` above).
        if smoggy_blocks
            && matches!(
                card.affliction,
                Some(crate::card_afflictions::CardAffliction {
                    kind: crate::card_afflictions::CardAfflictionKind::Smog,
                    ..
                })
            )
        {
            continue;
        }
        // Shiv's C# `TargetType` is dynamic — `AnyEnemy` by default but
        // `AllEnemies` while the player has FanOfKnives (Shiv.cs:23-32), under
        // which the Shiv auto-hits every enemy. RunSimulator gates targeting on
        // `needsTarget = TargetType == AnyEnemy`, so a FanOfKnives Shiv surfaces
        // ONE untargeted action rather than one per enemy. The spec stores the
        // static `Enemy`, so fold in the dynamic override here (and in
        // `play_shiv`, which resolves the matching AoE). Shared predicate —
        // GH #335 (see `cards::effective_target_type`).
        let effective_target = cards::effective_target_type(state, card);
        match effective_target {
            cards::TargetType::Enemy => {
                if alive_enemies.is_empty() {
                    // No live enemy — a pending-revive corpse (TestSubject
                    // `Adaptable`, bead `sts2-rs-jnf4a`) is keeping combat
                    // open until its next enemy turn. Do NOT surface this
                    // `Enemy`-target card: every Rust `Enemy` handler resolves
                    // `target.expect("…: Enemy-targeted")`, so `play_card`
                    // rejects a `target: None` `Enemy` play up front
                    // (`StepResult::InvalidTarget`, combat_loop.rs ~4274)
                    // WITHOUT mutating state. Offering it anyway produces a
                    // legal action that resolution always no-ops — a
                    // `legal_actions`/`play_card` divergence that livelocks a
                    // deterministic policy: it re-picks the no-op forever and
                    // the run-hash watchdog trips with `final_phase=Combat`
                    // (bead `sts2-rs-e5bsp`, GH #43; repro seeds 130853117 /
                    // 1635992940). EndTurn (always emitted below) is the real
                    // progress action in this state — it advances to the enemy
                    // turn where the corpse revives. The non-`Enemy` cards in
                    // hand (block/self/AoE, which DO handle an empty board) are
                    // still surfaced by their own arms. C# diverges here (it
                    // offers the card untargeted because its handlers tolerate a
                    // null target — see `tests/parity/known_divergences.md`); a
                    // Rust port of that needs every `Enemy` handler to accept
                    // `None`, which is out of scope for the livelock fix.
                } else {
                    for ei in &alive_enemies {
                        out.push(Action::PlayCard {
                            hand_index: hi_u8,
                            target: Some(*ei),
                        });
                    }
                }
            }
            cards::TargetType::Self_ | cards::TargetType::None => {
                out.push(Action::PlayCard {
                    hand_index: hi_u8,
                    target: None,
                });
            }
            cards::TargetType::AllEnemies | cards::TargetType::Random => {
                // No targeting choice surfaced; engine resolves the
                // distribution internally.
                out.push(Action::PlayCard {
                    hand_index: hi_u8,
                    target: None,
                });
            }
        }
    }

    // Potions. Bead sts2-rs-13q (P7-6) emits Action::DrinkPotion per
    // occupied slot in `state.player.potions`, gated by the per-potion
    // `character_lock` predicate. Targeted-potion predicate (per-enemy
    // fan-out for `AnyEnemy` potions like FirePotion) is deferred to
    // P7-8 / `sts2-rs-h9u`; for now every action is emitted with
    // `target: None` and the dispatcher resolves targets internally.
    for (slot, kind) in state.player.potions.iter().enumerate() {
        if let (Some(lock), Some(player_char)) = (kind.character_lock(), state.player.character) {
            if lock != player_char {
                continue;
            }
        }
        // `PotionUsage.Automatic` potions (Fairy in a Bottle) are never
        // manually drinkable — C# disables the drink button
        // (`NPotionPopup.cs:273`) and `FairyInABottle.OnUse` must only
        // fire from its death-prevention trigger. Skipping (not
        // removing) keeps later slots' indices intact, same as the
        // character-lock gate above. A8 audit §7a.
        if kind.is_automatic() {
            continue;
        }
        #[allow(clippy::cast_possible_truncation)]
        out.push(Action::DrinkPotion {
            potion_slot: slot as u8,
            target: None,
        });
    }

    // End turn is always legal in PlayerPlayPhase.
    out.push(Action::EndTurn);
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_log_defaults_empty_and_off() {
        let te = TurnEvents::default();
        assert!(te.card_play_log.is_empty());
        assert!(
            !te.record_card_play_log,
            "recording must default OFF for parity safety"
        );
    }

    #[test]
    fn play_log_entry_roundtrips_fields() {
        let e = PlayLogEntry {
            card_id: crate::cards::CardId::StrikeIronclad,
            upgraded: true,
            energy_spent: 2,
            auto_played: true,
            target: Some(1),
        };
        assert_eq!(e.card_id, crate::cards::CardId::StrikeIronclad);
        assert!(e.upgraded);
        assert_eq!(e.energy_spent, 2);
        assert!(e.auto_played);
        assert_eq!(e.target, Some(1));
    }

    /// Minimal alive enemy Creature for legal-action gating tests.
    fn alive_enemy(id: u32) -> Creature {
        Creature {
            id: NonZeroU32::new(id).expect("nz"),
            current_hp: 30,
            max_hp: 30,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            monster_id: MonsterId("TestEnemy"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        }
    }

    #[test]
    fn version_is_set() {
        assert!(!version().is_empty());
    }

    /// q6bix F-C: `player_power_amount` must see a power applied to the
    /// player's MIRROR Creature even when the legacy `state.player.powers`
    /// list is empty (card-driven `apply_power_to_player` writes only the
    /// mirror). A raw legacy scan returns 0 and silently no-ops reactive
    /// hooks (Juggernaut / FeelNoPain / DarkEmbrace).
    #[test]
    fn q6bix_player_power_amount_walks_mirror_union() {
        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 80;
        let player_uid = state.init_player_creature();
        // Apply Strength(5) to the mirror only (the realistic combat path).
        card_effects::apply_power_to_creature(&mut state, player_uid, PowerKind::Strength, 5);
        assert!(
            state.player.powers.is_empty(),
            "precondition: legacy surface empty — power lives on the mirror"
        );
        assert_eq!(
            state.player_power_amount(PowerKind::Strength),
            5,
            "union helper must find the mirror-applied power"
        );
        // A legacy-only power is also seen, deduped against the mirror.
        assert_eq!(state.player_power_amount(PowerKind::Juggernaut), 0);
    }

    /// Bead `sts2-rs-kese5` (f933s sub-1): ChainsOfBindingPower
    /// `AfterCardDrawn` afflicts up to `stacks` newly-drawn player
    /// cards with `Bound` per turn; further draws this turn are
    /// untouched. Mirrors C# `ChainsOfBindingPower.cs` (v0.103.2)
    /// `AfterCardDrawn` capped via
    /// `CombatManager.History.OfType<CardAfflictedEntry>().Count(...)`.
    /// Rust uses the per-turn cap-check budget on
    /// `TurnEvents::bound_cards_afflicted_by_chains_this_turn` (bead
    /// `sts2-rs-hmxmc`).
    #[test]
    fn kese5_chains_of_binding_afflicts_up_to_stack_count_then_stops() {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;

        let mut state = CombatState::default();
        // Player creature so accessor paths resolve (not strictly
        // required for this surface but matches realistic combat).
        let player_id = NonZeroU32::new(99).expect("nz");
        state.creatures.push(Creature {
            id: player_id,
            current_hp: 80,
            max_hp: 80,
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: None,
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Player"),
            ai: None,
        });
        state.player.creature_uid = Some(CreatureUid(player_id));

        // Push 5 Attack cards into the deck POD with sequential uids
        // 1..=5, then thread them into the player's hand.
        for i in 1u32..=5 {
            let uid = CardUid::new(i).expect("nz");
            state.cards.push(cards::Card {
                uid,
                card_id: cards::CardId::StrikeIronclad,
                cost: 1,
                card_type: cards::CardType::Attack,
                target_type: cards::TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                ..cards::Card::pod_defaults()
            });
        }
        // Place all 5 uids in hand AFTER recording the pre-draw
        // length so invoke_card_drawn_for_newly_drawn walks the
        // full suffix.
        let hand_len_before = state.player.hand.len();
        for i in 1u32..=5 {
            state.player.hand.push(CardUid::new(i).expect("nz"));
        }

        // Apply ChainsOfBinding(3) to player.
        let pow_uid = PowerUid::new(1).expect("nz");
        state.powers.push(Power {
            id: pow_uid.0,
            kind: powers::PowerKind::ChainsOfBinding,
            amount: 3,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(pow_uid);

        state.invoke_card_drawn_for_newly_drawn(hand_len_before, true);

        let bound_count = state
            .cards
            .iter()
            .filter(|c| {
                matches!(
                    c.affliction,
                    Some(card_afflictions::CardAffliction {
                        kind: card_afflictions::CardAfflictionKind::Bound,
                        ..
                    })
                )
            })
            .count();
        assert_eq!(
            bound_count, 3,
            "exactly 3 of 5 drawn cards must be Bound (cap = stacks=3)",
        );
        assert_eq!(
            state.combat_log.bound_cards_afflicted_by_chains_this_turn, 3,
            "per-turn cap-check budget tracks the affliction count",
        );

        // Subsequent draws THIS TURN must not be afflicted (cap hit).
        for i in 6u32..=8 {
            let uid = CardUid::new(i).expect("nz");
            state.cards.push(cards::Card {
                uid,
                card_id: cards::CardId::StrikeIronclad,
                cost: 1,
                card_type: cards::CardType::Attack,
                target_type: cards::TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                ..cards::Card::pod_defaults()
            });
        }
        let hand_len_before_2 = state.player.hand.len();
        for i in 6u32..=8 {
            state.player.hand.push(CardUid::new(i).expect("nz"));
        }
        state.invoke_card_drawn_for_newly_drawn(hand_len_before_2, true);
        for i in 6u32..=8 {
            let idx = i as usize - 1;
            assert!(
                state.cards[idx].affliction.is_none(),
                "card {i} drawn after cap must not be Bound",
            );
        }
        assert_eq!(
            state.combat_log.bound_cards_afflicted_by_chains_this_turn, 3,
            "budget unchanged after cap-hit draws",
        );
    }

    /// Bead `sts2-rs-lj36b` + `sts2-rs-ktqmg`: ConfusedPower.AfterCardDrawn
    /// (Snecko Eye / FAKE_SNECKO_EYE) re-rolls every drawn player card's cost
    /// off the `combat_energy_costs` stream. C# `ConfusedPower.cs::AfterCardDrawn`
    /// skips ONLY cards where `Canonical < 0` (Status/Curse with
    /// `canonicalCost=-1`). X-cost cards have `Canonical = 0` in C# so they
    /// DO consume one `NextInt(4)` read, even though the resulting modifier is
    /// a no-op (C# `GetWithModifiers` short-circuits on `CostsX`). In Rust:
    /// X-cost cards consume the read but keep `cost=-1` intact. Unplayable
    /// Status/Curse cards (cost=-1 but `is_x_cost()=false`) skip the roll.
    /// Confused is detected through the mirror+legacy power union
    /// (`player_power_amount`) so a mirror-applied stack is seen.
    #[test]
    fn lj36b_confused_rerolls_costs_off_combat_energy_costs_skipping_xcost() {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;

        let mut state = CombatState::default();
        let player_id = NonZeroU32::new(99).expect("nz");
        state.creatures.push(Creature {
            id: player_id,
            current_hp: 80,
            max_hp: 80,
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: None,
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Player"),
            ai: None,
        });
        state.player.creature_uid = Some(CreatureUid(player_id));

        // 3 normal cards (cost 1) + 1 X-cost card (cost -1, uid 4).
        for i in 1u32..=3 {
            state.cards.push(cards::Card {
                uid: CardUid::new(i).expect("nz"),
                card_id: cards::CardId::StrikeIronclad,
                cost: 1,
                card_type: cards::CardType::Attack,
                target_type: cards::TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                ..cards::Card::pod_defaults()
            });
        }
        state.cards.push(cards::Card {
            uid: CardUid::new(4).expect("nz"),
            card_id: cards::CardId::Whirlwind,
            cost: -1,
            card_type: cards::CardType::Attack,
            target_type: cards::TargetType::Enemy,
            upgraded: false,
            base_damage: 5,
            base_block: 0,
            ..cards::Card::pod_defaults()
        });

        let hand_len_before = state.player.hand.len();
        for i in 1u32..=4 {
            state.player.hand.push(CardUid::new(i).expect("nz"));
        }

        // Mirror-applied Confused(1) — exercises the union read path that
        // FAKE_SNECKO_EYE relies on (apply_power_to_creature writes the
        // mirror creature, not state.player.powers).
        let pow_uid = PowerUid::new(1).expect("nz");
        state.powers.push(Power {
            id: pow_uid.0,
            kind: powers::PowerKind::Confused,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.creatures[0].powers.push(pow_uid);
        assert_eq!(
            state.player_power_amount(PowerKind::Confused),
            1,
            "mirror-applied Confused must be visible via the union helper",
        );

        let cursor_before = state.run_rng.combat_energy_costs.counter();
        state.invoke_card_drawn_for_newly_drawn(hand_len_before, true);

        // 4 draws off the stream: 3 normal cards + 1 X-cost card.
        // C# `ConfusedPower.AfterCardDrawn` skips only `Canonical < 0`
        // (Status/Curse). X-cost has Canonical=0 in C#, so C# DOES consume
        // one read for it. Bead `sts2-rs-ktqmg`.
        assert_eq!(
            state.run_rng.combat_energy_costs.counter() - cursor_before,
            4,
            "one combat_energy_costs draw per non-unplayable card; X-cost card also consumes read",
        );
        for i in 1u32..=3 {
            let cost = state.cards[i as usize - 1].cost;
            assert!(
                (0..=3).contains(&cost),
                "card {i} cost must be re-rolled into [0,3], got {cost}",
            );
        }
        // X-cost sentinel must be preserved even though the RNG read was consumed.
        assert_eq!(
            state.cards[3].cost, -1,
            "X-cost card cost sentinel (-1) must be left untouched; only the RNG read is consumed",
        );
    }

    /// Mutant-coverage counterpart to the test above (2026-06-20 sweep):
    /// with NO Confused power, `invoke_card_drawn_for_newly_drawn` must NOT
    /// reroll costs or consume the `combat_energy_costs` stream. Kills the
    /// `player_power_amount(Confused) > 0` → `>= 0` mutant (which would make
    /// `confused` always true and reroll/consume even with Confused absent).
    #[test]
    fn confused_absent_does_not_reroll_costs_or_consume_stream() {
        use smallvec::SmallVec;
        use std::num::NonZeroU32;

        let mut state = CombatState::default();
        let player_id = NonZeroU32::new(99).expect("nz");
        state.creatures.push(Creature {
            id: player_id,
            current_hp: 80,
            max_hp: 80,
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: None,
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Player"),
            ai: None,
        });
        state.player.creature_uid = Some(CreatureUid(player_id));

        for i in 1u32..=3 {
            state.cards.push(cards::Card {
                uid: CardUid::new(i).expect("nz"),
                card_id: cards::CardId::StrikeIronclad,
                cost: 1,
                card_type: cards::CardType::Attack,
                target_type: cards::TargetType::Enemy,
                base_damage: 6,
                ..cards::Card::pod_defaults()
            });
        }
        let hand_len_before = state.player.hand.len();
        for i in 1u32..=3 {
            state.player.hand.push(CardUid::new(i).expect("nz"));
        }

        assert_eq!(
            state.player_power_amount(PowerKind::Confused),
            0,
            "precondition: no Confused power",
        );
        let cursor_before = state.run_rng.combat_energy_costs.counter();
        state.invoke_card_drawn_for_newly_drawn(hand_len_before, true);

        assert_eq!(
            state.run_rng.combat_energy_costs.counter(),
            cursor_before,
            "no Confused → the combat_energy_costs stream must NOT be consumed",
        );
        for i in 1u32..=3 {
            assert_eq!(
                state.cards[i as usize - 1].cost,
                1,
                "no Confused → card {i} cost must be left at its original 1",
            );
        }
    }

    /// Bead `sts2-rs-lj36b`: card-effect draws (e.g. Slimed's OnPlay
    /// `Draw 1`) route through `cards::draw_via_state`, which must fire the
    /// same per-card on-draw riders as the combat-loop draw path — C#
    /// `CardPileCmd.Draw` fires `Hook.AfterCardDrawn` for every drawn card
    /// regardless of origin. Pins that a Confused player's card-effect draw
    /// consumes the `combat_energy_costs` stream (the FAKE_SNECKO_EYE
    /// coverage divergence root: a Slimed-drawn card left the cursor one
    /// read behind C#).
    #[test]
    fn lj36b_draw_via_state_fires_confused_cost_reroll() {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;

        let mut state = CombatState::default();
        let player_id = NonZeroU32::new(99).expect("nz");
        state.creatures.push(Creature {
            id: player_id,
            current_hp: 80,
            max_hp: 80,
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: None,
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Player"),
            ai: None,
        });
        state.player.creature_uid = Some(CreatureUid(player_id));

        // Two normal cards sitting in the draw pile (uids 1,2).
        for i in 1u32..=2 {
            state.cards.push(cards::Card {
                uid: CardUid::new(i).expect("nz"),
                card_id: cards::CardId::StrikeIronclad,
                cost: 1,
                card_type: cards::CardType::Attack,
                target_type: cards::TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                ..cards::Card::pod_defaults()
            });
            state.player.draw.push(CardUid::new(i).expect("nz"));
        }

        // Mirror-applied Confused(1).
        let pow_uid = PowerUid::new(1).expect("nz");
        state.powers.push(Power {
            id: pow_uid.0,
            kind: powers::PowerKind::Confused,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.creatures[0].powers.push(pow_uid);

        let cursor_before = state.run_rng.combat_energy_costs.counter();
        let moved = crate::cards::draw_via_state(&mut state, 2);

        assert_eq!(moved, 2, "both draw-pile cards drawn into hand");
        assert_eq!(
            state.run_rng.combat_energy_costs.counter() - cursor_before,
            2,
            "card-effect draw must fire the Confused cost re-roll per drawn card",
        );
        for uid in [1u32, 2] {
            let cost = state.cards[uid as usize - 1].cost;
            assert!(
                (0..=3).contains(&cost),
                "drawn card {uid} cost re-rolled into [0,3], got {cost}",
            );
        }
    }

    /// Issue #175 (Silent combat parity, seed 1131): CorrosiveWave's
    /// `AfterCardDrawn` must apply a SEPARATE Poison per drawn card so the
    /// per-application SneckoSkull `+1` additive fires once per card.
    ///
    /// C# `CorrosiveWavePower.cs:39-41` calls `PowerCmd.Apply<PoisonPower>`
    /// (amount = `base.Amount`) once per drawn card; C# `SneckoSkull.cs:21-29`
    /// adds `+1` to EACH PoisonPower the player applies. So drawing N cards
    /// under CorrosiveWave(amount) + SneckoSkull yields `N*(amount+1)` Poison
    /// on each enemy — NOT `N*amount + 1` (the bug: accumulating CorrosiveWave
    /// into ONE apply ran the SneckoSkull additive only once).
    ///
    /// Concretely (seed 1131): Adrenaline draws 2 cards, CorrosiveWave amount
    /// 2, SneckoSkull owned → C# poison 6 (=2*(2+1)); the accumulate-once bug
    /// produced 5 (=2*2+1).
    #[test]
    #[serial_test::serial(global_registry)]
    fn issue175_corrosive_wave_applies_snecko_skull_per_drawn_card() {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;

        card_effects::ensure_installed();
        // Register the SneckoSkull `+1 Poison given` arm. `install()` calls
        // `register_modify_power_amount_given` directly (no OnceLock gate), so
        // it is robust against sibling tests that `.remove()` the arm on
        // teardown when running in parallel.
        crate::relic_effects::snecko_skull::install();

        let mut state = CombatState::default();
        // Player mirror creature.
        let player_id = NonZeroU32::new(99).expect("nz");
        state.creatures.push(Creature {
            id: player_id,
            current_hp: 80,
            max_hp: 80,
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: None,
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Player"),
            ai: None,
        });
        state.player.creature_uid = Some(CreatureUid(player_id));

        // Two enemies (the seed-1131 Bowlbugs) to confirm each gets the full
        // per-card poison.
        for id in [10u32, 11] {
            let uid = CreatureUid(NonZeroU32::new(id).expect("nz"));
            state.creatures.push(Creature {
                id: uid.0,
                current_hp: 40,
                max_hp: 40,
                block: 0,
                powers: SmallVec::new(),
                species: Some(MonsterKind::Nibbit),
                intent: None,
                intent_history: IntentHistory::default(),
                monster_id: MonsterId("Nibbit"),
                ai: None,
            });
            state.enemy_uids.push(uid);
        }

        // Equip SneckoSkull (player owns it).
        state.relics.push(Relic {
            id: NonZeroU32::new(1).expect("nz"),
            kind: RelicKind::SneckoSkull,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });

        // CorrosiveWave(2) applied to the player mirror (the realistic path).
        let pow_uid = PowerUid::new(1).expect("nz");
        state.powers.push(Power {
            id: pow_uid.0,
            kind: PowerKind::CorrosiveWave,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.creatures[0].powers.push(pow_uid);
        assert_eq!(
            state.player_power_amount(PowerKind::CorrosiveWave),
            2,
            "precondition: CorrosiveWave(2) is live on the player"
        );

        // Two cards in the draw pile (Adrenaline draws 2).
        for i in 1u32..=2 {
            state.cards.push(cards::Card {
                uid: CardUid::new(i).expect("nz"),
                card_id: cards::CardId::StrikeIronclad,
                cost: 1,
                card_type: cards::CardType::Attack,
                target_type: cards::TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                ..cards::Card::pod_defaults()
            });
            state.player.draw.push(CardUid::new(i).expect("nz"));
        }

        let moved = state.draw_into_hand_card_effect(2);
        assert_eq!(moved, 2, "both draw-pile cards drawn");

        // C#: 2 cards * (CorrosiveWave 2 + SneckoSkull 1) = 6 Poison per enemy.
        // The accumulate-once bug applied SneckoSkull only once → 5.
        let poison = |state: &CombatState, id: u32| -> i32 {
            let c = state
                .creatures
                .iter()
                .find(|c| c.id.get() == id)
                .expect("enemy present");
            c.powers
                .iter()
                .find_map(|puid| {
                    state
                        .powers
                        .get_by_id(puid.0)
                        .filter(|p| p.kind == PowerKind::Poison)
                        .map(|p| p.amount)
                })
                .unwrap_or(0)
        };
        assert_eq!(
            poison(&state, 10),
            6,
            "enemy 1: 2 cards each apply Poison(2)+1 (SneckoSkull) = 6, not accumulate-once 5"
        );
        assert_eq!(
            poison(&state, 11),
            6,
            "enemy 2: same per-card SneckoSkull additive"
        );
    }

    /// Bead `sts2-rs-ktqmg`: confirms the X-cost / Status/Curse split in
    /// `ConfusedPower.AfterCardDrawn`. C# skips only cards where
    /// `Canonical < 0`; X-cost cards have `Canonical = 0` so they DO
    /// consume one `CombatEnergyCosts.NextInt(4)` read. Status/Curse
    /// cards (canonicalCost=-1 → `Canonical=-1`) are skipped.
    ///
    /// C# ref: `ConfusedPower.cs::AfterCardDrawn` +
    ///         `CardEnergyCost.cs` ctor `Canonical = CostsX ? 0 : canonicalCost`
    ///         (v0.103.2).
    #[test]
    fn ktqmg_confused_rerolls_xcost_skips_status_curse() {
        use std::num::NonZeroU32;

        use smallvec::SmallVec;

        let mut state = CombatState::default();
        let player_id = NonZeroU32::new(99).expect("nz");
        state.creatures.push(Creature {
            id: player_id,
            current_hp: 80,
            max_hp: 80,
            block: 0,
            powers: SmallVec::<[PowerUid; 8]>::new(),
            species: None,
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Player"),
            ai: None,
        });
        state.player.creature_uid = Some(CreatureUid(player_id));

        // uid 1: normal cost-1 Attack — should be rolled
        state.cards.push(cards::Card {
            uid: CardUid::new(1).expect("nz"),
            card_id: cards::CardId::StrikeIronclad,
            cost: 1,
            card_type: cards::CardType::Attack,
            target_type: cards::TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..cards::Card::pod_defaults()
        });
        // uid 2: X-cost Attack (Whirlwind, cost=-1, is_x_cost=true) — RNG
        // read consumed, sentinel preserved
        state.cards.push(cards::Card {
            uid: CardUid::new(2).expect("nz"),
            card_id: cards::CardId::Whirlwind,
            cost: -1,
            card_type: cards::CardType::Attack,
            target_type: cards::TargetType::Enemy,
            upgraded: false,
            base_damage: 5,
            base_block: 0,
            ..cards::Card::pod_defaults()
        });
        // uid 3: Status card (cost=-1, card_type=Status) — Canonical=-1 in C#,
        // is_x_cost=false in Rust — should be SKIPPED (no RNG read)
        state.cards.push(cards::Card {
            uid: CardUid::new(3).expect("nz"),
            card_id: cards::CardId::Wound,
            cost: -1,
            card_type: cards::CardType::Status,
            target_type: cards::TargetType::None,
            upgraded: false,
            base_damage: 0,
            base_block: 0,
            ..cards::Card::pod_defaults()
        });

        let hand_len_before = state.player.hand.len();
        for i in 1u32..=3 {
            state.player.hand.push(CardUid::new(i).expect("nz"));
        }

        // Apply Confused via mirror creature.
        let pow_uid = PowerUid::new(1).expect("nz");
        state.powers.push(Power {
            id: pow_uid.0,
            kind: powers::PowerKind::Confused,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.creatures[0].powers.push(pow_uid);

        let cursor_before = state.run_rng.combat_energy_costs.counter();
        state.invoke_card_drawn_for_newly_drawn(hand_len_before, true);

        // Exactly 2 reads: uid-1 (normal, rolled) + uid-2 (X-cost, rolled).
        // uid-3 (Status) is skipped — Canonical=-1 in C#.
        assert_eq!(
            state.run_rng.combat_energy_costs.counter() - cursor_before,
            2,
            "normal card + X-cost card each consume one read; Status card skipped",
        );
        // Normal card gets a rolled cost in [0,3].
        assert!(
            (0..=3).contains(&state.cards[0].cost),
            "uid-1 (normal Attack) cost must be re-rolled into [0,3]",
        );
        // X-cost sentinel must remain -1.
        assert_eq!(
            state.cards[1].cost, -1,
            "uid-2 (X-cost Whirlwind) sentinel -1 must be preserved",
        );
        // Status card cost must remain -1 and no RNG was consumed for it.
        assert_eq!(
            state.cards[2].cost, -1,
            "uid-3 (Status Wound) cost must be unchanged",
        );
    }

    /// Bead `sts2-rs-hmxmc` (f933s sub-2): the
    /// `bound_cards_afflicted_by_chains_this_turn` per-turn cap-check
    /// counter increments on direct mutation and resets to 0 via
    /// `TurnEvents::reset_per_turn`, mirroring the per-turn-clear
    /// contract every other `*_this_turn` counter on `TurnEvents`
    /// follows. The increment path itself lands in
    /// [`CombatState::invoke_card_drawn_for_newly_drawn`] under
    /// sibling sub-1 (`sts2-rs-kese5`); this test pins the
    /// reset-boundary contract independently so the cap-check budget
    /// can never silently bleed across turns.
    #[test]
    fn hmxmc_bound_cards_chains_counter_resets_per_turn() {
        let mut te = TurnEvents::default();
        assert_eq!(
            te.bound_cards_afflicted_by_chains_this_turn, 0,
            "default per-turn cap-check budget is zero",
        );
        te.bound_cards_afflicted_by_chains_this_turn = 2;
        te.reset_per_turn();
        assert_eq!(
            te.bound_cards_afflicted_by_chains_this_turn, 0,
            "per-turn budget MUST clear on reset_per_turn",
        );
    }

    /// Bead `sts2-rs-a7cw8`: PlayerCombatState derives Default
    /// automatically. Five piles (hand/draw/discard/exhaust/play),
    /// two pin lists (pinned_top/pinned_bottom), powers, and the
    /// scalars all default to empty/zero. Sibling to j17ge
    /// (CombatState) and wwhq0 (ctx defaults). A regression that
    /// pre-populated any pile or seeded a non-zero scalar would
    /// corrupt every test using PlayerCombatState::default() as a
    /// baseline.
    #[test]
    fn player_combat_state_default_field_invariants() {
        let p = PlayerCombatState::default();
        // Piles all empty.
        assert!(p.hand.is_empty(), "default hand");
        assert!(p.draw.is_empty(), "default draw");
        assert!(p.discard.is_empty(), "default discard");
        assert!(p.exhaust.is_empty(), "default exhaust");
        assert!(p.play.is_empty(), "default play");
        // Pin lists empty.
        assert!(p.pinned_top.is_empty(), "default pinned_top");
        assert!(p.pinned_bottom.is_empty(), "default pinned_bottom");
        // Powers empty.
        assert!(p.powers.is_empty(), "default powers");
        // Scalars all zero.
        assert_eq!(p.energy, 0, "default energy");
        assert_eq!(p.max_energy, 0, "default max_energy");
        assert_eq!(p.stars, 0, "default stars");
        assert_eq!(p.current_hp, 0, "default current_hp");
        assert_eq!(p.max_hp, 0, "default max_hp");
        assert_eq!(p.block, 0, "default block");
        // Character None.
        assert!(p.character.is_none(), "default character");
        // Bead `sts2-rs-wwp5y`: potions slot (added by 13q) defaults
        // empty — sibling fill that the original a7cw8 test missed.
        assert!(p.potions.is_empty(), "default potions inventory");
    }

    /// Bead `sts2-rs-j17ge`: lock the scalar-field defaults of
    /// CombatState::default. Sibling to wwhq0/tuex4/3knr4 for the
    /// ctx + RunState default-pin family. A regression that flipped
    /// is_over to true or seeded round non-zero would corrupt every
    /// test using CombatState::default() as a baseline — the
    /// dispatch_compiles_for_all_hookids smoke uses defaults but
    /// doesn't verify what they ARE.
    #[test]
    fn combat_state_default_scalar_field_invariants() {
        let s = CombatState::default();
        assert_eq!(s.phase, CombatPhase::Setup, "default phase");
        assert_eq!(s.side, Side::Player, "default side");
        assert_eq!(s.round, 0, "default round");
        assert_eq!(s.ascension, 0, "default ascension");
        assert!(!s.is_over, "default is_over");
        assert!(!s.player_won, "default player_won");
        assert!(!s.is_elite, "default is_elite");
        assert_eq!(s.energy_wasted, 0, "default energy_wasted");
        assert_eq!(s.cards_drawn, 0, "default cards_drawn");
        assert_eq!(s.pending_run_gold_loss, 0, "default pending_run_gold_loss");
        assert!(s.pending_choice.is_none(), "default pending_choice");
        assert!(s.pending_play.is_none(), "default pending_play");
        assert!(
            !s.pending_turn_setup_resume,
            "default pending_turn_setup_resume"
        );
        assert!(
            !s.pending_wlp_retain_resume,
            "default pending_wlp_retain_resume"
        );
        assert!(s.last_added_card.is_none(), "default last_added_card");
        assert!(s.active_attack.is_none(), "default active_attack");
        assert!(s.master_deck.is_empty(), "default master_deck empty");
        assert!(s.allies.is_empty(), "default allies empty");
        assert!(s.enemy_uids.is_empty(), "default enemy_uids empty");
    }

    /// Bead `sts2-rs-d03` Phase 1 smoke test.
    ///
    /// After [`CombatState::init_player_creature`], the
    /// [`PlayerCombatState::creature_uid`] handle resolves to a
    /// [`Creature`] in `state.creatures` whose `current_hp` mirrors
    /// `state.player.current_hp`. Locks the additive-scaffolding
    /// contract that Phase 2 will rely on when migrating direct
    /// `state.player.{...}` accesses to read through `creature_uid`.
    ///
    /// Also exercises:
    ///   * [`CombatState::creature_kind`] classifying the player's
    ///     uid as [`CreatureKind::Player`] and a non-player uid as
    ///     [`CreatureKind::Monster`].
    ///   * [`CombatState::sync_player_creature`] keeping the Creature
    ///     mirror in lock-step after a mutation through
    ///     [`PlayerCombatState`].
    ///   * [`CombatState::teardown_player_creature`] flushing the
    ///     Creature back to `PlayerCombatState` and clearing the
    ///     handle.
    #[test]
    fn d03_phase1_player_creature_unification_smoke() {
        let mut state = CombatState::default();
        state.player.current_hp = 75;
        state.player.max_hp = 80;
        state.player.block = 0;

        let uid = state.init_player_creature();

        // 1. Creature is in the arena and the handle is captured.
        assert_eq!(state.player.creature_uid, Some(uid));
        let creature_idx = state
            .creatures
            .iter()
            .position(|c| c.id == uid.0)
            .expect("init_player_creature must push a Creature for the player");

        // 2. The Creature mirrors the player's combat scalars.
        assert_eq!(
            state.creatures[creature_idx].current_hp,
            state.player.current_hp
        );
        assert_eq!(state.creatures[creature_idx].max_hp, state.player.max_hp);
        assert_eq!(state.creatures[creature_idx].block, state.player.block);

        // 3. CreatureKind classifies the uid as Player.
        assert_eq!(state.creature_kind(uid), CreatureKind::Player);

        // 4. A foreign uid classifies as Monster.
        let foreign_uid = CreatureUid(NonZeroU32::new(999).expect("non-zero"));
        assert_eq!(state.creature_kind(foreign_uid), CreatureKind::Monster);

        // 5. Mutate PlayerCombatState then sync — Creature follows.
        state.player.current_hp = 60;
        state.player.block = 7;
        state.sync_player_creature();
        assert_eq!(state.creatures[creature_idx].current_hp, 60);
        assert_eq!(state.creatures[creature_idx].block, 7);

        // 6. Teardown removes the record and clears the handle.
        state.teardown_player_creature();
        assert!(state.player.creature_uid.is_none());
        assert!(state.creatures.iter().all(|c| c.id != uid.0));
    }

    #[test]
    fn default_state_clones_cleanly() {
        let state = CombatState::default();
        let cloned = state.clone();
        assert_eq!(state, cloned);
        // Sanity: defaults match the plan-spec'd starting position.
        assert_eq!(state.phase, CombatPhase::Setup);
        assert_eq!(state.round, 0);
        assert_eq!(state.side, Side::Player);
        assert!(state.creatures.is_empty());
        assert!(state.cards.is_empty());
        assert!(state.enemy_uids.is_empty());
        assert!(!state.is_over);
        assert!(!state.player_won);
    }

    /// `apply` returns `Advanced` for all action variants on a fresh
    /// `CombatState::default()` — an empty state gives every arm nothing
    /// to do: no hand to play from, no potions in the belt (so the
    /// `UsePotion`→`DrinkPotion` delegation bails on the slot bound), no
    /// open choice, and `phase=Setup` trips `EndTurn`'s out-of-phase
    /// guard. Locks the defensive-shape behavior across the action
    /// surface.
    #[test]
    fn apply_returns_advanced_for_all_actions() {
        let mut state = CombatState::default();

        let actions = [
            Action::PlayCard {
                hand_index: 0,
                target: None,
            },
            Action::PlayCard {
                hand_index: 3,
                target: Some(2),
            },
            Action::EndTurn,
            Action::UsePotion {
                slot: 0,
                target: None,
            },
            Action::UsePotion {
                slot: 1,
                target: Some(0),
            },
            Action::ResolveChoice {
                selection: SmallVec::new(),
            },
            Action::ResolveChoice {
                selection: SmallVec::from_slice(&[0u8, 1, 2]),
            },
        ];

        for action in actions {
            assert_eq!(apply(&mut state, action), StepResult::Advanced);
        }
    }

    // ── P3-7 (sts2-rs-e3k) Action::EndTurn dispatch tests ──────────────

    /// `apply(Action::EndTurn)` on a state with `phase=PlayerPlayPhase`
    /// walks the full 6-step end-turn chain via
    /// `combat_loop::end_turn_dispatch`. After a clean walk (no combat
    /// end mid-chain): phase loops back to `PlayerPlayPhase` (the
    /// resting state where the next `Action::PlayCard` is valid),
    /// `round` increments by 1.
    #[test]
    fn apply_end_turn_walks_full_player_to_player_chain() {
        use std::num::NonZeroU32;
        let mut state = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            round: 1,
            ..Default::default()
        };
        state.player.current_hp = 50;
        state.player.max_hp = 50;
        state.player.max_energy = 3;
        // One Cultist with Buff intent (no-op stub at Phase 3 — won't
        // damage the player). Provides a non-zero alive-enemy count so
        // the chain doesn't short-circuit on CombatWon.
        let id = NonZeroU32::new(1).expect("non-zero");
        state.creatures.push(Creature {
            id,
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: SmallVec::new(),
            species: Some(monsters::MonsterKind::Nibbit),
            intent: Some(monsters::Intent::Buff {
                kind: PowerKind::Strength,
                amount: 0,
            }),
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(id));

        let result = apply(&mut state, Action::EndTurn);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);
        assert_eq!(state.round, 2, "round bumped by enemy_turn_end");
        assert_eq!(state.player.current_hp, 50);
    }

    /// `apply(Action::EndTurn)` propagates `CombatLost` when a mid-chain
    /// enemy attack kills the player. `enemy_turn_exec`'s
    /// `apply_damage_to_player` flips `is_over` + `player_won=false`,
    /// the chain short-circuits, the result bubbles up through
    /// `end_turn_dispatch` to `apply`.
    #[test]
    fn apply_end_turn_short_circuits_on_combat_lost() {
        use std::num::NonZeroU32;
        let mut state = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            round: 1,
            ..Default::default()
        };
        state.player.current_hp = 5;
        state.player.max_hp = 50;
        state.player.max_energy = 3;
        let id = NonZeroU32::new(1).expect("non-zero");
        state.creatures.push(Creature {
            id,
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: SmallVec::new(),
            species: Some(monsters::MonsterKind::Nibbit),
            // Attack(6) kills a 5-HP player.
            intent: Some(monsters::Intent::Attack { damage: 6 }),
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(id));

        let result = apply(&mut state, Action::EndTurn);

        assert_eq!(result, StepResult::CombatLost);
        assert!(state.is_over);
        assert!(!state.player_won);
        assert_eq!(state.player.current_hp, 0);
    }

    /// `apply(Action::EndTurn)` short-circuits with `CombatWon` when
    /// the state enters with `is_over=true / player_won=true`. The
    /// first sub-step (`end_player_turn_phase_one`) hits its entry
    /// guard, returns `CombatWon` immediately, and the orchestrator
    /// propagates that up.
    #[test]
    fn apply_end_turn_short_circuits_on_combat_won() {
        let mut state = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            is_over: true,
            player_won: true,
            ..Default::default()
        };

        let result = apply(&mut state, Action::EndTurn);

        assert_eq!(result, StepResult::CombatWon);
        assert_eq!(
            state.phase,
            CombatPhase::PlayerPlayPhase,
            "phase unchanged on combat-already-resolved short-circuit",
        );
    }

    /// `apply(Action::EndTurn)` outside `PlayerPlayPhase` is a no-op
    /// (returns `Advanced` without mutation). Locks the
    /// out-of-phase-guard behavior — Phase 6 (sts2-rs-2un) lifts
    /// this to a tighter `Action::is_legal(state)` predicate that
    /// returns `Err` instead of `Advanced`.
    #[test]
    fn apply_end_turn_outside_play_phase_no_ops() {
        // Setup phase: out-of-phase EndTurn is a no-op.
        let mut state = CombatState {
            round: 7, // Sentinel — must be unchanged after no-op.
            ..Default::default()
        };
        assert_eq!(state.phase, CombatPhase::Setup);

        let result = apply(&mut state, Action::EndTurn);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.phase, CombatPhase::Setup, "phase unchanged");
        assert_eq!(state.round, 7, "round unchanged on out-of-phase no-op");
    }

    /// Same out-of-phase no-op semantics for an `EnemyTurnExec` entry
    /// (defensive — caller bug, but `apply` gracefully no-ops rather
    /// than corrupting state).
    #[test]
    fn apply_end_turn_in_enemy_turn_exec_no_ops() {
        let mut state = CombatState {
            phase: CombatPhase::EnemyTurnExec,
            round: 3,
            ..Default::default()
        };
        state.player.current_hp = 50;
        state.player.max_hp = 50;

        let result = apply(&mut state, Action::EndTurn);

        assert_eq!(result, StepResult::Advanced);
        assert_eq!(state.phase, CombatPhase::EnemyTurnExec);
        assert_eq!(state.round, 3);
        assert_eq!(state.player.current_hp, 50);
    }

    #[test]
    fn action_enum_exhaustiveness() {
        // Compile-time check that every variant is handled.
        let cases = [
            Action::PlayCard {
                hand_index: 0,
                target: None,
            },
            Action::EndTurn,
            Action::UsePotion {
                slot: 0,
                target: None,
            },
            Action::DrinkPotion {
                potion_slot: 0,
                target: None,
            },
            Action::Channel {
                orb: Orb::Lightning,
                target: None,
            },
            Action::Evoke { times: 1 },
            Action::ResolveChoice {
                selection: SmallVec::new(),
            },
        ];
        for case in cases {
            let label: &'static str = match case {
                Action::PlayCard { .. } => "play_card",
                Action::EndTurn => "end_turn",
                Action::UsePotion { .. } => "use_potion",
                Action::DrinkPotion { .. } => "drink_potion",
                Action::Channel { .. } => "channel",
                Action::Evoke { .. } => "evoke",
                Action::ResolveChoice { .. } => "resolve_choice",
            };
            assert!(!label.is_empty());
        }
    }

    #[test]
    fn uid_newtype_round_trips() {
        let uid = CardUid::new(42).expect("non-zero");
        assert_eq!(uid.get(), 42);
        assert!(CardUid::new(0).is_none());
    }

    #[test]
    fn terminal_value_reports_win_loss_and_inprogress() {
        let mut s = CombatState::default();
        assert_eq!(terminal_value(&s), (false, 0.0));
        assert!(!is_terminal(&s));
        s.is_over = true;
        s.player_won = true;
        // Default state has player.current_hp = 0 and 0 potions, so the
        // victory eval bottoms out at the win-bucket floor of 0.5. Bead
        // `sts2-rs-v2oh` replaced the flat +1 reward with a continuous
        // HP/potion-weighted score; see `evaluate_victory`. A real victory
        // from a healthy player will score well above 0.5.
        let (term, v) = terminal_value(&s);
        assert!(term);
        assert!((v - 0.5).abs() < 1e-6, "expected floor 0.5, got {v}");

        // High-HP win with potions in pocket scores near 1.0.
        s.player.current_hp = 80;
        s.player.max_hp = 80;
        s.player
            .potions
            .push(crate::potions::PotionKind::BlockPotion);
        s.player
            .potions
            .push(crate::potions::PotionKind::AttackPotion);
        let (_, v_healthy) = terminal_value(&s);
        assert!(
            v_healthy > 0.7,
            "healthy victory should score well above floor: got {v_healthy}"
        );

        s.player_won = false;
        // Losses are flat -1.0 regardless of remaining state.
        assert_eq!(terminal_value(&s), (true, -1.0));
        assert!(is_terminal(&s));
    }

    #[test]
    fn evaluate_victory_is_monotone_in_hp_and_potions() {
        let mut s = CombatState {
            is_over: true,
            player_won: true,
            ..CombatState::default()
        };
        s.player.max_hp = 80;

        s.player.current_hp = 1;
        let v_low = evaluate_victory(&s);
        s.player.current_hp = 80;
        let v_high = evaluate_victory(&s);
        assert!(
            v_high > v_low,
            "more HP should score higher: low={v_low} high={v_high}"
        );

        s.player.current_hp = 40;
        let v_no_pots = evaluate_victory(&s);
        s.player
            .potions
            .push(crate::potions::PotionKind::BlockPotion);
        let v_with_pot = evaluate_victory(&s);
        assert!(
            v_with_pot > v_no_pots,
            "holding a potion at victory should score higher: \
             no_pots={v_no_pots} with_pot={v_with_pot}"
        );

        // Floor: even a 1-HP no-potion win is still in the Win bucket.
        s.player.current_hp = 1;
        s.player.potions.clear();
        assert!(evaluate_victory(&s) >= 0.5);
    }

    #[test]
    fn evaluate_in_progress_stays_inside_widened_clamp() {
        // GH issue #11: the in-progress eval is now soft-saturated via
        // `tanh(raw / IN_PROGRESS_DIVISOR)` so the Q-gradient survives
        // at the extremes (near-dead vs. fully-dead enemies must not
        // project to the same plateau). `tanh` is asymptotic to `±1.0`
        // and never reaches it, so the `±1.0` reservation for
        // `terminal_value` still holds — every eval lives strictly
        // inside `(-1.0, 1.0)`.
        let mut s = CombatState::default();
        let v_empty = evaluate_in_progress(&s);
        assert!(
            v_empty.abs() < 1.0,
            "empty state outside tanh band: {v_empty}"
        );

        // Saturate the positive side: simulate "no enemies in arena
        // and a long-running, potion-rich game".
        s.round = 30;
        s.player
            .potions
            .push(crate::potions::PotionKind::BlockPotion);
        s.player
            .potions
            .push(crate::potions::PotionKind::AttackPotion);
        let v_pos = evaluate_in_progress(&s);
        assert!(v_pos < 1.0, "positive-bias eval above tanh band: {v_pos}");
        // The post-`tidt` joint-refit divisor (114.56) targets the 99th
        // percentile of the joint starter+realistic-floor-40+ distribution
        // (p99 ≈ 51.55 raw, → 0.45 normalized). This test state isn't a
        // p99 state — it's a moderate "long-game, potion-rich, no enemies"
        // positive — so its post-rescale magnitude under the joint divisor
        // is ~0.17. The load-bearing invariant is that a positive-bias
        // state stays strictly positive (i.e. the rescale doesn't compress
        // the signal into the `Tie` bucket); the pre-`zzpb` `±0.49` clamp
        // ceiling is no longer relevant under the wider `±0.9` band.
        assert!(
            v_pos > 0.0,
            "post-rescale positive-bias eval must stay positive: {v_pos}"
        );
    }

    #[test]
    fn eval_features_dot_weights_matches_evaluate_in_progress_raw() {
        // bead sts2-rs-uxyf (vsqc.2): the Python Texel harness fits
        // weights `w` over `eval_features` such that
        // `sigmoid(K · features.dot(w)) ≈ outcome`. For the
        // refactoring to be safe, the linear sum
        // `features.dot(EVAL_FEATURE_WEIGHTS)` must equal
        // `evaluate_in_progress_raw` to within f32 noise on a
        // synthetic sanity grid.
        let mut s = CombatState::default();
        s.player.max_hp = 80;
        s.player.current_hp = 40;
        s.player.block = 5;
        s.round = 7;
        s.energy_wasted = 3;
        s.cards_drawn = 12;
        let feats = eval_features(&s);
        let dot: f32 = feats
            .iter()
            .zip(EVAL_FEATURE_WEIGHTS.iter())
            .map(|(f, w)| f * w)
            .sum();
        let raw = evaluate_in_progress_raw(&s);
        assert!(
            (dot - raw).abs() < 1e-4,
            "feature dot product must equal raw evaluator: dot={dot} raw={raw}"
        );
    }

    /// Bead `sts2-rs-adulv`: wasted-block feature fires only when no
    /// alive non-minion enemy telegraphs Attack/MultiAttack **and** the
    /// player holds no block-preserving Power. Drives the
    /// `defend_no_attack_intent` blunder fix.
    #[test]
    #[allow(clippy::float_cmp)] // exact-zero pin: feature suppressed.
    fn wasted_block_penalty_fires_only_on_no_attack_intent_and_no_preserver() {
        use std::num::NonZeroU32;
        let player_uid = NonZeroU32::new(1).expect("non-zero");
        let enemy_uid = NonZeroU32::new(2).expect("non-zero");

        let mut base = CombatState::default();
        base.player.max_hp = 80;
        base.player.current_hp = 60;
        base.player.block = 15;
        base.player.creature_uid = Some(CreatureUid(player_uid));
        base.creatures.push(Creature {
            id: player_uid,
            current_hp: 60,
            max_hp: 80,
            block: 15,
            powers: SmallVec::new(),
            species: None,
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId(""),
            ai: None,
        });
        // Enemy telegraphing BUFF (no Attack intent).
        base.creatures.push(Creature {
            id: enemy_uid,
            current_hp: 40,
            max_hp: 40,
            block: 0,
            powers: SmallVec::new(),
            species: Some(monsters::MonsterKind::Nibbit),
            intent: Some(monsters::Intent::Defend { block: 5 }),
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        base.enemy_uids.push(CreatureUid(enemy_uid));

        // Probe 1: no attack intent + no preserving power → feature
        // fires (15 / 50 = 0.3).
        let feats = eval_features(&base);
        let wasted_idx = EVAL_FEATURE_NAMES
            .iter()
            .position(|n| *n == "wasted_block")
            .expect("wasted_block column present");
        assert!(
            (feats[wasted_idx] - 0.3).abs() < 1e-4,
            "expected wasted_block = 15/50 = 0.3, got {}",
            feats[wasted_idx]
        );

        // Probe 2: gain Metallicize → preserving power, feature
        // suppressed.
        let mut with_metal = base.clone();
        let metal_uid = PowerUid::new(7).expect("non-zero");
        with_metal.powers.push(Power {
            id: metal_uid.0,
            kind: PowerKind::Metallicize,
            amount: 3,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        with_metal.creatures[0].powers.push(metal_uid);
        let feats_metal = eval_features(&with_metal);
        assert_eq!(
            feats_metal[wasted_idx], 0.0,
            "Metallicize must suppress wasted_block, got {}",
            feats_metal[wasted_idx]
        );

        // Probe 3: enemy switches to Attack → feature suppressed.
        let mut with_attack = base.clone();
        with_attack.creatures[1].intent = Some(monsters::Intent::Attack { damage: 6 });
        let feats_attack = eval_features(&with_attack);
        assert_eq!(
            feats_attack[wasted_idx], 0.0,
            "Attack intent must suppress wasted_block, got {}",
            feats_attack[wasted_idx]
        );

        // Probe 4: dot-product invariant survives the new column.
        let dot: f32 = feats
            .iter()
            .zip(EVAL_FEATURE_WEIGHTS.iter())
            .map(|(f, w)| f * w)
            .sum();
        let raw = evaluate_in_progress_raw(&base);
        assert!(
            (dot - raw).abs() < 1e-4,
            "dot/raw mismatch with wasted_block feature: dot={dot} raw={raw}"
        );
    }

    #[test]
    fn eval_features_column_order_matches_names() {
        // bead sts2-rs-uxyf (vsqc.2): the column-order arrays must
        // stay in lockstep so the Python harness's header parses
        // correctly.
        assert_eq!(EVAL_FEATURE_NAMES.len(), EVAL_FEATURE_WEIGHTS.len());
        assert_eq!(
            EVAL_FEATURE_NAMES.len(),
            eval_features(&CombatState::default()).len()
        );
    }

    /// Bead `sts2-rs-4wie` (bottled_ai audit B-1): with player
    /// Intangible up, the threat term must clamp to 1 dmg/hit so a
    /// telegraphed 30-damage 2-hit attack reads as 2 dmg (then minus
    /// block) instead of 30. We compare two states identical except
    /// for an Intangible power on the player's mirror Creature and
    /// assert the Intangible state evaluates strictly higher (less
    /// threat-pessimism).
    #[test]
    fn evaluate_in_progress_intangible_caps_threat_term() {
        use std::num::NonZeroU32;
        let mut base = CombatState::default();
        base.player.max_hp = 80;
        base.player.current_hp = 40;
        base.player.block = 0;

        // Wire a player-mirror Creature (uid=1) so player_has_intangible
        // can resolve the Power lookup.
        let player_uid = NonZeroU32::new(1).expect("non-zero");
        base.player.creature_uid = Some(CreatureUid(player_uid));
        base.creatures.push(Creature {
            id: player_uid,
            current_hp: 40,
            max_hp: 80,
            block: 0,
            powers: SmallVec::new(),
            species: None,
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId(""),
            ai: None,
        });

        // One non-minion enemy telegraphing Attack(damage=15, hits=2)
        // — net_incoming = 30, total_hits = 2.
        let enemy_uid = NonZeroU32::new(2).expect("non-zero");
        base.creatures.push(Creature {
            id: enemy_uid,
            current_hp: 50,
            max_hp: 50,
            block: 0,
            powers: SmallVec::new(),
            species: Some(monsters::MonsterKind::Nibbit),
            intent: Some(monsters::Intent::MultiAttack {
                damage: 15,
                hits: 2,
            }),
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        base.enemy_uids.push(CreatureUid(enemy_uid));

        // Baseline: no Intangible.
        let v_no_intangible = evaluate_in_progress(&base);

        // With Intangible on the player.
        let mut with_intangible = base.clone();
        let intangible_uid = PowerUid::new(7).expect("non-zero");
        with_intangible.powers.push(Power {
            id: intangible_uid.0,
            kind: PowerKind::Intangible,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        // The mirror Creature is at index 0 (we pushed it first).
        with_intangible.creatures[0].powers.push(intangible_uid);

        let v_intangible = evaluate_in_progress(&with_intangible);

        assert!(
            v_intangible > v_no_intangible,
            "Intangible must reduce threat-term pessimism: \
             without={v_no_intangible:.4} with={v_intangible:.4}"
        );
        // Sanity: helper agrees.
        assert!(!player_has_intangible(&base));
        assert!(player_has_intangible(&with_intangible));
    }

    /// ldd1c follow-up (`enemy_slippery_stacks` feature): clearing an
    /// enemy's Slippery wall must raise the eval even when HP is
    /// unchanged. `Vantom HP 181 Slippery 9` and `Vantom HP 181
    /// Slippery 0` differ only in the negative-weighted
    /// `enemy_slippery_stacks` column; the cleared state is strictly
    /// better because the next attack lands at full damage instead of
    /// 1. Compared on the pre-tanh `_raw` so a deep-negative boss
    /// position can't saturate the signal away.
    #[test]
    fn evaluate_in_progress_ldd1c_rewards_clearing_enemy_slippery() {
        use std::num::NonZeroU32;
        let mut base = CombatState::default();
        base.player.max_hp = 80;
        base.player.current_hp = 60;

        let enemy_uid = NonZeroU32::new(2).expect("non-zero");
        base.creatures.push(Creature {
            id: enemy_uid,
            current_hp: 181,
            max_hp: 181,
            block: 0,
            powers: SmallVec::new(),
            species: Some(monsters::MonsterKind::Vantom),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Vantom"),
            ai: None,
        });
        base.enemy_uids.push(CreatureUid(enemy_uid));

        // Same position but with a 9-stack Slippery wall up.
        let mut with_slippery = base.clone();
        let slip_uid = PowerUid::new(7).expect("non-zero");
        with_slippery.powers.push(Power {
            id: slip_uid.0,
            kind: PowerKind::Slippery,
            amount: 9,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        with_slippery.creatures[0].powers.push(slip_uid);

        let v_wall_up = evaluate_in_progress_raw(&with_slippery);
        let v_wall_clear = evaluate_in_progress_raw(&base);
        assert!(
            v_wall_clear > v_wall_up,
            "clearing enemy Slippery must raise the eval: wall_up={v_wall_up:.4} cleared={v_wall_clear:.4}"
        );

        // The new column reads the stack count, and the dot/raw
        // invariant still holds with the wall up.
        let feats = eval_features(&with_slippery);
        let idx = EVAL_FEATURE_NAMES
            .iter()
            .position(|n| *n == "enemy_slippery_stacks")
            .expect("feature present");
        assert!(
            (feats[idx] - 9.0).abs() < 1e-6,
            "slippery column reads stack count"
        );
        let dot: f32 = feats
            .iter()
            .zip(EVAL_FEATURE_WEIGHTS.iter())
            .map(|(f, w)| f * w)
            .sum();
        let raw = evaluate_in_progress_raw(&with_slippery);
        assert!(
            (dot - raw).abs() < 1e-4,
            "dot/raw mismatch with slippery feature: dot={dot} raw={raw}"
        );
    }

    /// Bead `sts2-rs-infQb` (issue #29): regression for the
    /// minion-spawn / parasite encounter family. When the named
    /// non-minion enemy dies but minions are still alive, the engine
    /// continues combat (PersonalHive from Entomancer,
    /// PhrogParasite-on-death, Decimillipede splits, etc.). The
    /// pre-fix leaf eval read `enemy_hp_ratio = 0` (no live
    /// non-minions) and scored such positions as decisive wins
    /// (Q ≈ +5.8), driving MCTS to End-turn-stall to the round cap.
    /// The fix folds minion HP into the totals when no non-minion is
    /// alive, so the eval reflects the real remaining threat.
    #[test]
    fn evaluate_in_progress_counts_minions_when_named_enemy_dead() {
        use std::num::NonZeroU32;
        let mut state = CombatState::default();
        state.player.max_hp = 80;
        state.player.current_hp = 40;

        // Dead named enemy at slot 0 (e.g. Entomancer, HP 0/145).
        let named_uid = NonZeroU32::new(2).expect("non-zero");
        state.creatures.push(Creature {
            id: named_uid,
            current_hp: 0,
            max_hp: 145,
            block: 0,
            powers: SmallVec::new(),
            species: Some(monsters::MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Entomancer"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(named_uid));

        // Live minion (PersonalHive) at full HP, still attacking.
        let minion_uid = NonZeroU32::new(3).expect("non-zero");
        let minion_power_uid = PowerUid::new(9).expect("non-zero");
        state.powers.push(Power {
            id: minion_power_uid.0,
            kind: PowerKind::Minion,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        let mut minion_powers = SmallVec::new();
        minion_powers.push(minion_power_uid);
        state.creatures.push(Creature {
            id: minion_uid,
            current_hp: 20,
            max_hp: 20,
            block: 0,
            powers: minion_powers,
            species: Some(monsters::MonsterKind::Nibbit),
            intent: Some(monsters::Intent::MultiAttack { damage: 3, hits: 7 }),
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("PersonalHive"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(minion_uid));

        // Helper must report the minion totals as the fallback when
        // every non-minion is dead.
        let (cur, max, alive) = nonminion_enemy_hp_totals(&state);
        assert_eq!(
            (cur, max, alive),
            (20, 20, 1),
            "minion HP must back-fill when every non-minion is dead"
        );

        // The eval must score a still-live minion strictly lower
        // than the same state with the minion also dead — pre-fix
        // both registered as "decisive win" (`enemy_hp_ratio = 0`
        // either way, so `damage_dealt = 11.04 * 1.0`). Post-fix the
        // live-minion state reports ratio=20/20=1 → `damage_dealt
        // = 0`, while the all-dead state still reports ratio=0/145=0
        // → `damage_dealt = 11.04`. The Δ must be at least one full
        // `DAMAGE_DEALT_WEIGHT` step.
        let raw_live = evaluate_in_progress_raw(&state);
        let mut all_dead = state.clone();
        all_dead.creatures[1].current_hp = 0;
        let raw_dead = evaluate_in_progress_raw(&all_dead);
        assert!(
            raw_dead - raw_live >= DAMAGE_DEALT_WEIGHT * 0.9,
            "live-minion state must score strictly lower than all-dead: \
             live={raw_live} dead={raw_dead} weight={DAMAGE_DEALT_WEIGHT}"
        );
    }

    /// Bead `sts2-rs-ymvk` (bottled_ai audit B-2): with Barricade up,
    /// end-of-turn block carries to the next turn and must score
    /// strictly higher than the same state without Barricade. We
    /// build two states identical except for the Barricade power on
    /// the player's mirror Creature, give the player surplus block
    /// (more than `net_incoming`) so the regular threat term is
    /// already saturated, and assert the Barricade state evaluates
    /// strictly higher.
    #[test]
    fn evaluate_in_progress_barricade_rewards_block_carryover() {
        use std::num::NonZeroU32;
        let mut base = CombatState::default();
        base.player.max_hp = 80;
        base.player.current_hp = 40;
        // Surplus block: net_incoming below is 10, block is 30 — the
        // existing threat term saturates at 0 unblocked, so any
        // difference must come from the new barricade_block term.
        base.player.block = 30;

        let player_uid = NonZeroU32::new(1).expect("non-zero");
        base.player.creature_uid = Some(CreatureUid(player_uid));
        base.creatures.push(Creature {
            id: player_uid,
            current_hp: 40,
            max_hp: 80,
            block: 30,
            powers: SmallVec::new(),
            species: None,
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId(""),
            ai: None,
        });

        let enemy_uid = NonZeroU32::new(2).expect("non-zero");
        base.creatures.push(Creature {
            id: enemy_uid,
            current_hp: 50,
            max_hp: 50,
            block: 0,
            powers: SmallVec::new(),
            species: Some(monsters::MonsterKind::Nibbit),
            intent: Some(monsters::Intent::Attack { damage: 10 }),
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId("Nibbit"),
            ai: None,
        });
        base.enemy_uids.push(CreatureUid(enemy_uid));

        let v_no_barricade = evaluate_in_progress(&base);

        let mut with_barricade = base.clone();
        let barricade_uid = PowerUid::new(7).expect("non-zero");
        with_barricade.powers.push(Power {
            id: barricade_uid.0,
            kind: PowerKind::Barricade,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        with_barricade.creatures[0].powers.push(barricade_uid);

        let v_barricade = evaluate_in_progress(&with_barricade);

        assert!(
            v_barricade > v_no_barricade,
            "Barricade must credit end-of-turn block carryover: \
             without={v_no_barricade:.4} with={v_barricade:.4}"
        );
        assert!(!player_has_barricade(&base));
        assert!(player_has_barricade(&with_barricade));
    }

    /// Bead `sts2-rs-kpk5` (bottled_ai audit B-6): tiered player-side
    /// buff counters score the eval strictly higher when the player
    /// owns a LOVE-tier Power. Pin DemonForm amount=3 on the player
    /// mirror Creature and compare against the same state without it;
    /// the LOVE-tier feature with weight +3.0 must produce a strictly
    /// positive delta.
    #[test]
    fn evaluate_in_progress_kpk5_rewards_love_tier_buff() {
        use std::num::NonZeroU32;
        let mut base = CombatState::default();
        base.player.max_hp = 80;
        base.player.current_hp = 40;

        let player_uid = NonZeroU32::new(1).expect("non-zero");
        base.player.creature_uid = Some(CreatureUid(player_uid));
        base.creatures.push(Creature {
            id: player_uid,
            current_hp: 40,
            max_hp: 80,
            block: 0,
            powers: SmallVec::new(),
            species: None,
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId(""),
            ai: None,
        });

        let v_no_demon = evaluate_in_progress(&base);

        let mut with_demon = base.clone();
        let demon_uid = PowerUid::new(7).expect("non-zero");
        with_demon.powers.push(Power {
            id: demon_uid.0,
            kind: PowerKind::DemonForm,
            amount: 3,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        with_demon.creatures[0].powers.push(demon_uid);

        let v_demon = evaluate_in_progress(&with_demon);

        assert!(
            v_demon > v_no_demon,
            "LOVE-tier buff (DemonForm 3) must score strictly higher: \
             without={v_no_demon:.4} with={v_demon:.4}"
        );

        let tiers_no = player_buff_tier_amounts(&base);
        let tiers_demon = player_buff_tier_amounts(&with_demon);
        assert_eq!(tiers_no, [0, 0, 0, 0]);
        assert_eq!(tiers_demon, [3, 0, 0, 0]);
    }

    #[test]
    fn evaluate_in_progress_low_hp_cliff_amplifies_penalty() {
        // bead sts2-rs-9nzs: states under 20% HP score strictly worse
        // than the linear hp_ratio term alone predicts. Compare 5% HP
        // (deep in the cliff zone) vs 20% HP (right at threshold;
        // cliff contribution = 0). The cliff must contribute at least
        // 50% more drop than the linear term alone would produce.
        let mut at_threshold = CombatState::default();
        at_threshold.player.max_hp = 100;
        at_threshold.player.current_hp = 20;
        let mut below = at_threshold.clone();
        below.player.current_hp = 5;
        let v_threshold = evaluate_in_progress(&at_threshold);
        let v_below = evaluate_in_progress(&below);
        let linear_only_drop = 0.15 * PLAYER_HP_WEIGHT / IN_PROGRESS_DIVISOR;
        let actual_drop = v_threshold - v_below;
        assert!(
            actual_drop > linear_only_drop * 1.5,
            "cliff must amplify low-HP penalty: linear_only={linear_only_drop:.4} \
             actual={actual_drop:.4}"
        );
    }

    #[test]
    fn evaluate_in_progress_rewards_higher_player_hp() {
        // bead sts2-rs-0962: player_hp_ratio term breaks the "5 HP and
        // 80 HP look the same" compression. Two otherwise-identical
        // states must yield a strictly higher eval for the healthier
        // player.
        let mut low = CombatState::default();
        low.player.max_hp = 80;
        low.player.current_hp = 5;
        let mut high = low.clone();
        high.player.current_hp = 80;
        let v_low = evaluate_in_progress(&low);
        let v_high = evaluate_in_progress(&high);
        assert!(
            v_high > v_low,
            "higher HP must score higher: low={v_low} high={v_high}"
        );
    }

    #[test]
    fn evaluate_in_progress_penalizes_wound_in_hand() {
        // bead sts2-rs-gzbh: adding a Wound (CardType::Status) to the
        // player's hand must monotonically decrease the leaf eval.
        // Builds a state with no curses/status held, then pushes a
        // Wound CardUid into both the arena and `player.hand`, and
        // asserts the post-eval is strictly less than the pre-eval.
        use crate::cards::{Card, CardId, CardType};
        use crate::CardUid;

        let mut state = CombatState::default();
        state.player.max_hp = 80;
        state.player.current_hp = 80;
        let v_clean = evaluate_in_progress(&state);

        // Allocate a Wound into the arena and into the player's hand.
        // The uid is one past the existing arena length (current default
        // is empty), so the lookup-by-uid in `evaluate_in_progress_raw`
        // finds the new card and classifies it as Status.
        let next_raw = u32::try_from(state.cards.len() + 1).expect("arena fits in u32");
        let uid = CardUid::new(next_raw).expect("non-zero");
        let mut wound = Card::pod_defaults();
        wound.uid = uid;
        wound.card_id = CardId::Wound;
        wound.card_type = CardType::Status;
        state.cards.push(wound);
        state.player.hand.push(uid);
        let v_with_wound = evaluate_in_progress(&state);

        assert!(
            v_with_wound < v_clean,
            "adding a Wound must decrease the leaf eval: clean={v_clean} with_wound={v_with_wound}"
        );

        // Second wound must decrease further (monotonicity, not just a
        // single-step penalty). Reuses the same factory + arena pattern.
        let next_raw2 = u32::try_from(state.cards.len() + 1).expect("arena fits in u32");
        let uid2 = CardUid::new(next_raw2).expect("non-zero");
        let mut wound2 = Card::pod_defaults();
        wound2.uid = uid2;
        wound2.card_id = CardId::Wound;
        wound2.card_type = CardType::Status;
        state.cards.push(wound2);
        state.player.hand.push(uid2);
        let v_with_two_wounds = evaluate_in_progress(&state);
        assert!(
            v_with_two_wounds < v_with_wound,
            "second wound must decrease eval again: one={v_with_wound} two={v_with_two_wounds}"
        );
    }

    /// Bead `sts2-rs-mbj0` (bottled_ai audit B-8): exhausting a Curse
    /// or Status must monotonically *increase* the leaf eval — the
    /// permanent deck-purification reward. Two-step: clean → wound in
    /// hand (penalty) → wound moved to exhaust (eval recovers and then
    /// nets to a higher value than clean because of the reward).
    #[test]
    fn evaluate_in_progress_rewards_curse_in_exhaust_pile() {
        use crate::cards::{Card, CardId, CardType};
        use crate::CardUid;

        let mut state = CombatState::default();
        state.player.max_hp = 80;
        state.player.current_hp = 80;
        let v_clean = evaluate_in_progress(&state);

        let next_raw = u32::try_from(state.cards.len() + 1).expect("arena fits in u32");
        let uid = CardUid::new(next_raw).expect("non-zero");
        let mut wound = Card::pod_defaults();
        wound.uid = uid;
        wound.card_id = CardId::Wound;
        wound.card_type = CardType::Status;
        state.cards.push(wound);

        // First: wound sitting in hand (penalty path — pins the
        // existing gzbh contract under the new bigger schema).
        state.player.hand.push(uid);
        let v_in_hand = evaluate_in_progress(&state);
        assert!(
            v_in_hand < v_clean,
            "wound in hand must lower eval: clean={v_clean} in_hand={v_in_hand}"
        );

        // Now move the wound from hand to exhaust pile. The
        // hand-penalty disappears; the exhaust-pile reward fires.
        state.player.hand.clear();
        state.player.exhaust.push(uid);
        let v_in_exhaust = evaluate_in_progress(&state);
        assert!(
            v_in_exhaust > v_clean,
            "wound in exhaust must raise eval above clean baseline: \
             clean={v_clean} in_exhaust={v_in_exhaust}"
        );
        assert!(
            v_in_exhaust > v_in_hand,
            "moving wound from hand to exhaust must raise eval: \
             in_hand={v_in_hand} in_exhaust={v_in_exhaust}"
        );
    }

    // ----------------------------------------------------------------
    // Mutation-coverage tests for sts2-core helpers, surfaced by the
    // 2026-06-20 cargo-mutants sweep: functions executed but unasserted.
    // ----------------------------------------------------------------
    use std::num::NonZeroU32;

    fn mg_creature(id: NonZeroU32, current_hp: i32, max_hp: i32) -> Creature {
        Creature {
            id,
            current_hp,
            max_hp,
            block: 0,
            powers: SmallVec::new(),
            species: None,
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: crate::MonsterId(""),
            ai: None,
        }
    }

    fn mg_power(id: PowerUid, kind: PowerKind, amount: i32) -> Power {
        Power {
            id: id.0,
            kind,
            amount,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        }
    }

    #[test]
    fn time_pressure_signal_pins_softplus_shape() {
        let at = time_pressure_signal(5, 5);
        assert!(
            (at - -std::f32::consts::LN_2).abs() < 1e-4,
            "signal(round==threshold) must equal -ln2, got {at}"
        );
        let below = time_pressure_signal(0, 99);
        assert!(
            below.abs() < 1e-4,
            "deep sub-threshold must be ~0, got {below}"
        );
        let above = time_pressure_signal(205, 5);
        assert!(
            (above - -200.0).abs() < 1e-2,
            "deep post-threshold must ramp to -(round-threshold), got {above}"
        );
        assert!(
            at <= 0.0 && below <= 0.0 && above <= 0.0,
            "signal must be <= 0"
        );
        assert!(
            time_pressure_signal(15, 5) < time_pressure_signal(6, 5),
            "signal must strictly decrease past the threshold"
        );
    }

    #[test]
    fn monster_pressure_threshold_pins_vantom_and_default() {
        assert_eq!(monster_pressure_threshold(monsters::MonsterKind::Vantom), 5);
        assert_eq!(
            monster_pressure_threshold(monsters::MonsterKind::Nibbit),
            99
        );
    }

    #[test]
    fn evaluate_victory_pins_calibration() {
        let mk = |hp: i32, potions: usize, round: u16| {
            let mut s = CombatState {
                is_over: true,
                player_won: true,
                ..CombatState::default()
            };
            s.player.max_hp = 100;
            s.player.current_hp = hp;
            for _ in 0..potions {
                s.player
                    .potions
                    .push(crate::potions::PotionKind::BlockPotion);
            }
            s.round = round;
            evaluate_victory(&s)
        };
        // Pinned at VICTORY_POTION_HP_EQUIV = 8 (range [3500, 17500]).
        assert!((mk(50, 0, 0) - 0.678_571).abs() < 1e-3, "50hp/0pot/r0");
        assert!(
            (mk(50, 4, 0) - 0.792_857).abs() < 1e-3,
            "50hp/4pot/r0 pins potions*8"
        );
        assert!(
            (mk(50, 0, 50) - 0.676_786).abs() < 1e-4,
            "50hp/0pot/r50 pins -round*0.01"
        );
    }

    /// One HP-equivalent in the victory band is worth `0.5 * 100 / 14000` Q —
    /// the same exchange rate [`evaluate_victory`] gives one point of
    /// remaining HP. Pins the root-level potion opportunity-cost charge's
    /// unit conversion (GH #447 follow-up).
    #[test]
    #[allow(clippy::float_cmp)] // 0.0 * k == 0.0 exactly.
    fn victory_hp_equiv_to_q_matches_band_arithmetic() {
        assert!((victory_hp_equiv_to_q(1.0) - 0.003_571_4).abs() < 1e-6);
        assert!((victory_hp_equiv_to_q(8.0) - 0.028_571_4).abs() < 1e-6);
        assert_eq!(victory_hp_equiv_to_q(0.0), 0.0);
    }

    /// Unset ⇒ λ defaults to the deployed `VICTORY_POTION_HP_EQUIV` and the
    /// in-progress term collapses to the tuned `POTIONS_WEIGHT` (2.0), so the
    /// dot-product invariant + calibration pins above hold untouched (#253).
    #[test]
    #[allow(clippy::float_cmp)] // exact-by-construction: default const / parsed literal.
    fn potion_weight_defaults_to_victory_hp_equiv_when_env_unset() {
        // This test's process must not have the override set.
        std::env::remove_var("STS2_RUNAI_POTION_WEIGHT");
        assert_eq!(potion_weight(), VICTORY_POTION_HP_EQUIV);
        assert!((in_progress_potion_weight() - POTIONS_WEIGHT).abs() < 1e-6);
    }

    /// The env override scales both terms: victory = λ, in-progress = the tuned
    /// base scaled by the sweep ratio `λ / VICTORY_POTION_HP_EQUIV` (#253). Runs
    /// in its own nextest process so the `OnceLock` caches this λ.
    #[test]
    #[allow(clippy::float_cmp)] // exact-by-construction: parsed literal / ratio.
    fn potion_weight_env_override_scales_victory_and_in_progress() {
        std::env::set_var("STS2_RUNAI_POTION_WEIGHT", "12");
        assert_eq!(potion_weight(), 12.0);
        // in-progress = POTIONS_WEIGHT * (12 / 8) = 2.0 * 1.5 = 3.0
        assert!((in_progress_potion_weight() - 3.0).abs() < 1e-6);
        // A win terminal holding potions is worth strictly more at λ=12 than the
        // same state's potion-free value plus the round penalty would allow.
        let mut s = CombatState {
            is_over: true,
            player_won: true,
            ..CombatState::default()
        };
        s.player.max_hp = 100;
        s.player.current_hp = 50;
        let no_potions = evaluate_victory(&s);
        s.player
            .potions
            .push(crate::potions::PotionKind::BlockPotion);
        assert!(
            evaluate_victory(&s) > no_potions,
            "held potion must raise the victory eval under λ=12"
        );
        std::env::remove_var("STS2_RUNAI_POTION_WEIGHT");
    }

    /// Unset ⇒ w defaults to 1.0: the surplus vanishes and the in-progress
    /// HP term collapses to the tuned constant, so the dot-product
    /// invariant + calibration pins above hold untouched (2026-07-18
    /// HP-repricing campaign).
    #[test]
    #[allow(clippy::float_cmp)] // exact-by-construction: default const.
    fn hp_weight_defaults_to_one_when_env_unset() {
        // This test's process must not have the override set.
        std::env::remove_var("STS2_RUNAI_HP_WEIGHT");
        assert_eq!(hp_weight(), 1.0);
        assert!((in_progress_hp_weight() - PLAYER_HP_WEIGHT).abs() < 1e-6);
        let mut s = CombatState {
            is_over: true,
            player_won: true,
            ..CombatState::default()
        };
        s.player.max_hp = 100;
        s.player.current_hp = 80;
        assert_eq!(victory_hp_surplus(&s), 0.0);
    }

    /// The env override prices HP at exactly `w ×` the default in-band
    /// gradient via the OUT-OF-BAND surplus — the clamped victory band
    /// itself stays untouched (the pilot-1 in-band version was
    /// dose-compressed to ≤1.4× by the fixed band width). Runs in its
    /// own nextest process so the `OnceLock` caches this w.
    #[test]
    #[allow(clippy::float_cmp)] // exact-by-construction: parsed literal.
    fn hp_weight_env_override_prices_surplus_out_of_band() {
        std::env::set_var("STS2_RUNAI_HP_WEIGHT", "4");
        assert_eq!(hp_weight(), 4.0);
        let mk = |hp: i32| {
            let mut s = CombatState {
                is_over: true,
                player_won: true,
                ..CombatState::default()
            };
            s.player.max_hp = 100;
            s.player.current_hp = hp;
            s
        };
        // The clamped band is IDENTICAL to deployed pricing under any w.
        assert!(
            (evaluate_victory(&mk(50)) - 0.678_571).abs() < 1e-3,
            "clamped band must not move under w=4"
        );
        // Surplus = (w−1) · hp · VICTORY_HP_UNIT: the total per-HP price
        // (in-band unit + surplus) is exactly w × the default unit.
        let total_60 = evaluate_victory(&mk(60)) + victory_hp_surplus(&mk(60));
        let total_20 = evaluate_victory(&mk(20)) + victory_hp_surplus(&mk(20));
        assert!(
            (total_60 - total_20 - 4.0 * 40.0 * VICTORY_HP_UNIT).abs() < 1e-4,
            "40 HP must be worth exactly 4× the default gradient"
        );
        std::env::remove_var("STS2_RUNAI_HP_WEIGHT");
    }

    #[test]
    fn nonminion_enemy_hp_totals_counts_live_nonminion_enemies() {
        let player_uid = NonZeroU32::new(1).expect("non-zero");
        let a_uid = NonZeroU32::new(2).expect("non-zero");
        let b_uid = NonZeroU32::new(3).expect("non-zero");

        let mut s = CombatState::default();
        s.player.creature_uid = Some(CreatureUid(player_uid));
        s.creatures.push(mg_creature(player_uid, 70, 80));

        let weak_uid = PowerUid::new(7).expect("non-zero");
        s.powers.push(mg_power(weak_uid, PowerKind::Weak, 1));
        let mut a = mg_creature(a_uid, 30, 50);
        a.powers.push(weak_uid);
        s.creatures.push(a);
        s.creatures.push(mg_creature(b_uid, 20, 40));
        s.enemy_uids.push(CreatureUid(a_uid));
        s.enemy_uids.push(CreatureUid(b_uid));

        assert_eq!(nonminion_enemy_hp_totals(&s), (50, 90, 2));
    }

    #[test]
    fn nonminion_enemy_hp_totals_minion_fallback_edges() {
        let live_uid = NonZeroU32::new(2).expect("non-zero");
        let minion_uid = NonZeroU32::new(3).expect("non-zero");
        let minion_power = PowerUid::new(7).expect("non-zero");

        let mut s = CombatState::default();
        s.powers.push(mg_power(minion_power, PowerKind::Minion, 1));
        s.creatures.push(mg_creature(live_uid, 30, 50));
        let mut m = mg_creature(minion_uid, 10, 20);
        m.powers.push(minion_power);
        s.creatures.push(m);
        s.enemy_uids.push(CreatureUid(live_uid));
        s.enemy_uids.push(CreatureUid(minion_uid));
        assert_eq!(
            nonminion_enemy_hp_totals(&s),
            (30, 50, 1),
            "live non-minion present → no minion fallback"
        );

        let mut dead = CombatState::default();
        dead.powers
            .push(mg_power(minion_power, PowerKind::Minion, 1));
        let mut dm = mg_creature(minion_uid, 0, 20);
        dm.powers.push(minion_power);
        dead.creatures.push(dm);
        dead.enemy_uids.push(CreatureUid(minion_uid));
        assert_eq!(
            nonminion_enemy_hp_totals(&dead),
            (0, 0, 0),
            "a dead minion must not surface via the fallback"
        );
    }

    #[test]
    fn player_buff_tier_and_debuff_amount_pin_like_meh_and_sum() {
        let player_uid = NonZeroU32::new(1).expect("non-zero");
        let mut s = CombatState::default();
        s.player.creature_uid = Some(CreatureUid(player_uid));
        s.creatures.push(mg_creature(player_uid, 40, 80));

        let metal = PowerUid::new(7).expect("non-zero");
        s.powers.push(mg_power(metal, PowerKind::Metallicize, 4));
        s.creatures[0].powers.push(metal);
        let blur = PowerUid::new(8).expect("non-zero");
        s.powers.push(mg_power(blur, PowerKind::Blur, 2));
        s.creatures[0].powers.push(blur);
        let weak1 = PowerUid::new(9).expect("non-zero");
        s.powers.push(mg_power(weak1, PowerKind::Weak, 1));
        s.creatures[0].powers.push(weak1);
        let weak2 = PowerUid::new(10).expect("non-zero");
        s.powers.push(mg_power(weak2, PowerKind::Weak, 2));
        s.creatures[0].powers.push(weak2);

        assert_eq!(player_buff_tier(PowerKind::Metallicize), BuffTier::Like);
        assert_eq!(player_buff_tier(PowerKind::Blur), BuffTier::Meh);
        assert_eq!(player_buff_tier_amounts(&s), [0, 4, 2, 0]);
        assert_eq!(player_debuff_amount(&s, PowerKind::Weak), 3);
        assert_eq!(player_debuff_amount(&s, PowerKind::Frail), 0);
        let mut no_mirror = s.clone();
        no_mirror.player.creature_uid = None;
        assert_eq!(player_debuff_amount(&no_mirror, PowerKind::Weak), 0);
    }

    #[test]
    fn eval_features_dot_weights_covers_debuff_and_cliff_columns() {
        let player_uid = NonZeroU32::new(1).expect("non-zero");
        let enemy_uid = NonZeroU32::new(2).expect("non-zero");

        let mut s = CombatState::default();
        s.player.max_hp = 100;
        s.player.current_hp = 5;
        s.player.creature_uid = Some(CreatureUid(player_uid));
        s.creatures.push(mg_creature(player_uid, 5, 100));

        let pv = PowerUid::new(7).expect("non-zero");
        s.powers.push(mg_power(pv, PowerKind::Vulnerable, 2));
        s.creatures[0].powers.push(pv);
        let pw = PowerUid::new(8).expect("non-zero");
        s.powers.push(mg_power(pw, PowerKind::Weak, 1));
        s.creatures[0].powers.push(pw);
        let pf = PowerUid::new(9).expect("non-zero");
        s.powers.push(mg_power(pf, PowerKind::Frail, 1));
        s.creatures[0].powers.push(pf);

        let mut enemy = mg_creature(enemy_uid, 40, 50);
        let ew = PowerUid::new(10).expect("non-zero");
        s.powers.push(mg_power(ew, PowerKind::Weak, 3));
        enemy.powers.push(ew);
        let ea = PowerUid::new(11).expect("non-zero");
        s.powers.push(mg_power(ea, PowerKind::Artifact, 1));
        enemy.powers.push(ea);
        s.creatures.push(enemy);
        s.enemy_uids.push(CreatureUid(enemy_uid));

        {
            use crate::cards::{Card, CardId, CardType};
            use crate::CardUid;
            let next_raw = u32::try_from(s.cards.len() + 1).expect("fits u32");
            let uid = CardUid::new(next_raw).expect("non-zero");
            let mut wound = Card::pod_defaults();
            wound.uid = uid;
            wound.card_id = CardId::Wound;
            wound.card_type = CardType::Status;
            s.cards.push(wound);
            s.player.hand.push(uid);
        }

        let feats = eval_features(&s);
        for name in [
            "weak_stacks",
            "cliff_excess_sq",
            "curse_in_hand",
            "player_vuln_stacks",
            "player_weak_stacks",
            "player_frail_stacks",
            "enemy_artifact_stacks",
        ] {
            let idx = EVAL_FEATURE_NAMES
                .iter()
                .position(|n| *n == name)
                .unwrap_or_else(|| panic!("column {name} present"));
            assert!(
                feats[idx].abs() > 0.0,
                "fixture must drive {name} non-zero, got {}",
                feats[idx]
            );
        }

        let dot: f32 = feats
            .iter()
            .zip(EVAL_FEATURE_WEIGHTS.iter())
            .map(|(f, w)| f * w)
            .sum();
        let raw = evaluate_in_progress_raw(&s);
        assert!(
            (dot - raw).abs() < 1e-4,
            "weighted feature dot must equal raw evaluator across all columns: \
             dot={dot} raw={raw}"
        );
    }

    #[test]
    fn player_has_power_predicates_reject_zero_amount_and_foreign_powers() {
        let player_uid = NonZeroU32::new(1).expect("non-zero");
        let with_power = |kind: PowerKind, amount: i32| {
            let mut s = CombatState::default();
            s.player.creature_uid = Some(CreatureUid(player_uid));
            s.creatures.push(mg_creature(player_uid, 40, 80));
            let p = PowerUid::new(7).expect("non-zero");
            s.powers.push(mg_power(p, kind, amount));
            s.creatures[0].powers.push(p);
            s
        };

        assert!(!player_has_intangible(&with_power(
            PowerKind::Intangible,
            0
        )));
        assert!(player_has_intangible(&with_power(PowerKind::Intangible, 1)));
        assert!(!player_has_barricade(&with_power(PowerKind::Barricade, 0)));
        assert!(player_has_barricade(&with_power(PowerKind::Barricade, 1)));
        assert!(!player_has_block_preserving_power(&with_power(
            PowerKind::Metallicize,
            0
        )));
        assert!(player_has_block_preserving_power(&with_power(
            PowerKind::Metallicize,
            3
        )));
        assert!(!player_has_intangible(&with_power(PowerKind::Strength, 5)));
        assert!(!player_has_barricade(&with_power(PowerKind::Strength, 5)));
        assert!(!player_has_block_preserving_power(&with_power(
            PowerKind::Strength,
            5
        )));
    }

    // ---- potions ----

    #[test]
    fn mg_procure_random_potion_empty_belt_pushes_one_eligible() {
        let mut state = CombatState::default();
        state.player.character = Some(crate::potions::CharacterId::Ironclad);
        assert!(state.player.potions.is_empty());

        let added = state.procure_random_potion();

        assert!(
            added,
            "procure on an empty belt with a character must return true"
        );
        assert_eq!(
            state.player.potions.len(),
            1,
            "exactly one potion must be pushed"
        );
        let pushed = state.player.potions[0];
        assert!(
            !pushed.is_event_only(),
            "pushed potion must not be event-only: {pushed:?}"
        );
        match pushed.character_lock() {
            None => {}
            Some(locked) => assert_eq!(
                locked,
                crate::potions::CharacterId::Ironclad,
                "pushed potion must not be locked to a different character: {pushed:?}"
            ),
        }
    }

    #[test]
    fn mg_procure_random_potion_at_cap_is_noop() {
        let mut state = CombatState::default();
        state.player.character = Some(crate::potions::CharacterId::Ironclad);
        state.player.potion_slots = 1;
        assert_eq!(state.player.effective_potion_slots(), 1);
        state
            .player
            .potions
            .push(crate::potions::PotionKind::BlockPotion);
        assert_eq!(state.player.potions.len(), 1);

        let added = state.procure_random_potion();

        assert!(!added, "procure at the slot cap must return false");
        assert_eq!(
            state.player.potions.len(),
            1,
            "procure at the slot cap must add nothing"
        );
    }

    #[test]
    fn mg_fill_empty_potion_slots_fills_to_cap_and_counts() {
        let mut state = CombatState::default();
        state.player.character = Some(crate::potions::CharacterId::Ironclad);
        state.player.potion_slots = 3;
        assert_eq!(state.player.effective_potion_slots(), 3);
        assert!(state.player.potions.is_empty());

        let filled = state.fill_empty_potion_slots();

        assert_eq!(filled, 3, "must report exactly 3 slots filled");
        assert_eq!(
            state.player.potions.len(),
            3,
            "belt must be filled to the 3-slot cap"
        );
    }

    // ---- creature management ----

    fn mg_test_ai() -> crate::monster_ai::MonsterAi {
        use crate::monster_ai::{MonsterState, MoveState};
        use smallvec::smallvec;
        use std::collections::BTreeMap;
        fn noop(_: &mut CombatState, _: CreatureUid) {}
        let mut states: BTreeMap<&'static str, MonsterState> = BTreeMap::new();
        states.insert(
            "A",
            MonsterState::Move(MoveState {
                id: "A",
                effect: noop,
                intents: smallvec![Intent::Attack { damage: 1 }],
                follow_up: Some("A"),
                must_perform_once: false,
            }),
        );
        crate::monster_ai::MonsterAi::new(states, "A")
    }

    #[test]
    fn summon_pet_reuses_live_pet_of_same_species() {
        let mut state = CombatState::default();
        let first = state.summon_pet(MonsterKind::Nibbit, 20);
        let len_after_first = state.creatures.len();

        let second = state.summon_pet(MonsterKind::Nibbit, 20);
        assert_eq!(
            second, first,
            "second summon of same live species must reuse the pet uid"
        );
        assert_eq!(
            state.creatures.len(),
            len_after_first,
            "reuse must NOT push a duplicate"
        );

        let other = state.summon_pet(MonsterKind::Mawler, 15);
        assert_ne!(other, first, "distinct species must summon a fresh pet");
        assert_eq!(
            state.creatures.len(),
            len_after_first + 1,
            "distinct species must push exactly one new creature"
        );
    }

    #[test]
    fn summon_pet_skips_dead_corpse_and_summons_fresh() {
        let mut state = CombatState::default();
        let first = state.summon_pet(MonsterKind::Nibbit, 20);
        let idx = state
            .creatures
            .iter()
            .position(|c| c.id == first.0)
            .expect("pet present");
        state.creatures[idx].current_hp = 0;
        let len_with_corpse = state.creatures.len();

        let fresh = state.summon_pet(MonsterKind::Nibbit, 20);
        assert_ne!(fresh, first, "a 0-HP corpse is not a valid reuse target");
        assert_eq!(
            state.creatures.len(),
            len_with_corpse + 1,
            "fresh summon must push a creature"
        );
        assert!(
            state
                .creatures
                .iter()
                .any(|c| c.id == fresh.0 && c.current_hp > 0),
            "the fresh pet must be alive"
        );
    }

    #[test]
    fn set_creature_ai_targets_only_matching_uid() {
        let mut state = CombatState::default();
        let a = NonZeroU32::new(1).expect("non-zero");
        let b = NonZeroU32::new(2).expect("non-zero");
        state.creatures.push(mg_creature(a, 10, 10));
        state.creatures.push(mg_creature(b, 10, 10));

        state.set_creature_ai(CreatureUid(a), mg_test_ai());

        let row_a = state.creatures.iter().find(|c| c.id == a).expect("a");
        let row_b = state.creatures.iter().find(|c| c.id == b).expect("b");
        assert!(row_a.ai.is_some(), "target creature must receive the AI");
        assert!(row_b.ai.is_none(), "non-target creature must be untouched");
    }

    /// Build a slotted encounter: `n` enemies with AIs, stamped into
    /// `member_slots`. Mirrors what combat entry does.
    fn mg_slotted(member_slots: &[u8]) -> (CombatState, Vec<CreatureUid>) {
        let mut state = CombatState::default();
        let mut uids = Vec::new();
        for i in 0..member_slots.len() {
            let id = NonZeroU32::new(u32::try_from(i + 1).expect("fits")).expect("non-zero");
            state.creatures.push(mg_creature(id, 10, 10));
            let uid = CreatureUid(id);
            state.enemy_uids.push(uid);
            state.set_creature_ai(uid, mg_test_ai());
            uids.push(uid);
        }
        state.assign_encounter_slots(member_slots);
        (state, uids)
    }

    fn mg_kill(state: &mut CombatState, uid: CreatureUid) {
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
            c.current_hp = 0;
        }
    }

    /// The two free-slot rules are DIFFERENT and both are in C#:
    /// `EncounterModel.GetNextSlot` is `FirstOrDefault`, while
    /// `TwoTailedRat.CallForBackup` / `Ovicopter.LayEggsMove` inline
    /// `LastOrDefault`. Picking the wrong one puts a summon in the wrong slot,
    /// which reorders the turn AND the positional `target_index` (GH #552).
    #[test]
    fn free_slot_rules_are_first_and_last_and_skip_the_occupied() {
        // Two-Tailed Rats: seeds in 2/3/4 of five, so 0 and 1 are free.
        let (state, _) = mg_slotted(&[2, 3, 4]);
        assert_eq!(state.first_free_enemy_slot(5), Some(0));
        assert_eq!(state.last_free_enemy_slot(5), Some(1));
        // A full board yields None, which is C#'s empty-string default and the
        // `if (!string.IsNullOrEmpty(nextSlot))` guard every summoner writes.
        let (full, _) = mg_slotted(&[0, 1, 2]);
        assert_eq!(full.first_free_enemy_slot(3), None);
        assert_eq!(full.last_free_enemy_slot(3), None);
    }

    /// C# drops a corpse from `Enemies`, so its slot frees up and a later
    /// summon can take it. Rust retains corpses in `enemy_uids`, so the
    /// helpers filter on liveness by hand — the bug this guards is a summon
    /// that refuses a slot whose holder is already dead.
    #[test]
    fn a_dead_holder_frees_its_slot() {
        let (mut state, uids) = mg_slotted(&[0, 1, 2]);
        assert_eq!(state.first_free_enemy_slot(3), None, "full while all alive");
        mg_kill(&mut state, uids[1]);
        assert_eq!(state.first_free_enemy_slot(3), Some(1), "slot 1 recycled");
        assert_eq!(state.last_free_enemy_slot(3), Some(1));
    }

    /// The insert index is what reproduces `SortEnemiesBySlotName`: a summon
    /// lands amongst the LIVING in slot order, not on the end.
    #[test]
    fn slot_insert_index_keeps_living_enemies_slot_ascending() {
        // Fabricator shape: bots at 0 and 1, the Fabricator at 2.
        let (state, _) = mg_slotted(&[0, 1, 2]);
        // A TIE appends after the incumbent rather than displacing it: the
        // scan is `key > slot`, strictly. Ties are reachable — Fogmog and
        // TheObscura add into a LITERAL slot with no free check, so a second
        // illusion shares slot 0 with the first.
        assert_eq!(state.slot_insert_index(0), 1, "tie lands after the holder");
        assert_eq!(state.slot_insert_index(3), 3, "above everything → append");
        // Ovicopter shape: a freed MIDDLE slot must land between survivors,
        // which is exactly what a flat front-insert or append got wrong.
        let (mut state, uids) = mg_slotted(&[0, 2, 3]);
        mg_kill(&mut state, uids[0]);
        assert_eq!(
            state.slot_insert_index(1),
            1,
            "slot 1 sits after the corpse at index 0 and before the living slot-2"
        );
        // A corpse never blocks: it is skipped, not compared.
        assert_eq!(state.slot_insert_index(4), 3);
    }

    /// A creature with no recorded slot never displaces a newcomer. That
    /// fallback is what keeps a half-stamped or unstamped encounter degrading
    /// to plain append instead of scrambling the roster.
    #[test]
    fn unslotted_living_enemies_do_not_displace_a_summon() {
        let mut state = CombatState::default();
        for i in 0..2u32 {
            let id = NonZeroU32::new(i + 1).expect("non-zero");
            state.creatures.push(mg_creature(id, 10, 10));
            state.enemy_uids.push(CreatureUid(id));
            state.set_creature_ai(CreatureUid(id), mg_test_ai());
        }
        // Nothing stamped at all.
        assert_eq!(state.first_free_enemy_slot(3), Some(0));
        assert_eq!(state.slot_insert_index(0), 2, "append, not front");
    }

    /// `spawn_enemy_in_slot` is the three steps every summoner needs — place,
    /// attach the AI, record the slot — in the order the AI-backed slot store
    /// requires.
    #[test]
    fn spawn_enemy_in_slot_places_records_and_attaches() {
        let (mut state, uids) = mg_slotted(&[2, 3, 4]);
        let slot = state.last_free_enemy_slot(5).expect("1 is free");
        let new = state.spawn_enemy_in_slot(MonsterKind::ShrinkerBeetle, slot, mg_test_ai());
        assert_eq!(slot, 1);
        assert_eq!(state.enemy_uids[0], new, "slot 1 sorts ahead of 2/3/4");
        assert_eq!(state.enemy_slot(new), Some(1));
        assert!(state
            .creatures
            .iter()
            .find(|c| c.id == new.0)
            .is_some_and(|c| c.ai.is_some()));
        // And the pre-existing roster is untouched, in order.
        assert_eq!(&state.enemy_uids[1..], &uids[..]);
    }

    #[test]
    fn set_enemy_slot_targets_only_matching_uid() {
        let mut state = CombatState::default();
        let a = NonZeroU32::new(1).expect("non-zero");
        let b = NonZeroU32::new(2).expect("non-zero");
        state.creatures.push(mg_creature(a, 10, 10));
        state.creatures.push(mg_creature(b, 10, 10));
        state.set_creature_ai(CreatureUid(a), mg_test_ai());
        state.set_creature_ai(CreatureUid(b), mg_test_ai());

        state.set_enemy_slot(CreatureUid(a), 3);

        assert_eq!(
            state.enemy_slot(CreatureUid(a)),
            Some(3),
            "target slot recorded"
        );
        assert_eq!(
            state.enemy_slot(CreatureUid(b)),
            None,
            "non-target slot untouched"
        );
    }

    #[test]
    fn sync_creature_current_hp_from_player_copies_into_mirror() {
        let mut state = CombatState::default();
        let pid = NonZeroU32::new(1).expect("non-zero");
        state.creatures.push(mg_creature(pid, 12, 80));
        state.player.creature_uid = Some(CreatureUid(pid));
        state.player.current_hp = 47;

        state.sync_creature_current_hp_from_player();

        let row = state
            .creatures
            .iter()
            .find(|c| c.id == pid)
            .expect("mirror");
        assert_eq!(
            row.current_hp, 47,
            "mirror current_hp must equal player.current_hp"
        );
    }

    #[test]
    fn sync_creature_max_hp_from_player_copies_into_mirror() {
        let mut state = CombatState::default();
        let pid = NonZeroU32::new(1).expect("non-zero");
        state.creatures.push(mg_creature(pid, 50, 60));
        state.player.creature_uid = Some(CreatureUid(pid));
        state.player.max_hp = 99;

        state.sync_creature_max_hp_from_player();

        let row = state
            .creatures
            .iter()
            .find(|c| c.id == pid)
            .expect("mirror");
        assert_eq!(row.max_hp, 99, "mirror max_hp must equal player.max_hp");
    }

    // ---- cost-bank / Rupture ----

    #[test]
    fn rupture_deferred_strength_accumulate_and_drain() {
        let mut state = CombatState::default();
        let rupture_uid = PowerUid::new(1).expect("nz");
        state
            .powers
            .push(mg_power(rupture_uid, PowerKind::Rupture, 1));
        state.player.powers.push(rupture_uid);
        let str_uid = PowerUid::new(2).expect("nz");
        state.powers.push(mg_power(str_uid, PowerKind::Strength, 5));
        state.player.powers.push(str_uid);

        state.accumulate_rupture_deferred_strength(0);
        assert_eq!(
            state.powers.get_mut_by_id(rupture_uid.0).unwrap().counter,
            0,
            "delta==0 must not touch the Rupture counter"
        );

        state.accumulate_rupture_deferred_strength(3);
        state.accumulate_rupture_deferred_strength(2);
        assert_eq!(
            state.powers.get_mut_by_id(rupture_uid.0).unwrap().counter,
            5,
            "accumulate must sum delta into the Rupture counter"
        );
        assert_eq!(
            state.powers.get_mut_by_id(str_uid.0).unwrap().counter,
            0,
            "the non-Rupture power's counter must be untouched"
        );

        let owed = state.drain_rupture_deferred_strength();
        assert_eq!(
            owed, 5,
            "drain must return the accumulated deferred Strength"
        );
        assert_eq!(
            state.powers.get_mut_by_id(rupture_uid.0).unwrap().counter,
            0,
            "drain must reset the Rupture counter to 0"
        );
        assert_eq!(
            state.drain_rupture_deferred_strength(),
            0,
            "second drain owes 0"
        );
    }

    #[test]
    fn rupture_deferred_strength_no_instance_is_noop() {
        let mut state = CombatState::default();
        let str_uid = PowerUid::new(1).expect("nz");
        state.powers.push(mg_power(str_uid, PowerKind::Strength, 5));
        state.player.powers.push(str_uid);

        state.accumulate_rupture_deferred_strength(4);
        assert_eq!(
            state.powers.get_mut_by_id(str_uid.0).unwrap().counter,
            0,
            "with no Rupture instance, accumulate must touch nothing"
        );
        assert_eq!(
            state.drain_rupture_deferred_strength(),
            0,
            "no Rupture → drain 0"
        );
    }

    #[test]
    fn discount_pinpoint_on_skill_played_discounts_and_floors() {
        let mut state = CombatState::default();
        let mut pinpoint = Card {
            card_id: crate::cards::CardId::Pinpoint,
            cost: 1,
            ..Card::pod_defaults()
        };
        pinpoint.uid = CardUid::new(1).expect("nz");
        let mut other = Card {
            card_id: crate::cards::CardId::StrikeIronclad,
            cost: 1,
            ..Card::pod_defaults()
        };
        other.uid = CardUid::new(2).expect("nz");
        state.cards.push(pinpoint);
        state.cards.push(other);

        state.discount_pinpoint_on_skill_played();
        let p = state
            .cards
            .iter()
            .find(|c| c.card_id == crate::cards::CardId::Pinpoint)
            .unwrap();
        assert_eq!(p.cost, 0, "Pinpoint cost 1 must discount to 0");
        assert_eq!(
            p.combat_var(crate::cards::VarKey::PinpointCostReductionThisTurn),
            1,
            "the per-card discount must bank +1"
        );
        let o = state
            .cards
            .iter()
            .find(|c| c.card_id == crate::cards::CardId::StrikeIronclad)
            .unwrap();
        assert_eq!(o.cost, 1, "a non-Pinpoint card must be untouched");

        state.discount_pinpoint_on_skill_played();
        let p = state
            .cards
            .iter()
            .find(|c| c.card_id == crate::cards::CardId::Pinpoint)
            .unwrap();
        assert_eq!(p.cost, 0, "the cost>0 floor must prevent going negative");
        assert_eq!(
            p.combat_var(crate::cards::VarKey::PinpointCostReductionThisTurn),
            1,
            "a floored play must not bank a further discount"
        );
    }

    #[test]
    fn restore_pinpoint_costs_for_turn_start_reverts_and_clears() {
        let mut state = CombatState::default();
        let mut pinpoint = Card {
            card_id: crate::cards::CardId::Pinpoint,
            cost: 0,
            ..Card::pod_defaults()
        };
        pinpoint.uid = CardUid::new(1).expect("nz");
        pinpoint.set_combat_var(crate::cards::VarKey::PinpointCostReductionThisTurn, 2);
        state.cards.push(pinpoint);

        state.restore_pinpoint_costs_for_turn_start();
        let p = &state.cards[0];
        assert_eq!(
            p.cost, 2,
            "restore must add the banked discount back to cost"
        );
        assert_eq!(
            p.combat_var(crate::cards::VarKey::PinpointCostReductionThisTurn),
            0,
            "restore must clear the banked Pinpoint var"
        );
    }

    #[test]
    fn restore_snecko_costs_for_turn_start_reverts_and_clears() {
        let mut state = CombatState::default();
        let mut card = Card {
            card_id: crate::cards::CardId::StrikeIronclad,
            cost: 3,
            ..Card::pod_defaults()
        };
        card.uid = CardUid::new(1).expect("nz");
        card.set_combat_var(crate::cards::VarKey::SneckoCostDeltaThisTurn, 2);
        card.set_combat_var(crate::cards::VarKey::CostAbsolutelySet, 1);
        state.cards.push(card);

        state.restore_snecko_costs_for_turn_start();
        let c = &state.cards[0];
        assert_eq!(
            c.cost, 1,
            "restore must subtract the banked Snecko delta off cost"
        );
        assert_eq!(
            c.combat_var(crate::cards::VarKey::SneckoCostDeltaThisTurn),
            0,
            "restore must clear the banked Snecko var"
        );
        assert_eq!(
            c.combat_var(crate::cards::VarKey::CostAbsolutelySet),
            0,
            "the expiring Snecko roll must release the CostAbsolutelySet flag"
        );
    }

    // ---- post-choice effects ----

    fn pce_creature_power_amount(
        state: &CombatState,
        cuid: CreatureUid,
        kind: PowerKind,
    ) -> Option<i32> {
        let creature = state.creatures.iter().find(|c| c.id == cuid.0)?;
        creature.powers.iter().find_map(|puid| {
            state
                .powers
                .get_by_id(puid.0)
                .filter(|p| p.kind == kind)
                .map(|p| p.amount)
        })
    }

    fn pce_state_with_mirror() -> (CombatState, CreatureUid) {
        let mut state = CombatState::default();
        let player_uid = NonZeroU32::new(1).expect("non-zero");
        state.player.creature_uid = Some(CreatureUid(player_uid));
        state.creatures.push(mg_creature(player_uid, 50, 50));
        (state, CreatureUid(player_uid))
    }

    #[test]
    fn apply_post_choice_effect_gain_strength_positive_and_nonpositive() {
        let (mut state, puid) = pce_state_with_mirror();
        apply_post_choice_effect(&mut state, Some(PostChoiceEffect::GainStrength(3)));
        assert_eq!(
            pce_creature_power_amount(&state, puid, PowerKind::Strength),
            Some(3),
            "GainStrength(3) must apply 3 Strength to the player mirror",
        );

        let (mut state0, puid0) = pce_state_with_mirror();
        apply_post_choice_effect(&mut state0, Some(PostChoiceEffect::GainStrength(0)));
        assert_eq!(
            pce_creature_power_amount(&state0, puid0, PowerKind::Strength),
            None,
            "GainStrength(0) must be a no-op",
        );

        let (mut state_neg, puid_neg) = pce_state_with_mirror();
        apply_post_choice_effect(&mut state_neg, Some(PostChoiceEffect::GainStrength(-2)));
        assert_eq!(
            pce_creature_power_amount(&state_neg, puid_neg, PowerKind::Strength),
            None,
            "GainStrength(-2) must be a no-op",
        );
    }

    #[test]
    fn apply_post_choice_effect_apply_player_power_positive_and_nonpositive() {
        let (mut state, puid) = pce_state_with_mirror();
        apply_post_choice_effect(
            &mut state,
            Some(PostChoiceEffect::ApplyPlayerPower(PowerKind::Dexterity, 2)),
        );
        assert_eq!(
            pce_creature_power_amount(&state, puid, PowerKind::Dexterity),
            Some(2),
            "ApplyPlayerPower(Dexterity, 2) must apply 2 Dexterity",
        );

        let (mut state0, puid0) = pce_state_with_mirror();
        apply_post_choice_effect(
            &mut state0,
            Some(PostChoiceEffect::ApplyPlayerPower(PowerKind::Dexterity, 0)),
        );
        assert_eq!(
            pce_creature_power_amount(&state0, puid0, PowerKind::Dexterity),
            None,
            "ApplyPlayerPower(Dexterity, 0) must be a no-op",
        );

        let (mut state_neg, puid_neg) = pce_state_with_mirror();
        apply_post_choice_effect(
            &mut state_neg,
            Some(PostChoiceEffect::ApplyPlayerPower(PowerKind::Dexterity, -1)),
        );
        assert_eq!(
            pce_creature_power_amount(&state_neg, puid_neg, PowerKind::Dexterity),
            None,
            "ApplyPlayerPower(Dexterity, -1) must be a no-op",
        );
    }

    #[test]
    fn apply_post_choice_effect_draw_cards_positive_and_nonpositive() {
        let mut state = CombatState::default();
        for n in 1u32..=5 {
            state.player.draw.push(CardUid::new(n).expect("non-zero"));
        }
        apply_post_choice_effect(&mut state, Some(PostChoiceEffect::DrawCards(2)));
        assert_eq!(state.player.hand.len(), 2, "DrawCards(2) must draw 2 cards");
        assert_eq!(state.player.draw.len(), 3, "draw pile must shrink by 2");

        let mut state0 = CombatState::default();
        for n in 1u32..=5 {
            state0.player.draw.push(CardUid::new(n).expect("non-zero"));
        }
        apply_post_choice_effect(&mut state0, Some(PostChoiceEffect::DrawCards(0)));
        assert_eq!(state0.player.hand.len(), 0, "DrawCards(0) must be a no-op");
        assert_eq!(state0.player.draw.len(), 5, "draw pile unchanged on no-op");

        let mut state_neg = CombatState::default();
        for n in 1u32..=5 {
            state_neg
                .player
                .draw
                .push(CardUid::new(n).expect("non-zero"));
        }
        apply_post_choice_effect(&mut state_neg, Some(PostChoiceEffect::DrawCards(-3)));
        assert_eq!(
            state_neg.player.hand.len(),
            0,
            "DrawCards(-3) must be a no-op"
        );
        assert_eq!(
            state_neg.player.draw.len(),
            5,
            "draw pile unchanged on no-op"
        );
    }

    // ---- card-effect resolvers (replace-fn-with-() smoke tests) ----

    fn mg_choice(
        kind: ChoiceKind,
        card_id: crate::cards::CardId,
        uid: CardUid,
        pile: crate::cards::PileKind,
    ) -> PendingChoice {
        PendingChoice {
            kind,
            options: vec![ChoiceOption {
                card_id,
                upgraded: false,
                card_uid: Some(uid),
                source_pile: Some(pile),
            }],
            min_select: 1,
            max_select: 1,
            can_skip: false,
            post_effect: None,
            picked: SmallVec::new(),
        }
    }

    fn mg_card_uid1(card_id: crate::cards::CardId) -> crate::cards::Card {
        crate::cards::Card {
            uid: CardUid::new(1).expect("non-zero"),
            card_id,
            cost: 1,
            card_type: crate::cards::CardType::Skill,
            ..crate::cards::Card::pod_defaults()
        }
    }

    #[test]
    fn resolve_upgrade_in_hand_upgrades_chosen_hand_card() {
        let uid = CardUid::new(1).expect("non-zero");
        let mut s = CombatState::default();
        s.cards
            .push(mg_card_uid1(crate::cards::CardId::StrikeIronclad));
        s.player.hand.push(uid);
        let pc = mg_choice(
            ChoiceKind::UpgradeInHand,
            crate::cards::CardId::StrikeIronclad,
            uid,
            crate::cards::PileKind::Hand,
        );
        assert!(!s.cards[0].upgraded, "precondition: not upgraded");
        resolve_upgrade_in_hand(&mut s, &pc, &[0]);
        assert!(s.cards[0].upgraded, "chosen hand card must be upgraded");
    }

    #[test]
    fn resolve_add_from_discard_free_moves_to_hand_and_stamps_free() {
        let uid = CardUid::new(1).expect("non-zero");
        let mut s = CombatState::default();
        s.cards
            .push(mg_card_uid1(crate::cards::CardId::StrikeIronclad));
        s.player.discard.push(uid);
        let pc = mg_choice(
            ChoiceKind::AddFromDiscardFree,
            crate::cards::CardId::StrikeIronclad,
            uid,
            crate::cards::PileKind::Discard,
        );
        resolve_add_from_discard_free(&mut s, &pc, &[0]);
        assert!(s.player.hand.contains(&uid), "card moved discard→hand");
        assert!(!s.player.discard.contains(&uid), "removed from discard");
        assert_eq!(
            s.cards[0].combat_var(crate::cards::VarKey::FreeThisTurn),
            1,
            "card stamped FreeThisTurn",
        );
    }

    #[test]
    fn resolve_add_from_discard_to_hand_moves_chosen_card() {
        let uid = CardUid::new(1).expect("non-zero");
        let mut s = CombatState::default();
        s.cards
            .push(mg_card_uid1(crate::cards::CardId::StrikeIronclad));
        s.player.discard.push(uid);
        let pc = mg_choice(
            ChoiceKind::AddFromDiscardToHand,
            crate::cards::CardId::StrikeIronclad,
            uid,
            crate::cards::PileKind::Discard,
        );
        resolve_add_from_discard_to_hand(&mut s, &pc, &[0]);
        assert!(s.player.hand.contains(&uid), "card moved discard→hand");
        assert!(!s.player.discard.contains(&uid), "removed from discard");
    }

    /// Build a `Card` POD with an explicit uid + card_id so a discard pile of
    /// distinct cards can be assembled for the sequential multi-pick tests.
    /// The uid is `raw`, the slot index in `state.cards` is `raw - 1` (the uid
    /// → index convention several resolvers assume). Bead `sts2-rs-sopgf`.
    #[cfg(test)]
    fn mk_card(raw: u32, card_id: crate::cards::CardId) -> crate::cards::Card {
        let mut c = mg_card_uid1(card_id);
        c.uid = CardUid::new(raw).expect("non-zero");
        c.card_id = card_id;
        c
    }

    /// Build a fresh `AddFromDiscardToHand` multi-pick over a discard of
    /// distinct cards. Returns the state (with cards pushed in `cards`/`discard`
    /// in index order) and the choice option uids. Bead `sts2-rs-sopgf`.
    #[cfg(test)]
    fn discard_multipick_state(
        card_ids: &[crate::cards::CardId],
        min: u8,
        max: u8,
    ) -> (CombatState, Vec<CardUid>) {
        let mut s = CombatState::default();
        let mut uids = Vec::new();
        for (i, &cid) in card_ids.iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            let raw = (i + 1) as u32;
            let uid = CardUid::new(raw).expect("non-zero");
            s.cards.push(mk_card(raw, cid));
            s.player.discard.push(uid);
            uids.push(uid);
        }
        let options: Vec<ChoiceOption> = card_ids
            .iter()
            .zip(&uids)
            .map(|(&cid, &uid)| ChoiceOption {
                card_id: cid,
                upgraded: false,
                card_uid: Some(uid),
                source_pile: Some(crate::cards::PileKind::Discard),
            })
            .collect();
        s.pending_choice = Some(PendingChoice {
            kind: ChoiceKind::AddFromDiscardToHand,
            options,
            min_select: min,
            max_select: max,
            can_skip: false,
            post_effect: None,
            picked: SmallVec::new(),
        });
        (s, uids)
    }

    /// Bead `sts2-rs-sopgf`: an open `AddFromDiscardToHand` choice with
    /// `min == max == 2` over 3 distinct discard cards decomposes into single-
    /// index picks `[0], [1], [2]`; after ONE pick the picked index is excluded
    /// from the next `legal_actions`; finalizing after 2 picks resolves with
    /// BOTH chosen indices. The two incremental picks + FINALIZE produce the
    /// same end state as the ATOMIC resolve with the full `[i, j]` selection.
    #[test]
    fn legal_actions_multipick_sequential_decomposition_add_from_discard() {
        use crate::cards::CardId;
        let card_ids = [CardId::StrikeIronclad, CardId::Bash, CardId::DefendIronclad];

        // Fresh choice → one incremental pick per option, no early FINALIZE.
        let (mut s, uids) = discard_multipick_state(&card_ids, 2, 2);
        let sels: Vec<Vec<u8>> = legal_actions(&s)
            .iter()
            .map(|a| match a {
                Action::ResolveChoice { selection } => selection.to_vec(),
                _ => panic!("only ResolveChoice"),
            })
            .collect();
        assert_eq!(sels, vec![vec![0u8], vec![1], vec![2]]);

        // INCREMENTAL pick of option 1 (Bash): no effect fires, choice re-opens
        // with `picked = [1]`, and `legal_actions` now excludes index 1.
        let r = apply(
            &mut s,
            Action::ResolveChoice {
                selection: smallvec::smallvec![1u8],
            },
        );
        assert_eq!(r, StepResult::Advanced);
        let pc = s.pending_choice.as_ref().expect("choice re-opened");
        assert_eq!(pc.picked.as_slice(), &[1u8], "first pick accumulated");
        assert!(
            s.player.hand.is_empty() && s.player.discard.len() == 3,
            "incremental pick fires NO effect",
        );
        let sels: Vec<Vec<u8>> = legal_actions(&s)
            .iter()
            .map(|a| match a {
                Action::ResolveChoice { selection } => selection.to_vec(),
                _ => panic!("only ResolveChoice"),
            })
            .collect();
        assert_eq!(sels, vec![vec![0u8], vec![2]], "picked index 1 excluded");

        // Second pick of option 2 (Defend) reaches `max_select` → FINALIZE
        // fires the resolver over the full accumulated set `[1, 2]`.
        let r = apply(
            &mut s,
            Action::ResolveChoice {
                selection: smallvec::smallvec![2u8],
            },
        );
        assert_eq!(r, StepResult::Advanced);
        assert!(s.pending_choice.is_none(), "choice cleared at finalize");
        // Bash (uid 2) and Defend (uid 3) moved to hand; Strike (uid 1) stays.
        assert!(s.player.hand.contains(&uids[1]), "Bash added");
        assert!(s.player.hand.contains(&uids[2]), "Defend added");
        assert!(s.player.discard.contains(&uids[0]), "Strike not added");

        // ATOMIC equivalence: driving the FULL `[1, 2]` set in one shot via
        // `resolve_choice_atomic_apply` yields the identical end state.
        let (mut s_atomic, uids_a) = discard_multipick_state(&card_ids, 2, 2);
        let r = resolve_choice_atomic_apply(&mut s_atomic, &[1u8, 2u8]);
        assert_eq!(r, StepResult::Advanced);
        assert!(s_atomic.pending_choice.is_none());
        assert!(s_atomic.player.hand.contains(&uids_a[1]));
        assert!(s_atomic.player.hand.contains(&uids_a[2]));
        assert!(s_atomic.player.discard.contains(&uids_a[0]));
        // Same hand membership (sorted) between sequential and atomic paths.
        let mut seq_hand: Vec<u32> = s.player.hand.iter().map(|u| u.0.get()).collect();
        let mut atomic_hand: Vec<u32> = s_atomic.player.hand.iter().map(|u| u.0.get()).collect();
        seq_hand.sort_unstable();
        atomic_hand.sort_unstable();
        assert_eq!(seq_hand, atomic_hand, "sequential == atomic end state");
    }

    /// Bead `sts2-rs-sopgf`: with a single-pick `AddFromDiscardToHand`
    /// (`min == max == 1`) over a high-value and a low-value discard card, the
    /// engine CAN reach the state where the HIGH-value card is the one added —
    /// i.e. "which card" is now expressible as a distinct `ResolveChoice` the
    /// search can select (it is no longer collapsed to "take the first K").
    #[test]
    fn single_pick_add_from_discard_can_choose_which_card() {
        use crate::cards::CardId;
        // Index 0 = low value (Defend), index 1 = high value (Bash).
        let card_ids = [CardId::DefendIronclad, CardId::Bash];
        let (s, uids) = discard_multipick_state(&card_ids, 1, 1);

        // The choice fans out so BOTH options are reachable as distinct picks.
        let sels: Vec<Vec<u8>> = legal_actions(&s)
            .iter()
            .map(|a| match a {
                Action::ResolveChoice { selection } => selection.to_vec(),
                _ => panic!("only ResolveChoice"),
            })
            .collect();
        assert_eq!(
            sels,
            vec![vec![0u8], vec![1]],
            "both discard cards are individually selectable",
        );

        // Select the HIGH-value option (index 1 / Bash). Single-pick finalizes
        // immediately — the high-value card lands in hand, the low-value one
        // stays in discard.
        let mut s = s;
        let r = apply(
            &mut s,
            Action::ResolveChoice {
                selection: smallvec::smallvec![1u8],
            },
        );
        assert_eq!(r, StepResult::Advanced);
        assert!(s.pending_choice.is_none());
        assert!(
            s.player.hand.contains(&uids[1]),
            "high-value Bash reachable as the added card",
        );
        assert!(
            s.player.discard.contains(&uids[0]),
            "low-value Defend left in discard",
        );
    }

    #[test]
    fn resolve_apply_retain_in_hand_grants_retain_keyword() {
        let uid = CardUid::new(1).expect("non-zero");
        let mut s = CombatState::default();
        s.cards
            .push(mg_card_uid1(crate::cards::CardId::StrikeIronclad));
        s.player.hand.push(uid);
        let pc = mg_choice(
            ChoiceKind::ApplyRetainInHand,
            crate::cards::CardId::StrikeIronclad,
            uid,
            crate::cards::PileKind::Hand,
        );
        resolve_apply_retain_in_hand(&mut s, &pc, &[0]);
        assert!(
            s.cards[0]
                .keyword_override
                .expect("override set")
                .contains(crate::cards::KeywordOverride::RETAIN),
            "chosen card must gain RETAIN",
        );
    }

    #[test]
    fn resolve_retain_at_turn_end_records_uid() {
        let uid = CardUid::new(1).expect("non-zero");
        let mut s = CombatState::default();
        s.cards
            .push(mg_card_uid1(crate::cards::CardId::StrikeIronclad));
        s.player.hand.push(uid);
        let pc = mg_choice(
            ChoiceKind::RetainAtTurnEnd,
            crate::cards::CardId::StrikeIronclad,
            uid,
            crate::cards::PileKind::Hand,
        );
        resolve_retain_at_turn_end(&mut s, &pc, &[0]);
        assert!(
            s.player.retain_this_turn.contains(&uid),
            "uid pushed to retain_this_turn"
        );
    }

    #[test]
    fn resolve_apply_sly_in_hand_records_uid() {
        let uid = CardUid::new(1).expect("non-zero");
        let mut s = CombatState::default();
        s.cards
            .push(mg_card_uid1(crate::cards::CardId::StrikeIronclad));
        s.player.hand.push(uid);
        let pc = mg_choice(
            ChoiceKind::ApplySlyInHand,
            crate::cards::CardId::StrikeIronclad,
            uid,
            crate::cards::PileKind::Hand,
        );
        resolve_apply_sly_in_hand(&mut s, &pc, &[0]);
        assert!(
            s.player.sly_this_turn.contains(&uid),
            "uid pushed to sly_this_turn"
        );
    }

    #[test]
    fn resolve_apply_ethereal_in_hand_grants_ethereal_keyword() {
        let uid = CardUid::new(1).expect("non-zero");
        let mut s = CombatState::default();
        s.cards
            .push(mg_card_uid1(crate::cards::CardId::StrikeIronclad));
        s.player.hand.push(uid);
        let pc = mg_choice(
            ChoiceKind::ApplyEtherealInHand,
            crate::cards::CardId::StrikeIronclad,
            uid,
            crate::cards::PileKind::Hand,
        );
        resolve_apply_ethereal_in_hand(&mut s, &pc, &[0]);
        assert!(
            s.cards[0]
                .keyword_override
                .expect("override set")
                .contains(crate::cards::KeywordOverride::ETHEREAL),
            "chosen card must gain ETHEREAL",
        );
    }

    #[test]
    fn resolve_make_free_in_hand_zeros_cost_and_stars() {
        let uid = CardUid::new(1).expect("non-zero");
        let mut s = CombatState::default();
        let mut c = mg_card_uid1(crate::cards::CardId::StrikeIronclad);
        c.cost = 2;
        s.cards.push(c);
        s.player.hand.push(uid);
        let pc = mg_choice(
            ChoiceKind::MakeFreeInHand,
            crate::cards::CardId::StrikeIronclad,
            uid,
            crate::cards::PileKind::Hand,
        );
        resolve_make_free_in_hand(&mut s, &pc, &[0]);
        assert_eq!(s.cards[0].cost, 0, "energy cost zeroed");
        assert_eq!(
            s.cards[0].combat_var(crate::cards::VarKey::StarCostFreeThisCombat),
            1,
            "StarCostFreeThisCombat var stamped",
        );
    }

    #[test]
    fn resolve_clone_chosen_card_next_turn_stashes_uid_in_nightmare_counter() {
        let uid = CardUid::new(5).expect("non-zero");
        let mut s = CombatState::default();
        let pow_uid = PowerUid::new(1).expect("non-zero");
        s.powers.push(mg_power(pow_uid, PowerKind::Nightmare, 1));
        let pc = mg_choice(
            ChoiceKind::CloneChosenCardNextTurn,
            crate::cards::CardId::StrikeIronclad,
            uid,
            crate::cards::PileKind::Hand,
        );
        resolve_clone_chosen_card_next_turn(&mut s, &pc, &[0]);
        let stashed = s
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::Nightmare)
            .expect("nightmare")
            .counter;
        assert_eq!(stashed, 5, "chosen uid stashed into Nightmare counter");
    }

    /// GH #364 A1 — the production shape: `play_nightmare` opens the choice
    /// with NO power applied and rides the `Apply` in on `post_effect`, so the
    /// resolver must apply-then-stash. C# `Nightmare.cs:44` runs
    /// `(await PowerCmd.Apply<NightmarePower>(...)).SetSelectedCard(card)`
    /// AFTER the `await CardSelectCmd.FromHand` on `:32`. (The sibling test
    /// above pre-seeds the power, which is now only a defensive shape.)
    #[test]
    fn resolve_clone_chosen_card_next_turn_applies_the_deferred_power_then_stashes() {
        let uid = CardUid::new(5).expect("non-zero");
        let mut s = CombatState::default();
        let mut pc = mg_choice(
            ChoiceKind::CloneChosenCardNextTurn,
            crate::cards::CardId::StrikeIronclad,
            uid,
            crate::cards::PileKind::Hand,
        );
        pc.post_effect = Some(PostChoiceEffect::ApplyPlayerPower(PowerKind::Nightmare, 3));
        assert!(
            s.powers.iter().all(|p| p.kind != PowerKind::Nightmare),
            "no power exists while the select is open"
        );
        resolve_clone_chosen_card_next_turn(&mut s, &pc, &[0]);
        let power = s
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::Nightmare)
            .expect("post-choice effect applied the power");
        assert_eq!(power.amount, 3, "3 stacks from the deferred Apply");
        assert_eq!(power.counter, 5, "stashed into the instance just applied");
    }

    /// GH #364 A1 — C# gates the `Apply` on `selectedCard != null`
    /// (`Nightmare.cs:42`). An empty/invalid selection must therefore leave the
    /// player with no Nightmare power at all, not an un-stashed one.
    #[test]
    fn resolve_clone_chosen_card_next_turn_applies_nothing_without_a_selection() {
        let uid = CardUid::new(5).expect("non-zero");
        let mut s = CombatState::default();
        let mut pc = mg_choice(
            ChoiceKind::CloneChosenCardNextTurn,
            crate::cards::CardId::StrikeIronclad,
            uid,
            crate::cards::PileKind::Hand,
        );
        pc.post_effect = Some(PostChoiceEffect::ApplyPlayerPower(PowerKind::Nightmare, 3));
        resolve_clone_chosen_card_next_turn(&mut s, &pc, &[]);
        assert!(
            s.powers.iter().all(|p| p.kind != PowerKind::Nightmare),
            "no selection ⇒ FirstOrDefault() is null ⇒ no Apply"
        );
    }

    // ---- misc accessors / choice auto-resolve / swipe bookkeeping ----

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn act_index_accessor_returns_stored_field() {
        let mut s = CombatState::default();
        s.act_index = 3;
        assert_eq!(
            s.act_index(),
            3,
            "accessor must return the stored act_index field"
        );
    }

    #[test]
    fn power_store_from_iter_collects_all() {
        let a = mg_power(PowerUid::new(1).expect("non-zero"), PowerKind::Strength, 2);
        let b = mg_power(PowerUid::new(2).expect("non-zero"), PowerKind::Weak, 1);
        let store: PowerStore = [a, b].into_iter().collect();
        assert_eq!(store.len(), 2, "collected store must hold both powers");
        let got = store
            .get_by_id(NonZeroU32::new(2).expect("non-zero"))
            .expect("power id 2 must resolve");
        assert_eq!(got.kind, PowerKind::Weak);
        assert_eq!(got.amount, 1);
    }

    #[test]
    fn creature_gain_max_hp_syncs_player_mirror() {
        let player_uid = NonZeroU32::new(1).expect("non-zero");
        let mut s = CombatState::default();
        s.player.creature_uid = Some(CreatureUid(player_uid));
        s.player.max_hp = 50;
        s.player.current_hp = 40;
        s.creatures.push(mg_creature(player_uid, 40, 50));

        let delta = s.creature_gain_max_hp(CreatureUid(player_uid), 10);
        assert_eq!(delta, 10, "gain delta");
        assert_eq!(
            s.player.max_hp, 60,
            "player.max_hp mirror must re-sync on gain"
        );
        assert_eq!(
            s.player.current_hp, 50,
            "player.current_hp mirror must re-sync"
        );
    }

    #[test]
    fn creature_lose_max_hp_syncs_player_mirror_and_clamps() {
        let player_uid = NonZeroU32::new(1).expect("non-zero");
        let mut s = CombatState::default();
        s.player.creature_uid = Some(CreatureUid(player_uid));
        s.player.max_hp = 50;
        s.player.current_hp = 50;
        s.creatures.push(mg_creature(player_uid, 50, 50));

        let delta = s.creature_lose_max_hp(CreatureUid(player_uid), 10);
        assert_eq!(delta, -10, "lose delta is negative");
        assert_eq!(
            s.player.max_hp, 40,
            "player.max_hp mirror must re-sync on lose"
        );
        assert_eq!(
            s.player.current_hp, 40,
            "current_hp (was == max) must clamp and mirror"
        );
    }

    #[test]
    fn creature_lose_max_hp_below_max_leaves_current() {
        let mut c = mg_creature(NonZeroU32::new(1).expect("non-zero"), 20, 50);
        let delta = c.lose_max_hp(10);
        assert_eq!(delta, -10);
        assert_eq!(c.max_hp, 40);
        assert_eq!(c.current_hp, 20, "current below new max must be untouched");
    }

    #[test]
    fn open_hand_choice_autoresolves_forced_single_option() {
        let mut s = CombatState::default();
        let pc = PendingChoice {
            kind: ChoiceKind::ExhaustFromHand,
            options: vec![ChoiceOption {
                card_id: crate::cards::CardId::Wound,
                upgraded: false,
                card_uid: None,
                source_pile: None,
            }],
            min_select: 1,
            max_select: 1,
            can_skip: false,
            post_effect: None,
            picked: SmallVec::new(),
        };
        s.open_hand_choice(pc);
        assert!(
            s.pending_choice.is_none(),
            "forced single-option FromHand choice must auto-resolve"
        );
    }

    #[test]
    fn open_hand_choice_skippable_single_option_surfaces() {
        let mut s = CombatState::default();
        let pc = PendingChoice {
            kind: ChoiceKind::ExhaustFromHand,
            options: vec![ChoiceOption {
                card_id: crate::cards::CardId::Wound,
                upgraded: false,
                card_uid: None,
                source_pile: None,
            }],
            min_select: 1,
            max_select: 1,
            can_skip: true,
            post_effect: None,
            picked: SmallVec::new(),
        };
        s.open_hand_choice(pc);
        assert!(
            s.pending_choice.is_some(),
            "a skippable choice must stay surfaced"
        );
    }

    #[test]
    fn open_hand_choice_non_fromhand_kind_surfaces() {
        let mut s = CombatState::default();
        let pc = PendingChoice {
            kind: ChoiceKind::CardSelect,
            options: vec![ChoiceOption {
                card_id: crate::cards::CardId::Wound,
                upgraded: false,
                card_uid: None,
                source_pile: None,
            }],
            min_select: 1,
            max_select: 1,
            can_skip: false,
            post_effect: None,
            picked: SmallVec::new(),
        };
        s.open_hand_choice(pc);
        assert!(
            s.pending_choice.is_some(),
            "CardSelect must not auto-resolve"
        );
    }

    #[test]
    fn swipe_steal_record_and_reclaim() {
        let thief = CreatureUid(NonZeroU32::new(2).expect("non-zero"));
        let other = CreatureUid(NonZeroU32::new(3).expect("non-zero"));
        let mut s = CombatState::default();

        s.record_swipe_steal(thief, crate::cards::CardId::Wound, false);
        assert_eq!(
            s.swipe_stolen.len(),
            1,
            "record_swipe_steal must push an entry"
        );
        assert_eq!(s.swipe_stolen[0].thief, thief);
        assert_eq!(s.swipe_stolen[0].card_id, crate::cards::CardId::Wound);
        assert!(!s.swipe_stolen[0].reclaimed, "fresh steal is not reclaimed");

        s.reclaim_swipe_on_death(other);
        assert!(
            !s.swipe_stolen[0].reclaimed,
            "non-thief death must not reclaim"
        );

        s.reclaim_swipe_on_death(thief);
        assert!(
            s.swipe_stolen[0].reclaimed,
            "thief death must mark its steal reclaimed"
        );
    }

    /// GH #604: `HeistPower.BeforeDeath` banks the carried gold when the
    /// gremlin is KILLED. Escapes never reach the damage-death chokepoint
    /// this method is wired to, so a fled carrier keeps the gold — which is
    /// what makes the kill worth searching for.
    #[test]
    fn heist_gold_is_recovered_only_on_the_carrier_s_death() {
        let mut s = CombatState::default();
        let carrier = s.spawn_enemy(MonsterKind::FatGremlin);
        let bystander = s.spawn_enemy(MonsterKind::SneakyGremlin);
        crate::card_effects::apply_power_to_creature(&mut s, carrier, PowerKind::Heist, 37);

        assert_eq!(
            s.heist_recovered_gold, 0,
            "nothing recovered before a death"
        );

        s.recover_heist_on_death(bystander);
        assert_eq!(
            s.heist_recovered_gold, 0,
            "a non-carrier death recovers nothing"
        );

        s.recover_heist_on_death(carrier);
        assert_eq!(
            s.heist_recovered_gold, 37,
            "killing the carrier hands back exactly the HeistPower amount"
        );
    }

    /// GH #604: the loot term prices the three things a fleeing thief takes
    /// with it, and every one of them zeroes out when the thief is killed
    /// instead.
    // Exact equality is the assertion: a reclaimed steal must zero the charge
    // outright, not merely shrink it.
    #[allow(clippy::float_cmp)]
    #[test]
    fn loot_at_risk_prices_cards_gold_and_the_escape_gold_shrink() {
        let thief = CreatureUid(NonZeroU32::new(2).expect("non-zero"));
        let mut s = CombatState::default();
        assert_eq!(
            loot_at_risk_hp_equiv(&s),
            0.0,
            "a combat with nothing stolen is loot-neutral"
        );

        s.record_swipe_steal(thief, crate::cards::CardId::Bash, false);
        assert!(
            (loot_at_risk_hp_equiv(&s) - LOOT_CARD_HP_EQUIV).abs() < 1e-4,
            "one un-reclaimed steal costs exactly one card"
        );
        s.reclaim_swipe_on_death(thief);
        assert_eq!(
            loot_at_risk_hp_equiv(&s),
            0.0,
            "killing the thief returns the card and clears the charge"
        );

        // Stolen gold: the ThieveryPower counter is the running total taken.
        let merc = s.spawn_enemy(MonsterKind::GremlinMerc);
        crate::card_effects::apply_power_to_creature(&mut s, merc, PowerKind::Thievery, 20);
        let puid = s
            .creatures
            .iter()
            .find(|c| c.id == merc.0)
            .expect("merc spawned")
            .powers
            .clone();
        for u in puid {
            if let Some(p) = s.powers.get_mut_by_id(u.0) {
                if p.kind == PowerKind::Thievery {
                    p.counter = 40;
                }
            }
        }
        assert_eq!(stolen_gold_outstanding(&s), 40);
        assert!(
            (loot_at_risk_hp_equiv(&s) - 40.0 * LOOT_GOLD_HP_EQUIV).abs() < 1e-4,
            "outstanding stolen gold is priced at LOOT_GOLD_HP_EQUIV each"
        );
        s.heist_recovered_gold = 40;
        assert_eq!(
            stolen_gold_outstanding(&s),
            0,
            "a HeistPower refund of the same size settles the ledger"
        );
        assert_eq!(loot_at_risk_hp_equiv(&s), 0.0);

        // Escape shrinks the combat gold reward proportionally.
        s.escaped_creatures = 1;
        let one_of_one = loot_at_risk_hp_equiv(&s);
        assert!(
            (one_of_one - LOOT_ESCAPE_GOLD_BASE * LOOT_GOLD_HP_EQUIV).abs() < 1e-4,
            "the only monster fleeing forfeits the whole gold reward"
        );
        let _ = s.spawn_enemy(MonsterKind::SneakyGremlin);
        assert!(
            loot_at_risk_hp_equiv(&s) < one_of_one,
            "one of two fleeing forfeits only half the reward"
        );
    }

    #[test]
    fn legal_actions_only_end_turn_outside_play_phase() {
        let s = CombatState::default();
        let acts = legal_actions(&s);
        assert_eq!(acts.len(), 1);
        assert!(matches!(acts[0], Action::EndTurn));
    }

    #[test]
    fn legal_actions_empty_when_terminal() {
        let s = CombatState {
            is_over: true,
            ..CombatState::default()
        };
        assert!(legal_actions(&s).is_empty());
    }

    #[test]
    fn legal_actions_only_choose_when_pending() {
        let s = CombatState {
            pending_choice: Some(PendingChoice::default()),
            ..CombatState::default()
        };
        let acts = legal_actions(&s);
        assert_eq!(acts.len(), 1);
        assert!(matches!(acts[0], Action::ResolveChoice { .. }));
    }

    // ── sts2-rs-fa3w: curse play-gates (Enthralled, Normality) ─────────

    /// Bead `sts2-rs-fa3w`: Enthralled in hand prevents `legal_actions`
    /// from emitting `PlayCard` for any non-Enthralled card. The
    /// Enthralled card itself stays playable so the player can clear
    /// the gate.
    #[test]
    fn legal_actions_enthralled_blocks_other_plays() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        let mk = |uid_raw: u32, id: CardId, cost: i8| Card {
            uid: CardUid::new(uid_raw).expect("non-zero"),
            card_id: id,
            cost,
            card_type: CardType::Attack,
            target_type: TargetType::Self_,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        };
        let strike = CardUid::new(1).expect("non-zero");
        let enthralled = CardUid::new(2).expect("non-zero");
        let mut state = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..CombatState::default()
        };
        state.cards.push(mk(1, CardId::StrikeIronclad, 1));
        state.cards.push(mk(2, CardId::Enthralled, 2));
        state.player.hand.push(strike);
        state.player.hand.push(enthralled);
        state.player.energy = 3;

        let acts = legal_actions(&state);
        let play_card_ids: Vec<CardId> = acts
            .iter()
            .filter_map(|a| match a {
                Action::PlayCard { hand_index, .. } => state
                    .cards
                    .get(state.player.hand[*hand_index as usize].0.get() as usize - 1)
                    .map(|c| c.card_id),
                _ => None,
            })
            .collect();
        assert!(
            play_card_ids.contains(&CardId::Enthralled),
            "Enthralled itself must remain playable"
        );
        assert!(
            !play_card_ids.contains(&CardId::StrikeIronclad),
            "Strike must be gated out while Enthralled is in hand"
        );
    }

    /// C# `SmoggyPower.ShouldPlay` (`SmoggyPower.cs:64-71`) must be reflected in
    /// the legal-action surface, not only in `play_card` — otherwise MCTS plans
    /// a second Skill that the live game rejects with
    /// `unplayable_reason:"BlockedByHook"` (wire capture 2026-08-03, LivingFog
    /// ADVANCED_GAS). A `Smog`-afflicted card is filtered out while the player
    /// holds SmoggyPower; unafflicted cards in the same hand are untouched.
    #[test]
    fn legal_actions_smoggy_filters_smog_afflicted_cards() {
        use crate::card_afflictions::{CardAffliction, CardAfflictionKind};
        use crate::cards::{Card, CardId, CardType, TargetType};
        let mk = |uid_raw: u32, id: CardId| Card {
            uid: CardUid::new(uid_raw).expect("non-zero"),
            card_id: id,
            cost: 1,
            card_type: CardType::Skill,
            target_type: TargetType::Self_,
            base_block: 5,
            ..Card::pod_defaults()
        };
        let mut state = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..CombatState::default()
        };
        state.cards.push(mk(1, CardId::DefendIronclad));
        state.cards.push(mk(2, CardId::Dualcast));
        // Card 2 carries Smog (as it would after this turn's first Skill play).
        state.cards[1].affliction = Some(CardAffliction::new(CardAfflictionKind::Smog, 1));
        state.player.hand.push(CardUid::new(1).expect("non-zero"));
        state.player.hand.push(CardUid::new(2).expect("non-zero"));
        state.player.energy = 3;

        let playable = |s: &CombatState| -> Vec<CardId> {
            legal_actions(s)
                .iter()
                .filter_map(|a| match a {
                    Action::PlayCard { hand_index, .. } => s
                        .cards
                        .get(s.player.hand[*hand_index as usize].0.get() as usize - 1)
                        .map(|c| c.card_id),
                    _ => None,
                })
                .collect()
        };

        // Without SmoggyPower the Smog marker is inert.
        assert!(
            playable(&state).contains(&CardId::Dualcast),
            "no SmoggyPower ⇒ a Smog-marked card is still offered"
        );

        // Install Smoggy(1) on the legacy player-power surface — the surface
        // ADVANCED_GAS and the wire reconstruction both use.
        let pid = std::num::NonZeroU32::new(1).expect("non-zero");
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Smoggy,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(PowerUid(pid));

        let ids = playable(&state);
        assert!(
            !ids.contains(&CardId::Dualcast),
            "SmoggyPower + Smog ⇒ the card must not be offered (BlockedByHook)"
        );
        assert!(
            ids.contains(&CardId::DefendIronclad),
            "an unafflicted card in the same hand stays playable"
        );
    }

    /// Bead `sts2-rs-e5bsp` (GH #43): in the no-living-enemy "corpse" state
    /// (a pending-revive Adaptable corpse keeping combat open), an
    /// `Enemy`-target card must NOT be surfaced — `play_card` rejects an
    /// untargeted `Enemy` play (`target.expect(...)` in every handler), so
    /// offering it produces a no-op action that livelocks a deterministic
    /// policy. EndTurn (the real progress action) is still surfaced; a
    /// `Self_` block card is too.
    #[test]
    fn legal_actions_corpse_state_suppresses_untargeted_enemy_card() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        let mk = |uid_raw: u32, id: CardId, ctype: CardType, tt: TargetType, cost: i8| Card {
            uid: CardUid::new(uid_raw).expect("non-zero"),
            card_id: id,
            cost,
            card_type: ctype,
            target_type: tt,
            ..Card::pod_defaults()
        };
        let mut state = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..CombatState::default()
        };
        // A single DEAD enemy in `enemy_uids` (hp 0) → `alive_enemies` empty
        // while combat is not over (the jnf4a corpse state).
        let mut corpse = alive_enemy(10);
        corpse.current_hp = 0;
        let corpse_uid = CreatureUid(corpse.id);
        state.creatures.push(corpse);
        state.enemy_uids.push(corpse_uid);

        state.cards.push(mk(
            1,
            CardId::StrikeIronclad,
            CardType::Attack,
            TargetType::Enemy,
            1,
        ));
        state.cards.push(mk(
            2,
            CardId::DefendIronclad,
            CardType::Skill,
            TargetType::Self_,
            1,
        ));
        state.player.hand.push(CardUid::new(1).expect("non-zero"));
        state.player.hand.push(CardUid::new(2).expect("non-zero"));
        state.player.energy = 3;

        let acts = legal_actions(&state);
        let play_card_ids: Vec<CardId> = acts
            .iter()
            .filter_map(|a| match a {
                Action::PlayCard { hand_index, .. } => state
                    .cards
                    .get(state.player.hand[*hand_index as usize].0.get() as usize - 1)
                    .map(|c| c.card_id),
                _ => None,
            })
            .collect();
        assert!(
            !play_card_ids.contains(&CardId::StrikeIronclad),
            "untargeted Enemy card must be suppressed in the no-live-enemy corpse state"
        );
        assert!(
            play_card_ids.contains(&CardId::DefendIronclad),
            "Self_ block card stays playable (its handler tolerates an empty board)"
        );
        assert!(
            acts.iter().any(|a| matches!(a, Action::EndTurn)),
            "EndTurn must remain available — it is the only progress action here"
        );
    }

    /// q6bix F-E: `AnyAlly`-target cards (collapsed to `Self_` in the Rust
    /// specs) are UNPLAYABLE when the player has no living ally — mirroring
    /// C# `CardModel.CanPlay` (`PlayerCreatures.Count(alive) <= 1` →
    /// `NoLivingAllies`). With a living ally they become playable again.
    #[test]
    fn f_e_any_ally_card_unplayable_without_living_ally() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        let mut state = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..CombatState::default()
        };
        state.cards.push(Card {
            uid: CardUid::new(1).expect("non-zero"),
            card_id: CardId::BelieveInYou,
            cost: 0,
            card_type: CardType::Skill,
            target_type: TargetType::Self_, // spec-collapsed from AnyAlly
            ..Card::pod_defaults()
        });
        state.player.hand.push(CardUid::new(1).expect("non-zero"));
        state.player.energy = 3;

        let ids = |st: &CombatState| -> Vec<CardId> {
            legal_actions(st)
                .iter()
                .filter_map(|a| match a {
                    Action::PlayCard { hand_index, .. } => st
                        .cards
                        .get(st.player.hand[*hand_index as usize].0.get() as usize - 1)
                        .map(|c| c.card_id),
                    _ => None,
                })
                .collect()
        };

        assert!(
            !ids(&state).contains(&CardId::BelieveInYou),
            "AnyAlly card must be unplayable with no living ally (solo)"
        );

        // Summon a living ally → the card becomes playable.
        state.allies.push(PlayerCombatState {
            current_hp: 10,
            max_hp: 10,
            ..PlayerCombatState::default()
        });
        assert!(
            ids(&state).contains(&CardId::BelieveInYou),
            "AnyAlly card is playable once a living ally exists"
        );
    }

    /// v0.107.1: PactsEnd is ALWAYS playable regardless of exhaust-pile
    /// count — the v0.103.2 `IsPlayable` ≥3-exhaust requirement was removed
    /// (the 3-card threshold now only gates whether the play deals damage,
    /// `PactsEnd.cs` `CanDealDamage`).
    #[test]
    fn f_e_pacts_end_always_playable() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        let mut state = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..CombatState::default()
        };
        // Enemy so the AllEnemies-target PactsEnd has a target context.
        let enemy_id = NonZeroU32::new(1).expect("nz");
        state.creatures.push(Creature {
            id: enemy_id,
            current_hp: 40,
            max_hp: 40,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(enemy_id));
        state.cards.push(Card {
            uid: CardUid::new(1).expect("nz"),
            card_id: CardId::PactsEnd,
            cost: 0,
            card_type: CardType::Attack,
            target_type: TargetType::AllEnemies,
            ..Card::pod_defaults()
        });
        state.player.hand.push(CardUid::new(1).expect("nz"));
        state.player.energy = 3;

        let pacts_playable = |st: &CombatState| -> bool {
            legal_actions(st).iter().any(|a| match a {
                Action::PlayCard { hand_index, .. } => st
                    .cards
                    .get(st.player.hand[*hand_index as usize].0.get() as usize - 1)
                    .is_some_and(|c| c.card_id == CardId::PactsEnd),
                _ => false,
            })
        };

        assert!(
            pacts_playable(&state),
            "PactsEnd is playable with an empty exhaust pile (v0.107.1)"
        );
        // Still playable once 3 cards are exhausted (now it also deals damage).
        for i in 2..=4u32 {
            state.cards.push(Card {
                uid: CardUid::new(i).expect("nz"),
                card_id: CardId::StrikeIronclad,
                ..Card::pod_defaults()
            });
            state.player.exhaust.push(CardUid::new(i).expect("nz"));
        }
        assert!(
            pacts_playable(&state),
            "PactsEnd remains playable with 3 cards exhausted"
        );
    }

    /// Bead `sts2-rs-722vx-A`: 0-cost buff-prior cards (Rage,
    /// DrumOfBattle) emit ahead of other plays. This lets PUCT visit
    /// them first under tied-prior expansion so MCTS doesn't burn
    /// attacks before the buff lands.
    #[test]
    fn legal_actions_emits_zero_cost_buff_prior_first() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        let mk = |uid_raw: u32, id: CardId, cost: i8, ty: CardType, tgt: TargetType| Card {
            uid: CardUid::new(uid_raw).expect("non-zero"),
            card_id: id,
            cost,
            card_type: ty,
            target_type: tgt,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        };
        let mut state = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..CombatState::default()
        };
        // Hand: Strike, Strike, Rage, Strike, Defend — Rage is at index 2.
        state.cards.push(mk(
            1,
            CardId::StrikeIronclad,
            1,
            CardType::Attack,
            TargetType::Self_,
        ));
        state.cards.push(mk(
            2,
            CardId::StrikeIronclad,
            1,
            CardType::Attack,
            TargetType::Self_,
        ));
        state.cards.push(mk(
            3,
            CardId::RageCard,
            0,
            CardType::Skill,
            TargetType::Self_,
        ));
        state.cards.push(mk(
            4,
            CardId::StrikeIronclad,
            1,
            CardType::Attack,
            TargetType::Self_,
        ));
        state.cards.push(mk(
            5,
            CardId::DefendIronclad,
            1,
            CardType::Skill,
            TargetType::Self_,
        ));
        for raw in 1..=5 {
            state.player.hand.push(CardUid::new(raw).expect("non-zero"));
        }
        state.player.energy = 3;

        let acts = legal_actions(&state);
        let first_play = acts.iter().find_map(|a| match a {
            Action::PlayCard { hand_index, .. } => Some(*hand_index),
            _ => None,
        });
        assert_eq!(
            first_play,
            Some(2),
            "Rage (at hand_index 2) must be the first PlayCard emitted; got {first_play:?}"
        );
    }

    /// Bead `sts2-rs-vir3o`: cards with the `UNPLAYABLE` keyword
    /// (AscendersBane and other curses/statuses with `cost = -1`)
    /// must not surface as `Action::PlayCard`. Without this gate the
    /// `energy < cost` check passes since energy is non-negative and
    /// cost is -1, so the unplayable card was being recommended.
    #[test]
    fn legal_actions_filters_unplayable_curses() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        crate::card_effects::register_card_unplayable(CardId::AscendersBane);
        // Self-targeted Skill stand-in so the test doesn't need a live
        // enemy on the board; we only care that PlayCard is/isn't emitted.
        let mk = |uid_raw: u32, id: CardId, cost: i8| Card {
            uid: CardUid::new(uid_raw).expect("non-zero"),
            card_id: id,
            cost,
            card_type: CardType::Skill,
            target_type: TargetType::Self_,
            upgraded: false,
            base_damage: 0,
            base_block: 5,
            ..Card::pod_defaults()
        };
        let mut state = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..CombatState::default()
        };
        state.cards.push(mk(1, CardId::DefendIronclad, 1));
        state.cards.push(mk(2, CardId::AscendersBane, -1));
        state.player.hand.push(CardUid::new(1).expect("non-zero"));
        state.player.hand.push(CardUid::new(2).expect("non-zero"));
        state.player.energy = 3;

        let acts = legal_actions(&state);
        let play_card_ids: Vec<CardId> = acts
            .iter()
            .filter_map(|a| match a {
                Action::PlayCard { hand_index, .. } => state
                    .cards
                    .get(state.player.hand[*hand_index as usize].0.get() as usize - 1)
                    .map(|c| c.card_id),
                _ => None,
            })
            .collect();
        assert!(
            play_card_ids.contains(&CardId::DefendIronclad),
            "Defend must be playable"
        );
        assert!(
            !play_card_ids.contains(&CardId::AscendersBane),
            "AscendersBane (UNPLAYABLE) must not surface as a legal play"
        );
    }

    /// Bead `sts2-rs-starcost`: a card with a `CanonicalStarCost` override
    /// (the 21 Regent star cards) is unplayable when the player lacks the
    /// Stars — C# `PlayerCombatState.HasEnoughResourcesFor` →
    /// `StarCostTooHigh`. `FreeThisTurn` (Splash's `SetToFreeThisTurn` →
    /// `SetStarCostThisTurn(0)`) exempts it; enough Stars makes it playable.
    #[test]
    fn legal_actions_gates_star_cost_unless_free_or_paid() {
        use crate::cards::{Card, CardId, CardType, TargetType, VarKey};
        // Self-targeted stand-in (star gate keys on card_id, not type) so
        // the test needs no live enemy on the board.
        let mk = |uid_raw: u32, id: CardId| Card {
            uid: CardUid::new(uid_raw).expect("non-zero"),
            card_id: id,
            cost: 0,
            card_type: CardType::Skill,
            target_type: TargetType::Self_,
            upgraded: false,
            base_damage: 0,
            base_block: 0,
            ..Card::pod_defaults()
        };
        let star_card_offered = |stars: i8, free: bool| {
            let mut state = CombatState {
                phase: CombatPhase::PlayerPlayPhase,
                ..CombatState::default()
            };
            let mut c = mk(1, CardId::SevenStars);
            if free {
                c.set_combat_var(VarKey::FreeThisTurn, 1);
            }
            state.cards.push(c);
            state.player.hand.push(CardUid::new(1).expect("non-zero"));
            state.player.energy = 3;
            state.player.stars = stars;
            legal_actions(&state)
                .iter()
                .any(|a| matches!(a, Action::PlayCard { .. }))
        };
        assert!(
            !star_card_offered(0, false),
            "SevenStars (star cost 7) is unplayable at 0 stars"
        );
        assert!(
            star_card_offered(0, true),
            "FreeThisTurn zeros the star cost → playable at 0 stars"
        );
        assert!(
            star_card_offered(7, false),
            "7 stars covers SevenStars' star cost → playable"
        );
    }

    /// Bead `sts2-rs-starcost-free-combat`: `TouchOfInsanity`'s
    /// `SetToFreeThisCombat` zeros a card's star cost for the whole combat
    /// (`VarKey::StarCostFreeThisCombat`), so a star card stamped with it is
    /// playable at 0 stars — and `effective_star_cost` reports 0. This is the
    /// star half of C# `CardModel.SetToFreeThisCombat()` →
    /// `SetStarCostThisCombat(0)` that the energy-only `cost = 0` stamp missed
    /// (parity-fuzz Regent seeds 230/320/405). Even a 0-energy star card
    /// (FallingStar / Quasar) becomes playable.
    #[test]
    fn star_cost_free_this_combat_makes_star_card_playable_at_zero_stars() {
        use crate::cards::{Card, CardId, CardType, TargetType, VarKey};
        let mut state = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..CombatState::default()
        };
        // DyingStar: star cost 3, here with energy cost 0 (e.g. Confused), so
        // only the star gate could suppress it.
        let mut c = Card {
            uid: CardUid::new(1).expect("non-zero"),
            card_id: CardId::DyingStar,
            cost: 0,
            card_type: CardType::Attack,
            target_type: TargetType::AllEnemies,
            ..Card::pod_defaults()
        };
        // Without the free marker, base star cost stands.
        assert_eq!(effective_star_cost(&c), 3, "base DyingStar star cost is 3");
        c.set_combat_var(VarKey::StarCostFreeThisCombat, 1);
        assert_eq!(
            effective_star_cost(&c),
            0,
            "StarCostFreeThisCombat zeros the star cost"
        );
        state.cards.push(c);
        state.player.hand.push(CardUid::new(1).expect("non-zero"));
        state.player.energy = 3;
        state.player.stars = 0;
        assert!(
            legal_actions(&state)
                .iter()
                .any(|a| matches!(a, Action::PlayCard { .. })),
            "a SetToFreeThisCombat star card is playable at 0 stars"
        );
    }

    /// Bead `sts2-rs-voidform-starcost`: `VoidFormPower` zeroes the STAR cost
    /// (not just the energy cost) of the first `Amount` cards the owner plays
    /// each turn — C# `VoidFormPower.TryModifyStarCost` mirrors its
    /// `TryModifyEnergyCostInCombat`. `effective_star_cost_in_combat` folds the
    /// VoidForm gate in, so a star card is playable at 0 stars while the gate
    /// is open and gated again once `Amount` cards have been played.
    /// Parity-fuzz Regent seeds 12/15/44/382/419/432 (a VoidForm play freed
    /// the next ASTRAL_PULSE / RESONANCE in C#; Rust kept gating them on star
    /// cost).
    ///
    /// GH #363 A2: this test used to drive `player.cards_played_this_turn`,
    /// pinning the very conflation the issue reports — that bucket array is
    /// C# `History.CardPlaysFinished` (auto-plays and every replay iteration
    /// included), not `VoidFormPower.Data.cardsPlayedThisTurn`. It now drives
    /// the instance counter the gate actually reads.
    #[test]
    fn voidform_zeros_star_cost_for_first_n_cards() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        let mut state = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..CombatState::default()
        };
        // Player holds VoidForm(2): the first 2 owner plays this turn are free.
        let power_id = NonZeroU32::new(1).expect("non-zero");
        state
            .powers
            .push(Power::new(power_id, PowerKind::VoidForm, 2));
        state.player.powers.push(PowerUid(power_id));

        // A 0-energy / 3-star card (e.g. ASTRAL_PULSE-shaped).
        let c = Card {
            uid: CardUid::new(1).expect("non-zero"),
            card_id: CardId::DyingStar,
            cost: 0,
            card_type: CardType::Attack,
            target_type: TargetType::AllEnemies,
            ..Card::pod_defaults()
        };
        assert_eq!(effective_star_cost(&c), 3, "base DyingStar star cost is 3");

        // The gate reads VoidForm's OWN per-instance tally, held on `counter`.
        let set_tally = |st: &mut CombatState, n: i32| {
            st.powers
                .get_mut_by_id(power_id)
                .expect("voidform power")
                .counter = n;
        };

        // 0 cards played → first free card: star cost 0.
        assert_eq!(
            crate::combat_loop::void_form_cards_played_this_turn(&state),
            0
        );
        assert_eq!(
            effective_star_cost_in_combat(&state, &c),
            0,
            "VoidForm zeros the star cost of the 1st card played this turn"
        );

        // 1 card played → still under the cap (Amount 2): star cost 0.
        set_tally(&mut state, 1);
        assert_eq!(
            effective_star_cost_in_combat(&state, &c),
            0,
            "VoidForm zeros the star cost of the 2nd card too"
        );

        // 2 cards played → cap reached: base star cost stands again.
        set_tally(&mut state, 2);
        assert_eq!(
            effective_star_cost_in_combat(&state, &c),
            3,
            "after Amount cards, VoidForm no longer frees the star cost"
        );

        // GH #363 A2: the all-types bucket array is a DIFFERENT quantity —
        // filling it must not close the gate on its own, because auto-plays
        // and replay iterations land there and never touch VoidForm in C#.
        set_tally(&mut state, 0);
        state.player.cards_played_this_turn[CardType::Attack.index()] = 5;
        assert_eq!(
            effective_star_cost_in_combat(&state, &c),
            0,
            "History.CardPlaysFinished does not charge VoidForm"
        );

        // Legal gate at 0 stars: playable while the gate is open.
        state.cards.push(c);
        state.player.hand.push(CardUid::new(1).expect("non-zero"));
        state.player.energy = 3;
        state.player.stars = 0;
        state.player.cards_played_this_turn = [0; 5];
        assert!(
            legal_actions(&state)
                .iter()
                .any(|a| matches!(a, Action::PlayCard { .. })),
            "a VoidForm-freed star card is playable at 0 stars"
        );
    }

    /// Bead `sts2-rs-fa3w`: Normality in hand + ≥3 plays this turn
    /// gates all further plays. Below the cap (0-2 plays) Normality
    /// has no effect.
    #[test]
    fn legal_actions_normality_caps_at_three_plays() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        let mk = |uid_raw: u32, id: CardId| Card {
            uid: CardUid::new(uid_raw).expect("non-zero"),
            card_id: id,
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Self_,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        };
        let strike = CardUid::new(1).expect("non-zero");
        let normality = CardUid::new(2).expect("non-zero");
        let mut state = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..CombatState::default()
        };
        state.cards.push(mk(1, CardId::StrikeIronclad));
        state.cards.push(mk(2, CardId::Normality));
        state.player.hand.push(strike);
        state.player.hand.push(normality);
        state.player.energy = 3;

        // 0 cards played → Strike playable.
        let acts = legal_actions(&state);
        let strike_legal = acts
            .iter()
            .any(|a| matches!(a, Action::PlayCard { hand_index, .. } if *hand_index == 0));
        assert!(strike_legal, "Strike playable at 0 plays this turn");

        // 3 cards played → all plays gated.
        state.player.cards_played_this_turn[CardType::Attack.index()] = 3;
        let acts = legal_actions(&state);
        let any_play_card = acts.iter().any(|a| matches!(a, Action::PlayCard { .. }));
        assert!(
            !any_play_card,
            "Normality must gate all card plays at ≥3 plays/turn"
        );
    }

    /// Bead `sts2-rs-6diaj`: Clash's C# `IsPlayable`
    /// (`Clash.cs:20`, v0.103.2) requires EVERY card in hand to be an
    /// Attack. With a non-Attack (Skill) in hand Clash is unplayable;
    /// once the hand is all-Attacks Clash surfaces.
    #[test]
    fn legal_actions_clash_requires_all_attacks_in_hand() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        let mk = |uid_raw: u32, id: CardId, ct: CardType| Card {
            uid: CardUid::new(uid_raw).expect("non-zero"),
            card_id: id,
            cost: 0,
            card_type: ct,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 14,
            base_block: 0,
            ..Card::pod_defaults()
        };
        let clash = CardUid::new(1).expect("non-zero");
        let strike = CardUid::new(2).expect("non-zero");
        let defend = CardUid::new(3).expect("non-zero");
        let mut state = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..CombatState::default()
        };
        state.cards.push(mk(1, CardId::Clash, CardType::Attack));
        state
            .cards
            .push(mk(2, CardId::StrikeIronclad, CardType::Attack));
        state
            .cards
            .push(mk(3, CardId::DefendIronclad, CardType::Skill));
        state.player.energy = 3;
        // Give the player an alive enemy so Enemy-target plays surface.
        state
            .enemy_uids
            .push(CreatureUid(NonZeroU32::new(50).expect("nz")));
        state.creatures.push(alive_enemy(50));

        // Hand = [Clash, Strike, Defend] → a Skill present → Clash gated.
        state.player.hand.push(clash);
        state.player.hand.push(strike);
        state.player.hand.push(defend);
        let clash_legal = |s: &CombatState| {
            legal_actions(s).iter().any(|a| {
                matches!(a, Action::PlayCard { hand_index, .. }
                if s.player.hand[*hand_index as usize] == clash)
            })
        };
        assert!(
            !clash_legal(&state),
            "Clash unplayable while a non-Attack (Defend) sits in hand"
        );

        // Drop the Skill → hand all-Attacks → Clash playable.
        state.player.hand.retain(|u| *u != defend);
        assert!(
            clash_legal(&state),
            "Clash playable once every hand card is an Attack"
        );
    }

    /// Bead `sts2-rs-6diaj`: GrandFinale's C# `IsPlayable`
    /// (`GrandFinale.cs:17`, v0.103.2) requires the draw pile to be
    /// EMPTY. With a card still in draw it is unplayable; with an empty
    /// draw pile it surfaces.
    #[test]
    fn legal_actions_grand_finale_requires_empty_draw() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        let gf = CardUid::new(1).expect("non-zero");
        let drawn = CardUid::new(2).expect("non-zero");
        let mut state = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..CombatState::default()
        };
        state.cards.push(Card {
            uid: gf,
            card_id: CardId::GrandFinale,
            cost: 0,
            card_type: CardType::Attack,
            target_type: TargetType::AllEnemies,
            upgraded: false,
            base_damage: 60,
            base_block: 0,
            ..Card::pod_defaults()
        });
        state.cards.push(Card {
            uid: drawn,
            card_id: CardId::StrikeIronclad,
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        });
        state.player.energy = 3;
        state
            .enemy_uids
            .push(CreatureUid(NonZeroU32::new(50).expect("nz")));
        state.creatures.push(alive_enemy(50));
        state.player.hand.push(gf);

        let gf_legal = |s: &CombatState| {
            legal_actions(s).iter().any(|a| {
                matches!(a, Action::PlayCard { hand_index, .. }
                if s.player.hand[*hand_index as usize] == gf)
            })
        };

        // A card in the draw pile → GrandFinale gated.
        state.player.draw.push(drawn);
        assert!(
            !gf_legal(&state),
            "GrandFinale unplayable while the draw pile is non-empty"
        );

        // Empty draw → GrandFinale playable.
        state.player.draw.clear();
        assert!(
            gf_legal(&state),
            "GrandFinale playable once the draw pile is empty"
        );
    }

    // ── sts2-rs-419n: PendingChoice enumeration + dispatch ──────────────

    /// `legal_actions` for a single-pick `PutBack` choice with N options
    /// returns N entries — one per option index. Bead `sts2-rs-419n`.
    #[test]
    fn legal_actions_enumerates_one_per_putback_option() {
        let mk_opt = |raw: u32| ChoiceOption {
            card_id: cards::CardId::StrikeIronclad,
            upgraded: false,
            card_uid: Some(CardUid::new(raw).expect("non-zero")),
            source_pile: Some(cards::PileKind::Discard),
        };
        let s = CombatState {
            pending_choice: Some(PendingChoice {
                kind: ChoiceKind::PutBack,
                options: vec![mk_opt(1), mk_opt(2), mk_opt(3)],
                min_select: 1,
                max_select: 1,
                can_skip: false,
                post_effect: None,
                picked: SmallVec::new(),
            }),
            ..CombatState::default()
        };
        let acts = legal_actions(&s);
        assert_eq!(acts.len(), 3, "one ResolveChoice per option");
        let mut seen: Vec<u8> = Vec::new();
        for a in &acts {
            match a {
                Action::ResolveChoice { selection } => {
                    assert_eq!(selection.len(), 1, "single-pick selection");
                    seen.push(selection[0]);
                }
                _ => panic!("expected only ResolveChoice variants"),
            }
        }
        seen.sort_unstable();
        assert_eq!(seen, vec![0u8, 1, 2]);
    }

    /// Bead `sts2-rs-sopgf`: a freshly-opened multi-pick (`max_select > 1`,
    /// empty `picked`) fans out to SEQUENTIAL single-index incremental picks
    /// (one per option) plus a FINALIZE (empty) selection only once the
    /// minimum is satisfied or a skip is allowed — NOT the prior bounded
    /// "first-K" complete selections.
    #[test]
    fn legal_actions_multi_pick_fans_out_first_k_selections() {
        let mk_opt = |raw: u32| ChoiceOption {
            card_id: cards::CardId::StrikeIronclad,
            upgraded: false,
            card_uid: Some(CardUid::new(raw).expect("non-zero")),
            source_pile: Some(cards::PileKind::Hand),
        };
        let opts = vec![mk_opt(1), mk_opt(2), mk_opt(3), mk_opt(4)];

        // Fixed-count (min == max == 2), nothing picked: no FINALIZE yet (min
        // not met), one incremental pick per option.
        let s = CombatState {
            pending_choice: Some(PendingChoice {
                kind: ChoiceKind::CardSelect,
                options: opts.clone(),
                min_select: 2,
                max_select: 2,
                can_skip: false,
                post_effect: None,
                picked: SmallVec::new(),
            }),
            ..CombatState::default()
        };
        let sels: Vec<Vec<u8>> = legal_actions(&s)
            .iter()
            .map(|a| match a {
                Action::ResolveChoice { selection } => selection.to_vec(),
                _ => panic!("expected only ResolveChoice"),
            })
            .collect();
        assert_eq!(
            sels,
            vec![vec![0u8], vec![1], vec![2], vec![3]],
            "fresh fixed-count → single-index picks, no early FINALIZE"
        );

        // Variable (min 1, max 3), nothing picked: min not met → no FINALIZE,
        // one incremental pick per option.
        let s = CombatState {
            pending_choice: Some(PendingChoice {
                kind: ChoiceKind::CardSelect,
                options: opts.clone(),
                min_select: 1,
                max_select: 3,
                can_skip: false,
                post_effect: None,
                picked: SmallVec::new(),
            }),
            ..CombatState::default()
        };
        let sels: Vec<Vec<u8>> = legal_actions(&s)
            .iter()
            .map(|a| match a {
                Action::ResolveChoice { selection } => selection.to_vec(),
                _ => panic!("expected only ResolveChoice"),
            })
            .collect();
        assert_eq!(sels, vec![vec![0u8], vec![1], vec![2], vec![3]]);

        // Skippable (min 0, max 2), nothing picked: zero picks satisfy the
        // minimum → FINALIZE leads, then one incremental pick per option.
        let s = CombatState {
            pending_choice: Some(PendingChoice {
                kind: ChoiceKind::CardSelect,
                options: opts.clone(),
                min_select: 0,
                max_select: 2,
                can_skip: false,
                post_effect: None,
                picked: SmallVec::new(),
            }),
            ..CombatState::default()
        };
        let sels: Vec<Vec<u8>> = legal_actions(&s)
            .iter()
            .map(|a| match a {
                Action::ResolveChoice { selection } => selection.to_vec(),
                _ => panic!("expected only ResolveChoice"),
            })
            .collect();
        assert_eq!(
            sels,
            vec![vec![], vec![0u8], vec![1], vec![2], vec![3]],
            "min_select 0 emits the FINALIZE selection first"
        );

        // After ONE pick on the fixed-count choice (picked = [0]): the picked
        // index is excluded, still no FINALIZE (min not met), picks are the
        // remaining options.
        let s = CombatState {
            pending_choice: Some(PendingChoice {
                kind: ChoiceKind::CardSelect,
                options: opts.clone(),
                min_select: 2,
                max_select: 2,
                can_skip: false,
                post_effect: None,
                picked: smallvec::smallvec![0u8],
            }),
            ..CombatState::default()
        };
        let sels: Vec<Vec<u8>> = legal_actions(&s)
            .iter()
            .map(|a| match a {
                Action::ResolveChoice { selection } => selection.to_vec(),
                _ => panic!("expected only ResolveChoice"),
            })
            .collect();
        assert_eq!(
            sels,
            vec![vec![1u8], vec![2], vec![3]],
            "picked index 0 excluded; min still unmet so no FINALIZE"
        );

        // Variable choice with the minimum already met (picked = [0], min 1):
        // FINALIZE becomes legal and leads, then remaining picks up to max.
        let s = CombatState {
            pending_choice: Some(PendingChoice {
                kind: ChoiceKind::CardSelect,
                options: opts,
                min_select: 1,
                max_select: 3,
                can_skip: false,
                post_effect: None,
                picked: smallvec::smallvec![0u8],
            }),
            ..CombatState::default()
        };
        let sels: Vec<Vec<u8>> = legal_actions(&s)
            .iter()
            .map(|a| match a {
                Action::ResolveChoice { selection } => selection.to_vec(),
                _ => panic!("expected only ResolveChoice"),
            })
            .collect();
        assert_eq!(
            sels,
            vec![vec![], vec![1u8], vec![2], vec![3]],
            "min met → FINALIZE leads; picked index 0 excluded"
        );
    }

    /// `apply(ResolveChoice { [idx] })` for a `PutBack` choice removes
    /// the chosen card from the source pile and inserts it at top of
    /// draw (`draw[0]`); `pending_choice` is cleared. Bead `sts2-rs-419n`.
    #[test]
    fn resolve_putback_choice_moves_card_from_discard_to_top_of_draw() {
        let u1 = CardUid::new(11).expect("non-zero");
        let u2 = CardUid::new(22).expect("non-zero");
        let u3 = CardUid::new(33).expect("non-zero");
        let mut s = CombatState::default();
        s.player.discard.extend([u1, u2, u3]);
        let mk_opt = |uid| ChoiceOption {
            card_id: cards::CardId::StrikeIronclad,
            upgraded: false,
            card_uid: Some(uid),
            source_pile: Some(cards::PileKind::Discard),
        };
        s.pending_choice = Some(PendingChoice {
            kind: ChoiceKind::PutBack,
            options: vec![mk_opt(u1), mk_opt(u2), mk_opt(u3)],
            min_select: 1,
            max_select: 1,
            can_skip: false,
            post_effect: None,
            picked: SmallVec::new(),
        });
        let mut sel: SmallVec<[u8; 4]> = SmallVec::new();
        sel.push(1); // pick u2
        let res = apply(&mut s, Action::ResolveChoice { selection: sel });
        assert_eq!(res, StepResult::Advanced);
        assert!(s.pending_choice.is_none(), "choice cleared");
        assert_eq!(
            s.player.draw.first().copied(),
            Some(u2),
            "u2 on top of draw"
        );
        assert!(!s.player.discard.contains(&u2), "u2 removed from discard");
        assert_eq!(s.player.discard.len(), 2, "other discard entries kept");
        // Bead `sts2-rs-o6xw`: the PutBack resolver pins the inserted
        // card to `Top(0)` so MCTS PIMC determinization re-shuffles
        // preserve the known position.
        assert_eq!(
            s.player.pinned_top.first().copied(),
            Some(u2),
            "Headbutt-style resolver must pin the inserted card to Top(0)"
        );
    }

    /// `apply(ResolveChoice { [idx] })` for an `AddFromDrawByType`
    /// choice (SecretTechnique/SecretWeapon) removes the chosen card from
    /// the draw pile and appends it to hand; `pending_choice` is cleared.
    /// Bead `sts2-rs-1xmxp`.
    #[test]
    fn resolve_add_from_draw_by_type_moves_chosen_card_draw_to_hand() {
        let u1 = CardUid::new(11).expect("non-zero");
        let u2 = CardUid::new(22).expect("non-zero");
        let mut s = CombatState::default();
        s.player.draw.extend([u1, u2]);
        let mk_opt = |uid| ChoiceOption {
            card_id: cards::CardId::StrikeIronclad,
            upgraded: false,
            card_uid: Some(uid),
            source_pile: Some(cards::PileKind::Draw),
        };
        s.pending_choice = Some(PendingChoice {
            kind: ChoiceKind::AddFromDrawByType,
            options: vec![mk_opt(u1), mk_opt(u2)],
            min_select: 1,
            max_select: 1,
            can_skip: false,
            post_effect: None,
            picked: SmallVec::new(),
        });
        let mut sel: SmallVec<[u8; 4]> = SmallVec::new();
        sel.push(1); // pick u2
        let res = apply(&mut s, Action::ResolveChoice { selection: sel });
        assert_eq!(res, StepResult::Advanced);
        assert!(s.pending_choice.is_none(), "choice cleared");
        assert!(s.player.hand.contains(&u2), "u2 moved to hand");
        assert!(!s.player.draw.contains(&u2), "u2 removed from draw");
        assert_eq!(s.player.draw.len(), 1, "only u1 stays in draw");
        assert_eq!(s.player.draw.first().copied(), Some(u1), "u1 retained");
    }

    /// Bead `sts2-rs-09x5n`: if the chosen draw-pile card was previously
    /// PINNED (e.g. a Headbutt/ThinkingAhead top-pin later picked by
    /// SecretTechnique/Stratagem's `AddFromDrawByType`), the resolver must
    /// clear the stale pin — otherwise a later `pop_next_card` sees a
    /// non-empty `pinned_top` and wrongly short-circuits to a deterministic
    /// pop for what is now a plain heterogeneous middle, silently
    /// suppressing a chance node.
    #[test]
    fn resolve_add_from_draw_by_type_unpins_stale_pin_and_keeps_chance_node() {
        let u1 = CardUid::new(11).expect("non-zero");
        let u2 = CardUid::new(22).expect("non-zero");
        let u3 = CardUid::new(33).expect("non-zero");
        let mut s = CombatState::default();
        s.player.deferred_draw_order = true;
        // u2 sits physically on top, previously pinned there.
        s.player.draw.extend([u2, u1, u3]);
        cards::pin_top(&mut s.player, u2);

        let mk_opt = |uid| ChoiceOption {
            card_id: cards::CardId::StrikeIronclad,
            upgraded: false,
            card_uid: Some(uid),
            source_pile: Some(cards::PileKind::Draw),
        };
        s.pending_choice = Some(PendingChoice {
            kind: ChoiceKind::AddFromDrawByType,
            options: vec![mk_opt(u2)],
            min_select: 1,
            max_select: 1,
            can_skip: false,
            post_effect: None,
            picked: SmallVec::new(),
        });
        let mut sel: SmallVec<[u8; 4]> = SmallVec::new();
        sel.push(0); // pick u2, the pinned card
        let res = apply(&mut s, Action::ResolveChoice { selection: sel });

        assert_eq!(res, StepResult::Advanced);
        assert!(s.player.hand.contains(&u2), "u2 moved to hand");
        assert!(
            s.player.pinned_top.is_empty(),
            "the picked card's stale pin must not survive it leaving the draw pile"
        );
        // u1, u3 remain: a >=2-card unknown middle. `pop_next_card` must
        // still consume RNG for it — no stale pin may suppress the chance
        // node for a draw the player genuinely does not know the order of.
        let mut rng = crate::dotnet_rng::Rng::new(9);
        let before = rng.counter();
        let _ = cards::pop_next_card(&mut s.player, &mut rng);
        assert_eq!(
            rng.counter(),
            before + 1,
            "no stale pin should suppress the next draw's chance node"
        );
    }

    /// Final-branch-review finding (bead `sts2-rs-ztl78`): `ForegoneConclusion`
    /// (Regent) picks cards from the draw pile by (rarity, id) sort — not by
    /// physical position — so it can legitimately pick the card currently
    /// sitting at `pinned_top` (set by Headbutt/CosmicIndifference/the
    /// round-1 Innate move-to-top/ShiningStrike/Nostalgia). The resolver's
    /// raw `draw.remove(pos)` must clear that stale pin, or a later
    /// `pop_next_card` sees a non-empty `pinned_top` and wrongly
    /// short-circuits to a deterministic zero-RNG pop for what is now a
    /// plain heterogeneous middle, silently suppressing a chance node.
    #[test]
    fn resolve_foregone_conclusion_from_draw_unpins_stale_pin_and_keeps_chance_node() {
        let u1 = CardUid::new(11).expect("non-zero");
        let u2 = CardUid::new(22).expect("non-zero");
        let u3 = CardUid::new(33).expect("non-zero");
        let mut s = CombatState::default();
        s.player.deferred_draw_order = true;
        // u2 sits physically on top, previously pinned there.
        s.player.draw.extend([u2, u1, u3]);
        cards::pin_top(&mut s.player, u2);

        let mk_opt = |uid| ChoiceOption {
            card_id: cards::CardId::StrikeIronclad,
            upgraded: false,
            card_uid: Some(uid),
            source_pile: Some(cards::PileKind::Draw),
        };
        s.pending_choice = Some(PendingChoice {
            kind: ChoiceKind::ForegoneConclusionFromDraw,
            options: vec![mk_opt(u2)],
            min_select: 1,
            max_select: 1,
            can_skip: false,
            post_effect: None,
            picked: SmallVec::new(),
        });
        let mut sel: SmallVec<[u8; 4]> = SmallVec::new();
        sel.push(0); // pick u2, the pinned card
        let res = apply(&mut s, Action::ResolveChoice { selection: sel });

        assert_eq!(res, StepResult::Advanced);
        assert!(s.player.hand.contains(&u2), "u2 moved to hand");
        assert!(
            s.player.pinned_top.is_empty(),
            "ForegoneConclusion's resolver must not leave a stale pin on the picked card"
        );
        // u1, u3 remain: a >=2-card unknown middle. `pop_next_card` must
        // still consume RNG for it — no stale pin may suppress the chance
        // node for a draw the player genuinely does not know the order of.
        let mut rng = crate::dotnet_rng::Rng::new(9);
        let before = rng.counter();
        let _ = cards::pop_next_card(&mut s.player, &mut rng);
        assert_eq!(
            rng.counter(),
            before + 1,
            "no stale pin should suppress the next draw's chance node"
        );
    }

    /// Rollout-style: an MCTS-style step loop that plays a card setting
    /// up a `PutBack` pending choice, then resolves it via legal_actions
    /// + apply. Asserts the chosen card actually moved — i.e. the
    /// resolver fired through the dispatch trampoline rather than being
    /// silently dropped (the v2oh bug). Bead `sts2-rs-419n`.
    #[test]
    fn rollout_loop_resolves_putback_choice_to_completion() {
        let u1 = CardUid::new(101).expect("non-zero");
        let u2 = CardUid::new(102).expect("non-zero");
        let mut s = CombatState::default();
        s.player.discard.extend([u1, u2]);
        let mk_opt = |uid| ChoiceOption {
            card_id: cards::CardId::StrikeIronclad,
            upgraded: false,
            card_uid: Some(uid),
            source_pile: Some(cards::PileKind::Discard),
        };
        s.pending_choice = Some(PendingChoice {
            kind: ChoiceKind::PutBack,
            options: vec![mk_opt(u1), mk_opt(u2)],
            min_select: 1,
            max_select: 1,
            can_skip: false,
            post_effect: None,
            picked: SmallVec::new(),
        });
        // Simulate a rollout step loop bounded by depth=4 (anchored to
        // the documented MCTS depth=10 default; one resolve is enough
        // here, but the cap proves we don't loop).
        let mut steps = 0usize;
        while s.pending_choice.is_some() && steps < 4 {
            let acts = legal_actions(&s);
            assert!(
                !acts.is_empty(),
                "legal_actions must not be empty mid-choice"
            );
            // Pick the first emitted action (deterministic).
            let act = acts.into_iter().next().expect("at least one action");
            let _ = apply(&mut s, act);
            steps += 1;
        }
        assert!(s.pending_choice.is_none(), "rollout terminated the choice");
        assert_eq!(s.player.draw.first().copied(), Some(u1));
        assert_eq!(steps, 1, "single resolve step suffices");
    }

    /// `apply(ResolveChoice { ... })` always clears `pending_choice` —
    /// even for unimplemented `ChoiceKind` variants — so MCTS rollouts
    /// can never spin on an unresolved choice. This is the contract the
    /// removed `sts2-rs-v2oh` hotfix used to enforce; the per-variant
    /// dispatch must preserve it. Bead `sts2-rs-419n`.
    #[test]
    fn resolve_choice_always_clears_pending_choice() {
        for kind in [
            ChoiceKind::CardSelect,
            ChoiceKind::PutBack,
            ChoiceKind::ExhaustPile,
            ChoiceKind::OrbSlot,
            ChoiceKind::EnemyTarget,
        ] {
            let mut s = CombatState {
                pending_choice: Some(PendingChoice {
                    kind,
                    ..PendingChoice::default()
                }),
                ..CombatState::default()
            };
            let _ = apply(
                &mut s,
                Action::ResolveChoice {
                    selection: SmallVec::new(),
                },
            );
            assert!(
                s.pending_choice.is_none(),
                "ResolveChoice must clear pending_choice for kind {kind:?}",
            );
        }
    }

    // ── P7-6 (sts2-rs-13q): DrinkPotion legal_actions tests ───────────

    /// `legal_actions` emits an `Action::DrinkPotion` for every non-empty
    /// slot in `state.player.potions` when in `PlayerPlayPhase`. No
    /// `Action::UsePotion` variants appear (the old, legacy code path is
    /// no longer reachable through `legal_actions`).
    #[test]
    fn legal_actions_emits_drink_potion_per_slot() {
        let mut s = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..CombatState::default()
        };
        s.player.potions.push(PotionKind::BlockPotion);
        s.player.potions.push(PotionKind::AttackPotion);
        let acts = legal_actions(&s);
        let drinks: Vec<_> = acts
            .iter()
            .filter_map(|a| match a {
                Action::DrinkPotion { potion_slot, .. } => Some(*potion_slot),
                _ => None,
            })
            .collect();
        assert_eq!(drinks, vec![0, 1]);
        assert!(
            !acts.iter().any(|a| matches!(a, Action::UsePotion { .. })),
            "legacy UsePotion variant must not appear in legal_actions",
        );
    }

    /// `ChoiceKind::ALL` must list every variant exactly once, in the
    /// order `variant_index` declares. The exhaustive match in
    /// `variant_index` turns a NEW variant into a compile error; this
    /// catches the other half — a variant that was added to the match but
    /// never appended to `ALL`, or appended twice.
    #[test]
    fn choice_kind_all_agrees_with_variant_index() {
        for (i, k) in ChoiceKind::ALL.iter().enumerate() {
            assert_eq!(
                k.variant_index(),
                i,
                "ChoiceKind::ALL[{i}] = {k:?} is out of position"
            );
        }
        let mut seen: Vec<usize> = ChoiceKind::ALL.iter().map(|k| k.variant_index()).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            seen.len(),
            ChoiceKind::ALL.len(),
            "ChoiceKind::ALL contains a duplicate"
        );
    }

    /// GH #303: the legacy `Action::UsePotion` variant must consume the
    /// belt slot its caller named. It used to hit a Phase-7 stub that
    /// returned `Advanced` while leaving the potion in hand, so callers
    /// using the documented back-compat verb lost the drink and were
    /// told it worked.
    ///
    /// The belt holds two DIFFERENT potions and the drink names slot 1,
    /// so a delegation that dropped the field (or hardcoded 0) removes
    /// the wrong kind and fails here. Comparing against a parallel
    /// `DrinkPotion` run would NOT catch that: the arm delegates by
    /// construction, so the two states are equal whatever `slot` does.
    /// Forwarding of `target` needs a live potion body to observe and is
    /// covered in `sts2-content`
    /// (`use_potion_alias_forwards_slot_and_target`).
    #[test]
    fn use_potion_consumes_the_named_belt_slot() {
        let mut s = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..CombatState::default()
        };
        s.player.max_hp = 80;
        s.player.current_hp = 40;
        s.player.potions.push(PotionKind::BlockPotion);
        s.player.potions.push(PotionKind::AttackPotion);

        let r = apply(
            &mut s,
            Action::UsePotion {
                slot: 1,
                target: None,
            },
        );
        assert_eq!(r, StepResult::Advanced);
        assert_eq!(
            s.player.potions.as_slice(),
            [PotionKind::BlockPotion],
            "UsePotion must consume the NAMED slot, not silently no-op \
             and not the first slot"
        );
    }

    /// Character-locked potions are filtered out of `legal_actions`
    /// when the player's `character` does not match the potion's lock.
    /// Defect's `FocusPotion` is character-locked to `Defect`; an
    /// Ironclad holding it cannot drink it.
    #[test]
    fn legal_actions_filters_character_locked_potions() {
        let mut s = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..CombatState::default()
        };
        s.player.character = Some(CharacterId::Ironclad);
        s.player.potions.push(PotionKind::FocusPotion); // Defect-locked
        s.player.potions.push(PotionKind::BlockPotion); // unlocked
        s.player.potions.push(PotionKind::BloodPotion); // Ironclad-locked
        let acts = legal_actions(&s);
        let kinds: Vec<u8> = acts
            .iter()
            .filter_map(|a| match a {
                Action::DrinkPotion { potion_slot, .. } => Some(*potion_slot),
                _ => None,
            })
            .collect();
        // Slot 0 (FocusPotion) filtered out; slots 1 + 2 retained.
        assert_eq!(kinds, vec![1, 2]);
    }

    /// When `player.character` is `None`, gating is disabled and
    /// character-locked potions are still emitted (fixture-friendly
    /// default).
    #[test]
    fn legal_actions_no_character_means_no_gating() {
        let mut s = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..CombatState::default()
        };
        assert!(s.player.character.is_none());
        s.player.potions.push(PotionKind::FocusPotion);
        let acts = legal_actions(&s);
        assert_eq!(
            acts.iter()
                .filter(|a| matches!(a, Action::DrinkPotion { .. }))
                .count(),
            1,
        );
    }

    /// A8 audit §7a: `PotionUsage.Automatic` potions (Fairy in a
    /// Bottle) are not manually drinkable. The Fairy's slot must not
    /// surface a `DrinkPotion`, while a normal potion in the NEXT slot
    /// keeps its own slot index (the gate skips, it doesn't compact).
    #[test]
    fn legal_actions_skips_automatic_potions() {
        let mut s = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..CombatState::default()
        };
        s.player.potions.push(PotionKind::FairyInABottle); // Automatic
        s.player.potions.push(PotionKind::BlockPotion); // normal
        let acts = legal_actions(&s);
        let drinks: Vec<u8> = acts
            .iter()
            .filter_map(|a| match a {
                Action::DrinkPotion { potion_slot, .. } => Some(*potion_slot),
                _ => None,
            })
            .collect();
        assert_eq!(
            drinks,
            vec![1],
            "Fairy slot 0 must be skipped; BlockPotion keeps slot 1"
        );
    }

    /// A8 audit §7a: a manual `DrinkPotion` aimed at an Automatic slot
    /// is rejected as `InvalidTarget` — the non-mutating legality
    /// reject, NOT a success-reporting `Advanced` no-op that would
    /// desync an external driver's belt model (GH #303). C#
    /// `FairyInABottle.OnUse` "should never be executed directly by
    /// `UsePotionAction`". The Fairy stays in the belt (so its
    /// death-prevention trigger still has it) and no heal fires. The
    /// legacy `UsePotion` alias hits the same gate via delegation.
    #[test]
    fn manual_drink_of_automatic_potion_is_rejected() {
        let mut s = CombatState {
            phase: CombatPhase::PlayerPlayPhase,
            ..CombatState::default()
        };
        s.player.max_hp = 70;
        s.player.current_hp = 30;
        s.player.potions.push(PotionKind::FairyInABottle);
        for action in [
            Action::DrinkPotion {
                potion_slot: 0,
                target: None,
            },
            Action::UsePotion {
                slot: 0,
                target: None,
            },
        ] {
            let r = apply(&mut s, action);
            assert_eq!(r, StepResult::InvalidTarget);
            assert_eq!(
                s.player.potions.as_slice(),
                [PotionKind::FairyInABottle],
                "rejected drink must NOT consume the Fairy"
            );
            assert_eq!(s.player.current_hp, 30, "no heal may fire");
        }
    }

    #[test]
    fn step_result_is_copy() {
        // Copy bound is load-bearing for downstream MCTS rollout loops.
        fn assert_copy<T: Copy>() {}
        assert_copy::<StepResult>();
        assert_copy::<CombatPhase>();
        assert_copy::<Side>();
    }

    // --- v7mp: combat-side helper sentinels -------------------------------

    /// `creature_heal` clamps to `max_hp`, returns the applied delta,
    /// and mirrors the player-slot row to `state.player.current_hp`.
    #[test]
    fn creature_heal_clamps_and_mirrors_player_row() {
        let mut state = CombatState::default();
        state.player.current_hp = 50;
        state.player.max_hp = 80;
        let uid = state.init_player_creature();
        state.sync_player_creature();
        // Heal under max: full delta applied.
        assert_eq!(state.creature_heal(uid, 20), 20);
        assert_eq!(state.player.current_hp, 70);
        // Heal over max: clamps.
        assert_eq!(state.creature_heal(uid, 50), 10);
        assert_eq!(state.player.current_hp, 80);
        // Negative / zero is a no-op and returns 0.
        assert_eq!(state.creature_heal(uid, 0), 0);
        assert_eq!(state.creature_heal(uid, -5), 0);
        // Missing uid returns 0.
        let foreign = CreatureUid(NonZeroU32::new(999).expect("non-zero"));
        assert_eq!(state.creature_heal(foreign, 5), 0);
    }

    /// `get_random_enemy` returns `None` when no enemies are alive and
    /// `Some(uid)` from the alive-filtered `enemy_uids` set when at
    /// least one is alive.
    #[test]
    fn get_random_enemy_filters_dead_and_returns_alive_uids() {
        let mut state = CombatState::default();
        // No enemies: None.
        assert!(state.get_random_enemy().is_none());
        // Push a live enemy.
        let alive_id = NonZeroU32::new(7).expect("non-zero");
        state.creatures.push(Creature {
            id: alive_id,
            current_hp: 30,
            max_hp: 30,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(alive_id));
        // Push a dead enemy.
        let dead_id = NonZeroU32::new(8).expect("non-zero");
        state.creatures.push(Creature {
            id: dead_id,
            current_hp: 0,
            max_hp: 30,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(dead_id));
        // Only the live uid should ever come back.
        for _ in 0..16 {
            let picked = state.get_random_enemy().expect("alive enemy present");
            assert_eq!(picked, CreatureUid(alive_id));
        }
    }

    /// Helper: push an enemy Creature + register it in `enemy_uids`.
    #[cfg(test)]
    fn push_enemy(state: &mut CombatState, raw_id: u32, hp: i32) -> CreatureUid {
        let id = NonZeroU32::new(raw_id).expect("non-zero");
        state.creatures.push(Creature {
            id,
            current_hp: hp,
            max_hp: 30,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Nibbit"),
            ai: None,
        });
        let uid = CreatureUid(id);
        state.enemy_uids.push(uid);
        uid
    }

    /// Helper: attach a power of `kind`/`amount` to the creature `uid`.
    #[cfg(test)]
    fn give_power(state: &mut CombatState, uid: CreatureUid, kind: PowerKind, amount: i32) {
        let puid = PowerUid::new((state.powers.len() as u32) + 100).expect("non-zero");
        state.powers.push(Power {
            id: puid.0,
            kind,
            amount,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        let c = state
            .creatures
            .iter_mut()
            .find(|c| c.id == uid.0)
            .expect("creature");
        c.powers.push(puid);
    }

    /// Bead `sts2-rs-c8why`: a dead enemy with no retaining power is
    /// removed from the C# `Enemies` projection; a live enemy and a
    /// dead-but-`Illusion` corpse are retained. The canonical
    /// `enemy_uids` substrate is never mutated.
    #[test]
    fn active_enemy_uids_drops_dead_removable_keeps_retained_corpse() {
        let mut state = CombatState::default();
        // [aliveA, deadNormal, deadIllusion]
        let alive = push_enemy(&mut state, 11, 30);
        let dead_normal = push_enemy(&mut state, 12, 0);
        let dead_illusion = push_enemy(&mut state, 13, 0);
        give_power(&mut state, dead_illusion, PowerKind::Illusion, 1);

        // Predicate: normal dead -> remove; illusion dead -> retain;
        // alive -> never removed (caller gates on death, but the corpse
        // check returns false for it).
        assert!(state.should_remove_from_combat_on_death(dead_normal));
        assert!(!state.should_remove_from_combat_on_death(dead_illusion));
        assert!(state.is_removed_corpse(dead_normal));
        assert!(!state.is_removed_corpse(dead_illusion));
        assert!(!state.is_removed_corpse(alive));

        // Canonical substrate is untouched (NN slots stable).
        assert_eq!(state.enemy_uids.len(), 3);
        // Compacted C# view drops only the removable corpse.
        let active = state.active_enemy_uids();
        assert_eq!(active.as_slice(), &[alive, dead_illusion]);

        // Slot↔ordinal bridge: stable slot 0 -> ordinal 0; the removed
        // corpse at slot 1 has no ordinal; the retained corpse at slot 2
        // compacts to ordinal 1.
        assert_eq!(state.active_ordinal_of_enemy(0), Some(0));
        assert_eq!(state.active_ordinal_of_enemy(1), None);
        assert_eq!(state.active_ordinal_of_enemy(2), Some(1));
        assert_eq!(state.active_ordinal_of_enemy(3), None);
    }

    /// Bead `sts2-rs-c8why`: a power at `amount == 0` does not retain a
    /// corpse (mirrors the engine's zero-amount power purge and the
    /// bridge's `amount != 0` filter), and a *live* enemy carrying a
    /// retaining power is still not a removed corpse.
    #[test]
    fn active_enemy_uids_zero_amount_power_does_not_retain() {
        let mut state = CombatState::default();
        let dead_zero = push_enemy(&mut state, 21, 0);
        give_power(&mut state, dead_zero, PowerKind::Illusion, 0);
        assert!(state.should_remove_from_combat_on_death(dead_zero));
        assert!(state.is_removed_corpse(dead_zero));

        let live_illusion = push_enemy(&mut state, 22, 15);
        give_power(&mut state, live_illusion, PowerKind::Illusion, 1);
        assert!(!state.is_removed_corpse(live_illusion));

        assert_eq!(state.active_enemy_uids().as_slice(), &[live_illusion]);
    }

    /// `round_no` exposes `state.round` directly. The accessor is the
    /// canonical surface for relic listeners that need the current
    /// round payload (`MrStruggles`).
    #[test]
    fn round_no_exposes_round_field() {
        let mut state = CombatState::default();
        assert_eq!(state.round_no(), 0);
        state.round = 3;
        assert_eq!(state.round_no(), 3);
        state.round = u16::MAX;
        assert_eq!(state.round_no(), u16::MAX);
    }

    /// `is_elite` exposes the `is_elite` field directly. The accessor
    /// is the canonical surface for `after_combat_victory` listeners
    /// that gate on elite rooms (`SwordOfStone`). Bead `sts2-rs-jvk3`.
    #[test]
    fn is_elite_exposes_field() {
        let mut state = CombatState::default();
        assert!(!state.is_elite());
        state.is_elite = true;
        assert!(state.is_elite());
    }

    /// Bead `sts2-rs-c7ak` sentinel: `gain_energy` adds to
    /// `player.energy` for positive amounts and is a no-op for
    /// zero/negative amounts. Mirrors Python
    /// `CombatState.gain_energy` at `combat.py:1385-1388`.
    #[test]
    fn gain_energy_adds_to_player_energy_and_clamps_non_positive() {
        let mut state = CombatState::default();
        state.player.energy = 1;

        state.gain_energy(2);
        assert_eq!(state.player.energy, 3, "positive amount adds");

        state.gain_energy(0);
        assert_eq!(state.player.energy, 3, "zero is no-op");

        state.gain_energy(-5);
        assert_eq!(state.player.energy, 3, "negative is no-op");

        // Saturating behaviour — large amount clamps at i8::MAX.
        state.gain_energy(1000);
        assert_eq!(state.player.energy, i8::MAX, "saturates at i8::MAX");
    }

    /// Bead `sts2-rs-yb8m`: `gain_energy` suppresses all gain when player has
    /// `NoEnergyGain` power active, matching C# `NoEnergyGainPower.ModifyEnergyGain`.
    #[test]
    fn gain_energy_suppressed_by_no_energy_gain_power() {
        let mut state = CombatState::default();
        state.player.energy = 3;

        // Grant NoEnergyGain power to player.
        let power_id = NonZeroU32::new(1).unwrap();
        let power_uid = PowerUid(power_id);
        state.powers.push(Power {
            id: power_id,
            kind: PowerKind::NoEnergyGain,
            amount: 1,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(power_uid);

        // Try to gain energy — should be suppressed.
        state.gain_energy(4);
        assert_eq!(
            state.player.energy, 3,
            "NoEnergyGain suppresses energy gain"
        );

        // Remove the power and verify energy gain works again.
        state.player.powers.clear();
        state.powers.clear();
        state.gain_energy(2);
        assert_eq!(state.player.energy, 5, "gain works after power removed");
    }

    /// Bead `sts2-rs-c7ak` sentinel: `add_card_to_draw_pile` pushes
    /// the caller's [`Card`] POD into the arena with a fresh uid and
    /// inserts the uid at index 0 of the draw pile. Mirrors Python
    /// `combat.add_card_to_draw_pile(owner, "Dazed")` at
    /// `combat.py:1266-1269`.
    #[test]
    fn add_card_to_draw_pile_inserts_at_front_and_assigns_uid() {
        use crate::cards::{CardId, CardType, TargetType};

        let mut state = CombatState::default();
        // Pre-seed an existing card so we can prove insertion is at
        // the FRONT of `draw` (not the back) and the uid is
        // sequential.
        let existing_uid = CardUid::new(1).expect("non-zero");
        state.cards.push(Card {
            uid: existing_uid,
            card_id: CardId::StrikeIronclad,
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        });
        state.player.draw.push(existing_uid);

        let new_uid = state.add_card_to_draw_pile(Card {
            // `uid` is ignored — the accessor reassigns it.
            uid: CardUid::new(u32::MAX).expect("non-zero placeholder"),
            card_id: CardId::Dazed,
            cost: -1,
            card_type: CardType::Status,
            target_type: TargetType::None,
            upgraded: false,
            base_damage: 0,
            base_block: 0,
            ..Card::pod_defaults()
        });

        // Sequential uid (1-based, len+1 at time of push).
        assert_eq!(new_uid.get(), 2);
        // Inserted at front of draw pile (mirror Python `insert(0, …)`).
        assert_eq!(state.player.draw.len(), 2);
        assert_eq!(state.player.draw[0], new_uid);
        assert_eq!(state.player.draw[1], existing_uid);
        // Pushed onto the arena and carries the assigned uid.
        assert_eq!(state.cards.len(), 2);
        assert_eq!(state.cards[1].uid, new_uid);
        assert_eq!(state.cards[1].card_id, CardId::Dazed);
    }

    /// Bead `sts2-rs-6ynt.3.2` sentinel: drawing a [`CardId::Void`]
    /// drains 1 energy from the player (clamped at 0). Mirrors Python
    /// `CombatState._invoke_card_drawn` at
    /// `sts2-rl-agent@01c094d:sts2_env/core/combat.py:2322-2330`.
    #[test]
    fn drawing_void_drains_one_energy_and_clamps_to_zero() {
        use crate::cards::{CardId, CardType, TargetType};

        let mut state = CombatState::default();
        state.player.energy = 3;

        // Spawn a Void into the draw pile.
        let void_uid = state.add_card_to_draw_pile(Card {
            uid: CardUid::new(u32::MAX).expect("non-zero placeholder"),
            card_id: CardId::Void,
            cost: -1,
            card_type: CardType::Status,
            target_type: TargetType::None,
            upgraded: false,
            base_damage: 0,
            base_block: 0,
            ..Card::pod_defaults()
        });

        let moved = state.draw_into_hand(1);
        assert_eq!(moved, 1, "Void drawn into hand");
        assert_eq!(state.player.hand.last().copied(), Some(void_uid));
        assert_eq!(state.player.energy, 2, "draw drains 1 energy");

        // Drain again to verify clamping at 0.
        let void2 = state.add_card_to_draw_pile(Card {
            uid: CardUid::new(u32::MAX).expect("non-zero placeholder"),
            card_id: CardId::Void,
            cost: -1,
            card_type: CardType::Status,
            target_type: TargetType::None,
            upgraded: false,
            base_damage: 0,
            base_block: 0,
            ..Card::pod_defaults()
        });
        state.player.energy = 0;
        let _ = state.draw_into_hand(1);
        assert_eq!(state.player.energy, 0, "energy clamps to 0, never negative");
        assert!(state.player.hand.contains(&void2));
    }

    /// Bead `sts2-rs-q6bix` (F-S): `AutomationPower.AfterCardDrawn`
    /// decrements an internal `cardsLeft` (starts at 10) on every player
    /// card drawn; when it hits 0 the player gains `Amount` energy and
    /// `cardsLeft` resets to 10. Drawing 10 cards with Automation(1) active
    /// yields exactly +1 energy.
    #[test]
    fn automation_grants_energy_every_tenth_card_drawn() {
        use crate::cards::{CardId, CardType, TargetType};

        let mut state = CombatState::default();
        state.player.energy = 0;

        // Apply Automation(1) to the player (mirror+legacy via the legacy
        // helper is sufficient — the union read covers both surfaces).
        apply_player_power_legacy(&mut state, PowerKind::Automation, 1);

        // Seed exactly 10 cards into the draw pile.
        for _ in 0..10 {
            state.add_card_to_draw_pile(Card {
                uid: CardUid::new(u32::MAX).expect("non-zero placeholder"),
                card_id: CardId::StrikeIronclad,
                cost: 1,
                card_type: CardType::Attack,
                target_type: TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                ..Card::pod_defaults()
            });
        }

        let moved = state.draw_into_hand(10);
        assert_eq!(moved, 10, "all 10 cards drawn");
        assert_eq!(
            state.player.energy, 1,
            "Automation grants +1 energy on the 10th card drawn"
        );
    }

    /// Bead `sts2-rs-ryot` sentinel: `transform_card` rewrites the
    /// pile slot in place (preserving index), allocates a fresh
    /// arena uid for the replacement, and leaves the old uid orphan
    /// in the arena (no pile references it after the transform).
    /// Mirrors Python `combat.transform_card(old, new)` at
    /// `sts2_env/core/combat.py:1801-1820`.
    #[test]
    fn transform_card_replaces_slot_in_place() {
        use crate::cards::{CardId, CardType, TargetType};

        let mut state = CombatState::default();

        // Seed three cards in the draw pile so we can prove the
        // replacement lands at the SAME index (middle slot).
        let mk = |uid_raw: u32, id: CardId| Card {
            uid: CardUid::new(uid_raw).expect("non-zero"),
            card_id: id,
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        };
        let u1 = CardUid::new(1).expect("non-zero");
        let u2 = CardUid::new(2).expect("non-zero");
        let u3 = CardUid::new(3).expect("non-zero");
        state.cards.push(mk(1, CardId::StrikeIronclad));
        state.cards.push(mk(2, CardId::StrikeIronclad));
        state.cards.push(mk(3, CardId::StrikeIronclad));
        state.player.draw.push(u1);
        state.player.draw.push(u2);
        state.player.draw.push(u3);

        // Transform the middle card into a Bash. The replacement
        // Card's `uid` field is a placeholder that should be
        // overwritten by the accessor.
        let new_uid = state
            .transform_card(
                u2,
                Card {
                    uid: CardUid::new(u32::MAX).expect("placeholder"),
                    card_id: CardId::Bash,
                    cost: 2,
                    card_type: CardType::Attack,
                    target_type: TargetType::Enemy,
                    upgraded: false,
                    base_damage: 8,
                    base_block: 0,
                    ..Card::pod_defaults()
                },
            )
            .expect("transform succeeded");

        // Sequential uid: arena had 3 entries, so the new one is 4.
        assert_eq!(new_uid.get(), 4);
        // Slot resolves to the new spec (Bash) at the same index.
        assert_eq!(state.player.draw.len(), 3);
        assert_eq!(state.player.draw[0], u1);
        assert_eq!(state.player.draw[1], new_uid);
        assert_eq!(state.player.draw[2], u3);
        // Arena holds the new card with the assigned uid.
        assert_eq!(state.cards.len(), 4);
        assert_eq!(state.cards[3].uid, new_uid);
        assert_eq!(state.cards[3].card_id, CardId::Bash);
        // Old uid is orphan: not present in any pile.
        for pile in [
            crate::cards::PileKind::Hand,
            crate::cards::PileKind::Draw,
            crate::cards::PileKind::Discard,
            crate::cards::PileKind::Exhaust,
            crate::cards::PileKind::Play,
        ] {
            assert!(
                !crate::cards::pile_iter(&state.player, pile).any(|u| u == u2),
                "old uid still present in pile {pile:?}",
            );
        }

        // Missing uid returns None.
        let missing = CardUid::new(99).expect("non-zero");
        assert_eq!(
            state.transform_card(missing, mk(0xDEAD, CardId::Bash)),
            None
        );
    }

    /// Bead `sts2-rs-c7ak` sentinel: `active_attack` is `None` until
    /// `begin_attack` pushes a frame; `record_attack_hit` appends to
    /// the frame; `end_attack` clears it. Mirrors Python's
    /// `_attack_context_stack` push/pop in
    /// `sts2_env/core/combat.py:356-372`.
    #[test]
    fn active_attack_frame_round_trips_through_helpers() {
        let mut state = CombatState::default();
        assert!(state.active_attack().is_none(), "default has no frame");

        let target_uid = CreatureUid::new(1).expect("non-zero");
        state.begin_attack(None, Some(target_uid));
        let attack = state.active_attack().expect("frame pushed");
        assert_eq!(attack.target, Some(target_uid));
        assert!(attack.results.is_empty());

        // No-op while a frame exists but we record nothing yet —
        // confirm the borrow above didn't drop.
        state.record_attack_hit(target_uid, 3, 2);
        let attack = state.active_attack().expect("frame still present");
        assert_eq!(attack.results.len(), 1);
        assert_eq!(attack.results[0].target, target_uid);
        assert_eq!(attack.results[0].blocked, 3);
        assert_eq!(attack.results[0].hp_loss, 2);

        state.end_attack();
        assert!(state.active_attack().is_none(), "frame cleared");

        // record_attack_hit is a no-op with no frame.
        state.record_attack_hit(target_uid, 99, 99);
        assert!(state.active_attack().is_none(), "still no frame");
    }

    // ---- exhaust_from_draw_pile (bead sts2-rs-6ynt.6.8.1) ------------

    /// Test helper: push a Strike-Ironclad-shaped [`Card`] onto the
    /// draw pile and return its uid. Mirrors the construction in the
    /// `add_card_to_draw_pile` test above but on the back of `draw`
    /// (since `exhaust_from_draw_pile` pops from index 0 = top).
    fn push_draw_test_card(state: &mut CombatState) -> CardUid {
        use crate::cards::{CardId, CardType, TargetType};
        let raw = u32::try_from(state.cards.len() + 1).expect("fits");
        let uid = CardUid::new(raw).expect("non-zero");
        state.cards.push(Card {
            uid,
            card_id: CardId::StrikeIronclad,
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        });
        state.player.draw.push(uid);
        uid
    }

    #[test]
    fn exhaust_from_draw_pile_pops_top_count_into_exhaust() {
        let mut state = CombatState::default();
        // A living player: an exhaust doesn't begin once the combat is ending
        // (`exhaust_may_begin`), and a dead player is ending.
        state.player.current_hp = 80;
        let a = push_draw_test_card(&mut state);
        let b = push_draw_test_card(&mut state);
        let c = push_draw_test_card(&mut state);
        // draw is [a, b, c]; exhaust 2 should pop a then b in order.
        state.exhaust_from_draw_pile(2);
        assert_eq!(state.player.draw.as_slice(), &[c]);
        assert_eq!(state.player.exhaust.as_slice(), &[a, b]);
    }

    #[test]
    fn exhaust_from_draw_pile_reshuffles_discard_when_draw_empty() {
        let mut state = CombatState::default();
        // A living player: an exhaust doesn't begin once the combat is ending
        // (`exhaust_may_begin`), and a dead player is ending.
        state.player.current_hp = 80;
        let a = push_draw_test_card(&mut state);
        // Move `a` to discard so draw is empty.
        state.player.draw.clear();
        state.player.discard.push(a);

        state.exhaust_from_draw_pile(1);
        // Reshuffle moved `a` back to draw, then exhaust popped it.
        assert!(state.player.draw.is_empty());
        assert!(state.player.discard.is_empty());
        assert_eq!(state.player.exhaust.as_slice(), &[a]);
    }

    /// A refused draw-pile exhaust changes nothing: once the combat is ending,
    /// neither entry point reshuffles the discard pile into the empty draw
    /// pile nor draws from Shuffle (C# `CardCmd.Exhaust`, `CardCmd.cs:239`;
    /// the check comes before the reshuffle).
    #[test]
    fn a_refused_draw_pile_exhaust_moves_nothing_and_draws_no_shuffle() {
        for skip_innate_path in [false, true] {
            let mut state = CombatState {
                is_over: true, // the combat has ended
                ..Default::default()
            };
            state.player.current_hp = 80;
            let a = push_draw_test_card(&mut state);
            let b = push_draw_test_card(&mut state);
            state.player.draw.clear();
            state.player.discard.extend([a, b]);
            let shuffle_before = state.run_rng.shuffle.counter();
            if skip_innate_path {
                state.exhaust_top_from_draw_skipping_innate(false);
            } else {
                state.exhaust_from_draw_pile(1);
            }
            assert!(state.player.draw.is_empty());
            assert_eq!(state.player.discard.as_slice(), &[a, b]);
            assert!(state.player.exhaust.is_empty());
            assert_eq!(
                state.run_rng.shuffle.counter(),
                shuffle_before,
                "no Shuffle draw"
            );
        }
    }

    #[test]
    fn exhaust_from_draw_pile_empty_draw_and_discard_is_noop() {
        let mut state = CombatState::default();
        // count > 0 but both piles empty — should NOT panic or fire.
        state.exhaust_from_draw_pile(3);
        assert!(state.player.draw.is_empty());
        assert!(state.player.discard.is_empty());
        assert!(state.player.exhaust.is_empty());
    }

    #[test]
    fn exhaust_from_draw_pile_non_positive_count_is_noop() {
        let mut state = CombatState::default();
        let a = push_draw_test_card(&mut state);
        state.exhaust_from_draw_pile(0);
        state.exhaust_from_draw_pile(-5);
        assert_eq!(state.player.draw.as_slice(), &[a]);
        assert!(state.player.exhaust.is_empty());
    }

    /// Dual-bus dispatch: exhausting a card via this primitive fires
    /// BOTH the hooks bus (verified via `FeelNoPainPower`'s post-walk
    /// reaction → player gains block) AND the relic-effects bus
    /// (verified via a test-only registered listener that increments
    /// an atomic counter).
    #[test]
    fn exhaust_from_draw_pile_fires_both_buses() {
        use crate::card_effects::EffectResult;
        use crate::relic_effects::{
            register_after_card_exhausted, AfterCardExhaustedFn, CardEventCtx,
        };
        use crate::{PowerKind, RelicId};
        use std::sync::atomic::{AtomicU32, Ordering};

        // Test-only relic-bus listener. Static counter so multiple
        // dispatches accumulate.
        static RELIC_BUS_HITS: AtomicU32 = AtomicU32::new(0);
        fn h(_s: &mut CombatState, _ctx: &CardEventCtx) -> EffectResult {
            RELIC_BUS_HITS.fetch_add(1, Ordering::SeqCst);
            EffectResult::Resolved
        }
        // Re-register is idempotent (overwrites same RelicId slot);
        // safe even if another test in this binary already registered
        // a listener under the same RelicId.
        register_after_card_exhausted(RelicId::BlackBlood, h as AfterCardExhaustedFn);

        let mut state = CombatState::default();

        // A living player: a dead one makes the combat "ending", where C# fires no

        // `AfterCardExhausted` (`CardCmd.cs:239`).

        state.player.current_hp = 80;
        state.init_player_creature();
        // Give the player a FeelNoPain (amount=4): the hooks-bus
        // post-walk reaction in `dispatch_after_card_exhausted` will
        // call `gain_block_player(state, 4)` for each exhausted card.
        let power_id = NonZeroU32::new(1).expect("non-zero");
        state
            .powers
            .push(Power::new(power_id, PowerKind::FeelNoPain, 4));
        state.player.powers.push(PowerUid(power_id));

        let _a = push_draw_test_card(&mut state);
        let _b = push_draw_test_card(&mut state);

        let block_before = state.player.block;
        let hits_before = RELIC_BUS_HITS.load(Ordering::SeqCst);

        state.exhaust_from_draw_pile(2);

        // Hooks bus: FeelNoPain gained block twice (4 + 4 = 8).
        assert_eq!(
            state.player.block - block_before,
            8,
            "FeelNoPain block gain proves hooks bus fired per exhaust",
        );
        // Relic-effects bus: counter incremented at least once per
        // exhausted card. `>=` because other relics in the global
        // registry may also be listening.
        let hits = RELIC_BUS_HITS.load(Ordering::SeqCst) - hits_before;
        assert!(
            hits >= 2,
            "relic-effects bus fired at least once per exhausted card (got {hits})",
        );
    }

    // ------------------------------------------------------------------
    // Bead `sts2-rs-6ynt.6.11`: per-instance `counter` + `applier` on Power.
    // ------------------------------------------------------------------

    /// `Power::new` defaults `counter = 0` and `applier = None`. Existing
    /// call sites migrate through this constructor; powers that need a
    /// non-zero `counter` or a specific `applier` set the fields explicitly.
    #[test]
    fn power_new_defaults_counter_zero_and_applier_none() {
        let pid = NonZeroU32::new(7).expect("non-zero");
        let p = Power::new(pid, PowerKind::Strength, 3);
        assert_eq!(p.id, pid);
        assert_eq!(p.kind, PowerKind::Strength);
        assert_eq!(p.amount, 3);
        assert_eq!(p.counter, 0, "default counter is zero");
        assert!(p.applier.is_none(), "default applier is None");
    }

    /// Hashing includes `counter`: two Powers differing only in `counter`
    /// hash to distinct fingerprints. Mirrors the precedent set by
    /// `Relic::counter` (bead `sts2-rs-3nis`) where the per-instance scratch
    /// is canonical state.
    #[test]
    fn power_hash_distinguishes_counter() {
        let mut a = CombatState::default();
        let mut b = CombatState::default();
        let pid = NonZeroU32::new(1).expect("non-zero");
        let mut pa = Power::new(pid, PowerKind::Strength, 2);
        let mut pb = Power::new(pid, PowerKind::Strength, 2);
        pa.counter = 0;
        pb.counter = 5;
        a.powers.push(pa);
        b.powers.push(pb);
        a.player.powers.push(crate::PowerUid(pid));
        b.player.powers.push(crate::PowerUid(pid));
        assert_ne!(
            crate::hashing::state_hash(&a),
            crate::hashing::state_hash(&b),
            "counter must participate in canonical hash"
        );
    }

    /// Hashing includes `applier`: two Powers differing only in `applier`
    /// hash to distinct fingerprints (Grapple-style applier-gated listeners
    /// observe the field).
    #[test]
    fn power_hash_distinguishes_applier() {
        let mut a = CombatState::default();
        let mut b = CombatState::default();
        let pid = NonZeroU32::new(1).expect("non-zero");
        let mut pa = Power::new(pid, PowerKind::Vulnerable, 2);
        let mut pb = Power::new(pid, PowerKind::Vulnerable, 2);
        pa.applier = None;
        pb.applier = Some(CreatureUid::new(42).expect("non-zero"));
        a.powers.push(pa);
        b.powers.push(pb);
        a.player.powers.push(crate::PowerUid(pid));
        b.player.powers.push(crate::PowerUid(pid));
        assert_ne!(
            crate::hashing::state_hash(&a),
            crate::hashing::state_hash(&b),
            "applier must participate in canonical hash"
        );
    }

    /// Stack policy: when `apply_power_to_creature` stacks a second
    /// instance of the same `kind`, the existing instance's `counter`
    /// is preserved (per-instance scratch is not reset by re-application).
    #[test]
    fn power_stack_preserves_existing_counter() {
        use crate::card_effects::apply_power_to_creature;
        let mut state = CombatState::default();
        // Spawn a creature with full hp so the alive-guard passes.
        let cid = NonZeroU32::new(1).expect("non-zero");
        state.creatures.push(Creature {
            id: cid,
            current_hp: 10,
            max_hp: 10,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(cid));
        // Seed an existing Strength power with counter = 7.
        let pid = NonZeroU32::new(1).expect("non-zero");
        let mut existing = Power::new(pid, PowerKind::Strength, 2);
        existing.counter = 7;
        state.powers.push(existing);
        state.creatures[0].powers.push(crate::PowerUid(pid));

        // Re-apply 3 more stacks of Strength.
        apply_power_to_creature(&mut state, CreatureUid(cid), PowerKind::Strength, 3);

        // Amount sums; counter is preserved (NOT reset, NOT bumped).
        assert_eq!(state.powers.len(), 1, "no new Power record allocated");
        assert_eq!(state.powers[0].amount, 5, "amount sums on stack");
        assert_eq!(
            state.powers[0].counter, 7,
            "counter preserved on stack (per-instance scratch)"
        );
        assert!(
            state.powers[0].applier.is_none(),
            "applier untouched (this surface doesn't carry one)"
        );
    }

    // ---- move_card_to_hand / insert_card_into_draw_pile (bead sts2-rs-grne) -

    #[test]
    fn move_card_to_hand_lifts_resolving_card_out_of_play_pile() {
        let mut state = CombatState::default();
        let uid = push_draw_test_card(&mut state);
        // Simulate "currently resolving": card is in play, not draw.
        state.player.draw.clear();
        state.player.play.push(uid);

        state.move_card_to_hand(uid);

        assert!(state.player.play.is_empty(), "removed from play");
        assert_eq!(state.player.hand.as_slice(), &[uid], "appended to hand");
    }

    #[test]
    fn move_card_to_hand_routes_to_discard_when_hand_full() {
        let mut state = CombatState::default();
        // Fill hand to HAND_LIMIT with placeholder uids.
        for _ in 0..crate::cards::HAND_LIMIT {
            let placeholder = push_draw_test_card(&mut state);
            state.player.draw.pop();
            state.player.hand.push(placeholder);
        }
        let uid = push_draw_test_card(&mut state);
        state.player.draw.clear();
        state.player.play.push(uid);

        state.move_card_to_hand(uid);

        assert_eq!(
            state.player.hand.len(),
            crate::cards::HAND_LIMIT,
            "hand stays at HAND_LIMIT"
        );
        assert!(!state.player.hand.contains(&uid), "card not in hand");
        assert_eq!(
            state.player.discard.as_slice(),
            &[uid],
            "overflow routed to discard"
        );
    }

    #[test]
    fn move_card_to_hand_unknown_uid_is_noop() {
        let mut state = CombatState::default();
        let phantom = CardUid::new(999).expect("non-zero");
        state.move_card_to_hand(phantom);
        assert!(state.player.hand.is_empty());
        assert!(state.player.discard.is_empty());
    }

    #[test]
    fn insert_card_into_draw_pile_top_inserts_at_index_zero() {
        let mut state = CombatState::default();
        let existing = push_draw_test_card(&mut state); // draw = [existing]
        let resolving = push_draw_test_card(&mut state); // draw = [existing, resolving]
                                                         // Move `resolving` to play, mimicking mid-resolution.
        let pos = state
            .player
            .draw
            .iter()
            .position(|u| *u == resolving)
            .expect("present");
        state.player.draw.remove(pos);
        state.player.play.push(resolving);

        state.insert_card_into_draw_pile(resolving, false);

        assert!(state.player.play.is_empty(), "lifted out of play");
        assert_eq!(
            state.player.draw.as_slice(),
            &[resolving, existing],
            "inserted at index 0 (top)"
        );
        // Final-branch-review finding (bead `sts2-rs-ztl78`): a definite-
        // position insert at draw-top is the same "player watched it land
        // on top" class as Headbutt/CosmicIndifference/ShiningStrike/
        // Nostalgia, all of which pin — this currently-dead-in-content
        // branch must future-proof the same way.
        assert_eq!(
            state.player.pinned_top.first().copied(),
            Some(resolving),
            "random_position:false insert must pin the card to Top(0)"
        );
    }

    #[test]
    fn insert_card_into_draw_pile_random_position_consumes_rng() {
        let mut state = CombatState::default();
        let _a = push_draw_test_card(&mut state);
        let _b = push_draw_test_card(&mut state);
        let _c = push_draw_test_card(&mut state);
        let resolving = push_draw_test_card(&mut state);
        // draw = [a, b, c, resolving] — pull resolving into play.
        state.player.draw.pop();
        state.player.play.push(resolving);

        state.insert_card_into_draw_pile(resolving, true);

        assert!(state.player.play.is_empty(), "lifted out of play");
        assert_eq!(state.player.draw.len(), 4, "all four cards in draw");
        assert!(
            state.player.draw.contains(&resolving),
            "resolving card present"
        );
    }

    #[test]
    fn insert_card_into_draw_pile_unknown_uid_is_noop() {
        let mut state = CombatState::default();
        let phantom = CardUid::new(999).expect("non-zero");
        state.insert_card_into_draw_pile(phantom, false);
        assert!(state.player.draw.is_empty());
    }

    /// Bead `sts2-rs-3jo9` (rebased by `sts2-rs-5dp60` 2026-05-24):
    /// `spawn_enemy` applies per-species static on-spawn self-buffs
    /// for monsters whose C# class has an `AfterAddedToRoom` that
    /// calls `PowerCmd.Apply<Power>(base.Creature, ...)`.
    ///
    /// Original 3jo9 list was sourced from the Python pin and
    /// included VineShambler/SlitheringStrangler/SnappingJaxfruit/
    /// Myte — all four are Python-pin fabrications; their real C#
    /// classes either have no AfterAddedToRoom or apply the relevant
    /// power mid-move, not on spawn. 5dp60 removed those rows.
    /// A8 `ToughEnemies` scales these spawn powers (C# `AfterAddedToRoom`
    /// through `GetValueIfAscension(ToughEnemies, …)`): the A8 amount at
    /// ascension 8 and above, the base at 7.
    #[test]
    fn spawn_powers_scale_at_tough_enemies() {
        use crate::powers::PowerKind;
        let cases: &[(MonsterKind, PowerKind, i32, i32)] = &[
            (MonsterKind::Vantom, PowerKind::Slippery, 8, 9),
            (MonsterKind::SewerClam, PowerKind::Plating, 8, 9),
            (MonsterKind::FrogKnight, PowerKind::Plating, 15, 19),
            (MonsterKind::SlumberingBeetle, PowerKind::Plating, 15, 18),
            (MonsterKind::PhantasmalGardener, PowerKind::Skittish, 6, 7),
            (MonsterKind::TerrorEel, PowerKind::Shriek, 70, 75),
            (MonsterKind::LouseProgenitor, PowerKind::CurlUp, 14, 18),
        ];
        for &(species, kind, base, tough) in cases {
            for (ascension, want) in [(7u8, base), (8, tough), (10, tough)] {
                let mut state = CombatState {
                    ascension,
                    ..CombatState::default()
                };
                let uid = state.spawn_enemy(species);
                let creature = state.creatures.iter().find(|c| c.id == uid.0).unwrap();
                let stack: i32 = creature
                    .powers
                    .iter()
                    .filter_map(|puid| state.powers.get_by_id(puid.0))
                    .filter(|p| p.kind == kind)
                    .map(|p| p.amount)
                    .sum();
                assert_eq!(stack, want, "{species:?} {kind:?} at A{ascension}");
            }
        }
    }

    #[test]
    fn spawn_enemy_applies_on_spawn_self_buffs() {
        use crate::powers::PowerKind;

        let cases: &[(MonsterKind, PowerKind, i32)] = &[
            // C# PhrogParasite.cs:35 — InfestedPower(4). (Python pin
            // claimed CurlUp(10) — wrong.)
            (MonsterKind::PhrogParasite, PowerKind::Infested, 4),
            // C# v0.107.1 Vantom.cs:64,94 — SlipperyPower(SlipperyAmt) = 8 at
            // A0 (GetValueIfAscension(ToughEnemies, 9, 8)); `…wb8lk.11` nerf.
            (MonsterKind::Vantom, PowerKind::Slippery, 8),
            // C# BygoneEffigy.cs:35 — SlowPower(1) (sts2-rs-yqdlt).
            (MonsterKind::BygoneEffigy, PowerKind::Slow, 1),
            // C# Byrdonis.cs:35 — TerritorialPower(1).
            (MonsterKind::Byrdonis, PowerKind::Territorial, 1),
        ];
        for &(species, kind, amount) in cases {
            let mut state = CombatState::default();
            let uid = state.spawn_enemy(species);
            let creature = state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .expect("spawned");
            let stack: i32 = creature
                .powers
                .iter()
                .filter_map(|puid| state.powers.get_by_id(puid.0))
                .filter(|p| p.kind == kind)
                .map(|p| p.amount)
                .sum();
            assert_eq!(
                stack, amount,
                "{species:?} should spawn with {amount} stacks of {kind:?}; saw {stack}"
            );
        }
    }

    /// Aeonglass (v0.107.1 Act-3 boss) spawn powers: C#
    /// `Aeonglass.AfterAddedToRoom` applies `WitheringPresence(6)` to the
    /// player (opponent) and `Artifact(3)` to itself. Bead
    /// `sts2-rs-v0107-content-port-wb8lk.2`.
    #[test]
    fn spawn_aeonglass_applies_withering_presence_and_artifact() {
        use crate::powers::PowerKind;
        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        let player = state.init_player_creature();
        let uid = state.spawn_enemy(MonsterKind::Aeonglass);

        let self_stack = |state: &CombatState, who: CreatureUid, kind: PowerKind| -> i32 {
            state
                .creatures
                .iter()
                .find(|c| c.id == who.0)
                .map_or(0, |c| {
                    c.powers
                        .iter()
                        .filter_map(|p| state.powers.get_by_id(p.0))
                        .filter(|p| p.kind == kind)
                        .map(|p| p.amount)
                        .sum()
                })
        };
        assert_eq!(
            self_stack(&state, uid, PowerKind::Artifact),
            3,
            "Aeonglass spawns with Artifact(3) on itself"
        );
        // C# `Aeonglass.cs:78-84`: WitheringPresence is owner==target==the
        // monster (Instanced, logically targeting the player). It lives on
        // Aeonglass, NOT the player (parity: monster 2 powers, player 0).
        assert_eq!(
            self_stack(&state, uid, PowerKind::WitheringPresence),
            6,
            "Aeonglass spawns with WitheringPresence(6) on ITSELF (monster-owned)"
        );
        assert_eq!(
            self_stack(&state, player, PowerKind::WitheringPresence),
            0,
            "the player carries no WitheringPresence power"
        );
        // HP is fixed 512 (A0).
        let hp = state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .map_or(0, |c| c.max_hp);
        assert_eq!(hp, 512, "Aeonglass A0 HP is 512");
    }

    /// Ascension HP scaling reaches the `spawn_enemy` path via
    /// `CombatState::ascension` (FuzzyWurmCrawler vertical slice). C#
    /// `FuzzyWurmCrawler.cs` MinInitialHp 55→58 / MaxInitialHp 57→59 at
    /// ToughEnemies (ascension >= 8).
    #[test]
    fn spawn_enemy_scales_fuzzy_wurm_hp_with_ascension() {
        let spawn_hp = |ascension: u8| {
            let mut state = CombatState {
                ascension,
                ..CombatState::default()
            };
            let uid = state.spawn_enemy(MonsterKind::FuzzyWurmCrawler);
            state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .expect("spawned wurm")
                .max_hp
        };
        assert!((55..=57).contains(&spawn_hp(0)), "A0 HP in 55..=57");
        assert!(
            (55..=57).contains(&spawn_hp(7)),
            "A7 (below ToughEnemies) HP in 55..=57"
        );
        assert!(
            (58..=59).contains(&spawn_hp(8)),
            "A8 (ToughEnemies) HP in 58..=59"
        );
        assert!((58..=59).contains(&spawn_hp(10)), "A10 HP in 58..=59");
    }

    /// Bead `sts2-rs-ybtmu`: Rocket's C# `AfterAddedToRoom` applies
    /// `SurroundedPower(1)` to its opponents (the player) in addition to
    /// its own `BackAttackRight(1)` + `CrabRage(1)` self-buffs. The
    /// opponent-targeted power must land on the player creature.
    #[test]
    fn rocket_spawn_surrounds_the_player() {
        use crate::powers::PowerKind;

        let mut state = CombatState::default();
        state.player.current_hp = 70;
        state.player.max_hp = 70;
        // Player creature MUST exist before the spawn so the opponent-debuff
        // has a target.
        let player_uid = state.init_player_creature();
        let rocket_uid = state.spawn_enemy(MonsterKind::Rocket);

        let player_has = |kind: PowerKind| {
            state
                .creatures
                .iter()
                .find(|c| c.id == player_uid.0)
                .is_some_and(|c| {
                    c.powers
                        .iter()
                        .filter_map(|u| state.powers.get_by_id(u.0))
                        .any(|p| p.kind == kind)
                })
        };
        let rocket_has = |kind: PowerKind| {
            state
                .creatures
                .iter()
                .find(|c| c.id == rocket_uid.0)
                .is_some_and(|c| {
                    c.powers
                        .iter()
                        .filter_map(|u| state.powers.get_by_id(u.0))
                        .any(|p| p.kind == kind)
                })
        };

        assert!(
            player_has(PowerKind::Surrounded),
            "Rocket spawn applies Surrounded(1) to the player"
        );
        assert!(
            rocket_has(PowerKind::BackAttackRight),
            "Rocket keeps its own BackAttackRight self-buff"
        );
        assert!(
            rocket_has(PowerKind::CrabRage),
            "Rocket keeps its own CrabRage self-buff"
        );
        assert!(
            !rocket_has(PowerKind::Surrounded),
            "Surrounded is opponent-targeted — Rocket does NOT surround itself"
        );
    }

    /// Bead `sts2-rs-o1mwv` (C.2): EyeWithTeeth (Fogmog's ILLUSION
    /// spawn) must come into combat carrying both `Illusion(1)` and
    /// `Minion(1)`. C# `EyeWithTeeth.cs:30` `AfterAddedToRoom` applies
    /// `IllusionPower(1)`, whose `AfterApplied` (IllusionPower.cs:67)
    /// chains `MinionPower(1)`. Net: `powers == [Illusion(1),
    /// Minion(1)]` in that order. Drives the `monsters[0].powers.len`
    /// parity drift on FOGMOG trials.
    #[test]
    fn eye_with_teeth_spawns_with_illusion_and_minion() {
        use crate::powers::PowerKind;

        let mut state = CombatState::default();
        let uid = state.spawn_enemy(MonsterKind::EyeWithTeeth);
        let creature = state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .expect("spawned");
        let kinds: Vec<(PowerKind, i32)> = creature
            .powers
            .iter()
            .filter_map(|puid| state.powers.get_by_id(puid.0))
            .map(|p| (p.kind, p.amount))
            .collect();
        assert_eq!(
            kinds,
            vec![(PowerKind::Illusion, 1), (PowerKind::Minion, 1)],
            "EyeWithTeeth must spawn with [Illusion(1), Minion(1)] in C# AfterApplied-chain order",
        );
    }

    /// Bead `sts2-rs-ve0oq`: `request_card_choice` with `>255` options
    /// must truncate the options vec (not silently leave trailing
    /// unselectable entries behind). Builds a synthetic 300-option
    /// list, asserts the stored `PendingChoice.options.len()` is
    /// exactly `255`, and that `max_select` is clamped to `255`.
    /// The `debug_assert` inside the helper fires in test builds when
    /// the cap activates, so this test runs under `--release` to
    /// exercise the production path.
    ///
    /// Skipped on debug builds — the `debug_assert!` panics there to
    /// catch the regression in production callers. The release run
    /// confirms the truncation contract.
    #[test]
    #[cfg(not(debug_assertions))]
    fn request_card_choice_truncates_above_u8_max_options() {
        let mut state = CombatState::default();
        // Alive player — a dead one trips the C# IsOverOrEnding select
        // guard and the request would (correctly) no-op.
        state.player.current_hp = 80;
        let opts: Vec<ChoiceOption> = (0..300)
            .map(|_| ChoiceOption {
                card_id: crate::cards::CardId::StrikeIronclad,
                upgraded: false,
                card_uid: None,
                source_pile: None,
            })
            .collect();
        state.request_card_choice(ChoiceKind::CardSelect, opts, 1, 250, false);
        let pc = state.pending_choice.as_ref().expect("choice opened");
        assert_eq!(pc.options.len(), 255, "truncated to u8 index space");
        assert_eq!(
            pc.max_select, 250,
            "max_select preserved when ≤ truncated len"
        );
    }

    /// Bead `sts2-rs-ve0oq`: under-truncation boundary — exactly 255
    /// options is the largest size that fits in the u8 index space, so
    /// no truncation should occur. Runs on both debug + release.
    #[test]
    fn request_card_choice_keeps_exactly_255_options() {
        let mut state = CombatState::default();
        // Alive player — a dead one trips the C# IsOverOrEnding select
        // guard and the request would (correctly) no-op.
        state.player.current_hp = 80;
        let opts: Vec<ChoiceOption> = (0..255)
            .map(|_| ChoiceOption {
                card_id: crate::cards::CardId::StrikeIronclad,
                upgraded: false,
                card_uid: None,
                source_pile: None,
            })
            .collect();
        state.request_card_choice(ChoiceKind::CardSelect, opts, 1, 1, true);
        let pc = state.pending_choice.as_ref().expect("choice opened");
        assert_eq!(pc.options.len(), 255);
        assert_eq!(pc.max_select, 1);
        assert!(pc.can_skip);
    }

    /// C# guards every `CardSelectCmd` variant on combat end
    /// (`FromCombatPile` → `IsEnding`, CardSelectCmd.cs:383): a select
    /// requested after the killing blow never surfaces — a lethal
    /// NeowsFury goes straight to rewards with no discard-pick (shadow
    /// census v8 seed 1048). The engine mirrors the guard at
    /// `request_card_choice`.
    #[test]
    fn request_card_choice_noops_when_combat_is_ending() {
        use std::num::NonZeroU32;
        let mut state = CombatState::default();
        state.player.current_hp = 80;
        // One dead primary enemy → combat_is_ending() == true.
        let uid = NonZeroU32::new(7).expect("nz");
        state.creatures.push(Creature {
            id: uid,
            current_hp: 0,
            max_hp: 20,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(crate::monsters::MonsterKind::Nibbit),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Nibbit"),
            ai: None,
        });
        state.enemy_uids.push(CreatureUid(uid));
        let opts = vec![ChoiceOption {
            card_id: crate::cards::CardId::StrikeIronclad,
            upgraded: false,
            card_uid: None,
            source_pile: None,
        }];
        state.request_card_choice(ChoiceKind::CardSelect, opts, 0, 1, true);
        assert!(
            state.pending_choice.is_none(),
            "no select may surface once combat is ending (C# IsEnding guard)"
        );
    }

    /// Bead `sts2-rs-ldd1c`: enemy `Slippery` stacks raise
    /// `time_to_lethal_estimate` because each stack caps one player
    /// hit at 1 damage. The estimate must exceed the no-Slippery
    /// baseline whenever the player's average per-hit damage > 1.
    #[test]
    fn time_to_lethal_estimate_penalizes_enemy_slippery() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        let mk_strike = |uid_raw: u32| Card {
            uid: CardUid::new(uid_raw).expect("non-zero"),
            card_id: CardId::StrikeIronclad,
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        };
        let build = |slippery_stacks: i32| -> CombatState {
            let mut state = CombatState::default();
            let enemy_uid = state.spawn_enemy(MonsterKind::Mawler);
            let enemy = state
                .creatures
                .iter_mut()
                .find(|c| c.id == enemy_uid.0)
                .expect("spawned");
            enemy.current_hp = 60;
            enemy.max_hp = 60;
            if slippery_stacks > 0 {
                let puid = PowerUid::new(101).expect("non-zero");
                state.powers.push(Power {
                    id: puid.0,
                    kind: PowerKind::Slippery,
                    amount: slippery_stacks,
                    counter: 0,
                    applier: None,
                    skip_next_tick: false,
                });
                state
                    .creatures
                    .iter_mut()
                    .find(|c| c.id == enemy_uid.0)
                    .expect("spawned")
                    .powers
                    .push(puid);
            }
            for i in 1..=3u32 {
                state.cards.push(mk_strike(i));
                state.player.hand.push(CardUid::new(i).expect("non-zero"));
            }
            state
        };
        let baseline = time_to_lethal_estimate(&build(0));
        let slippery = time_to_lethal_estimate(&build(2));
        assert!(
            slippery > baseline,
            "Slippery must raise time_to_lethal: baseline={baseline}, slippery={slippery}"
        );
        // 3 Strikes × 6 = 18 raw potential; 2 Slippery × (6-1) = 10 loss → 8.
        assert!(
            (slippery - 60.0_f32 / 8.0).abs() < 0.01,
            "expected t2l ≈ 7.5 with 2 Slippery, got {slippery}"
        );
        assert!(
            (baseline - 60.0_f32 / 18.0).abs() < 0.01,
            "expected baseline t2l ≈ 3.33, got {baseline}"
        );
    }

    /// Bead `sts2-rs-ldd1c`: when `slippery_total >= attack_count`,
    /// the Slippery cap saturates at `attack_count` hits — extra
    /// stacks beyond the hand's attack count don't grow the penalty
    /// further (effective potential floors at `attack_count` damage).
    #[test]
    fn time_to_lethal_estimate_slippery_saturates_at_attack_count() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        let mk_strike = |uid_raw: u32| Card {
            uid: CardUid::new(uid_raw).expect("non-zero"),
            card_id: CardId::StrikeIronclad,
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        };
        let build_with = |stacks: i32| -> CombatState {
            let mut state = CombatState::default();
            let enemy_uid = state.spawn_enemy(MonsterKind::Mawler);
            let enemy = state
                .creatures
                .iter_mut()
                .find(|c| c.id == enemy_uid.0)
                .expect("spawned");
            enemy.current_hp = 60;
            enemy.max_hp = 60;
            let puid = PowerUid::new(101).expect("non-zero");
            state.powers.push(Power {
                id: puid.0,
                kind: PowerKind::Slippery,
                amount: stacks,
                counter: 0,
                applier: None,
                skip_next_tick: false,
            });
            state
                .creatures
                .iter_mut()
                .find(|c| c.id == enemy_uid.0)
                .expect("spawned")
                .powers
                .push(puid);
            for i in 1..=3u32 {
                state.cards.push(mk_strike(i));
                state.player.hand.push(CardUid::new(i).expect("non-zero"));
            }
            state
        };
        let cap_at_three = time_to_lethal_estimate(&build_with(3));
        let over_cap = time_to_lethal_estimate(&build_with(9));
        assert!(
            (cap_at_three - over_cap).abs() < 0.01,
            "Slippery should saturate at attack_count; got cap={cap_at_three}, over={over_cap}"
        );
    }

    /// Bead `sts2-rs-3jo9`: monsters without on-spawn buffs (the
    /// default — every species whose AI carries its buffs through
    /// `Buff` intent moves at runtime) enter combat with an empty
    /// power list.
    #[test]
    fn spawn_enemy_no_buff_species_pushes_clean_creature() {
        let mut state = CombatState::default();
        let uid = state.spawn_enemy(MonsterKind::Mawler);
        let creature = state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .expect("spawned");
        assert!(
            creature.powers.is_empty(),
            "Mawler has no on-spawn buffs; saw {:?}",
            creature.powers
        );
    }

    /// Bead `sts2-rs-caz92`: CubexConstruct gains 13 starting block
    /// at spawn (C# `CubexConstruct.cs:84` `GainBlock(creature, 13)`)
    /// alongside Artifact(1).
    #[test]
    fn spawn_enemy_cubex_construct_gets_thirteen_starting_block() {
        let mut state = CombatState::default();
        let uid = state.spawn_enemy(MonsterKind::CubexConstruct);
        let creature = state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .expect("spawned");
        assert_eq!(
            creature.block, 13,
            "CubexConstruct starts with 13 block per AfterAddedToRoom"
        );
        assert!(
            !creature.powers.is_empty(),
            "CubexConstruct also has Artifact(1) buff",
        );
    }

    /// Bead `sts2-rs-caz92`: non-Cubex species default to 0 starting
    /// block via the `on_spawn_block` surface (only CubexConstruct
    /// returns non-zero at the current pin).
    #[test]
    fn spawn_enemy_default_species_starts_with_zero_block() {
        let mut state = CombatState::default();
        let uid = state.spawn_enemy(MonsterKind::CorpseSlug);
        let creature = state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .expect("spawned");
        assert_eq!(creature.block, 0);
    }

    /// D1 (`docs/plans/2026-06-07-mcts-single-threaded-perf-plan.md`):
    /// the `PowerStore` O(1) `uid -> slot` index must agree with a linear
    /// scan across push, in-place mutation, compacting `retain` (which
    /// shifts slots), and `max+1` id *reuse* after removal — the exact
    /// aliasing footgun the plan flagged.
    #[test]
    fn power_store_index_tracks_push_retain_and_id_reuse() {
        let pk = PowerKind::Strength;
        let nz = |n: u32| NonZeroU32::new(n).expect("non-zero");
        let agrees = |store: &PowerStore| {
            // O(1) index must resolve every live power to its own slot, and
            // every absent id to None.
            for (slot, p) in store.iter().enumerate() {
                assert_eq!(
                    store.get_by_id(p.id).map(core::ptr::from_ref),
                    Some(core::ptr::from_ref(&store[slot])),
                    "id {} mis-resolved",
                    p.id
                );
            }
        };

        let mut store = PowerStore::new();
        // ids 1,2,3 (mirrors max+1 allocation).
        for id in 1..=3 {
            store.push(Power::new(nz(id), pk, i32::try_from(id).unwrap()));
        }
        agrees(&store);
        assert_eq!(store.len(), 3);
        assert_eq!(store.get_by_id(nz(2)).map(|p| p.amount), Some(2));
        assert!(store.get_by_id(nz(9)).is_none());

        // In-place mutation keeps the slot (id unchanged) → index still valid.
        store.get_mut_by_id(nz(2)).expect("present").amount = 42;
        agrees(&store);
        assert_eq!(store.get_by_id(nz(2)).map(|p| p.amount), Some(42));

        // Remove the middle power: compaction shifts slot 2 -> slot 1.
        store.retain(|p| p.id != nz(2));
        agrees(&store);
        assert_eq!(store.len(), 2);
        assert!(store.get_by_id(nz(2)).is_none());
        assert_eq!(store.get_by_id(nz(3)).map(|p| p.amount), Some(3));

        // Re-push id 2 (id reuse): index must point at the new slot, not the
        // stale pre-removal one.
        store.push(Power::new(nz(2), pk, 7));
        agrees(&store);
        assert_eq!(store.get_by_id(nz(2)).map(|p| p.amount), Some(7));

        // `From<Vec>` rebuilds the index from scratch and matches.
        let rebuilt = PowerStore::from(store.as_slice().to_vec());
        agrees(&rebuilt);
        assert_eq!(rebuilt, store, "From<Vec> preserves arena equality");

        // Eq ignores the (derived) index: a freshly built store equals an
        // engine-mutated one with the same arena.
        store.clear();
        assert_eq!(store, PowerStore::new());
    }

    /// RP85 Necrobinder `Melancholy.AfterDeath`: each creature death reduces
    /// every in-combat-pile Melancholy's cost by 1 (clamped at 0); a Melancholy
    /// outside the combat piles (master deck) is unaffected. Mirrors C#
    /// `Melancholy.cs::AfterDeath` (`EnergyCost.AddThisCombat(-1)`, gated on
    /// `Pile.IsCombatPile`).
    #[test]
    fn melancholy_cost_drops_per_death_only_in_combat_piles() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        let mk = |raw: u32| Card {
            uid: CardUid::new(raw).expect("non-zero"),
            card_id: CardId::Melancholy,
            cost: 3,
            card_type: CardType::Skill,
            target_type: TargetType::Self_,
            ..Card::pod_defaults()
        };
        let mut state = CombatState::default();
        // uid 1: in hand (a combat pile) — should drop.
        // uid 2: in discard (a combat pile) — should drop.
        // uid 3: NOT in any pile (stand-in for the master deck) — unaffected.
        state.cards.push(mk(1));
        state.cards.push(mk(2));
        state.cards.push(mk(3));
        state.player.hand.push(CardUid::new(1).expect("non-zero"));
        state
            .player
            .discard
            .push(CardUid::new(2).expect("non-zero"));

        let cost = |st: &CombatState, raw: u32| {
            st.cards
                .iter()
                .find(|c| c.uid == CardUid::new(raw).expect("non-zero"))
                .expect("card present")
                .cost
        };

        // N = 3 creature deaths: each in-pile Melancholy drops by N (3 → 0),
        // clamped at 0; the deck copy stays at 3.
        for _ in 0..3 {
            state.reduce_melancholy_cost_on_death();
        }
        assert_eq!(cost(&state, 1), 0, "hand Melancholy: 3 deaths → cost 0");
        assert_eq!(cost(&state, 2), 0, "discard Melancholy: 3 deaths → cost 0");
        assert_eq!(
            cost(&state, 3),
            3,
            "deck-only Melancholy is NOT in a combat pile → unaffected"
        );

        // A further death keeps the in-pile copies clamped at 0 (no underflow).
        state.reduce_melancholy_cost_on_death();
        assert_eq!(cost(&state, 1), 0, "clamped at 0 on extra death");
        assert_eq!(cost(&state, 2), 0, "clamped at 0 on extra death");
    }

    /// Mutant-coverage (2026-06-20 sweep): the test above uses a SYMMETRIC
    /// two-in-pile layout, so the `c.uid == uid` find-flip (`==`→`!=`)
    /// passes by coincidence (it just swaps which of the two pile cards is
    /// decremented, netting the same totals). This asymmetric layout —
    /// exactly ONE Melancholy in a pile, a second Melancholy parked first in
    /// `state.cards` but NOT in any pile — pins the find so a `!=` flip
    /// decrements the wrong (deck-only) card and fails.
    #[test]
    fn melancholy_cost_drop_targets_the_piled_card_not_a_decoy() {
        use crate::cards::{Card, CardId, CardType, TargetType};
        let mk = |raw: u32| Card {
            uid: CardUid::new(raw).expect("non-zero"),
            card_id: CardId::Melancholy,
            cost: 3,
            card_type: CardType::Skill,
            target_type: TargetType::Self_,
            ..Card::pod_defaults()
        };
        let mut state = CombatState::default();
        // uid 1: a Melancholy parked first in the arena but NOT in any combat
        //        pile (deck-only decoy) — must stay at 3.
        // uid 2: the only Melancholy in a combat pile (hand) — must drop.
        state.cards.push(mk(1));
        state.cards.push(mk(2));
        state.player.hand.push(CardUid::new(2).expect("non-zero"));

        let cost = |st: &CombatState, raw: u32| {
            st.cards
                .iter()
                .find(|c| c.uid == CardUid::new(raw).expect("non-zero"))
                .expect("card present")
                .cost
        };

        state.reduce_melancholy_cost_on_death();
        assert_eq!(cost(&state, 2), 2, "the piled Melancholy must drop 3→2");
        assert_eq!(cost(&state, 1), 3, "the deck-only decoy must be untouched");
    }
}
