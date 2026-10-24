//! Deterministic structural hash of [`CombatState`].
//!
//! Bead sts2-rs-uiq (P8-1): MCTS transposition tables and replay-trace
//! deduplication need a fast, deterministic hash of canonical game state.
//! This module computes one with `std::hash::Hasher` fed in a fixed,
//! sort-invariant order so two structurally identical states hash equal
//! regardless of internal `Vec` insertion order.
//!
//! **Stability contract:** the hash is stable across runs of the same
//! compiled binary (same git HEAD). It is **NOT** stable across pin
//! bumps — the hasher implementation and the set of fields fed into the
//! hash may both change between releases. Persistence to disk across
//! pin bumps is therefore unsupported; consumers must treat the value
//! as ephemeral cache key only.
//!
//! **Hasher choice (perf):** these hashes run once per MCTS node
//! expansion / chance-outcome dedup — the single hottest fixed cost of
//! a search after the rollouts themselves (SipHash was ~11% of all
//! instructions on the 2026-07-03 realistic-deck callgrind). We
//! therefore use a **fixed-seed** [`ahash::AHasher`] instead of the std
//! SipHash [`DefaultHasher`]: ~an order of magnitude cheaper per write
//! while keeping the deterministic-within-a-binary contract (the seeds
//! are compile-time constants, so two identically-stepped states hash
//! equal across processes of the same build).

use std::hash::{BuildHasher, Hash, Hasher};

/// Concrete hasher used by every state/run hash in this module.
type StateHasher = ahash::AHasher;

/// Fixed-seed hasher constructor — deterministic across processes of the
/// same compiled binary (unlike `RandomState`), unlike `DefaultHasher`
/// it is NOT documented-stable across releases, which our contract
/// already disclaims.
#[inline]
fn new_hasher() -> StateHasher {
    // Arbitrary odd constants; value only matters for determinism.
    ahash::RandomState::with_seeds(
        0x9E37_79B9_7F4A_7C15,
        0xD1B5_4A32_D192_ED03,
        0x8AF8_63C4_43D1_2E9B,
        0x2545_F491_4F6C_DD1D,
    )
    .build_hasher()
}

use crate::cards::PileKind;
use crate::run_manager::RunManager;
use crate::run_rng_set::RunRngSet;
use crate::{CombatPhase, CombatState, PlayerCombatState, Side};

/// Compute a deterministic structural hash of `state`.
///
/// The hash covers, in fixed order:
///   * `phase`, `round`, `side`, `is_over`, `player_won`, `ascension`
///   * RNG state, as the `(seed, counter)` pair of `state.rng` plus every
///     [`RunRngSet`] stream cursor. A `dotnet_rng::Rng` is fully determined
///     by that pair, so hashing the two scalars is a complete key.
///     [`search_hash`] omits this block entirely — that is the only
///     difference between the two hashes.
///   * Per-pile card-uid contents, folded through an **order-invariant
///     commutative mix** (a wrapping sum of per-uid `splitmix64` values —
///     see [`pile_mix`]) so insertion order is irrelevant. Each pile is
///     hashed with its [`PileKind`] discriminant prefixed so a card moving
///     Hand→Discard produces a different hash even if the total multiset
///     is unchanged.
///   * Player scalars: `energy`, `max_energy`, `stars`, `current_hp`,
///     `max_hp`, `block`.
///   * Player powers, sorted by `(power_uid, kind discriminant, amount)`.
///   * The Defect orb belt (`orbs.capacity` plus each slot in FIFO channel
///     order — order is significant).
///   * Each enemy's `id` / `current_hp` / `max_hp` / `block` / `kind` /
///     `intent` / sorted powers, in `enemy_uids` order (which is the
///     stable canonical order — slot identity matters because Python's
///     `target_index` is `enemy_uids` index).
///   * `pending_choice` payload (`kind`, `min_select`, `max_select`,
///     `can_skip`, and the ordered `options` list). Order is
///     significant — Python `PendingCardChoice.selected_indices`
///     references options by position, so reshuffling would be a
///     semantically distinct state.
#[must_use]
pub fn state_hash(state: &CombatState) -> u64 {
    let mut h = new_hasher();
    state_hash_walk::<FastEncoding, _>(state, &mut h);
    h.finish()
}

/// Full [`state_hash`] field walk (scalars + RNG cursors + piles), generic
/// over the hasher and the [`HashEncoding`] strategy so the fast path
/// (ahash + [`FastEncoding`]) and the frozen legacy path ([`legacy_run`]:
/// `DefaultHasher` + [`LegacyEncoding`]) share one walk — `Hash::hash`
/// writes identical byte streams to any hasher.
fn state_hash_walk<E: HashEncoding, H: Hasher>(state: &CombatState, h: &mut H) {
    // Field order is byte-stable: scalars, then the RNG cursors, then the
    // piles — exactly the historical layout. `hash_game_state` is *not*
    // called as one unit here because that would move the RNG sections to
    // the end and silently change every `state_hash` value (the hashers
    // are order-sensitive). Interleave the two shared halves around the
    // RNG blocks instead, so `state_hash` stays identical to prior builds
    // and `search_hash` is still exactly `state_hash` minus the RNG
    // sections.
    hash_scalars(state, h);

    // ── RNG ─────────────────────────────────────────────────────────────
    // `state.rng` is fully determined by its `(seed, counter)` pair — the
    // underlying `DotNetRandom` is `new(seed)` advanced `counter` times
    // (exactly what `Rng::with_counter` reconstructs). Hashing those two
    // scalars is a complete, faithful key and avoids the per-call
    // `format!("{:?}", …)` allocation that the Debug-string approach paid.
    0x02u8.hash(h);
    state.rng.seed().hash(h);
    state.rng.counter().hash(h);

    // ── RunRngSet streams (bead `sts2-rs-cfe5g`, 943w7 Stage 5) ──────────
    // `state.run_rng` is the primary combat RNG, carrying the shuffle /
    // card-selection / energy-cost / target / monster-ai / niche / orb /
    // card-&-potion-generation cursors (plus the run-side reward / shop /
    // map streams). The legacy single-stream `state.rng` hash above does
    // NOT reflect any of these, so two combat states identical except for
    // RNG cursor position would hash-collide. Hash every stream's `seed()`
    // + `counter()` (the cursor) so state-equality tracks RNG position.
    0x0Au8.hash(h);
    hash_run_rng_set(&state.run_rng, h);

    hash_piles::<E, H>(state, h);
}

/// Compute a deterministic structural hash of `state` that is **insensitive
/// to RNG cursor position** — identical to [`state_hash`] but with the
/// `state.rng` and `state.run_rng` blocks omitted.
///
/// This is the transposition key for **chance-node MCTS** (bead
/// `feat/mcts-chance-nodes`). In chance mode, randomness is modeled
/// explicitly via chance-node outcomes whose future is re-randomized per
/// outcome, so two game-identical states reached via different outcome seeds
/// (different RNG cursors) are the *same node* for search purposes and must
/// merge in the transposition table. [`state_hash`] folds RNG cursors in
/// (deliberately, to avoid cross-determinization aliasing in the default
/// PIMC path — see the `0x02`/`0x0A` blocks above), so chance mode needs
/// this cursor-free variant instead.
///
/// `search_hash == state_hash` minus the two RNG sections; both share
/// [`hash_game_state`] so the game-observable field set cannot drift between
/// them.
#[must_use]
pub fn search_hash(state: &CombatState) -> u64 {
    let mut h = new_hasher();
    hash_game_state::<FastEncoding, _>(state, &mut h);
    h.finish()
}

/// Fold every **game-observable** field of `state` into `h` in fixed order.
/// Shared by [`state_hash`] (which interleaves the RNG cursors between the
/// scalar and pile halves) and [`search_hash`] (which omits the RNG cursors
/// entirely). Both hashes call the same [`hash_scalars`] + [`hash_piles`]
/// halves, so the game-observable field set cannot drift between them.
/// Generic over the hasher + [`HashEncoding`] so [`legacy_run`] reuses it.
fn hash_game_state<E: HashEncoding, H: Hasher>(state: &CombatState, h: &mut H) {
    hash_scalars(state, h);
    hash_piles::<E, H>(state, h);
}

/// Order- AND instance-uid-insensitive dedup key for `DrawChance`
/// chance-outcome deduplication (bead `sts2-rs-i8bgu`, F1 plan §3.2 step
/// 3; `crates/sts2-mcts`'s `chance_widen_or_sample` / DrawChance
/// outcome-0 build are the only callers). Identical to [`search_hash`]
/// (same scalars, RNG-cursor omission, allies, enemies, cards-arena
/// content, potions, relics, pending choice, Discard/Exhaust/Play piles)
/// EXCEPT the player's HAND and DRAW piles: instead of `search_hash`'s
/// per-uid [`pile_mix`] fingerprint, HAND (in full) and DRAW's
/// unknown-middle segment ([`crate::cards::unknown_middle_uids`] — the
/// same segment [`crate::cards::shuffle`] permutes) are each folded as a
/// SORTED list of [`card_observational_key`] values
/// ([`hash_pile_observational`]).
///
/// Why HAND too, not just DRAW: under `DrawChance`, two chance outcomes
/// that drew observationally-identical-but-differently-uid'd duplicate
/// cards (e.g. two un-upgraded Strikes) are indistinguishable to the
/// player — but the drawn card lands in HAND, so leaving HAND
/// `pile_mix`-encoded (uid-based) would still fail to merge those
/// outcomes even after fixing DRAW alone; both piles must go
/// uid-insensitive together or the dedup this hash exists for doesn't
/// actually fire on the dominant real case (duplicate Strikes/Defends in
/// every starter deck). `search_hash`'s pile fingerprint is keyed on the
/// exact physical `uid`, so without this the DPW outcome fan-out would
/// explode on any deck with duplicate cards. This hash makes such
/// outcomes hash equal so they dedup instead.
///
/// Pinned positions are hashed exactly like [`search_hash`] (uid-based,
/// order-sensitive) — they are player-KNOWN information (Headbutt /
/// Forethought), not part of the duplicate-card ambiguity this hash
/// resolves.
///
/// Chance-outcome dedup ONLY: the transposition-table key
/// (`MctsNode::search_hash` in `sts2-mcts`) and every other use of
/// [`search_hash`] are completely untouched by this function's
/// existence — it is a new, additive, opt-in-by-caller hash.
#[must_use]
pub fn chance_outcome_hash(state: &CombatState) -> u64 {
    let mut h = new_hasher();
    hash_scalars(state, &mut h);
    0x03u8.hash(&mut h);
    hash_player_chance_dedup(&state.player, &mut h, state);
    0x04u8.hash(&mut h);
    (state.allies.len() as u64).hash(&mut h);
    for ally in &state.allies {
        hash_player::<FastEncoding, _>(ally, &mut h, state);
    }
    hash_enemies_cards_items_pending::<FastEncoding, _>(state, &mut h);
    h.finish()
}

/// Top-level scalar fields (phase / round / side / over / ascension …).
/// Hashed first by both [`state_hash`] and [`search_hash`]; split out from
/// [`hash_piles`] so `state_hash` can fold the RNG cursors *between* the two
/// halves and preserve its historical byte layout.
fn hash_scalars<H: Hasher>(state: &CombatState, h: &mut H) {
    // ── Top-level scalars ───────────────────────────────────────────────
    // Tag each section with a small constant so a future field-set
    // change is easy to detect via test failures rather than silent
    // collisions.
    0x01u8.hash(h);
    discriminant_phase(state.phase).hash(h);
    if let CombatPhase::Resolved { player_won } = state.phase {
        player_won.hash(h);
    }
    state.round.hash(h);
    discriminant_side(state.side).hash(h);
    state.is_over.hash(h);
    state.player_won.hash(h);
    state.ascension.hash(h);
    // Per-turn "one Bound card already played" lockout (ChainsOfBinding /
    // Queen fight): distinguishes a position where the single allowed Bound
    // play is still available from one where it has been spent. Without it
    // the transposition table aliases pre- and post-Bound-play states.
    0x0Cu8.hash(h);
    state.combat_log.chains_bound_card_played_this_turn.hash(h);
    // Necrobinder per-turn combat-log flags: DeathsDoor's tripled block
    // reads `doom_applied_by_player_this_turn`, and Rattle's hit count /
    // Flatten's free-cost read `osty_attacks_this_turn`. Without them the
    // transposition table aliases pre- and post-trigger positions, exactly
    // like the ChainsOfBinding lockout above.
    0x0Du8.hash(h);
    state.combat_log.doom_applied_by_player_this_turn.hash(h);
    state.combat_log.osty_attacks_this_turn.hash(h);
}

/// Player / ally / enemy / card-arena / potion / relic / pending sections.
/// Hashed after the scalars (and, in [`state_hash`], after the RNG cursors).
/// The two per-strategy encodings (pile fingerprint, relic kind, card-arena
/// iteration order) route through `E`; everything else is byte-identical
/// across the fast and legacy paths.
fn hash_piles<E: HashEncoding, H: Hasher>(state: &CombatState, h: &mut H) {
    hash_player_and_allies::<E, H>(state, h);
    hash_enemies_cards_items_pending::<E, H>(state, h);
}

/// Player + allies sections of [`hash_piles`] (tags `0x03`/`0x04`), split
/// out so [`chance_outcome_hash`] (bead `sts2-rs-i8bgu`, F1) can swap in
/// its own player encoding while reusing this section verbatim for
/// allies. Pure extraction — same hash calls in the same order as the
/// pre-split `hash_piles` body, so this is not a behavior change.
fn hash_player_and_allies<E: HashEncoding, H: Hasher>(state: &CombatState, h: &mut H) {
    // ── Player piles ────────────────────────────────────────────────────
    0x03u8.hash(h);
    hash_player::<E, H>(&state.player, h, state);

    // ── Allies ──────────────────────────────────────────────────────────
    0x04u8.hash(h);
    (state.allies.len() as u64).hash(h);
    for ally in &state.allies {
        hash_player::<E, H>(ally, h, state);
    }
}

/// Enemy / card-arena / potion / relic / pending-choice sections of
/// [`hash_piles`] (tags `0x05`-`0x09`), split out so
/// [`chance_outcome_hash`] can reuse this section unmodified — it does not
/// touch the player's draw pile at all. Pure extraction, same hash calls
/// in the same order as the pre-split `hash_piles` body.
fn hash_enemies_cards_items_pending<E: HashEncoding, H: Hasher>(state: &CombatState, h: &mut H) {
    // ── Enemies (in canonical enemy_uids order — slot identity matters) ─
    0x05u8.hash(h);
    (state.enemy_uids.len() as u64).hash(h);
    for uid in &state.enemy_uids {
        uid.get().hash(h);
        if let Some(c) = crate::creature_by_id(&state.creatures, uid.0) {
            c.id.get().hash(h);
            c.current_hp.hash(h);
            c.max_hp.hash(h);
            c.block.hash(h);
            // `enemy_uids` only carries enemies, so `species` should
            // always be `Some(_)` here; bead `sts2-rs-a19m` migrated
            // off the `MonsterKind::ShrinkerBeetle` player sentinel.
            // `species`/`intent` both derive `Hash`, so hash the values
            // directly rather than allocating a Debug string per enemy.
            c.species.hash(h);
            c.monster_id.0.hash(h);
            c.intent.hash(h);
            hash_powers(&c.powers, state, h);
        } else {
            0xDEADu16.hash(h);
        }
    }

    // ── Cards arena (uid order) ─────────────────────────────────────────
    // Iteration order is per-strategy (`E::for_each_card`): the fast path
    // iterates the arena directly (append-only ascending uids — see
    // [`FastEncoding`]); the legacy path keeps its historical collect+sort.
    // Under the ascending-uid invariant both orders are identical, so the
    // per-card field walk below is shared verbatim.
    0x06u8.hash(h);
    (state.cards.len() as u64).hash(h);
    E::for_each_card(state, |c| {
        c.uid.get().hash(h);
        // `card_id` / `card_type` / `target_type` all derive `Hash`; hash
        // the enums directly instead of materializing a Debug string per
        // card (this block runs once per card, every node expansion).
        c.card_id.hash(h);
        c.cost.hash(h);
        c.card_type.hash(h);
        c.target_type.hash(h);
        c.upgraded.hash(h);
        c.base_damage.hash(h);
        c.base_block.hash(h);
        // `VarKey` / `EnchantmentId` derive `Ord`, so sort by the key
        // itself rather than its formatted string. The sort order changes
        // (declaration order vs lexicographic), but equality is preserved
        // — equal maps still produce the same canonical sequence, which is
        // all the hash contract requires.
        hash_entries_by_key(c.combat_vars.as_slice(), h);
        hash_entries_by_key(c.enchantments.as_slice(), h);
        // Card afflictions (Bound / ChainsOfBinding "freeze" from the Queen
        // boss, etc.) are real searchable state: two hands differing only in
        // which cards are Bound are genuinely different positions and must
        // NOT alias in the transposition table. `CardAffliction: Copy + Hash`.
        c.affliction.hash(h);
    });

    // ── Potions / relics — slot order matters in Python, so we preserve
    // insertion order here (not sorted). Phase 7 will introduce real
    // payloads; for now just the uid sequence.
    0x07u8.hash(h);
    (state.potions.len() as u64).hash(h);
    for p in &state.potions {
        p.id.get().hash(h);
    }
    0x08u8.hash(h);
    (state.relics.len() as u64).hash(h);
    for r in &state.relics {
        r.id.get().hash(h);
        // Kind encoding is per-strategy: fast hashes the `RelicKind` enum
        // directly (no per-relic allocation); legacy keeps its historical
        // Debug-string encoding.
        E::relic_kind(r.kind, h);
    }
    // ToyBox melt state. Folded in ONLY when something is actually melted so
    // every pre-ToyBox state keeps its historical hash byte-for-byte (the
    // same reason `counter` is absent from this stream). Melt cannot change
    // mid-combat — `ToyBox.AfterCombatEnd` runs at combat end — so this can
    // never split a transposition inside one search; it exists so a
    // cross-combat cache cannot confuse a live relic with a dead one.
    if state.relics.iter().any(|r| r.is_melted) {
        0x1Fu8.hash(h);
        for r in &state.relics {
            if r.is_melted {
                r.id.get().hash(h);
            }
        }
    }

    // ── Pending choice / pending play ───────────────────────────────────
    0x09u8.hash(h);
    hash_pending_choice(state.pending_choice.as_ref(), h);
    state.pending_play.is_some().hash(h);
    // Auto-play suspended on a surfaced card-select (parity bug
    // `combat_select_ironclad`) — track presence like `pending_play`.
    state.pending_autoplay_resume.is_some().hash(h);
}

/// Fold a card's `(key, amount)` map (`combat_vars` / `enchantments`) into
/// `h` in ascending key order.
///
/// Most maps are empty or already in key order, and sorting one of those
/// moves nothing (`sort_unstable` leaves a non-decreasing slice as it is),
/// so it is hashed straight from the slice. Collecting it into a `Vec` cost
/// a malloc/free pair per card with any entry, on every hash. An
/// out-of-order map is still sorted exactly as before (a `Vec` of
/// references, `sort_unstable_by_key`), so the hash bytes are the same on
/// both paths, a repeated key included.
fn hash_entries_by_key<K: Copy + Ord + Hash, H: Hasher>(entries: &[(K, i32)], h: &mut H) {
    if entries.windows(2).all(|w| w[0].0 <= w[1].0) {
        for (k, v) in entries {
            k.hash(h);
            v.hash(h);
        }
        return;
    }
    let mut sorted: Vec<_> = entries.iter().collect();
    sorted.sort_unstable_by_key(|(k, _)| *k);
    for (k, v) in sorted {
        k.hash(h);
        v.hash(h);
    }
}

/// Hash every stream's `seed()` + `counter()` (the cursor) of a
/// [`RunRngSet`] so state-equality tracks RNG position. Shared by
/// [`state_hash`] (combat-side `state.run_rng`) and [`run_hash`]
/// (run-side `RunState.rng`).
fn hash_run_rng_set<H: Hasher>(rr: &RunRngSet, h: &mut H) {
    rr.seed.hash(h);
    for stream in [
        &rr.up_front,
        &rr.shuffle,
        &rr.unknown_map_point,
        &rr.combat_card_generation,
        &rr.combat_potion_generation,
        &rr.combat_card_selection,
        &rr.combat_energy_costs,
        &rr.combat_targets,
        &rr.monster_ai,
        &rr.niche,
        &rr.combat_orbs,
        &rr.treasure_room,
        &rr.run_manager,
        &rr.rewards,
        &rr.shops,
        &rr.transformations,
    ] {
        stream.seed().hash(h);
        stream.counter().hash(h);
    }
    // `map_rngs` is a `BTreeMap` — deterministic key iteration order.
    for (act_index, rng) in &rr.map_rngs {
        act_index.hash(h);
        rng.seed().hash(h);
        rng.counter().hash(h);
    }
}

/// Compute a deterministic structural hash of an entire run
/// ([`RunManager`]).
///
/// Bead `sts2-rs` (AutoSlay-inspired full-run fuzzer, B2): the full-run
/// fuzzer needs a run-level analog of [`state_hash`] for **livelock
/// detection** (the hash unchanged across a window of steps ⇒ the run
/// made no observable progress) and for trace dedup. `state_hash` only
/// covers a [`CombatState`]; this folds the run scalars, deck, relics,
/// potions, map cursor, run-level RNG cursor, and the live combat
/// sub-state (when in [`crate::run_manager::Phase::Combat`]).
///
/// Folded, in fixed order:
///   * `phase`, `seed`, `character_id`, `ascension_level`
///   * run scalars: current/max HP, gold, act/floor cursors, `is_over`,
///     `player_won`, pending-removal counters
///   * the master deck (in `Vec` order — order is observable), relics +
///     relic counters, the potion belt
///   * `visited_map_coords` (the map cursor)
///   * the run-level RNG cursor ([`RunRngSet`] stream seeds + counters)
///   * the run-level `pending_choice`
///   * the live [`CombatState`] via [`state_hash`] when a combat is active
///
/// Same stability contract as [`state_hash`]: stable within a compiled
/// binary, NOT across pin bumps.
#[must_use]
pub fn run_hash(rm: &RunManager) -> u64 {
    legacy_run::run_hash(rm)
}

/// Compute a deterministic structural hash of an entire run
/// ([`RunManager`]) that is **insensitive to RNG cursor position** —
/// identical to [`run_hash`] but with the run-level [`RunRngSet`] cursor
/// block omitted, and with any live combat sub-state hashed via
/// [`search_hash`] (also cursor-insensitive) instead of [`state_hash`].
///
/// This is the transposition/dedup key for the **outer-loop MCTS**
/// (AlphaZero-style planner, AZ M3): two runs that reached the same
/// observable state via different RNG draws (map rolls, shop rolls,
/// reward rolls, or a different combat outcome seed) are the same node
/// for search purposes and must merge in the transposition table, the
/// same way [`search_hash`] merges chance-node combat outcomes.
///
/// Same stability contract as [`run_hash`]: stable within a compiled
/// binary, NOT across pin bumps.
#[must_use]
pub fn run_search_hash(rm: &RunManager) -> u64 {
    legacy_run::run_search_hash(rm)
}

/// The two per-strategy encoding points of the shared field walk. The
/// fast ([`state_hash`]/[`search_hash`]) and legacy ([`legacy_run`]) hash
/// paths walk the exact same field set in the exact same order; they
/// differ ONLY in the hasher (fixed-seed ahash vs SipHash `DefaultHasher`,
/// handled by the `H: Hasher` generics) and in these three encodings.
/// Both impls are zero-sized; the strategy is a compile-time parameter.
trait HashEncoding {
    /// Fingerprint one pile's card uids (length + order-invariant body).
    fn pile<H: Hasher>(pile: &[crate::CardUid], h: &mut H);
    /// Encode a relic's kind discriminant.
    fn relic_kind<H: Hasher>(kind: crate::relics::RelicKind, h: &mut H);
    /// Visit every card in the arena in the strategy's canonical order.
    fn for_each_card(state: &CombatState, f: impl FnMut(&crate::Card));
}

/// Fast-path encodings (fixed-seed ahash hashes, [`state_hash`] /
/// [`search_hash`]): sort-free [`pile_mix`] pile fingerprint, direct
/// `RelicKind` enum hash, direct arena iteration.
struct FastEncoding;

impl HashEncoding for FastEncoding {
    #[inline]
    fn pile<H: Hasher>(pile: &[crate::CardUid], h: &mut H) {
        (pile.len() as u64).hash(h);
        pile_mix(pile).hash(h);
    }

    #[inline]
    fn relic_kind<H: Hasher>(kind: crate::relics::RelicKind, h: &mut H) {
        // `RelicKind` derives `Hash`; hash the enum directly instead of
        // allocating a Debug string per relic per node expansion.
        kind.hash(h);
    }

    #[inline]
    fn for_each_card(state: &CombatState, mut f: impl FnMut(&crate::Card)) {
        // The combat card arena is append-only with ascending uids
        // (production sites all assign `uid == cards.len() + 1`, the
        // canonical lookup pattern from bead P1-2; fixtures may skip
        // values but still append ascending), so plain iteration IS
        // uid-ascending order — the collect + sort the pre-2026-07-05
        // code paid here (~5.5K calls/search via `search_hash`) was a
        // no-op permutation.
        debug_assert!(
            state.cards.windows(2).all(|w| w[0].uid < w[1].uid),
            "card arena uids not ascending; hash_piles relies on append-only uid order"
        );
        for c in &state.cards {
            f(c);
        }
    }
}

/// Frozen legacy encodings (SipHash `DefaultHasher`, [`legacy_run`]):
/// sorted-uid-sequence pile fingerprint, Debug-string relic kind,
/// collect + sort-by-uid arena order. These byte streams are pinned by
/// `tests::legacy_run_hash_values_pinned` — do NOT change them.
struct LegacyEncoding;

impl HashEncoding for LegacyEncoding {
    fn pile<H: Hasher>(pile: &[crate::CardUid], h: &mut H) {
        let mut sorted: Vec<u32> = pile.iter().map(|u| u.get()).collect();
        sorted.sort_unstable();
        (sorted.len() as u64).hash(h);
        for uid in sorted {
            uid.hash(h);
        }
    }

    fn relic_kind<H: Hasher>(kind: crate::relics::RelicKind, h: &mut H) {
        format!("{kind:?}").hash(h);
    }

    fn for_each_card(state: &CombatState, mut f: impl FnMut(&crate::Card)) {
        // Historical sorted walk, kept verbatim. Under the ascending-uid
        // arena invariant (debug_asserted on the fast path) this is the
        // same order as plain iteration — the legacy contract only covers
        // real production states, which satisfy it — but the sort is kept
        // so the pinned values hold even for pathological arenas.
        let mut card_idx: Vec<&crate::Card> = state.cards.iter().collect();
        card_idx.sort_by_key(|c| c.uid.get());
        for c in card_idx {
            f(c);
        }
    }
}

/// Salt mixed into every per-uid [`pile_mix`] term. Arbitrary odd
/// constant; it only de-trivializes the splitmix64 input so small
/// consecutive uids don't feed the mixer near-zero values.
const PILE_MIX_SALT: u64 = 0xA076_1D64_78BD_642F;

/// Order-invariant commutative fingerprint of one pile's card uids:
/// the wrapping sum of `splitmix64(uid ^ PILE_MIX_SALT)` over the pile.
///
/// Perf (2026-07-04 realistic-deck callgrind): `search_hash` +
/// `hash_piles` ran ~5.5K times per search (~3.5–4% of instructions),
/// and the previous implementation collected every pile into a
/// `SmallVec<[u32; 48]>` scratch and `sort_unstable`d it purely to make
/// the hash insertion-order-invariant. The commutative sum needs no
/// scratch and no sort while keeping every invariant the sort provided:
/// order-invariance (addition commutes), determinism, and multiset
/// sensitivity (splitmix64 is a bijection on `u64`, so distinct uids
/// contribute well-mixed, independent terms; the pile length is hashed
/// alongside the sum). The concrete hash VALUE changes — allowed by the
/// module's ephemeral-hash contract (stable within a build only).
#[inline]
fn pile_mix(pile: &[crate::CardUid]) -> u64 {
    let mut acc = 0u64;
    for uid in pile {
        acc = acc.wrapping_add(crate::run_rng_set::splitmix64(
            u64::from(uid.get()) ^ PILE_MIX_SALT,
        ));
    }
    acc
}

/// Hash one [`PlayerCombatState`] with order-invariant pile semantics
/// (`E::pile` — [`pile_mix`] on the fast path, sorted uid sequence on the
/// legacy path). The pile discriminant is prefixed so a card moving
/// between piles changes the hash even if the union is unchanged.
fn hash_player<E: HashEncoding, H: Hasher>(p: &PlayerCombatState, h: &mut H, state: &CombatState) {
    for (kind, pile) in [
        (PileKind::Hand, p.hand.as_slice()),
        (PileKind::Draw, p.draw.as_slice()),
        (PileKind::Discard, p.discard.as_slice()),
        (PileKind::Exhaust, p.exhaust.as_slice()),
        (PileKind::Play, p.play.as_slice()),
    ] {
        kind.hash(h);
        E::pile(pile, h);
    }
    hash_player_tail(p, h, state);
}

/// Scalar / power / orb tail shared by [`hash_player`] and
/// [`hash_player_chance_dedup`] (bead `sts2-rs-i8bgu`, F1) — everything
/// about a [`PlayerCombatState`] EXCEPT the five card piles, which the two
/// callers encode differently. Pure extraction from [`hash_player`]'s
/// prior body; same hash calls in the same order.
fn hash_player_tail<H: Hasher>(p: &PlayerCombatState, h: &mut H, state: &CombatState) {
    p.energy.hash(h);
    p.max_energy.hash(h);
    p.stars.hash(h);
    p.current_hp.hash(h);
    p.max_hp.hash(h);
    p.block.hash(h);
    // Player powers: canonical-order hash including kind/amount/counter/
    // applier per-instance scalars. Bead `sts2-rs-6ynt.6.11` widened this
    // from a uid-only fingerprint (the pre-bead body only hashed
    // `power.id`, masking counter/applier diffs on player-side stacked
    // powers like Unmovable / CrimsonMantle / Inferno).
    hash_powers(&p.powers, state, h);

    // Defect orb belt. Bead `sts2-rs-jyd15`: the orb belt
    // (Lightning/Frost/Dark passive + evoke payloads) is observable
    // per-player combat state, but was absent from the hash — two states
    // differing only in orb contents collided, letting an MCTS transposition
    // table alias semantically distinct nodes. Orb slot ORDER is significant
    // (FIFO channel order, front = oldest), so the belt is hashed in
    // sequence, not sorted, alongside its capacity. `Orb` derives `Hash`;
    // hash the values directly. (The same bead also hashed `player.stance`;
    // that field was removed wholesale in GH #408 as dead STS1 state.)
    p.orbs.capacity.hash(h);
    (p.orbs.slots.len() as u64).hash(h);
    for orb in &p.orbs.slots {
        orb.hash(h);
    }

    // The potion belt (sts2-agent fork), when the search prices it (the state
    // carries a belt value): which potions the player holds, in belt order (the
    // physical slot indices are left out: a state's index list may be filled
    // lazily, which would split equal states). Without it, two states differing
    // only in the potion left (drink the Fire Potion or the Explosive Ampoule)
    // merged in the MCTS transposition table, which is always on: both drinks
    // shared one child, its values and visits, so a set-aware belt value could
    // not tell them apart. Left out without a belt value, as the deployed engine
    // hashes (its searches and the parity goldens unchanged).
    if state.belt_value.is_some() {
        (p.potions.len() as u64).hash(h);
        for kind in &p.potions {
            kind.hash(h);
        }
    }

    // Necrobinder Osty pet: the pet Creature lives in `state.creatures`
    // but is in NEITHER `enemy_uids` NOR `allies`, so no other hash
    // section visits it — without this block two states differing only in
    // Osty hp/max_hp/block/powers (or alive vs dead-but-revivable) collide
    // in the MCTS transposition table. Osty MaxHp compounds across summons
    // and resets on death, and Unleash/Protector/Sacrifice/DieForYou all
    // key off these fields. The raw `pet_uid` value is deliberately NOT
    // hashed (it is an arena index — history-dependent, not observable);
    // presence + observable creature fields are.
    match p
        .pet_uid
        .and_then(|uid| state.creatures.iter().find(|c| c.id == uid.0))
    {
        Some(c) => {
            1u8.hash(h);
            c.current_hp.hash(h);
            c.max_hp.hash(h);
            c.block.hash(h);
            hash_powers(&c.powers, state, h);
        }
        None => 0u8.hash(h),
    }
}

/// [`chance_outcome_hash`]'s player encoding (bead `sts2-rs-i8bgu`, F1):
/// identical to [`hash_player`] except the DRAW pile uses
/// [`hash_draw_pile_chance_dedup`] instead of the uid-based [`pile_mix`].
///
/// HAND gets the same observational treatment (via
/// [`hash_pile_observational`]), not just DRAW: the card a draw-chance
/// outcome adds lands in HAND, so a physically-different-but-
/// observationally-equal drawn card (two un-upgraded Strikes with
/// different `uid`s) would still fail to dedup if HAND stayed
/// `pile_mix`-encoded — that's the exact case this hash exists to
/// collapse (a starter deck's duplicate Strikes/Defends). Discard/Exhaust/
/// Play are left exactly as [`FastEncoding::pile`] encodes them in
/// [`search_hash`] — a plain draw only ever moves a card into Hand (the
/// `draw` full-hand semantics never redirect to Discard), so those piles
/// don't carry the ambiguity this hash resolves; scoping the change there
/// too would only widen dedup further, never break it, but is left for a
/// follow-up if a redirect-to-discard draw path is found to matter.
fn hash_player_chance_dedup<H: Hasher>(p: &PlayerCombatState, h: &mut H, state: &CombatState) {
    PileKind::Hand.hash(h);
    hash_pile_observational(&p.hand, state, h);
    for (kind, pile) in [
        (PileKind::Discard, p.discard.as_slice()),
        (PileKind::Exhaust, p.exhaust.as_slice()),
        (PileKind::Play, p.play.as_slice()),
    ] {
        kind.hash(h);
        FastEncoding::pile(pile, h);
    }
    PileKind::Draw.hash(h);
    hash_draw_pile_chance_dedup(p, state, h);
    hash_player_tail(p, h, state);
}

/// The DRAW-pile encoding for [`chance_outcome_hash`]: pinned positions
/// stay uid/position-sensitive (player-KNOWN information, hashed exactly
/// like [`FastEncoding::pile`] would over just that sub-sequence), and
/// the unknown middle ([`crate::cards::unknown_middle_uids`]) is folded
/// via [`hash_pile_observational`] — order- AND instance-uid-insensitive,
/// content-sensitive.
fn hash_draw_pile_chance_dedup<H: Hasher>(p: &PlayerCombatState, state: &CombatState, h: &mut H) {
    (p.draw.len() as u64).hash(h);
    (p.pinned_top.len() as u64).hash(h);
    for uid in &p.pinned_top {
        uid.get().hash(h);
    }
    let middle = crate::cards::unknown_middle_uids(p);
    hash_pile_observational(&middle, state, h);
    (p.pinned_bottom.len() as u64).hash(h);
    for uid in &p.pinned_bottom {
        uid.get().hash(h);
    }
}

/// Fold `pile` into `h` as a SORTED list of [`card_observational_key`]
/// values — order- AND instance-uid-insensitive, content-sensitive.
/// Shared by [`hash_player_chance_dedup`]'s HAND encoding and
/// [`hash_draw_pile_chance_dedup`]'s unknown-middle encoding. A `uid`
/// with no matching arena entry is dropped (conservative: it contributes
/// nothing rather than panicking or aliasing another card's key).
fn hash_pile_observational<H: Hasher>(pile: &[crate::CardUid], state: &CombatState, h: &mut H) {
    // Stack buffer sized like `unknown_middle_uids`' (a hand never comes
    // close); the sorted `u64`s are the same whatever holds them.
    let mut keys: smallvec::SmallVec<[u64; 32]> = pile
        .iter()
        .filter_map(|uid| crate::card_by_uid(&state.cards, *uid))
        .map(card_observational_key)
        .collect();
    keys.sort_unstable();
    (keys.len() as u64).hash(h);
    for k in &keys {
        k.hash(h);
    }
}

/// Content-only fingerprint of `card`, excluding its [`crate::CardUid`] (and a
/// non-X card's per-play `energy_spent`, see
/// [`crate::cards::observational_energy_spent`]) —
/// the identity a player can actually observe. Mirrors
/// [`crate::cards::cards_observationally_equal`]'s all-fields-except-uid
/// comparison by construction (clone, unify `uid` on a fixed sentinel,
/// hash the whole card via its derived `Hash`) rather than hand-listing
/// fields, so the two stay in lockstep automatically as [`crate::Card`]
/// grows — a new field joins the derive, and a non-`Hash` field fails to
/// compile. Until 2026-08-17 the "hash the whole card" step was
/// `format!("{c:?}")` over the clone; under the PR #467 EndTurn-widening
/// fan-out that Debug rendering dominated the entire MCTS search profile,
/// so it now hashes the struct directly — same equality classes (both
/// encodings are injective over the non-uid fields up to hash collision),
/// different hash VALUES, which only ever feed the chance-outcome dedup
/// comparison. Used only by [`hash_pile_observational`].
fn card_observational_key(card: &crate::Card) -> u64 {
    let sentinel = crate::CardUid::new(1).expect("1 is a valid non-zero CardUid");
    let mut c = *card;
    c.uid = sentinel;
    c.energy_spent = crate::cards::observational_energy_spent(card);
    let mut h = new_hasher();
    c.hash(&mut h);
    h.finish()
}

fn hash_powers<H: Hasher>(power_uids: &[crate::PowerUid], state: &CombatState, h: &mut H) {
    // `counter` (per-instance scratch) and `applier` (per-instance source-of-
    // application uid) are part of canonical state — two `Power` records that
    // differ only in either field are observationally distinct (Unmovable's
    // block-gains-this-turn, CrimsonMantle's self_damage accumulator,
    // Grapple's applier-gated listener). Bead `sts2-rs-6ynt.6.11` extends the
    // hashing tuple to include them; `applier` is hashed as `Option<u32>`
    // (0 = `None`, non-zero = uid value + 1 to keep `Some(0)` and `None`
    // distinct, though `CreatureUid` is `NonZeroU32` so the encoding is
    // strictly total either way).
    // `PowerKind` derives `Hash + Ord`, so carry the enum itself in the
    // sort tuple instead of its Debug string (avoids a per-power
    // allocation in this hot helper). `uid` is the unique primary sort
    // key, so the kind only ever acts as an unreachable tiebreak — the
    // canonical order is identical to the prior string-keyed sort.
    let mut rows: smallvec::SmallVec<[(u32, crate::PowerKind, i32, i32, u32); 16]> = power_uids
        .iter()
        .filter_map(|puid| {
            let p = state.powers.get_by_id(puid.0)?;
            let applier_key = p.applier.map_or(0u32, |uid| uid.0.get());
            Some((puid.get(), p.kind, p.amount, p.counter, applier_key))
        })
        .collect();
    rows.sort_unstable();
    (rows.len() as u64).hash(h);
    for (uid, kind, amount, counter, applier_key) in rows {
        uid.hash(h);
        kind.hash(h);
        amount.hash(h);
        counter.hash(h);
        applier_key.hash(h);
    }
}

/// Mix a [`crate::PendingChoice`] (or its absence) into the running
/// hasher in a deterministic, total way. Order of `options` is
/// significant — Python's `selected_indices` is a `set[int]` indexed
/// into `options`, so a reordered list is a semantically distinct
/// choice. `card_uid` is hashed as `Option<u32>` (using `0` for the
/// `None` branch sentinel tag) so the discriminator is total.
fn hash_pending_choice<H: Hasher>(pc: Option<&crate::PendingChoice>, h: &mut H) {
    match pc {
        None => 0x00u8.hash(h),
        Some(pc) => {
            0x01u8.hash(h);
            pc.kind.hash(h);
            pc.min_select.hash(h);
            pc.max_select.hash(h);
            pc.can_skip.hash(h);
            // `PostChoiceEffect` derives `Hash`; mix Option<_> totally
            // (sentinel tag for the `None` branch). Bead `sts2-rs-pe5zo`.
            match pc.post_effect {
                None => 0x00u8.hash(h),
                Some(eff) => {
                    0x01u8.hash(h);
                    eff.hash(h);
                }
            }
            (pc.options.len() as u64).hash(h);
            for opt in &pc.options {
                opt.card_id.hash(h);
                opt.upgraded.hash(h);
                match opt.card_uid {
                    None => 0x00u8.hash(h),
                    Some(uid) => {
                        0x01u8.hash(h);
                        uid.get().hash(h);
                    }
                }
                match opt.source_pile {
                    None => 0x00u8.hash(h),
                    Some(pile) => {
                        0x01u8.hash(h);
                        pile.hash(h);
                    }
                }
            }
            // Bead `sts2-rs-sopgf`: mix the sequential multi-pick progress so
            // two states mid-accumulation with different already-picked sets
            // hash distinctly (an MCTS node at `picked=[0]` is NOT the same
            // decision as one at `picked=[1]`). Empty for single-pick / fresh.
            (pc.picked.len() as u64).hash(h);
            for idx in &pc.picked {
                idx.hash(h);
            }
        }
    }
}

const fn discriminant_phase(p: CombatPhase) -> u8 {
    match p {
        CombatPhase::Setup => 0,
        CombatPhase::PlayerTurnStart => 1,
        CombatPhase::PlayerPlayPhase => 2,
        CombatPhase::PlayerEndTurnPhaseOne => 3,
        CombatPhase::PlayerEndTurnPhaseTwo => 4,
        CombatPhase::EnemyTurnStart => 5,
        CombatPhase::EnemyTurnExec => 6,
        CombatPhase::EnemyTurnEnd => 7,
        CombatPhase::Resolved { .. } => 8,
    }
}

const fn discriminant_side(s: Side) -> u8 {
    match s {
        Side::Player => 0,
        Side::Enemy => 1,
    }
}

/// Frozen pre-2026-07-04 (SipHash `DefaultHasher`) hash instantiations,
/// kept EXCLUSIVELY for the run-level planner keys [`run_hash`] /
/// [`run_search_hash`].
///
/// Why: the outer-loop (sts2-runai) planner derives its chance-node
/// determinization bases from these hashes — including the live-combat
/// sub-hash branch — and its loop-closure fixtures (trained prior nets +
/// pinned seeds) discriminate only under the exact historical hash values.
/// Run-level hashing is COLD (per outer-planner node, not per combat
/// simulation), so it keeps the byte-exact legacy values, while the hot
/// combat-side [`state_hash`] / [`search_hash`] moved to fixed-seed ahash +
/// the sort-free pile mix.
///
/// The field walk itself is SHARED with the fast path (the generic
/// [`hash_scalars`] / [`hash_piles`] / [`state_hash_walk`] /
/// [`hash_game_state`] walk in the parent module — `Hash::hash` writes
/// identical byte streams to any `Hasher`); only two things are pinned
/// here: the hasher (`DefaultHasher`) and the [`LegacyEncoding`] strategy
/// (sorted-uid pile fingerprint, Debug-string relic kind, sorted card-arena
/// walk). The exact values are locked by the
/// `tests::legacy_run_hash_values_pinned` constants. If the run-level
/// fixtures are ever retrained/retuned, this module can be deleted and the
/// run hashes pointed at the fast path.
mod legacy_run {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    use crate::run_manager::RunManager;
    use crate::CombatState;

    use super::{
        hash_game_state, hash_pending_choice, hash_run_rng_set, state_hash_walk, LegacyEncoding,
    };

    /// Legacy [`super::state_hash`]: the shared full walk (scalars + RNG
    /// cursors + piles) under `DefaultHasher` + [`LegacyEncoding`].
    #[must_use]
    fn state_hash(state: &CombatState) -> u64 {
        let mut h = DefaultHasher::new();
        state_hash_walk::<LegacyEncoding, _>(state, &mut h);
        h.finish()
    }

    /// Legacy [`super::search_hash`]: the shared game-observable walk
    /// (RNG-cursor-insensitive) under `DefaultHasher` + [`LegacyEncoding`].
    #[must_use]
    fn search_hash(state: &CombatState) -> u64 {
        let mut h = DefaultHasher::new();
        hash_game_state::<LegacyEncoding, _>(state, &mut h);
        h.finish()
    }

    /// Shared body of [`run_hash`] / [`run_search_hash`] — the two differ
    /// ONLY in the section-tag base byte (`0xA0` vs `0xB0`), whether the
    /// run-level RNG-cursor section is folded in, and which combat
    /// sub-hash keys a live combat. Byte streams are unchanged from the
    /// historical hand-unrolled pair.
    fn run_hash_impl(
        rm: &RunManager,
        tag_base: u8,
        include_rng: bool,
        combat_hash: fn(&CombatState) -> u64,
    ) -> u64 {
        let mut h = DefaultHasher::new();
        let rs = &rm.run_state;

        // ── Run identity + phase ────────────────────────────────────────
        tag_base.hash(&mut h);
        rm.phase().hash(&mut h);
        rm.seed.hash(&mut h);
        format!("{:?}", rm.character_id).hash(&mut h);
        rm.ascension_level.hash(&mut h);

        // ── Run scalars ─────────────────────────────────────────────────
        (tag_base + 1).hash(&mut h);
        rs.current_hp.hash(&mut h);
        rs.max_hp.hash(&mut h);
        rs.gold.hash(&mut h);
        rs.act_index.hash(&mut h);
        rs.act_floor.hash(&mut h);
        rs.total_floor.hash(&mut h);
        rs.is_over.hash(&mut h);
        rs.player_won.hash(&mut h);
        rs.pending_deck_removals.hash(&mut h);
        rs.interactive_card_removal.hash(&mut h);
        rs.pending_deck_upgrades.hash(&mut h);
        rs.interactive_card_upgrade.hash(&mut h);
        rs.card_shop_removals_used.hash(&mut h);

        // ── Master deck (Vec order is observable) ───────────────────────
        (tag_base + 2).hash(&mut h);
        (rs.master_deck.len() as u64).hash(&mut h);
        for c in &rs.master_deck {
            c.uid.get().hash(&mut h);
            format!("{:?}", c.card_id).hash(&mut h);
            c.cost.hash(&mut h);
            c.upgraded.hash(&mut h);
        }

        // ── Relics + counters + potions ─────────────────────────────────
        (tag_base + 3).hash(&mut h);
        (rs.relics.len() as u64).hash(&mut h);
        for r in &rs.relics {
            format!("{r:?}").hash(&mut h);
        }
        // `relic_counters` is a `BTreeMap` — deterministic iteration order.
        for (rid, count) in &rs.relic_counters {
            format!("{rid:?}").hash(&mut h);
            count.hash(&mut h);
        }
        // ToyBox melt state, folded in only when non-empty so every run
        // without a ToyBox keeps its historical hash byte-for-byte. Two runs
        // that differ ONLY in which wax relics are still live are genuinely
        // different positions.
        if !rs.melted_relics.is_empty() {
            for rid in &rs.melted_relics {
                format!("melted:{rid:?}").hash(&mut h);
            }
        }
        (rs.potions.len() as u64).hash(&mut h);
        for p in &rs.potions {
            format!("{p:?}").hash(&mut h);
        }

        // ── Map cursor ──────────────────────────────────────────────────
        (tag_base + 4).hash(&mut h);
        (rs.visited_map_coords.len() as u64).hash(&mut h);
        for coord in &rs.visited_map_coords {
            format!("{coord:?}").hash(&mut h);
        }

        // ── Run-level RNG cursor ────────────────────────────────────────
        // Folded by `run_hash`; DELIBERATELY OMITTED (tag included) by
        // `run_search_hash` — see the public doc-comments.
        if include_rng {
            (tag_base + 5).hash(&mut h);
            hash_run_rng_set(&rs.rng, &mut h);
        }

        // ── Pending choice (reuse the combat-side payload hasher) ───────
        (tag_base + 6).hash(&mut h);
        hash_pending_choice(rs.pending_choice.as_ref(), &mut h);

        // ── Live combat sub-state ───────────────────────────────────────
        (tag_base + 7).hash(&mut h);
        match &rm.combat {
            None => 0x00u8.hash(&mut h),
            Some(c) => {
                0x01u8.hash(&mut h);
                combat_hash(c).hash(&mut h);
            }
        }

        h.finish()
    }

    /// Legacy run hash — see [`super::run_hash`] for the field-set
    /// contract. Cursor-sensitive: folds the run-level RNG cursors and
    /// keys any live combat via the cursor-sensitive [`state_hash`].
    #[must_use]
    pub(super) fn run_hash(rm: &RunManager) -> u64 {
        run_hash_impl(rm, 0xA0, true, state_hash)
    }

    /// Legacy run search hash — see [`super::run_search_hash`] for the
    /// field-set contract. Cursor-INsensitive: omits the run-level RNG
    /// cursor section and keys any live combat via [`search_hash`].
    #[must_use]
    pub(super) fn run_search_hash(rm: &RunManager) -> u64 {
        run_hash_impl(rm, 0xB0, false, search_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CardUid, CombatState};
    use smallvec::SmallVec;

    #[test]
    fn deterministic_same_state() {
        let a = CombatState::default();
        let b = CombatState::default();
        assert_eq!(state_hash(&a), state_hash(&b));
    }

    #[test]
    fn changes_with_round() {
        let mut a = CombatState::default();
        let b_hash = state_hash(&a);
        a.round = 7;
        assert_ne!(state_hash(&a), b_hash);
    }

    // ── search_hash: RNG-cursor-INsensitive transposition key ──────────
    // (chance-node MCTS, bead feat/mcts-chance-nodes). `search_hash` must
    // ignore RNG cursor advances that `state_hash` tracks, while still
    // reacting to every game-observable field.

    #[test]
    fn search_hash_deterministic_same_state() {
        let a = CombatState::default();
        let b = CombatState::default();
        assert_eq!(search_hash(&a), search_hash(&b));
    }

    #[test]
    fn search_hash_ignores_rng_cursor() {
        // Advancing the combat RNG streams must NOT change `search_hash`
        // (so game-identical outcomes from different outcome seeds merge),
        // but MUST change `state_hash` (the default PIMC TT key).
        let mut a = CombatState::default();
        let s0 = search_hash(&a);
        let h0 = state_hash(&a);
        let _ = a.run_rng.shuffle.next_int(0, 100);
        let _ = a.run_rng.monster_ai.next_int(0, 100);
        a.rng = crate::dotnet_rng::Rng::with_counter(a.rng.seed() as i32, a.rng.counter() + 3);
        assert_eq!(search_hash(&a), s0, "search_hash must ignore RNG cursor");
        assert_ne!(state_hash(&a), h0, "state_hash must track RNG cursor");
    }

    #[test]
    fn search_hash_reacts_to_game_state() {
        let mut a = CombatState::default();
        let base = search_hash(&a);
        a.round = 7;
        assert_ne!(
            search_hash(&a),
            base,
            "search_hash must react to game-observable fields"
        );
    }

    // ── chance_outcome_hash: order/uid-insensitive dedup key for
    // DrawChance chance outcomes (bead sts2-rs-i8bgu, F1 plan §3.2 step 3) ─

    fn draw_state(cards: &[(crate::cards::CardId, bool)]) -> CombatState {
        let mut s = CombatState::default();
        for (i, &(card_id, upgraded)) in cards.iter().enumerate() {
            let uid = CardUid::new(i as u32 + 1).expect("non-zero");
            s.cards.push(crate::Card {
                uid,
                card_id,
                upgraded,
                ..crate::Card::pod_defaults()
            });
            s.player.draw.push(uid);
        }
        s
    }

    #[test]
    fn chance_outcome_hash_deterministic_same_state() {
        use crate::cards::CardId;
        let a = draw_state(&[(CardId::StrikeIronclad, false), (CardId::Bash, false)]);
        let b = draw_state(&[(CardId::StrikeIronclad, false), (CardId::Bash, false)]);
        assert_eq!(chance_outcome_hash(&a), chance_outcome_hash(&b));
    }

    #[test]
    fn chance_outcome_hash_ignores_which_duplicate_instance_is_where() {
        use crate::cards::CardId;
        // Two states: same OBSERVABLE draw pile (two un-upgraded Strikes +
        // one Bash), but the two physical Strike instances are swapped —
        // uid 1 <-> uid 2 hold the Strike identity while uid 3 is the
        // Bash in both. `search_hash`'s pile fingerprint (uid-based) is
        // insensitive to which SPECIFIC Strike occupies which SLOT (order
        // is commutative), but distinguishes "Strike uid=1 in the pile"
        // from "a *different* Strike uid" only when the SET of uids
        // present differs — which it doesn't here, both states use
        // {1,2,3}. So this scenario alone doesn't yet exercise anything
        // `search_hash` couldn't already do; the real ambiguity this hash
        // resolves is a DIFFERENT uid drawing an observationally-equal
        // card — modeled below by dropping one Strike from the pile
        // (simulating "this Strike was drawn") in two states that use
        // DIFFERENT physical uids for the remaining Strike.
        let mut a = draw_state(&[
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::Bash, false),
        ]);
        let mut b = a.clone();
        // In `a`, "draw" uid 1 (leaving uid 2 Strike + uid 3 Bash).
        a.player.draw.retain(|u| *u != CardUid::new(1).unwrap());
        // In `b`, "draw" uid 2 instead (leaving uid 1 Strike + uid 3 Bash)
        // — same OBSERVABLE remaining pile (one Strike + one Bash), but a
        // physically different Strike uid remains.
        b.player.draw.retain(|u| *u != CardUid::new(2).unwrap());

        assert_eq!(
            chance_outcome_hash(&a),
            chance_outcome_hash(&b),
            "remaining piles are observationally identical (1 Strike + 1 \
             Bash) regardless of which physical Strike uid is left — must \
             hash equal so the chance outcomes dedup"
        );
        // Sanity: search_hash (uid-based) is NOT expected to agree here —
        // if it did, this whole hash wouldn't be necessary.
        assert_ne!(
            search_hash(&a),
            search_hash(&b),
            "sanity: the uid-based search_hash DOES distinguish these two \
             (that's the ambiguity chance_outcome_hash exists to collapse)"
        );
    }

    #[test]
    fn chance_outcome_hash_distinguishes_a_genuinely_different_remaining_card() {
        use crate::cards::CardId;
        let mut a = draw_state(&[
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::Bash, false),
        ]);
        // Drop a Strike, leaving {Strike, Bash}.
        a.player.draw.retain(|u| *u != CardUid::new(1).unwrap());
        let mut b = a.clone();
        // Now genuinely change the remaining Strike's content (upgrade
        // it) — a real observable difference, must NOT dedup.
        if let Some(c) = b
            .cards
            .iter_mut()
            .find(|c| c.uid == CardUid::new(2).unwrap())
        {
            c.upgraded = true;
        }
        assert_ne!(
            chance_outcome_hash(&a),
            chance_outcome_hash(&b),
            "a genuinely different remaining card must still change the hash"
        );
    }

    #[test]
    fn chance_outcome_hash_reacts_to_game_state() {
        let mut a = CombatState::default();
        let base = chance_outcome_hash(&a);
        a.round = 7;
        assert_ne!(
            chance_outcome_hash(&a),
            base,
            "chance_outcome_hash must still react to non-draw game state"
        );
    }

    #[test]
    fn chance_outcome_hash_pins_stay_uid_sensitive() {
        use crate::cards::CardId;
        // Two states with the SAME multiset but a DIFFERENT card pinned to
        // the top must NOT dedup — pins are player-KNOWN, not part of the
        // ambiguity this hash resolves.
        let mut a = draw_state(&[
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::Bash, false),
        ]);
        let mut b = a.clone();
        crate::cards::pin_top(&mut a.player, CardUid::new(1).unwrap());
        crate::cards::pin_top(&mut b.player, CardUid::new(3).unwrap());
        assert_ne!(
            chance_outcome_hash(&a),
            chance_outcome_hash(&b),
            "different pinned cards must change the hash even if the \
             draw-pile multiset is identical"
        );
    }

    #[test]
    fn chance_outcome_hash_ignores_which_hand_instance_is_where() {
        use crate::cards::CardId;
        // The scenario that actually matters for DrawChance: two outcomes
        // that each drew ONE of several observationally-identical Strikes
        // into HAND, leaving different specific uids behind in the pile.
        // If HAND stayed uid-based this would NOT dedup even though DRAW's
        // remaining segment does (see the sibling test) — the drawn card's
        // uid ends up in hand, not in draw.
        let mut a = draw_state(&[
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::Bash, false),
        ]);
        let mut b = a.clone();
        // `a`: "drew" uid 1 into hand, leaving {uid 2 Strike, uid 3 Bash}.
        a.player.draw.retain(|u| *u != CardUid::new(1).unwrap());
        a.player.hand.push(CardUid::new(1).unwrap());
        // `b`: "drew" uid 2 instead, leaving {uid 1 Strike, uid 3 Bash} —
        // same OBSERVABLE hand (one un-upgraded Strike) and same
        // observable remaining pile, different physical uids throughout.
        b.player.draw.retain(|u| *u != CardUid::new(2).unwrap());
        b.player.hand.push(CardUid::new(2).unwrap());

        assert_eq!(
            chance_outcome_hash(&a),
            chance_outcome_hash(&b),
            "drawing observationally-identical duplicate Strikes into hand \
             must dedup regardless of which physical uid was drawn"
        );
        assert_ne!(
            search_hash(&a),
            search_hash(&b),
            "sanity: search_hash DOES distinguish these (uid-based) — \
             that's the ambiguity chance_outcome_hash exists to collapse"
        );
    }

    /// `hash_pile_observational` finds a card through its canonical
    /// `cards[uid - 1]` slot, falling back to a scan. On a dense arena and on
    /// a sparse fixture arena (skipped uids) it must fold exactly what the
    /// scan-only lookup folded, including dropping a uid with no card.
    #[test]
    fn pile_observational_lookup_matches_scan_on_sparse_arena() {
        use crate::cards::CardId;
        let scan_fold = |pile: &[CardUid], s: &CombatState| {
            let mut keys: Vec<u64> = pile
                .iter()
                .filter_map(|uid| s.cards.iter().find(|c| c.uid == *uid))
                .map(card_observational_key)
                .collect();
            keys.sort_unstable();
            let mut h = new_hasher();
            (keys.len() as u64).hash(&mut h);
            for k in &keys {
                k.hash(&mut h);
            }
            h.finish()
        };
        let fold = |pile: &[CardUid], s: &CombatState| {
            let mut h = new_hasher();
            hash_pile_observational(pile, s, &mut h);
            h.finish()
        };
        let uids = |ns: &[u32]| -> Vec<CardUid> {
            ns.iter()
                .map(|&n| CardUid::new(n).expect("non-zero"))
                .collect()
        };
        let dense = draw_state(&[
            (CardId::StrikeIronclad, false),
            (CardId::Bash, true),
            (CardId::DefendIronclad, false),
        ]);
        // Same cards at uids 2/5/9: every lookup misses its canonical slot
        // (`cards[1]` holds uid 5, `cards[4]` and `cards[8]` do not exist).
        let mut sparse = dense.clone();
        for (c, uid) in sparse.cards.iter_mut().zip([2u32, 5, 9]) {
            c.uid = CardUid::new(uid).expect("non-zero");
        }
        for (s, pile) in [
            (&dense, uids(&[3, 1, 2])),
            (&dense, uids(&[2, 7])),
            (&sparse, uids(&[9, 2, 5])),
            (&sparse, uids(&[1, 3, 5])),
        ] {
            assert_eq!(fold(&pile, s), scan_fold(&pile, s), "pile {pile:?}");
        }
        // The fallback reads the card it found: uid 5's content shows.
        let mut changed = sparse.clone();
        changed.cards[1].upgraded = false;
        assert_ne!(fold(&uids(&[5]), &sparse), fold(&uids(&[5]), &changed));
    }

    /// GH #741: `play_card` now stamps a non-X card's paid cost into
    /// `energy_spent`; the observational key masks it so a played Strike keys
    /// the same as an unplayed one (as it did before #741). An X-cost card's
    /// stamped X still splits the key.
    #[test]
    fn observational_key_masks_non_x_energy_spent_only() {
        let base = crate::Card {
            uid: crate::CardUid::new(1).expect("nz"),
            cost: 2,
            ..crate::Card::pod_defaults()
        };
        let mut played = base;
        played.energy_spent = 2;
        assert_eq!(
            card_observational_key(&base),
            card_observational_key(&played)
        );

        let x = crate::Card { cost: -1, ..base };
        let mut x_played = x;
        x_played.energy_spent = 3;
        assert!(x.is_x_cost());
        assert_ne!(
            card_observational_key(&x),
            card_observational_key(&x_played)
        );
    }

    /// `hash_entries_by_key` hashes an in-order map straight from its slice
    /// and sorts only an out-of-order one. Both paths must write the bytes
    /// the always-sort fold wrote (`Vec` of references +
    /// `sort_unstable_by_key`), including for a key that appears twice.
    #[test]
    fn entries_by_key_matches_always_sort_fold() {
        use crate::cards::VarKey;
        let sort_fold = |entries: &[(VarKey, i32)]| {
            let mut sorted: Vec<_> = entries.iter().collect();
            sorted.sort_unstable_by_key(|(k, _)| *k);
            let mut h = new_hasher();
            for (k, v) in sorted {
                k.hash(&mut h);
                v.hash(&mut h);
            }
            h.finish()
        };
        let fold = |entries: &[(VarKey, i32)]| {
            let mut h = new_hasher();
            hash_entries_by_key(entries, &mut h);
            h.finish()
        };
        let mut keys = [
            VarKey::FreeThisTurn,
            VarKey::CostAbsolutelySet,
            VarKey::RampageDamage,
        ];
        keys.sort_unstable();
        let [a, b, c] = keys;
        for entries in [
            vec![],
            vec![(b, 4)],
            vec![(a, 1), (b, 2), (c, 3)],
            vec![(a, 1), (a, 7), (c, 3)],
            vec![(c, 3), (a, 1), (b, 2)],
            vec![(b, 2), (a, 7), (b, 9), (a, 1)],
        ] {
            assert_eq!(fold(&entries), sort_fold(&entries), "{entries:?}");
        }
        // Key order is what is hashed, not insertion order.
        assert_eq!(fold(&[(a, 1), (b, 2)]), fold(&[(b, 2), (a, 1)]));
    }

    // ── sts2-rs-jyd15: the orb belt must contribute to the hash ────────
    // Regression guards: before this bead, `state_hash` walked player
    // piles/powers but NOT `player.orbs`, so two states differing only in
    // the Defect orb belt collided — an MCTS transposition table would
    // alias semantically distinct nodes. (The bead's stance half went away
    // with the field itself in GH #408.)

    #[test]
    fn changes_with_orb_slot_contents() {
        use crate::Orb;
        let mut a = CombatState::default();
        let empty_hash = state_hash(&a);
        a.player.orbs.capacity = 3;
        a.player.orbs.slots = SmallVec::from_slice(&[Orb::lightning()]);
        let one_orb = state_hash(&a);
        assert_ne!(
            empty_hash, one_orb,
            "channeling an orb must perturb the hash"
        );

        // A different orb kind in the same slot must hash differently.
        let mut b = a.clone();
        b.player.orbs.slots = SmallVec::from_slice(&[Orb::frost()]);
        assert_ne!(one_orb, state_hash(&b), "orb kind contributes to the hash");
    }

    #[test]
    fn changes_with_orb_slot_order() {
        use crate::Orb;
        // Orb belt is FIFO (front = oldest); slot order is observable
        // (evoke/passive target the front), so the hash must NOT be
        // order-invariant the way card piles are.
        let mut a = CombatState::default();
        let mut b = CombatState::default();
        a.player.orbs.capacity = 3;
        b.player.orbs.capacity = 3;
        a.player.orbs.slots = SmallVec::from_slice(&[Orb::lightning(), Orb::frost()]);
        b.player.orbs.slots = SmallVec::from_slice(&[Orb::frost(), Orb::lightning()]);
        assert_ne!(
            state_hash(&a),
            state_hash(&b),
            "orb channel order is significant — must change the hash"
        );
    }

    #[test]
    fn changes_with_orb_capacity() {
        let mut a = CombatState::default();
        let base = state_hash(&a);
        a.player.orbs.capacity = 3;
        assert_ne!(
            state_hash(&a),
            base,
            "orb capacity (Capacitor/Signal Boost) must perturb the hash"
        );
    }

    #[test]
    fn pile_order_invariant() {
        // Two states with the same hand-multiset but different insertion
        // order must hash equal — MCTS transposition tables depend on this.
        let mut a = CombatState::default();
        let mut b = CombatState::default();
        let u1 = CardUid::new(1).unwrap();
        let u2 = CardUid::new(2).unwrap();
        let u3 = CardUid::new(3).unwrap();
        a.player.hand = SmallVec::from_slice(&[u1, u2, u3]);
        b.player.hand = SmallVec::from_slice(&[u3, u1, u2]);
        assert_eq!(state_hash(&a), state_hash(&b));
    }

    #[test]
    fn card_affliction_and_bound_flag_change_hash() {
        // Two states identical but for one card's `Bound` affliction (the
        // Queen boss's ChainsOfBinding "freeze") MUST hash differently, or
        // the MCTS transposition table aliases frozen and un-frozen hands
        // and shares node stats across genuinely different positions.
        use crate::card_afflictions::{CardAffliction, CardAfflictionKind};
        use crate::cards::{Card, CardId, CardType, TargetType};

        let mk = |afflict: Option<CardAffliction>| {
            let mut s = CombatState::default();
            let mut c = Card {
                uid: CardUid::new(1).unwrap(),
                card_id: CardId::StrikeIronclad,
                cost: 1,
                card_type: CardType::Attack,
                target_type: TargetType::Enemy,
                base_damage: 6,
                base_block: 0,
                ..Card::pod_defaults()
            };
            c.affliction = afflict;
            s.cards.push(c);
            s.player.hand = SmallVec::from_slice(&[CardUid::new(1).unwrap()]);
            s
        };
        let clean = mk(None);
        let bound = mk(Some(CardAffliction {
            kind: CardAfflictionKind::Bound,
            stacks: 1,
            applied_exhaust: false,
        }));
        assert_ne!(
            state_hash(&clean),
            state_hash(&bound),
            "Bound affliction on a card must perturb the hash"
        );

        // The per-turn one-Bound-play lockout flag must also be hashed, so
        // pre- and post-Bound-play positions do not collide.
        let mut a = CombatState::default();
        let base = state_hash(&a);
        a.combat_log.chains_bound_card_played_this_turn = true;
        assert_ne!(
            state_hash(&a),
            base,
            "chains_bound_card_played_this_turn must perturb the hash"
        );
    }

    #[test]
    fn osty_pet_state_changes_hash() {
        // The Necrobinder's Osty pet lives in `state.creatures` but is in
        // NEITHER `enemy_uids` NOR `allies`, so a hash that only walks
        // those two groups ignores Osty entirely — the MCTS transposition
        // table then aliases "built-up Osty alive" with "Osty dead"
        // positions (same class as the ChainsOfBinding freeze bug above).
        // Osty MaxHp compounds across summons and resets on death, and
        // Unleash/Protector/Sacrifice/DieForYou all key off its hp/max_hp,
        // so every observable Osty field must perturb all three hashes.
        let no_pet = CombatState::default();
        let mut with_pet = CombatState::default();
        with_pet.summon_osty(10);
        assert_ne!(
            state_hash(&no_pet),
            state_hash(&with_pet),
            "Osty presence must perturb state_hash"
        );
        assert_ne!(
            search_hash(&no_pet),
            search_hash(&with_pet),
            "Osty presence must perturb search_hash"
        );
        assert_ne!(
            chance_outcome_hash(&no_pet),
            chance_outcome_hash(&with_pet),
            "Osty presence must perturb chance_outcome_hash"
        );

        let pet_uid = with_pet.player.pet_uid.expect("summon sets pet_uid");
        let mutate = |state: &CombatState, f: &dyn Fn(&mut crate::Creature)| {
            let mut s = state.clone();
            let c = s
                .creatures
                .iter_mut()
                .find(|c| c.id == pet_uid.0)
                .expect("pet creature in arena");
            f(c);
            s
        };

        let damaged = mutate(&with_pet, &|c| c.current_hp = 4);
        assert_ne!(
            state_hash(&with_pet),
            state_hash(&damaged),
            "Osty current_hp must perturb the hash (Unleash damage / soak capacity)"
        );

        let grown = mutate(&with_pet, &|c| c.max_hp = 25);
        assert_ne!(
            state_hash(&with_pet),
            state_hash(&grown),
            "Osty max_hp must perturb the hash (Sacrifice block / Protector damage)"
        );

        let blocked = mutate(&with_pet, &|c| c.block = 6);
        assert_ne!(
            state_hash(&with_pet),
            state_hash(&blocked),
            "Osty block must perturb the hash"
        );

        // Dead-but-revivable: `pet_uid` stays `Some` while current_hp == 0.
        let dead = mutate(&with_pet, &|c| c.current_hp = 0);
        assert_ne!(
            state_hash(&with_pet),
            state_hash(&dead),
            "Osty alive vs dead-but-revivable must perturb the hash"
        );
    }

    #[test]
    fn necrobinder_turn_flags_change_hash() {
        // Per-turn Necrobinder combat-log flags gate card effects
        // (DeathsDoor's 3x block reads `doom_applied_by_player_this_turn`;
        // Rattle's hit count and Flatten's free-cost read
        // `osty_attacks_this_turn`), so pre- and post-trigger positions
        // must not collide — exactly like `chains_bound_card_played_this_
        // turn` above.
        let base = state_hash(&CombatState::default());

        let mut doomed = CombatState::default();
        doomed.combat_log.doom_applied_by_player_this_turn = true;
        assert_ne!(
            state_hash(&doomed),
            base,
            "doom_applied_by_player_this_turn must perturb the hash"
        );

        let mut attacked = CombatState::default();
        attacked.combat_log.osty_attacks_this_turn = 1;
        assert_ne!(
            state_hash(&attacked),
            base,
            "osty_attacks_this_turn must perturb the hash"
        );
    }

    // ── sts2-rs-42j: PendingChoice payload hashing ─────────────────
    //
    // The hash MUST be:
    //   1. deterministic (same payload → same hash, run-to-run for the
    //      same compiled binary).
    //   2. total over every public field (mutating any field must
    //      perturb the hash — otherwise MCTS transposition tables can
    //      conflate semantically distinct choices).
    //   3. sensitive to option ORDER, because Python's
    //      `selected_indices` indexes positionally into `options`.

    use crate::{cards::CardId, cards::PileKind, ChoiceKind, ChoiceOption, PendingChoice};

    fn opt(card_id: CardId, upgraded: bool) -> ChoiceOption {
        ChoiceOption {
            card_id,
            upgraded,
            card_uid: None,
            source_pile: None,
        }
    }

    #[test]
    fn pending_choice_some_vs_none_differs() {
        let mut a = CombatState::default();
        let mut b = CombatState::default();
        a.pending_choice = None;
        b.pending_choice = Some(PendingChoice::default());
        assert_ne!(state_hash(&a), state_hash(&b));
    }

    #[test]
    fn pending_choice_deterministic_clone() {
        let pc = PendingChoice {
            kind: ChoiceKind::CardSelect,
            options: vec![opt(CardId::StrikeIronclad, false), opt(CardId::Bash, true)],
            min_select: 1,
            max_select: 2,
            can_skip: true,
            post_effect: None,
            picked: smallvec::SmallVec::new(),
        };
        let mut a = CombatState::default();
        let mut b = CombatState::default();
        a.pending_choice = Some(pc.clone());
        b.pending_choice = Some(pc);
        assert_eq!(state_hash(&a), state_hash(&b));
    }

    #[test]
    fn pending_choice_kind_matters() {
        let mk = |kind| PendingChoice {
            kind,
            ..PendingChoice::default()
        };
        let mut a = CombatState::default();
        let mut b = CombatState::default();
        a.pending_choice = Some(mk(ChoiceKind::CardSelect));
        b.pending_choice = Some(mk(ChoiceKind::ExhaustPile));
        assert_ne!(state_hash(&a), state_hash(&b));
    }

    #[test]
    fn pending_choice_minmax_skip_matter() {
        let base = PendingChoice::default();
        let a = CombatState {
            pending_choice: Some(base.clone()),
            ..Default::default()
        };
        let ha = state_hash(&a);

        for mutate in [
            |pc: &mut PendingChoice| pc.min_select = 99,
            |pc: &mut PendingChoice| pc.max_select = 99,
            |pc: &mut PendingChoice| pc.can_skip = !pc.can_skip,
        ] {
            let mut bp = base.clone();
            mutate(&mut bp);
            let b = CombatState {
                pending_choice: Some(bp),
                ..Default::default()
            };
            assert_ne!(state_hash(&b), ha);
        }
    }

    #[test]
    fn pending_choice_option_order_matters() {
        let mut a = CombatState::default();
        let mut b = CombatState::default();
        let o1 = opt(CardId::StrikeIronclad, false);
        let o2 = opt(CardId::Bash, false);
        a.pending_choice = Some(PendingChoice {
            options: vec![o1, o2],
            ..Default::default()
        });
        b.pending_choice = Some(PendingChoice {
            options: vec![o2, o1],
            ..Default::default()
        });
        assert_ne!(state_hash(&a), state_hash(&b));
    }

    #[test]
    fn pending_choice_option_fields_matter() {
        let base_opt = opt(CardId::StrikeIronclad, false);
        let make = |o: ChoiceOption| {
            let s = CombatState {
                pending_choice: Some(PendingChoice {
                    options: vec![o],
                    ..Default::default()
                }),
                ..Default::default()
            };
            state_hash(&s)
        };
        let baseline = make(base_opt);

        // Flip upgraded.
        let mut o = base_opt;
        o.upgraded = true;
        assert_ne!(make(o), baseline);

        // Change card_id.
        let mut o = base_opt;
        o.card_id = CardId::Bash;
        assert_ne!(make(o), baseline);

        // Add card_uid.
        let mut o = base_opt;
        o.card_uid = Some(CardUid::new(7).unwrap());
        assert_ne!(make(o), baseline);

        // Add source_pile.
        let mut o = base_opt;
        o.source_pile = Some(PileKind::Hand);
        assert_ne!(make(o), baseline);

        // Different pile.
        let mut o2 = base_opt;
        o2.source_pile = Some(PileKind::Discard);
        let mut o3 = base_opt;
        o3.source_pile = Some(PileKind::Exhaust);
        assert_ne!(make(o2), make(o3));
    }

    #[test]
    fn pile_identity_matters() {
        // Same multiset across all piles, but a card moved Hand→Discard
        // must produce a different hash.
        let mut a = CombatState::default();
        let mut b = CombatState::default();
        let u1 = CardUid::new(1).unwrap();
        a.player.hand = SmallVec::from_slice(&[u1]);
        b.player.discard = SmallVec::from_slice(&[u1]);
        assert_ne!(state_hash(&a), state_hash(&b));
    }

    // ── run_hash (sts2-rs full-run fuzzer B2) ──────────────────────────

    use crate::run_manager::RunManager;
    use crate::CharacterId;

    fn fresh_run() -> RunManager {
        RunManager::new(42, CharacterId::Ironclad)
    }

    #[test]
    fn run_hash_deterministic_same_seed() {
        // Two runs built from the same seed + character hash equal.
        let a = fresh_run();
        let b = fresh_run();
        assert_eq!(run_hash(&a), run_hash(&b));
    }

    #[test]
    fn run_hash_changes_with_gold() {
        let mut a = fresh_run();
        let base = run_hash(&a);
        a.run_state.gold += 25;
        assert_ne!(run_hash(&a), base, "gold must perturb the run hash");
    }

    #[test]
    fn run_hash_changes_with_hp() {
        let mut a = fresh_run();
        let base = run_hash(&a);
        a.run_state.current_hp -= 7;
        assert_ne!(run_hash(&a), base, "HP must perturb the run hash");
    }

    #[test]
    fn run_hash_changes_with_deck() {
        let mut a = fresh_run();
        let base = run_hash(&a);
        // Flip the upgrade bit on the first deck card (if any), else push.
        if let Some(c) = a.run_state.master_deck.first_mut() {
            c.upgraded = !c.upgraded;
        } else {
            // Empty deck shouldn't happen for a starter, but guard anyway.
            a.run_state.gold += 1;
        }
        assert_ne!(run_hash(&a), base, "deck mutation must perturb the hash");
    }

    #[test]
    fn run_hash_changes_when_rng_advances() {
        let mut a = fresh_run();
        let base = run_hash(&a);
        // Advance one run-level RNG stream's cursor.
        let _ = a.run_state.rng.shuffle.next_int_exclusive(0, 100);
        assert_ne!(
            run_hash(&a),
            base,
            "advancing a run RNG stream must perturb the hash"
        );
    }

    // ── run_search_hash: RNG-cursor-INsensitive run-state hash (AZ M3) ──
    // Outer-loop MCTS transposition/dedup needs a run-level analog of
    // `search_hash`: identical to `run_hash` but ignoring RNG cursor
    // position, so two runs that reached the same observable state via
    // different chance-node outcome seeds merge in the transposition table.

    #[test]
    fn run_search_hash_ignores_rng_cursors_but_tracks_deck() {
        // Build two identical fresh runs, then advance ONLY the rng cursors
        // of one.
        let a = fresh_run();
        let mut b = a.clone();
        // Perturb b's run-draw RNG cursor without changing observable state.
        let _ = b.run_state.rng.rewards.next_int_exclusive(0, 100);
        assert_eq!(
            run_search_hash(&a),
            run_search_hash(&b),
            "search hash must ignore RNG cursor position"
        );
        assert_ne!(
            run_hash(&a),
            run_hash(&b),
            "sanity: cursor-sensitive run_hash DOES differ"
        );
        // A real observable change (gold) must change the search hash.
        b.run_state.gold += 5;
        assert_ne!(
            run_search_hash(&a),
            run_search_hash(&b),
            "search hash must track observable run state"
        );
    }

    #[test]
    fn run_search_hash_deterministic_same_seed() {
        let a = fresh_run();
        let b = fresh_run();
        assert_eq!(run_search_hash(&a), run_search_hash(&b));
    }

    // ── Pinned legacy SipHash values (PR #199 dedup refactor guard) ────
    //
    // `run_hash` / `run_search_hash` are contractually FROZEN at their
    // pre-2026-07-04 std-`DefaultHasher` (SipHash-1-3) values: the
    // sts2-runai outer-loop planner derives chance-node determinization
    // bases from them, and its loop-closure fixtures (trained prior nets +
    // pinned seeds) only discriminate under the exact historical values.
    //
    // This test pins those values as hardcoded constants over a sample
    // state that exercises as much of the walk as practical (combat with
    // an enemy + powers on both sides, cards with combat_vars /
    // enchantments, unsorted hand pile, potions, relics, pending choices
    // at both run and combat level, advanced RNG cursors, orbs).
    // It doubles as the drift guard for the shared-field-walk refactor:
    // any byte-stream change in the legacy instantiation shows up here as
    // a hard failure, independent of the fast-path (ahash) values.
    fn pinned_legacy_sample() -> RunManager {
        use crate::cards::{EnchantmentId, VarKey};
        use crate::monsters::MonsterKind;
        use crate::potions::PotionKind;
        use crate::relics::{RelicId, RelicKind};
        use crate::{Card, Orb, Potion, Power, PowerKind, PowerUid, Relic};
        use std::num::NonZeroU32;

        let mut rm = fresh_run();

        // Run-level enrichment: scalars, belt, relic counter, map cursor,
        // pending choice, advanced RNG cursor.
        rm.run_state.gold = 137;
        rm.run_state.current_hp -= 9;
        let _ = rm.run_state.add_potion_to_belt(PotionKind::BlockPotion);
        rm.run_state.relic_counters.insert(RelicId::BurningBlood, 3);
        let _ = rm.run_state.rng.rewards.next_int_exclusive(0, 100);
        rm.run_state.pending_choice = Some(PendingChoice {
            kind: ChoiceKind::CardSelect,
            options: vec![opt(CardId::Bash, true)],
            min_select: 1,
            max_select: 1,
            can_skip: true,
            post_effect: None,
            picked: smallvec::SmallVec::new(),
        });

        // Live combat sub-state.
        let mut cs = CombatState {
            phase: crate::CombatPhase::PlayerPlayPhase,
            round: 3,
            ascension: 2,
            ..CombatState::default()
        };

        // Enemy with a power.
        let euid = cs.spawn_enemy(MonsterKind::Nibbit);
        let p1 = NonZeroU32::new(1).unwrap();
        cs.powers.push(Power {
            counter: 1,
            applier: Some(euid),
            ..Power::new(p1, PowerKind::Vulnerable, 2)
        });
        if let Some(c) = cs.creatures.iter_mut().find(|c| c.id == euid.0) {
            c.powers.push(PowerUid(p1));
            c.block = 4;
        }

        // Player scalars, power, orb belt.
        let p2 = NonZeroU32::new(2).unwrap();
        cs.powers.push(Power::new(p2, PowerKind::Strength, 3));
        cs.player.powers.push(PowerUid(p2));
        cs.player.energy = 2;
        cs.player.block = 5;
        cs.player.orbs.capacity = 3;
        cs.player.orbs.slots = SmallVec::from_slice(&[Orb::lightning(), Orb::frost()]);

        // Cards arena (ascending uids) with combat_vars + enchantments;
        // hand deliberately NOT uid-sorted to exercise the legacy pile sort.
        let u1 = CardUid::new(1).unwrap();
        let u2 = CardUid::new(2).unwrap();
        cs.cards.push(Card {
            uid: u1,
            card_id: CardId::Bash,
            cost: 2,
            base_damage: 8,
            combat_vars: [(VarKey::RampageDamage, 5)].into_iter().collect(),
            enchantments: [(EnchantmentId::Steady, 1)].into_iter().collect(),
            ..Card::pod_defaults()
        });
        cs.cards.push(Card {
            uid: u2,
            base_damage: 6,
            ..Card::pod_defaults()
        });
        cs.player.hand = SmallVec::from_slice(&[u2, u1]);

        // Combat-side potion + relic (relic kind exercises the legacy
        // Debug-string encoding).
        cs.potions.push(Potion {
            id: NonZeroU32::new(1).unwrap(),
        });
        cs.relics.push(Relic {
            id: NonZeroU32::new(1).unwrap(),
            kind: RelicKind::BurningBlood,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });

        // Combat-side pending choice with option payload + pick progress.
        cs.pending_choice = Some(PendingChoice {
            kind: ChoiceKind::CardSelect,
            options: vec![
                ChoiceOption {
                    card_id: CardId::StrikeIronclad,
                    upgraded: false,
                    card_uid: Some(u2),
                    source_pile: Some(PileKind::Hand),
                },
                opt(CardId::Bash, true),
            ],
            min_select: 1,
            max_select: 2,
            can_skip: false,
            post_effect: None,
            picked: smallvec::smallvec![0u8],
        });

        // Advanced combat RNG cursors (state_hash-only sections).
        cs.rng = crate::dotnet_rng::Rng::with_counter(7, 3);
        let _ = cs.run_rng.shuffle.next_int(0, 100);
        let _ = cs.run_rng.monster_ai.next_int(0, 100);

        rm.combat = Some(cs);
        rm
    }

    #[test]
    fn legacy_run_hash_values_pinned() {
        let rm = pinned_legacy_sample();
        // Values re-pinned 2026-08-09 (GH #408): `player.stance` left the
        // observable field set entirely — the dead STS1 stance machine and
        // its `PlayerCombatState` field were deleted, so `hash_player_tail`
        // no longer folds it in. Purely a byte-stream shift; nothing
        // persisted is keyed on these values (the MCTS TT is in-memory and
        // the parity-cache bundles are keyed on the C# oracle's content
        // hash), so no rebake follows.
        // Prior pin 2026-07-20: the game-observable field set gained
        // the Necrobinder Osty pet section (presence + hp/max_hp/block/
        // powers in `hash_player_tail`) and the `doom_applied_by_player_
        // this_turn` / `osty_attacks_this_turn` scalars (fixing TT aliasing
        // of Osty-alive-vs-dead and pre/post-Doom-trigger states, same
        // class as the 2026-07-12 Bound/"freeze" re-pin below). This
        // deliberately shifts the frozen SipHash; it is a determinism
        // guard, not an external contract.
        // Prior pin 2026-07-12: per-card `affliction` +
        // `chains_bound_card_played_this_turn` (Queen-fight freeze
        // aliasing).
        assert_eq!(
            run_hash(&rm),
            15_769_890_208_358_649_762_u64,
            "run_hash must keep its frozen SipHash value (re-pinned 2026-08-09)"
        );
        assert_eq!(
            run_search_hash(&rm),
            17_752_695_356_920_422_117_u64,
            "run_search_hash must keep its frozen SipHash value (re-pinned 2026-08-09)"
        );
    }

    #[test]
    fn run_search_hash_tracks_live_combat_via_search_hash() {
        // When a combat is live, `run_search_hash` must be insensitive to
        // the combat sub-state's RNG cursors too (routed through
        // `search_hash`, not `state_hash`).
        let mut a = fresh_run();
        a.combat = Some(CombatState::default());
        let mut b = a.clone();
        if let Some(c) = b.combat.as_mut() {
            let _ = c.run_rng.shuffle.next_int(0, 100);
        }
        assert_eq!(
            run_search_hash(&a),
            run_search_hash(&b),
            "run_search_hash must ignore combat sub-state RNG cursor"
        );

        // But a game-observable combat change must still perturb it.
        let mut c = a.clone();
        if let Some(cs) = c.combat.as_mut() {
            cs.round = 7;
        }
        assert_ne!(
            run_search_hash(&a),
            run_search_hash(&c),
            "run_search_hash must track observable combat state"
        );
    }
}
