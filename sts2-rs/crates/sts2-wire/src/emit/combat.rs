//! `CombatState` → the wire dict, plus the bridge-shape action /
//! pending-choice dicts and the hand previews.
//!
//! [`combat_state_to_value`] is the inverse of [`crate::state_from_wire`]. It
//! emits the canonical wire schema (`docs/sts2-hint-state-schema.json`, the
//! same shape [`crate::parse_state_dict`] accepts on the way in).
//!
//! Lossy by design — see the `state_equiv()` test predicate in
//! `sts2-py/tests/round_trip.rs` for the exhaustive list of fields
//! intentionally NOT round-tripped. The significant ones:
//!
//!   * `rng` (`Xoshiro256PlusPlus`) — only the wire-schema-recorded
//!     `rng.seed` survives; the in-process cursor is opaque.
//!   * `combat_log` (`TurnEvents`) — append-only buffer, not in the
//!     wire format.
//!   * Hook listeners — closure state on the relic / power buses,
//!     reconstructed lazily from the relic/power kinds on the next
//!     `apply()`.
//!   * `intent_history` — Rust's internal shape is private; the wire
//!     format only carries an i32-encoded summary which we re-emit as
//!     an empty list because `state_from_dict_impl` defaults it anyway.
//!
//! All other wire-schema fields are bit-faithful: the parsed state
//! emitted by `parse_state_dict(to_dict(s))` should match the parsed
//! state that produced `s`, modulo the dropped fields above.

use std::collections::BTreeMap;

use serde_json::Value;

use sts2_core::cards::{Card, CardType, VarKey};
use sts2_core::{
    Action, CardUid, CharacterId, ChoiceOption, CombatPhase, CombatState, CreatureUid, Intent,
    MonsterKind, Orb, PendingChoice, Power, PowerKind, PowerUid, Side, DARK_ORB_BASE_EVOKE,
};

use super::map_ext::{obj, set, val};
use super::names::{card_type_python_name, choice_kind_python_name};
use super::EmitError;
use crate::state_dict::{card_affliction_kind_wire_name, var_key_wire_name, WIRE_VERSION};

const fn combat_phase_to_str(p: CombatPhase) -> &'static str {
    match p {
        CombatPhase::Setup => "Setup",
        CombatPhase::PlayerTurnStart => "PlayerDraw",
        CombatPhase::PlayerPlayPhase
        | CombatPhase::PlayerEndTurnPhaseOne
        | CombatPhase::PlayerEndTurnPhaseTwo => "PlayerPlayPhase",
        CombatPhase::EnemyTurnStart | CombatPhase::EnemyTurnExec | CombatPhase::EnemyTurnEnd => {
            "EnemyPhase"
        }
        CombatPhase::Resolved { player_won: true } => "Victory",
        CombatPhase::Resolved { player_won: false } => "Defeat",
    }
}

const fn side_to_str(s: Side) -> &'static str {
    match s {
        Side::Player => "Player",
        Side::Enemy => "Enemy",
    }
}

const fn class_str_from_character(c: Option<CharacterId>) -> &'static str {
    match c {
        // `None` (no character gating set yet) defaults to "ironclad" so
        // callers don't see a schema rejection.
        Some(CharacterId::Ironclad) | None => "ironclad",
        Some(CharacterId::Silent) => "silent",
        Some(CharacterId::Defect) => "defect",
        Some(CharacterId::Necrobinder) => "necrobinder",
        Some(CharacterId::Regent) => "regent",
    }
}

/// Serialize a single [`Card`] runtime POD into the wire-format
/// `CardSpec` dict.
///
/// `state` is needed to resolve the card's *effective* energy cost
/// (base cost plus any active cost modifiers); the resolved value is
/// emitted as the additive `cost_override` field so display consumers
/// (e.g. the workbench combat panel) can show the real cost without
/// re-deriving it. `cost_override` is not a required CardSpec key, so
/// emitting it is backward-compatible and ignored on round-trip.
#[must_use]
pub fn card_to_value(state: &CombatState, card: &Card) -> Value {
    let mut d = obj();
    set(&mut d, "id", card.card_id.to_python_name());
    // `stable_idx` is wire-only metadata; we emit the uid as a
    // best-effort proxy since the Rust port doesn't carry a separate
    // stable index — the uid IS the stable handle within a combat.
    set(
        &mut d,
        "stable_idx",
        i32::try_from(card.uid.get()).unwrap_or(0),
    );
    let kind = match card.card_type {
        CardType::Status => "status",
        CardType::Curse => "curse",
        _ => "card",
    };
    set(&mut d, "kind", kind);
    set(&mut d, "upgraded", card.upgraded);
    set(
        &mut d,
        "cost_override",
        sts2_core::hooks::effective_card_cost(state, card.uid),
    );
    // The instance's own cost, separate from the effective one above, plus
    // the per-instance scratch that explains the difference. Emitting only
    // the effective cost made every transient discount permanent on rebuild;
    // see `ParsedCard::base_cost`.
    set(&mut d, "base_cost", i32::from(card.cost));
    if !card.combat_vars.is_empty() {
        let mut cv = obj();
        for (key, value) in &card.combat_vars {
            set(&mut cv, var_key_wire_name(*key), *value);
        }
        d.insert("combat_vars".to_owned(), Value::Object(cv));
    }
    // Base damage / block let display consumers fill the `{Damage}` /
    // `{Block}` template vars in card descriptions. Additive (not in
    // REQUIRED_CARDSPEC_KEYS); 0 for cards that deal/block neither.
    set(&mut d, "base_damage", card.base_damage);
    set(&mut d, "base_block", card.base_block);
    // The card face's runtime-calculated value (Ashen Strike's 6 becomes
    // 6 + 3 x <cards in the Exhaust Pile>), for the ~40 cards that state
    // one. Additive, and ABSENT for every other card: consumers must read a
    // missing key as "this card has no such number", never as 0 — `(Hits 0
    // times)` on a card face reads as a fact, and zero is never the truth
    // for these on a live board. Untargeted (`None`), which is what the
    // game itself shows until the player hovers an enemy.
    if let Some(v) = sts2_content::cards::preview::calculated_value(state, card, None) {
        set(&mut d, "calculated_value", v);
    }
    // Instance keyword flags, in the C# producers' vocabulary
    // (`VALID_CARD_FLAGS`), so the mirror's `_card_key` can compare them —
    // with neither side emitting flags, the diff was structurally BLIND to
    // every keyword divergence. ONLY the override/registry-derived subset is
    // emitted: the C# producer names flags only on HAND cards and never
    // names the free riders at all, so a var-derived emission (FreeThisTurn,
    // GhostSeed's EtherealThisCombat) is knowledge only the mirror's own
    // model holds — emitting it manufactured a per-decision tuple desync on
    // every generated-free card (the v29wr hand/discard explosion: mirror
    // `('free_turn',)` vs world `()`). The vars still DO their behavioral
    // work; they are just not part of the comparison vocabulary.
    //
    // The keyword `retain` is emitted in every pile (sts2-agent fork, A4d),
    // symmetric with the parse, which now folds it into the permanent
    // override: a granted Retain (Snap, Choices Paradox) round-trips, and an
    // engine-vs-C# Retain-registry divergence shows as a flag difference.
    //
    // The single-turn grants (`sly_turn`, `retain_turn`) are emitted only for
    // cards IN HAND, where they exist. Keyword `sly` (intrinsic, or
    // MasterPlanner's combat-long `AddKeyword`) is emitted in every pile: the
    // grant stays on the card wherever it goes (`MasterPlannerPower.cs`), and
    // the parse restores it from every pile, so a rebuild keeps a Sly Defend
    // Sly when it is drawn. GH #370.
    {
        let mut flags: Vec<&str> = Vec::new();
        // GH #372. Two independent sources make a card Ethereal and the emit
        // must union them, because the PARSE already does:
        //
        //   * `card_ethereal_inst` — the permanent keyword (per-instance
        //     `keyword_override`, else the static registry).
        //   * `VarKey::EtherealThisCombat` — the per-combat grant. GhostSeed
        //     stamps it on every basic Strike/Defend
        //     (`relics/shop_event.rs:1964`), and `state_from_dict` maps the
        //     wire's `ethereal` flag onto this same var (`:2290`).
        //
        // Emitting only the first made the round trip LOSSY in one direction:
        // a GhostSeed run's wire arrives with `flags: ["ethereal"]`, parses
        // into the combat var, and comes back out with no flag at all —
        // verified by round-tripping seed 1002's hand. Since `_card_key`
        // diffs the flag tuple across all five piles, that is a per-decision
        // desync on every basic Strike/Defend for the whole fight.
        //
        // Symmetric emit/parse is the invariant here; see the sibling note on
        // the free riders below.
        if sts2_core::card_effects::card_ethereal_inst(card)
            || card.combat_var(VarKey::EtherealThisCombat) == 1
        {
            flags.push("ethereal");
        }
        if sts2_core::card_effects::card_unplayable_inst(card) {
            flags.push("unplayable");
        }
        if sts2_core::card_effects::card_innate_inst(card) {
            flags.push("innate");
        }
        if sts2_core::card_effects::card_retain_inst(card) {
            flags.push("retain");
        }
        // Keyword first, then the single-turn halves with the keyword
        // subtracted, so a card that is BOTH intrinsically Sly and
        // Hand-Tricked emits `sly` alone — as C# does, since `IsSlyThisTurn`
        // short-circuits on the keyword.
        let sly_keyword = sts2_core::card_effects::card_sly(card.card_id)
            || state.player.sly_this_combat.contains(&card.uid);
        if sly_keyword {
            flags.push("sly");
        }
        if state.player.hand.contains(&card.uid) {
            if !sly_keyword && state.player.sly_this_turn.contains(&card.uid) {
                flags.push("sly_turn");
            }
            if !sts2_core::card_effects::card_retain_inst(card)
                && state.player.retain_this_turn.contains(&card.uid)
            {
                flags.push("retain_turn");
            }
        }
        if !flags.is_empty() {
            set(&mut d, "flags", flags);
        }
    }
    // Enchantments, in the C# producers' spelling so both dialects of this
    // field read the same. The engine allows a stack of several; the wire
    // shape (one `enchantment` + one `enchantment_amount`) is C#'s, which
    // holds at most one — emit the first and let the rest be lossy rather
    // than invent a shape no consumer parses.
    if let Some((id, amount)) = card.enchantments.first() {
        set(&mut d, "enchantment", id.wire_name());
        set(&mut d, "enchantment_amount", *amount);
    }
    // The card's own extra plays (Hidden Gem); an enchantment's are the
    // enchantment's. Absent when 0, as before the field.
    if card.base_replay_count != 0 {
        set(&mut d, "replay_count", card.base_replay_count);
    }
    // An X-cost card's energy captured at its last play (C#
    // `CardEnergyCost.CapturedXValue`, its shown cost outside the hand once
    // played; S8). A non-X card's stamped paid cost is a per-play transient,
    // not part of the card (`observational_energy_spent`). Absent when 0.
    let energy_spent = sts2_core::cards::observational_energy_spent(card);
    if energy_spent != 0 {
        set(&mut d, "energy_spent", energy_spent);
    }
    // Per-card affliction (Bound/Devoured/…): additive optional fields
    // so afflicted cards survive the round-trip — without them a
    // reconstructed ChainsOfBinding state believes every card playable.
    if let Some(aff) = card.affliction {
        set(
            &mut d,
            "affliction",
            card_affliction_kind_wire_name(aff.kind),
        );
        set(&mut d, "affliction_stacks", aff.stacks);
        if aff.applied_exhaust {
            set(&mut d, "affliction_applied_exhaust", true);
        }
    }
    Value::Object(d)
}

fn pile_to_value(
    state: &CombatState,
    uids: &[CardUid],
    cards: &[Card],
    path: &str,
) -> Result<Value, EmitError> {
    let mut list = Vec::with_capacity(uids.len());
    for uid in uids {
        let card = cards
            .iter()
            .find(|c| c.uid == *uid)
            .ok_or_else(|| EmitError::at(path, &format!("uid {} not in arena", uid.get())))?;
        list.push(card_to_value(state, card));
    }
    Ok(Value::Array(list))
}

/// Per-stream RNG read counts, keyed by the `RunRngType` snake_case names.
///
/// Emitted twice, deliberately from one definition: as the top-level
/// `rng_counters` *diagnostic* (a counter mismatch in a real-vs-real replay
/// names the exact stream and read-count where two engines desynced, which a
/// value-only field diff cannot show when the desync hides behind a constant),
/// and inside `$.rng.counters`, where it is *load-bearing* — that copy is what
/// `state_from_dict` restores the streams from, putting a reconstruction on
/// the same die roll the producer is about to make.
///
/// `unknown_map_point` and `treasure_room` are omitted: `state_from_dict`
/// accepts them, but no combat-scoped producer moves them, so shipping them on
/// every combat wire would be noise.
#[must_use]
pub fn run_rng_counters(state: &CombatState) -> Value {
    let rng = &state.run_rng;
    let mut d = obj();
    set(&mut d, "up_front", rng.up_front.counter());
    set(&mut d, "shuffle", rng.shuffle.counter());
    set(
        &mut d,
        "combat_card_generation",
        rng.combat_card_generation.counter(),
    );
    set(
        &mut d,
        "combat_potion_generation",
        rng.combat_potion_generation.counter(),
    );
    set(
        &mut d,
        "combat_card_selection",
        rng.combat_card_selection.counter(),
    );
    set(
        &mut d,
        "combat_energy_costs",
        rng.combat_energy_costs.counter(),
    );
    set(&mut d, "combat_targets", rng.combat_targets.counter());
    set(&mut d, "monster_ai", rng.monster_ai.counter());
    set(&mut d, "niche", rng.niche.counter());
    set(&mut d, "combat_orbs", rng.combat_orbs.counter());
    // PlayerRng-side streams (C# `PlayerRng.{Rewards,Shops,Transformations}`),
    // surfaced so the lockstep harness can diff the shared `rewards` stream
    // read-count per step — the exact tool for the shared-Rewards
    // roll-interleaving divergence class (act-2 potion drop / deck |0|1).
    set(&mut d, "rewards", rng.rewards.counter());
    set(&mut d, "shops", rng.shops.counter());
    set(&mut d, "transformations", rng.transformations.counter());
    Value::Object(d)
}

/// One [`sts2_core::hooks::DamageModifier`] row → `{name, kind, side, add?, mul?}`.
///
/// `name` is the engine registry id (`WEAK`, `STRENGTH`, `BURNING_BLOOD`) —
/// the same vocabulary [`powers_to_value`] emits, so a consumer resolves a
/// title and an icon for a modifier exactly as it does for a power. `side`
/// says whose listener it is (`"enemy"` = the attacker's, `"player"` = the
/// target's), which is what separates "the attacker's Strength" from "your
/// Vulnerable" in a sentence.
///
/// `add` / `mul` are present only for the channel this listener actually
/// moved, so a consumer can print `+3` or `×3/4` without deciding which of
/// three identity defaults to suppress. `mul` is the raw `[num, den]`
/// rational the engine composes — NOT a float: the pipeline floors exactly
/// once at the end, and a `0.75` in the payload invites a consumer to
/// reconstruct that with float multiplication and land a different number.
#[must_use]
pub fn damage_modifier_to_value(
    state: &CombatState,
    m: &sts2_core::hooks::DamageModifier,
) -> Value {
    use sts2_core::hooks::{CreatureRef, DamageModifierSource};
    let mut d = obj();
    let (kind, name) = match m.source {
        DamageModifierSource::Power(k) => ("power", k.python_name()),
        DamageModifierSource::Relic(k) => ("relic", k.python_name()),
        DamageModifierSource::Other => ("other", "OTHER"),
    };
    set(&mut d, "kind", kind);
    set(&mut d, "name", name);
    let side = match m.owner {
        Some(CreatureRef::Player) => Some("player"),
        Some(CreatureRef::Enemy(uid)) => {
            // The player's own pet is player-side for the reader's purposes
            // even though the arena stores it as a creature.
            if state.player.pet_uid == Some(uid) {
                Some("player")
            } else {
                Some("enemy")
            }
        }
        Some(CreatureRef::Ally(_)) => Some("player"),
        _ => None,
    };
    if let Some(side) = side {
        set(&mut d, "side", side);
    }
    if m.additive != 0 {
        set(&mut d, "add", m.additive);
    }
    if m.mul != sts2_core::hooks::DamageMul::ONE {
        // A LIST, not a tuple: this dict is consumed both in-process (workbench
        // tests, the hint adapter) and as JSON by the browser, and a tuple
        // makes those two disagree on the Python side of an `==`.
        set(&mut d, "mul", vec![m.mul.num, m.mul.den]);
    }
    if m.cap != i32::MAX {
        set(&mut d, "cap", m.cap);
    }
    Value::Object(d)
}

/// Serialize a single [`Intent`] into the wire-format intent dict:
/// `{"kind": PascalCase str, "amount": Option<i32>, "times": Option<i32>}`.
///
/// `kind` uses the same PascalCase vocabulary `parse_intent` accepts
/// (`Attack`/`Defend`/`Buff`/…) so the dict round-trips, rather than the
/// SCREAMING_SNAKE [`Intent::python_name`] (which targets a different,
/// bridge-replay exporter). `amount` carries attack damage / buff-debuff
/// magnitude; `times` carries the multi-attack hit count.
#[must_use]
pub fn intent_to_value(intent: Intent, state: &CombatState, dealer: CreatureUid) -> Value {
    let Value::Object(mut d) = bare_intent_to_value(intent) else {
        unreachable!("bare_intent_to_value always emits an object");
    };
    // `amount` is the RAW base from the monster's move table, because that is
    // what `Intent` stores and what `unbake_wire_intent` reconstructs on the
    // way back in — keeping the dict round-trip exact. But the *game* reports
    // the baked number (C# `AttackIntent.GetSingleDamage`), so a consumer that
    // compares a mirror wire against a real one would be diffing raw against
    // baked and flagging every Strength / Weak / Shrink / player-Vulnerable
    // board as a divergence. Emit the baked number alongside, computed by the
    // engine's own pipeline rather than a second implementation of it.
    //
    // Additive only: `parse_intent` reads keys by name and ignores unknown
    // ones, so `amount` remains the round-trip carrier and ingest is unchanged.
    // `DeathBlowIntent : SingleAttackIntent` in C#, so it reports a baked
    // number on the same code path as a plain attack.
    if let Intent::Attack { damage }
    | Intent::MultiAttack { damage, .. }
    | Intent::DeathBlow { damage } = intent
    {
        let effective = sts2_core::combat_loop::preview_damage_to_player(
            state,
            sts2_core::hooks::CreatureRef::Enemy(dealer),
            damage,
            sts2_core::hooks::ValueProps::MOVE,
            None,
        );
        set(&mut d, "effective_amount", effective);
        // …and WHICH listeners moved it. `effective_amount` alone is a number
        // with no story: bug report `wb-4eae3bad6ebd57f8` is a player who
        // applied Weak to an enemy already carrying it, saw the telegraph not
        // budge, and reported the engine as broken — Weak is a duration
        // counter (`WeakPower.StackType = Counter`), so the ×0.75 was already
        // baked into the number and nothing on the board said so.
        //
        // Display-only and additive: `parse_intent` reads keys by name and
        // ignores unknown ones, so this changes no round-trip. The rows come
        // from the engine's own per-listener entry points (see
        // `hooks::damage_modifier_breakdown`) rather than a UI-side
        // re-derivation of the damage pipeline.
        let mods = sts2_core::combat_loop::preview_damage_to_player_modifiers(
            state,
            sts2_core::hooks::CreatureRef::Enemy(dealer),
            sts2_core::hooks::ValueProps::MOVE,
        );
        if !mods.is_empty() {
            let rows: Vec<Value> = mods
                .iter()
                .map(|m| damage_modifier_to_value(state, m))
                .collect();
            d.insert("modifiers".to_owned(), Value::Array(rows));
        }
    }
    Value::Object(d)
}

/// The state-free half of an intent's wire shape — `{kind, amount?, times?}`,
/// exactly the keys `parse_intent` needs to rebuild it — with no board to bake
/// a telegraph against. [`intent_to_value`] adds the baked `effective_amount`
/// and its `modifiers` on top; the SimLab catalog (`sts2_rs.monster_moves`)
/// emits this bare form for moves that have not been dealt yet.
#[must_use]
pub fn bare_intent_to_value(intent: Intent) -> Value {
    let mut d = obj();
    let kind = match intent {
        Intent::Attack { .. } | Intent::MultiAttack { .. } => "Attack",
        Intent::Defend { .. } => "Defend",
        Intent::Buff { .. } => "Buff",
        Intent::Debuff { .. } => "Debuff",
        Intent::DebuffStrong => "DebuffStrong",
        Intent::Sleep => "Sleep",
        Intent::Stun => "Stun",
        Intent::StatusCard => "StatusCard",
        Intent::Summon => "Summon",
        Intent::Heal => "Heal",
        // Bead `sts2-rs-f99e7`: GasBomb EXPLODE / WaterfallGiant
        // AboutToBlow — C# `IntentType.DeathBlow`.
        Intent::DeathBlow { .. } => "DeathBlow",
        // Bead `sts2-rs-8f336`: FatGremlin FLEE — C# `IntentType.Escape`.
        Intent::Escape => "Escape",
        Intent::Unknown => "Unknown",
        // `Intent` is #[non_exhaustive]; future variants degrade to a
        // telegraphed-but-unspecified intent rather than failing to build.
        _ => "Unknown",
    };
    set(&mut d, "kind", kind);
    let amount = match intent {
        Intent::Attack { damage }
        | Intent::MultiAttack { damage, .. }
        | Intent::DeathBlow { damage } => Some(damage),
        Intent::Buff { amount, .. } | Intent::Debuff { amount, .. } => Some(amount),
        _ => None,
    };
    if let Some(a) = amount {
        set(&mut d, "amount", a);
    }
    if let Intent::MultiAttack { hits, .. } = intent {
        set(&mut d, "times", i32::from(hits));
    }
    Value::Object(d)
}

/// The `powers` list for one creature: `[{name, amount, display_amount,
/// counter}, …]` in `uids` order.
///
/// # Errors
///
/// A power uid with no entry in `arena` — the state failed its own
/// `assert_invariants` before we got here.
pub fn powers_to_value(uids: &[PowerUid], arena: &[Power], path: &str) -> Result<Value, EmitError> {
    let mut list = Vec::with_capacity(uids.len());
    for uid in uids {
        let pow = arena.iter().find(|p| p.id == uid.0).ok_or_else(|| {
            EmitError::at(path, &format!("power uid {} not in arena", uid.0.get()))
        })?;
        let mut d = obj();
        set(&mut d, "name", pow.kind.python_name());
        set(&mut d, "amount", pow.amount);
        // Player-facing value for Counter powers. Mirrors C# `PowerModel.
        // DisplayAmount`: most powers show `amount`, but Slow shows
        // `SlowAmount * 10` (the per-instance `counter`, bumped per card
        // played and reset at the owner's turn start).
        let display_amount = match pow.kind {
            PowerKind::Slow => pow.counter.saturating_mul(10),
            // GH #363 A1: C# `OrbitPower.DisplayAmount => 4 - energySpent % 4`
            // (`OrbitPower.cs:25`) — the energy still owed before the next
            // grant, in 1..=4 (a fresh Orbit shows 4, not 0). The engine's
            // `counter` is the cumulative energy spent, so this is the same
            // complement the reconstruction arm inverts. Emitting `amount`
            // here — the energy the power GRANTS — made the mirror's own wire
            // disagree with the world's on a field the diff compares.
            PowerKind::Orbit => 4 - pow.counter.rem_euclid(4),
            _ => pow.amount,
        };
        set(&mut d, "display_amount", display_amount);
        // The per-instance accumulator, verbatim. `amount` does not describe
        // it and `display_amount` only sometimes encodes it, so without this
        // key every reconstruction restarts CrimsonMantle's self-damage,
        // Slow's tally and Surrounded's facing at zero. See
        // `ParsedPower::counter`.
        set(&mut d, "counter", pow.counter);
        list.push(Value::Object(d));
    }
    Ok(Value::Array(list))
}

/// Build the wire-format dict for `state`. The dict round-trips through
/// [`crate::state_from_wire`] modulo the documented ignored fields (see the
/// module-level comment).
///
/// # Errors
///
/// A pile contains a uid not in `state.cards`, or a power uid not in
/// `state.powers` (i.e. the state failed its own `assert_invariants`
/// before we got here). The `PyO3` host raises this as `ValueError`.
pub fn combat_state_to_value(state: &CombatState) -> Result<Value, EmitError> {
    let mut d = obj();
    set(&mut d, "wire_version", WIRE_VERSION);
    // The engine signals combat termination via `is_over`/`player_won`
    // (the authoritative flags MCTS reads — lethal_detector.rs:214,
    // value_net.rs:1398), NOT by transitioning `phase` to
    // `CombatPhase::Resolved`, which is never assigned in the combat loop.
    // So a player death during the enemy phase (or a victory) leaves
    // `phase` stuck at `EnemyTurnExec`/`PlayerPlayPhase`, and emitting the
    // raw phase here would report "EnemyPhase" forever — wire/PyO3 callers
    // (incl. the C#/Rust parity harness) could never observe combat-over
    // (sts2-rs-8ukqd: 40/100 parity-fuzz trials diverged on `combat_terminal`
    // for exactly this — C# enters Defeat/Victory while Rust reported
    // "EnemyPhase"). Report the resolved phase off the authoritative flags.
    let phase_str = if state.is_over {
        if state.player_won {
            "Victory"
        } else {
            "Defeat"
        }
    } else {
        combat_phase_to_str(state.phase)
    };
    set(&mut d, "phase", phase_str);
    set(&mut d, "round", i32::from(state.round));
    set(&mut d, "side", side_to_str(state.side));
    set(&mut d, "is_elite", state.is_elite);
    // Wire schema uses `turn`, which mirrors `round` in this Rust port
    // (Python sim splits them; the wire field name predates the merge).
    set(&mut d, "turn", i32::from(state.round));
    set(&mut d, "combat_id", 0_i64);
    // Declare the intent dialect. We emit the engine's own pre-modifier
    // amounts; the C#/adapter producers emit UI-effective ones and are
    // inverted on ingest. Saying so explicitly is what keeps a
    // `state_to_dict` → `state_from_dict` round-trip from subtracting
    // Strength/Vulnerable that were never added. See
    // `ParsedState::intent_amounts`.
    set(&mut d, "intent_amounts", "base");

    // Per-stream RNG cursor (counter = number of public `next_*` reads so
    // far on that stream). Mirrors C# `RunRngSet.ToSerializable().Counters`
    // (each `Rng.Counter`), keyed by the `RunRngType` snake_case names so
    // the two dumps line up field-for-field. This is a *diagnostic*: in a
    // real-vs-real replay (parity_fuzz / full-run) a counter mismatch is
    // the exact stream + read-count where the engines desynced — which the
    // value-only field diff cannot show when the desync is masked by a
    // constant (e.g. a fixed-HP monster). Cheap to emit, no behavior change.
    d.insert("rng_counters".to_owned(), run_rng_counters(state));

    // --- player ---
    let mut player = obj();
    set(
        &mut player,
        "class",
        class_str_from_character(state.player.character),
    );
    // Wire format rejects max_hp <= 0; clamp to 1 so an empty default
    // state still survives parse. The state_equiv predicate ignores
    // this if both sides agree on the clamp.
    let emitted_max_hp = state.player.max_hp.max(1);
    set(&mut player, "max_hp", emitted_max_hp);
    let emitted_hp = state.player.current_hp.clamp(0, emitted_max_hp);
    set(&mut player, "hp", emitted_hp);
    set(&mut player, "block", state.player.block.max(0));
    set(&mut player, "energy", i32::from(state.player.energy).max(0));
    set(
        &mut player,
        "max_energy",
        i32::from(state.player.max_energy).max(0),
    );
    set(
        &mut player,
        "energy_regen_per_turn",
        i32::from(state.player.max_energy).max(0),
    );
    // Gold: gates SealOfGold's turn-start hook, so an engine -> dict ->
    // engine round-trip must not silently zero it.
    set(&mut player, "gold", state.gold);

    // Relics.
    let mut relics_list = Vec::with_capacity(state.relics.len());
    for r in &state.relics {
        let mut rd = obj();
        set(&mut rd, "id", r.kind.python_name());
        if r.counter != 0 {
            set(&mut rd, "counter", r.counter);
        }
        // ToyBox wax/melt state, emitted symmetrically with the parser
        // (and with the C# producer in `Sts2Headless/RunSimulator.cs`) so a
        // world-vs-mirror wire diff cannot false-diverge on a melted relic.
        // Omitted when false to keep every non-ToyBox wire byte-identical.
        if r.is_wax {
            set(&mut rd, "is_wax", true);
        }
        if r.is_melted {
            set(&mut rd, "is_melted", true);
        }
        relics_list.push(Value::Object(rd));
    }
    player.insert("relics".to_owned(), Value::Array(relics_list));

    // Player powers. The d03 player-Creature unification is mid-migration
    // (Phase 1): the player's powers are *split* across two surfaces.
    //   * Relic combat-start grants (BronzeScales Thorns, Vajra Strength,
    //     OddlySmoothStone Dexterity, …) are applied via
    //     `apply_power_to_creature(player_uid, …)` and live on the mirror
    //     Creature's `powers` (the listeners resolve the player through
    //     `state.player.creature_uid`).
    //   * Card self-power grants (Inflame Strength, Flex, Footwork, …) are
    //     applied via the per-character `apply_power_to_player` helpers,
    //     which still push onto the legacy `player.powers` SmallVec.
    // C# carries a single unified power list, so we must serialize the
    // *union* of both surfaces — Creature first (relics apply at combat
    // start, before any card play), then any legacy power not already
    // present, deduped by power-arena id. `init_player_creature` clones
    // `player.powers` into the Creature at combat start, so a power that
    // predates combat appears in both lists with the same id; dedup keeps
    // it once. (States built by `state_from_dict` leave `creature_uid`
    // None and keep all powers on the legacy field — the fallback below
    // serializes that single list unchanged.)
    let creature_powers = state
        .player
        .creature_uid
        .and_then(|uid| state.creatures.iter().find(|c| c.id == uid.0))
        .map(|c| c.powers.as_slice());
    let player_power_uids: Vec<PowerUid> = match creature_powers {
        Some(cp) => {
            let mut merged: Vec<PowerUid> = cp.to_vec();
            for uid in &state.player.powers {
                if !merged.iter().any(|u| u.0 == uid.0) {
                    merged.push(*uid);
                }
            }
            merged
        }
        None => state.player.powers.iter().copied().collect(),
    };
    player.insert(
        "powers".to_owned(),
        powers_to_value(&player_power_uids, &state.powers, "$.player.powers")?,
    );

    // Potions — a flat list of ids (None for empty slot). The Rust
    // PlayerCombatState carries only filled slots; emit each as a
    // string id.
    let potions_list: Vec<Value> = state
        .player
        .potions
        .iter()
        .map(|k| val(k.python_name()))
        .collect();
    player.insert("potions".to_owned(), Value::Array(potions_list));

    if state.player.stars != 0 {
        set(&mut player, "stars", i32::from(state.player.stars));
    }

    // Per-turn play counters — emitted only mid-turn (any bucket non-zero)
    // so start-of-turn dicts keep the historical shape. The parser side
    // documents why dropping these stalls round-tripped searches (Ringing/
    // Normality/Sloth ShouldPlay gates).
    if state.player.cards_played_this_turn.iter().any(|&n| n > 0) {
        set(
            &mut player,
            "cards_played_this_turn",
            state.player.cards_played_this_turn.to_vec(),
        );
    }
    // ChainsOfBinding's once-per-turn Bound gate — same rationale as the
    // buckets above: dropping it re-opens vetoed plays on round-trip.
    if state.combat_log.chains_bound_card_played_this_turn {
        set(&mut player, "bound_card_played_this_turn", true);
    }

    // Necrobinder's Osty pet. It lives in `state.creatures` (referenced by
    // `player.pet_uid`), deliberately *not* in `enemy_uids`, so the encounter
    // loop below skips it. Emit the `$.player.osty` shape the parser reads
    // (see `ParsedOsty`: alive/hp/max_hp/block) so downstream consumers — the
    // workbench Rust backend — can surface the companion the way the C#
    // backend already does. Name is not part of the wire shape; the consumer
    // knows the species is Osty.
    if let Some(pet_uid) = state.player.pet_uid {
        if let Some(c) = state.creatures.iter().find(|c| c.id == pet_uid.0) {
            let emitted_max_hp = c.max_hp.max(1);
            let mut osty = obj();
            set(&mut osty, "alive", c.current_hp > 0);
            set(&mut osty, "hp", c.current_hp.clamp(0, emitted_max_hp));
            set(&mut osty, "max_hp", emitted_max_hp);
            set(&mut osty, "block", c.block.max(0));
            player.insert("osty".to_owned(), Value::Object(osty));
        }
    }

    // Defect orbs. Mirror the C# `CombatPlayState` shape (RunSimulator.cs:
    // emits `orbs` + `orb_slots` whenever the belt has capacity) so the
    // workbench renders the orb belt — including its empty slots — for the
    // Rust backend too. `orbs` is the list of *channelled* (non-`Empty`)
    // orbs; `orb_slots` is the belt capacity the frontend pads with empties.
    // `passive`/`evoke` match C#'s `OrbModel.PassiveVal`/`EvokeVal`, which
    // fold in the player's Focus via `ModifyOrbValue` (Plasma excepted).
    // The Rust backend emits `<KIND>_ORB.title` loc keys (server.py resolves
    // them; the C# backend already ships a plain name).
    if state.player.orbs.capacity > 0 {
        let focus = sts2_core::orbs::focus_bonus(state);
        let mut orbs_list = Vec::new();
        let mut idx: i32 = 0;
        // Focus-scaled passive/evoke clamp at 0 (C# `FocusPower.ModifyOrbValue`
        // → `Math.Max(value + focus, 0)`); `focus_scaled` applies the same clamp
        // the engine effect sites use, so a negative-Focus orb serializes 0 (not
        // a negative passive) to match the C# parity surface.
        use sts2_core::orbs::focus_scaled;
        for orb in &state.player.orbs.slots {
            let (type_str, loc_key, passive, evoke) = match orb {
                Orb::Empty => continue,
                Orb::Lightning => (
                    "Lightning",
                    "LIGHTNING_ORB.title",
                    focus_scaled(3, focus),
                    focus_scaled(8, focus),
                ),
                Orb::Frost => (
                    "Frost",
                    "FROST_ORB.title",
                    focus_scaled(2, focus),
                    focus_scaled(5, focus),
                ),
                // Dark's passive shows the per-turn increment (clamped 6 + focus).
                // Its evoke is the C# `_evokeVal`, which STARTS at 6 and is grown
                // by the passive (`DarkOrb.cs:17,25,39`) — no second Focus add.
                // `stored` holds only the accumulated growth, so the base has to
                // go back on: emitting `stored` raw put every mirror-channelled
                // Dark orb 6 low on the wire (257 orb elements across the live
                // A1 Defect corpus), and the matching parse leg subtracted
                // nothing, so the two errors cancelled in any round-trip test.
                Orb::Dark { stored } => (
                    "Dark",
                    "DARK_ORB.title",
                    focus_scaled(6, focus),
                    stored.saturating_add(DARK_ORB_BASE_EVOKE),
                ),
                // Plasma is unaffected by Focus (C# literal 1m / 2m).
                Orb::Plasma => ("Plasma", "PLASMA_ORB.title", 1, 2),
                // Glass: passive = clamp(current_passive + focus); evoke =
                // passive * 2 (C# `GlassOrb.EvokeVal => PassiveVal * 2m`).
                Orb::Glass { current_passive } => {
                    let passive = focus_scaled(*current_passive, focus);
                    ("Glass", "GLASS_ORB.title", passive, passive * 2)
                }
            };
            let mut orb_dict = obj();
            set(&mut orb_dict, "index", idx);
            set(&mut orb_dict, "name", loc_key);
            set(&mut orb_dict, "type", type_str);
            set(&mut orb_dict, "passive", passive);
            set(&mut orb_dict, "evoke", evoke);
            orbs_list.push(Value::Object(orb_dict));
            idx += 1;
        }
        player.insert("orbs".to_owned(), Value::Array(orbs_list));
        set(
            &mut player,
            "orb_slots",
            i32::from(state.player.orbs.capacity),
        );
    }

    d.insert("player".to_owned(), Value::Object(player));

    // --- deck ---
    let mut deck = obj();
    deck.insert(
        "draw_pile".to_owned(),
        pile_to_value(state, &state.player.draw, &state.cards, "$.deck.draw_pile")?,
    );
    deck.insert(
        "discard_pile".to_owned(),
        pile_to_value(
            state,
            &state.player.discard,
            &state.cards,
            "$.deck.discard_pile",
        )?,
    );
    deck.insert(
        "hand".to_owned(),
        pile_to_value(state, &state.player.hand, &state.cards, "$.deck.hand")?,
    );
    deck.insert(
        "exhaust_pile".to_owned(),
        pile_to_value(
            state,
            &state.player.exhaust,
            &state.cards,
            "$.deck.exhaust_pile",
        )?,
    );
    // Cards in flight. Emitted only when non-empty, which is only ever
    // mid-resolution — most notably while a card-select screen the played
    // card opened is still up. Omitting it dropped that card on every
    // reconstruction: the choice then resolved with an empty play pile and
    // the card never reached the discard.
    if !state.player.play.is_empty() {
        deck.insert(
            "play_pile".to_owned(),
            pile_to_value(state, &state.player.play, &state.cards, "$.deck.play_pile")?,
        );
    }

    // master_deck: best-effort — emit as `None` when empty so the
    // receiver doesn't try to materialise zero entries.
    if !state.master_deck.is_empty() {
        let mut md = Vec::with_capacity(state.master_deck.len());
        for (i, cid) in state.master_deck.iter().enumerate() {
            let mut entry = obj();
            set(&mut entry, "id", cid.to_python_name());
            // master_deck has no per-card uid in the Rust port; emit a
            // synthetic stable_idx for schema validity.
            set(&mut entry, "stable_idx", i32::try_from(i).unwrap_or(0));
            set(&mut entry, "kind", "card");
            set(&mut entry, "upgraded", false);
            md.push(Value::Object(entry));
        }
        deck.insert("master_deck".to_owned(), Value::Array(md));
    }

    // Draw-pile knowledge, as indices into the `draw_pile` list emitted
    // above. Stale pins (uids no longer physically in `draw`) have no
    // index and are dropped — the same retain-sweep `cards::shuffle` /
    // `cards::pop_next_card` / `blindfold::canonicalize_unknown_middle`
    // perform before trusting either list, so dropping them here loses
    // nothing a consumer would have honored.
    let pin_indices = |pins: &[CardUid]| -> Vec<usize> {
        pins.iter()
            .filter_map(|uid| state.player.draw.iter().position(|d| d == uid))
            .collect()
    };
    let top_idx = pin_indices(&state.player.pinned_top);
    if !top_idx.is_empty() {
        set(&mut deck, "pinned_top", top_idx);
    }
    let bot_idx = pin_indices(&state.player.pinned_bottom);
    if !bot_idx.is_empty() {
        set(&mut deck, "pinned_bottom", bot_idx);
    }
    if state.player.deferred_draw_order {
        set(&mut deck, "deferred_draw_order", true);
    }

    d.insert("deck".to_owned(), Value::Object(deck));

    // --- encounter ---
    let mut enc = Vec::with_capacity(state.enemy_uids.len());
    for uid in &state.enemy_uids {
        let c = state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .ok_or_else(|| EmitError::at("$.encounter", "enemy uid not in creatures"))?;
        let mut m = obj();
        // Enemy uids only — `species` must be `Some(_)` here
        // (bead sts2-rs-a19m migrated off the player Cultist sentinel).
        let species = c
            .species
            .expect("enemy creature must carry a species (bead sts2-rs-a19m)");
        set(&mut m, "kind", species.python_name());
        set(&mut m, "uid", i64::from(c.id.get()));
        // Clamp shape to satisfy parse_state_dict's invariants.
        let m_max_hp = c.max_hp.max(1);
        set(&mut m, "max_hp", m_max_hp);
        set(&mut m, "hp", c.current_hp.clamp(0, m_max_hp));
        set(&mut m, "block", c.block.max(0));
        m.insert(
            "powers".to_owned(),
            powers_to_value(&c.powers, &state.powers, "$.encounter.powers")?,
        );
        // `intents` carries the creature's currently-telegraphed move so
        // display consumers (workbench combat panel) can render it. We emit a
        // 0-or-1 element list: `Creature.intent` is a single `Option<Intent>`.
        // `intent_history` stays empty: the parser only needs the current
        // intent, and Rust's `IntentHistory` shape is private and dropped on
        // the rebuild path (round-trip equiv ignores it).
        //
        // ── KNOWN GAP: multi-intent moves are truncated here ──────────────
        // C# `MoveState` takes `params AbstractIntent[]`
        // (`../sts2-wiki/decompiled/v0.107.1/MegaCrit.Sts2.Core.MonsterMoves.MonsterMoveStateMachine/MoveState.cs:48-53`)
        // and real movesets use it — e.g. `TerrorEel.cs:72`
        // `new MoveState("THRASH_MOVE", ThrashMove, new MultiAttackIntent(...),
        // new BuffIntent())`. The world therefore telegraphs `[Attack x3,
        // Buff]` where this emits `[Attack x3]`.
        //
        // The engine's DSL ALREADY models the list —
        // `monster_ai::MoveState.intents` is a `SmallVec<[Intent; 4]>` and 44
        // content moves declare two or three — but every consumer collapses it
        // to `intents.first()` (`combat_loop::perform_intent`, the intent
        // priming in `hooks`, `parse_enemy`). Widening the wire alone
        // would be half a fix: a 2026-08 census over the 12-seed live corpus
        // (`logs/diag/mirror_d1.jsonl`) found 153 world records with a
        // multi-intent telegraph, of which only 34 sit on a move this repo
        // declares with a matching intent count. The other 119 span ~26 moves
        // (Aeonglass EBB/INCREASING_INTENSITY, Stabbot STAB, every
        // DecimillipedeSegment BULK/CONSTRICT, Queen BURN_BRIGHT_FOR_ME,
        // MechaKnight WINDUP, Ovicopter TENDERIZER, Axebot BOOT_UP/
        // HAMMER_UPPERCUT, CubexConstruct REPEATER_BLAST, Nibbit SLICE,
        // GlobeHead SHOCKING_SLAP/GALVANIC_BURST, TheLost DEBILITATING_SMOG,
        // KnowledgeDemon PONDER, InfestedPrism PULSATE, TestSubject RESPAWN,
        // …) whose CONTENT declaration is itself short — so emitting the tail
        // today would trade a silent truncation for a loud divergence class
        // dominated by under-declared movesets, taint those sweep runs
        // (`STS2_MIRROR_OBSERVE`), and still not carry the real telegraph.
        //
        // The faithful fix is: (1) audit those ~26 movesets against their C#
        // `MoveState` ctors, (2) emit the whole `current_move().intents` list
        // when the primed `c.intent` is still its head, (3) teach
        // `parse_enemy` to keep the tail. Simulation is NOT affected by the
        // gap — the non-head halves are performed by each move's `effect_fn`,
        // not by the intent — so this is a telegraph-fidelity item, not a
        // combat-correctness one. See `wire_emits_only_the_primed_head_intent`
        // in `sts2-py/src/state_dict.rs`, which pins the current shape so a
        // change here is deliberate.
        let mut intents = Vec::new();
        if let Some(intent) = c.intent {
            intents.push(intent_to_value(intent, state, CreatureUid(c.id)));
        }
        m.insert("intents".to_owned(), Value::Array(intents));
        m.insert("intent_history".to_owned(), Value::Array(Vec::new()));
        // Debug-only: the AI's current move-state name (for lockstep
        // tracing vs C# `move_state`). `normalize_rust` builds its own
        // monster dict and ignores this key, so it never reaches the
        // parity diff. `None` when the creature is driven by the
        // species-`roll_intent` path instead of a `MonsterAi`.
        if let Some(ai) = &c.ai {
            set(&mut m, "move_state", ai.current_state_id());
            // Hidden AI counters the wire dialect discloses (2026-08-02):
            // emit them so a native round-trip keeps what the C# producers
            // now ship. The Fabricator's counter IS the last-spawned bot
            // code; the KnowledgeDemon's IS the curse index (0..3).
            if species == MonsterKind::Fabricator {
                let code = ai.counter();
                let kind = [
                    MonsterKind::Zapbot,
                    MonsterKind::Stabbot,
                    MonsterKind::Guardbot,
                    MonsterKind::Noisebot,
                ]
                .into_iter()
                .find(|&k| {
                    sts2_content::monsters::act3_glory::fabricator_bot_code(k) == code && code != 0
                });
                if let Some(kind) = kind {
                    set(&mut m, "fabricator_last_spawned", kind.python_name());
                }
            }
            if species == MonsterKind::KnowledgeDemon {
                set(&mut m, "curse_of_knowledge_counter", ai.counter());
            }
        }
        // C# `Creature.IsAlive` is `CurrentHp > 0`, so a 0-HP enemy
        // serializes as dead (the parity harness derives
        // `alive = !is_dead`). Deriving this from HP — rather than the
        // old hardcoded `false` — fixes the FOGMOG IllusionPower parity
        // bug (sts2-rs-vvi6g): the player-killed EyeWithTeeth used to
        // read as alive at 0 HP. `state_from_dict` keeps an `is_dead`
        // enemy as a corpse (a reviver's: Decimillipede segments, Eye with
        // Teeth), so a dumped corpse round-trips.
        set(&mut m, "is_dead", c.current_hp <= 0);
        set(&mut m, "is_escaped", false);
        // C# `RemoveCreature` parity (bead sts2-rs-c8why): a dead enemy
        // leaves the active `Enemies` list unless a power retains its
        // corpse (IllusionPower etc). We keep the creature in the stable
        // `enemy_uids` substrate (so NN slots don't reshuffle) and surface
        // this derived flag so the parity harness can compact the Rust
        // view down to C#'s `Enemies` shape. Alive enemies and retained
        // corpses are `false`.
        set(&mut m, "removed_after_death", state.is_removed_corpse(*uid));
        // Cards this creature is currently holding hostage via `SwipePower`
        // (`ThievingHopper`'s THIEVERY move). C# keeps the card ON the power
        // instance (`SwipePower.StolenCard`, surfaced as a hover tip and as
        // the card node pinned to the thief's `%StolenCardPos`); the Rust
        // model keeps one flat ledger on `CombatState`, so the identity has to
        // be re-attached to its thief here or a display consumer sees only a
        // bare "Swipe 1" and cannot say WHICH card left the deck.
        //
        // Emission-only, like `move_state` / `is_dead` above: `parse_enemy`
        // drops the key, and `state_from_dict` rebuilds with an empty
        // `swipe_stolen`. That is not a fidelity loss for search — the ledger
        // is read at `exit_combat` (deck removal) and on thief death, both
        // outside a hint-time combat rebuild.
        //
        // Reclaimed entries (thief died → `SwipePower.BeforeDeath` gave the
        // card back) are omitted: the card is the player's again, and the
        // thief that held it is a corpse.
        let mut stolen = Vec::new();
        for s in &state.swipe_stolen {
            if s.reclaimed || s.thief.0 != c.id {
                continue;
            }
            let mut sd = obj();
            set(&mut sd, "id", s.card_id.to_python_name());
            set(&mut sd, "upgraded", s.upgraded);
            stolen.push(Value::Object(sd));
        }
        if !stolen.is_empty() {
            m.insert("stolen_cards".to_owned(), Value::Array(stolen));
        }
        enc.push(Value::Object(m));
    }
    d.insert("encounter".to_owned(), Value::Array(enc));

    // --- rng / ascension ---
    let mut rng = obj();
    // We can't recover the original seed from the in-process RNG, so
    // emit `0` — the state_equiv predicate ignores this on round-trip
    // (rng cursor is in the documented-ignored list).
    set(&mut rng, "seed", 0_u64);
    set(&mut rng, "ascension", state.ascension);
    // The RUN's dice, unlike the search seed above, ARE recoverable and do
    // round-trip: the master seed every stream salts from, plus each stream's
    // read count. Emitting them makes a Rust-produced wire carry the same RNG
    // lineage the C# producers carry, so the native-engine oracle exercises
    // the identical restore path the DLL mirror uses.
    set(
        &mut rng,
        "run_seed",
        u32::from_ne_bytes(state.run_rng.seed.to_ne_bytes()),
    );
    rng.insert("counters".to_owned(), run_rng_counters(state));
    d.insert("rng".to_owned(), Value::Object(rng));

    Ok(Value::Object(d))
}

// ---------------------------------------------------------------------------
// Bridge-shape action / pending-choice dicts
// ---------------------------------------------------------------------------

/// Render an [`Action`] as a bridge-shape dict — the element shape of
/// [`legal_actions`].
///
/// Shape contract (mirrors the `sts2-rl-agent` rollout bridge):
///   * `{"action": "play", "card_index": i, "target_index": j | None}`
///   * `{"action": "end_turn"}`
///   * `{"action": "choose", "index": i, "selection": [..]}` — emitted iff a
///     `pending_choice` is open; in that case no other actions are legal.
///   * `{"action": "potion", "slot": i, "target_index": j | None}`
#[must_use]
pub fn combat_action_to_value(action: &Action) -> Value {
    let mut d = obj();
    match action {
        Action::PlayCard { hand_index, target } => {
            set(&mut d, "action", "play");
            set(&mut d, "card_index", *hand_index);
            set(&mut d, "target_index", *target);
        }
        Action::EndTurn => {
            set(&mut d, "action", "end_turn");
        }
        Action::UsePotion { slot, target } => {
            set(&mut d, "action", "potion");
            set(&mut d, "slot", *slot);
            set(&mut d, "target_index", *target);
        }
        Action::DrinkPotion {
            potion_slot,
            target,
        } => {
            set(&mut d, "action", "drink_potion");
            set(&mut d, "slot", *potion_slot);
            set(&mut d, "target_index", *target);
        }
        Action::Channel { orb, target } => {
            set(&mut d, "action", "channel");
            set(&mut d, "orb", format!("{orb:?}"));
            set(&mut d, "target_index", *target);
        }
        Action::Evoke { times } => {
            set(&mut d, "action", "evoke");
            set(&mut d, "times", *times);
        }
        Action::ResolveChoice { selection } => {
            set(&mut d, "action", "choose");
            // `index` — the leading chosen index (default 0 for the
            // empty/skip selection) — is kept for back-compat with
            // single-pick callers that read only `index`.
            set(&mut d, "index", selection.first().copied().unwrap_or(0u8));
            // `selection` — the FULL index list — lets callers drive a
            // multi-pick choice (HIDDEN_DAGGERS `DiscardFromHand`
            // min=max=2 emits `[0,1]`). Without it the harness could
            // only ever express a single discard, silently dropping the
            // second required card (bead
            // `sts2-rs-hd-multipick-discard-uhsk1`). It also disambiguates
            // the empty-selection SKIP (`selection == []`) from picking
            // option 0 — both collapse to `index == 0` above.
            let sel: Vec<u8> = selection.iter().copied().collect();
            set(&mut d, "selection", sel);
        }
    }
    Value::Object(d)
}

/// Enumerate the legal actions for `state` as a list of bridge-shape dicts
/// ([`combat_action_to_value`] per action). Empty list when the state is
/// terminal — MCTS callers treat this as the leaf signal.
#[must_use]
pub fn legal_actions(state: &CombatState) -> Value {
    Value::Array(
        sts2_core::legal_actions(state)
            .iter()
            .map(combat_action_to_value)
            .collect(),
    )
}

/// Where `uid` currently sits, as the `(pile, index)` pair
/// `PyCombatState::open_injected_choice` reads back — or `None` when the
/// card is in no pile at all (mid-flight during resolution, or already gone).
///
/// Search order matches the locator's own `match` arms. A card is in at most
/// one pile, so the order only decides which name a (bug-induced) duplicate
/// reports under.
#[must_use]
pub fn locate_card(state: &CombatState, uid: CardUid) -> Option<(&'static str, usize)> {
    let p = &state.player;
    // Each pile is a `SmallVec` with its own inline capacity, so the array
    // has to hold plain slices for the element types to unify.
    for (name, pile) in [
        ("hand", p.hand.as_slice()),
        ("draw", p.draw.as_slice()),
        ("discard", p.discard.as_slice()),
        ("exhaust", p.exhaust.as_slice()),
        ("play", p.play.as_slice()),
    ] {
        if let Some(i) = pile.iter().position(|c| *c == uid) {
            return Some((name, i));
        }
    }
    None
}

/// Build the per-option dict entry. The shape matches Python's
/// `combat_state_to_bridge_state` lines 307-311:
///
/// ```text
/// {'id': '<canonical name>', 'type': '<Attack|Skill|Power>', 'upgraded': bool}
/// ```
///
/// When the [`ChoiceOption`] has a `card_uid` we resolve the concrete
/// [`Card`] in `state.cards` to read the runtime `card_type` /
/// `upgraded` fields (so upgraded flags survive into the bridge). When
/// `card_uid` is `None` we fall back to the spec-level `card_type` via
/// the content catalog lookup, leaving `upgraded` false — this matches
/// the synthetic-options pattern used by `OrbSlot` / discovery / card
/// reward flavors at this pin.
#[must_use]
pub fn choice_option_to_value(option: &ChoiceOption, state: &CombatState) -> Value {
    let cards = state.cards.as_slice();
    let mut d = obj();
    set(&mut d, "id", option.card_id.to_python_name());

    // Where the option's card physically sits RIGHT NOW, as the
    // `pile`/`index` locator pair `open_injected_choice` reads back. Emitting
    // it makes this dict the exact inverse of that constructor, so a mirror
    // can re-open its own derived choice — and a driver can hold the live
    // option list to a concrete board position instead of a card name (which
    // duplicates make ambiguous). Current location, not `source_pile`: the
    // locator resolves against the live piles, and for an OPEN choice the two
    // agree anyway (nothing has moved the card yet). Absent for synthetic
    // options and for a card that has since left every pile.
    if let Some(uid) = option.card_uid {
        if let Some((pile, index)) = locate_card(state, uid) {
            set(&mut d, "pile", pile);
            set(&mut d, "index", index);
        }
    }

    // Resolve runtime card metadata via card_uid when present. Look up
    // by uid match in the arena (parallels `state.cards.iter().find` in
    // parity.rs:127 — the arena is small and this matches the
    // existing access pattern).
    let resolved = option
        .card_uid
        .and_then(|uid| cards.iter().find(|c| c.uid == uid));
    // When the option has no `card_uid` (synthetic options for orb
    // slot / enemy target / discovery before deck materialization),
    // we omit `type` since we can't determine it without resolving
    // through a card catalog — at this pin no production callers
    // build such options. `upgraded` falls back to `ChoiceOption.upgraded`.
    let (ctype_opt, upgraded) = match resolved {
        Some(card) => (Some(card.card_type), card.upgraded),
        None => (None, option.upgraded),
    };
    if let Some(ct) = ctype_opt {
        set(&mut d, "type", card_type_python_name(ct));
    }
    set(&mut d, "upgraded", upgraded);
    Value::Object(d)
}

/// Render a [`PendingChoice`] as the bridge-shape dict. Mirrors
/// `combat_state_to_bridge_state` (`bridge_replay.py:303-316`) with the
/// Rust-port addition of an explicit `kind` discriminant for non-
/// `CardSelect` flavors.
///
/// ```text
/// {
///   'type': 'card_select',          # always — matches Python bridge
///   'kind': 'put_back',             # Rust-only: ChoiceKind discriminant
///   'cards': [{'id': 'Headbutt', 'type': 'Attack', 'upgraded': False},
///             ...],
///   'min_select': 1,
///   'max_select': 1,
///   'can_skip': False,
/// }
/// ```
#[must_use]
pub fn pending_choice_to_value(pc: &PendingChoice, state: &CombatState) -> Value {
    let mut d = obj();
    // `type` is always `"card_select"` for byte-equality with the
    // Python bridge payload (Python has one PendingCardChoice class).
    set(&mut d, "type", "card_select");
    // `kind` is the Rust-side discriminant — see
    // [`choice_kind_python_name`] for the variant→string table.
    set(&mut d, "kind", choice_kind_python_name(pc.kind));
    let cards_list: Vec<Value> = pc
        .options
        .iter()
        .map(|o| choice_option_to_value(o, state))
        .collect();
    d.insert("cards".to_owned(), Value::Array(cards_list));
    set(&mut d, "min_select", pc.min_select);
    set(&mut d, "max_select", pc.max_select);
    set(&mut d, "can_skip", pc.can_skip);
    Value::Object(d)
}

/// The open pending-choice description ([`pending_choice_to_value`]), or
/// `None` when no choice is currently pending — `CombatState.pending_choice()`.
#[must_use]
pub fn pending_choice(state: &CombatState) -> Option<Value> {
    state
        .pending_choice
        .as_ref()
        .map(|pc| pending_choice_to_value(pc, state))
}

// ---------------------------------------------------------------------------
// Hand previews
// ---------------------------------------------------------------------------

/// A clone of `state` with `card_uid` moved from the hand to the play pile —
/// the board as the modifier chain sees it mid-dispatch — so play-gated
/// listeners fire when a preview reads the chain (GH #520).
///
/// Only the two piles move. In particular `cards_played_this_turn` is NOT
/// bumped: the real dispatch raises it only after the card body resolves
/// (`combat_loop.rs:3710`), which is why Lethality's "first Attack this turn"
/// gate reads `== 0`. Bumping it here would gate the boost back off.
#[must_use]
pub fn staged_for_play(state: &CombatState, card_uid: CardUid) -> CombatState {
    let mut staged = state.clone();
    if let Some(pos) = staged.player.hand.iter().position(|&u| u == card_uid) {
        staged.player.hand.remove(pos);
    }
    staged.player.play.push(card_uid);
    staged
}

/// Read-only damage preview for the live hand — `CombatState.card_damage_preview()`.
///
/// For each hand card with base damage, the damage it would deal to each
/// living enemy right now, with the FULL modifier pipeline folded in — the
/// player's Strength / Vigor / Weak, the target's Vulnerable / Intangible,
/// relic and card multipliers (C# `Hook.ModifyDamage`). Nothing is mutated;
/// this is a pure display aid so the workbench can show "9" on a Strike aimed
/// at a Vulnerable target instead of the base "6", matching the live number
/// the game UI shows.
///
/// The outer list is aligned with the hand (one entry per hand card, same
/// order as `state_to_dict(..)["deck"]["hand"]`). Each inner map maps an
/// enemy's FULL encounter-array index — the position in `enemy_uids` that
/// `state_to_dict` labels each enemy with, and the `target_index` the play
/// path expects — to the modified per-hit damage. Cards with no base damage
/// (skills/powers/0-damage) and dead enemy slots are omitted (empty map).
///
/// A typed return, not a `Value`: the Python surface keys the inner dict by
/// INT, which JSON cannot spell. `PyO3` converts the `usize` keys natively; a
/// JSON consumer sees them as strings.
///
/// Each card is scored against [`staged_for_play`] — the modifier
/// chain is read as if that card were mid-dispatch — so play-gated
/// listeners fire. Without it Lethality (`+Amount%` on the turn's first
/// powered Attack, gated on a non-empty Play pile) was silently skipped and
/// a Strike under Lethality 50 previewed its bare 6 instead of 9 (GH #520).
#[must_use]
pub fn card_damage_preview(state: &CombatState) -> Vec<BTreeMap<usize, i32>> {
    let mut out = Vec::with_capacity(state.player.hand.len());
    for card_uid in &state.player.hand {
        let mut entry = BTreeMap::new();
        if let Some(card) = state.cards.iter().find(|c| c.uid == *card_uid) {
            let base = card.base_damage;
            let card_id = card.card_id;
            if base > 0 {
                let staged = staged_for_play(state, *card_uid);
                for (idx, enemy_uid) in staged.enemy_uids.iter().enumerate() {
                    let alive = staged
                        .creatures
                        .iter()
                        .any(|c| c.id == enemy_uid.0 && c.current_hp > 0);
                    if !alive {
                        continue;
                    }
                    let dmg = sts2_core::damage::compute_modified_damage(
                        &staged,
                        sts2_core::hooks::CreatureRef::Player,
                        *enemy_uid,
                        base,
                        sts2_core::hooks::ValueProps::MOVE,
                        Some(card_id),
                    );
                    entry.insert(idx, dmg);
                }
            }
        }
        out.push(entry);
    }
    out
}

/// Read-only block preview for the live hand — the block half of
/// [`card_damage_preview`] (`CombatState.card_block_preview()`).
///
/// For each hand card with base block, returns the block it would actually
/// grant if played right now, with Dexterity / Frail / Fasten folded in via
/// [`sts2_core::card_effects::modify_player_block_move`] — the same
/// `Hook.ModifyBlock` stage `gain_block_player` runs. `None` for cards with
/// no base block, so the list is hand-aligned and a caller can skip a slot
/// without index arithmetic.
///
/// The workbench had no block counterpart to the damage preview, so a
/// Defend under Dexterity 5 rendered its printed "5 Block" while granting
/// 10 — the display gap that made Necrobinder's summon cards look like the
/// better mitigation (GH #520, bug report `wb-b5c453e74f1b80e3`).
#[must_use]
pub fn card_block_preview(state: &CombatState) -> Vec<Option<i32>> {
    let mut out = Vec::with_capacity(state.player.hand.len());
    for card_uid in &state.player.hand {
        let base = state
            .cards
            .iter()
            .find(|c| c.uid == *card_uid)
            .map_or(0, |c| c.base_block);
        if base > 0 {
            let staged = staged_for_play(state, *card_uid);
            out.push(Some(sts2_core::card_effects::modify_player_block_move(
                &staged, base,
            )));
        } else {
            out.push(None);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU32;

    fn keys(v: &Value) -> Vec<&str> {
        v.as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect()
    }

    #[test]
    fn default_state_emits_the_top_level_wire_shape() {
        let state = CombatState::default();
        let v = combat_state_to_value(&state).expect("default state serializes");
        assert_eq!(
            keys(&v),
            [
                "combat_id",
                "deck",
                "encounter",
                "intent_amounts",
                "is_elite",
                "phase",
                "player",
                "rng",
                "rng_counters",
                "round",
                "side",
                "turn",
                "wire_version",
            ]
        );
        assert_eq!(v["wire_version"], WIRE_VERSION);
        assert_eq!(v["intent_amounts"], "base");
        assert_eq!(v["combat_id"], 0);
        // An empty default state clamps to a parseable shape: `max_hp >= 1`
        // and the `None` character defaults to ironclad.
        assert_eq!(v["player"]["class"], "ironclad");
        assert_eq!(v["player"]["max_hp"], 1);
        assert_eq!(v["player"]["hp"], 0);
        assert_eq!(
            keys(&v["player"]),
            [
                "block",
                "class",
                "energy",
                "energy_regen_per_turn",
                "gold",
                "hp",
                "max_energy",
                "max_hp",
                "potions",
                "powers",
                "relics",
            ]
        );
        assert_eq!(
            keys(&v["deck"]),
            ["discard_pile", "draw_pile", "exhaust_pile", "hand"]
        );
        assert_eq!(v["encounter"], serde_json::json!([]));
        assert_eq!(
            keys(&v["rng"]),
            ["ascension", "counters", "run_seed", "seed"]
        );
        // The diagnostic copy and the load-bearing copy come from one
        // definition.
        assert_eq!(v["rng"]["counters"], v["rng_counters"]);
        // Omitted-when-default keys are OMITTED, not null.
        for absent in [
            "stars",
            "osty",
            "orbs",
            "orb_slots",
            "cards_played_this_turn",
        ] {
            assert!(
                v["player"].get(absent).is_none(),
                "{absent} should be absent"
            );
        }
    }

    #[test]
    fn default_state_round_trips_through_the_parser() {
        let state = CombatState::default();
        let v = combat_state_to_value(&state).expect("serializes");
        let parsed = crate::parse_json(&v).expect("the emitted dict must parse");
        assert_eq!(parsed.player.class, "ironclad");
        assert_eq!(parsed.wire_version, WIRE_VERSION);
    }

    #[test]
    fn default_state_has_no_legal_actions_or_choice_and_empty_previews() {
        let state = CombatState::default();
        // No hand → nothing to preview.
        assert!(card_damage_preview(&state).is_empty());
        assert!(card_block_preview(&state).is_empty());
        assert_eq!(pending_choice(&state), None);
        assert!(legal_actions(&state).is_array());
    }

    #[test]
    fn intent_dict_carries_amount_times_and_effective_amount() {
        let st = CombatState::default();
        let uid = CreatureUid(NonZeroU32::new(1).expect("non-zero"));
        let d = intent_to_value(Intent::Attack { damage: 12 }, &st, uid);
        assert_eq!(d["kind"], "Attack");
        assert_eq!(d["amount"], 12);
        assert!(d.get("times").is_none());
        assert_eq!(d["effective_amount"], 12);
        let m = intent_to_value(Intent::MultiAttack { damage: 5, hits: 3 }, &st, uid);
        assert_eq!(m["amount"], 5);
        assert_eq!(m["times"], 3);
        let s = intent_to_value(Intent::Sleep, &st, uid);
        assert_eq!(s, serde_json::json!({"kind": "Sleep"}));
    }

    #[test]
    fn combat_action_dicts_spell_none_targets_as_null() {
        let v = combat_action_to_value(&Action::PlayCard {
            hand_index: 1,
            target: None,
        });
        assert_eq!(
            v,
            serde_json::json!({"action": "play", "card_index": 1, "target_index": null})
        );
        let v = combat_action_to_value(&Action::ResolveChoice {
            selection: smallvec::SmallVec::from_slice(&[0, 1]),
        });
        assert_eq!(
            v,
            serde_json::json!({"action": "choose", "index": 0, "selection": [0, 1]})
        );
        let v = combat_action_to_value(&Action::Channel {
            orb: Orb::Lightning,
            target: None,
        });
        assert_eq!(v["orb"], "Lightning");
    }
}
