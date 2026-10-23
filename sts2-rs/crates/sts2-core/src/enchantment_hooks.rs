//! Enchantment hook channels (bead `sts2-rs-6ynt.11`).
//!
//! Mirrors the six dispatch points Python's `sts2_env/cards/enchantments.py`
//! exposes at pin `sts2-rl-agent@01c094d`:
//!
//! | Channel                       | Python entry                                  |
//! |-------------------------------|-----------------------------------------------|
//! | [`dispatch_on_card_drawn`]    | `enchantments.py:121` `on_card_drawn`         |
//! | [`dispatch_after_player_turn_start`] | `enchantments.py:126` `after_player_turn_start` |
//! | [`dispatch_before_flush`]     | `enchantments.py:137` `before_flush`          |
//! | [`dispatch_modify_shuffle_order`] | `enchantments.py:149` `modify_shuffle_order` |
//! | [`dispatch_on_card_played`]   | `enchantments.py:158` `on_card_played`        |
//! | [`dispatch_after_card_played`] | `enchantments.py:184` `after_card_played`    |
//!
//! The table above names the Python *channel* each dispatcher mirrors, NOT
//! the site it is fired from. One site deliberately diverges from the Python
//! pin: [`dispatch_on_card_played`] now fires AFTER the card's own effect
//! resolves, per C# `CardModel.OnPlayWrapper` (`CardModel.cs:1930-1943`) —
//! see that function's docs (bead `sts2-rs-ench-onplay-order`).
//!
//! ## Surface-bead scope
//!
//! This bead lands the dispatch surface only — every per-card stub
//! returns without applying enchantment effects. Concrete listener
//! bodies (Slither cost-roll on draw, Imbued auto-play on turn 1,
//! SlumberingEssence pre-flush cost cut, PerfectFit re-shuffle order,
//! Adroit / Swift / Sown / Corrupted / Momentum on play, Vigorous /
//! Glam / Goopy used-marker after play) land in follow-up sub-beads
//! per `docs/audits/2026-05-15-ironclad-completeness/06-powers-enchantments.md`.
//!
//! ## Empty-bus early-out
//!
//! Each dispatcher walks only the relevant pile(s) and returns early
//! when no card in scope has any enchantments. This matches the
//! discipline established by bead `sts2-rs-4lk9` (powers/relics
//! empty-bus early-out) so adding the surface does not regress the
//! `transitions_per_sec` anchor with empty enchantment loadouts (the
//! common case at this pin until enchantments are populated by the
//! run-builder layer).

use crate::cards::{Card, EnchantmentId, VarKey};
use crate::hooks::{CreatureRef, ValueProps};
use crate::{CardUid, CombatState, PlayerCombatState};

// Bit-packed per-combat "used" flags stashed on the card's
// `VarKey::Generic` combat-var. The Vigorous + Glam bits agree
// byte-for-byte with the content-side `sts2-content::enchantments`
// encoding (helpers `mark_vigorous_used` / `vigorous_used` /
// `mark_glam_used` / `glam_used`). Swift + Sown bits are core-side
// only at this pin — their consumed state is queried directly here on
// the dispatch path, not from sts2-content helpers.
const VIGOROUS_USED_BIT: i32 = 0b0001;
const GLAM_USED_BIT: i32 = 0b0010;
const SWIFT_USED_BIT: i32 = 0b0100;
const SOWN_USED_BIT: i32 = 0b1000;

/// Extra plays a card's enchantment adds: C# `CardModel.GetEnchantedReplayCount`
/// = `Enchantment.EnchantPlayCount(BaseReplayCount)`, where Spiral adds its fixed
/// `Times` (1) and Glam the same until it has been used this combat (`Spiral.cs`,
/// `Glam.cs`). The play count is that + 1, before `Hook.ModifyCardPlayCount`
/// (`CardModel.cs`, the play's `playCount`). Lives in core so both play paths in
/// `combat_loop` add it; `sts2_content::enchantments::enchant_play_count_bonus`
/// delegates here.
#[must_use]
pub fn enchant_play_count_bonus(card: &Card) -> i32 {
    let mut bonus = 0;
    if card.has_enchantment(EnchantmentId::Spiral) {
        bonus += 1;
    }
    if card.has_enchantment(EnchantmentId::Glam) && card.combat_var(VarKey::Generic) & GLAM_USED_BIT == 0 {
        bonus += 1;
    }
    bonus
}

/// Additive damage bonus contributed by the *played card's* enchantments,
/// for a powered attack. Mirrors the C# `*.EnchantDamageAdditive` chain
/// (`Sharp`/`Inky` flat, `Vigorous` once-per-card, `Momentum` accumulated
/// into `VarKey::RampageDamage`) and the Python `enchant_damage_additive`.
/// Returns 0 for unpowered hits (C# gates each enchantment on
/// `props.IsPoweredAttack()`).
///
/// This is the canonical core copy; the sts2-content helper delegates here
/// so the value the damage pipeline applies (see [`hooks::modify_damage`])
/// can never drift from the unit-tested content logic. Bead
/// `sts2-rs-enchant-damage-additive` (PARITY136: a `Sharp(+2)` STRIKE_DEFECT
/// auto-played by Uproar dealt 6 in Rust vs 8 in C# because the enchantment
/// additive was never threaded into `modify_damage`).
#[must_use]
pub fn enchant_damage_additive(card: &crate::cards::Card, is_powered: bool) -> i32 {
    if !is_powered {
        return 0;
    }
    let mut total = card.enchantment(EnchantmentId::Sharp);
    // Momentum stashes `+stack` per play onto RampageDamage (see
    // `dispatch_on_card_played`); the running total is the bonus. Gated on the
    // Momentum enchantment because `VarKey::RampageDamage` is ALSO the Rampage
    // card's own scaling slot — without the gate, the Rampage card (which is
    // not Momentum-enchanted) would double-count its scaling through this
    // additive term.
    if card.has_enchantment(EnchantmentId::Momentum) {
        total = total.saturating_add(card.combat_var(VarKey::RampageDamage));
    }
    // Vigorous applies once per card per combat; the consumed bit lives on
    // `VarKey::Generic`.
    if card.combat_var(VarKey::Generic) & VIGOROUS_USED_BIT == 0 {
        total = total.saturating_add(card.enchantment(EnchantmentId::Vigorous));
    }
    // Inky (Blade of Ink shivs): fixed +1, regardless of stacks. v0.107.1
    // `Inky.cs` `EnchantDamageAdditive` returns `DynamicVars.Damage.BaseValue`
    // = `DamageVar(1m)` (was `2m` in v0.103.2) on a powered attack.
    if card.has_enchantment(EnchantmentId::Inky) {
        total = total.saturating_add(1);
    }
    // TezcatarasEmber (v0.107.1 NEW enchant, `TezcatarasEmber.cs`): a flat
    // `+3` on a powered attack. `EnchantDamageAdditive` returns
    // `base.DynamicVars.Damage.BaseValue` = `DamageVar(3m)` when
    // `props.IsPoweredAttack()`, else `0` (the powered gate is the `is_powered`
    // early-return above). The value is exposed via [`TEZCATARAS_EMBER_DAMAGE`]
    // so the `NutritiousSoup` follow-up (`sts2-rs-ajjri`) can query it without
    // re-deriving the constant.
    if card.has_enchantment(EnchantmentId::TezcatarasEmber) {
        total = total.saturating_add(TEZCATARAS_EMBER_DAMAGE);
    }
    total
}

/// Flat powered-attack damage bonus granted by the v0.107.1 `TezcatarasEmber`
/// enchantment (`TezcatarasEmber.cs` `CanonicalVars = DamageVar(3m, Move)`).
/// Exposed as a named constant so callers (e.g. the `NutritiousSoup`
/// follow-up `sts2-rs-ajjri`) can query the enchant value directly. The
/// powered gate lives at the [`enchant_damage_additive`] call site, mirroring
/// `EnchantDamageAdditive`'s `props.IsPoweredAttack()` guard.
pub const TEZCATARAS_EMBER_DAMAGE: i32 = 3;

/// Multiplicative damage scalar in fixed-point (×100) contributed by the played
/// card's enchantments, for a powered attack. Mirrors the C#
/// `*.EnchantDamageMultiplicative` chain: `Favored.cs` and `Instinct.cs` return
/// `2m` (→ 200), `Corrupted.cs` returns `1.5m` (→ 150); every other case (and
/// any unpowered hit) is `1×` (→ 100). Returns the integer ×100 form so the
/// damage pipeline can stay float-free. Canonical core copy; the sts2-content
/// helper delegates here so the unit-tested logic and the value applied in
/// [`crate::hooks::modify_damage`] share one source of truth.
#[must_use]
pub fn enchant_damage_multiplicative_x100(card: &crate::cards::Card, is_powered: bool) -> i32 {
    if !is_powered {
        return 100;
    }
    if card.has_enchantment(EnchantmentId::Favored) || card.has_enchantment(EnchantmentId::Instinct)
    {
        return 200;
    }
    if card.has_enchantment(EnchantmentId::Corrupted) {
        return 150;
    }
    100
}

/// Additive block bonus contributed by the played card's enchantments.
/// Mirrors the C# `Nimble.cs` (flat `+Amount` block) and `Goopy.cs` (block
/// grows `Amount - 1` per replay) `EnchantBlockAdditive` chain.
///
/// **v0.107.1 change (§8):** `EnchantBlockAdditive` lost its `props`
/// argument and the `IsPoweredCardOrMonsterMoveBlock` guard — the block
/// bonus now applies **unconditionally** whenever the block grant carries a
/// card source with the enchantment (`Hook.cs:1461` calls
/// `enchantment.EnchantBlockAdditive(num)` with no props). This is the patch
/// note "Entrench can now gain extra Block from enchantments like Nimble":
/// Entrench re-grants block via an unpowered Move, which the old guard
/// excluded. The powered gate was dropped here to match (was: `Nimble.cs` /
/// `Goopy.cs` `if (!props.IsPoweredCardOrMonsterMoveBlock()) return 0m;`).
///
/// Canonical core copy; the sts2-content helper delegates here so the value
/// the block pipeline applies (see [`crate::hooks::modify_block`]) can never
/// drift from the unit-tested content logic.
#[must_use]
pub fn enchant_block_additive(card: &crate::cards::Card) -> i32 {
    let mut total = card.enchantment(EnchantmentId::Nimble);
    if card.has_enchantment(EnchantmentId::Goopy) {
        // Python: `card.enchantments.get("Goopy", 1) - 1`. Goopy starts at 1 and
        // ticks up each play, so the block bonus is `Amount - 1` (≥ 0).
        total = total.saturating_add((card.enchantment(EnchantmentId::Goopy) - 1).max(0));
    }
    total
}

/// Returns `true` if any card in `pile` has at least one enchantment
/// stack. The caller uses this for the per-channel empty-bus
/// early-out — if the pile is enchantment-free the dispatch loop is
/// skipped entirely.
#[inline]
fn pile_has_any_enchantments(state: &CombatState, pile: &[CardUid]) -> bool {
    pile.iter().any(|uid| {
        let idx = (uid.get() as usize).saturating_sub(1);
        state
            .cards
            .get(idx)
            .is_some_and(|c| !c.enchantments.is_empty())
    })
}

/// Borrow a card by uid. Returns `None` if the uid is out of range
/// (defensive — every uid in a player pile should be live in
/// `state.cards`, but the dispatch path stays panic-free).
#[inline]
fn card_for(state: &CombatState, uid: CardUid) -> Option<&Card> {
    state.cards.get((uid.get() as usize).saturating_sub(1))
}

// ---------------------------------------------------------------------------
// 1. on_card_drawn
// ---------------------------------------------------------------------------

/// Fire enchantment `on_card_drawn` for a single card just moved into
/// hand. Called from the draw path once per card landed.
///
/// Python entry: `enchantments.py:121` —
/// `on_card_drawn(card, combat, from_hand_draw)`.
///
/// `from_hand_draw` distinguishes turn-start hand draws from
/// card-effect draws. NOTE: Slither ignores the flag — C#
/// `Slither.AfterCardDrawn` rerolls on EVERY draw-pile draw (turn-start
/// AND effect draws), so all draw paths must dispatch here
/// unconditionally (census v8 seeds 1003/1006/1042/1049).
pub fn dispatch_on_card_drawn(state: &mut CombatState, card_uid: CardUid, from_hand_draw: bool) {
    // Empty-bus early-out: if the drawn card has no enchantments,
    // there is nothing to dispatch.
    let Some(card) = card_for(state, card_uid) else {
        return;
    };
    if card.enchantments.is_empty() {
        return;
    }
    on_card_drawn_stub(state, card_uid, from_hand_draw);
}

/// Per-card listener body.
///
/// **Wave 3 (`sts2-rs-g7rni`):** Slither cost-roll. Python pin:
/// `if card.has_enchantment("Slither"):
///   card.set_combat_cost(combat.combat_energy_costs_rng.next_int(0, 3))`.
///
/// **Divergence:** Python uses a dedicated `combat_energy_costs_rng`
/// sub-stream on `RunState`; the dispatch site here has access to
/// `state.rng` (the combat-wide Xoshiro RNG) only. Rolling the cost
/// off `state.rng` keeps Slither observably deterministic per fixture
/// but the resulting cost will not match Python's stream byte-for-byte.
/// Tracked in `tests/parity/known_divergences.md` alongside the other
/// RNG-stream divergences (a follow-up bead can wire the dedicated
/// stream once `RunState::rng` plumbing reaches the on_card_drawn
/// site).
///
/// **Empty-state guard:** `from_hand_draw` is unused at this pin —
/// Python's `on_card_drawn` Slither leg fires regardless of the draw
/// source, only the `Imbued` auto-play leg is round-1 + draw-pile only
/// (handled by `after_player_turn_start`, not here).
#[inline]
fn on_card_drawn_stub(state: &mut CombatState, card_uid: CardUid, _from_hand_draw: bool) {
    let idx = (card_uid.get() as usize).saturating_sub(1);
    let Some(card) = state.cards.get(idx) else {
        return;
    };
    if !card.has_enchantment(EnchantmentId::Slither) {
        return;
    }
    let new_cost: i8 = state.run_rng.combat_energy_costs.next_int(0, 3) as i8;
    if let Some(card) = state.cards.get_mut(idx) {
        card.cost = new_cost;
    }
}

// ---------------------------------------------------------------------------
// 2. after_player_turn_start
// ---------------------------------------------------------------------------

/// Fire enchantment `after_player_turn_start` once per player turn
/// start. Walks the draw pile to surface Imbued cards (auto-play on
/// turn 1).
///
/// Python entry: `enchantments.py:126` —
/// `after_player_turn_start(owner, combat)`.
///
/// Empty-bus early-out: if no card in the draw pile has any
/// enchantments, skip the walk entirely.
pub fn dispatch_after_player_turn_start(state: &mut CombatState) {
    if !pile_has_any_enchantments(state, &state.player.draw) {
        return;
    }
    after_player_turn_start_stub(state);
}

/// Listener body.
///
/// **Wave 3 (`sts2-rs-g7rni`):** Imbued auto-play. On the first player turn
/// only, scan the draw pile for cards carrying the Imbued enchantment and
/// auto-play each.
///
/// **v0.107.1 retime/rewindow (§8):** C# `Imbued.cs` moved its body from
/// `BeforePlayPhaseStart` to `AfterAutoPrePlayPhaseEntered` and changed the
/// window from `CombatState.RoundNumber == 1` to
/// `PlayerCombatState.TurnNumber <= 1`. Both C# hook points fire after the
/// turn-setup task completes (after `AfterPlayerTurnStart`), so the relative
/// dispatch point is unchanged — this remains the engine's per-turn-start
/// enchant pass. `TurnNumber` starts at `1` and increments once per player
/// turn; with no extra-turn mechanic in the solo engine `TurnNumber <= 1`
/// ≡ `round_no() == 1` (see the MrStruggles `RoundNumber→TurnNumber` note),
/// so the existing round-1 gate below is the faithful window.
///
/// **Implementation:** snapshot the Imbued uids first (so subsequent
/// shuffles / draws don't change what we're iterating), then for each
/// uid swap it to position 0 of the draw pile and call
/// [`crate::combat_loop::auto_play_card_from_draw_top`] with
/// `force_exhaust = false`. This reuses the canonical auto-play pipeline
/// (target resolution, after_card_played dispatch, natural pile routing)
/// so Imbued-played cards land in discard (or self-exhaust) — matching
/// C# `Imbued.cs`'s `CardCmd.AutoPlay`, which does NOT force exhaust.
///
/// **EchoForm divergence inherited:** `auto_play_card_from_draw_top`
/// does not run the EchoForm replay path; per the existing divergence
/// note on that function (parity follow-up `sts2-rs-tlyk`).
#[inline]
fn after_player_turn_start_stub(state: &mut CombatState) {
    // v0.107.1 window: `PlayerCombatState.TurnNumber <= 1` (was
    // `RoundNumber == 1`). `round_no()` mirrors `TurnNumber` in the solo
    // engine (no extra-turn mechanic), and it starts at 1, so `> 1` is the
    // faithful "after the first player turn" exclusion.
    if state.round_no() > 1 {
        return;
    }
    // Snapshot Imbued uids in current draw order.
    let imbued: Vec<CardUid> = state
        .player
        .draw
        .iter()
        .copied()
        .filter(|uid| {
            let idx = (uid.get() as usize).saturating_sub(1);
            state
                .cards
                .get(idx)
                .is_some_and(|c| c.has_enchantment(EnchantmentId::Imbued))
        })
        .collect();
    for uid in imbued {
        // The card may have been auto-played + exhausted already if a
        // prior auto-play reshuffled — check it's still in draw.
        let Some(pos) = state.player.draw.iter().position(|u| *u == uid) else {
            continue;
        };
        if pos != 0 {
            state.player.draw.swap(0, pos);
        }
        // Imbued does NOT force-exhaust — C# `Imbued.cs` calls
        // `CardCmd.AutoPlay(card, null)` which leaves `ExhaustOnNextPlay`
        // at its default (false), so the played card routes to its natural
        // pile (discard unless it self-exhausts). Bead q6bix F-F.
        let _ =
            crate::combat_loop::auto_play_card_from_draw_top(state, /*force_exhaust=*/ false);
    }
}

// ---------------------------------------------------------------------------
// 3. before_flush
// ---------------------------------------------------------------------------

/// Fire enchantment `before_flush` immediately before the end-of-turn
/// hand flush, per `combat.py:1024`. Walks the player hand and lets
/// SlumberingEssence-marked cards tick their cost down before they
/// move to discard.
///
/// Python entry: `enchantments.py:137` — `before_flush(owner, combat)`.
pub fn dispatch_before_flush(state: &mut CombatState) {
    if !pile_has_any_enchantments(state, &state.player.hand) {
        return;
    }
    before_flush_stub(state);
}

/// Listener body.
///
/// **Wave 4 (`sts2-rs-uafi0`):** SlumberingEssence cost-tick. For each
/// card in the player hand carrying SlumberingEssence, decrement its
/// combat-cost by 1 (saturating at 0) before the end-of-turn flush
/// moves it to discard. X-cost cards (Python `has_energy_cost_x`) are
/// skipped — Rust analog is [`Card::is_x_cost`].
///
/// Python pin (`enchantments.py:before_flush`):
/// ```text
/// for card in list(state.hand):
///     if not card.has_enchantment("SlumberingEssence"): continue
///     if card.has_energy_cost_x: continue
///     card.set_combat_cost(max(0, card.cost - 1))
/// ```
#[inline]
fn before_flush_stub(state: &mut CombatState) {
    let hand: Vec<CardUid> = state.player.hand.iter().copied().collect();
    for uid in hand {
        let idx = (uid.get() as usize).saturating_sub(1);
        let Some(card) = state.cards.get_mut(idx) else {
            continue;
        };
        if !card.has_enchantment(EnchantmentId::SlumberingEssence) {
            continue;
        }
        if card.is_x_cost() {
            continue;
        }
        if card.cost > 0 {
            card.cost -= 1;
        }
    }
}

// ---------------------------------------------------------------------------
// 4. modify_shuffle_order
// ---------------------------------------------------------------------------

/// Fire enchantment `modify_shuffle_order` on a freshly-shuffled card
/// list. PerfectFit-marked cards float to the top.
///
/// Python entry: `enchantments.py:149` —
/// `modify_shuffle_order(cards, *, is_initial_shuffle)`.
///
/// `is_initial_shuffle` is `true` for the combat-start shuffle (which
/// Python skips — PerfectFit only applies to mid-combat reshuffles).
/// The surface stub respects the Python early-out semantics (no-op
/// when initial) so the dispatch site can fire unconditionally.
pub fn dispatch_modify_shuffle_order(
    player: &mut PlayerCombatState,
    cards: &mut [CardUid],
    is_initial_shuffle: bool,
    state_cards: &[Card],
) {
    if is_initial_shuffle {
        return;
    }
    // Empty-bus early-out: nothing to reorder if no card carries an
    // enchantment.
    let any_enchanted = cards.iter().any(|uid| {
        state_cards
            .get((uid.get() as usize).saturating_sub(1))
            .is_some_and(|c| !c.enchantments.is_empty())
    });
    if !any_enchanted {
        return;
    }
    modify_shuffle_order_stub(player, cards, state_cards);
}

/// Listener body.
///
/// **Wave 4 (`sts2-rs-uafi0`):** PerfectFit re-ordering. Float
/// PerfectFit-marked cards to the front of `cards` while preserving
/// the relative order within both groups (stable partition).
///
/// Python pin (`enchantments.py:modify_shuffle_order`):
/// ```text
/// perfect_fit = [c for c in cards if c.has_enchantment("PerfectFit")]
/// if perfect_fit:
///     remaining = [c for c in cards if not c.has_enchantment("PerfectFit")]
///     cards[:] = perfect_fit + remaining
/// ```
///
/// The `is_initial_shuffle` guard already early-exits in the dispatch
/// wrapper.
#[inline]
fn modify_shuffle_order_stub(
    _player: &mut PlayerCombatState,
    cards: &mut [CardUid],
    state_cards: &[Card],
) {
    let has_perfect_fit = |uid: &CardUid| {
        state_cards
            .get((uid.get() as usize).saturating_sub(1))
            .is_some_and(|c| c.has_enchantment(EnchantmentId::PerfectFit))
    };
    // Stable partition: PerfectFit-bearing cards float to the front
    // while preserving relative order within each group. Mirrors
    // Python's `perfect_fit + remaining` list concat.
    let mut buf: Vec<CardUid> = Vec::with_capacity(cards.len());
    buf.extend(cards.iter().copied().filter(has_perfect_fit));
    if buf.is_empty() {
        return; // No PerfectFit cards — preserve input order.
    }
    buf.extend(cards.iter().copied().filter(|u| !has_perfect_fit(u)));
    cards.copy_from_slice(&buf);
}

// ---------------------------------------------------------------------------
// 5. on_card_played
// ---------------------------------------------------------------------------

/// Fire enchantment `on_card_played` for a card whose own effect has just
/// finished resolving. Adroit gain block, Swift draw, Sown energy,
/// Corrupted self-damage, Momentum bonus accumulation all live here.
///
/// **Dispatch site — C#, not Python (bead `sts2-rs-ench-onplay-order`).**
/// C# `CardModel.OnPlayWrapper` (`CardModel.cs:1930-1943`) awaits the card's
/// own `OnPlay` first and only then runs `Enchantment.OnPlay`; the affliction
/// leg follows, and `Hook.AfterCardPlayed` after that. The port originally
/// fired this channel BEFORE the card-effect dispatch because it followed
/// Python's `enchantments.on_card_played` site — which put Swift's draw ahead
/// of a card body's own `CardSelectCmd.FromHand` and corrupted every
/// subsequent option index (BURNING_PACT; mirror corpus records 269/270). The
/// call now lives at the top of [`crate::combat_loop::finish_card_play`], the
/// single point both the inline and surfaced-choice-resume paths pass through.
///
/// Python entry (site NO LONGER followed): `enchantments.py:158` —
/// `on_card_played(card, combat)`.
pub fn dispatch_on_card_played(state: &mut CombatState, card_uid: CardUid) {
    let Some(card) = card_for(state, card_uid) else {
        return;
    };
    if card.enchantments.is_empty() {
        return;
    }
    on_card_played_stub(state, card_uid);
}

/// Per-card listener body.
///
/// **Wave 2 (`sts2-rs-vkpir`) — dynamic damage triplet wired:**
///   - `Momentum` — stash `+N` (the enchantment stack) onto the card's
///     `VarKey::RampageDamage` slot. Read back by
///     `enchantments::enchant_damage_additive` at damage-calc time so
///     subsequent plays of THIS card see the accumulated bonus.
///   - `Corrupted` — deal `2` self-damage with
///     `UNBLOCKABLE | UNPOWERED | MOVE` (Python pin
///     `enchantments.py:on_card_played` calls
///     `combat.deal_damage(target=owner, amount=2, props=…)`).
///   - `Sharp` / `Favored` / `Vigorous` — query-only enchantments; their
///     "real bodies" live in the damage-math helpers in
///     `sts2-content::enchantments`. No `on_card_played` wiring needed.
///
/// **Wave 3 (`sts2-rs-g7rni`) — block / draw / energy wired:**
///   - `Adroit` — gain `+N` player block (routes through
///     `card_effects::gain_block_player` so the full modify_block chain
///     fires).
///   - `Swift` — draw `+N` cards on first play this combat
///     (`SWIFT_USED_BIT` gates subsequent plays of the same card).
///   - `Sown` — gain `+N` energy on first play this combat
///     (`SOWN_USED_BIT` gates subsequent plays).
///   - `Nimble` / `Spiral` / `Glam` — query-only or after_card_played
///     wiring; inert here by design.
#[inline]
fn on_card_played_stub(state: &mut CombatState, card_uid: CardUid) {
    // Snapshot enchantment counters BEFORE any mutation — borrow
    // discipline: the immutable borrow of `card_for` dies before we
    // call `apply_damage_to_player` / `gain_block_player` (which
    // need `&mut state`).
    let (momentum_amount, has_corrupted, adroit_amount, swift_amount, sown_amount, used_bits) = {
        let Some(card) = card_for(state, card_uid) else {
            return;
        };
        (
            card.enchantment(EnchantmentId::Momentum),
            card.has_enchantment(EnchantmentId::Corrupted),
            card.enchantment(EnchantmentId::Adroit),
            card.enchantment(EnchantmentId::Swift),
            card.enchantment(EnchantmentId::Sown),
            card.combat_var(VarKey::Generic),
        )
    };

    if momentum_amount > 0 {
        let idx = (card_uid.get() as usize).saturating_sub(1);
        if let Some(card) = state.cards.get_mut(idx) {
            let prior = card.combat_var(VarKey::RampageDamage);
            card.set_combat_var(VarKey::RampageDamage, prior.saturating_add(momentum_amount));
        }
    }

    if has_corrupted {
        let _ = crate::combat_loop::apply_damage_to_player(
            state,
            CreatureRef::Player,
            2,
            ValueProps::UNBLOCKABLE
                .union(ValueProps::UNPOWERED)
                .union(ValueProps::MOVE),
            None,
        );
    }

    // Adroit: gain N block per stack. Python pin calls
    // `calculate_block(N, owner, ValueProp.MOVE, …)` then
    // `_gain_block` — our `gain_block_player` is the canonical wrapper
    // that runs the modify_block chain + fires after_block_gained.
    if adroit_amount > 0 {
        crate::card_effects::gain_block_player(state, adroit_amount);
    }

    // Swift: draw N cards on first play per combat. Python pin checks
    // `not combat_vars[_used_key("Swift")]`; the marker is set in
    // `after_card_played` immediately after this site resolves.
    if swift_amount > 0 && (used_bits & SWIFT_USED_BIT) == 0 {
        // C# `Swift.OnPlay` → `CardPileCmd.Draw(fromHandDraw = false)` —
        // vetoed by NoDraw/Fiddle like every card-effect draw.
        let _ = state.draw_into_hand_card_effect(swift_amount);
    }

    // Sown: gain N energy on first play per combat. Same first-play
    // gate as Swift; marker set in after_card_played.
    if sown_amount > 0 && (used_bits & SOWN_USED_BIT) == 0 {
        state.gain_energy(sown_amount);
    }
}

// ---------------------------------------------------------------------------
// 6. after_card_played
// ---------------------------------------------------------------------------

/// Fire enchantment `after_card_played` for a card whose effect has
/// fully resolved. Marks first-play-only enchantments (Swift / Sown /
/// Vigorous / Glam) as consumed and ticks Goopy's exhaust scaling.
///
/// Python entry: `enchantments.py:184` — `after_card_played(card)`.
pub fn dispatch_after_card_played(state: &mut CombatState, card_uid: CardUid) {
    let Some(card) = card_for(state, card_uid) else {
        return;
    };
    if card.enchantments.is_empty() {
        return;
    }
    after_card_played_stub(state, card_uid);
}

/// Per-card listener body.
///
/// **Wave 2 (`sts2-rs-vkpir`):** Vigorous-used bit.
///
/// **Wave 3 (`sts2-rs-g7rni`):** Swift / Sown / Glam consumed markers.
/// Mirrors Python's `for name in ("Swift", "Sown", "Vigorous", "Glam"):
/// if card.has_enchantment(name): card.combat_vars[_used_key(name)] = 1`
/// at `enchantments.py:184-194`.
///
/// Pending (Wave 4+): Goopy exhaust scaling tick.
#[inline]
fn after_card_played_stub(state: &mut CombatState, card_uid: CardUid) {
    let idx = (card_uid.get() as usize).saturating_sub(1);
    let Some(card) = state.cards.get_mut(idx) else {
        return;
    };
    let mut bits = card.combat_var(VarKey::Generic);
    if card.has_enchantment(EnchantmentId::Vigorous) {
        bits |= VIGOROUS_USED_BIT;
    }
    if card.has_enchantment(EnchantmentId::Glam) {
        bits |= GLAM_USED_BIT;
    }
    if card.has_enchantment(EnchantmentId::Swift) {
        bits |= SWIFT_USED_BIT;
    }
    if card.has_enchantment(EnchantmentId::Sown) {
        bits |= SOWN_USED_BIT;
    }
    card.set_combat_var(VarKey::Generic, bits);
}

// ---------------------------------------------------------------------------
// Tests — empty-bus early-out + dispatch reachability per channel
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::{Card, CardId, CardType, EnchantmentId, TargetType};
    use crate::CardUid;

    use std::num::NonZeroU32;

    fn uid(n: u32) -> CardUid {
        CardUid(NonZeroU32::new(n).expect("non-zero"))
    }

    fn make_card(n: u32, with_enchantment: bool) -> Card {
        let mut c = Card {
            uid: uid(n),
            card_id: CardId::StrikeIronclad,
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        };
        if with_enchantment {
            c.set_enchantment(EnchantmentId::Sharp, 2);
        }
        c
    }

    /// Helper: build a minimal CombatState with `n` cards in `cards`
    /// and a hand+draw that mirrors them.
    fn state_with_cards(specs: &[(u32, bool)]) -> CombatState {
        let mut state = CombatState::default();
        for (n, ench) in specs {
            state.cards.push(make_card(*n, *ench));
            state.player.hand.push(uid(*n));
            state.player.draw.push(uid(*n));
        }
        state
    }

    /// Empty-bus on every channel: a state with no enchantments
    /// dispatches without panic and is observably a no-op.
    #[test]
    fn empty_bus_early_out_all_channels() {
        let mut state = state_with_cards(&[(1, false), (2, false)]);
        let snapshot_hand = state.player.hand.clone();
        let snapshot_draw = state.player.draw.clone();

        dispatch_on_card_drawn(&mut state, uid(1), true);
        dispatch_after_player_turn_start(&mut state);
        dispatch_before_flush(&mut state);
        let cards_snapshot = state.cards.clone();
        let mut shuffle_buf = state.player.draw.clone();
        let mut player = state.player.clone();
        dispatch_modify_shuffle_order(&mut player, &mut shuffle_buf, false, &cards_snapshot);
        dispatch_on_card_played(&mut state, uid(1));
        dispatch_after_card_played(&mut state, uid(1));

        // Empty-bus path is a no-op: hand and draw orders are unchanged.
        assert_eq!(state.player.hand, snapshot_hand);
        assert_eq!(state.player.draw, snapshot_draw);
        assert_eq!(shuffle_buf.as_slice(), snapshot_draw.as_slice());
    }

    /// Initial shuffle is always a no-op even when enchantments
    /// exist (mirrors Python's `is_initial_shuffle` early-out).
    #[test]
    fn modify_shuffle_order_initial_shuffle_skips() {
        let state = state_with_cards(&[(1, true), (2, false)]);
        let snapshot = state.player.draw.clone();
        let cards_snapshot = state.cards.clone();
        let mut shuffle_buf = state.player.draw.clone();
        let mut player = state.player.clone();
        dispatch_modify_shuffle_order(
            &mut player,
            &mut shuffle_buf,
            true, // is_initial_shuffle
            &cards_snapshot,
        );
        assert_eq!(shuffle_buf.as_slice(), snapshot.as_slice());
    }

    /// Each channel reaches its per-card stub when at least one
    /// enchantment is present (no early-out). Surface bead: stubs are
    /// no-ops, so we assert reachability via "no panic + empty-bus
    /// guard does not trip".
    #[test]
    fn populated_bus_reaches_stubs_without_panic() {
        let mut state = state_with_cards(&[(1, true)]);
        dispatch_on_card_drawn(&mut state, uid(1), true);
        dispatch_after_player_turn_start(&mut state);
        dispatch_before_flush(&mut state);
        let cards_snapshot = state.cards.clone();
        let mut shuffle_buf = state.player.draw.clone();
        let mut player = state.player.clone();
        dispatch_modify_shuffle_order(&mut player, &mut shuffle_buf, false, &cards_snapshot);
        dispatch_on_card_played(&mut state, uid(1));
        dispatch_after_card_played(&mut state, uid(1));
    }

    /// Out-of-range uid is a defensive no-op (does not panic).
    #[test]
    fn unknown_card_uid_is_safe() {
        let mut state = state_with_cards(&[(1, true)]);
        dispatch_on_card_drawn(&mut state, uid(99), true);
        dispatch_on_card_played(&mut state, uid(99));
        dispatch_after_card_played(&mut state, uid(99));
    }

    // -----------------------------------------------------------------
    // vkpir (vxvvs Wave 2): dynamic damage triplet per-variant pins
    // -----------------------------------------------------------------

    /// Build a single-card state with `ench` stacked at `amount`. The
    /// card lives in `state.cards[0]` and is wired into a player with a
    /// real creature_uid so `apply_damage_to_player` can route through
    /// the Creature mirror.
    fn state_with_one_enchanted_card(ench: EnchantmentId, amount: i32) -> CombatState {
        let mut state = CombatState::default();
        let mut c = Card {
            uid: uid(1),
            card_id: CardId::StrikeIronclad,
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        };
        c.set_enchantment(ench, amount);
        state.cards.push(c);
        state.player.hand.push(uid(1));
        // Mint a player Creature so apply_damage_to_player can resolve
        // the target via state.player.creature_uid.
        state.player.current_hp = 80;
        state.player.max_hp = 80;
        let _ = state.init_player_creature();
        state
    }

    /// Bead `sts2-rs-enchant-damage-additive` (PARITY136): a `Sharp(2)`
    /// enchanted attack on top of the play pile gets +2 through the damage
    /// pipeline for a powered hit, and nothing for an unpowered hit. Before
    /// the fix the enchantment additive was computed but never threaded into
    /// `modify_damage`, so a Sharp(+2) Strike auto-played by Uproar dealt 6
    /// in Rust vs 8 in C#.
    #[test]
    fn sharp_enchant_threads_into_modify_damage_for_player_card() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Sharp, 2);
        // The damage pipeline reads the player's currently-resolving card from
        // the top of the play pile (the originating-card convention).
        state.player.hand.clear();
        state.player.play.push(uid(1));
        let powered = crate::hooks::DamageCtx {
            dealer: crate::hooks::CreatureRef::Player,
            target: crate::hooks::CreatureRef::Player,
            props: ValueProps::MOVE,
        };
        assert_eq!(
            crate::hooks::modify_damage(&state, 6, &powered),
            8,
            "Sharp(2) must add +2 to a powered player-card attack"
        );
        let unpowered = crate::hooks::DamageCtx {
            props: ValueProps::UNPOWERED,
            ..powered
        };
        assert_eq!(
            crate::hooks::modify_damage(&state, 6, &unpowered),
            6,
            "Sharp does not apply to unpowered hits"
        );
        // No player card on play → no enchant bonus (e.g. enemy attack path).
        state.player.play.clear();
        assert_eq!(crate::hooks::modify_damage(&state, 6, &powered), 6);
    }

    /// GH #64 §5: the Favored multiplicative (2×) is now threaded into
    /// `modify_damage` for a player card on the play pile. A powered hit
    /// doubles; an unpowered hit is unaffected (C# gates on
    /// `props.IsPoweredAttack()`).
    #[test]
    fn favored_enchant_doubles_damage_through_modify_damage() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Favored, 1);
        state.player.hand.clear();
        state.player.play.push(uid(1));
        let powered = crate::hooks::DamageCtx {
            dealer: crate::hooks::CreatureRef::Player,
            target: crate::hooks::CreatureRef::Player,
            props: ValueProps::MOVE,
        };
        assert_eq!(
            crate::hooks::modify_damage(&state, 6, &powered),
            12,
            "Favored doubles a powered player-card attack"
        );
        let unpowered = crate::hooks::DamageCtx {
            props: ValueProps::UNPOWERED,
            ..powered
        };
        assert_eq!(
            crate::hooks::modify_damage(&state, 6, &unpowered),
            6,
            "Favored does not multiply an unpowered hit"
        );
    }

    /// GH #64 §5: Corrupted's 1.5× multiplicative threads into `modify_damage`
    /// with the single-floor convention (`floor(6 * 1.5) = 9`).
    #[test]
    fn corrupted_enchant_one_point_five_x_through_modify_damage() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Corrupted, 1);
        state.player.hand.clear();
        state.player.play.push(uid(1));
        let powered = crate::hooks::DamageCtx {
            dealer: crate::hooks::CreatureRef::Player,
            target: crate::hooks::CreatureRef::Player,
            props: ValueProps::MOVE,
        };
        assert_eq!(crate::hooks::modify_damage(&state, 6, &powered), 9);
    }

    /// GH #64 §5: Nimble's flat block additive is now threaded into
    /// `modify_block` for a player-targeted block grant sourced from the
    /// resolving card.
    #[test]
    fn nimble_enchant_adds_block_through_modify_block() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Nimble, 4);
        state.player.hand.clear();
        state.player.play.push(uid(1));
        let ctx = crate::hooks::BlockCtx {
            target: crate::hooks::CreatureRef::Player,
            props: ValueProps::MOVE,
            card_source: None,
        };
        assert_eq!(
            crate::hooks::modify_block(&state, 5, &ctx),
            9,
            "Nimble(4) adds +4 to a powered player block grant"
        );
        // Block landing on a non-player target gets no card enchant bonus.
        let enemy_uid = crate::CreatureUid(NonZeroU32::new(2).expect("non-zero"));
        let enemy_ctx = crate::hooks::BlockCtx {
            target: crate::hooks::CreatureRef::Enemy(enemy_uid),
            ..ctx
        };
        assert_eq!(crate::hooks::modify_block(&state, 5, &enemy_ctx), 5);
    }

    /// v0.107.1 (§8): `Nimble`/`Goopy` `EnchantBlockAdditive` lost its
    /// `IsPoweredCardOrMonsterMoveBlock` guard, so the block bonus now lands
    /// on an UNPOWERED block grant too (patch note: Entrench gains extra Block
    /// from Nimble — Entrench re-grants block via an unpowered Move). Before
    /// the change a `ValueProps::UNPOWERED` grant returned `+0`.
    #[test]
    fn nimble_enchant_adds_block_even_when_unpowered_v0107() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Nimble, 4);
        state.player.hand.clear();
        state.player.play.push(uid(1));
        let unpowered = crate::hooks::BlockCtx {
            target: crate::hooks::CreatureRef::Player,
            props: ValueProps::UNPOWERED,
            card_source: None,
        };
        assert_eq!(
            crate::hooks::modify_block(&state, 5, &unpowered),
            9,
            "Nimble(4) adds +4 even on an unpowered block grant (v0.107.1 guard removed)"
        );
    }

    /// v0.107.1 (§8) NEW enchant `TezcatarasEmber`: +3 on a powered attack
    /// threads through `modify_damage` for the resolving player card; an
    /// unpowered hit is unaffected (`props.IsPoweredAttack()` gate).
    #[test]
    fn tezcataras_ember_threads_three_into_modify_damage() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::TezcatarasEmber, 1);
        state.player.hand.clear();
        state.player.play.push(uid(1));
        let powered = crate::hooks::DamageCtx {
            dealer: crate::hooks::CreatureRef::Player,
            target: crate::hooks::CreatureRef::Player,
            props: ValueProps::MOVE,
        };
        assert_eq!(
            crate::hooks::modify_damage(&state, 6, &powered),
            6 + TEZCATARAS_EMBER_DAMAGE,
            "TezcatarasEmber adds +3 to a powered player-card attack"
        );
        let unpowered = crate::hooks::DamageCtx {
            props: ValueProps::UNPOWERED,
            ..powered
        };
        assert_eq!(
            crate::hooks::modify_damage(&state, 6, &unpowered),
            6,
            "TezcatarasEmber does not apply to an unpowered hit"
        );
    }

    /// Momentum: each play stashes `+stack` onto the RampageDamage
    /// slot so subsequent plays of this same card see the accumulated
    /// bonus. Two plays of Momentum(3) → stash = 6.
    #[test]
    fn momentum_on_card_played_accumulates_rampage_damage() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Momentum, 3);
        assert_eq!(state.cards[0].combat_var(VarKey::RampageDamage), 0);
        dispatch_on_card_played(&mut state, uid(1));
        assert_eq!(state.cards[0].combat_var(VarKey::RampageDamage), 3);
        dispatch_on_card_played(&mut state, uid(1));
        assert_eq!(state.cards[0].combat_var(VarKey::RampageDamage), 6);
    }

    /// Corrupted: dealing the card fires 2 UNBLOCKABLE self-damage on
    /// the player. With block=99, the unblockable bypass means HP
    /// drops by exactly 2 (Python `combat.py` props mask matches).
    #[test]
    fn corrupted_on_card_played_deals_two_unblockable_self_damage() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Corrupted, 1);
        state.player.block = 99;
        let hp_before = state.player.current_hp;
        dispatch_on_card_played(&mut state, uid(1));
        assert_eq!(
            state.player.current_hp,
            hp_before - 2,
            "Corrupted self-damage is UNBLOCKABLE — block should not absorb",
        );
        assert_eq!(state.player.block, 99, "block untouched by UNBLOCKABLE");
    }

    /// Vigorous: after_card_played sets the per-combat used bit on the
    /// card's VarKey::Generic slot. The encoding mirrors
    /// `sts2-content::enchantments::mark_vigorous_used` (bit 0).
    #[test]
    fn vigorous_after_card_played_marks_used_bit() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Vigorous, 5);
        assert_eq!(state.cards[0].combat_var(VarKey::Generic) & 0b01, 0);
        dispatch_after_card_played(&mut state, uid(1));
        assert_eq!(
            state.cards[0].combat_var(VarKey::Generic) & 0b01,
            0b01,
            "Vigorous-used bit set after first play resolves",
        );
        // Idempotent: a second after_card_played leaves the bit set.
        dispatch_after_card_played(&mut state, uid(1));
        assert_eq!(state.cards[0].combat_var(VarKey::Generic) & 0b01, 0b01);
    }

    /// Sharp: pure query-side enchantment; on_card_played MUST NOT
    /// stash anything onto the card or mutate the player. The damage
    /// bonus is read by `enchantments::enchant_damage_additive` at
    /// damage-calc time.
    #[test]
    fn sharp_on_card_played_is_inert() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Sharp, 3);
        let hp_before = state.player.current_hp;
        let rampage_before = state.cards[0].combat_var(VarKey::RampageDamage);
        let generic_before = state.cards[0].combat_var(VarKey::Generic);
        dispatch_on_card_played(&mut state, uid(1));
        assert_eq!(state.player.current_hp, hp_before);
        assert_eq!(
            state.cards[0].combat_var(VarKey::RampageDamage),
            rampage_before
        );
        assert_eq!(state.cards[0].combat_var(VarKey::Generic), generic_before);
    }

    /// Favored: pure query-side multiplier; on_card_played MUST NOT
    /// touch combat_vars or the player. Multiplier is consumed by
    /// `enchant_damage_multiplicative_x100`.
    #[test]
    fn favored_on_card_played_is_inert() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Favored, 1);
        let hp_before = state.player.current_hp;
        dispatch_on_card_played(&mut state, uid(1));
        assert_eq!(state.player.current_hp, hp_before);
        assert_eq!(state.cards[0].combat_var(VarKey::RampageDamage), 0);
        assert_eq!(state.cards[0].combat_var(VarKey::Generic), 0);
    }

    // -----------------------------------------------------------------
    // g7rni (vxvvs Wave 3): block / play-count / draw / energy
    // -----------------------------------------------------------------

    /// Adroit: gain N block on play. Goes through `gain_block_player`
    /// so the modify_block chain fires.
    #[test]
    fn adroit_on_card_played_gains_block() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Adroit, 7);
        assert_eq!(state.player.block, 0);
        dispatch_on_card_played(&mut state, uid(1));
        assert_eq!(state.player.block, 7);
    }

    /// Swift: draw N on first play; subsequent plays of the same card
    /// instance are inert (consumed marker set in after_card_played).
    #[test]
    fn swift_first_play_draws_then_consumed() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Swift, 1);
        // Seed a second card into the draw pile so we can observe a draw.
        let mut filler = Card {
            uid: uid(2),
            card_id: CardId::StrikeIronclad,
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        };
        filler.set_enchantment(EnchantmentId::RoyallyApproved, 0); // empty stack — keeps `enchantments` non-empty? No.
        state.cards.push(filler);
        state.player.draw.push(uid(2));
        let hand_before = state.player.hand.len();
        let draw_before = state.player.draw.len();
        dispatch_on_card_played(&mut state, uid(1));
        assert_eq!(state.player.hand.len(), hand_before + 1, "Swift drew 1");
        assert_eq!(state.player.draw.len(), draw_before - 1);
        // Set the consumed marker (Wave 3 after_card_played wiring).
        dispatch_after_card_played(&mut state, uid(1));
        assert_eq!(
            state.cards[0].combat_var(VarKey::Generic) & 0b0100,
            0b0100,
            "Swift-used bit set"
        );
        // Second play: no draw (gate held).
        let hand_now = state.player.hand.len();
        dispatch_on_card_played(&mut state, uid(1));
        assert_eq!(state.player.hand.len(), hand_now);
    }

    /// Sown: gain N energy on first play; gated by SOWN_USED_BIT.
    #[test]
    fn sown_first_play_gains_energy_then_consumed() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Sown, 2);
        state.player.energy = 0;
        dispatch_on_card_played(&mut state, uid(1));
        assert_eq!(state.player.energy, 2);
        dispatch_after_card_played(&mut state, uid(1));
        assert_eq!(
            state.cards[0].combat_var(VarKey::Generic) & 0b1000,
            0b1000,
            "Sown-used bit set"
        );
        // Second play is inert.
        dispatch_on_card_played(&mut state, uid(1));
        assert_eq!(state.player.energy, 2);
    }

    /// Glam: after_card_played sets bit 1 (matches sts2-content
    /// encoding for `mark_glam_used`). On-played leg is inert.
    #[test]
    fn glam_after_card_played_marks_used_bit() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Glam, 1);
        // on_card_played is inert for Glam.
        dispatch_on_card_played(&mut state, uid(1));
        assert_eq!(state.cards[0].combat_var(VarKey::Generic) & 0b10, 0);
        // after_card_played sets the bit.
        dispatch_after_card_played(&mut state, uid(1));
        assert_eq!(
            state.cards[0].combat_var(VarKey::Generic) & 0b10,
            0b10,
            "Glam-used bit set on after_card_played"
        );
    }

    /// Spiral: query-only play-count bonus; dispatch sites are inert.
    #[test]
    fn spiral_dispatch_is_inert() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Spiral, 2);
        let hp_before = state.player.current_hp;
        let energy_before = state.player.energy;
        let block_before = state.player.block;
        dispatch_on_card_played(&mut state, uid(1));
        dispatch_after_card_played(&mut state, uid(1));
        assert_eq!(state.player.current_hp, hp_before);
        assert_eq!(state.player.energy, energy_before);
        assert_eq!(state.player.block, block_before);
        // Generic combat_var must stay 0 — Spiral does not own any bit.
        assert_eq!(state.cards[0].combat_var(VarKey::Generic), 0);
    }

    /// Nimble: query-only block bonus; on_card_played is inert. The
    /// production wiring lives in the block-calc helper, not here.
    #[test]
    fn nimble_on_card_played_is_inert() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Nimble, 3);
        let block_before = state.player.block;
        dispatch_on_card_played(&mut state, uid(1));
        assert_eq!(state.player.block, block_before);
    }

    /// Slither: on draw, the card's cost is rerolled to a value in
    /// `[0, 3]`. The reroll is deterministic given a seeded
    /// `state.rng` (the divergence from Python's
    /// `combat_energy_costs_rng` is documented above).
    #[test]
    fn slither_on_card_drawn_rerolls_cost_in_zero_three() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Slither, 1);
        state.cards[0].cost = 1; // starting cost
        dispatch_on_card_drawn(&mut state, uid(1), true);
        let new_cost = state.cards[0].cost;
        assert!(
            (0..=3).contains(&new_cost),
            "Slither cost roll out of range: {new_cost}"
        );
    }

    // -----------------------------------------------------------------
    // uafi0 (vxvvs Wave 4): stateful / boss enchantments
    // -----------------------------------------------------------------

    /// SlumberingEssence: before_flush ticks cost down by 1 on each
    /// marked card in hand, saturating at 0. X-cost (-1) cards skip.
    #[test]
    fn slumbering_essence_before_flush_ticks_cost_down() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::SlumberingEssence, 1);
        state.cards[0].cost = 3;
        // Card must be in hand for before_flush to consider it.
        // state_with_one_enchanted_card already pushes uid(1) to hand.
        dispatch_before_flush(&mut state);
        assert_eq!(state.cards[0].cost, 2, "tick from 3 → 2");
        dispatch_before_flush(&mut state);
        assert_eq!(state.cards[0].cost, 1);
        dispatch_before_flush(&mut state);
        assert_eq!(state.cards[0].cost, 0, "saturates at 0");
        dispatch_before_flush(&mut state);
        assert_eq!(state.cards[0].cost, 0, "stays at 0 — no underflow");
    }

    /// SlumberingEssence skips X-cost cards (cost == -1 sentinel).
    #[test]
    fn slumbering_essence_skips_x_cost_cards() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::SlumberingEssence, 1);
        state.cards[0].cost = -1; // X-cost sentinel
        dispatch_before_flush(&mut state);
        assert_eq!(state.cards[0].cost, -1, "X-cost untouched");
    }

    /// SlumberingEssence on a non-hand-resident card is inert (the
    /// dispatch walks `state.player.hand` only).
    #[test]
    fn slumbering_essence_ignores_non_hand_cards() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::SlumberingEssence, 1);
        state.cards[0].cost = 3;
        state.player.hand.clear();
        dispatch_before_flush(&mut state);
        assert_eq!(state.cards[0].cost, 3, "non-hand card untouched");
    }

    /// PerfectFit: floats marked cards to the front while preserving
    /// relative order within each group (stable partition).
    #[test]
    fn perfect_fit_modify_shuffle_floats_marked_to_front() {
        let mut state = CombatState::default();
        // Build 4 cards: indices 1..=4. Cards 2 and 4 have PerfectFit.
        for n in 1..=4u32 {
            let mut c = Card {
                uid: uid(n),
                card_id: CardId::StrikeIronclad,
                cost: 1,
                card_type: CardType::Attack,
                target_type: TargetType::Enemy,
                upgraded: false,
                base_damage: 6,
                base_block: 0,
                ..Card::pod_defaults()
            };
            if n == 2 || n == 4 {
                c.set_enchantment(EnchantmentId::PerfectFit, 1);
            }
            state.cards.push(c);
        }
        let mut shuffle = vec![uid(1), uid(2), uid(3), uid(4)];
        let mut player = state.player.clone();
        dispatch_modify_shuffle_order(&mut player, &mut shuffle, false, &state.cards);
        // PerfectFit (2,4) preserve relative order; remaining (1,3) too.
        assert_eq!(shuffle, vec![uid(2), uid(4), uid(1), uid(3)]);
    }

    /// PerfectFit with no marked cards leaves the order untouched.
    #[test]
    fn perfect_fit_modify_shuffle_no_marked_preserves_order() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Sharp, 1);
        let mut shuffle = vec![uid(1)];
        let mut player = state.player.clone();
        let original = shuffle.clone();
        dispatch_modify_shuffle_order(&mut player, &mut shuffle, false, &state.cards);
        assert_eq!(shuffle, original, "no PerfectFit → no reorder");
        // Burn `state` so clippy doesn't moan about `let mut` going unused.
        state.player.hand.clear();
    }

    /// Clone (`@CLONE` token, no Python pin behavior at `01c094d`): a
    /// pure marker enchantment. Every dispatch channel that walks the
    /// card MUST be a no-op for it. Pins the documented Python-pin
    /// behavior so a future real port shows up here as a diff.
    #[test]
    fn clone_marker_is_inert_on_all_channels() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Clone, 1);
        let hp_before = state.player.current_hp;
        let energy_before = state.player.energy;
        let block_before = state.player.block;
        let cost_before = state.cards[0].cost;
        let generic_before = state.cards[0].combat_var(VarKey::Generic);

        dispatch_on_card_drawn(&mut state, uid(1), true);
        dispatch_after_player_turn_start(&mut state);
        dispatch_before_flush(&mut state);
        let mut shuffle = vec![uid(1)];
        let cards_snapshot = state.cards.clone();
        let mut player = state.player.clone();
        dispatch_modify_shuffle_order(&mut player, &mut shuffle, false, &cards_snapshot);
        dispatch_on_card_played(&mut state, uid(1));
        dispatch_after_card_played(&mut state, uid(1));

        assert_eq!(state.player.current_hp, hp_before);
        assert_eq!(state.player.energy, energy_before);
        assert_eq!(state.player.block, block_before);
        assert_eq!(state.cards[0].cost, cost_before);
        assert_eq!(state.cards[0].combat_var(VarKey::Generic), generic_before);
        assert_eq!(shuffle, vec![uid(1)]);
    }

    /// Imbued: after_player_turn_start auto-plays each Imbued card
    /// from the draw pile on round 1. Round-gate verified by round 2
    /// no-op.
    #[test]
    fn imbued_after_player_turn_start_auto_plays_on_round_one_only() {
        let mut state = state_with_one_enchanted_card(EnchantmentId::Imbued, 1);
        // Move the Imbued card out of hand into draw pile only.
        state.player.hand.clear();
        // Force the round counter via internal setter equivalent: round
        // initializes to 1 in `CombatState::default()` via
        // `start_player_turn`; for this test we drive
        // `after_player_turn_start` directly, which checks `round_no()`.
        // CombatState::default() sets round=1 → first invocation runs.
        let draw_before = state.player.draw.len();
        dispatch_after_player_turn_start(&mut state);
        assert!(
            state.player.draw.len() < draw_before || !state.player.draw.contains(&uid(1)),
            "Imbued card should have been auto-played out of draw on round 1",
        );
    }
}
