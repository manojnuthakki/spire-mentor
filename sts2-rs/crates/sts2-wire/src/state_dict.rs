//! Wire-format parse + `CombatState` reconstruction, with no `PyO3` in sight.
//!
//! **Bead:** `sts2-rs-92sk` (hint-T1) / `sts2-rs-osyf` (T2) — moved out of
//! `sts2-py` when the parse layer was generalised over [`WireValue`].
//!
//! This module implements the *fail-loud* schema parse for the JSON dict the
//! hint adapter (`sts2_hint/adapter.py`) and the SpireMentor advisor lane feed
//! in, plus the translation of the resulting [`ParsedState`] into a fully
//! initialised `CombatState`. Per the integration plan
//! (`docs/plans/2026-05-14-integrate-sts2-rs-mcts-hint-system-into-projects-sts2-sts2-c.md`,
//! §Extended wire schema), every unknown id, unmodelled power, or invariant
//! violation produces an error carrying the offending JSON path
//! (`player.relics[3]`, `deck.hand[1]: STRIKE_X`, `encounter[0].kind:
//! GHIBBET_MAW`, …).
//!
//! Two halves, unchanged in substance from when they lived in `sts2-py`:
//!
//! * **T1** — [`parse_state_dict`] validates the dict into the [`ParsedState`]
//!   value-object. No `CombatState` is built.
//! * **T2** — [`state_from_dict_impl`] translates [`ParsedState`] into a
//!   fully-initialised `CombatState` (player Creature wiring, monster
//!   Creatures, deck piles, powers, relics, intent history, …).
//!
//! What *did* change: the parse reads through [`WireValue`] rather than
//! `PyDict` directly, so the same code serves a Python dict and a
//! `serde_json::Value` with no interpreter in the process. `sts2-py` supplies
//! the `PyO3` implementation and maps [`WireError`] back onto its two Python
//! exception types; nothing about the Python-facing behaviour moved.
//!
//! `registry_names_for()` is colocated here because it owns the
//! canonical-name source-of-truth tables used by [`parse_state_dict`] to
//! recognise valid ids — keeping the two in one file means a new
//! `MonsterKind` variant added without updating the local registry mirror
//! fails both the recognition path and the introspection in the same
//! `cargo test` cycle, not just one of them.

use std::collections::BTreeSet;

use std::num::NonZeroU32;

use smallvec::SmallVec;

use sts2_content::monster_dispatch::{build_ai, MonsterSpawnArgs, SpawnPosition};
use sts2_content::names::resolve_card_name;
use sts2_core::cards::{Card, CardId, EnchantmentId, VarKey};
use sts2_core::hooks::DamageMul;
use sts2_core::{
    CardUid, CharacterId, CombatPhase, CombatState as CoreCombatState, Creature, CreatureUid,
    Intent, IntentHistory, MonsterAi, MonsterId, MonsterKind, Orb, OrbKind, PotionKind, Power,
    PowerKind, PowerUid, Relic, RelicKind, Side, DARK_ORB_BASE_EVOKE,
};

use crate::error::{schema_err, unsupported, WireResult};
use crate::value::WireValue;

// ---------------------------------------------------------------------------
// Wire schema constants
// ---------------------------------------------------------------------------

/// Current wire-format version. Bumped on any non-additive schema
/// change. Adapters check this against their compile-time pin so older
/// sts2-cli builds detect a `sts2_rs` that needs more fields without
/// silently producing garbage.
///
/// See plan §Architectural decisions table.
pub const WIRE_VERSION: u32 = 1;

/// Top-level keys that MUST be present in the wire dict. Missing-key
/// errors point at this list directly.
pub const REQUIRED_TOP_KEYS: &[&str] = &[
    "wire_version",
    "phase",
    "round",
    "side",
    "is_elite",
    "player",
    "deck",
    "encounter",
    "turn",
    "combat_id",
];

/// Required keys on the `player` sub-object.
pub const REQUIRED_PLAYER_KEYS: &[&str] = &[
    "class",
    "hp",
    "max_hp",
    "block",
    "energy",
    "max_energy",
    "energy_regen_per_turn",
    "relics",
    "powers",
    "potions",
];

/// Required keys on a `deck` sub-object. `master_deck` is documented
/// as `... | None` in the wire schema; we allow it missing (treated
/// the same as `None`).
pub const REQUIRED_DECK_KEYS: &[&str] = &["draw_pile", "discard_pile", "hand", "exhaust_pile"];

/// Required keys on each `CardSpec` element.
pub const REQUIRED_CARDSPEC_KEYS: &[&str] = &["id", "stable_idx", "kind", "upgraded"];

/// Required keys on each `encounter` element.
pub const REQUIRED_ENCOUNTER_KEYS: &[&str] = &[
    "kind",
    "uid",
    "hp",
    "max_hp",
    "block",
    "powers",
    "intents",
    "intent_history",
    "is_dead",
    "is_escaped",
];

/// Recognised `phase` strings. Mirrors the `CombatPhase` discriminant
/// at the time of the bead; new variants must be added here when the
/// adapter wants to round-trip them. Unknown phases fail loud.
pub const VALID_PHASES: &[&str] = &[
    "Setup",
    "PlayerDraw",
    "PlayerPlayPhase",
    "EnemyPhase",
    "EndOfTurn",
    "Victory",
    "Defeat",
];

pub const VALID_SIDES: &[&str] = &["Player", "Enemy"];

pub const VALID_CLASSES: &[&str] = &["ironclad", "silent", "defect", "necrobinder", "regent"];

pub const VALID_CARD_KINDS: &[&str] = &["card", "status", "curse"];

pub const VALID_CARD_FLAGS: &[&str] = &[
    "free_once",
    "free_turn",
    "retain",
    "ethereal",
    "unplayable",
    "innate",
    // The keyword `Sly` set (intrinsic, or granted for the rest of the
    // combat by MasterPlanner's `AddKeyword`), and the two SINGLE-TURN
    // grants that C# keeps in private bools cleared by `EndOfTurnCleanup`
    // (GH #370). The producers emit `ShouldRetainThisTurn`/`IsSlyThisTurn`,
    // which OR the keyword in; the adapter subtracts it so `retain_turn` /
    // `sly_turn` here mean strictly "transient grant, keyword absent".
    "sly",
    "retain_turn",
    "sly_turn",
];

// ---------------------------------------------------------------------------
// Registry mirrors
// ---------------------------------------------------------------------------
//
// The core enums expose `python_name()` (forward) but not `from_python_name`
// (backward). The wire format is keyed by `python_name`, so we need the
// reverse map. We could iterate `CardId::all()` etc. inside every call,
// but for fail-loud rejection we only need set membership — a static
// sorted list is enough and lets `list_registry_names()` return a
// pre-sorted view in O(N) without allocating.

/// All `MonsterKind` variants currently wired into sts2-core. Aliased
/// to the canonical [`MonsterKind::ALL_KNOWN`] const in sts2-core
/// (bead `sts2-rs-23ky`) so the cross-crate scenario-resolver
/// coverage guard and this file's `monster_kinds_table_matches_python_names`
/// invariant share one source of truth.
const MONSTER_KINDS: &[MonsterKind] = MonsterKind::ALL_KNOWN;

/// Every `RelicKind` variant, aliased to the macro-derived
/// [`RelicKind::ALL`] roster (soft-drop census wave 3, 2026-07-17).
///
/// This mirror was previously a hand-maintained copy of the enum's
/// variant list and drifted behind it THREE times (commits
/// `8d81f85f5`, `9d0cae1c4`, and the wave-3 census tail): relics with
/// fully-wired engine listeners were missing here, so
/// `list_registry_names("relic")` under-reported the engine and the
/// pycli hint adapter soft-dropped those relics out of every MCTS
/// search state. `RelicKind::ALL` is generated by
/// `declare_relic_kinds!` in `sts2-core::relics` from the enum
/// declaration itself, so a new variant can never be missing from this
/// registry again. There is deliberately NO opt-out: combat-inert
/// relics are listed as no-op markers (established pattern since bead
/// `sts2-rs-cia9`) — the registry answers "does the engine know this
/// relic id", not "does it have combat behavior".
const RELIC_KINDS: &[RelicKind] = RelicKind::ALL;

const ORB_KINDS: &[OrbKind] = &[
    OrbKind::Empty,
    OrbKind::Lightning,
    OrbKind::Frost,
    OrbKind::Dark,
    OrbKind::Plasma,
    OrbKind::Glass,
];

const fn orb_python_name(o: OrbKind) -> &'static str {
    match o {
        OrbKind::Empty => "EMPTY",
        OrbKind::Lightning => "LIGHTNING",
        OrbKind::Frost => "FROST",
        OrbKind::Dark => "DARK",
        OrbKind::Plasma => "PLASMA",
        OrbKind::Glass => "GLASS",
    }
}

// ---------------------------------------------------------------------------
// Parsed value-objects
// ---------------------------------------------------------------------------

/// Top-level parsed wire dict. T2 consumes this to construct a
/// `CoreCombatState`. The shape mirrors the wire schema; lossy fields
/// (`active_attack`, `pending_choice`, `pending_play`) are not modelled
/// here because at hint-time they're documented to be `None` — any
/// non-`None` value fails loud at parse-time.
#[derive(Debug, Clone)]
pub struct ParsedState {
    pub wire_version: u32,
    pub phase: String,
    pub round: i32,
    pub side: String,
    pub is_elite: bool,
    pub player: ParsedPlayer,
    pub deck: ParsedDeck,
    pub encounter: Vec<ParsedEnemy>,
    pub turn: i32,
    pub combat_id: i64,
    /// Optional RNG seed. The wire schema permits a `{"rng": {"seed":
    /// int, "ascension": int}}` block; both fields are optional in this
    /// bead's surface — T2 owns translating them into the state.
    pub rng_seed: Option<u64>,
    /// The RUN's master seed (`$.rng.run_seed`), once-hashed exactly as C#
    /// `RunRngSet.Seed` is — the value [`sts2_core::run_rng_set::RunRngSet::new`]
    /// takes.
    ///
    /// Distinct from [`Self::rng_seed`], which is the *search* seed and has
    /// nothing to do with the game's dice. Without this the reconstruction's
    /// `monster_ai` and `shuffle` streams are unrelated to the world's, so
    /// every enemy intent and every reshuffle it predicts is a guess — and a
    /// guess that misses is not evidence of anything, because the value was
    /// never knowable from the state.
    pub run_seed: Option<i32>,
    /// Per-stream read counts (`$.rng.counters`), keyed by the same wire
    /// names `state_to_dict` emits. Applied on top of the streams derived
    /// from [`Self::run_seed`], which puts the reconstruction on the exact
    /// die roll the world is about to make. Ignored without a `run_seed` —
    /// a cursor into an unknown stream means nothing.
    pub rng_counters: Vec<(String, u32)>,
    pub ascension: u8,
    /// Aggregate of [`ParsedEnemy::intent_damage_known`] across the
    /// encounter (`AND`). `true` only when every enemy's current intent
    /// has trustworthy damage. Surfaced through `PyCombatState` so
    /// `mcts_recommend` can flag degraded hint quality. Bead
    /// `sts2-rs-bpt4`.
    pub intent_damage_known: bool,
    /// Which convention this dict's Attack telegraphs use.
    ///
    /// The two producers disagree, and the difference is load-bearing:
    ///
    /// * The C# DLL / live game / `sts2_hint.adapter` producers telegraph
    ///   the **UI-effective** number — Strength and Vulnerable already
    ///   folded in — so the parser has to invert them
    ///   ([`unbake_wire_intent`]) or the engine double-counts every
    ///   modifier when it performs the move.
    /// * [`state_to_dict`] emits the engine's own **base** amount, which
    ///   `perform_intent` will push through the damage pipeline itself.
    ///   Unbaking those subtracts modifiers that were never added.
    ///
    /// Absent ⇒ `Effective`, so every pre-existing producer keeps its
    /// behavior; `state_to_dict` stamps `Base` on the way out. Found by
    /// the Phase-1 mirror oracle, which round-trips `state_to_dict`
    /// output and saw a 12-damage Strength-buffed telegraph come back
    /// as 10.
    pub intent_amounts: IntentAmounts,
}

/// See [`ParsedState::intent_amounts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IntentAmounts {
    /// Modifiers already folded in — invert before ingesting.
    #[default]
    Effective,
    /// The engine's own pre-modifier amount — ingest verbatim.
    Base,
}

#[derive(Debug, Clone)]
pub struct ParsedPlayer {
    pub class: String,
    pub hp: i32,
    pub max_hp: i32,
    pub block: i32,
    pub energy: i32,
    pub max_energy: i32,
    pub energy_regen_per_turn: i32,
    pub relics: Vec<ParsedRelic>,
    pub powers: Vec<ParsedPower>,
    pub potions: Vec<Option<PotionKind>>,
    pub orbs: Option<Vec<ParsedOrb>>,
    pub orb_slots: Option<u32>,
    pub stars: Option<i32>,
    pub osty: Option<ParsedOsty>,
    pub cards_played_this_turn: Option<[u32; 5]>,
    /// Run gold. Optional — older producers omit it. Gates in-combat relic
    /// hooks (SealOfGold spends 5 gold for +1 energy at every player turn
    /// start); rebuilt without it the hook could never fire — 90 of the 178
    /// `$.player.energy` divergences on the v27wr sweep (2026-08-01).
    pub gold: Option<i32>,
    /// ChainsOfBinding's once-per-turn gate (C#
    /// `ChainsOfBindingPower.Data.boundCardPlayed`): `true` when a
    /// Bound card was already played this turn, so further Bound plays
    /// are vetoed until turn end. Optional wire field; absent → false.
    pub bound_card_played_this_turn: bool,
}

#[derive(Debug, Clone)]
pub struct ParsedRelic {
    pub kind: RelicKind,
    pub counter: Option<i32>,
    /// C# `RelicModel.IsWax` — ToyBox marked this relic meltable
    /// (`ToyBox.cs:93`). Absent on the wire → `false`, so pre-melt
    /// producers keep round-tripping unchanged.
    pub is_wax: bool,
    /// C# `RelicModel.IsMelted` — ToyBox has melted it. The relic is still
    /// in the inventory but C# drops it from every hook-listener list
    /// (`CombatState.cs:130-137`), so the rebuilt state must keep its
    /// hooks silent. Absent on the wire → `false`.
    pub is_melted: bool,
}

#[derive(Debug, Clone)]
pub struct ParsedPower {
    pub kind: PowerKind,
    pub amount: i32,
    /// Opaque `extra` payload. We don't structurally inspect it at T1
    /// — T2 routes it through the relevant power's apply hook.
    pub has_extra: bool,
    /// C# `Power.DisplayAmount` when the wire carries it and it differs
    /// from `amount`. For countdown-style powers (WitheringPresence)
    /// `amount` is the static cycle length while `DisplayAmount` is the
    /// live cards-left counter — dropping it reset every reconstruction
    /// to "a full cycle away" (2026-07-17 Aeonglass audit).
    pub display: Option<i32>,
    /// The power's PER-INSTANCE scratch ([`sts2_core::Power::counter`]),
    /// when the producer carries it verbatim.
    ///
    /// This is where the engine keeps accumulator state that `amount` does
    /// not describe: CrimsonMantle's self-damage total, Slow's cards-played
    /// tally, Surrounded's facing. Absent it, every reconstruction restarted
    /// those at zero — measured on the 60-run realdll_v5 sweep, a
    /// reconstructed CrimsonMantle dealt NO self-damage, leaving the mirror
    /// exactly 1 HP high after every `end_turn` (54 single-field
    /// `$.player.hp +1` divergences, all with CrimsonMantle on the player
    /// and no other common factor).
    ///
    /// `None` for the C# / adapter dialects, which have no notion of it —
    /// they fall back to the per-kind inference from `display`.
    pub counter: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct ParsedOrb {
    pub kind: OrbKind,
    pub passive: i32,
    pub evoke: i32,
}

#[derive(Debug, Clone)]
pub struct ParsedOsty {
    pub alive: bool,
    pub hp: i32,
    pub max_hp: i32,
    pub block: i32,
}

#[derive(Debug, Clone)]
pub struct ParsedDeck {
    pub draw_pile: Vec<ParsedCard>,
    pub discard_pile: Vec<ParsedCard>,
    pub hand: Vec<ParsedCard>,
    pub exhaust_pile: Vec<ParsedCard>,
    /// Cards currently *in flight* — played but not yet flushed to
    /// discard/exhaust ([`sts2_core::PlayerCombatState::play`]).
    ///
    /// Non-empty only in a mid-resolution snapshot, which is exactly the
    /// moment a card-select screen is open: the card that opened it is
    /// sitting here. Dropping it (the pre-mirror behavior — this pile had
    /// no wire representation at all) meant a reconstruction resolved the
    /// choice with nothing to flush afterwards, and the played card
    /// silently vanished from the deck. Optional; empty on every
    /// between-actions producer.
    pub play_pile: Vec<ParsedCard>,
    pub master_deck: Option<Vec<ParsedCard>>,
    /// Indices into [`Self::draw_pile`] naming the cards the player
    /// KNOWS sit on top of the draw pile (Headbutt-style pins), in
    /// [`sts2_core::PlayerCombatState::pinned_top`] order — element 0 is
    /// the next card drawn. Empty when the wire omits the field.
    ///
    /// Indices, not uids: `state_from_dict_impl` re-allocates
    /// [`CardUid`]s sequentially as it walks the piles, so a serialized
    /// uid means nothing to the receiver, but a *position* does.
    pub pinned_top: Vec<usize>,
    /// Symmetric to [`Self::pinned_top`] for bottom-of-draw pins, in
    /// [`sts2_core::PlayerCombatState::pinned_bottom`] order.
    pub pinned_bottom: Vec<usize>,
    /// Mirrors [`sts2_core::PlayerCombatState::deferred_draw_order`] —
    /// `true` means [`Self::draw_pile`]'s unknown middle is a canonical
    /// belief multiset rather than a materialized order. Defaults to
    /// `false` (every non-search producer).
    pub deferred_draw_order: bool,
}

#[derive(Debug, Clone)]
pub struct ParsedCard {
    /// `None` for `kind == "status"` / `"curse"` with a synthetic id
    /// like `"status:Burn"` that doesn't map to a `CardId` variant —
    /// T2 handles those via the status-card path. Otherwise `Some(CardId)`.
    pub card_id: Option<CardId>,
    /// Raw id string preserved verbatim for the status/curse path and
    /// for diagnostic messages.
    pub raw_id: String,
    pub stable_idx: i32,
    pub kind: String,
    pub upgraded: bool,
    /// Optional current base attack/block for the card. `state_to_dict`
    /// always emits these (`card.base_damage`/`base_block`); a reconstruction
    /// source (C# view via the adapter) that carries them lets us restore an
    /// UPGRADED/enchanted card's true stats. `None` falls back to the
    /// registry (unupgraded) spec — the historical behavior. Dropping this
    /// silently reset every upgraded card to its unupgraded value (upgraded
    /// Strike 9 -> 6), so the search planned the deck weaker than it is
    /// (2026-07-15 csview recon audit).
    pub base_damage: Option<i32>,
    pub base_block: Option<i32>,
    /// The card's EFFECTIVE cost, every transient modifier already folded
    /// in ([`sts2_core::hooks::effective_card_cost`]). Display consumers
    /// read this; the rebuild prefers [`Self::base_cost`] when present.
    pub cost_override: Option<i32>,
    /// The instance's OWN cost field, before any transient modifier.
    ///
    /// Baking the effective cost into `Card.cost` turned a this-turn
    /// discount into a permanent one: a Stomp-reduced or free-this-turn
    /// card came back cheap forever, and the search then planned hands it
    /// could not actually afford. Measured on the 60-run realdll_v5 sweep,
    /// the mirror was cheaper than the game on 1135 of ~1700 card-cost
    /// divergences, concentrated on `end_turn` — the exact moment the real
    /// discount expires. `None` for the C# / adapter dialects, which fall
    /// back to `cost_override` as before.
    pub base_cost: Option<i32>,
    /// What [`Self::base_cost`] becomes once THIS TURN's cost modifiers wear
    /// off (C# `EndOfTurnCleanup`).
    ///
    /// A local cost modifier carries a lifetime — end of combat, end of turn,
    /// when played — that no single cost number can express. Stomp's "1 less
    /// per Attack played this turn" and Enlightenment's "costs 1 this combat"
    /// both arrive as a cheaper card, but only the first springs back at turn
    /// start, so a rebuild from `base_cost` alone keeps every transient
    /// discount for the rest of the fight (v8 mirror sweep: 163 of STOMP's
    /// 175 cost divergences landed on the `end_turn` that should have undone
    /// its shave).
    ///
    /// `None` when it cannot differ from `base_cost` — which is the common
    /// case, and always so for the Rust dialect, whose `combat_vars` already
    /// carry the exact per-effect decomposition.
    pub persistent_cost: Option<i32>,
    pub played_cost: Option<i32>,
    pub stable_cost: Option<i32>,
    pub flags: Vec<String>,
    pub enchantment: Option<String>,
    /// Stack size of [`Self::enchantment`] (C# `EnchantmentModel.Amount`).
    /// The C# producers omit it when it is 0 — which for the flag-shaped
    /// enchantments (Steady, Goopy, …) is their normal state — so absent
    /// means "one stack", not "no stacks": `has_enchantment` is `> 0`, and
    /// stamping 0 would reconstruct the enchantment as not there at all.
    pub enchantment_amount: Option<i32>,
    /// Per-instance scratch ([`sts2_core::Card::combat_vars`]) as
    /// `(key, value)` pairs. This is where the transient cost modifiers
    /// live (`FreeThisTurn`, `StompCostReductionThisTurn`,
    /// `SneckoCostDeltaThisTurn`), alongside Rampage/Claw/GeneticAlgorithm
    /// scaling. Empty for producers that don't carry it.
    pub combat_vars: Vec<(VarKey, i32)>,
    /// Optional per-card affliction (C# `CardModel.Affliction`), by wire
    /// name (`"Bound"`, `"Devoured"`, …). Dropping this on the wire made
    /// reconstructed states plan through ChainsOfBinding as if every
    /// Bound card were playable (live Queen fight, 2026-07-13).
    pub affliction: Option<String>,
    /// `AfflictionModel._amount` for the affliction above. Ignored when
    /// `affliction` is `None`.
    pub affliction_stacks: i32,
    /// Devoured's `AppliedExhaust` flag (`Devoured.cs:9-22`). Ignored
    /// for every other affliction kind.
    pub affliction_applied_exhaust: bool,
    /// The producer's own `CardModel.Type`, lower-cased, when it ships one.
    ///
    /// Almost every card's type is a constant of its class and is taken from
    /// the registry spec instead. The exception is MadScience, whose
    /// `Type => TinkerTimeType`
    /// (`../sts2-wiki/decompiled/v0.107.1/MegaCrit.Sts2.Core.Models.Cards/MadScience.cs:80`)
    /// is PER INSTANCE — so for that card this is the only wire signal of
    /// which of the three OnPlay branches will run. `None` for producers that
    /// do not send it (the engine's own `state_to_dict` never has to: its
    /// `combat_vars.generic` carries the packed variant directly).
    pub card_type: Option<String>,
    /// C# `MadScience.TinkerTimeRider`, a public `[SavedProperty]`
    /// (`MadScience.cs:125-137`), by enum name. `None` when the producer does
    /// not send it — which is currently EVERY live producer, see the
    /// MadScience block in [`build_card`].
    pub tinker_time_rider: Option<String>,
    /// C# `CardModel.BaseReplayCount`: the card's own extra plays (Hidden Gem,
    /// Transfigure), NOT counting an enchantment's (Spiral, Glam), which the
    /// engine adds from the enchantment itself. [`sts2_core::Card::base_replay_count`].
    /// Absent → 0, the value every card had on the wire before the field.
    pub replay_count: i32,
    /// An X-cost card's energy captured at its last play (C#
    /// `CardEnergyCost.CapturedXValue`; [`sts2_core::Card::energy_spent`]).
    /// Absent → 0 (never played, or not an X-cost card). sts2-agent fork.
    pub energy_spent: i32,
}

#[derive(Debug, Clone)]
pub struct ParsedEnemy {
    pub kind: MonsterKind,
    pub uid: i64,
    pub hp: i32,
    pub max_hp: i32,
    pub block: i32,
    pub powers: Vec<ParsedPower>,
    /// Current (next-to-execute) intent translated from the wire dict's
    /// `intents` list. `None` only when the wire surfaces an empty list
    /// (the enemy has no telegraphed move — e.g. summoned mid-turn).
    /// Bead `sts2-rs-2rp7`: previously this was a no-op `intents_len`
    /// counter so every Rust-side rollout saw enemies as inert
    /// (`Intent::None`), which made every multi-enemy hint produce
    /// 100%-Tie rollouts and "LIKELY-LOSS 0%" displays.
    pub intent: Option<Intent>,
    pub intent_history: Vec<i32>,
    pub is_dead: bool,
    pub is_escaped: bool,
    /// `false` when this enemy's *current* intent is an Attack /
    /// `MultiAttack` whose damage we couldn't pin down — wire `amount`
    /// was missing/0 *and* the optional `label` field (if present) had
    /// no parseable leading integer. MCTS rollouts will still tick the
    /// intent, but the attack will resolve for 0 damage which makes the
    /// continuous Q over-estimate safety. Bead `sts2-rs-bpt4`.
    ///
    /// `true` for non-Attack intents, intents that translated cleanly,
    /// and the `intent.is_none()` case (no telegraphed move ⇒ nothing
    /// to be wrong about).
    pub intent_damage_known: bool,
    /// The enemy AI's current move-state id (the `move_state` field
    /// [`state_to_dict`](crate::state_dict::state_to_dict_py) emits from
    /// `MonsterAi::current_state_id`), when present. Used to rebuild the
    /// enemy AI so it keeps acting past its first telegraphed intent
    /// (bead `sts2-rs-3f3rz`). `None` for wires that omit it (older
    /// adapters / the C# backend, which never emitted it).
    pub move_state: Option<String>,
    /// Reflection-exported `Fabricator._lastSpawned` (canonical bot id,
    /// e.g. `"ZAPBOT"`), on the Fabricator's own entry. 2026-08-02 wire
    /// disclosure; when present it supersedes the max-uid inference,
    /// whose documented gap is a most-recent bot that already died.
    /// Absent = nothing spawned yet (relic-latch convention).
    pub fabricator_last_spawned: Option<String>,
    /// Reflection-exported `KnowledgeDemon._curseOfKnowledgeCounter`
    /// (0..3 — the Disintegration 6/7/8 curse index), on the demon's own
    /// entry. Without it reconstruction rebuilt the counter at 0, so a
    /// mid-fight mirror re-offered curse set 0 (the documented
    /// DISINTEGRATION off-by-one).
    pub curse_of_knowledge_counter: Option<i32>,
}
// ---------------------------------------------------------------------------
// Wire helpers — read-or-fail
// ---------------------------------------------------------------------------
//
// Each of these was a `PyDict`-typed accessor; they now read through
// [`WireValue`], so the same parse serves a `PyDict` and a `serde_json::Value`
// identically. The error strings are unchanged — `sts2-py/tests/fail_loud.rs`
// asserts on them.

fn require_keys<V: WireValue>(dict: &V, path: &str, keys: &[&str]) -> WireResult<()> {
    for k in keys {
        // Present-but-null passes, exactly as `PyDict::get_item(k)?.is_none()`
        // did: that tests key ABSENCE, not the value's nullity.
        if dict.get(k).is_none() {
            return Err(schema_err(path, &format!("missing required key `{k}`")));
        }
    }
    Ok(())
}

fn get_item<V: WireValue>(dict: &V, path: &str, key: &str) -> WireResult<V> {
    dict.get(key)
        .ok_or_else(|| schema_err(path, &format!("missing required key `{key}`")))
}

fn extract_str<V: WireValue>(dict: &V, path: &str, key: &str) -> WireResult<String> {
    get_item(dict, path, key)?
        .as_str()
        .ok_or_else(|| schema_err(path, &format!("`{key}` must be a string")))
}

fn extract_i64<V: WireValue>(dict: &V, path: &str, key: &str) -> WireResult<i64> {
    get_item(dict, path, key)?
        .as_i64()
        .ok_or_else(|| schema_err(path, &format!("`{key}` must be an integer")))
}

fn extract_i32<V: WireValue>(dict: &V, path: &str, key: &str) -> WireResult<i32> {
    let v = get_item(dict, path, key)?;
    v.as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| schema_err(path, &format!("`{key}` must fit in i32")))
}

fn extract_u32<V: WireValue>(dict: &V, path: &str, key: &str) -> WireResult<u32> {
    let v = get_item(dict, path, key)?;
    v.as_i64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| schema_err(path, &format!("`{key}` must fit in u32")))
}

fn extract_bool<V: WireValue>(dict: &V, path: &str, key: &str) -> WireResult<bool> {
    get_item(dict, path, key)?
        .as_bool()
        .ok_or_else(|| schema_err(path, &format!("`{key}` must be a boolean")))
}

/// The value at `key`, or `None` when the key is absent **or** explicitly
/// null. (Unlike the other helpers this is infallible — the `PyO3` version
/// returned a `PyResult` only because `PyDict::get_item` can raise.)
fn extract_optional<V: WireValue>(dict: &V, key: &str) -> Option<V> {
    dict.get(key).filter(|v| !v.is_null())
}

fn as_dict<V: WireValue>(value: &V, path: &str) -> WireResult<V> {
    if value.is_object() {
        Ok(value.clone())
    } else {
        Err(schema_err(path, "expected dict"))
    }
}

fn as_list<V: WireValue>(value: &V, path: &str) -> WireResult<Vec<V>> {
    value
        .as_array()
        .ok_or_else(|| schema_err(path, "expected list"))
}

/// `(key, value)` pairs of an object, key-sorted (see [`WireValue::entries`]).
fn as_entries<V: WireValue>(value: &V, path: &str) -> WireResult<Vec<(String, V)>> {
    value
        .entries()
        .ok_or_else(|| schema_err(path, "expected dict"))
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Parse + validate a wire-format dict into [`ParsedState`].
///
/// Fail-loud surface:
/// * Missing required keys → [`crate::WireError::Schema`].
/// * `wire_version != 1` → [`crate::WireError::Unsupported`].
/// * Unknown relic/card/monster/power id → [`crate::WireError::Unsupported`] with
///   the offending JSON path.
/// * `hp > max_hp`, `energy < 0`, etc. → [`crate::WireError::Schema`].
/// * `active_attack`, `pending_choice`, or `pending_play` non-null →
///   [`crate::WireError::Unsupported`] (mid-resolution states are out of scope).
pub fn parse_state_dict<V: WireValue>(d: &V) -> WireResult<ParsedState> {
    require_keys(d, "$", REQUIRED_TOP_KEYS)?;

    let wire_version = extract_u32(d, "$", "wire_version")?;
    if wire_version != WIRE_VERSION {
        return Err(unsupported(
            "$.wire_version",
            &format!("expected {WIRE_VERSION}, got {wire_version}"),
        ));
    }

    let phase = extract_str(d, "$", "phase")?;
    if !VALID_PHASES.contains(&phase.as_str()) {
        return Err(unsupported("$.phase", &phase));
    }

    let side = extract_str(d, "$", "side")?;
    if !VALID_SIDES.contains(&side.as_str()) {
        return Err(unsupported("$.side", &side));
    }

    let round = extract_i32(d, "$", "round")?;
    if round < 0 {
        return Err(schema_err("$.round", "must be >= 0"));
    }
    let is_elite = extract_bool(d, "$", "is_elite")?;
    let turn = extract_i32(d, "$", "turn")?;
    if turn < 0 {
        return Err(schema_err("$.turn", "must be >= 0"));
    }
    let combat_id = extract_i64(d, "$", "combat_id")?;

    // Optional intent dialect marker — see `ParsedState::intent_amounts`.
    let intent_amounts = match extract_optional(d, "intent_amounts") {
        None => IntentAmounts::Effective,
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| schema_err("$.intent_amounts", "must be a string or null"))?;
            match s.as_str() {
                "effective" => IntentAmounts::Effective,
                "base" => IntentAmounts::Base,
                other => {
                    return Err(schema_err(
                        "$.intent_amounts",
                        &format!("must be 'effective' or 'base', got {other:?}"),
                    ))
                }
            }
        }
    };

    // Mid-resolution guards. The schema lists these as documented-null
    // at hint time; any non-null value means the adapter is calling us
    // during a choice / mid-card-resolution and the rollout cannot
    // safely continue. Documented in plan §Out of scope.
    for forbidden in ["active_attack", "pending_choice", "pending_play"] {
        if let Some(v) = extract_optional(d, forbidden) {
            return Err(unsupported(
                format!("$.{forbidden}"),
                &format!(
                    "mid-resolution state not modellable (value: {})",
                    v.render()
                ),
            ));
        }
    }

    // Optional `rng` block. Every key inside is optional individually.
    let (rng_seed, ascension, run_seed, rng_counters) = if let Some(rng_val) =
        extract_optional(d, "rng")
    {
        let rng_dict = as_dict(&rng_val, "$.rng")?;
        // `as_u64`, not `as_i64` — a seed above `i64::MAX` is ordinary and
        // the `PyO3` path accepted it.
        let seed = if let Some(v) = extract_optional(&rng_dict, "seed") {
            Some(
                v.as_u64()
                    .ok_or_else(|| schema_err("$.rng.seed", "must be a u64"))?,
            )
        } else {
            None
        };
        let asc = if let Some(v) = extract_optional(&rng_dict, "ascension") {
            v.as_i64()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| schema_err("$.rng.ascension", "must be a u8"))?
        } else {
            0
        };
        // The RUN's own dice: the once-hashed master seed plus each stream's
        // read count. A producer that carries them puts the reconstruction on
        // the exact roll the world is about to make; one that doesn't leaves
        // the streams unrelated to the game's, and every intent the state
        // predicts is a guess.
        let run_seed = match extract_optional(&rng_dict, "run_seed") {
            Some(v) => Some(
                // C# `RunRngSet.Seed` is a uint; the Rust constructor takes
                // the same 32 bits as i32.
                v.as_i64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| schema_err("$.rng.run_seed", "must be a u32"))?
                    as i32,
            ),
            None => None,
        };
        let mut counters: Vec<(String, u32)> = Vec::new();
        if let Some(v) = extract_optional(&rng_dict, "counters") {
            for (name, val) in as_entries(&v, "$.rng.counters")? {
                let n = val
                    .as_i64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| schema_err(format!("$.rng.counters.{name}"), "must be a u32"))?;
                counters.push((name, n));
            }
            counters.sort();
        }
        (seed, asc, run_seed, counters)
    } else {
        (None, 0u8, None, Vec::new())
    };

    let player_val = get_item(d, "$", "player")?;
    let player_dict = as_dict(&player_val, "$.player")?;
    let player = parse_player(&player_dict)?;

    let deck_val = get_item(d, "$", "deck")?;
    let deck_dict = as_dict(&deck_val, "$.deck")?;
    let deck = parse_deck(&deck_dict)?;

    let enc_val = get_item(d, "$", "encounter")?;
    let enc_list = as_list(&enc_val, "$.encounter")?;
    let mut encounter = Vec::with_capacity(enc_list.len());
    for (i, item) in enc_list.iter().enumerate() {
        let path = format!("$.encounter[{i}]");
        let item_dict = as_dict(item, &path)?;
        encounter.push(parse_enemy(&item_dict, &path)?);
    }

    // Aggregate the per-enemy intent_damage_known into a single state-
    // level flag. `true` only when every alive-and-non-escaped enemy's
    // current intent is either non-Attack or has trustworthy damage.
    // Dead/escaped enemies don't contribute (their intent never fires).
    // Bead `sts2-rs-bpt4`.
    let intent_damage_known = encounter
        .iter()
        .filter(|e| !e.is_dead && !e.is_escaped)
        .all(|e| e.intent_damage_known);

    Ok(ParsedState {
        wire_version,
        phase,
        round,
        side,
        is_elite,
        player,
        deck,
        encounter,
        turn,
        combat_id,
        rng_seed,
        run_seed,
        rng_counters,
        ascension,
        intent_damage_known,
        intent_amounts,
    })
}

fn parse_player<V: WireValue>(d: &V) -> WireResult<ParsedPlayer> {
    let path = "$.player";
    require_keys(d, path, REQUIRED_PLAYER_KEYS)?;

    let class = extract_str(d, path, "class")?;
    if !VALID_CLASSES.contains(&class.as_str()) {
        return Err(unsupported("$.player.class", &class));
    }

    let hp = extract_i32(d, path, "hp")?;
    let max_hp = extract_i32(d, path, "max_hp")?;
    let block = extract_i32(d, path, "block")?;
    let energy = extract_i32(d, path, "energy")?;
    let max_energy = extract_i32(d, path, "max_energy")?;
    let energy_regen_per_turn = extract_i32(d, path, "energy_regen_per_turn")?;

    if hp > max_hp {
        return Err(schema_err(
            "$.player",
            &format!("hp ({hp}) > max_hp ({max_hp})"),
        ));
    }
    if hp < 0 {
        return Err(schema_err("$.player.hp", "must be >= 0"));
    }
    if max_hp <= 0 {
        return Err(schema_err("$.player.max_hp", "must be > 0"));
    }
    if energy < 0 {
        return Err(schema_err("$.player.energy", "must be >= 0"));
    }
    if max_energy < 0 {
        return Err(schema_err("$.player.max_energy", "must be >= 0"));
    }
    if energy_regen_per_turn < 0 {
        return Err(schema_err("$.player.energy_regen_per_turn", "must be >= 0"));
    }
    if block < 0 {
        return Err(schema_err("$.player.block", "must be >= 0"));
    }

    // Relics
    let relics_val = get_item(d, path, "relics")?;
    let relics_list = as_list(&relics_val, "$.player.relics")?;
    let mut relics = Vec::with_capacity(relics_list.len());
    for (i, r) in relics_list.iter().enumerate() {
        let rp = format!("$.player.relics[{i}]");
        let rd = as_dict(r, &rp)?;
        let id = extract_str(&rd, &rp, "id")?;
        let kind = relic_kind_from_python_name(&id).ok_or_else(|| unsupported(rp.clone(), &id))?;
        let counter = match extract_optional(&rd, "counter") {
            Some(v) => Some(
                v.as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or_else(|| schema_err(&rp, "`counter` must be an integer or null"))?,
            ),
            None => None,
        };
        // ToyBox wax/melt state. Optional so older producers (and every
        // wire written before RunSimulator.cs started emitting them) parse
        // unchanged; absent means "not wax, not melted".
        let flag = |key: &str| -> WireResult<bool> {
            match extract_optional(&rd, key) {
                Some(v) => v
                    .as_bool()
                    .ok_or_else(|| schema_err(&rp, &format!("`{key}` must be a bool or null"))),
                None => Ok(false),
            }
        };
        let is_wax = flag("is_wax")?;
        let is_melted = flag("is_melted")?;
        relics.push(ParsedRelic {
            kind,
            counter,
            is_wax,
            is_melted,
        });
    }

    // Powers
    let powers = parse_powers(&get_item(d, path, "powers")?, "$.player.powers")?;

    // Potions
    let potions_val = get_item(d, path, "potions")?;
    let potions_list = as_list(&potions_val, "$.player.potions")?;
    let mut potions: Vec<Option<PotionKind>> = Vec::with_capacity(potions_list.len());
    for (i, p) in potions_list.iter().enumerate() {
        let pp = format!("$.player.potions[{i}]");
        if p.is_null() {
            potions.push(None);
            continue;
        }
        let id = p
            .as_str()
            .ok_or_else(|| schema_err(&pp, "expected null or string id"))?;
        let kind = potion_kind_from_python_name(&id).ok_or_else(|| unsupported(pp.clone(), &id))?;
        potions.push(Some(kind));
    }

    // Optional: orbs / orb_slots / stars / osty
    let (orbs, orb_slots) = match extract_optional(d, "orbs") {
        Some(ov) => {
            let ol = as_list(&ov, "$.player.orbs")?;
            let mut parsed = Vec::with_capacity(ol.len());
            for (i, o) in ol.iter().enumerate() {
                let op = format!("$.player.orbs[{i}]");
                let od = as_dict(o, &op)?;
                let ty = extract_str(&od, &op, "type")?;
                let kind =
                    orb_kind_from_python_name(&ty).ok_or_else(|| unsupported(op.clone(), &ty))?;
                let passive = extract_i32(&od, &op, "passive")?;
                let evoke = extract_i32(&od, &op, "evoke")?;
                parsed.push(ParsedOrb {
                    kind,
                    passive,
                    evoke,
                });
            }
            let slots = match extract_optional(d, "orb_slots") {
                Some(v) => Some(
                    v.as_i64()
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or_else(|| schema_err("$.player.orb_slots", "must be u32"))?,
                ),
                None => None,
            };
            (Some(parsed), slots)
        }
        None => (None, None),
    };

    let stars = match extract_optional(d, "stars") {
        Some(v) => Some(
            v.as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| schema_err("$.player.stars", "must be i32"))?,
        ),
        None => None,
    };

    let osty = match extract_optional(d, "osty") {
        Some(v) => {
            let od = as_dict(&v, "$.player.osty")?;
            let alive = extract_bool(&od, "$.player.osty", "alive")?;
            let hp = extract_i32(&od, "$.player.osty", "hp")?;
            let max_hp = extract_i32(&od, "$.player.osty", "max_hp")?;
            let block = extract_i32(&od, "$.player.osty", "block")?;
            if hp > max_hp {
                return Err(schema_err(
                    "$.player.osty",
                    &format!("hp ({hp}) > max_hp ({max_hp})"),
                ));
            }
            Some(ParsedOsty {
                alive,
                hp,
                max_hp,
                block,
            })
        }
        None => None,
    };

    // Optional per-turn play counters (one bucket per CardType, canonical
    // index order). Mid-turn ShouldPlay gates (Ringing's one-card-per-turn,
    // Normality's three, Sloth) read these; without them a round-tripped
    // mid-turn state believes zero cards were played this turn and its
    // legal-action surface offers plays the source engine silently rejects
    // (rung-3 stall vs Ceremonial Beast, 2026-07-12). Absent → zeros, so
    // wire producers that don't track it (bridge) stay valid.
    let cards_played_this_turn = match extract_optional(d, "cards_played_this_turn") {
        Some(v) => {
            let cp = "$.player.cards_played_this_turn";
            let list = as_list(&v, cp)?;
            if list.len() != 5 {
                return Err(schema_err(cp, "must be a list of 5 counters"));
            }
            let mut buckets = [0u32; 5];
            for (i, item) in list.iter().enumerate() {
                buckets[i] = item
                    .as_i64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| schema_err(format!("{cp}[{i}]"), "must be a u32"))?;
            }
            Some(buckets)
        }
        None => None,
    };

    let bound_card_played_this_turn = match extract_optional(d, "bound_card_played_this_turn") {
        Some(v) => v.as_bool().ok_or_else(|| {
            schema_err(
                "$.player.bound_card_played_this_turn",
                "must be bool or null",
            )
        })?,
        None => false,
    };

    let gold = match extract_optional(d, "gold") {
        Some(v) => Some(
            v.as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| schema_err("$.player.gold", "must be int or null"))?,
        ),
        None => None,
    };
    if matches!(gold, Some(g) if g < 0) {
        return Err(schema_err("$.player.gold", "must be >= 0"));
    }

    Ok(ParsedPlayer {
        class,
        hp,
        max_hp,
        block,
        energy,
        max_energy,
        energy_regen_per_turn,
        relics,
        powers,
        potions,
        orbs,
        orb_slots,
        stars,
        osty,
        cards_played_this_turn,
        gold,
        bound_card_played_this_turn,
    })
}

fn parse_powers<V: WireValue>(value: &V, path: &str) -> WireResult<Vec<ParsedPower>> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let list = as_list(value, path)?;
    let mut out = Vec::with_capacity(list.len());
    for (i, p) in list.iter().enumerate() {
        let pp = format!("{path}[{i}]");
        let pd = as_dict(p, &pp)?;
        let name = extract_str(&pd, &pp, "name")?;
        let kind = power_kind_from_python_name(&name)
            .ok_or_else(|| unsupported(format!("{pp}.name"), &name))?;
        let amount = extract_i32(&pd, &pp, "amount")?;
        let has_extra = extract_optional(&pd, "extra").is_some();
        let display = match extract_optional(&pd, "display") {
            Some(v) => Some(
                v.as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or_else(|| schema_err(&pp, "`display` must fit in i32"))?,
            ),
            None => None,
        };
        let counter = match extract_optional(&pd, "counter") {
            Some(v) => Some(
                v.as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or_else(|| schema_err(&pp, "`counter` must fit in i32"))?,
            ),
            None => None,
        };
        out.push(ParsedPower {
            kind,
            amount,
            has_extra,
            display,
            counter,
        });
    }
    Ok(out)
}

fn parse_deck<V: WireValue>(d: &V) -> WireResult<ParsedDeck> {
    let path = "$.deck";
    require_keys(d, path, REQUIRED_DECK_KEYS)?;

    let draw_pile = parse_pile(&get_item(d, path, "draw_pile")?, "$.deck.draw_pile")?;
    let discard_pile = parse_pile(&get_item(d, path, "discard_pile")?, "$.deck.discard_pile")?;
    let hand = parse_pile(&get_item(d, path, "hand")?, "$.deck.hand")?;
    let exhaust_pile = parse_pile(&get_item(d, path, "exhaust_pile")?, "$.deck.exhaust_pile")?;
    // Optional: only a mid-resolution snapshot has anything in flight.
    let play_pile = match extract_optional(d, "play_pile") {
        Some(v) => parse_pile(&v, "$.deck.play_pile")?,
        None => Vec::new(),
    };

    let master_deck = match extract_optional(d, "master_deck") {
        Some(v) => Some(parse_pile(&v, "$.deck.master_deck")?),
        None => None,
    };

    // Optional draw-pile *knowledge* (bead `sts2-rs-o6xw` pins + the F2
    // belief flag). Absent on every pre-mirror producer, hence optional
    // and defaulted — see docs/schema-version-policy.md ("pure additions
    // of optional fields never bump the version").
    let pinned_top = parse_pin_indices(d, "pinned_top", draw_pile.len())?;
    let pinned_bottom = parse_pin_indices(d, "pinned_bottom", draw_pile.len())?;
    for idx in &pinned_bottom {
        if pinned_top.contains(idx) {
            return Err(schema_err(
                "$.deck.pinned_bottom",
                &format!("draw-pile index {idx} is pinned to both top and bottom"),
            ));
        }
    }
    let deferred_draw_order = match extract_optional(d, "deferred_draw_order") {
        Some(v) => v
            .as_bool()
            .ok_or_else(|| schema_err("$.deck.deferred_draw_order", "must be a boolean or null"))?,
        None => false,
    };

    Ok(ParsedDeck {
        draw_pile,
        discard_pile,
        hand,
        exhaust_pile,
        play_pile,
        master_deck,
        pinned_top,
        pinned_bottom,
        deferred_draw_order,
    })
}

/// Parse one optional `$.deck.<key>` pin list: draw-pile indices, each
/// in `0..draw_len`, no repeats.
///
/// Deliberately does NOT assert that the indices form a prefix/suffix of
/// the draw pile. That invariant only holds *after* a shuffle —
/// [`sts2_core::cards::pin_top`] records the pin without physically
/// moving the card, so a mid-turn snapshot can legitimately carry a pin
/// at an arbitrary position.
fn parse_pin_indices<V: WireValue>(d: &V, key: &str, draw_len: usize) -> WireResult<Vec<usize>> {
    let path = format!("$.deck.{key}");
    let Some(v) = extract_optional(d, key) else {
        return Ok(Vec::new());
    };
    let list = as_list(&v, &path)?;
    let mut out: Vec<usize> = Vec::with_capacity(list.len());
    for (i, item) in list.iter().enumerate() {
        let ip = format!("{path}[{i}]");
        let idx = item
            .as_i64()
            .ok_or_else(|| schema_err(&ip, "must be an integer draw-pile index"))?;
        let idx = usize::try_from(idx)
            .map_err(|_| schema_err(&ip, &format!("index {idx} must be >= 0")))?;
        if idx >= draw_len {
            return Err(schema_err(
                &ip,
                &format!("index {idx} out of range for a {draw_len}-card draw pile"),
            ));
        }
        if out.contains(&idx) {
            return Err(schema_err(&ip, &format!("duplicate draw-pile index {idx}")));
        }
        out.push(idx);
    }
    Ok(out)
}

fn parse_pile<V: WireValue>(value: &V, path: &str) -> WireResult<Vec<ParsedCard>> {
    let list = as_list(value, path)?;
    let mut out = Vec::with_capacity(list.len());
    for (i, c) in list.iter().enumerate() {
        let cp = format!("{path}[{i}]");
        let cd = as_dict(c, &cp)?;
        out.push(parse_card(&cd, &cp)?);
    }
    Ok(out)
}

/// Resolve a synthetic `status:Foo` / `curse:Foo Bar` wire id to its real
/// [`CardId`], or `None` when the remainder names no known card.
///
/// The producers spell the tail as a display name (`"Slimed"`,
/// `"Curse of the Bell"`) while the registry uses UPPER_SNAKE
/// (`SLIMED`, `CURSE_OF_THE_BELL`), so the tail is normalized before lookup:
/// camel boundaries become underscores, every other non-alphanumeric run
/// becomes a single underscore, and the whole thing is upper-cased.
///
/// `None` still falls through to the AscendersBane placeholder in
/// [`build_card`] — that path exists for ids the registry genuinely does not
/// have, and keeping it means a future unknown status degrades instead of
/// failing the whole reconstruction.
fn card_id_from_synthetic_name(raw_id: &str) -> Option<CardId> {
    let tail = raw_id.split_once(':').map_or(raw_id, |(_, t)| t);
    let mut out = String::with_capacity(tail.len() + 4);
    let mut prev_lower_or_digit = false;
    for ch in tail.chars() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() && prev_lower_or_digit {
                out.push('_');
            }
            out.push(ch.to_ascii_uppercase());
            prev_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        } else {
            if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
            prev_lower_or_digit = false;
        }
    }
    card_id_from_python_name(out.trim_matches('_'))
}

fn parse_card<V: WireValue>(d: &V, path: &str) -> WireResult<ParsedCard> {
    require_keys(d, path, REQUIRED_CARDSPEC_KEYS)?;
    let raw_id = extract_str(d, path, "id")?;
    let stable_idx = extract_i32(d, path, "stable_idx")?;
    let kind = extract_str(d, path, "kind")?;
    let upgraded = extract_bool(d, path, "upgraded")?;

    if !VALID_CARD_KINDS.contains(&kind.as_str()) {
        return Err(unsupported(format!("{path}.kind"), &kind));
    }

    // For `kind == "card"` the id MUST resolve to a `CardId` variant.
    // Status/curse may carry synthetic ids like `"status:Burn"` — those
    // are translated by T2; here we only verify the ones tagged as
    // regular cards.
    let card_id = if kind == "card" {
        Some(card_id_from_python_name(&raw_id).ok_or_else(|| unsupported(path, &raw_id))?)
    } else {
        // For status/curse, accept a `status:Foo` shape OR a bare
        // CardId name (curses in the master deck are real CardIds).
        // Validation: if it doesn't have the `<kind>:` prefix it must
        // resolve as a CardId.
        if raw_id.starts_with("status:") || raw_id.starts_with("curse:") {
            // …but almost all of them DO name a real card once the prefix
            // is stripped, and treating them as unresolvable was expensive:
            // `build_card` funnels an unresolved id into AscendersBane, so
            // every Slimed / Dazed / Burn / Wound the enemy inflicted, and
            // every Clumsy, entered the searched deck as an unplayable
            // ethereal curse instead. Burn's end-of-turn damage simply never
            // happened in rollouts. Measured on the 60-run realdll_v6 sweep:
            // 1374 AscendersBane in the mirror against 751 `status:Slimed`,
            // 392 `status:Dazed`, 278 `curse:Clumsy`, 182 `status:Burn` and
            // 120 `status:Wound` in the game — the largest remaining
            // reconstruction gap on the DLL path.
            card_id_from_synthetic_name(&raw_id)
        } else {
            Some(card_id_from_python_name(&raw_id).ok_or_else(|| unsupported(path, &raw_id))?)
        }
    };

    // Optional integer fields all share this shape: absent-or-null → `None`,
    // present → must fit in i32.
    let opt_i32 = |key: &str, msg: &str| -> WireResult<Option<i32>> {
        match extract_optional(d, key) {
            Some(v) => Ok(Some(
                v.as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or_else(|| schema_err(path, msg))?,
            )),
            None => Ok(None),
        }
    };

    let cost_override = opt_i32("cost_override", "`cost_override` must be int or null")?;

    // Optional current base attack/block. `None` (or an explicit null) keeps
    // the registry-spec default; a present value overrides it so upgraded /
    // enchanted cards reconstruct at their true stats. `state_to_dict` always
    // emits both, so a native round-trip is faithful; the C#-view adapter
    // supplies them from `stats.damage` / `stats.block`.
    let base_damage = opt_i32("base_damage", "`base_damage` must be int or null")?;
    let base_block = opt_i32("base_block", "`base_block` must be int or null")?;

    let flags = match extract_optional(d, "flags") {
        Some(v) => {
            let l = as_list(&v, &format!("{path}.flags"))?;
            let mut fs = Vec::with_capacity(l.len());
            for (i, f) in l.iter().enumerate() {
                let fp = format!("{path}.flags[{i}]");
                let s = f
                    .as_str()
                    .ok_or_else(|| schema_err(&fp, "must be a string"))?;
                if !VALID_CARD_FLAGS.contains(&s.as_str()) {
                    return Err(unsupported(fp, &s));
                }
                fs.push(s);
            }
            fs
        }
        None => Vec::new(),
    };

    let enchantment = match extract_optional(d, "enchantment") {
        Some(v) => Some(
            v.as_str()
                .ok_or_else(|| schema_err(path, "`enchantment` must be string or null"))?,
        ),
        None => None,
    };

    let enchantment_amount = opt_i32(
        "enchantment_amount",
        "`enchantment_amount` must be int or null",
    )?;

    let base_cost = opt_i32("base_cost", "`base_cost` must be int or null")?;
    let persistent_cost = opt_i32("persistent_cost", "`persistent_cost` must be int or null")?;
    let played_cost = opt_i32("played_cost", "`played_cost` must be int or null")?;
    let stable_cost = opt_i32("stable_cost", "`stable_cost` must be int or null")?;
    // Two banks require the third number. `rebank_wire_cost_after_edge`
    // recomputes the surviving bank against `stable_cost`, which it reads back
    // out of a combat var that DEFAULTS TO 0 — so with BOTH banks live and no
    // `stable_cost` the second edge drives the card's cost to `0 - cost`. That
    // is not degradation, it is corruption wearing the shape of a value.
    //
    // Scoped to exactly that case on purpose, not "all three or none". A wire
    // carrying `persistent_cost` alone is the pre-v16 dialect and is still
    // perfectly sound: only one bank is ever live, `rebank` early-returns on
    // the zero one, and `stable_cost` is never read. Refusing that would break
    // every older producer for a hazard it cannot reach.
    //
    // Both in-tree producers derive all three from the same
    // `FoldLocalModifiers` behind identical guards, so they ship together and
    // this can only fire on a hand-written or third-party dict. Refusing it
    // here is what keeps that a schema error instead of a silent miscost.
    if persistent_cost.is_some() && played_cost.is_some() && stable_cost.is_none() {
        return Err(schema_err(
            path,
            "`persistent_cost` and `played_cost` together need `stable_cost` — with both \
             banks live the second edge re-banks against a defaulted 0 and drives the \
             cost to `0 - cost`",
        ));
    }
    let mut combat_vars: Vec<(VarKey, i32)> = Vec::new();
    if let Some(v) = extract_optional(d, "combat_vars") {
        for (name, val) in as_entries(&v, &format!("{path}.combat_vars"))? {
            let key = var_key_from_wire(&name)
                .ok_or_else(|| unsupported(format!("{path}.combat_vars"), &name))?;
            let amount = val
                .as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| {
                    schema_err(format!("{path}.combat_vars.{name}"), "value must be an i32")
                })?;
            combat_vars.push((key, amount));
        }
    }

    let affliction = match extract_optional(d, "affliction") {
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| schema_err(path, "`affliction` must be string or null"))?;
            // Fail loud on an unknown kind: soft-dropping is exactly how
            // the move_state / card-id name-dialect bugs stayed invisible.
            if card_affliction_kind_from_wire(&s).is_none() {
                return Err(unsupported(format!("{path}.affliction"), &s));
            }
            Some(s)
        }
        None => None,
    };
    let affliction_stacks = match extract_optional(d, "affliction_stacks") {
        Some(v) => v
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(|| schema_err(path, "`affliction_stacks` must be int or null"))?,
        None => 1,
    };
    let affliction_applied_exhaust = match extract_optional(d, "affliction_applied_exhaust") {
        Some(v) => v
            .as_bool()
            .ok_or_else(|| schema_err(path, "`affliction_applied_exhaust` must be bool or null"))?,
        None => false,
    };

    // Optional per-instance `CardModel.Type` / `MadScience.TinkerTimeRider`.
    // Unknown spellings are tolerated (stored raw, resolved by the one
    // consumer) rather than rejected: `type` is a display-ish field on every
    // producer that ships it, and only MadScience reads it.
    let card_type = match extract_optional(d, "type") {
        Some(v) => Some(
            v.as_str()
                .ok_or_else(|| schema_err(path, "`type` must be a string or null"))?
                .to_ascii_lowercase(),
        ),
        None => None,
    };
    let tinker_time_rider = match extract_optional(d, "tinker_time_rider") {
        Some(v) => Some(
            v.as_str()
                .ok_or_else(|| schema_err(path, "`tinker_time_rider` must be a string or null"))?,
        ),
        None => None,
    };

    let replay_count = opt_i32("replay_count", "`replay_count` must be int or null")?.unwrap_or(0);
    if replay_count < 0 {
        return Err(schema_err(format!("{path}.replay_count"), "must be >= 0"));
    }

    let energy_spent = opt_i32("energy_spent", "`energy_spent` must be int or null")?.unwrap_or(0);
    if energy_spent < 0 {
        return Err(schema_err(format!("{path}.energy_spent"), "must be >= 0"));
    }

    Ok(ParsedCard {
        card_id,
        raw_id,
        stable_idx,
        kind,
        upgraded,
        base_damage,
        base_block,
        cost_override,
        base_cost,
        persistent_cost,
        played_cost,
        stable_cost,
        flags,
        enchantment,
        enchantment_amount,
        combat_vars,
        affliction,
        affliction_stacks,
        affliction_applied_exhaust,
        card_type,
        tinker_time_rider,
        replay_count,
        energy_spent,
    })
}

/// Wire name → [`CardAfflictionKind`]. The wire vocabulary is the C#
/// affliction type name (`Bound.cs`, `Devoured.cs`, …), which the doc
/// comments on the enum variants pin 1:1.
pub fn card_affliction_kind_from_wire(
    s: &str,
) -> Option<sts2_core::card_afflictions::CardAfflictionKind> {
    use sts2_core::card_afflictions::CardAfflictionKind as K;
    Some(match s {
        "Bound" => K::Bound,
        "Devoured" => K::Devoured,
        "Entangled" => K::Entangled,
        "Galvanized" => K::Galvanized,
        "Hexed" => K::Hexed,
        "Ringing" => K::Ringing,
        "Smog" => K::Smog,
        "Weighted" => K::Weighted,
        "Tainted" => K::Tainted,
        _ => return None,
    })
}

/// [`CardAfflictionKind`] → wire name (inverse of
/// [`card_affliction_kind_from_wire`]; exhaustive so a new engine
/// variant forces a wire decision here).
pub fn card_affliction_kind_wire_name(
    k: sts2_core::card_afflictions::CardAfflictionKind,
) -> &'static str {
    use sts2_core::card_afflictions::CardAfflictionKind as K;
    match k {
        K::Bound => "Bound",
        K::Devoured => "Devoured",
        K::Entangled => "Entangled",
        K::Galvanized => "Galvanized",
        K::Hexed => "Hexed",
        K::Ringing => "Ringing",
        K::Smog => "Smog",
        K::Weighted => "Weighted",
        K::Tainted => "Tainted",
    }
}

fn parse_enemy<V: WireValue>(d: &V, path: &str) -> WireResult<ParsedEnemy> {
    require_keys(d, path, REQUIRED_ENCOUNTER_KEYS)?;
    let kind_str = extract_str(d, path, "kind")?;
    let kind = monster_kind_from_python_name(&kind_str)
        .ok_or_else(|| unsupported(format!("{path}.kind"), &kind_str))?;
    let uid = extract_i64(d, path, "uid")?;
    let hp = extract_i32(d, path, "hp")?;
    let max_hp = extract_i32(d, path, "max_hp")?;
    let block = extract_i32(d, path, "block")?;
    if hp > max_hp {
        return Err(schema_err(path, &format!("hp ({hp}) > max_hp ({max_hp})")));
    }
    if block < 0 {
        return Err(schema_err(format!("{path}.block"), "must be >= 0"));
    }

    let powers = parse_powers(&get_item(d, path, "powers")?, &format!("{path}.powers"))?;

    let intents_val = get_item(d, path, "intents")?;
    let intents_list = as_list(&intents_val, &format!("{path}.intents"))?;
    // The list is the shown move's every intent (a C# `MoveState`'s
    // `params AbstractIntent[]`; Headless and the engine's emitter both send
    // the whole list). The head is the current intent the rollout performs;
    // the move's other halves are performed by its effect, so the tail is
    // parsed only to check it is well formed. Bead `sts2-rs-2rp7`.
    let mut intent_damage_known = true;
    let intent = if intents_list.is_empty() {
        None
    } else {
        parse_intent(
            &intents_list[0],
            &format!("{path}.intents[0]"),
            &mut intent_damage_known,
        )?
    };
    for (i, tail) in intents_list.iter().enumerate().skip(1) {
        // A tail attack's unknown damage says nothing about the head's.
        let mut tail_known = true;
        parse_intent(tail, &format!("{path}.intents[{i}]"), &mut tail_known)?;
    }

    let history_val = get_item(d, path, "intent_history")?;
    let history_list = as_list(&history_val, &format!("{path}.intent_history"))?;
    let mut intent_history = Vec::with_capacity(history_list.len());
    for (i, h) in history_list.iter().enumerate() {
        let hp_path = format!("{path}.intent_history[{i}]");
        intent_history.push(
            h.as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| schema_err(&hp_path, "must be i32"))?,
        );
    }

    let is_dead = extract_bool(d, path, "is_dead")?;
    let is_escaped = extract_bool(d, path, "is_escaped")?;

    // Optional: the AI move-state id (only emitted by the Rust `state_to_dict`).
    let move_state = match extract_optional(d, "move_state") {
        Some(v) => Some(
            v.as_str()
                .ok_or_else(|| schema_err(path, "`move_state` must be a string"))?,
        ),
        None => None,
    };

    // Optional reflection-exported hidden counters (2026-08-02 producers).
    let fabricator_last_spawned = match extract_optional(d, "fabricator_last_spawned") {
        Some(v) => Some(
            v.as_str()
                .ok_or_else(|| schema_err(path, "`fabricator_last_spawned` must be a string"))?,
        ),
        None => None,
    };
    let curse_of_knowledge_counter = match extract_optional(d, "curse_of_knowledge_counter") {
        Some(v) => Some(
            v.as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| schema_err(path, "`curse_of_knowledge_counter` must be an int"))?,
        ),
        None => None,
    };

    Ok(ParsedEnemy {
        kind,
        uid,
        hp,
        max_hp,
        block,
        powers,
        intent,
        intent_history,
        is_dead,
        is_escaped,
        intent_damage_known,
        move_state,
        fabricator_last_spawned,
        curse_of_knowledge_counter,
    })
}

/// Translate one wire-dict intent (shape produced by
/// `sts2-cli/python/sts2_hint/adapter.py::_translate_intent`) into a
/// Rust [`Intent`] value. Returns `Ok(None)` when the wire entry is
/// well-formed but represents a kind the Rust enum doesn't have yet
/// (e.g. `Heal`, `Summon`, `Escape` — see `canonical_maps.INTENT_TYPES`
/// for the full wire vocabulary). Returns `Err` on schema violations
/// (missing `kind`, wrong field type).
///
/// The C# emitter's canonical `kind` strings:
/// `Attack`, `Defend`, `Buff`, `Heal`, `Debuff`, `DebuffStrong`,
/// `CardDebuff`, `StatusCard`, `DeathBlow`, `Escape`, `Summon`,
/// `Sleep`, `Stun`, `Hidden`, `Unknown`.
///
/// Field shape: `{"kind": str, "amount": Option<i32>, "times": Option<i32>}`.
/// `times` is only meaningful for `Attack` — when present and `> 1` we
/// produce [`Intent::MultiAttack`].
///
/// Extract `(damage, hits)` from an English intent label string when
/// the wire `amount` / `times` fields are missing or zero. Returns
/// `None` if no leading integer can be found.
///
/// Patterns we handle (in order of preference):
///   * `"6 × 3"`, `"6 x 3"`, `"6*3"` — `(damage, hits) = (6, 3)`
///   * `"Attack 6"`, `"Deal 6 damage"`, `"6"` — `(damage, hits) = (6, 1)`
///
/// We do *not* try to handle:
///   * Localised labels (German "Angriff 6", etc.) — `Sts2Headless`'s
///     hint sidecar is English-only at this pin (see `adapter.py`
///     module docstring); other locales would need a wire-side rewrite.
///   * Block / debuff amounts — the bead scope is Attack damage only;
///     Defend / Buff / Debuff intents don't influence the
///     player-death rollout signal that motivates `bpt4`.
///   * Compound expressions like `"6 + 4"` — STS2 attack labels don't
///     mix damage values; multi-hit is the only canonical compound.
///
/// Bead `sts2-rs-bpt4`.
pub fn extract_label_damage(label: &str) -> Option<(i32, i32)> {
    // Walk the bytes, collecting up to two unsigned decimal integers.
    // Treat any non-ASCII-digit byte as a separator. The first integer
    // is the per-hit damage; the second (if present) is the hit count.
    // This naturally handles `"Attack 6"`, `"Deal 6 damage"`,
    // `"6 × 3"` (the `×` is a multi-byte UTF-8 sequence — the bytes
    // between digits are skipped uniformly), `"6 x 3"`, `"6*3"`.
    let bytes = label.as_bytes();
    let mut nums: [i32; 2] = [0, 0];
    let mut found: usize = 0;
    let mut i = 0;
    while i < bytes.len() && found < nums.len() {
        let b = bytes[i];
        if b.is_ascii_digit() {
            // Parse an unsigned integer, saturating at i32::MAX to
            // avoid pathological labels overflowing.
            let mut n: i64 = 0;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                n = n
                    .saturating_mul(10)
                    .saturating_add(i64::from(bytes[i] - b'0'));
                i += 1;
            }
            #[allow(clippy::cast_possible_truncation)]
            let clamped = n.min(i64::from(i32::MAX)) as i32;
            nums[found] = clamped;
            found += 1;
        } else {
            i += 1;
        }
    }
    match found {
        0 => None,
        1 => Some((nums[0], 1)),
        // Two integers ⇒ "damage × hits". Reject hits == 0 (canonical
        // multi-hit is always >= 1) — fall back to single-hit if the
        // label is malformed.
        _ => Some((nums[0], nums[1].max(1))),
    }
}

/// `damage_known` is set to `false` by this function when the parsed
/// intent is an Attack/MultiAttack whose damage couldn't be determined
/// from the wire dict (i.e. `amount` was missing/0 *and* the optional
/// `label` field had no parseable leading integer). Caller-initialised
/// `true`; never set back to `true` here. See `bpt4` for context.
///
/// Bead `sts2-rs-2rp7`, `sts2-rs-bpt4`.
pub fn parse_intent<V: WireValue>(
    raw: &V,
    path: &str,
    damage_known: &mut bool,
) -> WireResult<Option<Intent>> {
    if !raw.is_object() {
        return Err(schema_err(path, "intent must be a dict"));
    }
    let kind_str = get_item(raw, path, "kind")?
        .as_str()
        .ok_or_else(|| schema_err(format!("{path}.kind"), "must be a string"))?;
    let amount: Option<i32> = match extract_optional(raw, "amount") {
        Some(v) => Some(
            v.as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| schema_err(format!("{path}.amount"), "must be i32"))?,
        ),
        None => None,
    };
    let times: Option<i32> = match extract_optional(raw, "times") {
        Some(v) => Some(
            v.as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| schema_err(format!("{path}.times"), "must be i32"))?,
        ),
        None => None,
    };
    // Optional `label` field — Sts2Headless does not currently emit it
    // (verified 2026-05-15 against `RunSimulator.cs:2042-2064`), but
    // older mod variants and the in-game STS2MCP build can. Used purely
    // as a salvage path when `amount` is missing/0. Bead `sts2-rs-bpt4`.
    let label: Option<String> = match extract_optional(raw, "label") {
        Some(v) => Some(
            v.as_str()
                .ok_or_else(|| schema_err(format!("{path}.label"), "must be a string"))?,
        ),
        None => None,
    };

    // Salvage `(damage, hits)` from the label when the wire amount/times
    // are absent. Only consulted for Attack intents — non-Attack intents
    // don't carry damage at this pin, so a missing `amount` isn't a
    // gap to fill. `extract_label_damage` returns `Some((dmg, hits))`
    // when the label has at least one parseable integer.
    let salvaged: Option<(i32, i32)> =
        if matches!(kind_str.as_str(), "Attack") && (amount.is_none() || amount == Some(0)) {
            label.as_deref().and_then(extract_label_damage)
        } else {
            None
        };

    let damage = amount
        .filter(|&a| a != 0)
        .or_else(|| salvaged.map(|(d, _)| d))
        .unwrap_or(0);
    let hits = times
        .or_else(|| salvaged.map(|(_, h)| h))
        .unwrap_or(1)
        .max(1);

    Ok(Some(match kind_str.as_str() {
        "Attack" => {
            // Flag degraded quality: an Attack/MultiAttack whose final
            // damage is 0 will resolve as a no-op in MCTS rollouts and
            // bias the continuous Q toward "safe". Downstream consumers
            // (CLI `--mcts-log`) surface this so the win/loss split
            // can be discounted.
            if damage == 0 {
                *damage_known = false;
            }
            if hits > 1 {
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                Intent::MultiAttack {
                    damage,
                    hits: hits.min(255) as u8,
                }
            } else {
                Intent::Attack { damage }
            }
        }
        "Defend" => Intent::Defend { block: 0 },
        "Buff" => Intent::Buff {
            kind: PowerKind::Strength,
            amount: 0,
        },
        "Debuff" => Intent::Debuff {
            kind: PowerKind::Weak,
            amount: 0,
        },
        "DebuffStrong" => Intent::DebuffStrong,
        "Sleep" => Intent::Sleep,
        "Stun" => Intent::Stun,
        "StatusCard" => Intent::StatusCard,
        "Escape" => Intent::Escape,
        "Summon" => Intent::Summon,
        "Heal" => Intent::Heal,
        "DeathBlow" => {
            // Same degraded-quality flag as `Attack`: a DeathBlow is an
            // attack telegraph, so a 0-damage one would rollout as a
            // no-op and bias the continuous Q toward "safe".
            if damage == 0 {
                *damage_known = false;
            }
            Intent::DeathBlow { damage }
        }
        // Wire kinds with no 1-to-1 Rust enum at this pin. `Hidden` and
        // `Unknown` are the game's own "you may not see this" telegraphs,
        // so collapsing them loses nothing; `CardDebuff` has no variant
        // yet. Surfacing as `Unknown` keeps the enemy turn ticking rather
        // than sitting inert (the bug `2rp7` fixes).
        "CardDebuff" | "Hidden" | "Unknown" => Intent::Unknown,
        other => {
            return Err(schema_err(
                format!("{path}.kind"),
                &format!("unknown intent kind {other:?}"),
            ));
        }
    }))
}

// ---------------------------------------------------------------------------
// Reverse-name lookups (linear scan over small tables)
// ---------------------------------------------------------------------------

pub fn relic_kind_from_python_name(s: &str) -> Option<RelicKind> {
    // `RELIC_KINDS` now aliases the macro-derived `RelicKind::ALL`, so the
    // primary python-name scan covers every implemented variant. The
    // fallback via the run-side resolver (`relic_id_from_str`,
    // case/underscore-insensitive → `RelicId` → `to_relic_kind()`) is kept
    // for lenient spellings that aren't exact `UPPER_SNAKE` python names.
    RELIC_KINDS
        .iter()
        .copied()
        .find(|k| k.python_name() == s)
        .or_else(|| {
            sts2_content::names::relic_id_from_str(s).and_then(sts2_core::RelicId::to_relic_kind)
        })
}

/// Convert `PascalCase` (or already-`UPPER_SNAKE`) to canonical
/// `UPPER_SNAKE` form — `BowlbugRock` → `BOWLBUG_ROCK`,
/// `KinPriest` → `KIN_PRIEST`, `EYE_WITH_TEETH` → `EYE_WITH_TEETH`.
/// Lowercase letters that follow an uppercase letter stay attached;
/// each uppercase boundary that follows a lowercase letter or digit
/// gets an underscore prefix. Pure ASCII; non-ASCII chars pass
/// through unchanged.
fn pascal_to_upper_snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut prev_was_lower_or_digit = false;
    for ch in s.chars() {
        if ch.is_ascii_uppercase() && prev_was_lower_or_digit {
            out.push('_');
        }
        out.push(ch.to_ascii_uppercase());
        prev_was_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
    }
    out
}

pub fn monster_kind_from_python_name(s: &str) -> Option<MonsterKind> {
    // Try direct PascalCase match first (legacy callers / tests use
    // `MonsterKind::python_name()` which still returns PascalCase).
    // Fall back to UPPER_SNAKE matching against the Python pin's
    // `Creature(monster_id=...)` form — sts2log fixtures emit that
    // form, and the prior strict match dropped 532 of 686 fixtures
    // into the `unknown monster kind` bucket (see
    // `docs/reality-checks/2026-05-16-sts2log-content-gap-report.md`).
    if let Some(k) = MONSTER_KINDS.iter().copied().find(|k| k.python_name() == s) {
        return Some(k);
    }
    let upper = pascal_to_upper_snake(s);
    MONSTER_KINDS
        .iter()
        .copied()
        .find(|k| pascal_to_upper_snake(k.python_name()) == upper)
}

pub fn power_kind_from_python_name(s: &str) -> Option<PowerKind> {
    PowerKind::all()
        .iter()
        .copied()
        .find(|k| k.python_name() == s)
}

pub fn card_id_from_python_name(s: &str) -> Option<CardId> {
    // Direct match against `CardId::to_python_name()` first. Then
    // retry against the `_CARD`-stripped form so sts2log fixtures
    // (e.g. `RUPTURE`, `VICIOUS`) resolve against the canonical
    // `RUPTURE_CARD` / `VICIOUS_CARD` ids. Mirrors the same fallback
    // wired into `names::card_id_from_str` via `sts2-rs-u5zc`.
    if let Some(c) = CardId::all()
        .iter()
        .copied()
        .find(|k| k.to_python_name() == s)
    {
        return Some(c);
    }
    CardId::all().iter().copied().find(|k| {
        let canonical = k.to_python_name();
        canonical.strip_suffix("_CARD") == Some(s)
    })
}

pub fn potion_kind_from_python_name(s: &str) -> Option<PotionKind> {
    PotionKind::ALL
        .iter()
        .copied()
        .find(|k| k.python_name() == s)
}

fn orb_kind_from_python_name(s: &str) -> Option<OrbKind> {
    // Case-insensitive: the wire has two producers with different casing.
    // The bridge/adapter path sends the parser's canonical UPPER names
    // ("PLASMA"), but `state_to_dict` emits the C#-parity mixed-case
    // display names ("Plasma") for the workbench — so a round-trip of any
    // state with a channelled orb failed parse until 2026-07-12 (rung-3-PPO
    // seed 1019: `$.player.orbs[0]: Plasma`).
    ORB_KINDS
        .iter()
        .copied()
        .find(|k| orb_python_name(*k).eq_ignore_ascii_case(s))
}

/// Plain-Rust counterpart to [`list_registry_names_impl`] usable outside
/// a `PyO3` context (e.g. from the `dump_state_schema` example which runs
/// without an embedded Python interpreter). Same sort order and set of
/// accepted `kind` discriminants.
pub fn registry_names_for(kind: &str) -> Result<Vec<String>, String> {
    // The PPO registry kinds (card/potion/event/relic_id/boss/enchantment)
    // are single-sourced in `sts2_vocab` (Task 4, 2026-07-07
    // ppo-rust-inference plan) — same `BTreeSet`-sorted `python_name`
    // iteration this function used to do locally. `relic`/`monster`/
    // `power`/`orb` stay local: they have no PPO-vocab analogue.
    if let Some(names) = sts2_vocab::registry_names(kind) {
        return Ok(names);
    }
    let mut set: BTreeSet<&'static str> = BTreeSet::new();
    match kind {
        "relic" => {
            for k in RELIC_KINDS {
                set.insert(k.python_name());
            }
        }
        "monster" => {
            for k in MONSTER_KINDS {
                set.insert(k.python_name());
            }
        }
        "power" => {
            for k in PowerKind::all() {
                set.insert(k.python_name());
            }
        }
        "orb" => {
            for k in ORB_KINDS {
                set.insert(orb_python_name(*k));
            }
        }
        other => {
            return Err(format!(
                "unknown registry kind `{other}` (expected one of: relic, relic_id, card, monster, power, potion, orb, event, boss, enchantment)"
            ));
        }
    }
    Ok(set.into_iter().map(String::from).collect())
}
// ---------------------------------------------------------------------------
// state_from_dict — ParsedState → CoreCombatState (bead sts2-rs-osyf, T2)
// ---------------------------------------------------------------------------
//
// Translates a [`ParsedState`] (output of [`parse_state_dict`]) into a
// fully-initialised [`CoreCombatState`]. The plan
// (`docs/plans/2026-05-14-integrate-sts2-rs-mcts-hint-system-into-projects-sts2-sts2-c.md`,
// §T2) requires the following populated:
//
//   * the player Creature record linked via `player.creature_uid`;
//   * each monster Creature (HP, block, kind, intent_history);
//   * deck piles wired into `state.cards` with stable CardUids;
//   * player + per-creature powers in `state.powers`;
//   * relics in `state.relics` (with optional `counter` carried over);
//   * `ascension` / `is_elite`;
//   * best-effort `master_deck` (the optional wire field — when missing
//     we leave the field empty; documented in `state_from_dict`'s
//     doc-comment as a known-stale slot);
//   * RNG re-seeded from `state.rng_seed + state.turn` (NOT the in-process
//     RNG cursor — see plan decisions table).
//
// Mid-resolution slots (`active_attack`, `pending_choice`, `pending_play`)
// are not modelled at all — `parse_state_dict` already rejects
// non-`None` values for those.

fn character_id_from_class(class: &str) -> Option<CharacterId> {
    match class {
        "ironclad" => Some(CharacterId::Ironclad),
        "silent" => Some(CharacterId::Silent),
        "defect" => Some(CharacterId::Defect),
        "necrobinder" => Some(CharacterId::Necrobinder),
        "regent" => Some(CharacterId::Regent),
        _ => None,
    }
}

fn combat_phase_from_str(s: &str) -> CombatPhase {
    // Wire schema collapses several Rust-side phase variants into one
    // string (the wire format predates the full enum split). Map the
    // observed strings to the closest Rust variant; T2's job is to
    // build a state that callers can usefully consult with
    // `legal_actions()`, not to round-trip the phase enum bit-perfect.
    // The wildcard arm and the "PlayerPlayPhase" arm intentionally
    // share a body — parse_state_dict has already rejected unknown
    // strings, so the wildcard is a defensive belt-and-braces.
    match s {
        "Setup" => CombatPhase::Setup,
        "PlayerDraw" => CombatPhase::PlayerTurnStart,
        "EnemyPhase" => CombatPhase::EnemyTurnExec,
        "EndOfTurn" => CombatPhase::PlayerEndTurnPhaseOne,
        "Victory" => CombatPhase::Resolved { player_won: true },
        "Defeat" => CombatPhase::Resolved { player_won: false },
        // "PlayerPlayPhase" + unreachable fallback.
        _ => CombatPhase::PlayerPlayPhase,
    }
}

fn side_from_str(s: &str) -> Side {
    match s {
        "Enemy" => Side::Enemy,
        // "Player" + fallback (parse_state_dict has already rejected
        // any other value).
        _ => Side::Player,
    }
}

/// Build a runtime [`Card`] POD from a [`ParsedCard`]. For status / curse
/// cards whose `card_id` could not be resolved at parse time (the
/// `status:Burn` synthetic-id path), we fall back to a placeholder
/// `CardId::AscendersBane` curse and stamp `card_type = Curse`. The
/// hint adapter rarely needs status piles to be byte-faithful — they
/// matter only for hand legality at mid-turn, and status cards never
/// appear in the draw pile in a snapshot the hint engine consults.
///
/// Bead `sts2-rs-osyf` (T2). The status-card path is intentionally
/// loose so unmodelled card flavors don't crash the adapter.
fn build_card(parsed: &ParsedCard, uid: CardUid, path: &str) -> WireResult<Card> {
    // Resolve `card_id` → CardSpec via the content registry. Falls back
    // to a synthetic AscendersBane curse for status/curse paths whose
    // raw_id doesn't resolve (e.g. `status:Burn`).
    let (card_id, spec) = if let Some(cid) = parsed.card_id {
        let spec = resolve_card_name(cid.to_python_name(), None).map_err(|e| {
            unsupported(
                path,
                &format!("no spec for `{}`: {:?}", cid.to_python_name(), e),
            )
        })?;
        (cid, spec)
    } else {
        // status:* / curse:* synthetic id — fall back to AscendersBane
        // so the arena has *some* runtime spec. The card_id field is
        // not load-bearing for legality of non-hand piles.
        let spec = resolve_card_name("AscendersBane", None).map_err(|e| {
            unsupported(
                path,
                &format!("status/curse fallback `AscendersBane` did not resolve: {e:?}"),
            )
        })?;
        (CardId::AscendersBane, spec)
    };
    // The factory's UPGRADED build, fetched once for every `upgraded: true`
    // card. It serves two fallbacks: the keyword override below (historical),
    // and — since the live-bridge dialect ships NO `stats` block — the base
    // stat/cost defaults. Without the stat fallback an upgraded card from a
    // stats-less wire reconstructed at its UNUPGRADED registry values
    // (Strike+ 9 -> 6, Defend+ 8 -> 5): on the live path only, since the DLL
    // wire always carries `stats`. That was the mirror_live2
    // `$.player.block` class — the world exactly +3 over the mirror
    // (the Defend/Colossus upgrade delta) on 14 of 17 records. Explicit wire
    // stats keep precedence, so DLL wires and native round-trips are
    // unchanged.
    let upgraded_build = if parsed.upgraded {
        sts2_core::lookup_card_factory().and_then(|f| f(card_id, true))
    } else {
        None
    };
    // `base_cost` is the instance's own cost; `cost_override` is that cost
    // with every transient modifier already applied. Prefer the base, or a
    // this-turn discount reconstructs as a PERMANENT one — the modifiers are
    // restored separately via `combat_vars` below and re-applied by
    // `effective_card_cost`, which is also what expires them.
    //
    // X-cost guard: for a card whose registry cost is -1 (`HasEnergyCostX`
    // in C#, e.g. Cascade/Whirlwind), the live bridge's resolved `cost` is
    // the CURRENT ENERGY — a moment-to-moment display, not an instance cost
    // (C# `Cascade.cs: HasEnergyCostX => true`, base cost -1; the X shown on
    // the card tracks the energy pool). Adopting it froze the display into
    // `Card.cost`, so the mirror re-emitted a stale X for the rest of the
    // fight — 137 of the 421 live1-unexplained records were CASCADE alone,
    // always exactly one action behind the world's energy. Only the
    // base_cost-less dialect is affected: engine/DLL wires carry
    // `base_cost: -1` for X-cost cards and keep taking the first arm.
    let registry_x_cost = spec.cost == -1 || upgraded_build.as_ref().is_some_and(|b| b.cost == -1);
    let cost = match (parsed.base_cost, parsed.cost_override) {
        (Some(c), _) => i8::try_from(c).unwrap_or(spec.cost),
        (None, Some(_)) if registry_x_cost => -1,
        (None, Some(c)) => i8::try_from(c).unwrap_or(spec.cost),
        (None, None) => upgraded_build.as_ref().map_or(spec.cost, |b| b.cost),
    };
    let mut card = Card {
        uid,
        card_id,
        cost,
        card_type: spec.card_type,
        target_type: spec.target_type,
        upgraded: parsed.upgraded,
        // Prefer the wire's explicit base stats (present for an upgraded /
        // enchanted card whose current base differs from the unupgraded
        // registry spec); then the upgraded factory build for an
        // `upgraded: true` card from a stats-less wire (the live bridge);
        // the unupgraded registry spec last. `resolve_card_name` only ever
        // returns the UNUPGRADED spec, so without these an upgraded Strike
        // reconstructs at 6 damage instead of 9 (2026-07-15 audit).
        base_damage: parsed.base_damage.unwrap_or_else(|| {
            upgraded_build
                .as_ref()
                .map_or(spec.base_damage, |b| b.base_damage)
        }),
        base_block: parsed.base_block.unwrap_or_else(|| {
            upgraded_build
                .as_ref()
                .map_or(spec.base_block, |b| b.base_block)
        }),
        combat_vars: parsed.combat_vars.iter().copied().collect(),
        // Filled from `parsed.enchantment` right below — the constructor
        // cannot, because an unknown spelling has to fail loud rather than
        // reconstruct a differently-enchanted card.
        enchantments: sts2_core::cards::CardEnchantments::new(),
        // Canonical tags for the resolved id, exactly as `card_from_spec`
        // and the scenario builder assign them. `CardTags::NONE` here made
        // every reconstructed card untagged, so tag-gated handlers silently
        // read an empty deck: PerfectedStrike counted ZERO Strikes and dealt
        // its 6 base instead of 6 + 2·n (found by the Phase-1 mirror oracle,
        // Ironclad seed 3 — 4 damage where the native engine dealt 12), and
        // FastenPower's Defend-sourced block bonus never applied.
        tags: sts2_content::cards::status::tags_for_card_id(card_id),
        energy_spent: parsed.energy_spent,
        base_replay_count: parsed.replay_count,
        keyword_override: None,
        affliction: None,
    };
    // Run-persistent enchantments. Parsed since the wire gained the field but
    // never APPLIED until 2026-08-01, so every reconstructed card came back
    // plain: a Nimble Defend blocked its base 5 where the real game blocked 7
    // (`enchant_block_additive`), a Sharp attack hit for its base, and Steady's
    // Retain went missing. That is a per-play error the search plans on, and it
    // is what the DLL mirror kept reporting as `$.player.block` desyncs.
    //
    // `set_enchantment` only — NOT `apply_static_enchantment`. The static arms
    // mutate the card (TezcatarasEmber zeroes `cost`, the keyword arms fold
    // into the effective set) and the producer already sent us the post-enchant
    // card, so re-running them would apply the same edit twice. The stack
    // itself is all the dynamic hooks and `effective_keywords` need, and both
    // read it as a pure function of the card.
    if let Some(name) = parsed.enchantment.as_deref() {
        let id = EnchantmentId::from_wire_name(name)
            .ok_or_else(|| unsupported(path, &format!("unknown enchantment `{name}`")))?;
        // Absent amount means one stack; see `ParsedCard::enchantment_amount`.
        card.set_enchantment(id, parsed.enchantment_amount.unwrap_or(1));
    }

    // Upgrade-aware keyword set. `resolve_card_name` returns the UNUPGRADED
    // spec and only a LIVE `Card::upgrade` stamps `keyword_override` from the
    // upgrade delta, so a rebuilt `upgraded: true` card silently reverted to
    // the base keyword set for the ~27 cards whose keywords change on
    // upgrade: a reconstructed Discovery+ kept EXHAUST (11 mirror-only
    // exhaust events on the v28wr sweep), an Aggression+ lost INNATE, a
    // BurningPact+ kept EXHAUST. Adopt the factory's upgraded build's
    // override, exactly as `Card::upgrade` does (cards with no keyword delta
    // leave it `None` and keep deferring to the registries).
    if let Some(build) = upgraded_build.as_ref() {
        if build.keyword_override.is_some() {
            card.keyword_override = build.keyword_override;
        }
    }

    // Wire card flags. Parsed and validated since the field landed, but never
    // READ — the world's GhostSeed runs ship `flags: ["ethereal"]` on every
    // basic Strike/Defend and the reconstruction dropped it, so the mirror
    // discarded at end of turn what the world exhausted (27 of the v28wr
    // card-flow records, 1:1 with the flag). `ethereal` maps to the same
    // per-instance var the engine's own GhostSeed arm stamps; the free flags
    // map to their cost riders; `unplayable`/`innate` fold into the keyword
    // override on top of the effective base set. `retain` is the card's
    // Retain KEYWORD (intrinsic, or granted for good by `AddKeyword`:
    // Snap, Choices Paradox, Steady's enchantment) and folds into the
    // permanent override like `innate`. It used to be left out because a
    // producer once sent the this-turn grant as `retain`; the wire now tells
    // the two apart (GH #370's `retain_turn`, seeded into the per-turn uid
    // list at the pile walk, where it expires on schedule), so a granted
    // keyword Retain no longer has to be dropped (sts2-agent fork, A4d).
    // `sly` is handled at the pile walk, not here.
    if !parsed.flags.is_empty() {
        let fold = |card: &mut Card, bit: sts2_core::cards::KeywordOverride| {
            let base = card
                .keyword_override
                .unwrap_or_else(|| sts2_core::card_effects::registry_keyword_override(card_id));
            card.keyword_override = Some(sts2_core::cards::KeywordOverride(base.0 | bit.0));
        };
        for f in &parsed.flags {
            match f.as_str() {
                "ethereal" => card.set_combat_var(VarKey::EtherealThisCombat, 1),
                "free_turn" => card.set_combat_var(VarKey::FreeThisTurn, 1),
                "free_once" => card.set_combat_var(VarKey::FreeUntilPlayed, 1),
                "unplayable" => fold(&mut card, sts2_core::cards::KeywordOverride::UNPLAYABLE),
                "innate" => fold(&mut card, sts2_core::cards::KeywordOverride::INNATE),
                "retain" => fold(&mut card, sts2_core::cards::KeywordOverride::RETAIN),
                _ => {}
            }
        }
    }

    // MadScience is the one card whose `CardType` is per-INSTANCE: C#
    // `MadScience.cs:80` `Type => TinkerTimeType` and `:82-92`
    // `TargetType => TinkerTimeType != Attack ? Self : AnyEnemy`, with
    // `OnPlay` (`:189-214`) branching Attack / Skill / Power and the Power arm
    // applying the Expertise / Curious / Improvement rider. The engine keeps
    // that variant packed in `VarKey::Generic`.
    //
    // Two ingest dialects:
    //  * the engine's own `state_to_dict` ships `combat_vars.generic`, so the
    //    variant is already in `card.combat_vars` by here;
    //  * the live bridge ships neither — only the card's `type`. Without a
    //    seed the reconstruction defaulted to the spec (Attack / no rider) and
    //    the mirror played the WRONG BRANCH: idx 61 dealt 7 damage and applied
    //    nothing where the world applied CURIOUS 1 and dealt 0, and likewise
    //    idx 81 / 90 / 232 / 16 (the last two IMPROVEMENT).
    //
    // Seed the mode from the wire's `type` when nothing packed arrived, then
    // re-stamp `card_type` / `target_type` off the resolved variant in BOTH
    // dialects — the registry spec is the default-constructed Attack build, so
    // even a native round-trip previously came back typed Attack and was
    // invisible to every card-type-gated surface (CuriousPower's own Power
    // discount, targeting legality).
    //
    // The RIDER still cannot be recovered from any live producer. It is a
    // public `[SavedProperty]` on the C# side (`MadScience.cs:125-137`
    // `TinkerTimeRider`), so the mod only has to put its enum name on the Card
    // object as `tinker_time_rider` (`"Curious"`, `"Improvement"`, …) and the
    // `tinker_time_rider` key parsed above picks it up with no further change
    // here. Until then a live Power-mode MadScience reconstructs with the
    // correct type and cost but no rider effect.
    if card_id == CardId::MadScience {
        use sts2_content::cards::colorless::{
            mad_science_variant_of, set_mad_science_variant, TinkerTimeRider,
        };
        let packed = card.combat_var(VarKey::Generic);
        let (mode, rider) = if packed == 0 {
            let mode = match parsed.card_type.as_deref() {
                Some("skill") => sts2_core::cards::CardType::Skill,
                Some("power") => sts2_core::cards::CardType::Power,
                _ => sts2_core::cards::CardType::Attack,
            };
            let rider = parsed
                .tinker_time_rider
                .as_deref()
                .and_then(TinkerTimeRider::from_wire_name)
                .unwrap_or(TinkerTimeRider::None);
            (mode, rider)
        } else {
            mad_science_variant_of(&card)
        };
        set_mad_science_variant(&mut card, mode, rider);
    }

    // Bank the part of `cost` that a this-turn modifier is responsible for,
    // so the turn-start restore can expire it on schedule. The producer sends
    // the card's current cost and what that cost becomes once the turn's
    // modifiers wear off; the difference is the transient shave, and without
    // banking it a Stomp rebuilt mid-turn keeps its per-Attack discount for
    // the rest of the fight.
    //
    // The engine's own per-effect banks describe the same fact exactly, so a
    // producer carrying those must not also send `persistent_cost` — two
    // representations of one shave would restore it twice. They are mutually
    // exclusive by construction (only the C# dialects send `persistent_cost`,
    // only `state_to_dict` sends the banks), so disagreement is a schema
    // error rather than something to reconcile.
    if let Some(persistent) = parsed.persistent_cost {
        const ENGINE_OWNED: [VarKey; 3] = [
            VarKey::StompCostReductionThisTurn,
            VarKey::PinpointCostReductionThisTurn,
            VarKey::SneckoCostDeltaThisTurn,
        ];
        if let Some(k) = ENGINE_OWNED
            .iter()
            .find(|k| card.combat_var(**k) != 0)
            .copied()
        {
            return Err(schema_err(
                path,
                &format!(
                    "`persistent_cost` and the engine-owned `{}` bank both describe this \
                     turn's cost modifiers; a producer must send one or the other",
                    var_key_wire_name(k)
                ),
            ));
        }
        let shave = persistent - i32::from(cost);
        if shave != 0 && cost >= 0 {
            card.set_combat_var(VarKey::WireTransientCostThisTurn, shave);
        }
    }

    // The other transient edge. A "this turn" discount is really "this turn
    // OR until played" (C# `SetThisTurnOrUntilPlayed`), so restoring it only
    // at the turn boundary leaves it alive on the card that just spent it.
    if let Some(played) = parsed.played_cost {
        let shave = played - i32::from(cost);
        if shave != 0 && cost >= 0 {
            card.set_combat_var(VarKey::WireCostExpiringOnPlay, shave);
        }
    }
    // Only meaningful alongside a bank, and only then is it read. Note the
    // ambiguity that DOES exist runs the other way: `combat_var` defaults to 0,
    // so an absent `stable_cost` would read back as a legitimate-looking
    // `stable == 0`. `parse_card`'s all-or-nothing gate on the trio is what
    // makes that unreachable, not this condition.
    if let Some(stable) = parsed.stable_cost {
        if card.combat_var(VarKey::WireTransientCostThisTurn) != 0
            || card.combat_var(VarKey::WireCostExpiringOnPlay) != 0
        {
            card.set_combat_var(VarKey::WireCostStable, stable);
        }
    }

    // Restore a Wither's FakeUpgrade level from the wire's base damage.
    // C# `Wither.cs` grows its `DamageVar` +3 per `FakeUpgrade()`; the level
    // itself is never on the wire, and the Rust turn-end arm computes damage
    // ONLY from `VarKey::WitherFakeUpgradeLevel` (ignoring `base_damage`).
    // Without this every reconstructed Wither chipped the base 3 while the
    // real card hits 3+3L — seed-1003 trace: level-2/3 Withers (9-12 real)
    // cycling through a <=25-hp hand (2026-07-17 Aeonglass audit).
    if card_id == CardId::Wither {
        if let Some(d) = parsed.base_damage {
            if d > 3 {
                card.set_combat_var(VarKey::WitherFakeUpgradeLevel, (d - 3) / 3);
            }
        }
    }
    if let Some(aff) = parsed.affliction.as_deref() {
        // parse_card already validated the wire name.
        let kind = card_affliction_kind_from_wire(aff)
            .ok_or_else(|| unsupported(format!("{path}.affliction"), aff))?;
        let applied = card.set_affliction(sts2_core::card_afflictions::CardAffliction {
            kind,
            stacks: parsed.affliction_stacks,
            applied_exhaust: parsed.affliction_applied_exhaust,
        });
        // A faithful wire can't carry an affliction the card type
        // rejects (C# gated the application on CanAfflictCardType);
        // fail loud rather than silently planning without it.
        if !applied {
            return Err(schema_err(
                path,
                &format!(
                    "affliction `{aff}` not allowed on card type {:?}",
                    card.card_type
                ),
            ));
        }
    }
    Ok(card)
}

/// Per-monster aliases from the C# wire's `MoveState` id (what the
/// live-game bridge puts in `move_state`) to this engine's AI state id,
/// for the cases [`MonsterAi::force_current_state_by_name_relaxed`]'s
/// generic `_MOVE`-segment stripping cannot bridge. Two origins, both
/// verified against the v0.107.1 decomp (2026-07-13 sweep-8 audit):
///
/// * The Rust port named the state after the C# *handler method* rather
///   than the C# state id — e.g. C# `MoveState("POKEY_POUNCE_MOVE",
///   ClumpShotMove, …)` became Rust `"CLUMP_SHOT"`, C# `"EXPEL_MOVE"`
///   (handler `ExpelBlastMove`) became `"EXPEL_BLAST"`.
/// * C#'s synthetic interrupt-stun (`Creature.StunInternal` injects
///   `MoveState("STUNNED")`) mapped onto a monster that models its stun
///   as a REAL graph state (`DIZZY`, `STUN_MOVE`, `WAKE_UP_MOVE`).
///
/// Keep entries alphabetical by kind. The
/// `wire_move_state_aliases_resolve_on_their_ais` test asserts every
/// target id still exists on the freshly-built AI, so a content-side
/// rename fails loudly here instead of silently reviving the
/// inert-enemy bug.
const WIRE_MOVE_STATE_ALIASES: &[(MonsterKind, &str, &str)] = &[
    (MonsterKind::BowlbugRock, "STUNNED", "DIZZY"),
    // C# initial "SLEEP_MOVE" follows to WAKE (+10 Strength). The relaxed
    // matcher strips `_MOVE` and lands on engine "SLEEP" — which is C#'s
    // *other* sleep state (SLEEP_MOVE_2, follows straight to SLASHES), so a
    // reconstructed sleeping Effigy silently skipped its wake buff and MCTS
    // planned against a 13-damage slasher instead of a 23-damage one
    // (2026-07-14 fight-playout probe: reconstructed cost 2.8 HP vs 20.6
    // native / 21.5 real).
    (MonsterKind::BygoneEffigy, "SLEEP_MOVE", "INITIAL_SLEEP"),
    (MonsterKind::CeremonialBeast, "STUNNED", "STUN"),
    (MonsterKind::CubexConstruct, "EXPEL_MOVE", "EXPEL_BLAST"),
    (
        MonsterKind::CubexConstruct,
        "REPEATER_BLAST_MOVE",
        "REPEATER",
    ),
    (
        MonsterKind::CubexConstruct,
        "REPEATER_BLAST_MOVE_2",
        "REPEATER_2",
    ),
    (MonsterKind::Exoskeleton, "MANDIBLES_MOVE", "MANDIBLE"),
    (MonsterKind::LeafSlimeS, "TACKLE_MOVE", "BUTT"),
    // C# "POWER_SHIELD_MOVE"; the Rust port names the one-time opener
    // "FIRST_POWER_SHIELD", unreachable by the `_MOVE`-strip matcher — the
    // AI was dropped entirely, losing both the +5 secondary block and every
    // intent roll (8 events on the v27wr sweep, mirror `move_state: null`).
    (
        MonsterKind::MagiKnight,
        "POWER_SHIELD_MOVE",
        "FIRST_POWER_SHIELD",
    ),
    (MonsterKind::Queen, "YOU_ARE_MINE_MOVE", "YOURE_MINE_MOVE"),
    (MonsterKind::ShrinkerBeetle, "SHRINKER_MOVE", "SHRINK"),
    // C# "THWACK" — the Rust port typo'd the state id as "TWACK".
    (MonsterKind::SlitheringStrangler, "THWACK", "TWACK"),
    (MonsterKind::SlumberingBeetle, "STUNNED", "WAKE_UP_MOVE"),
    (MonsterKind::SpinyToad, "PROTRUDING_SPIKES_MOVE", "SPINES"),
    (MonsterKind::SpinyToad, "SPIKE_EXPLOSION_MOVE", "EXPLOSION"),
    (MonsterKind::SpinyToad, "TONGUE_LASH_MOVE", "LASH"),
    (MonsterKind::TerrorEel, "STUNNED", "STUN_MOVE"),
    (MonsterKind::TerrorEel, "THRASH_MOVE", "ThrashMove"),
    (MonsterKind::TheInsatiable, "THRASH_MOVE", "THRASH_1"),
    (MonsterKind::TorchHeadAmalgam, "TACKLE_MOVE", "TACKLE_1"),
    (MonsterKind::Tunneler, "BELOW_MOVE", "BELOW_MOVE_1"),
    (MonsterKind::Tunneler, "STUNNED", "DIZZY_MOVE"),
    (MonsterKind::TurretOperator, "UNLOAD_MOVE", "UNLOAD_MOVE_1"),
    (MonsterKind::TwigSlimeM, "POKEY_POUNCE_MOVE", "CLUMP_SHOT"),
    (MonsterKind::TwigSlimeS, "TACKLE_MOVE", "BUTT"),
];

/// Look up a [`WIRE_MOVE_STATE_ALIASES`] entry for this monster + wire id.
fn wire_move_state_alias(kind: MonsterKind, wire_id: &str) -> Option<&'static str> {
    WIRE_MOVE_STATE_ALIASES
        .iter()
        .find(|(k, w, _)| *k == kind && *w == wire_id)
        .map(|(_, _, engine_id)| *engine_id)
}

/// Invert one wire attack-intent amount from UI-EFFECTIVE back to BASE.
///
/// The wire dialects (SpireMentor and Sts2Headless) carry the number the
/// game SHOWS the player — C#'s intent display already folds in the
/// attacker's Strength/Weak/Shrink and the target's Vulnerable. The
/// engine's `Creature.intent` semantics are BASE amounts:
/// `perform_intent` pushes them through the full damage pipeline
/// (`hooks::modify_damage`), which applies those same modifiers.
/// Ingesting the effective number verbatim therefore DOUBLE-COUNTS every
/// modifier active at snapshot time. Measured on the 2026-07-13 rung-3.5
/// fight-replay audit: +50% incoming-damage over-prediction on every
/// turn the player was Vulnerable (Flyconid SMASH base 11 → wire 16 →
/// engine dealt 24 = 16×1.5), and a per-monster positive bias tracking
/// self-Strengthening enemies (Kin Priest, Bygone Effigy).
///
/// Forward chain (mirrors `hooks::modify_damage` for the monster-intent
/// path): `floor((base + strength) * mul)` with a SINGLE flooring
/// `div_euclid` at the end (`DamageMul::apply`, bead 58w). We invert by
/// solving for the smallest `base + strength` whose forward result
/// reproduces the wire amount exactly; if no integer does (an unmodelled
/// modifier such as Cruelty shifted the multiplier), fall back to the
/// nearest approximation — still strictly closer to the truth than the
/// raw effective number.
fn unbake_intent_damage(eff: i32, dealer_strength: i32, mul_num: i64, mul_den: i64) -> i32 {
    if eff <= 0 {
        return eff;
    }
    let eff64 = i64::from(eff);
    // Smallest x with (x * mul_num).div_euclid(mul_den) == eff, i.e.
    // x >= ceil(eff * mul_den / mul_num).
    let x = (eff64 * mul_den + mul_num - 1).div_euclid(mul_num);
    let x = if (x * mul_num).div_euclid(mul_den) == eff64 {
        x
    } else {
        // No exact preimage under our modelled chain — nearest match.
        (eff64 * mul_den + mul_num / 2).div_euclid(mul_num)
    };
    #[allow(clippy::cast_possible_truncation)]
    let base = (x - i64::from(dealer_strength)) as i32;
    base.max(0)
}

/// Which damage-carrying telegraph [`unbake_wire_intent`] is inverting, so it
/// can rebuild the same variant it took apart.
#[derive(Debug, Clone, Copy)]
enum AttackShape {
    Single,
    Multi(u8),
    /// `Intent::DeathBlow` — an attack whose move body also kills the dealer.
    DeathBlow,
}

/// The BASE magnitude the RECONSTRUCTED AI would telegraph for the move the
/// wire says this enemy is parked on — the second, independent observation
/// [`unbake_wire_intent`] uses to break truncation ties.
///
/// Read off the SAME object the forward engine reads
/// (`combat_loop::roll_next_enemy_intents` → `ai.current_move().intents[0]`),
/// which is why it must be the reconstructed AI and not a freshly built one
/// ([`wire_base_intent`]): reconstruction is where the spawn payload and the
/// mid-fight counters land (Wriggler slot, TestSubject `Adaptable` /
/// `ExtraMultiClawCount`, Aeonglass `_witherUpgradeCount`), and a fresh AI
/// silently drops all of them.
///
/// Only the per-hit DAMAGE is returned, and only when the wire and the move
/// table agree on the attack VARIANT. The wire's hit count is never touched:
/// TestSubject's MULTI_CLAW escalation lives in the hit count (C#
/// `MultiClawTotalCount = BaseMultiClawCount + ExtraMultiClawCount`, modelled
/// here as the owner's PainfulStabs counter and added at telegraph time, NOT
/// stored in the move table), so a move-table `hits` would be stale by
/// construction while its `damage` is exact.
fn reconstructed_base_damage(ai: Option<&MonsterAi>, wire: Intent) -> Option<i32> {
    let engine = ai?.current_move_opt()?.intents.first().copied()?;
    match (wire, engine) {
        (Intent::Attack { .. }, Intent::Attack { damage })
        | (Intent::MultiAttack { .. }, Intent::MultiAttack { damage, .. })
        | (Intent::DeathBlow { .. }, Intent::DeathBlow { damage }) => Some(damage),
        _ => None,
    }
}

/// Apply [`unbake_intent_damage`] to an enemy's parsed intent using the
/// modifiers visible in the same wire snapshot: the enemy's own
/// Strength/Weak/Shrink and the player's Vulnerable.
///
/// `engine_base` is [`reconstructed_base_damage`] for this enemy — preferred
/// over the arithmetic inverse whenever it is CONSISTENT with the wire (see
/// the tie-break comment at the bottom of the body).
fn unbake_wire_intent(
    intent: Option<Intent>,
    enemy_powers: &[ParsedPower],
    player_powers: &[ParsedPower],
    back_attack_boost: bool,
    engine_base: Option<i32>,
) -> Option<Intent> {
    let intent = intent?;
    // `DeathBlow` belongs in here with the plain attacks, not in the pass-through
    // arm. C# `DeathBlowIntent : SingleAttackIntent : AttackIntent`
    // (`MonsterMoves.Intents/DeathBlowIntent.cs`, `SingleAttackIntent.cs`), and
    // `RunSimulator`'s serializer emits `AttackIntent.GetSingleDamage` for *any*
    // `AttackIntent` — so a DeathBlow's wire number is baked exactly like a plain
    // attack's. `perform_intent` then pushes it through the same
    // `apply_damage_to_player` as `Intent::Attack` (`combat_loop.rs`), so leaving
    // it verbatim re-applies the dealer's Strength, the player's
    // Vulnerable/Tainted and the Surrounded ×1.5 on top of numbers that already
    // include them, and the search plans against an exploder hit the game will
    // never land that hard.
    let (damage, shape) = match intent {
        Intent::Attack { damage } => (damage, AttackShape::Single),
        Intent::MultiAttack { damage, hits } => (damage, AttackShape::Multi(hits)),
        Intent::DeathBlow { damage } => (damage, AttackShape::DeathBlow),
        other => return Some(other),
    };
    // Additive terms, in the order C# applies them: the DEALER's Strength and
    // the TARGET's Tainted, both folded in before the multiplier
    // (`TaintedPower.ModifyDamageAdditive` returns `Amount` for any powered
    // attack whose target is its owner — the player).
    //
    // Tainted is a target-side additive and was missed because everything else
    // inverted here is dealer-side. It arrives via VitalSpark, which afflicts
    // the player's Skills and applies Tainted at its own stack count when one
    // is played, so the reconstruction over-stated every InfestedPrism
    // telegraph by exactly the VitalSpark amount: on the v12 sweep the
    // world-minus-mirror damage delta equalled the VitalSpark stack in 93 of
    // its 119 intent divergences. That is not only a diff artifact — the
    // over-stated base feeds the search, which then plans against incoming
    // damage the game never intended to deal.
    //
    // The DEALER's Vigor is a third additive, baked in exactly like Strength:
    // `VigorPower.ModifyDamageAdditive` returns `Amount` for any powered attack
    // its owner deals, and at telegraph time both of its latch guards are
    // skipped (`VigorPower.cs:57-77`), so `AttackIntent.GetSingleDamage` folds
    // it into the wire number. It was missed because monster-owned Vigor is
    // rare — TerrorEel's SHRIEK line (`TerrorEel.cs:102`) is the only carrier
    // across four 12-seed live A1 corpora. Left out, the wire value was stored
    // verbatim as the base and Vigor was added a second time on the re-bake:
    // wire 22 → stored 22 → 28. All 21 records in those corpora recover the
    // move-table constant 16 once it is subtracted here.
    let strength = enemy_powers
        .iter()
        .find(|p| p.kind == PowerKind::Strength)
        .map_or(0, |p| p.amount)
        + enemy_powers
            .iter()
            .find(|p| p.kind == PowerKind::Vigor)
            .map_or(0, |p| p.amount)
        + player_powers
            .iter()
            .find(|p| p.kind == PowerKind::Tainted)
            .map_or(0, |p| p.amount);
    // `DebilitatePower` never multiplies damage itself — it DEEPENS the Weak /
    // Vulnerable multipliers of the creature it sits on (`DebilitatePower.cs:39-50`
    // makes Weak 0.75 → 0.5, `:26-37` makes Vulnerable 1.5 → 2.0), and the two
    // status powers consult it inline (`WeakPower.cs:40-44`,
    // `VulnerablePower.cs:51-55`). Both amplifications are already in the forward
    // pipeline, so omitting them here made this a non-inverse: on a Debilitated
    // dealer the recovered base came out ~2/3 of truth and every reconstruction
    // under-stated the incoming hit by a third, for the whole fight. 29 intent
    // divergences in the 2026-08-03 live Necrobinder corpus are exactly this,
    // and 22 of the 29 sat on a mirror whose telegraph was already EXACT — the
    // divergence was manufactured by the round trip.
    //
    // Presence, not `amount > 0`, matches the forward gates.
    let dealer_debilitate = enemy_powers.iter().any(|p| p.kind == PowerKind::Debilitate);
    let target_debilitate = player_powers
        .iter()
        .any(|p| p.kind == PowerKind::Debilitate);
    let mut mul = DamageMul::ONE;
    if player_powers
        .iter()
        .any(|p| p.kind == PowerKind::Vulnerable && p.amount > 0)
    {
        mul = mul.compose(if target_debilitate {
            DamageMul::DOUBLE
        } else {
            DamageMul::VULNERABLE
        });
    }
    if enemy_powers
        .iter()
        .any(|p| p.kind == PowerKind::Weak && p.amount > 0)
    {
        mul = mul.compose(if dealer_debilitate {
            DamageMul::HALF
        } else {
            DamageMul::WEAK
        });
    }
    if enemy_powers
        .iter()
        .any(|p| p.kind == PowerKind::Shrink && p.amount > 0)
    {
        mul = mul.compose(DamageMul::SHRINK);
    }
    // ColossusPower halves incoming powered attacks from a Vulnerable dealer
    // (`ColossusPower.ModifyDamageMultiplicative`, v0.107.1) and the telegraph
    // is `AttackIntent.GetSingleDamage`, which runs the full ModifyDamage
    // chain — so the ×0.5 is already baked into the wire number and must be
    // inverted here, or `perform_intent` applies it a second time and the
    // mirror plans against exactly half the real incoming damage (29 of the
    // 33 `$.player.hp` end_turn divergences on the v27wr sweep; the halved
    // hit usually vanished into block, hiding real HP loss from the search).
    // Gates mirror `hooks.rs`'s forward arm: Colossus present on the player
    // (any amount), Vulnerable on the dealer.
    if player_powers.iter().any(|p| p.kind == PowerKind::Colossus)
        && enemy_powers
            .iter()
            .any(|p| p.kind == PowerKind::Vulnerable && p.amount > 0)
    {
        mul = mul.compose(DamageMul::HALF);
    }
    // Surrounded back-attack (KaiserCrab duo): the wire telegraph bakes the
    // 1.5× for the arm behind the player. Without inverting it the searched
    // first enemy turn double-applied the multiplier (wire 18 stored as base
    // 18 → ×1.5 again = 27 vs the real 18) — 2026-07-17 Crusher+Rocket audit.
    if back_attack_boost {
        mul = mul.compose(DamageMul::new(3, 2));
    }
    if strength == 0 && mul == DamageMul::ONE {
        return Some(intent);
    }
    // TIE-BREAK, not an override. `unbake_intent_damage` inverts a FLOORED
    // forward chain, so under Weak (×3/4) roughly a third of effective values
    // have TWO integer preimages and it must guess — it takes the smallest.
    // That guess is stable only while the modifier set is; the moment the
    // player's card changes it inside the same step (applies Weak, strips
    // Strength) the mirror re-bakes from a base that is 1 low and every hit
    // of the telegraph under-states incoming damage. Seven v34wr intent
    // entries, one per Weak+Strength board, all exactly 1 low and all with
    // the true base sitting in the move table: TestSubject BIG_POUNCE 45
    // (mirror 44, ×2), Rocket LASER 31 (30), Tunneler BITE 13 (12),
    // LouseProgenitor WEB_CANNON 9 (8), ScrollOfBiting CHEW 5 (4),
    // Exoskeleton SKITTER 1 (0).
    //
    // The reconstructed AI knows the answer, so consult it — but ONLY when it
    // is a preimage of what the wire actually reported. The wire stays the
    // authority: an engine base that cannot forward-produce the wire amount
    // (mis-resolved move state, content-parity drift, an escalation the move
    // table doesn't carry) is discarded and the arithmetic inverse stands.
    // Because the accepted value is itself a preimage, it is never SMALLER
    // than the smallest-preimage fallback — this can only move the base up
    // inside the truncation window, never manufacture damage the wire denies.
    let base = engine_base
        .filter(|b| *b >= 0 && mul.apply(b.saturating_add(strength)) == damage)
        .unwrap_or_else(|| unbake_intent_damage(damage, strength, mul.num, mul.den));
    Some(match shape {
        AttackShape::Single => Intent::Attack { damage: base },
        AttackShape::Multi(hits) => Intent::MultiAttack { damage: base, hits },
        AttackShape::DeathBlow => Intent::DeathBlow { damage: base },
    })
}

/// Turn one enemy's wire telegraph into the engine-side [`Intent`].
///
/// Dispatches on the dict's declared dialect
/// ([`ParsedState::intent_amounts`]): a `base` dict is already in engine
/// semantics and is ingested verbatim, while the default `effective` dialect
/// is inverted through [`unbake_wire_intent`] so `perform_intent` doesn't
/// double-count the modifiers the producer already folded in.
///
/// `ai` is this enemy's RECONSTRUCTED AI, and must be passed only when its
/// current state was genuinely forced to the wire's `move_state` — see
/// [`reconstructed_base_damage`].
fn ingest_intent(
    m: &ParsedEnemy,
    parsed: &ParsedState,
    surrounded_facing: Option<i32>,
    ai: Option<&MonsterAi>,
) -> Option<Intent> {
    if parsed.intent_amounts == IntentAmounts::Base {
        return m.intent;
    }
    use sts2_core::hooks::{SURROUNDED_FACING_LEFT, SURROUNDED_FACING_RIGHT};
    // This arm back-attacks (wire telegraph bakes ×1.5) iff it sits behind
    // the inferred Surrounded facing.
    let back_attack_boost = match surrounded_facing {
        Some(SURROUNDED_FACING_RIGHT) => {
            m.powers.iter().any(|p| p.kind == PowerKind::BackAttackLeft)
        }
        Some(SURROUNDED_FACING_LEFT) => m
            .powers
            .iter()
            .any(|p| p.kind == PowerKind::BackAttackRight),
        _ => false,
    };
    unbake_wire_intent(
        m.intent,
        &m.powers,
        &parsed.player.powers,
        back_attack_boost,
        m.intent.and_then(|i| reconstructed_base_damage(ai, i)),
    )
}

/// Rebuild an enemy's AI at its wire move state and return that move's BASE
/// intent (the engine-side telegraph before any damage modifiers).
///
/// FRESH AI — `MonsterSpawnArgs::default()`, counters at 0. That is fine for
/// [`infer_surrounded_facing`], which runs before any creature exists and only
/// needs a ×1.5 discriminator. Do NOT reach for it when ingesting a telegraph:
/// a fresh AI drops the spawn payload and every mid-fight counter the
/// reconstruction went to the trouble of recovering, which is what
/// [`reconstructed_base_damage`] exists to read instead.
fn wire_base_intent(m: &ParsedEnemy) -> Option<Intent> {
    let ms = m.move_state.as_deref()?;
    let mut ai = build_ai(
        m.kind,
        MonsterSpawnArgs::default(),
        SpawnPosition::default(),
    );
    let resolved = wire_move_state_alias(m.kind, ms)
        .is_some_and(|alias| ai.force_current_state_by_name(alias))
        || ai.force_current_state_by_name_relaxed(ms);
    if !resolved {
        return None;
    }
    ai.current_move().intents.first().copied()
}

/// Infer the Surrounded facing (`SurroundedPower._facing`, hidden C# state
/// not on the wire) for a KaiserCrab reconstruction. 2026-07-17 matched-seed
/// gap audit, seeds 1010/1023/1030/1031/1057: rebuilding the player's
/// `Surrounded.counter` at 0 (facing Right) misattributed the 1.5×
/// back-attack whenever the real fight faced Left — Crusher's hits +50%
/// over-, Rocket's −33% under-estimated in every rollout.
///
/// The facing IS recoverable: each arm's wire telegraph bakes the 1.5× iff
/// that arm is behind the player, so forward-model each alive arm's base
/// intent (from its wire move state) both plain and boosted and see which
/// one the wire matches. With one arm dead C# `SurroundedPower.AfterDeath`
/// re-faces toward the survivor (no boost possible). Falls back to the C#
/// spawn default (Right) when no telegraph is discriminating.
fn infer_surrounded_facing(parsed: &ParsedState) -> Option<i32> {
    use sts2_core::hooks::{SURROUNDED_FACING_LEFT, SURROUNDED_FACING_RIGHT};
    parsed
        .player
        .powers
        .iter()
        .find(|p| p.kind == PowerKind::Surrounded && p.amount > 0)?;
    let arm_side = |m: &ParsedEnemy| {
        m.powers.iter().find_map(|p| match p.kind {
            PowerKind::BackAttackLeft | PowerKind::BackAttackRight => Some(p.kind),
            _ => None,
        })
    };
    let arms: Vec<(&ParsedEnemy, PowerKind)> = parsed
        .encounter
        .iter()
        .filter_map(|m| arm_side(m).map(|side| (m, side)))
        .collect();
    if arms.is_empty() {
        return None;
    }
    let alive: Vec<&(&ParsedEnemy, PowerKind)> = arms
        .iter()
        .filter(|(m, _)| !m.is_dead && m.hp > 0)
        .collect();
    if alive.len() == 1 {
        // Face the survivor: its back-attack can no longer trigger.
        return Some(match alive[0].1 {
            PowerKind::BackAttackLeft => SURROUNDED_FACING_LEFT,
            _ => SURROUNDED_FACING_RIGHT,
        });
    }
    for (m, side) in &arms {
        if m.is_dead || m.hp <= 0 {
            continue;
        }
        let Some(base_intent) = wire_base_intent(m) else {
            continue;
        };
        let (Intent::Attack { damage: base } | Intent::MultiAttack { damage: base, .. }) =
            base_intent
        else {
            continue;
        };
        let Some(Intent::Attack { damage: wire } | Intent::MultiAttack { damage: wire, .. }) =
            m.intent
        else {
            continue;
        };
        let strength = m
            .powers
            .iter()
            .find(|p| p.kind == PowerKind::Strength)
            .map_or(0, |p| p.amount);
        let mut mul = DamageMul::ONE;
        if parsed
            .player
            .powers
            .iter()
            .any(|p| p.kind == PowerKind::Vulnerable && p.amount > 0)
        {
            mul = mul.compose(DamageMul::VULNERABLE);
        }
        if m.powers
            .iter()
            .any(|p| p.kind == PowerKind::Weak && p.amount > 0)
        {
            mul = mul.compose(DamageMul::WEAK);
        }
        // Compose Shrink too (matching `unbake_wire_intent`): a Surrounded
        // arm also carrying Shrink would otherwise have both `plain` and
        // `boosted` computed without the ×0.7, so neither matches the wire
        // telegraph and facing silently falls back to the default.
        if m.powers
            .iter()
            .any(|p| p.kind == PowerKind::Shrink && p.amount > 0)
        {
            mul = mul.compose(DamageMul::SHRINK);
        }
        // And Colossus (same reason): the wire bakes the ×0.5 when the
        // player holds ColossusPower and this arm is Vulnerable, so both
        // candidates must carry it or neither matches (2026-08-02, run 1046
        // Crusher board reconstructed with a nonsense base of 1).
        if parsed
            .player
            .powers
            .iter()
            .any(|p| p.kind == PowerKind::Colossus)
            && m.powers
                .iter()
                .any(|p| p.kind == PowerKind::Vulnerable && p.amount > 0)
        {
            mul = mul.compose(DamageMul::HALF);
        }
        let x = base + strength;
        if x <= 0 {
            continue;
        }
        let plain = mul.apply(x);
        let boosted = mul.compose(DamageMul::new(3, 2)).apply(x);
        if plain == boosted {
            continue;
        }
        // Facing Right boosts the BackAttackLeft carrier and vice versa.
        if wire == boosted {
            return Some(match side {
                PowerKind::BackAttackLeft => SURROUNDED_FACING_RIGHT,
                _ => SURROUNDED_FACING_LEFT,
            });
        }
        if wire == plain {
            return Some(match side {
                PowerKind::BackAttackLeft => SURROUNDED_FACING_LEFT,
                _ => SURROUNDED_FACING_RIGHT,
            });
        }
    }
    // No discriminating telegraph (both arms on non-attack moves) — C#
    // spawn default.
    Some(SURROUNDED_FACING_RIGHT)
}

/// C# encounter-slot rank of a monster kind, for the encounters whose
/// roster can carry a DEAD-BUT-REVIVING corpse (the only case where the
/// wire's dead-last convention loses C# `Enemies` order — ordinary corpses
/// are removed from `Enemies` and never reappear).
///
/// Returns `(group, rank)`; a reorder is applied only when the WHOLE
/// roster resolves to one group (see the call site). Slot tables, from the
/// v0.107.1 decomp:
///   * `TheObscuraNormal.Slots = ["illusion", "obscura"]` — Parafright is
///     summoned into `"illusion"` (`TheObscura.cs:84`), ahead of TheObscura.
///   * `FogmogNormal.Slots = ["illusion", "fogmog"]` — EyeWithTeeth into
///     `"illusion"` (`Fogmog.cs:66`), ahead of Fogmog.
///   * `DecimillipedeElite.Slots = ["segment1", "segment2", "segment3"]` —
///     Front/Middle/Back spawn into segment1/2/3 in `GenerateMonsters`
///     order (`DecimillipedeElite.cs:37-52`).
fn reviver_slot_rank(kind: MonsterKind) -> Option<(u8, u8)> {
    use MonsterKind as MK;
    Some(match kind {
        MK::Parafright => (0, 0),
        MK::TheObscura => (0, 1),
        MK::EyeWithTeeth => (1, 0),
        MK::Fogmog => (1, 1),
        MK::DecimillipedeSegmentFront => (2, 0),
        MK::DecimillipedeSegmentMiddle => (2, 1),
        MK::DecimillipedeSegmentBack => (2, 2),
        _ => return None,
    })
}

/// Translate a [`ParsedState`] into a fully-initialised
/// [`CoreCombatState`]. See the module-level `state_from_dict` comment
/// for the populated fields.
///
/// # Errors
///
/// Returns `UnsupportedStateError` when the parsed state references a
/// card / monster / power / relic whose Rust-side spec cannot be
/// resolved (which should be impossible after a successful
/// [`parse_state_dict`] — guarded as a defensive belt).
pub fn state_from_dict_impl(parsed: &ParsedState) -> WireResult<CoreCombatState> {
    // Bead `sts2-rs-7hhd`: ensure the card-effect dispatch registry is
    // populated. The pymodule init also installs this, but
    // `state_from_dict_impl` is exported as a library entry point and
    // may be called from contexts (e.g. Rust integration tests, future
    // non-PyO3 callers) that don't go through the pymodule. Without this,
    // every `Action::PlayCard` against the produced state silently no-ops
    // and MCTS rollouts can never win. Idempotent.
    sts2_content::cards::dispatch::ensure_installed();
    // Same defensive shape for the card FACTORY (normally registered by the
    // pymodule init via `relics::dispatch::ensure_installed`): `build_card`
    // uses it for the upgraded-card keyword override and, for stats-less
    // wires (live bridge), the upgraded base stats. Without it a library
    // caller silently reconstructs upgraded cards at unupgraded values.
    sts2_core::register_card_factory(sts2_content::cards::spec_index::card_pod_from_id);

    let mut state = CoreCombatState {
        phase: combat_phase_from_str(&parsed.phase),
        round: u16::try_from(parsed.round).unwrap_or(0),
        side: side_from_str(&parsed.side),
        rng: sts2_core::dotnet_rng::Rng::new(
            parsed
                .rng_seed
                .unwrap_or(0)
                .wrapping_add(u64::try_from(parsed.turn).unwrap_or(0)) as i32,
        ),
        ascension: parsed.ascension,
        is_elite: parsed.is_elite,
        ..CoreCombatState::default()
    };

    // The run's own dice, when the producer carries them. Rebuilding the
    // whole `RunRngSet` from the master seed reproduces every per-stream salt
    // exactly as C# derives it, and fast-forwarding each stream to the read
    // count the world reported lands the reconstruction on the very next roll
    // the game will make.
    //
    // Without this the streams come from `RunRngSet::default()` and have no
    // relationship to the game's, so a predicted enemy intent or reshuffle
    // order is a guess — one that tells us nothing when it misses, because
    // the value was never derivable from the state. With it, a mismatch means
    // the two engines did different things with the SAME die roll, which is
    // exactly the kind of divergence worth crashing on.
    //
    // This is truth for the mirror, not for the search: the search runs on a
    // root-blindfolded clone whose invariance to this cursor is asserted
    // mechanically (sts2-mcts `root_blindfold` tests).
    if let Some(run_seed) = parsed.run_seed {
        state.run_rng = sts2_core::run_rng_set::RunRngSet::new(run_seed);
        for (name, counter) in &parsed.rng_counters {
            // One shared name→stream table (`RunRngSet::stream_mut`), also
            // used by the run-level live-state import. An unknown stream
            // name is a producer/engine vocabulary mismatch, not a value to
            // guess at.
            let Some(stream) = state.run_rng.stream_mut(name.as_str()) else {
                return Err(schema_err(
                    format!("$.rng.counters.{name}"),
                    "not a stream this engine has",
                ));
            };
            stream.fast_forward_counter(*counter);
        }
    }

    // Mirror the forward direction (see `state_to_dict`): a terminal wire
    // phase carries the `is_over`/`player_won` truth the engine reads, so
    // restore those flags here. Otherwise a round-tripped Victory/Defeat
    // state would deserialize as `phase == Resolved` but `is_over == false`
    // (the struct default) — silently "active" to every consumer that
    // checks `is_over` (sts2-rs-8ukqd).
    if let CombatPhase::Resolved { player_won } = state.phase {
        state.is_over = true;
        state.player_won = player_won;
    }

    // --- powers arena: populate first so creatures can refer to PowerUids ---
    // We manipulate `next_power_id` + push into `state.powers` manually
    // (no helper closure) to keep the borrow on `state` short — wrapping
    // this in a `FnMut` closure would mutably-borrow `state` for the
    // closure's full lifetime and conflict with the subsequent
    // `state.cards.push(...)` walks.
    let mut next_power_id: u32 = 1;

    // KaiserCrab Surrounded facing — inferred once, used for the player's
    // Surrounded counter AND each arm's intent unbake below.
    let surrounded_facing = infer_surrounded_facing(parsed);

    // Player powers.
    let mut player_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
    for pp in &parsed.player.powers {
        let raw = next_power_id;
        next_power_id += 1;
        let id = NonZeroU32::new(raw).expect("non-zero power id");
        // A producer that carries the raw per-instance counter wins outright;
        // the inferences below exist only for the dialects that don't.
        let counter = pp.counter.unwrap_or(match pp.kind {
            // C# `SurroundedPower._facing` is hidden state; the engine
            // stores it on `counter` (0=Right, 1=Left).
            PowerKind::Surrounded => surrounded_facing.unwrap_or(0),
            // C# `TenderPower.DisplayAmount => CardsPlayedThisTurn`
            // (`TenderPower.cs:22`). Tender drains 1 Strength AND 1 Dexterity
            // per card played and gives all of it back at the owning side's
            // turn end (`AfterSideTurnEnd`, TenderPower.cs:55-61); the engine
            // holds that count on `counter` and the turn-end restore is
            // `+counter` to each. `amount` is the Tender STACK, an unrelated
            // number, so `display` is the wire's only carrier for it.
            //
            // Reconstructing it at 0 made the restore a no-op, so the drain
            // was permanent: the mirror's player accumulated equal negative
            // Strength and Dexterity that the world had given back and pruned
            // (13 of the 43 single-field `$.player.powers` records in v17,
            // shaped `mirror STRENGTH -3 / DEXTERITY -3` vs `world` nothing).
            PowerKind::Tender => pp.display.unwrap_or(0),
            // Same family, same carrier. C# `PanachePower.DisplayAmount =>
            // DynamicVars["CardsLeft"]` (`PanachePower.cs:33`) and the engine's
            // `counter` IS CardsLeft (5 at max, decrement per card, fire and
            // reset at <= 0). Rebuilding at 0 is not merely "lost progress":
            // the engine's decrement takes 0 -> -1, which is <= 0, so a
            // reconstructed Panache fires on the very next card played, every
            // time. `-1` is its own "not yet initialised" sentinel, so absent
            // `display` keeps that rather than the 0 the other kinds default to.
            //
            // NOT fully recovered, and the residual is one card wide.
            // `CardsLeft == 5` is ambiguous on the wire: it means either "just
            // applied, nothing played yet" (engine sentinel `counter == -1`) or
            // "just fired and reset" (`counter == 5`). Taking `display`
            // verbatim always reads it as the second, so a freshly-applied
            // Panache loses one card of its first cycle. Strictly better than
            // the old 0, and as far as the wire can be pushed without a new
            // field — the two states are genuinely identical in every value C#
            // exposes.
            PowerKind::Panache => pp.display.unwrap_or(-1),
            // C# `AutomationPower.DisplayAmount => cardsLeft`, counting DOWN
            // from 10 (`AutomationPower.cs:14,25,41-48`); the engine counts the
            // same cards UP from 0 to 10, so the two are complements rather
            // than equal. Cards DRAWN, not played — C# ticks it from
            // `AfterCardDrawn` and the engine's counter rides the same hook.
            // `amount` here is the energy the power grants, NOT the cycle
            // length, so the constant is 10 and not `pp.amount` (the difference
            // from the WitheringPresence arm above).
            PowerKind::Automation => (10 - pp.display.unwrap_or(10)).clamp(0, 10),
            // GH #363 A1. C# `OrbitPower` keeps `energySpent` (cumulative) and
            // `triggerCount` privately and publishes only
            // `DisplayAmount => 4 - energySpent % 4` (`OrbitPower.cs:25`) —
            // the energy still needed for the next grant, in 1..=4. The engine
            // holds cumulative energy on `counter` and derives
            // `prev_triggers = counter / 4`, which is exactly C#'s invariant
            // `triggerCount == energySpent / 4`.
            //
            // The RESIDUE alone is sufficient, so no fidelity is lost by
            // rebuilding at `energySpent % 4` instead of the true total: with
            // `energySpent = 4q + r`, C# fires
            // `(4q + r + spend)/4 - q == (r + spend)/4`, which is what the
            // engine computes from a counter of `r`.
            //
            // Rebuilding at 0 made every reconstruction claim a full 4 energy
            // was still owed, so Orbit's grant landed late or early depending
            // on the phase — 30 of the 38 fully-explained `$.player.energy`
            // records on the 2026-08-03 live Regent A1 corpus (22 mirror-low,
            // 8 mirror-high). `display` is the wire's only carrier.
            //
            // MUST land together with `"ORBIT"` in `combat_mirror`'s `derived`
            // allow-list: without it `transplant_hidden` carries the mirror's
            // own drifting counter straight back over this derivation and the
            // arm is dead code.
            PowerKind::Orbit => (4 - pp.display.unwrap_or(4)).rem_euclid(4),
            _ => 0,
        });
        state.powers.push(Power {
            id,
            kind: pp.kind,
            amount: pp.amount,
            counter,
            applier: None,
            skip_next_tick: false,
        });
        player_powers.push(PowerUid(id));
    }

    // --- player scalars ---
    state.player.current_hp = parsed.player.hp;
    state.player.max_hp = parsed.player.max_hp;
    state.player.block = parsed.player.block;
    state.player.energy = i8::try_from(parsed.player.energy).unwrap_or(i8::MAX);
    state.player.max_energy = i8::try_from(parsed.player.max_energy).unwrap_or(i8::MAX);
    state.player.character = character_id_from_class(&parsed.player.class);
    state.player.powers.clone_from(&player_powers);
    state.player.stars = i8::try_from(parsed.player.stars.unwrap_or(0)).unwrap_or(0);
    if let Some(buckets) = parsed.player.cards_played_this_turn {
        state.player.cards_played_this_turn = buckets;
    }
    // GH #370. `card_plays_started_this_turn` gates card LEGALITY, not damage:
    // `SlothPower.ShouldPlay` returns `_cardsPlayedThisTurn < Amount` and
    // `RingingPower.ShouldPlay` refuses an afflicted card once any play has
    // started this turn. Rebuilt at 0 on every reconcile, both gates stay
    // stuck OPEN for the whole fight, so the mirror plans through actions the
    // world forbids.
    //
    // C# `SlothPower.DisplayAmount => _cardsPlayedThisTurn` (SlothPower.cs:21)
    // — `amount` is the THRESHOLD and `display` is the live count, the same
    // split as the Tender/Panache/Automation/Orbit family above. So when a
    // Sloth is up the wire discloses this exactly; `transplant_hidden` falls
    // back to the mirror's own accumulation when it does not (Ringing has no
    // carrier at all — its C# side is a `History.CardPlaysStarted` scan with
    // nothing stored on the power).
    // Seeded on Sloth's PRESENCE, not on `display` being populated, so that
    // this and `transplant_hidden`'s "did the wire disclose it?" guard test
    // exactly the same condition — otherwise a Sloth arriving without a
    // `display` would suppress the carry without supplying a replacement, and
    // the field would sit at 0 with neither mechanism owning it. Both
    // producers emit `display_amount` unconditionally
    // (`RunSimulator.cs:3747`, `SimStateExtractor.cs:816`), so the `None`
    // branch is defensive rather than a live path.
    if let Some(pp) = parsed
        .player
        .powers
        .iter()
        .find(|pp| pp.kind == PowerKind::Sloth)
    {
        state.player.card_plays_started_this_turn =
            u32::try_from(pp.display.unwrap_or(0)).unwrap_or(0);
    }
    state.combat_log.chains_bound_card_played_this_turn = parsed.player.bound_card_played_this_turn;
    // Run gold gates in-combat relic hooks (SealOfGold: spend 5 gold, +1
    // energy at every player turn start). The constructors hardcode 0 for
    // states not built through the run loop, so a wire that carries it must
    // win or the hook can never fire in a reconstruction.
    if let Some(gold) = parsed.player.gold {
        state.gold = gold;
    }

    // Defect orb belt. Parsed (and validated) since the wire gained
    // `orbs`/`orb_slots`, but never APPLIED until 2026-07-12 — a
    // round-tripped Defect state silently lost its whole belt, so every
    // searched future started orb-less.
    //
    // GH #368.2: both producers emit `passive`/`evoke` ALREADY Focus-scaled
    // (C# `OrbModel.PassiveVal => ModifyOrbValue(_passiveVal)`), so the parse
    // leg has to take Focus back off for any orb whose engine payload is the
    // RAW value. Glass is the only one: `Orb::Glass { current_passive }` is
    // C#'s private `_passiveVal`, which `GlassOrb.Passive` decays by 1 per
    // FIRING tick (GlassOrb.cs:34-39) — the damage is Focus-scaled but the
    // decay is not. Storing the scaled value made the orb both hit for
    // `raw + 2*focus` and, worse, take `focus` extra ticks to decay to 0: the
    // "Glass decay ±1 out of phase" class (49 elements / 33 records on the
    // live A1 Defect corpus), whose sign tracks the sign of Focus exactly.
    //
    // This was invisible to every round-trip test because the emit leg scales
    // and the parse leg did not un-scale — with Focus 0 the two agree, and
    // Focus 0 is what a fixture defaults to.
    //
    // Dark needs no such treatment: its `stored` is the accumulated growth and
    // both legs adjust by `DARK_ORB_BASE_EVOKE` alone (C# `DarkOrb._evokeVal`
    // is grown by the already-scaled passive, with no second Focus add).
    if let Some(orbs) = &parsed.player.orbs {
        // Powers are populated above, so the player's Focus is readable here.
        let focus = sts2_core::orbs::focus_bonus(&state);
        let capacity = parsed
            .player
            .orb_slots
            .unwrap_or(orbs.len() as u32)
            .min(u32::from(u8::MAX)) as u8;
        let mut belt = sts2_core::OrbSlots {
            slots: SmallVec::new(),
            capacity,
        };
        for po in orbs.iter().take(usize::from(capacity)) {
            belt.slots.push(match po.kind {
                OrbKind::Empty => Orb::Empty,
                OrbKind::Lightning => Orb::Lightning,
                OrbKind::Frost => Orb::Frost,
                // The wire's `evoke` is the C# `DarkOrb.EvokeVal`, which starts
                // at 6 and is grown by the passive (`DarkOrb.cs:17,25,39`).
                // `Orb::Dark`'s `stored` holds only the accumulated growth, so
                // the base has to come back off. Storing `evoke` verbatim made
                // every reconstructed Dark orb evoke for 6 too much — doubled
                // under Dualcast, ×4 under Quadcast — so MCTS planned lethals
                // the game would not land. Measured on a live wire: the
                // reconstruction dealt 48 where C# deals 36.
                OrbKind::Dark => Orb::Dark {
                    stored: (po.evoke - DARK_ORB_BASE_EVOKE).max(0),
                },
                OrbKind::Plasma => Orb::Plasma,
                // Invert the emit leg's `focus_scaled(current_passive, focus)`.
                // Exact whenever the scaled passive stayed positive. Under
                // enough NEGATIVE Focus the emit clamped to 0 and the raw is
                // unrecoverable — the wire says only `raw <= -focus` — so this
                // returns that upper bound. The orb is inert either way while
                // Focus is that negative (`GlassOrb.Passive` skips damage AND
                // decay on a non-positive `PassiveVal`, and so does the
                // engine's arm); taking the lower bound instead would delete
                // the orb's remaining life for good once Focus recovers.
                OrbKind::Glass => Orb::Glass {
                    current_passive: (po.passive - focus).max(0),
                },
            });
        }
        // Pad back out to `capacity` with empty slots. The engine's belt
        // invariant is `slots.len() == capacity` (see `OrbSlots::with_capacity`
        // / `add_slots`) and `channel` fills the first `is_empty()` slot,
        // falling back to EVICTING the oldest orb when it finds none. A
        // compact belt therefore made every channel on a non-full belt evict
        // instead of fill — the reconstructed Defect's orb count could never
        // grow past whatever it was rebuilt with (found by the Phase-1 mirror
        // oracle: LightningRod channeled at end of turn and the belt stayed
        // at one orb while the native engine reached two). The emitter drops
        // empty slots, so interior gaps compact to the front; that is the
        // channel order anyway.
        while belt.slots.len() < usize::from(capacity) {
            belt.slots.push(Orb::Empty);
        }
        state.player.orbs = belt;
    }
    // Necrobinder's Osty pet. `state_to_dict` emits `$.player.osty` and the
    // parser above reads it into `ParsedOsty`, but without this apply step
    // the parsed value was dropped on the floor: a state with a built-up
    // Osty round-tripped to one with `pet_uid == None`. Osty MaxHp compounds
    // across summons and resets to the summon amount on death, so silently
    // losing it discards the most valuable accumulated Necrobinder resource
    // (and Unleash/Protector/DieForYou all key off its hp/max_hp).
    //
    // The summon itself is DEFERRED to the end of this function, after the
    // player and enemy `Creature` rows exist — see the Osty block below the
    // monster loop. It cannot run here: `summon_osty` derives a fresh pet's
    // id from `creatures.len() + 1`, and at this point the arena is still
    // EMPTY, so the Osty would take id 1 — the id the player row is about to
    // be pushed with.

    // Potions (None → skip slot; Some(kind) → push).
    for k in parsed.player.potions.iter().flatten() {
        if state.player.potions.len() < state.player.potions.capacity() {
            state.player.potions.push(*k);
        }
    }

    // --- relics ---
    // Collected first, then one `extend`: every `RelicStore` write rebuilds
    // its per-combat relic plan (M01).
    let mut relics = Vec::with_capacity(parsed.player.relics.len());
    for (raw, r) in (1_u32..).zip(parsed.player.relics.iter()) {
        let id = NonZeroU32::new(raw).expect("non-zero relic id");
        let mut counter = r.counter.unwrap_or(0);
        // RedSkull's `_strengthApplied` latch is a pure function of the wire:
        // C# keeps `StrengthApplied == (hp <= 50% max)` at every quiescent
        // point (RedSkull.cs re-probes on every HP change), so derive it
        // instead of defaulting an undisclosed `counter: null` to 0 — a
        // below-threshold rebuild with a zeroed latch re-applied the +3
        // Strength (16 of the 188 `$.player.powers` divergences on the
        // v27wr sweep, all mirror exactly +3 with HP under half).
        if r.kind == RelicKind::RedSkull && r.counter.is_none() {
            counter = i32::from(2 * parsed.player.hp <= parsed.player.max_hp);
        }
        relics.push(Relic {
            id,
            kind: r.kind,
            counter,
            is_wax: r.is_wax,
            is_melted: r.is_melted,
        });
    }
    state.relics.extend(relics);

    // --- cards arena + piles ---
    // We allocate CardUids sequentially as we walk each pile in the
    // canonical order. Piles point at the freshly-allocated uids.
    let mut next_card_id: u32 = 1;
    let mut alloc_card_into = |state: &mut CoreCombatState,
                               parsed_card: &ParsedCard,
                               path: &str|
     -> WireResult<CardUid> {
        let raw = next_card_id;
        next_card_id += 1;
        let uid = CardUid::new(raw).ok_or_else(|| schema_err(path, "uid overflow"))?;
        let card = build_card(parsed_card, uid, path)?;
        state.cards.push(card);
        Ok(uid)
    };

    let mut draw_uids: SmallVec<[CardUid; 32]> = SmallVec::new();
    for (i, pc) in parsed.deck.draw_pile.iter().enumerate() {
        let path = format!("$.deck.draw_pile[{i}]");
        draw_uids.push(alloc_card_into(&mut state, pc, &path)?);
    }
    let mut hand_uids: SmallVec<[CardUid; 16]> = SmallVec::new();
    for (i, pc) in parsed.deck.hand.iter().enumerate() {
        let path = format!("$.deck.hand[{i}]");
        hand_uids.push(alloc_card_into(&mut state, pc, &path)?);
    }
    let mut discard_uids: SmallVec<[CardUid; 32]> = SmallVec::new();
    for (i, pc) in parsed.deck.discard_pile.iter().enumerate() {
        let path = format!("$.deck.discard_pile[{i}]");
        discard_uids.push(alloc_card_into(&mut state, pc, &path)?);
    }
    let mut exhaust_uids: SmallVec<[CardUid; 32]> = SmallVec::new();
    for (i, pc) in parsed.deck.exhaust_pile.iter().enumerate() {
        let path = format!("$.deck.exhaust_pile[{i}]");
        exhaust_uids.push(alloc_card_into(&mut state, pc, &path)?);
    }
    let mut play_uids: SmallVec<[CardUid; 8]> = SmallVec::new();
    for (i, pc) in parsed.deck.play_pile.iter().enumerate() {
        let path = format!("$.deck.play_pile[{i}]");
        play_uids.push(alloc_card_into(&mut state, pc, &path)?);
    }
    // Draw-pile knowledge. `parse_deck` already range-checked every
    // index against `draw_pile.len()`, and `draw_uids` was built from
    // that same list, so the lookups cannot fail.
    state.player.pinned_top = parsed
        .deck
        .pinned_top
        .iter()
        .map(|&i| draw_uids[i])
        .collect();
    state.player.pinned_bottom = parsed
        .deck
        .pinned_bottom
        .iter()
        .map(|&i| draw_uids[i])
        .collect();
    state.player.deferred_draw_order = parsed.deck.deferred_draw_order;

    // GH #370. Sly / Retain reach the engine through three per-instance uid
    // lists with three different lifetimes, and a rebuilt mirror re-created
    // all three empty — so a Well-Laid-Plans retain flushed at end of turn
    // and a Hand-Trick / MasterPlanner Sly stopped auto-playing on discard.
    //
    // Seeded from the card flags rather than carried across the transplant,
    // for the same reason Sloth's counter is: reality owns the value. The
    // uid lists cannot be carried anyway — a rebuild re-allocates every
    // `CardUid` from the pile walk above, and `transplant_hidden` aligns only
    // the draw pile (length + index) and the enemy roster, so a carried uid
    // would point at a different card or none.
    //
    // The single-turn grants are HAND-SCOPED: they exist only in hand (C#
    // clears them at end of turn). Keyword Sly from MasterPlanner lasts the
    // combat and stays on the card in every pile (`MasterPlannerPower.cs`):
    // a rebuild that kept it only for hand cards lost it on a Sly card in the
    // draw pile, which then didn't auto-play when discarded after the draw
    // (Acrobatics). So `sly` is read from every pile, below.
    let other_piles: [(&[CardUid], &[ParsedCard]); 4] = [
        (&draw_uids, &parsed.deck.draw_pile),
        (&discard_uids, &parsed.deck.discard_pile),
        (&exhaust_uids, &parsed.deck.exhaust_pile),
        (&play_uids, &parsed.deck.play_pile),
    ];
    for (uids, cards) in other_piles {
        for (&uid, pc) in uids.iter().zip(cards.iter()) {
            if pc.flags.iter().any(|f| f == "sly")
                && !state
                    .cards
                    .get(uid.0.get() as usize - 1)
                    .is_some_and(|c| sts2_core::card_effects::card_sly(c.card_id))
            {
                state.player.sly_this_combat.push(uid);
            }
        }
    }
    for (&uid, pc) in hand_uids.iter().zip(parsed.deck.hand.iter()) {
        for f in &pc.flags {
            match f.as_str() {
                // C# `ShouldRetainThisTurn` minus the keyword — the
                // Well-Laid-Plans single-turn grant. The keyword `retain`
                // flag stays deliberately unfolded (see the flag-fold block
                // above): folding a transient grant into the permanent
                // keyword override would retain the card all fight.
                "retain_turn" => state.player.retain_this_turn.push(uid),
                // C# `IsSlyThisTurn` minus the keyword — Hand Trick.
                "sly_turn" => state.player.sly_this_turn.push(uid),
                // Keyword Sly. Intrinsic Sly is already known from the
                // registry (`card_effects::card_sly`), so only an instance
                // the registry does NOT call Sly can have got the keyword
                // from `AddKeyword` — MasterPlanner, which lasts the whole
                // combat and belongs in the combat-scoped list.
                "sly" => {
                    let intrinsic = state
                        .cards
                        .get(uid.0.get() as usize - 1)
                        .is_some_and(|c| sts2_core::card_effects::card_sly(c.card_id));
                    if !intrinsic {
                        state.player.sly_this_combat.push(uid);
                    }
                }
                _ => {}
            }
        }
    }

    state.player.draw = draw_uids;
    state.player.hand = hand_uids;
    state.player.discard = discard_uids;
    state.player.exhaust = exhaust_uids;
    state.player.play = play_uids;

    // `deferred_draw_order` is an ASSERTION about the pile, not a preference:
    // it declares the draw order is a canonical belief, which lets
    // `pop_next_card` re-sort instead of permuting and so consume RNG
    // differently. Inside the engine the flag is only ever set immediately
    // after canonicalizing, so the invariant held by construction — its
    // doc-comment still says "no env-visible engine path ever sets it".
    // Round-tripping it through the wire made it settable by any producer,
    // and a producer that asserts it without establishing it gets no error:
    // the search just runs against a pile whose order means something other
    // than what the flag claims. Verify rather than trust.
    //
    // What this catches is PIN/SEGMENT coherence: canonical form is
    // `[pinned_top …, sorted middle …, pinned_bottom …]`, the pins ride the
    // wire as indices into `draw_pile`, so a pin the listed order
    // contradicts is a real, detectable lie.
    //
    // What it CANNOT catch — stated plainly because the check looks
    // stronger than it is — is an unpinned materialized shuffle. "Canonical"
    // means uid-ascending, and uids are an engine-internal creation order
    // the wire does not carry: `alloc_card_into` mints them positionally
    // just above, so ANY listed order arrives uid-ascending. The residual
    // exposure is small (the listed order survives ingest either way, so a
    // liar gets only the belief-mode RNG consumption it asked for) and the
    // real defense is upstream: `blindfold::belief_root` is the flag's only
    // writer. Both halves are pinned by tests in `tests/round_trip.rs`.
    if state.player.deferred_draw_order
        && !sts2_core::blindfold::unknown_middle_is_canonical(&state.player)
    {
        return Err(schema_err(
            "$.deck.deferred_draw_order",
            "may only be true when the draw pile is in canonical order — pinned_top \
             at the head, pinned_bottom at the tail, the middle sorted. This flag \
             asserts the pile is a belief, not a materialized shuffle, and the engine \
             changes its RNG consumption on it",
        ));
    }

    // master_deck: best-effort from optional wire field.
    if let Some(md) = &parsed.deck.master_deck {
        let mut master = Vec::with_capacity(md.len());
        for pc in md {
            if let Some(cid) = pc.card_id {
                master.push(cid);
            }
            // status/curse synthetic entries skipped — see build_card
            // for the rationale; master_deck is run-scope identity and
            // synthetic combat-only ids don't belong there.
        }
        state.master_deck = master;
    }

    // --- player Creature mirror ---
    // Build the player Creature row with the freshly-populated player
    // scalars + powers. This is the d03 Phase-1 link
    // (`PlayerCombatState.creature_uid`).
    let mut next_creature_id: u32 = 1;
    let player_creature_raw = next_creature_id;
    next_creature_id += 1;
    let player_creature_id = NonZeroU32::new(player_creature_raw).expect("non-zero");
    state.creatures.push(Creature {
        id: player_creature_id,
        current_hp: state.player.current_hp,
        max_hp: state.player.max_hp,
        block: state.player.block,
        powers: player_powers,
        // Player-Creature has `species: None` (bead sts2-rs-a19m).
        // Discrimination is via `state.creature_kind(uid)` which checks
        // `state.player.creature_uid`.
        species: None,
        monster_id: MonsterId("Player"),
        intent: None,
        intent_history: IntentHistory::default(),
        ai: None,
    });
    state.player.creature_uid = Some(CreatureUid(player_creature_id));

    // --- monster creatures ---
    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    // Parallel to `enemy_uids`: the C# encounter-slot rank of each rebuilt
    // enemy, for the rosters where it is kind-determined (see
    // `reviver_slot_rank`). Used after the loop to restore C# `Enemies`
    // order, which the wire's dead-last convention destroys.
    let mut slot_ranks: SmallVec<[Option<(u8, u8)>; 8]> = SmallVec::new();
    for (i, m) in parsed.encounter.iter().enumerate() {
        if m.is_escaped {
            // Skip escaped enemies — they left combat for good and
            // `legal_actions` wouldn't target them.
            continue;
        }
        // Dead enemies are NOT skipped: whatever the wire still carries
        // as a corpse is a REVIVER (Decimillipede segment awaiting
        // REATTACH, Eye with Teeth illusion, ...) — the game removes
        // ordinary corpses from its enemy list. The engine natively
        // keeps corpses in `state.creatures`/`enemy_uids` (alive-filters
        // gate targeting and the perform loop), and its revive machinery
        // keys off the corpse being present (`ReattachPower` DEAD→
        // REATTACH rotation; IllusionPower's `hp <= 0` revive scan).
        // Skipping them made every kill look permanent to MCTS rollouts
        // — measured on the 2026-07-13 rung-3.5 roster audit: real
        // Decimillipede segments regrow next turn (every occurrence),
        // Fogmog's Eye with Teeth revives (17/42 end_turns), while the
        // reconstructed search saw neither.
        let raw = next_creature_id;
        next_creature_id += 1;
        let cid = NonZeroU32::new(raw)
            .ok_or_else(|| schema_err(format!("$.encounter[{i}]"), "creature id overflow"))?;

        // Monster powers.
        let mut mpowers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        for pp in &m.powers {
            let praw = next_power_id;
            next_power_id += 1;
            let pid = NonZeroU32::new(praw).expect("non-zero power id");
            // Countdown-style powers keep `amount` as the static cycle
            // length and track live progress on `counter`. The wire's
            // `display` (C# `DisplayAmount` = cards LEFT) recovers the
            // phase: WitheringPresence's hook counts `counter` UP from 0
            // to `amount`, so counter = amount - display. Without this a
            // reconstruction at "next card spawns a Wither" restarted the
            // whole 6-card cycle (2026-07-17 Aeonglass audit).
            let counter = pp.counter.unwrap_or(match (pp.kind, pp.display) {
                (PowerKind::WitheringPresence, Some(display)) => {
                    (pp.amount - display).clamp(0, pp.amount.max(0))
                }
                // C# `SlowPower.DisplayAmount => SlowAmount * 10`
                // (`SlowPower.cs:22`). `SlowAmount` counts cards the player has
                // played this turn and drives
                // `ModifyDamageMultiplicative => 1 + 0.1 * SlowAmount` against
                // the owner (`SlowPower.cs:32-43`), resetting at the owner's
                // side-turn start. The engine holds it on `counter`; `amount` is
                // the Slow STACK, so `display` is the wire's only carrier.
                //
                // Rebuilding it at 0 made the mirror under-damage every Slow
                // enemy — it applied 1.0x or 1.1x where the game applied 1.3x —
                // which is why the mirror sat HP-HIGH. BygoneEffigy alone was
                // 135 of the 250 single-field `$.encounter[].hp` records in
                // v18, the largest class in the corpus.
                (PowerKind::Slow, Some(display)) => (display / 10).max(0),
                _ => 0,
            });
            state.powers.push(Power {
                id: pid,
                kind: pp.kind,
                amount: pp.amount,
                counter,
                applier: None,
                skip_next_tick: false,
            });
            mpowers.push(PowerUid(pid));
        }

        // Rebuild the enemy AI at its serialized move-state so it keeps
        // acting past the first telegraphed intent. Bead `sts2-rs-3f3rz`:
        // prior to this every reconstructed enemy was hard-coded `ai: None`,
        // which left it inert after one move — MCTS rollouts then ground the
        // passive enemy down and scored every line as a near-certain win.
        // A missing or unknown id falls back to `None` (the prior behavior).
        // The relaxed matcher is required for the live-game bridge, whose
        // `move_state` carries the C# `MoveState` id with its `_MOVE`
        // segment (`"WRIGGLE_MOVE"`) while the ported AI ids only sometimes
        // kept it (`"WRIGGLE"` vs Kin Priest's `"RITUAL_MOVE"`): exact-only
        // matching silently dropped the AI on 79% of live decisions
        // (2096/2642 in the 2026-07-13 sweep-7b audit), so rollouts fought
        // inert enemies and MCTS chronically under-blocked.
        // Spawn-time payload the wire does not carry but the ENCOUNTER ORDER
        // does. A Wriggler's slot decides where `SPAWNED_MOVE` goes after the
        // spawn-turn stun (slots 2/4 -> WRIGGLE, 1/3 -> NASTY_BITE), and
        // `MonsterSpawnArgs::default()` made every rebuilt wriggler slot-1, so
        // the mirror sent all four to NASTY_BITE while the game alternated.
        //
        // Reproduced on oracle Silent-14 (native engine both sides, so no
        // content-parity confound and `cursor_drifts == 0`): both sides agree
        // all four sit on `SPAWNED_MOVE`, then the mirror's own advance takes
        // every one to NASTY_BITE against a world going NB/WR/NB/WR. The
        // resolution did NOT fail — `create_wriggler_ai` has a `SPAWNED_MOVE`,
        // it is just wired to an unconditional NASTY_BITE follow-up.
        //
        // The ordinal among Wrigglers in encounter order matches C#'s
        // `wriggler1..4` as long as the roster is intact, so it is recoverable
        // from the wire without a new field — but only until one dies. The
        // adapter appends corpses to the end of `encounter`, so a reaped
        // Wriggler slides the survivors' ordinals down and three of four then
        // get the wrong slot. Bounded rather than fixed: the slot only selects
        // `SPAWNED_MOVE`'s follow-up, and all four sit on `SPAWNED_MOVE` only on
        // the spawn turn, before anything has had a chance to die. Carrying the
        // real slot would need a wire field.
        let spawn_args = if m.kind == MonsterKind::Wriggler {
            use sts2_content::monsters::shared::DenseVegetationWrigglerSlot as WSlot;
            // Ordinal by UID RANK among ALL Wrigglers on the wire — corpses
            // included. Uids are per-combat spawn-ordered, so the rank IS
            // C#'s `wriggler1..4` even after deaths; the previous
            // position-based count (`encounter[..i]`) broke the moment one
            // died, because the adapter re-appends corpses at the END of the
            // encounter list — every survivor's ordinal slid down and the
            // slot-conditional SPAWNED_MOVE follow-up inverted (attack↔buff)
            // for the rest of the fight (13 v34wr intent entries, seeds
            // 1018/1033/1045: world WRIGGLE/BITE/WRIGGLE for surviving slots
            // 2/3/4, mirror BITE/WRIGGLE/BITE).
            let ordinal = parsed
                .encounter
                .iter()
                .filter(|e| e.kind == MonsterKind::Wriggler && e.uid < m.uid)
                .count();
            MonsterSpawnArgs::DenseVegetationWriggler {
                slot: match ordinal % 4 {
                    0 => WSlot::First,
                    1 => WSlot::Second,
                    2 => WSlot::Third,
                    _ => WSlot::Fourth,
                },
            }
        } else {
            MonsterSpawnArgs::default()
        };
        // Whether the AI's current state was genuinely FORCED to the wire's
        // `move_state` (as opposed to the fallbacks below, which keep the
        // default-initial state). Only then does its move table describe the
        // move the wire is telegraphing, so only then may `ingest_intent`
        // read a base magnitude off it.
        let mut ai_at_wire_move = false;
        let ai: Option<MonsterAi> = m.move_state.as_deref().and_then(|ms| {
            let mut ai = build_ai(m.kind, spawn_args, SpawnPosition::default());
            // Alias table FIRST: entries are per-monster verified mappings,
            // including CORRECTIVE ones where the generic `_MOVE`-strip
            // lands on the wrong state (BygoneEffigy "SLEEP_MOVE" ≠ engine
            // "SLEEP"). For every non-corrective entry the wire id doesn't
            // relaxed-resolve at all, so the order change is inert there.
            ai_at_wire_move = wire_move_state_alias(m.kind, ms)
                .is_some_and(|alias| ai.force_current_state_by_name(alias))
                || ai.force_current_state_by_name_relaxed(ms);
            let resolved = ai_at_wire_move
                // C# `Creature.StunInternal` injects a SYNTHETIC
                // `MoveState("STUNNED")` (follow-up = the interrupted move) that
                // exists in no monster's static graph. Monsters with their own
                // stun state are aliased above; for the rest, keep the
                // freshly-built AI at its default initial state — the current
                // turn's no-op is already carried by the wire intent, and
                // resuming the default rotation afterwards beats an inert enemy.
                || ms == "STUNNED"
                // Corpses: C# arms synthetic revive states (`REVIVE_MOVE`)
                // that some ported AIs don't have as graph states (the
                // IllusionPower revive is combat-loop machinery here, not an
                // AI state). Keep the default-initial AI so the REVIVED
                // enemy isn't inert; the revive itself is driven by the
                // corpse's powers, not this pin.
                || m.is_dead;
            resolved.then_some(ai)
        });

        // TestSubject mid-fight phase inference (2026-07-17 matched-seed gap
        // audit, seeds 1012/1016/1026/1028). The wire carries the boss's
        // powers but NOT C#'s `Respawns` / `ExtraMultiClawCount` fields, so a
        // 2nd-form reconstruction left Adaptable.counter = 0 — the search
        // then believed TWO revives remained (a phantom repeat 2nd form:
        // +200 hp with the wrong kit) and its MULTI_CLAW playouts stayed at
        // the base 3 hits while the real boss escalates by one hit per
        // repeat. Both are inferable from what the wire DOES carry:
        // PainfulStabs exists exactly while the 2nd form lives (applied on
        // the 1st revive, removed on the 2nd — `adaptable_after_death`), and
        // the telegraphed MULTI_CLAW hit count is Base(3) + extra.
        let mut ai = ai;
        if m.kind == MonsterKind::TestSubject
            && m.powers.iter().any(|p| p.kind == PowerKind::PainfulStabs)
        {
            for pu in &mpowers {
                if let Some(p) = state.powers.get_mut_by_id(pu.0) {
                    match p.kind {
                        // Exactly one revive spent → one revive (to the 3rd
                        // form: ThirdFormHp + Nemesis) remains.
                        PowerKind::Adaptable => p.counter = 1,
                        // Recover ExtraMultiClawCount from the telegraph.
                        PowerKind::PainfulStabs => {
                            if let Some(Intent::MultiAttack { hits, .. }) = m.intent {
                                p.counter = (i32::from(hits)
                                    - sts2_core::monsters::test_subject::BASE_MULTI_CLAW_COUNT)
                                    .max(0);
                            }
                        }
                        _ => {}
                    }
                }
            }
            // Mirror the respawn count onto the AI counter so the
            // REVIVE_BRANCH predicate routes the (single) remaining revive
            // to the 3rd-form moveset, matching `adaptable_after_death`.
            if let Some(ai) = ai.as_mut() {
                ai.set_counter(1);
            }
        }

        // Aeonglass mid-fight escalation inference (2026-07-17 matched-seed
        // gap audit, seeds 1003/1020/1048/1050/1053). C# `_witherUpgradeCount`
        // / `_additionalStrength` are private monster fields (always equal at
        // rest) NOT on the wire; the Rust port models both as the boss AI
        // counter, which reconstruction rebuilt at 0 — so every future
        // INCREASING_INTENSITY rollout granted the flat base Strength (+3
        // instead of the real +3+N ramp) and spawned level-0 Withers (3 chip
        // vs the real 9-12). The count IS recoverable from what the wire
        // carries: every Wither in the combat is matched to the count when
        // spawned/bumped, so the max Wither FakeUpgrade level (= (damage-3)/3)
        // across the piles equals it whenever any matched Wither survives.
        // Deliberately NOT inverted from the boss's Strength stacks: external
        // sources contaminate that (seed 1020 carried +1 Str before the first
        // INCREASING_INTENSITY).
        if m.kind == MonsterKind::Aeonglass {
            if let Some(ai) = ai.as_mut() {
                let max_wither_level = [
                    &parsed.deck.draw_pile,
                    &parsed.deck.discard_pile,
                    &parsed.deck.hand,
                    &parsed.deck.exhaust_pile,
                ]
                .iter()
                .flat_map(|pile| pile.iter())
                .filter(|c| c.card_id == Some(CardId::Wither))
                .filter_map(|c| c.base_damage)
                .map(|d| (d - 3) / 3)
                .max()
                .unwrap_or(0);
                ai.set_counter(max_wither_level.max(0));
            }
        }

        // Fabricator `_lastSpawned` inference (v35 roster corpus, seeds
        // 1001/1018). C# `Fabricator.SpawnBot` EXCLUDES the last-spawned
        // bot kind from the next pick (`options.Where(m => m !=
        // _lastSpawned)`, `Fabricator.cs:114-115`); the engine stores that
        // kind on the Fabricator's `MonsterAi.counter`
        // (`fabricator_bot_code`). `_lastSpawned` is a private monster
        // field NOT on the wire, and rebuilding it at 0 turns a
        // C#-singleton pool back into a coin flip — the mirror then spawns
        // Stabbot where the game was FORCED to Zapbot (and vice versa),
        // deranging the bot roster for the rest of the fight.
        //
        // Recoverable from what the wire carries: uids are per-combat
        // spawn-ordered, so the highest-uid bot still on the wire IS the
        // most recent spawn. Two documented gaps: (a) when the most recent
        // bot has already died (ordinary corpse — dropped from the wire),
        // the surviving-max-uid bot may be an OLDER spawn; (b) with two
        // Fabricators the shared inference cannot attribute bots per
        // owner. Both leave the pre-existing behavior (worst case = the
        // old always-0 coin flip); no wire field short of `_lastSpawned`
        // itself can close them.
        if m.kind == MonsterKind::Fabricator {
            if let Some(ai) = ai.as_mut() {
                // Prefer the reflection-exported `fabricator_last_spawned`
                // (2026-08-02 producers) — it stays correct when the most
                // recent bot already died, the max-uid inference's
                // documented gap (a). Older wires fall back to the
                // inference below.
                let wire_code = m
                    .fabricator_last_spawned
                    .as_deref()
                    .and_then(monster_kind_from_python_name)
                    .map(sts2_content::monsters::act3_glory::fabricator_bot_code)
                    .filter(|&c| c != 0);
                let last_bot_code = wire_code.or_else(|| {
                    parsed
                        .encounter
                        .iter()
                        .filter(|e| {
                            sts2_content::monsters::act3_glory::fabricator_bot_code(e.kind) != 0
                        })
                        .max_by_key(|e| e.uid)
                        .map(|e| sts2_content::monsters::act3_glory::fabricator_bot_code(e.kind))
                });
                if let Some(code) = last_bot_code {
                    ai.set_counter(code);
                }
            }
        }

        // KnowledgeDemon `_curseOfKnowledgeCounter` (2026-08-02 wire
        // disclosure). The curse index (0..3) lives on the demon's
        // `MonsterAi.counter` (`knowledge_demon_curse_of_knowledge_effect`,
        // act2_hive.rs) and rebuilt at 0 on every reconstruction, so a
        // mid-fight mirror re-offered curse set 0 and applied
        // Disintegration 6 where the game was on 7/8 — the documented
        // DISINTEGRATION off-by-one. Older wires keep the 0 fallback.
        if m.kind == MonsterKind::KnowledgeDemon {
            if let Some(counter) = m.curse_of_knowledge_counter {
                if let Some(ai) = ai.as_mut() {
                    ai.set_counter(counter);
                }
            }
        }

        // No wire dialect carries a Defend telegraph's block (C#'s
        // `DefendIntent` is icon-only), so `parse_intent` builds
        // `Intent::Defend { block: 0 }` — and `perform_intent` gains monster
        // block FROM THAT PAYLOAD, so every reconstructed primary-Defend move
        // resolved as a 0-block no-op: Axebot BOOT_UP 10, MechaKnight WINDUP
        // 15, LouseProgenitor CURL_AND_GROW 14, PunchConstruct READY 10,
        // MagiKnight PREP 5, CrossbowRubyRaider RELOAD 3 (76 of the 127
        // `$.encounter[*].block` divergences on the v27wr sweep). The move's
        // `effect_fn` still fired — the Louse's +5 Strength matched while its
        // 14 Block did not — so recover the amount from the move table the
        // reconstructed AI is parked on. The variant-match guard keeps a
        // mis-resolved move state from injecting a bogus block.
        let intent = match ingest_intent(
            m,
            parsed,
            surrounded_facing,
            if ai_at_wire_move { ai.as_ref() } else { None },
        ) {
            Some(Intent::Defend { block: 0 }) => Some(
                ai.as_ref()
                    .and_then(sts2_core::monster_ai::MonsterAi::current_move_opt)
                    .and_then(|mv| mv.intents.first().copied())
                    .filter(|i| matches!(i, Intent::Defend { block } if *block > 0))
                    .unwrap_or(Intent::Defend { block: 0 }),
            ),
            other => other,
        };
        state.creatures.push(Creature {
            id: cid,
            // The engine's dead-convention is `current_hp <= 0`; clamp in
            // case a wire dialect flags `is_dead` with a residual hp.
            current_hp: if m.is_dead { m.hp.min(0) } else { m.hp },
            max_hp: m.max_hp,
            block: m.block,
            powers: mpowers,
            species: Some(m.kind),
            monster_id: MonsterId(m.kind.python_name()),
            // Translated from the wire dict's `intents[0]` (bead
            // `sts2-rs-2rp7`: prior to this, every enemy here was
            // hard-coded `None`, which made rollouts treat all
            // monsters as inert), with the UI-effective wire amount
            // inverted back to the engine's BASE-amount semantics —
            // see `unbake_wire_intent`. A dict that declares itself
            // already-base (`state_to_dict`) is ingested verbatim; see
            // `ParsedState::intent_amounts`.
            intent,
            // We don't reconstruct the full intent_history from the
            // i32-encoded wire dump (the Rust IntentHistory's internal
            // shape is private). A default history is sufficient for
            // hint-time roll-outs — the AI will re-roll from scratch.
            intent_history: IntentHistory::default(),
            ai,
        });
        enemy_uids.push(CreatureUid(cid));
        slot_ranks.push(reviver_slot_rank(m.kind));
    }

    // --- Fabricator-fight bot slots ---
    //
    // `FabricatorNormal.Slots = ["bot1", "bot2", "fabricator", "bot3",
    // "bot4"]`: C# gives a new bot the FIRST free bot slot
    // (`Encounter.GetNextSlot`) and keeps `Enemies` slot-sorted
    // (`SortEnemiesBySlotName`), so the wire's ALIVE order encodes each
    // living bot's slot — bots listed before the Fabricator hold slots
    // 0/1 in order, bots after hold 3/4. The engine's
    // `fabricator_spawn_bot` keys the NEXT spawn's slot pick and
    // insertion point off `enemy_slot()`; a reconstruction that leaves
    // every bot slot-unknown makes the next mid-step spawn claim slot 0
    // and front-insert, where the game refills the first genuinely free
    // slot (v35 roster corpus, seed 1001 round 4: mirror put the new bot
    // FIRST while the world put it in bot2, behind the survivor).
    if let Some(fab_pos) = parsed
        .encounter
        .iter()
        .position(|e| e.kind == MonsterKind::Fabricator && !e.is_dead && !e.is_escaped)
    {
        let mut uid_iter = enemy_uids.iter();
        let (mut before, mut after) = (0u8, 0u8);
        for (i, e) in parsed.encounter.iter().enumerate() {
            if e.is_escaped {
                continue;
            }
            let uid = *uid_iter
                .next()
                .expect("enemy_uids is parallel to the non-escaped wire entries");
            if e.is_dead || sts2_content::monsters::act3_glory::fabricator_bot_code(e.kind) == 0 {
                continue;
            }
            let slot = if i < fab_pos {
                let s = before.min(1);
                before += 1;
                s
            } else {
                let s = 3 + after.min(1);
                after += 1;
                s
            };
            state.set_enemy_slot(uid, slot);
        }
    }

    // --- Ovicopter-fight egg slots ---
    //
    // `OvicopterNormal.Slots = ["egg1", .., "egg5", "ovicopter"]`: the
    // Ovicopter holds slot 5 and `LayEggsMove` gives each egg the HIGHEST
    // slot no living enemy holds (`Slots.LastOrDefault`, Ovicopter.cs:87),
    // with `Enemies` kept slot-sorted, so living eggs are listed before the
    // Ovicopter. `ovicopter_lay_eggs_effect` reads both from `enemy_slot()`:
    // left unstamped, the Ovicopter sorts as slot-unknown and the next lay
    // appends its eggs AFTER it where the game puts them before. The eggs'
    // exact slots are not on the wire; the k listed eggs take the top k egg
    // slots in order (exact unless a lower egg died while a higher one
    // lived).
    if let Some(ovi_pos) = parsed
        .encounter
        .iter()
        .position(|e| e.kind == MonsterKind::Ovicopter && !e.is_dead && !e.is_escaped)
    {
        const OVICOPTER_SLOT: u8 = 5;
        let live: Vec<(usize, CreatureUid)> = parsed
            .encounter
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.is_escaped)
            .zip(enemy_uids.iter())
            .filter(|((_, e), _)| !e.is_dead)
            .map(|((i, _), uid)| (i, *uid))
            .collect();
        let eggs: Vec<CreatureUid> = live
            .iter()
            .filter(|(i, _)| *i < ovi_pos && parsed.encounter[*i].kind == MonsterKind::ToughEgg)
            .map(|(_, uid)| *uid)
            .collect();
        let first =
            OVICOPTER_SLOT.saturating_sub(u8::try_from(eggs.len()).unwrap_or(OVICOPTER_SLOT));
        for (k, uid) in eggs.iter().enumerate() {
            state.set_enemy_slot(*uid, first + u8::try_from(k).unwrap_or(0));
        }
        if let Some((_, uid)) = live.iter().find(|(i, _)| *i == ovi_pos) {
            state.set_enemy_slot(*uid, OVICOPTER_SLOT);
        }
    }

    // --- C# `Enemies`-order (slot) restoration for reviver rosters ---
    //
    // The adapter wire lists ALIVE enemies in C# `Enemies` order and
    // re-appends dead-but-reviving corpses LAST (`RunSimulator.cs`
    // `enemies` / `dead_enemies`), which erases a corpse's real position.
    // C# itself never moves one: `CombatState._enemies` stays sorted by
    // encounter-slot index (`SortEnemiesBySlotName`, `CombatState.cs:678-683`,
    // re-run on every slotted add, `CombatManager.cs:851-854`) and
    // Reattach/Illusion corpses stay in `Enemies`
    // (`ShouldCreatureBeRemovedFromCombatAfterDeath() == false`).
    // Rebuilding in wire order therefore parks the corpse at the END, and
    // the error surfaces one step later: after the corpse REVIVES the alive
    // orders differ (v35 corpus: mirror (TheObscura, Parafright) vs world
    // (Parafright, TheObscura); (Fogmog, EyeWithTeeth) reversed;
    // Decimillipede (Front, Back, Middle)), and after a FURTHER death the
    // corpse relative order differs (mirror (Middle, Back†, Front†) vs
    // world (Middle, Front†, Back†)).
    //
    // For every roster that can carry a reviving corpse the slot is fully
    // determined by the monster KIND (see `reviver_slot_rank`), so restore
    // slot order whenever the entire roster belongs to one such encounter
    // group. Ties (impossible today) keep wire order via the stable sort;
    // any roster with a kind outside a single group is left untouched.
    //
    // The rank IS the slot index for these encounters (illusion/obscura =
    // 0/1, illusion/fogmog = 0/1, segment1/2/3 = 0/1/2), so STAMP it as well
    // as sorting by it. The stamp is what a later summon reads: `Fogmog`'s
    // ILLUSION goes through `spawn_enemy_in_slot(.., 0, ..)`, whose
    // `slot_insert_index` scans for the first LIVING enemy whose recorded
    // slot exceeds 0 — and on a slot-unknown board that scan matches nothing
    // and degrades to APPEND. Same failure the Fabricator block above fixes:
    // a lone reconstructed Fogmog summoned its Eye BEHIND itself while the
    // world put it in front (`$.encounter.roster` mirror
    // `[Fogmog, EyeWithTeeth]` vs world `[EyeWithTeeth, Fogmog]`, oracle
    // sweep Ironclad-5 / Defect-17).
    //
    // Stamping is NOT gated on `len() > 1` the way the reorder is: a
    // one-enemy roster has nothing to reorder but still needs its slot known
    // before it summons. That single-enemy board is exactly the case that
    // was diverging.
    {
        let one_group = slot_ranks
            .first()
            .copied()
            .flatten()
            .map(|(g, _)| g)
            .filter(|g| slot_ranks.iter().all(|r| r.is_some_and(|(g2, _)| g2 == *g)));
        if one_group.is_some() {
            for (rank, uid) in slot_ranks.iter().zip(enemy_uids.iter()) {
                // Silently no-ops on an AI-less creature (the slot lives on
                // `MonsterAi`), which is the same best-effort contract the
                // Fabricator stamp above runs under.
                state.set_enemy_slot(*uid, rank.expect("gated on is_some").1);
            }
        }
        if one_group.is_some() && enemy_uids.len() > 1 {
            let mut order: SmallVec<[(u8, CreatureUid); 8]> = slot_ranks
                .iter()
                .zip(enemy_uids.iter())
                .map(|(r, uid)| (r.expect("gated on is_some").1, *uid))
                .collect();
            order.sort_by_key(|(rank, _)| *rank);
            enemy_uids = order.into_iter().map(|(_, uid)| uid).collect();
        }
    }
    state.enemy_uids = enemy_uids;

    // The Necrobinder's pet. Parsed since the wire gained `$.player.osty` and
    // — exactly like the Defect orb belt before 2026-07-12 — never APPLIED, so
    // every reconstruction silently threw the Osty away and left `pet_uid`
    // None. The mirror then had no pet to absorb damage, and BoundPhylactery
    // re-summoned a fresh 1-HP one at the next turn start. Osty MaxHp also
    // compounds across summons, so losing it discards the most valuable
    // accumulated Necrobinder resource (Unleash / Protector / DieForYou all key
    // off its hp/max_hp).
    //
    // Measured on the oracle (native engine both sides, `cursor_drifts == 0`
    // on 125 of 127): the mirror's osty sat at `hp 1 / max_hp 1` against a
    // world at 2/8, 3/7, 5/11, 5/8, and the player ate every hit the real Osty
    // would have soaked — the single largest oracle class, 127 single-field
    // `$.player.hp` records, mirror always LOWER.
    //
    // Restored even when DEAD. `summon_osty`'s revive branch keys off an
    // existing `pet_uid` with `current_hp == 0`, and the corpse keeps the
    // max_hp the pet had grown to — dropping it would both lose that growth
    // and turn the next summon into a fresh spawn. (Caught by the new
    // `$.player.osty` observable the moment it was added: an
    // `alive:false, hp:0, max_hp:7` pet round-tripped to nothing.)
    //
    // LAST, deliberately: `restore_osty` allocates the pet's creature id from
    // `creatures.len() + 1`, which is only a free id once every other creature
    // is in the arena. Run from the player-scalars section where the rest of
    // `$.player` is applied, the arena is still empty, the Osty takes id 1, and
    // the player row is pushed with id 1 straight afterwards. Two creatures then
    // answer to the same id, `ensure_die_for_you_on` attaches DieForYou to
    // whichever `find(|c| c.id == 1)` reaches first, and the aliasing walks
    // outward from there: found by the mirror oracle on Necrobinder seed 10,
    // where a pure `state_to_dict → state_from_dict` round-trip turned an
    // enemy's `STRENGTH 14` into a `DIE_FOR_YOU 1` shared with the player, and
    // the next end-of-turn power sweep left a dangling uid that failed
    // serialization outright ("power uid 3 not in arena").
    if let Some(osty) = parsed.player.osty.as_ref() {
        state.restore_osty(osty.hp, osty.max_hp, osty.block);
    }

    Ok(state)
}

// ---------------------------------------------------------------------------
// VarKey wire names — shared by the parse (above) and the `sts2-py`
// serializer, which imports them from here.
// ---------------------------------------------------------------------------

/// Wire names for [`VarKey`], the per-instance card scratch.
///
/// Exhaustive by construction: a new variant fails to compile here rather
/// than silently vanishing off the wire, which is how `combat_vars` came to
/// be dropped in the first place.
pub const fn var_key_wire_name(k: VarKey) -> &'static str {
    match k {
        VarKey::RampageDamage => "rampage_damage",
        VarKey::ClawBuff => "claw_buff",
        VarKey::GeneticAlgorithmBlock => "genetic_algorithm_block",
        VarKey::RitualDaggerDamage => "ritual_dagger_damage",
        VarKey::FreeThisTurn => "free_this_turn",
        VarKey::EtherealThisCombat => "ethereal_this_combat",
        VarKey::StarCostFreeThisCombat => "star_cost_free_this_combat",
        VarKey::MakeItSoInterval => "make_it_so_interval",
        VarKey::StompCostReductionThisTurn => "stomp_cost_reduction_this_turn",
        VarKey::PinpointCostReductionThisTurn => "pinpoint_cost_reduction_this_turn",
        VarKey::SneckoCostDeltaThisTurn => "snecko_cost_delta_this_turn",
        VarKey::ReturnIfPlayedRound => "return_if_played_round",
        VarKey::DampenDowngradedMarker => "dampen_downgraded_marker",
        VarKey::IsDupe => "is_dupe",
        VarKey::GeneratedInCombat => "generated_in_combat",
        VarKey::CostAbsolutelySet => "cost_absolutely_set",
        VarKey::ThrashFoldRemNum => "thrash_fold_rem_num",
        VarKey::ThrashFoldRemDen => "thrash_fold_rem_den",
        VarKey::FreeUntilPlayed => "free_until_played",
        VarKey::WitherFakeUpgradeLevel => "wither_fake_upgrade_level",
        VarKey::PendingBatchAutoPlay => "pending_batch_auto_play",
        VarKey::AutoPlayStarX => "auto_play_star_x",
        VarKey::WireTransientCostThisTurn => "wire_transient_cost_this_turn",
        VarKey::WireCostExpiringOnPlay => "wire_cost_expiring_on_play",
        VarKey::WireCostStable => "wire_cost_stable",
        VarKey::Generic => "generic",
        VarKey::EnchantmentUsed => "enchantment_used",
    }
}

/// Inverse of [`var_key_wire_name`], built by scanning the forward table so
/// the two cannot drift.
pub fn var_key_from_wire(name: &str) -> Option<VarKey> {
    const ALL: &[VarKey] = &[
        VarKey::RampageDamage,
        VarKey::ClawBuff,
        VarKey::GeneticAlgorithmBlock,
        VarKey::RitualDaggerDamage,
        VarKey::FreeThisTurn,
        VarKey::EtherealThisCombat,
        VarKey::StarCostFreeThisCombat,
        VarKey::MakeItSoInterval,
        VarKey::StompCostReductionThisTurn,
        VarKey::PinpointCostReductionThisTurn,
        VarKey::SneckoCostDeltaThisTurn,
        VarKey::ReturnIfPlayedRound,
        VarKey::DampenDowngradedMarker,
        VarKey::IsDupe,
        VarKey::GeneratedInCombat,
        VarKey::CostAbsolutelySet,
        VarKey::ThrashFoldRemNum,
        VarKey::ThrashFoldRemDen,
        VarKey::FreeUntilPlayed,
        VarKey::WitherFakeUpgradeLevel,
        VarKey::PendingBatchAutoPlay,
        VarKey::AutoPlayStarX,
        VarKey::WireTransientCostThisTurn,
        VarKey::Generic,
        VarKey::EnchantmentUsed,
    ];
    ALL.iter().copied().find(|k| var_key_wire_name(*k) == name)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
//
// The interpreter-free half of what used to be `sts2-py`'s `state_dict`
// tests: the intent un-baking, the registry mirrors, and the name resolvers.
// They never needed a `PyDict`, so they belong with the code they exercise.
// The `PyDict`-shaped ones (round-trips through `state_to_dict`, `parse_intent`
// over real Python objects) stayed behind in `sts2-py`, where they now cover
// this same parser through the `PyWire` dialect.

#[cfg(test)]
mod tests {
    use super::*;
    /// Every [`WIRE_MOVE_STATE_ALIASES`] target must exist as a move
    /// state on the freshly-built AI of its monster — a content-side
    /// state rename would otherwise silently revive the inert-enemy
    /// reconstruction bug the table exists to fix.
    #[test]
    fn wire_move_state_aliases_resolve_on_their_ais() {
        for (kind, wire_id, engine_id) in WIRE_MOVE_STATE_ALIASES {
            let mut ai = build_ai(*kind, MonsterSpawnArgs::default(), SpawnPosition::default());
            assert!(
                ai.force_current_state_by_name(engine_id),
                "alias ({kind:?}, {wire_id:?}) -> {engine_id:?}: \
                 target state does not exist on the built AI",
            );
            // And the relaxed matcher must NOT already land on the SAME
            // state — if it does (e.g. after a rename to match C#), the
            // alias entry is dead weight and should be removed. A relaxed
            // hit on a DIFFERENT state is fine: that's a corrective alias
            // overriding a wrong relaxed match (e.g. BygoneEffigy
            // "SLEEP_MOVE" relaxed-lands on "SLEEP" = C#'s SLEEP_MOVE_2,
            // but the wire id is C#'s INITIAL sleep).
            let mut fresh = build_ai(*kind, MonsterSpawnArgs::default(), SpawnPosition::default());
            if fresh.force_current_state_by_name_relaxed(wire_id) {
                assert_ne!(
                    fresh.current_state_id(),
                    *engine_id,
                    "alias ({kind:?}, {wire_id:?}) is redundant: the relaxed \
                     matcher already resolves it to {engine_id:?} — remove the entry",
                );
            }
        }
    }

    /// A move's intents after the head (a C# `MoveState`'s whole
    /// `params AbstractIntent[]`, which Headless and the engine now emit) are
    /// parsed too: a well-formed tail, `CardDebuff` included, is accepted and
    /// the head stays the current intent; a malformed one is refused.
    #[test]
    fn parse_enemy_accepts_a_moves_tail_and_checks_it() {
        let enemy = |tail: serde_json::Value| {
            serde_json::json!({
                "kind": "VINE_SHAMBLER", "uid": 2, "hp": 30, "max_hp": 30, "block": 0,
                "powers": [],
                "intents": [{"kind": "Attack", "amount": 8}, tail],
                "intent_history": [], "is_dead": false, "is_escaped": false,
            })
        };
        let ok = enemy(serde_json::json!({"kind": "CardDebuff"}));
        let parsed = parse_enemy(&&ok, "$.encounter[0]").expect("a CardDebuff tail parses");
        assert_eq!(parsed.intent, Some(Intent::Attack { damage: 8 }));
        let bad = enemy(serde_json::json!({"kind": "Attack", "amount": "eight"}));
        assert!(parse_enemy(&&bad, "$.encounter[0]").is_err());
        let unknown = enemy(serde_json::json!({"kind": "NotAnIntent"}));
        assert!(parse_enemy(&&unknown, "$.encounter[0]").is_err());
    }

    /// Wire intent amounts are UI-effective; ingestion must invert them
    /// to base so the damage pipeline doesn't double-apply modifiers.
    /// Numbers from the 2026-07-13 rung-3.5 fight-replay audit:
    /// Flyconid SMASH base 11, player Vulnerable → wire 16 (= ⌊11·3/2⌋);
    /// the real game dealt 16, the pre-fix engine dealt 24 (= ⌊16·3/2⌋).
    #[test]
    fn unbake_wire_intent_inverts_effective_amounts() {
        let vuln = ParsedPower {
            kind: PowerKind::Vulnerable,
            amount: 2,
            has_extra: false,
            display: None,
            counter: None,
        };
        let strength = |n: i32| ParsedPower {
            kind: PowerKind::Strength,
            amount: n,
            has_extra: false,
            display: None,
            counter: None,
        };
        // Player Vulnerable: 16 → 11 (and forward: ⌊11·3/2⌋ == 16).
        let got = unbake_wire_intent(
            Some(Intent::Attack { damage: 16 }),
            &[],
            std::slice::from_ref(&vuln),
            false,
            None,
        );
        assert_eq!(got, Some(Intent::Attack { damage: 11 }));
        assert_eq!(DamageMul::VULNERABLE.apply(11), 16);
        // Dealer Strength baked in: wire 9 with Str 3 → base 6.
        let got = unbake_wire_intent(
            Some(Intent::Attack { damage: 9 }),
            &[strength(3)],
            &[],
            false,
            None,
        );
        assert_eq!(got, Some(Intent::Attack { damage: 6 }));
        // Both: ⌊(6+3)·3/2⌋ = 13 → base 6.
        let got = unbake_wire_intent(
            Some(Intent::Attack { damage: 13 }),
            &[strength(3)],
            std::slice::from_ref(&vuln),
            false,
            None,
        );
        assert_eq!(got, Some(Intent::Attack { damage: 6 }));
        // MultiAttack inverts the per-hit amount, hits untouched.
        let got = unbake_wire_intent(
            Some(Intent::MultiAttack { damage: 6, hits: 3 }),
            &[],
            std::slice::from_ref(&vuln),
            false,
            None,
        );
        assert_eq!(got, Some(Intent::MultiAttack { damage: 4, hits: 3 }));
        let weak = |n: i32| ParsedPower {
            kind: PowerKind::Weak,
            amount: n,
            has_extra: false,
            display: None,
            counter: None,
        };
        let debilitate = |n: i32| ParsedPower {
            kind: PowerKind::Debilitate,
            amount: n,
            has_extra: false,
            display: None,
            counter: None,
        };
        let vigor = |n: i32| ParsedPower {
            kind: PowerKind::Vigor,
            amount: n,
            has_extra: false,
            display: None,
            counter: None,
        };
        // Dealer Vigor is baked into the telegraph exactly like Strength
        // (`VigorPower.cs:57-77` skips both latch guards at telegraph time).
        // TerrorEel CRASH_MOVE, live A1 corpora: base 16 + Vigor 6 → wire 22.
        // Storing 22 verbatim re-added the Vigor and searched a 28.
        let got = unbake_wire_intent(
            Some(Intent::Attack { damage: 22 }),
            &[vigor(6)],
            &[],
            false,
            None,
        );
        assert_eq!(got, Some(Intent::Attack { damage: 16 }));
        // Same eel two rounds later, now Weak'd: ⌊(16+6)·3/4⌋ = 16 on the wire.
        let got = unbake_wire_intent(
            Some(Intent::Attack { damage: 16 }),
            &[vigor(6), weak(2)],
            &[],
            false,
            None,
        );
        assert_eq!(got, Some(Intent::Attack { damage: 16 }));
        // Debilitate DEEPENS the dealer's Weak rather than multiplying on its
        // own (`DebilitatePower.cs:39-50`): 0.75 - (1 - 0.75) = 0.5.
        // Weak alone: wire 9 → ⌈9·4/3⌉ = 12.
        let got = unbake_wire_intent(
            Some(Intent::Attack { damage: 9 }),
            &[weak(1)],
            &[],
            false,
            None,
        );
        assert_eq!(got, Some(Intent::Attack { damage: 12 }));
        // Weak + Debilitate: the true chain is ×1/2, so wire 9 → base 18.
        let got = unbake_wire_intent(
            Some(Intent::Attack { damage: 9 }),
            &[weak(1), debilitate(2)],
            &[],
            false,
            None,
        );
        assert_eq!(got, Some(Intent::Attack { damage: 18 }));
        assert_eq!(DamageMul::HALF.apply(18), 9);
        // Debilitate WITHOUT Weak does nothing on its own.
        let got = unbake_wire_intent(
            Some(Intent::Attack { damage: 9 }),
            &[debilitate(2)],
            &[],
            false,
            None,
        );
        assert_eq!(got, Some(Intent::Attack { damage: 9 }));
        // Target-side: the player's own Debilitate deepens Vulnerable
        // 1.5 → 2.0 (`DebilitatePower.cs:26-37`). Wire 24 → base 12.
        let got = unbake_wire_intent(
            Some(Intent::Attack { damage: 24 }),
            &[],
            &[vuln.clone(), debilitate(1)],
            false,
            None,
        );
        assert_eq!(got, Some(Intent::Attack { damage: 12 }));
        assert_eq!(DamageMul::DOUBLE.apply(12), 24);
        // No modifiers → untouched (the rung-3 round-trip path).
        let got = unbake_wire_intent(Some(Intent::Attack { damage: 7 }), &[], &[], false, None);
        assert_eq!(got, Some(Intent::Attack { damage: 7 }));
        // Non-attack intents pass through.
        let got = unbake_wire_intent(
            Some(Intent::Stun),
            &[],
            std::slice::from_ref(&vuln),
            false,
            None,
        );
        assert_eq!(got, Some(Intent::Stun));
        // Surrounded back-attack baked in: Crusher THRASH wire 18 = ⌊12·3/2⌋.
        let got = unbake_wire_intent(Some(Intent::Attack { damage: 18 }), &[], &[], true, None);
        assert_eq!(got, Some(Intent::Attack { damage: 12 }));

        // TARGET-side additive: the player's Tainted adds its stacks to any
        // powered attack aimed at them, and everything else inverted here is
        // dealer-side, so it was missed. It reaches the board through
        // VitalSpark, which afflicts the player's Skills and applies Tainted
        // at its own stack count — on the v12 sweep the world-minus-mirror
        // damage delta equalled the VitalSpark amount in 93 of InfestedPrism's
        // 119 intent divergences, and the over-stated base fed the search.
        let tainted = |n: i32| ParsedPower {
            kind: PowerKind::Tainted,
            amount: n,
            has_extra: false,
            display: None,
            counter: None,
        };
        let got = unbake_wire_intent(
            Some(Intent::Attack { damage: 17 }),
            &[],
            &[tainted(2)],
            false,
            None,
        );
        assert_eq!(got, Some(Intent::Attack { damage: 15 }));
        // Composes with the dealer's Strength and the multiplier, in C#'s
        // order: ⌊(6 + 3 + 2)·3/2⌋ = 16 → base 6.
        let got = unbake_wire_intent(
            Some(Intent::Attack { damage: 16 }),
            &[strength(3)],
            &[vuln.clone(), tainted(2)],
            false,
            None,
        );
        assert_eq!(got, Some(Intent::Attack { damage: 6 }));

        // ColossusPower: the player's Colossus halves powered attacks from a
        // VULNERABLE dealer, and the wire telegraph bakes it (the producer's
        // `AttackIntent.GetSingleDamage` runs the full ModifyDamage chain).
        // Byrdonis SWOOP base 17, dealer Vulnerable, player Colossus: wire
        // ⌊17·1/2⌋ = 8. Un-inverted, `perform_intent` re-halved the stored 8
        // and the mirror planned against half the real damage (29 of the 33
        // `$.player.hp` end_turn divergences on the v27wr sweep). The
        // inversion recovers the SMALLEST preimage — 16, not the true 17;
        // forward-equivalent for the telegraph (⌊16/2⌋ == ⌊17/2⌋ == 8),
        // which is the known truncation-ambiguity limit of inverting, not a
        // Colossus gap.
        let colossus = ParsedPower {
            kind: PowerKind::Colossus,
            amount: 1,
            has_extra: false,
            display: None,
            counter: None,
        };
        let enemy_vuln = ParsedPower {
            kind: PowerKind::Vulnerable,
            amount: 2,
            has_extra: false,
            display: None,
            counter: None,
        };
        let got = unbake_wire_intent(
            Some(Intent::Attack { damage: 8 }),
            std::slice::from_ref(&enemy_vuln),
            std::slice::from_ref(&colossus),
            false,
            None,
        );
        assert_eq!(got, Some(Intent::Attack { damage: 16 }));
        assert_eq!(DamageMul::HALF.apply(16), 8);
        assert_eq!(DamageMul::HALF.apply(17), 8);
        // Colossus without a Vulnerable dealer never fires — verbatim.
        let got = unbake_wire_intent(
            Some(Intent::Attack { damage: 8 }),
            &[],
            std::slice::from_ref(&colossus),
            false,
            None,
        );
        assert_eq!(got, Some(Intent::Attack { damage: 8 }));
    }

    /// Inverting a FLOORED chain has to guess: under Weak (×3/4) every third
    /// effective value has TWO integer preimages and `unbake_intent_damage`
    /// takes the smallest. The guess survives only while the modifier set
    /// does — the moment a card changes it inside the same step, the mirror
    /// re-bakes from a base 1 low (7 v34wr intent entries). The reconstructed
    /// AI's move table breaks the tie, and only ever inside the ambiguity
    /// window: a base that cannot reproduce the wire amount is discarded.
    #[test]
    fn unbake_wire_intent_breaks_truncation_ties_with_the_engine_base() {
        let weak = ParsedPower {
            kind: PowerKind::Weak,
            amount: 1,
            has_extra: false,
            display: None,
            counter: None,
        };
        // Tunneler BITE_MOVE (move-table base 13) under Weak telegraphs
        // ⌊13·3/4⌋ = 9, whose preimages are {12, 13}.
        let unaided = unbake_wire_intent(
            Some(Intent::Attack { damage: 9 }),
            std::slice::from_ref(&weak),
            &[],
            false,
            None,
        );
        assert_eq!(
            unaided,
            Some(Intent::Attack { damage: 12 }),
            "smallest-preimage fallback is unchanged when there is no AI"
        );
        let aided = unbake_wire_intent(
            Some(Intent::Attack { damage: 9 }),
            std::slice::from_ref(&weak),
            &[],
            false,
            Some(13),
        );
        assert_eq!(aided, Some(Intent::Attack { damage: 13 }));
        assert_eq!(DamageMul::WEAK.apply(12), 9, "both really are preimages");
        assert_eq!(DamageMul::WEAK.apply(13), 9);
        // The WIRE stays the authority: a move-table base that cannot
        // forward-produce the reported amount (mis-resolved move state,
        // content drift, an escalation the table doesn't carry) is dropped.
        let bogus = unbake_wire_intent(
            Some(Intent::Attack { damage: 9 }),
            std::slice::from_ref(&weak),
            &[],
            false,
            Some(31),
        );
        assert_eq!(bogus, Some(Intent::Attack { damage: 12 }));
        // Hit counts are never sourced from the move table — only `damage` is
        // (see `reconstructed_base_damage`).
        let multi = unbake_wire_intent(
            Some(Intent::MultiAttack { damage: 9, hits: 6 }),
            std::slice::from_ref(&weak),
            &[],
            false,
            Some(13),
        );
        assert_eq!(
            multi,
            Some(Intent::MultiAttack {
                damage: 13,
                hits: 6
            })
        );
    }

    /// Lock the `MONSTER_KINDS` table against `MonsterKind::python_name`'s
    /// match arms — adding a new monster kind without updating the
    /// table fails this test loudly.
    #[test]
    fn monster_kinds_table_matches_python_names() {
        // Property: every kind in the table has a non-empty python_name.
        let names: Vec<&'static str> = MONSTER_KINDS.iter().map(|k| k.python_name()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "duplicate monster name");
        for n in &names {
            assert!(!n.is_empty());
        }
        // Sanity: a few canonical STS2-native enemies are present.
        for k in [
            MonsterKind::Nibbit,
            MonsterKind::Mawler,
            MonsterKind::ShrinkerBeetle,
        ] {
            assert!(MONSTER_KINDS.contains(&k), "missing {k:?}");
        }
    }

    #[test]
    fn reverse_lookups_round_trip() {
        assert_eq!(
            relic_kind_from_python_name("BURNING_BLOOD"),
            Some(RelicKind::BurningBlood)
        );
        assert_eq!(
            monster_kind_from_python_name("Nibbit"),
            Some(MonsterKind::Nibbit)
        );
        assert_eq!(
            card_id_from_python_name("STRIKE_IRONCLAD"),
            Some(CardId::StrikeIronclad)
        );
        assert_eq!(
            power_kind_from_python_name("STRENGTH"),
            Some(PowerKind::Strength)
        );
        assert_eq!(
            potion_kind_from_python_name("ATTACK_POTION"),
            Some(PotionKind::AttackPotion)
        );
        assert_eq!(
            orb_kind_from_python_name("LIGHTNING"),
            Some(OrbKind::Lightning)
        );

        assert!(relic_kind_from_python_name("FAKE_RELIC_THAT_DOES_NOT_EXIST").is_none());
        assert!(monster_kind_from_python_name("Ghoulrod").is_none());
    }

    /// Bead `sts2-rs-t8yv`: sts2log fixtures emit the Python pin's
    /// `Creature(monster_id=...)` form which is `UPPER_SNAKE` (e.g.
    /// `BOWLBUG_ROCK`), not the `MonsterKind::python_name()`
    /// `PascalCase` form (`BowlbugRock`). The resolver must accept
    /// both. Drains the top-5 `unknown monster kind` rows in
    /// `docs/reality-checks/2026-05-16-sts2log-content-gap-report.md`
    /// (114 blocked fixtures); see also the wider 53-kind table.
    #[test]
    fn monster_kind_resolver_accepts_upper_snake_for_sts2log_gap_top5() {
        let cases: &[(&str, MonsterKind)] = &[
            ("BOWLBUG_ROCK", MonsterKind::BowlbugRock),
            ("EXOSKELETON", MonsterKind::Exoskeleton),
            ("INFESTED_PRISM", MonsterKind::InfestedPrism),
            ("SLUDGE_SPINNER", MonsterKind::SludgeSpinner),
            ("ENTOMANCER", MonsterKind::Entomancer),
        ];
        for (input, expected) in cases {
            assert_eq!(
                monster_kind_from_python_name(input),
                Some(*expected),
                "UPPER_SNAKE form {input} should resolve to {expected:?}"
            );
        }
        // PascalCase still resolves (legacy callers / sts2-cli paths).
        assert_eq!(
            monster_kind_from_python_name("BowlbugRock"),
            Some(MonsterKind::BowlbugRock)
        );
        // Unknown still fails cleanly.
        assert!(monster_kind_from_python_name("FAKE_MONSTER_KIND").is_none());
        assert!(monster_kind_from_python_name("FakeMonsterKind").is_none());
    }

    /// Bead `sts2-rs-9fuj`: wave-D drains the next 5 "unknown monster
    /// kind" rows from `docs/reality-checks/2026-05-16-sts2log-content-gap-report.md`
    /// (74 blocked fixtures across `Toadpole`, `Tunneler`, `CorpseSlug`,
    /// `DecimillipedeSegmentFront`, `FlailKnight`). All 5 variants were
    /// already in `MONSTER_KINDS`, `monster_dispatch::create_monster_ai`,
    /// and encounter wiring by the time this bead claimed (a side-effect
    /// of `sts2-rs-bay` Necrobinder/Regent content-port work landing
    /// these variants earlier); the wave-D close criterion is the
    /// resolver matching both `PascalCase` and `UPPER_SNAKE` forms for each.
    /// This test pins that contract so a future `MONSTER_KINDS` edit can't
    /// silently drop a wave-D variant.
    #[test]
    fn monster_kind_resolver_accepts_upper_snake_for_sts2log_gap_wave_d() {
        let cases: &[(&str, MonsterKind)] = &[
            ("TOADPOLE", MonsterKind::Toadpole),
            ("TUNNELER", MonsterKind::Tunneler),
            ("CORPSE_SLUG", MonsterKind::CorpseSlug),
            (
                "DECIMILLIPEDE_SEGMENT_FRONT",
                MonsterKind::DecimillipedeSegmentFront,
            ),
            ("FLAIL_KNIGHT", MonsterKind::FlailKnight),
        ];
        for (input, expected) in cases {
            assert_eq!(
                monster_kind_from_python_name(input),
                Some(*expected),
                "UPPER_SNAKE form {input} should resolve to {expected:?}"
            );
        }
        // PascalCase round-trip for each.
        for (_, expected) in cases {
            let pascal = expected.python_name();
            assert_eq!(
                monster_kind_from_python_name(pascal),
                Some(*expected),
                "PascalCase form {pascal} should resolve to {expected:?}"
            );
        }
    }

    /// Bead `sts2-rs-t8yv`: sts2log emits card ids without the
    /// canonical `_CARD` suffix (`RUPTURE` vs `RUPTURE_CARD`). The
    /// suffix-strip fallback mirrors `card_id_from_str` from u5zc.
    /// Drains the `unknown card` bucket in the gap report (80
    /// fixtures across 9 cards).
    #[test]
    fn card_id_resolver_accepts_card_suffix_stripped_for_sts2log_gap() {
        // Canonical form still works.
        assert_eq!(
            card_id_from_python_name("RUPTURE_CARD"),
            Some(CardId::RuptureCard)
        );
        // Suffix-stripped form now resolves.
        assert_eq!(
            card_id_from_python_name("RUPTURE"),
            Some(CardId::RuptureCard)
        );
        // Cards without `_CARD` suffix still resolve directly.
        assert_eq!(
            card_id_from_python_name("STRIKE_IRONCLAD"),
            Some(CardId::StrikeIronclad)
        );
        // Unknown still fails.
        assert!(card_id_from_python_name("FAKE_CARD_THAT_DOES_NOT_EXIST").is_none());
    }

    #[test]
    fn pascal_to_upper_snake_handles_common_shapes() {
        assert_eq!(pascal_to_upper_snake("BowlbugRock"), "BOWLBUG_ROCK");
        assert_eq!(pascal_to_upper_snake("KinPriest"), "KIN_PRIEST");
        assert_eq!(pascal_to_upper_snake("InfestedPrism"), "INFESTED_PRISM");
        // Already upper-snake passes through.
        assert_eq!(pascal_to_upper_snake("EYE_WITH_TEETH"), "EYE_WITH_TEETH");
        // Single word.
        assert_eq!(pascal_to_upper_snake("Nibbit"), "NIBBIT");
        // Digit boundary.
        assert_eq!(pascal_to_upper_snake("Bowlbug2"), "BOWLBUG2");
    }

    /// Bead `sts2-rs-uxs2` (sts2log gap wave-E): the next 5 highest-
    /// blocked monster kinds after waves A / D. All 5 were already
    /// wired (variants + factories + `monster_dispatch` arms) via
    /// earlier act 4 ports — this test pins that the t8yv resolver
    /// fix actually drains them when sts2log emits the Python
    /// `Creature(monster_id=...)` `UPPER_SNAKE` form.
    ///
    /// Source: docs/reality-checks/2026-05-16-sts2log-content-gap-report.md
    /// §Unknown monster kinds (13 + 13 + 13 + 13 + 12 = 64 fixtures
    /// blocked across these 5 rows).
    #[test]
    fn monster_kind_resolver_drains_sts2log_gap_wave_e() {
        let cases: &[(&str, MonsterKind)] = &[
            ("PHANTASMAL_GARDENER", MonsterKind::PhantasmalGardener),
            ("TERROR_EEL", MonsterKind::TerrorEel),
            ("PUNCH_CONSTRUCT", MonsterKind::PunchConstruct),
            ("GREMLIN_MERC", MonsterKind::GremlinMerc),
            ("SKULKING_COLONY", MonsterKind::SkulkingColony),
        ];
        for (input, expected) in cases {
            assert_eq!(
                monster_kind_from_python_name(input),
                Some(*expected),
                "UPPER_SNAKE form {input} should resolve to {expected:?}"
            );
            // PascalCase resolution still works for legacy callers
            // (sts2-cli scenario loader / parity bridge tests).
            assert_eq!(
                monster_kind_from_python_name(expected.python_name()),
                Some(*expected),
                "PascalCase form {} should resolve to {expected:?}",
                expected.python_name()
            );
        }
    }

    /// Bead `sts2-rs-jt3k` (sts2log gap wave-F): the next 5 highest-
    /// blocked monster kinds after waves A / D / E. All 5 were already
    /// wired (variants + `python_name` + HP ranges + `monster_dispatch`
    /// arms) via earlier act 2/3 ports — `Crusher` in `act2.rs`, the
    /// rest in `act3.rs`. This test pins that the t8yv resolver fix
    /// actually drains them when sts2log emits the Python
    /// `Creature(monster_id=...)` `UPPER_SNAKE` form.
    ///
    /// Source: docs/reality-checks/2026-05-16-sts2log-content-gap-report.md
    /// §Unknown monster kinds (12 + 12 + 11 + 11 + 10 = 56 fixtures
    /// blocked across these 5 rows).
    #[test]
    fn monster_kind_resolver_drains_sts2log_gap_wave_f() {
        let cases: &[(&str, MonsterKind)] = &[
            ("LIVING_SHIELD", MonsterKind::LivingShield),
            ("TEST_SUBJECT", MonsterKind::TestSubject),
            ("MECHA_KNIGHT", MonsterKind::MechaKnight),
            ("CRUSHER", MonsterKind::Crusher),
            ("SOUL_NEXUS", MonsterKind::SoulNexus),
        ];
        for (input, expected) in cases {
            assert_eq!(
                monster_kind_from_python_name(input),
                Some(*expected),
                "UPPER_SNAKE form {input} should resolve to {expected:?}"
            );
            // PascalCase resolution still works for legacy callers.
            assert_eq!(
                monster_kind_from_python_name(expected.python_name()),
                Some(*expected),
                "PascalCase form {} should resolve to {expected:?}",
                expected.python_name()
            );
        }
    }

    /// Bead `sts2-rs-qqjg` (sts2log gap wave-G): the 5 visible rows
    /// not yet covered by waves A / D / E / F — `ScrollOfBiting` (18),
    /// `ThievingHopper` (17), `DevotedSculptor` (17), `Seapunk` (14),
    /// `Ovicopter` (10). 76 blocked fixtures combined. All 5 wired
    /// via earlier act 2/3/4 ports; this test pins the t8yv resolver
    /// drain on the `Creature(monster_id=...)` `UPPER_SNAKE` form.
    ///
    /// Source: `docs/reality-checks/2026-05-16-sts2log-content-gap-report.md`
    /// §Unknown monster kinds.
    #[test]
    fn monster_kind_resolver_drains_sts2log_gap_wave_g() {
        let cases: &[(&str, MonsterKind)] = &[
            ("SCROLL_OF_BITING", MonsterKind::ScrollOfBiting),
            ("THIEVING_HOPPER", MonsterKind::ThievingHopper),
            ("DEVOTED_SCULPTOR", MonsterKind::DevotedSculptor),
            ("SEAPUNK", MonsterKind::Seapunk),
            ("OVICOPTER", MonsterKind::Ovicopter),
        ];
        for (input, expected) in cases {
            assert_eq!(
                monster_kind_from_python_name(input),
                Some(*expected),
                "UPPER_SNAKE form {input} should resolve to {expected:?}"
            );
            // PascalCase resolution still works for legacy callers.
            assert_eq!(
                monster_kind_from_python_name(expected.python_name()),
                Some(*expected),
                "PascalCase form {} should resolve to {expected:?}",
                expected.python_name()
            );
        }
    }

    /// Bead `sts2-rs-6h3h` (sts2log gap wave-H): reaches into the
    /// gap-report residual single-digit tail (`…plus 28 more with
    /// single-digit counts`) — the visible top-25 is fully covered by
    /// waves A / D / E / F / G. Picks 5 high-recurrence Bowlbug-family
    /// and Kin-pair minor enemies (`BowlbugEgg`, `BowlbugNectar`,
    /// `BowlbugSilk`, `KinPriest`, `KinFollower`). All 5 wired via
    /// earlier act 1/2 ports (`h37x` for the Kin pair, `6ynt.10` for
    /// the Bowlbug family); this test pins the t8yv resolver drain on
    /// the `Creature(monster_id=...)` `UPPER_SNAKE` form so the
    /// residual single-digit tail stays drained as variants get
    /// added or shuffled.
    ///
    /// Source: `docs/reality-checks/2026-05-16-sts2log-content-gap-report.md`
    /// §Unknown monster kinds (residual single-digit tail).
    #[test]
    fn monster_kind_resolver_drains_sts2log_gap_wave_h() {
        let cases: &[(&str, MonsterKind)] = &[
            ("BOWLBUG_EGG", MonsterKind::BowlbugEgg),
            ("BOWLBUG_NECTAR", MonsterKind::BowlbugNectar),
            ("BOWLBUG_SILK", MonsterKind::BowlbugSilk),
            ("KIN_PRIEST", MonsterKind::KinPriest),
            ("KIN_FOLLOWER", MonsterKind::KinFollower),
        ];
        for (input, expected) in cases {
            assert_eq!(
                monster_kind_from_python_name(input),
                Some(*expected),
                "UPPER_SNAKE form {input} should resolve to {expected:?}"
            );
            // PascalCase resolution still works for legacy callers.
            assert_eq!(
                monster_kind_from_python_name(expected.python_name()),
                Some(*expected),
                "PascalCase form {} should resolve to {expected:?}",
                expected.python_name()
            );
        }
    }

    /// Bead `sts2-rs-95q5` (wave-I): pins `UPPER_SNAKE` resolution for
    /// the next 5 high-recurrence act1/act2 `MonsterKind` variants not
    /// covered by waves A/D/E/F/G/H. All 5 were already wired by
    /// earlier act 1/2 ports (gremlin family in `act1.rs`, `HauntedShip`
    /// in `act2.rs`, `LivingFog` in `act2.rs`, `CalcifiedCultist` in
    /// `act1.rs`); this test extends the resolver-drain regression
    /// coverage from t8yv onto the unenumerated single-digit tail.
    ///
    /// Source: `docs/reality-checks/2026-05-16-sts2log-content-gap-report.md`
    /// §Unknown monster kinds (residual single-digit tail).
    #[test]
    fn monster_kind_resolver_drains_sts2log_gap_wave_i() {
        let cases: &[(&str, MonsterKind)] = &[
            ("HAUNTED_SHIP", MonsterKind::HauntedShip),
            ("LIVING_FOG", MonsterKind::LivingFog),
            ("SNEAKY_GREMLIN", MonsterKind::SneakyGremlin),
            ("FAT_GREMLIN", MonsterKind::FatGremlin),
            ("CALCIFIED_CULTIST", MonsterKind::CalcifiedCultist),
        ];
        for (input, expected) in cases {
            assert_eq!(
                monster_kind_from_python_name(input),
                Some(*expected),
                "UPPER_SNAKE form {input} should resolve to {expected:?}"
            );
            assert_eq!(
                monster_kind_from_python_name(expected.python_name()),
                Some(*expected),
                "PascalCase form {} should resolve to {expected:?}",
                expected.python_name()
            );
        }
    }

    /// Bead `sts2-rs-ikxj` (wave-K): pins `UPPER_SNAKE` ↔ `PascalCase`
    /// round-trip for 5 Act 1/2 `MonsterKind` variants from the
    /// gap-report residual not covered by waves A/D/E/F/G/H/I/J or
    /// `ck77` / `qds8` / `qms2` / `shqg`. All 5 are already wired
    /// (variant + `python_name` arm + `monster_dispatch`); this
    /// extends the resolver-drain regression coverage onto the next
    /// 5 named variants and catches silent drops if `MONSTER_KINDS`
    /// or the matcher arms are edited.
    #[test]
    fn monster_kind_resolver_drains_sts2log_gap_wave_k() {
        let cases: &[(&str, MonsterKind)] = &[
            ("CHOMPER", MonsterKind::Chomper),
            ("FABRICATOR", MonsterKind::Fabricator),
            ("BYGONE_EFFIGY", MonsterKind::BygoneEffigy),
            ("BYRDONIS", MonsterKind::Byrdonis),
            ("CEREMONIAL_BEAST", MonsterKind::CeremonialBeast),
        ];
        for (input, expected) in cases {
            assert_eq!(
                monster_kind_from_python_name(input),
                Some(*expected),
                "UPPER_SNAKE form {input} should resolve to {expected:?}"
            );
            assert_eq!(
                monster_kind_from_python_name(expected.python_name()),
                Some(*expected),
                "PascalCase form {} should resolve to {expected:?}",
                expected.python_name()
            );
        }
    }

    /// Bead `sts2-rs-qms2` (wave-J residual tail): pins the
    /// bidirectional `UPPER_SNAKE` ↔ `PascalCase` round-trip for 5
    /// `MonsterKind` variants from the gap-report's "…plus 28 more
    /// with single-digit counts" residual tail — these are in the
    /// `§Unknown monster kinds` section but below the enumerated
    /// top-25 and were not previously named in any wave-A..I/ck77
    /// resolver test. All 5 were already wired by earlier act ports;
    /// this test extends the resolver-drain coverage onto the tail.
    ///
    /// Source: `docs/reality-checks/2026-05-16-sts2log-content-gap-report.md`
    /// §Unknown monster kinds (residual single-digit tail).
    #[test]
    fn monster_kind_resolver_drains_sts2log_gap_wave_j_tail() {
        let cases: &[(&str, MonsterKind)] = &[
            ("NIBBIT", MonsterKind::Nibbit),
            ("SHRINKER_BEETLE", MonsterKind::ShrinkerBeetle),
            ("FUZZY_WURM_CRAWLER", MonsterKind::FuzzyWurmCrawler),
            ("CUBEX_CONSTRUCT", MonsterKind::CubexConstruct),
            ("EYE_WITH_TEETH", MonsterKind::EyeWithTeeth),
        ];
        for (input, expected) in cases {
            assert_eq!(
                monster_kind_from_python_name(input),
                Some(*expected),
                "UPPER_SNAKE form {input} should resolve to {expected:?}"
            );
            assert_eq!(
                monster_kind_from_python_name(expected.python_name()),
                Some(*expected),
                "PascalCase form {} should resolve to {expected:?}",
                expected.python_name()
            );
        }
    }

    /// Bead `sts2-rs-shqg` (wave-J): pins the bidirectional
    /// `UPPER_SNAKE` ↔ `PascalCase` round-trip for the 5 highest-blocked
    /// `MonsterKind` variants from the §Unknown monster kinds top-5
    /// (`BowlbugRock` 33 / `Exoskeleton` 22 / `InfestedPrism` 21 /
    /// `SludgeSpinner` 20 / `Entomancer` 18 — 114 blocked fixtures
    /// combined). Wave-A (t8yv) already pinned the `UPPER_SNAKE`
    /// direction in `monster_kind_resolver_accepts_upper_snake_for_sts2log_gap_top5`;
    /// this wave adds the `PascalCase` symmetry so the top-5 stays
    /// drained even if the resolver shape changes again.
    ///
    /// Source: `docs/reality-checks/2026-05-16-sts2log-content-gap-report.md`
    /// §Unknown monster kinds top-5.
    #[test]
    fn monster_kind_resolver_drains_sts2log_gap_wave_j() {
        let cases: &[(&str, MonsterKind)] = &[
            ("BOWLBUG_ROCK", MonsterKind::BowlbugRock),
            ("EXOSKELETON", MonsterKind::Exoskeleton),
            ("INFESTED_PRISM", MonsterKind::InfestedPrism),
            ("SLUDGE_SPINNER", MonsterKind::SludgeSpinner),
            ("ENTOMANCER", MonsterKind::Entomancer),
        ];
        for (input, expected) in cases {
            assert_eq!(
                monster_kind_from_python_name(input),
                Some(*expected),
                "UPPER_SNAKE form {input} should resolve to {expected:?}"
            );
            assert_eq!(
                monster_kind_from_python_name(expected.python_name()),
                Some(*expected),
                "PascalCase form {} should resolve to {expected:?}",
                expected.python_name()
            );
        }
    }

    /// Bead `sts2-rs-bpt4`: when the wire dict's `amount` is missing
    /// or 0, fall back to a leading-integer scan of the optional
    /// `label` field. Covers the dominant English `Sts2Headless` / mod
    /// label patterns. Single-integer labels become `Intent::Attack`;
    /// `"<dmg> × <hits>"` style labels become `Intent::MultiAttack`.
    #[test]
    fn extract_label_damage_handles_canonical_patterns() {
        // Single-integer labels — Attack with hits=1.
        assert_eq!(extract_label_damage("Attack 6"), Some((6, 1)));
        assert_eq!(extract_label_damage("Deal 6 damage"), Some((6, 1)));
        assert_eq!(extract_label_damage("6"), Some((6, 1)));
        // Multi-hit forms — `×` (U+00D7), ASCII `x`, ASCII `*`.
        assert_eq!(extract_label_damage("6 × 3"), Some((6, 3)));
        assert_eq!(extract_label_damage("6 x 3"), Some((6, 3)));
        assert_eq!(extract_label_damage("6*3"), Some((6, 3)));
        assert_eq!(extract_label_damage("Attack 6 x 3"), Some((6, 3)));
        // Malformed `hits=0` → coerced to 1 (single-hit).
        assert_eq!(extract_label_damage("6 x 0"), Some((6, 1)));
        // No integers anywhere → None (caller keeps damage=0).
        assert!(extract_label_damage("Attack").is_none());
        assert!(extract_label_damage("").is_none());
        assert!(extract_label_damage("Defend").is_none());
    }
}
