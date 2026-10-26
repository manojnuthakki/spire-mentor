//! `RunManager` accessor twins — one free function per Python method, named
//! after it.
//!
//! Each function reads a [`RunManager`] and returns exactly what the
//! `sts2_rs.RunManager` method of the same name returns: a
//! [`serde_json::Value`] where Python returned a dict / list of dicts, a
//! plain Rust type where Python returned a list of strings, a list of tuples,
//! a tuple-or-`None`, or a scalar (see the module doc on
//! [`super`](crate::emit) for why). `None` ⇔ `Value::Null` for the
//! `Value`-returning ones.
//!
//! Methods that MUTATE the run (`take_action`, `materialize_event`,
//! `force_event`, the `dump_*` oracles, `random_playout`,
//! `surrogate_resolve_combat`) are not here — the mutation stays with the
//! host; where such a method also serialized something, the serializer is
//! here (`event_option_infos` is what `materialize_event` sets up for).
//! `path_plan` is also absent: it is computed by `sts2-runai`, whose
//! tract-onnx / rayon graph must stay out of this crate (see `Cargo.toml`).

use serde_json::Value;

use sts2_core::act_selection::ActVariant;
use sts2_core::event_log::{EventLogRow, LogCreature};
use sts2_core::map::{ActMap, MapCoord};
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::{Phase, RunAction, RunManager};

use super::map_ext::{obj, set, val};
use super::names::{map_point_type_str, map_point_type_ui_str, room_type_str};
use super::run_action::run_action_to_value;

// ---------------------------------------------------------------------------
// Maps
// ---------------------------------------------------------------------------

/// Serialize an [`ActMap`] into the flat lowercase
/// `[{col,row,point_type,children}, …]` corpus-tooling schema (the shape
/// [`act_map`] and [`act_map_for`] both return). Factored out so the
/// arbitrary-act dump and the live-act dump cannot drift.
///
/// `children` entries are `[col, row]` pairs here; the Python surface
/// presents them as tuples (the `PyO3` host restores that after `pythonize`).
#[must_use]
pub fn serialize_act_map(act_map: &ActMap) -> Value {
    let mut list = Vec::new();
    // Iterate the live `grid` instead of the underlying `points` Vec,
    // since `MapPathPruning::remove_point` clears grid + edges but leaves
    // stale entries in `points`; exposing those ghosts contaminates path
    // enumeration in the parity tooling.
    for (coord, &idx) in &act_map.grid {
        let p = act_map.point(idx);
        let mut d = obj();
        set(&mut d, "col", coord.col);
        set(&mut d, "row", coord.row);
        set(&mut d, "point_type", map_point_type_str(p.point_type));
        let mut children = Vec::new();
        for &child_idx in &p.children {
            let cp = act_map.point(child_idx);
            let child_coord = cp.coord;
            let in_grid = act_map.grid.contains_key(&child_coord);
            let is_boss = Some(child_idx) == act_map.boss;
            if in_grid || is_boss {
                children.push(val([child_coord.col, child_coord.row]));
            }
        }
        d.insert("children".to_owned(), Value::Array(children));
        list.push(Value::Object(d));
    }
    // Append the boss meta-node, which lives outside `grid` but is what the
    // path walk terminates at.
    if let Some(boss_idx) = act_map.boss {
        let b = act_map.point(boss_idx);
        let mut d = obj();
        set(&mut d, "col", b.coord.col);
        set(&mut d, "row", b.coord.row);
        set(&mut d, "point_type", map_point_type_str(b.point_type));
        d.insert("children".to_owned(), Value::Array(Vec::new()));
        list.push(Value::Object(d));
    }
    Value::Array(list)
}

/// The act boss encounter when it is deterministic — i.e. the act's boss
/// pool has exactly one entry, so the identity is known before the boss
/// room is entered (the port otherwise picks the boss lazily on entry).
/// `act_index` is 1-based. Returns `None` for multi-boss acts and
/// out-of-range indices.
///
/// Act 1 is intentionally treated as multi-boss: the live `sts2-content`
/// encounter pool in `encounters/act1_overgrowth.rs` has three bosses
/// (Vantom, Ceremonial Beast, The Kin), even though the legacy
/// `sts2_core::encounters::act1_boss_pool()` still pins a single Vantom
/// entry from the P9.0 thin slice. Returning `None` here suppresses a
/// stale, often-wrong "Vantom" label in the workbench map header.
fn single_boss_for_act(act_index: u8) -> Option<&'static sts2_core::encounters::EncounterSpec> {
    let pool: &'static [sts2_core::encounters::EncounterSpec] = match act_index {
        2 => sts2_core::encounters::act2_boss_pool(),
        3 => sts2_core::encounters::act3_boss_pool(),
        _ => return None,
    };
    match pool {
        [only] => Some(only),
        _ => None,
    }
}

/// Dump the current act's `ActMap` as a list of point dicts
/// ([`serialize_act_map`]), or `Null` if the run has no current act map
/// (possible mid-run between acts). Used by R3.2 corpus parity tooling.
/// Bead `sts2-rs-5hsqp`.
#[must_use]
pub fn act_map(rm: &RunManager) -> Value {
    match rm.run_state.current_act_map.as_ref() {
        Some(act_map) => serialize_act_map(act_map),
        None => Value::Null,
    }
}

/// Generate the ActMap for an ARBITRARY 1-based act (1/2/3) on this
/// run's seed and serialize it in the same flat schema as
/// [`act_map`], WITHOUT advancing the run. The map graph is a
/// pure function of `(seed, act)` — C# seeds it `new Rng(seed,
/// $"act_{act}_map")` (`StandardActMap.CreateFor`), independent of
/// run progress — so acts 2/3 are fully determined at run start and
/// need no combat to reach. This is the Rust oracle for
/// `act23_map_parity.py`, the all-acts analogue of the Act-1-only
/// `run_setup_parity.py` (which can only read `current_act_map`).
///
/// Mirrors [`sts2_core::runstate::RunState::generate_map`] exactly —
/// per-act `num_rooms` (15/14/13), the `act_{act}_map` named RNG, the
/// `modify_generated_map` relic channel, and the A10 second-boss
/// attach on the final act — but builds the RNG as a LOCAL stream
/// (never touching `RunRngSet::map_rngs`, which `map_rng()` would
/// mutate) and returns the map instead of storing it, so it is safe
/// to call for any act in any order. Returns `Null` for an out-of-
/// range act. Intended for a throwaway `RunManager` used only to dump.
///
/// `&mut` only because the relic `modify_generated_map` channel is
/// dispatched on `&mut RunState`; the run's own state is not changed.
#[must_use]
pub fn act_map_for(rm: &mut RunManager, act: u8) -> Value {
    if act < 1 || usize::from(act) > rm.run_state.acts.len().max(3) {
        return Value::Null;
    }
    let num_rooms = rm
        .run_state
        .acts
        .get(usize::from(act - 1))
        .map_or(sts2_core::acts::DEFAULT_ACT_NUM_ROOMS, |a| {
            i32::from(a.num_rooms)
        });
    let ascension_level = i32::from(rm.run_state.ascension_level);
    // LOCAL per-act RNG (does not poison `map_rngs`); same salt name as
    // `RunRngSet::map_rng(act-1)` → `act_{act}_map`.
    let mut rng =
        sts2_core::dotnet_rng::Rng::with_name(rm.run_state.rng.seed, &format!("act_{act}_map"));
    let mut act_map = sts2_core::map_generator::generate_act_map(
        num_rooms,
        &mut rng,
        ascension_level,
        false,
        i32::from(act),
        None,
    );
    rm.run_state.modify_generated_map(&mut act_map, act);
    // A10 DoubleBoss: only the final act carries a second boss node
    // (no RNG); mirror `generate_map`'s `is_last_act()` gate.
    if rm.run_state.has_double_boss && usize::from(act) == rm.run_state.acts.len() {
        act_map.attach_second_boss();
    }
    serialize_act_map(&act_map)
}

fn coord_value(c: MapCoord) -> Value {
    let mut d = obj();
    set(&mut d, "col", c.col);
    set(&mut d, "row", c.row);
    Value::Object(d)
}

/// Dump the current act's map in the C# `full_map` schema the sts2-pycli
/// web UI consumes (`web/src/api.ts` `FullMap`, rendered by
/// `MapView.tsx`). Shape:
///
/// ```text
/// {
///   "rows": [[{ "col": int, "row": int, "type": str,
///              "children": [{"col": int, "row": int}, ...],
///              "visited": bool, "current": bool }, ...], ...],
///   "boss": { "col": int, "row": int, "type": "Boss" } | None,
///   "second_boss": { "col": int, "row": int, "type": "Boss",
///                    "id": str } | None,
///   "current_coord": { "col": int, "row": int } | None,
/// }
/// ```
///
/// The `boss` dict intentionally has NO `children` key (the boss is a
/// terminal node with no forward edges) — downstream code must not
/// expect one. `second_boss` is the A10 DoubleBoss node's IDENTITY
/// (it is a real, selectable `rows` entry as well); `None` below A10.
///
/// `rows` is indexed by row (row 0 = entrance, boss excluded — it is
/// surfaced under `boss`, matching C# `GetFullMapImpl`). `type` is
/// `PascalCase` to match the UI's `NODE` glyph table. Returns `Null` when
/// no act map is loaded (mid-act transition). Distinct from [`act_map`],
/// which keeps its flat lowercase shape for corpus parity tooling.
#[must_use]
pub fn full_map(rm: &RunManager) -> Value {
    let Some(act_map) = rm.run_state.current_act_map.as_ref() else {
        return Value::Null;
    };
    let current = rm.run_state.current_room.as_ref().and_then(|r| r.map_coord);
    let view = act_map.full_map_view(current, &rm.run_state.visited_map_coords);

    let mut rows = Vec::with_capacity(view.rows.len());
    for row in &view.rows {
        let mut row_list = Vec::with_capacity(row.len());
        for n in row {
            let mut nd = obj();
            set(&mut nd, "col", n.coord.col);
            set(&mut nd, "row", n.coord.row);
            set(&mut nd, "type", map_point_type_ui_str(n.point_type));
            let children: Vec<Value> = n.children.iter().map(|ch| coord_value(*ch)).collect();
            nd.insert("children".to_owned(), Value::Array(children));
            set(&mut nd, "visited", n.visited);
            set(&mut nd, "current", n.current);
            row_list.push(Value::Object(nd));
        }
        rows.push(Value::Array(row_list));
    }

    let mut out = obj();
    out.insert("rows".to_owned(), Value::Array(rows));
    match view.boss {
        Some(b) => {
            let mut bd = obj();
            set(&mut bd, "col", b.coord.col);
            set(&mut bd, "row", b.coord.row);
            set(&mut bd, "type", map_point_type_ui_str(b.point_type));
            // Surface the boss's identity for display. The content
            // side pre-rolls the boss at act start
            // (`pre_roll_act_boss`) and stashes it on
            // `RunState::pinned_act_boss`, so the boss known to the
            // map widget is the encounter the player will actually
            // fight. Falls back to `single_boss_for_act` (for acts
            // whose pool is still single-entry at the current pin)
            // so raw-core hosts without the content callbacks still
            // get a label when one is unambiguous. `kind` is the
            // lead monster's python name, which the Python side
            // maps to a display title via the same path as combat
            // enemies.
            // The C#-faithful boss is `up_front.NextItem(AllBossEncounters)`,
            // pre-generated at run start and stored on
            // `RunState::generated_rooms` (`run_room_gen`). Its
            // `UPPER_SNAKE` id matches the headless `BossEncounter.Id.Entry`
            // seed-for-seed, so it is the source of truth for the map label
            // (and the `run_parity.py --rng-parity` check). Bead
            // `sts2-rs-jenwp`. `kind` still comes from the content-side
            // pre-roll when present (display only).
            let gen_boss_id = rm
                .run_state
                .act_index
                .checked_sub(1)
                .and_then(|i| rm.run_state.generated_rooms.get(usize::from(i)))
                .map(|g| g.boss_id);
            if let Some(pin) = rm.run_state.pinned_act_boss.as_ref() {
                set(&mut bd, "kind", pin.lead_kind.python_name());
                set(&mut bd, "id", gen_boss_id.unwrap_or(pin.display_name));
            } else if let Some(id) = gen_boss_id {
                set(&mut bd, "id", id);
            } else if let Some(spec) = single_boss_for_act(rm.run_state.act_index) {
                if let Some(lead) = spec.members.first() {
                    set(&mut bd, "kind", lead.kind.python_name());
                    set(&mut bd, "id", spec.name);
                }
            }
            out.insert("boss".to_owned(), Value::Object(bd));
        }
        None => {
            out.insert("boss".to_owned(), Value::Null);
        }
    }
    // A10 DoubleBoss: the final act carries a SECOND boss node directly
    // above the first (`ActMap::attach_second_boss`), and the game fights
    // a DIFFERENT encounter there — C# `NBossMapPoint._Ready` picks
    // `Point == Map.SecondBossMapPoint ? Act.SecondBossEncounter :
    // Act.BossEncounter`, and `RoomSet.NextBossEncounter` resolves the
    // second visit to `second_boss_id`. The node itself stays an ordinary
    // `rows` entry (the PPO map featurizer reads it there — see
    // `mapfeat::nodes_from_full_map`); this meta key only carries its
    // IDENTITY, which rows cannot express. `None` below A10.
    match act_map.second_boss {
        Some(sb_idx) => {
            let p = act_map.point(sb_idx);
            let mut sd = obj();
            set(&mut sd, "col", p.coord.col);
            set(&mut sd, "row", p.coord.row);
            set(&mut sd, "type", map_point_type_ui_str(p.point_type));
            if let Some(id) = rm
                .run_state
                .act_index
                .checked_sub(1)
                .and_then(|i| rm.run_state.generated_rooms.get(usize::from(i)))
                .and_then(|g| g.second_boss_id)
            {
                set(&mut sd, "id", id);
            }
            out.insert("second_boss".to_owned(), Value::Object(sd));
        }
        None => {
            out.insert("second_boss".to_owned(), Value::Null);
        }
    }
    out.insert(
        "current_coord".to_owned(),
        view.current_coord.map_or(Value::Null, coord_value),
    );
    Value::Object(out)
}

/// `RunManager.map_coord()` — the current room's map node as `(col, row)`,
/// or `None`.
#[must_use]
pub fn map_coord(rm: &RunManager) -> Option<(i32, i32)> {
    rm.run_state
        .current_room
        .as_ref()
        .and_then(|r| r.map_coord)
        .map(|c| (c.col, c.row))
}

/// `RunManager.current_room()` — `{"room_type": str, "map_coord": [col, row]
/// | None}`, or `Null` outside a room.
///
/// `map_coord` is a `[col, row]` pair here; the Python surface presents it as
/// a tuple (the `PyO3` host restores that after `pythonize`).
#[must_use]
pub fn current_room(rm: &RunManager) -> Value {
    match rm.run_state.current_room {
        Some(r) => {
            let mut d = obj();
            set(&mut d, "room_type", room_type_str(r.room_type));
            d.insert(
                "map_coord".to_owned(),
                r.map_coord.map_or(Value::Null, |c| val([c.col, c.row])),
            );
            Value::Object(d)
        }
        None => Value::Null,
    }
}

// ---------------------------------------------------------------------------
// Scalars
// ---------------------------------------------------------------------------

/// `RunManager.act1` — which Act-1 model this run ACTUALLY walks (GH #544):
/// `"overgrowth"` or `"underdocks"`.
///
/// The RESOLVED act, not the `act1=` argument — construct with
/// `act1="roll"` and this reports which of the two the seed landed on.
/// Read it from here rather than re-deriving it from the seed: the run is
/// the authority, and a second derivation is a second thing that can be
/// wrong (`canonicalize_seed` alone is enough to make them disagree).
#[must_use]
pub const fn act1(rm: &RunManager) -> &'static str {
    match rm.run_state.act1 {
        ActVariant::Overgrowth => "overgrowth",
        ActVariant::Underdocks => "underdocks",
        ActVariant::Hive => "hive",
        ActVariant::Glory => "glory",
    }
}

/// `RunManager.run_seed` — the run's own hashed master seed, as `u32`: the
/// value every RNG stream salts from, and the one the wire spells
/// `$.rng.run_seed` (C# `RunRngSet.Seed` is a uint).
#[must_use]
pub fn run_seed(rm: &RunManager) -> u32 {
    u32::from_ne_bytes(rm.run_state.rng.seed.to_ne_bytes())
}

/// `RunManager.gold` — while a fight is ACTIVE, the combat-time gold mirror
/// so SealOfGold's per-turn spends are visible mid-combat (matching C#). The
/// combat slot is kept around through the post-combat reward screen
/// (`is_over == true`), where the run gold — already reconciled by
/// `exit_combat` and then credited with reward gold — is authoritative.
/// Bead `sts2-rs-RP17`.
#[must_use]
pub fn gold(rm: &RunManager) -> i32 {
    match rm.combat.as_ref() {
        Some(c) if !c.is_over => c.gold,
        _ => rm.run_state.gold,
    }
}

/// `RunManager.rng_counters()` — run-level RNG stream cursors (read counts
/// so far), for whole-run parity diagnostics (`run_parity.py`). Keyed to
/// line up with the C# `RunSimulator.RngCounters()` dump — except
/// `run_manager`, which is a Python-pin stream with no C# `RunRngType`
/// analog (its non-zero count vs C#'s advanced `up_front` is the run-RNG
/// divergence signal).
#[must_use]
pub fn rng_counters(rm: &RunManager) -> Value {
    let rng = &rm.run_state.rng;
    let mut d = obj();
    set(&mut d, "up_front", rng.up_front.counter());
    set(&mut d, "shuffle", rng.shuffle.counter());
    set(&mut d, "run_manager", rng.run_manager.counter());
    set(&mut d, "monster_ai", rng.monster_ai.counter());
    set(&mut d, "niche", rng.niche.counter());
    set(&mut d, "treasure_room_relics", rng.treasure_room.counter());
    // PlayerRng streams (diagnostic): rewards/shops/transformations cursors.
    set(&mut d, "rewards", rng.rewards.counter());
    set(&mut d, "shops", rng.shops.counter());
    set(&mut d, "transformations", rng.transformations.counter());
    Value::Object(d)
}

/// `RunManager.generated_rooms()` — per-act pre-generated room contents
/// (`run_room_gen::generate_all_rooms`): the C#-faithful `up_front`
/// GenerateRooms output. Each entry is `{"normal": [id, ...], "elite": [id,
/// ...], "boss": id, "second_boss": id | None, "events_shuffled": [...]}`
/// with `UPPER_SNAKE` encounter ids matching the headless `dump_rooms`
/// `Id.Entry`s. Ground truth for `run_parity.py --encounter-parity`. Bead
/// `sts2-rs-jenwp` (M3).
#[must_use]
pub fn generated_rooms(rm: &RunManager) -> Value {
    let mut list = Vec::with_capacity(rm.run_state.generated_rooms.len());
    for g in &rm.run_state.generated_rooms {
        let mut d = obj();
        set(&mut d, "normal", &g.normal_encounters);
        set(&mut d, "elite", &g.elite_encounters);
        set(&mut d, "boss", g.boss_id);
        // A10 DoubleBoss: the final act's second boss (None below A10).
        set(&mut d, "second_boss", g.second_boss_id);
        // M5 event-pool parity (bead `sts2-rs-1hiht`): the per-act
        // `RoomSet.events` shuffled order as `EventId::python_name`s.
        set(&mut d, "events_shuffled", &g.events_shuffled);
        // sts2-agent fork: the act's opening Ancient (`Neow`, `Orobas`, ...),
        // the event a host shows for the boss-relic choice.
        set(&mut d, "ancient", g.ancient.map(|a| format!("{a:?}")));
        list.push(Value::Object(d));
    }
    Value::Array(list)
}

// ---------------------------------------------------------------------------
// Deck / relics / potions
// ---------------------------------------------------------------------------

/// Indices into `run_state.master_deck` that are NOT currently stolen by an
/// in-progress `SwipePower` thief (`ThievingHopper`). Bead `sts2-rs-RP13`.
///
/// C# `SwipePower.Steal` calls `CardPileCmd.RemoveFromDeck` the instant the
/// card is stolen, so the deck the game serializes shrinks DURING combat
/// (and stays shrunk unless the thief dies, which restores it). The Rust
/// engine defers the real `master_deck` removal to `exit_combat`, so the
/// live `master_deck` still contains the stolen card mid-combat. The deck
/// VIEW ([`master_deck`] / [`master_deck_detailed`], consumed by the
/// workbench player serialization) must reflect C#'s mid-combat shrink, so
/// drop one `(card_id, upgraded)` match per un-reclaimed `swipe_stolen`
/// entry of the active combat. Out of combat this is the identity (no
/// active combat), and `exit_combat` has by then removed the cards for real.
#[must_use]
pub fn effective_master_deck_indices(rm: &RunManager) -> Vec<usize> {
    let mut keep: Vec<usize> = (0..rm.run_state.master_deck.len()).collect();
    let Some(combat) = rm.combat.as_ref() else {
        return keep;
    };
    for stolen in &combat.swipe_stolen {
        if stolen.reclaimed {
            continue;
        }
        if let Some(pos) = keep.iter().position(|&i| {
            let c = &rm.run_state.master_deck[i];
            c.card_id == stolen.card_id && c.upgraded == stolen.upgraded
        }) {
            keep.remove(pos);
        }
    }
    keep
}

/// One card outside combat, in the combat wire's card vocabulary
/// (sts2-agent fork): `id` (python name), `upgraded`, `base_cost`,
/// `base_damage`, `base_block`, `type` (title-cased; `Quest` for the game's
/// Quest cards), the instance's keyword
/// `flags` (`eternal`, `exhaust`, `ethereal`, `unplayable`, `innate`,
/// `retain`), its first enchantment, its affliction and its own
/// `replay_count`, each only when present. `eternal` is the removability gate
/// the removal and transform offers read (C# `CardModel.IsRemovable`).
#[must_use]
pub fn card_value(card: &sts2_core::cards::Card) -> Value {
    use sts2_core::card_effects as ce;
    let mut d = obj();
    set(&mut d, "id", card.card_id.to_python_name());
    set(&mut d, "upgraded", card.upgraded);
    set(&mut d, "base_cost", i32::from(card.cost));
    set(&mut d, "base_damage", card.base_damage);
    set(&mut d, "base_block", card.base_block);
    // The game's Quest cards carry a stand-in type in the engine (S9).
    let card_type = if card.card_id.is_quest() {
        "Quest"
    } else {
        super::names::card_type_python_name(card.card_type)
    };
    set(&mut d, "type", card_type);
    let mut flags: Vec<&str> = Vec::new();
    if sts2_core::lookup_card_removable().is_some_and(|removable| !removable(card)) {
        flags.push("eternal");
    }
    if ce::card_exhausts_inst(card) {
        flags.push("exhaust");
    }
    if ce::card_ethereal_inst(card) {
        flags.push("ethereal");
    }
    if ce::card_unplayable_inst(card) {
        flags.push("unplayable");
    }
    if ce::card_innate_inst(card) {
        flags.push("innate");
    }
    if ce::card_retain_inst(card) {
        flags.push("retain");
    }
    if !flags.is_empty() {
        set(&mut d, "flags", flags);
    }
    if let Some((id, amount)) = card.enchantments.first() {
        set(&mut d, "enchantment", id.wire_name());
        set(&mut d, "enchantment_amount", *amount);
    }
    if card.base_replay_count != 0 {
        set(&mut d, "replay_count", card.base_replay_count);
    }
    if let Some(aff) = card.affliction {
        set(
            &mut d,
            "affliction",
            crate::state_dict::card_affliction_kind_wire_name(aff.kind),
        );
        set(&mut d, "affliction_stacks", aff.stacks);
    }
    Value::Object(d)
}

/// `RunManager.master_deck_cards()` (sts2-agent fork) — the deck view of
/// [`master_deck`] (in-progress steals excluded) as [`card_value`] dicts,
/// each with its raw master-deck position as `deck_index`.
#[must_use]
pub fn master_deck_cards(rm: &RunManager) -> Value {
    Value::Array(
        effective_master_deck_indices(rm)
            .into_iter()
            .map(|i| {
                let mut v = card_value(&rm.run_state.master_deck[i]);
                if let Value::Object(d) = &mut v {
                    d.insert("deck_index".to_owned(), Value::from(i));
                }
                v
            })
            .collect(),
    )
}

/// `RunManager.master_deck()` — card ids in `PascalCase` `Debug` form. The
/// view excludes cards stolen by an in-progress SwipePower thief, so the
/// mid-combat deck matches C#'s immediate `RemoveFromDeck`. Bead
/// `sts2-rs-RP13`.
#[must_use]
pub fn master_deck(rm: &RunManager) -> Vec<String> {
    effective_master_deck_indices(rm)
        .into_iter()
        .map(|i| format!("{:?}", rm.run_state.master_deck[i].card_id))
        .collect()
}

/// `RunManager.master_deck_detailed()` — master deck with per-card upgrade
/// flag as `[(id_str, upgraded), …]`. Used by the R-Act1-Replay probe to
/// verify that on-obtain relic listeners like Whetstone actually applied
/// their upgrades, which the bare [`master_deck`] string formatter elides.
/// The view excludes in-progress SwipePower steals (see [`master_deck`]).
#[must_use]
pub fn master_deck_detailed(rm: &RunManager) -> Vec<(String, bool)> {
    effective_master_deck_indices(rm)
        .into_iter()
        .map(|i| {
            let c = &rm.run_state.master_deck[i];
            (format!("{:?}", c.card_id), c.upgraded)
        })
        .collect()
}

/// `RunManager.card_removal_options()` — `(card_id, upgraded)` for the
/// pending deck-removal offer, in C# `FromDeckForRemoval` order (removable
/// cards only, Curse-type sorted to the front). The workbench renders these
/// as the removal `card_select` options; the chosen index is a position in
/// THIS list, which `remove_deck_card` maps back to the raw master-deck
/// position. Mirrors `RunState::removal_offer_order`. Bead: deck-removal
/// curse-front parity.
#[must_use]
pub fn card_removal_options(rm: &RunManager) -> Vec<(String, bool)> {
    rm.run_state
        .removal_offer_order()
        .into_iter()
        .map(|i| {
            let c = &rm.run_state.master_deck[i];
            (format!("{:?}", c.card_id), c.upgraded)
        })
        .collect()
}

/// `RunManager.card_grant_options()` — `(PascalCase id, upgraded)` of the
/// FRONT pending card grant's generated options, in C# generation order —
/// the option pool a UI host renders and the index space `grant_deck_card`'s
/// `idx` selects. Empty when no grant is open. Bead `sts2-rs-gorge-select`.
#[must_use]
pub fn card_grant_options(rm: &RunManager) -> Vec<(String, bool)> {
    rm.run_state
        .pending_card_grants
        .first()
        .map(|g| {
            g.options
                .iter()
                .map(|c| (format!("{:?}", c.card_id), c.upgraded))
                .collect()
        })
        .unwrap_or_default()
}

/// `RunManager.relics()` — owned relics as the C# game-style
/// `RELIC.<UPPER_SNAKE>` identifiers — matches the format emitted by
/// scraped `.run` files at `../sts2_run_scraper/data/files/`. Bead
/// `sts2-rs-plrg2`.
#[must_use]
pub fn relics(rm: &RunManager) -> Vec<String> {
    rm.run_state
        .relics
        .iter()
        .map(|r| format!("RELIC.{}", r.to_python_name()))
        .collect()
}

/// `RunManager.run_relic_state()` (sts2-agent fork): the run-side relic
/// state a host shows as relic counters outside a fight and for relics whose
/// count isn't in a combat relic's `counter`, read-only. `counters`: every
/// `RunState.relic_counters` entry by the relic's python name (the value the
/// next fight seeds its combat relic with); `paels_wing_sacrificed`: card
/// rewards sacrificed to Pael's Wing (C# `PaelsWing.RewardsSacrificed`);
/// `paels_tooth_stored`: cards Pael's Tooth holds (C# `PaelsTooth`'s
/// serializable cards, its `DisplayAmount`).
#[must_use]
pub fn run_relic_state(rm: &RunManager) -> Value {
    let rs = &rm.run_state;
    let mut counters = obj();
    for (relic, value) in &rs.relic_counters {
        set(&mut counters, relic.to_python_name(), *value);
    }
    let mut d = obj();
    d.insert("counters".to_owned(), Value::Object(counters));
    set(&mut d, "paels_wing_sacrificed", rs.pael_sacrificed_count());
    set(&mut d, "paels_tooth_stored", rs.paels_tooth_stored.len());
    Value::Object(d)
}

/// `RunManager.potions()` — the potion belt as `PascalCase` `PotionKind`
/// debug names (e.g. `"FirePotion"`). The workbench's `display_maps.loc_id`
/// normalizes these to the SCREAMING_SNAKE loc-key / art stem. Empty when
/// the belt is empty.
///
/// During combat the live belt is the combat-side
/// `CombatState::player.potions` (seeded from the run belt at
/// `enter_combat`, written back at `exit_combat`), so a potion drunk
/// mid-fight disappears from this list immediately — without it, the
/// UI would keep showing a spent potion until combat ended. Outside
/// combat the run-level belt is authoritative. Bead `sts2-rs-01o`.
///
/// The combat belt is only authoritative while combat is actually
/// LIVE ([`Phase::Combat`]). After a non-boss fight, `rm.combat`
/// is NOT torn down — it lingers (with `is_over == true`) through the
/// card-reward / map / event phases until the next `enter_combat`
/// replaces it. Reading that stale combat belt during those phases
/// would shadow `run_state.potions`, hiding any potion the combat-end
/// reward stream just dropped (it lands on the run belt, then seeds
/// the next combat). So gate on the phase, not merely `combat.is_some()`.
#[must_use]
pub fn potions(rm: &RunManager) -> Vec<String> {
    if rm.phase() == Phase::Combat {
        if let Some(combat) = rm.combat.as_ref() {
            return combat
                .player
                .potions
                .iter()
                .map(|k| format!("{k:?}"))
                .collect();
        }
    }
    rm.run_state
        .potions
        .iter()
        .map(|k| format!("{k:?}"))
        .collect()
}

/// `RunManager.relic_reward_ids()` — relic ids (`PascalCase` `Debug` form,
/// e.g. `"LostWisp"`) currently offered as relic rewards (head of the
/// `relic_rewards` queue first), so a UI host can name the relic on the
/// reward screen instead of a bare "Take relic". Empty when no relic reward
/// is pending.
#[must_use]
pub fn relic_reward_ids(rm: &RunManager) -> Vec<String> {
    rm.run_state
        .relic_rewards
        .iter()
        .rev()
        .map(|r| format!("{:?}", r.relic_id))
        .collect()
}

/// `RunManager.boss_relic_options()` — relic ids (`PascalCase` `Debug`
/// form, e.g. `"SneckoEye"`) offered on the post-boss boss-relic-choice
/// screen, in the order a `PickBossRelic{idx}` action indexes them. Empty
/// when no `BossRelic` phase is active.
#[must_use]
pub fn boss_relic_options(rm: &RunManager) -> Vec<String> {
    rm.boss_relic_options()
        .iter()
        .map(|r| format!("{r:?}"))
        .collect()
}

/// `RunManager.rest_site_options()` — `(option_id, enabled)` for every
/// rest-site option the site offers, in the order a `RestOption{idx}`
/// action indexes them. `option_id` is the game's own
/// `RestSiteOption.OptionId` (`"HEAL"` / `"SMITH"` / `"KINDLE"` / ...), so a
/// UI host can look up the real plaque art + `rest_site_ui` loc row instead
/// of numbering the buttons. Empty outside `Phase::RestSite`.
///
/// The offered set is dynamic (Shovel appends Dig, Girya Lift,
/// PumpkinCandle Kindle, MeatCleaver Cook, a ByrdonisEgg Hatch), so
/// an index-keyed label table in the host is wrong the moment a
/// relic adds a third option — which is what shipped a bare
/// "Option 2" plaque for Kindle (`wb-500a91f0d29256f6`).
#[must_use]
pub fn rest_site_options(rm: &RunManager) -> Vec<(String, bool)> {
    if rm.phase() != Phase::RestSite {
        return Vec::new();
    }
    rm.run_state
        .rest_site_options()
        .iter()
        .map(|o| (o.option_id().to_string(), o.enabled()))
        .collect()
}

/// `RunManager.smith_options()` — card ids (`PascalCase` `Debug` form) of
/// the upgradeable cards offered by an open Smith rest-site choice, in the
/// order a `SmithCard{idx}` action indexes them. Empty when no `CardSelect`
/// pending choice is open.
#[must_use]
pub fn smith_options(rm: &RunManager) -> Vec<String> {
    rm.run_state
        .pending_choice
        .as_ref()
        .filter(|pc| pc.kind == sts2_core::ChoiceKind::CardSelect)
        .map(|pc| {
            pc.options
                .iter()
                .map(|opt| format!("{:?}", opt.card_id))
                .collect()
        })
        .unwrap_or_default()
}

/// `RunManager.neow_offers()` — 3-tuple of `SCREAMING_SNAKE_CASE` relic ids
/// `(positive_1, positive_2, cursed)` rolled for the run-start Neow event,
/// or `None` if the run was not started at Neow. Bead `sts2-rs-k8z6m`.
#[must_use]
pub fn neow_offers(rm: &RunManager) -> Option<(String, String, String)> {
    let o = rm.run_state.neow_offers?;
    Some((
        o.positive_1.snake_case_id().to_string(),
        o.positive_2.snake_case_id().to_string(),
        o.cursed.snake_case_id().to_string(),
    ))
}

// ---------------------------------------------------------------------------
// Card rewards
// ---------------------------------------------------------------------------

/// `RunManager.card_reward_options()` — the `PickCardReward{Some(idx)}`
/// actions currently available, as run-action dicts. `card_reward_options`
/// is private on `RunManager`; the only public path is through
/// `get_available_actions()`, which surfaces them as `PickCardReward`
/// variants. Callers wanting names use [`card_reward_option_ids`].
#[must_use]
pub fn card_reward_options(rm: &RunManager) -> Value {
    Value::Array(
        rm.get_available_actions()
            .iter()
            .filter(|a| matches!(a, RunAction::PickCardReward { idx: Some(_) }))
            .map(run_action_to_value)
            .collect(),
    )
}

/// `RunManager.card_reward_option_ids()` — currently-offered card-reward
/// ids as `["CARD.<UPPER_SNAKE>", …]` strings — matches the format emitted
/// by scraped `.run` files. Empty when no card-reward fan-out is pending.
/// Index aligns with `PickCardReward{Some(idx)}`. Bead `sts2-rs-ze744`.
#[must_use]
pub fn card_reward_option_ids(rm: &RunManager) -> Vec<String> {
    rm.card_reward_options()
        .iter()
        .map(|cid| format!("CARD.{}", cid.to_python_name()))
        .collect()
}

/// `RunManager.card_reward_option_upgraded()` — per-option upgrade flags,
/// index-aligned with [`card_reward_option_ids`]. Shorter than the ids list
/// (or empty) means "not upgraded": bundle and choose-screen offerings carry
/// no upgrade state, per `RunManager::card_reward_upgraded_flags` (GH #545).
#[must_use]
pub fn card_reward_option_upgraded(rm: &RunManager) -> Vec<bool> {
    rm.card_reward_upgraded_flags().to_vec()
}

/// `RunManager.card_reward_bundles()` — when the current `CardReward`
/// offering is a Scroll Boxes bundle pick, the packs as
/// `[["CARD.<UPPER_SNAKE>", …], …]`, one inner list per selectable pack,
/// index-aligned with `PickCardReward{Some(i)}`. `None` for an ordinary
/// single-card reward.
#[must_use]
pub fn card_reward_bundles(rm: &RunManager) -> Option<Vec<Vec<String>>> {
    rm.card_reward_bundles().map(|packs| {
        packs
            .iter()
            .map(|pack| {
                pack.iter()
                    .map(|cid| format!("CARD.{}", cid.to_python_name()))
                    .collect()
            })
            .collect()
    })
}

// ---------------------------------------------------------------------------
// Shop / events
// ---------------------------------------------------------------------------

/// `RunManager.shop_inventory()` — the live merchant inventory while in
/// `Phase::Shop`, or `Null`.
///
/// Surfaces concrete ids + prices so a UI host can render names/costs
/// (the `shop_buy_*` actions carry only an index). Card entries combine
/// the character row then the colorless row, so `idx` matches the
/// `shop_buy_card` index space. `available` is `true` for in-stock,
/// payload-bearing slots (sold-out slots have a sentinel price).
#[must_use]
pub fn shop_inventory(rm: &RunManager) -> Value {
    let Some(inv) = rm.shop_inventory() else {
        return Value::Null;
    };
    let avail = sts2_core::shop::is_shop_entry_available;
    // C# `MerchantEntry.Cost` folds `Hook.ModifyMerchantPrice`
    // (Courier / MembershipCard) over the rolled base while in the
    // shop, so the displayed cost is discounted for the owner.
    let relics = rm.run_state.active_relics();
    let price_of = |base: i32| sts2_core::shop::merchant_price_with_modifiers(&relics, base);
    let mut d = obj();

    let mut cards = Vec::new();
    for (idx, e) in inv
        .cards
        .iter()
        .chain(inv.colorless_cards.iter())
        .enumerate()
    {
        let mut cd = obj();
        set(&mut cd, "idx", idx);
        if let Some(c) = &e.card {
            set(&mut cd, "id", format!("{:?}", c.card_id));
        }
        set(&mut cd, "price", price_of(e.price));
        set(&mut cd, "on_sale", e.on_sale);
        set(&mut cd, "is_colorless", e.is_colorless);
        set(&mut cd, "available", avail(e.price) && e.card.is_some());
        cards.push(Value::Object(cd));
    }
    d.insert("cards".to_owned(), Value::Array(cards));

    let mut relic_rows = Vec::new();
    for (idx, e) in inv.relics.iter().enumerate() {
        let mut rd = obj();
        set(&mut rd, "idx", idx);
        if let Some(r) = e.relic_id {
            set(&mut rd, "id", format!("RELIC.{}", r.to_python_name()));
        }
        set(&mut rd, "price", price_of(e.price));
        set(&mut rd, "available", avail(e.price) && e.relic_id.is_some());
        relic_rows.push(Value::Object(rd));
    }
    d.insert("relics".to_owned(), Value::Array(relic_rows));

    let mut potion_rows = Vec::new();
    for (idx, e) in inv.potions.iter().enumerate() {
        let mut pd = obj();
        set(&mut pd, "idx", idx);
        if let Some(k) = e.potion_kind {
            set(&mut pd, "id", format!("{k:?}"));
        }
        set(&mut pd, "price", price_of(e.price));
        set(
            &mut pd,
            "available",
            avail(e.price) && e.potion_kind.is_some(),
        );
        potion_rows.push(Value::Object(pd));
    }
    d.insert("potions".to_owned(), Value::Array(potion_rows));

    set(&mut d, "removal_cost", price_of(inv.removal_cost));
    set(&mut d, "removal_used", inv.removal_used);
    Value::Object(d)
}

/// `RunManager.event_options()` — active-event option list as
/// `[(option_id, label), …]`, or `None` when no event is currently
/// materialized (i.e. `RunState::current_event` is `None`). Mirrors
/// `sts2_content::events::dispatch::present`. Bead `sts2-rs-h9qn3` (W4):
/// lets drivers map scraped `TextKey` strings
/// (`<EVENT_ID>.pages.INITIAL.options.<OPT>.title`) to the engine option
/// index by lower-casing the trailing path segment and looking it up
/// against the returned `option_id` column.
#[must_use]
pub fn event_options(rm: &RunManager) -> Option<Vec<(String, String)>> {
    sts2_content::events::dispatch::present(&rm.run_state).map(|opts| {
        opts.into_iter()
            .map(|opt| (opt.option_id.to_string(), opt.label))
            .collect()
    })
}

fn event_vars_value(vars: &[(&'static str, sts2_core::EventVar)]) -> Value {
    let mut d = obj();
    for (name, value) in vars {
        // int stays int: the template DSL's `:plural:` form
        // compares the value to 1, so a count handed over as
        // text takes the plural branch every time.
        match value {
            sts2_core::EventVar::Int(v) => set(&mut d, name, v),
            sts2_core::EventVar::Text(v) => set(&mut d, name, v),
        }
    }
    Value::Object(d)
}

/// `RunManager.event_option_infos()` — rich active-event option list as
/// `[{"option_id", "label", "description", "enabled", "loc_key", "vars",
/// "title_vars", …}, …]`, or `Null` when no event is materialized. Unlike
/// [`event_options`] (which returns only `(option_id, label)` tuples), this
/// surfaces the per-choice long-form `description` (e.g. "Take 5 damage,
/// gain a colorless card.") and the `enabled` gate so a UI can render the
/// full choice screen the C# game shows on arrival.
///
/// Also carries `loc_key` + `vars`: the game's own localization path
/// for the choice and the values for its `{VarName}` placeholders. A
/// display layer should PREFER those — `label` / `description` are
/// developer shorthand written for logs, and read as such next to the
/// real game ("Gain a random potion." vs "Procure 1 random Potion.").
///
/// The host's `materialize_event` (a mutation: it binds the room's event
/// when none is bound yet) is what makes this return the real choices on
/// arrival; the serialization itself is read-only and lives here.
#[must_use]
pub fn event_option_infos(rm: &RunManager) -> Value {
    let Some(opts) = sts2_content::events::dispatch::present(&rm.run_state) else {
        return Value::Null;
    };
    let mut list = Vec::with_capacity(opts.len());
    for (idx, opt) in opts.into_iter().enumerate() {
        let mut d = obj();
        set(&mut d, "option_id", opt.option_id);
        set(&mut d, "label", &opt.label);
        set(&mut d, "description", &opt.description);
        set(&mut d, "enabled", opt.enabled);
        // Localization path + template values, so a UI can render
        // the GAME's wording ("Procure 1 random Potion.") instead
        // of `label`/`description`, which are developer shorthand
        // ("Gain a random potion."). Empty `loc_key` = this choice
        // has no row in `localization_eng/events.json` (Ancient /
        // Neow relic offers, per-run-chosen rows); render the
        // shorthand then. See `EventOptionInfo::loc_key`.
        set(&mut d, "loc_key", opt.loc_key);
        d.insert("vars".to_owned(), event_vars_value(&opt.vars));
        // Title-only overrides. Empty for every option but The
        // Future of Potions, whose `{Rarity}` means the POTION's
        // rarity in its title and the CARD's in its description.
        d.insert("title_vars".to_owned(), event_vars_value(&opt.title_vars));
        // Structured effect payload (GH #237): surface the
        // engine's own gold/HP deltas so the PPO encoder can
        // featurize the option WITHOUT regex-scraping the
        // English description (a lossy round-trip that goes
        // silently dark on phrasing/version churn). `None`
        // (an option that can't be previewed) omits the keys
        // and the encoder falls back to its description parse.
        if let Some(delta) =
            sts2_content::events::dispatch::preview_option_effect(&rm.run_state, idx)
        {
            set(&mut d, "gold_delta", delta.gold_delta);
            set(&mut d, "hp_delta", delta.hp_delta);
            set(&mut d, "max_hp_delta", delta.max_hp_delta);
            // Gained/lost pairs, not net counts: an option that
            // TRADES (Relic Trader, a deck transform) nets to zero
            // and would featurize as doing nothing at all.
            set(&mut d, "relics_gained", delta.relics_gained);
            set(&mut d, "relics_lost", delta.relics_lost);
            set(&mut d, "potions_gained", delta.potions_gained);
            set(&mut d, "potions_lost", delta.potions_lost);
            set(&mut d, "cards_gained", delta.cards_gained);
            set(&mut d, "cards_lost", delta.cards_lost);
        }
        list.push(Value::Object(d));
    }
    Value::Array(list)
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// `RunManager.reward_screen()` (sts2-agent fork) — the open rewards screen
/// (`RunState::interactive_rewards`) as `[{"type": "gold", "amount"} |
/// {"type": "potion", "id": "POTION.<UPPER_SNAKE>"} | {"type": "relic", "id":
/// "RELIC.<UPPER_SNAKE>"} | {"type": "card", "options": [{"id":
/// "CARD.<UPPER_SNAKE>", "upgraded"}, …]}, …]`, index-aligned with
/// `claim_reward`'s `idx`; `Null` when no screen is open. A card reward's
/// options are listed for the host's own bookkeeping: the game shows them only
/// once the reward is claimed.
#[must_use]
pub fn reward_screen(rm: &RunManager) -> Value {
    use sts2_core::run_manager::ScreenReward;
    let Some(screen) = rm.reward_screen() else {
        return Value::Null;
    };
    Value::Array(
        screen
            .iter()
            .map(|r| {
                let mut d = obj();
                match r {
                    ScreenReward::Gold { amount } => {
                        set(&mut d, "type", "gold");
                        set(&mut d, "amount", *amount);
                    }
                    ScreenReward::Potion { kind } => {
                        set(&mut d, "type", "potion");
                        set(&mut d, "id", format!("POTION.{}", kind.python_name()));
                    }
                    ScreenReward::Relic { relic } => {
                        set(&mut d, "type", "relic");
                        set(
                            &mut d,
                            "id",
                            format!("RELIC.{}", relic.relic_id.to_python_name()),
                        );
                    }
                    ScreenReward::Card { options, upgraded } => {
                        set(&mut d, "type", "card");
                        let opts: Vec<Value> = options
                            .iter()
                            .enumerate()
                            .map(|(i, cid)| {
                                let mut o = obj();
                                set(&mut o, "id", format!("CARD.{}", cid.to_python_name()));
                                set(
                                    &mut o,
                                    "upgraded",
                                    upgraded.get(i).copied().unwrap_or(false),
                                );
                                Value::Object(o)
                            })
                            .collect();
                        d.insert("options".to_owned(), Value::Array(opts));
                    }
                }
                Value::Object(d)
            })
            .collect(),
    )
}

/// `RunManager.available_actions()` — every legal [`RunAction`] as a
/// run-action dict ([`run_action_to_value`]).
#[must_use]
pub fn available_actions(rm: &RunManager) -> Value {
    Value::Array(
        rm.get_available_actions()
            .iter()
            .map(run_action_to_value)
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Combat logs
// ---------------------------------------------------------------------------

/// `RunManager.card_play_log()` — the current combat's ordered card
/// play-log as `[{"id": str, "upgraded": bool, "energy_spent": int,
/// "auto_played": bool, "target": int | None}, …]`. Empty list outside
/// combat. NOT part of `state_to_dict` and NOT a parity surface.
#[must_use]
pub fn card_play_log(rm: &RunManager) -> Value {
    let mut list = Vec::new();
    if let Some(c) = rm.combat.as_ref() {
        for e in &c.combat_log.card_play_log {
            let mut d = obj();
            set(&mut d, "id", e.card_id.to_python_name());
            set(&mut d, "upgraded", e.upgraded);
            set(&mut d, "energy_spent", e.energy_spent);
            set(&mut d, "auto_played", e.auto_played);
            set(&mut d, "target", e.target.map(i32::from));
            list.push(Value::Object(d));
        }
    }
    Value::Array(list)
}

/// Serialize one [`LogCreature`] as
/// `{"kind": "player"|"monster", "id": str, "uid": int|None}`.
///
/// `id` is the id C# prints (`IRONCLAD`, `Nibbit`), never a display name: the
/// workbench owns localization and needs a stable key to look up.
#[must_use]
pub fn log_creature_to_value(who: LogCreature) -> Value {
    let mut d = obj();
    match who {
        LogCreature::Player(_) => {
            set(&mut d, "kind", "player");
            set(&mut d, "id", who.id_str());
            d.insert("uid".to_owned(), Value::Null);
        }
        LogCreature::Monster { uid, .. } => {
            set(&mut d, "kind", "monster");
            set(&mut d, "id", who.id_str());
            set(&mut d, "uid", uid);
        }
    }
    Value::Object(d)
}

/// Serialize one event-log row. See [`combat_log`].
#[must_use]
pub fn event_row_to_value(row: &EventLogRow, character: Option<CharacterId>) -> Value {
    use sts2_core::event_log::{EventLogEntry as E, LogSource};

    let mut d = obj();
    set(&mut d, "round", row.round);
    set(
        &mut d,
        "side",
        match row.side {
            sts2_core::Side::Player => "player",
            sts2_core::Side::Enemy => "enemy",
        },
    );
    set(&mut d, "text", row.entry.description(character));
    match row.source {
        Some(LogSource::Card { card, upgraded }) => {
            let mut sd = obj();
            set(&mut sd, "kind", "card");
            set(&mut sd, "id", card.to_python_name());
            set(&mut sd, "upgraded", upgraded);
            d.insert("source".to_owned(), Value::Object(sd));
        }
        Some(LogSource::Move { monster, move_id }) => {
            let mut sd = obj();
            set(&mut sd, "kind", "move");
            set(&mut sd, "monster", monster.0);
            set(&mut sd, "move_id", move_id);
            d.insert("source".to_owned(), Value::Object(sd));
        }
        None => {
            d.insert("source".to_owned(), Value::Null);
        }
    }

    let opt_creature = |who: Option<LogCreature>| who.map_or(Value::Null, log_creature_to_value);

    match row.entry {
        E::CardPlayStarted {
            card,
            upgraded,
            energy_spent,
            auto_played,
            target,
        } => {
            set(&mut d, "kind", "card_play_started");
            set(&mut d, "card", card.to_python_name());
            set(&mut d, "upgraded", upgraded);
            set(&mut d, "energy_spent", energy_spent);
            set(&mut d, "auto_played", auto_played);
            set(&mut d, "target", target);
        }
        E::CardPlayFinished { card, upgraded } => {
            set(&mut d, "kind", "card_play_finished");
            set(&mut d, "card", card.to_python_name());
            set(&mut d, "upgraded", upgraded);
        }
        E::MonsterMove { monster, move_id } => {
            set(&mut d, "kind", "monster_move");
            d.insert("monster".to_owned(), log_creature_to_value(monster));
            set(&mut d, "move_id", move_id);
        }
        E::DamageReceived {
            dealer,
            receiver,
            unblocked,
            blocked,
            hp_before,
            hp_after,
        } => {
            set(&mut d, "kind", "damage");
            d.insert("dealer".to_owned(), opt_creature(dealer));
            d.insert("receiver".to_owned(), log_creature_to_value(receiver));
            set(&mut d, "unblocked", unblocked);
            set(&mut d, "blocked", blocked);
            set(&mut d, "hp_before", hp_before);
            set(&mut d, "hp_after", hp_after);
        }
        E::BlockGained { receiver, amount } => {
            set(&mut d, "kind", "block");
            d.insert("receiver".to_owned(), log_creature_to_value(receiver));
            set(&mut d, "amount", amount);
        }
        E::PowerReceived {
            applier,
            receiver,
            power,
            amount,
        } => {
            set(&mut d, "kind", "power");
            d.insert("applier".to_owned(), opt_creature(applier));
            d.insert("receiver".to_owned(), log_creature_to_value(receiver));
            set(&mut d, "power", power.python_name());
            set(&mut d, "amount", amount);
        }
        E::PowerFlashed { creature, power } => {
            // Not a C# CombatHistory row -- a VIEW signal the workbench turns
            // into `power_flash_vfx`. See `EventLogEntry::PowerFlashed`.
            set(&mut d, "kind", "power_flash");
            d.insert("creature".to_owned(), log_creature_to_value(creature));
            set(&mut d, "power", power.python_name());
        }
        E::CardDrawn {
            card,
            upgraded,
            from_hand_draw,
        } => {
            set(&mut d, "kind", "card_drawn");
            set(&mut d, "card", card.to_python_name());
            set(&mut d, "upgraded", upgraded);
            set(&mut d, "from_hand_draw", from_hand_draw);
        }
        E::CardGenerated {
            card,
            upgraded,
            pile,
            by_player,
        } => {
            set(&mut d, "kind", "card_generated");
            set(&mut d, "card", card.to_python_name());
            set(&mut d, "upgraded", upgraded);
            set(&mut d, "pile", pile);
            set(&mut d, "by_player", by_player);
        }
        E::CardDiscarded { card, upgraded } => {
            set(&mut d, "kind", "card_discarded");
            set(&mut d, "card", card.to_python_name());
            set(&mut d, "upgraded", upgraded);
        }
        E::CardExhausted { card, upgraded } => {
            set(&mut d, "kind", "card_exhausted");
            set(&mut d, "card", card.to_python_name());
            set(&mut d, "upgraded", upgraded);
        }
        E::OrbChanneled { orb } => {
            set(&mut d, "kind", "orb_channeled");
            set(&mut d, "orb", sts2_core::event_log::orb_entry(orb));
        }
        E::OrbEvoked { orb } => {
            set(&mut d, "kind", "orb_evoked");
            set(&mut d, "orb", sts2_core::event_log::orb_entry(orb));
        }
        E::PotionUsed { potion, target } => {
            set(&mut d, "kind", "potion");
            set(&mut d, "potion", potion.python_name());
            set(&mut d, "target", target);
        }
        E::EnergySpent { amount } => {
            set(&mut d, "kind", "energy_spent");
            set(&mut d, "amount", amount);
        }
    }
    Value::Object(d)
}

/// `RunManager.combat_log()` — the current combat's ordered event log,
/// oldest first.
///
/// Each row is `{"kind": str, "round": int, "side": "player"|"enemy",
/// "text": str, "source": {...} | None, ...kind-specific fields}`. `text`
/// is the C# `CombatHistoryEntry.Description` verbatim (ids, not display
/// names) — useful for debugging and for diffing against the world-side
/// `[HISTTRACE]` stream; the workbench renders its own localized line from
/// the structured fields instead.
///
/// Empty outside combat, and empty unless recording was switched on
/// (`set_record_combat_log`). NOT part of `state_to_dict` and NOT a parity
/// surface.
#[must_use]
pub fn combat_log(rm: &RunManager) -> Value {
    let Some(c) = rm.combat.as_ref() else {
        return Value::Array(Vec::new());
    };
    let character = c.player.character;
    Value::Array(
        c.combat_log
            .event_log
            .rows()
            .iter()
            .map(|row| event_row_to_value(row, character))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sts2_content::run_callbacks::{initialize_run_setup, new_run_manager_for_character};

    /// A run built the way the `PyO3` constructor builds one for
    /// `RunManager(seed=0, character="Ironclad")`: the character factory,
    /// the default Overgrowth Act 1, then the C#-faithful run setup.
    fn ironclad_seed_0() -> RunManager {
        let mut rm = new_run_manager_for_character(0, CharacterId::Ironclad)
            .with_act1(ActVariant::Overgrowth);
        initialize_run_setup(&mut rm);
        rm
    }

    fn keys(v: &Value) -> Vec<&str> {
        v.as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect()
    }

    fn fresh_card(id: sts2_core::cards::CardId) -> sts2_core::cards::Card {
        sts2_content::relics::dispatch::ensure_installed();
        sts2_content::cards::dispatch::ensure_installed();
        sts2_core::lookup_card_factory().expect("factory")(id, false).expect("card")
    }

    fn flags(v: &Value) -> Vec<&str> {
        v.get("flags")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    }

    /// The game shows Ascender's Bane's Eternal (`AscendersBane.cs`
    /// `CanonicalKeywords`); the card dict names it, read from the same
    /// removability gate the removal and transform offers use (S19).
    #[test]
    fn card_value_names_eternal() {
        use sts2_core::cards::CardId;
        let bane = card_value(&fresh_card(CardId::AscendersBane));
        assert_eq!(flags(&bane), ["eternal", "ethereal", "unplayable"]);
        let strike = card_value(&fresh_card(CardId::StrikeIronclad));
        assert!(!flags(&strike).contains(&"eternal"));
    }

    /// The game's Quest cards (C# `CardType.Quest`: Byrdonis Egg, Lantern
    /// Key, Spoils Map) show that type; the engine's card types have no
    /// Quest, so the card dict names it (S9).
    #[test]
    fn card_value_names_a_quest_cards_type() {
        use sts2_core::cards::CardId;
        for id in [CardId::SpoilsMap, CardId::ByrdonisEgg, CardId::LanternKey] {
            assert_eq!(card_value(&fresh_card(id))["type"], "Quest", "{id:?}");
        }
        assert_eq!(
            card_value(&fresh_card(CardId::StrikeIronclad))["type"],
            "Attack"
        );
    }

    /// The run-side relic state the Python API couldn't read (S2's rest):
    /// a carried counter, Pael's Wing's sacrifices, Pael's Tooth's cards.
    #[test]
    fn run_relic_state_reads_counters_and_pael_trackers() {
        use sts2_core::RelicId;
        let mut rm = ironclad_seed_0();
        let empty = run_relic_state(&rm);
        assert_eq!(empty["paels_wing_sacrificed"], 0);
        assert_eq!(empty["paels_tooth_stored"], 0);
        rm.run_state.relic_counters.insert(RelicId::SwordOfStone, 3);
        let _ = rm.run_state.pael_record_sacrifice();
        let _ = rm.run_state.pael_record_sacrifice();
        let strike = rm.run_state.master_deck[0].clone();
        rm.run_state.paels_tooth_stored.push(strike);
        let v = run_relic_state(&rm);
        assert_eq!(v["counters"]["SWORD_OF_STONE"], 3);
        assert_eq!(v["paels_wing_sacrificed"], 2);
        assert_eq!(v["paels_tooth_stored"], 1);
    }

    #[test]
    fn full_map_has_the_ui_schema() {
        let rm = ironclad_seed_0();
        let v = full_map(&rm);
        assert_eq!(keys(&v), ["boss", "current_coord", "rows", "second_boss"]);
        assert!(!v["rows"].as_array().unwrap().is_empty());
        let node = &v["rows"][0][0];
        assert_eq!(
            keys(node),
            ["children", "col", "current", "row", "type", "visited"]
        );
        // The boss dict has no `children` key, and carries the generated id.
        let boss = &v["boss"];
        assert!(boss.get("children").is_none());
        assert_eq!(boss["type"], "Boss");
        assert!(boss["id"].is_string(), "{boss}");
        assert_eq!(v["second_boss"], Value::Null, "no A10 second boss at A0");
        // At run start the player stands nowhere yet.
        assert_eq!(v["current_coord"], Value::Null);
        assert_eq!(map_coord(&rm), None);
        assert_eq!(current_room(&rm), Value::Null);
    }

    #[test]
    fn act_map_flat_schema_and_arbitrary_act_dump_agree_on_act_1() {
        let mut rm = ironclad_seed_0();
        let live = act_map(&rm);
        let list = live.as_array().expect("list");
        assert!(!list.is_empty());
        assert_eq!(keys(&list[0]), ["children", "col", "point_type", "row"]);
        // Children are `[col, row]` pairs (tuples on the Python side).
        for p in list {
            for ch in p["children"].as_array().unwrap() {
                assert_eq!(ch.as_array().unwrap().len(), 2);
            }
        }
        // The last entry is the boss meta-node with no children.
        let last = list.last().unwrap();
        assert_eq!(last["point_type"], "boss");
        assert_eq!(last["children"], serde_json::json!([]));
        // The arbitrary-act dump for act 1 is the live map: same seed, same
        // salt, same relic channel.
        assert_eq!(act_map_for(&mut rm, 1), live);
        assert_eq!(act_map_for(&mut rm, 0), Value::Null);
        assert_eq!(act_map_for(&mut rm, 9), Value::Null);
    }

    #[test]
    fn map_choice_phase_surfaces_move_actions_and_nothing_else() {
        let rm = ironclad_seed_0();
        assert_eq!(rm.phase(), Phase::MapChoice);
        let actions = available_actions(&rm);
        let list = actions.as_array().unwrap();
        assert!(!list.is_empty());
        for a in list {
            assert_eq!(a["action"], "move");
            assert!(a["col"].is_number());
        }
        assert_eq!(shop_inventory(&rm), Value::Null);
        assert_eq!(event_options(&rm), None);
        assert_eq!(event_option_infos(&rm), Value::Null);
        assert_eq!(card_reward_options(&rm), serde_json::json!([]));
        assert!(card_reward_option_ids(&rm).is_empty());
        assert!(card_reward_option_upgraded(&rm).is_empty());
        assert_eq!(card_reward_bundles(&rm), None);
        assert!(relic_reward_ids(&rm).is_empty());
        assert!(boss_relic_options(&rm).is_empty());
        assert!(rest_site_options(&rm).is_empty());
        assert!(smith_options(&rm).is_empty());
        assert!(card_grant_options(&rm).is_empty());
        assert_eq!(neow_offers(&rm), None);
        assert_eq!(card_play_log(&rm), serde_json::json!([]));
        assert_eq!(combat_log(&rm), serde_json::json!([]));
    }

    #[test]
    fn deck_relics_potions_and_scalars_read_the_starter_run() {
        let rm = ironclad_seed_0();
        let deck = master_deck(&rm);
        assert!(deck.iter().any(|c| c == "StrikeIronclad"), "{deck:?}");
        let detailed = master_deck_detailed(&rm);
        assert_eq!(detailed.len(), deck.len());
        assert!(detailed.iter().all(|(_, upgraded)| !upgraded));
        assert_eq!(
            effective_master_deck_indices(&rm),
            (0..deck.len()).collect::<Vec<_>>()
        );
        // Removal offers every removable card in deck order (no curses yet).
        assert_eq!(card_removal_options(&rm).len(), deck.len());
        let relics = relics(&rm);
        assert!(relics.iter().all(|r| r.starts_with("RELIC.")), "{relics:?}");
        assert!(potions(&rm).is_empty());
        assert_eq!(act1(&rm), "overgrowth");
        assert_eq!(gold(&rm), rm.run_state.gold);
        assert_eq!(
            run_seed(&rm),
            u32::from_ne_bytes(rm.run_state.rng.seed.to_ne_bytes())
        );
        assert_eq!(
            keys(&rng_counters(&rm)),
            [
                "monster_ai",
                "niche",
                "rewards",
                "run_manager",
                "shops",
                "shuffle",
                "transformations",
                "treasure_room_relics",
                "up_front",
            ]
        );
        let rooms = generated_rooms(&rm);
        let rooms = rooms.as_array().unwrap();
        assert_eq!(rooms.len(), rm.run_state.acts.len());
        assert_eq!(
            keys(&rooms[0]),
            [
                "ancient",
                "boss",
                "elite",
                "events_shuffled",
                "normal",
                "second_boss"
            ]
        );
        assert_eq!(rooms[0]["second_boss"], Value::Null);
        assert_eq!(rooms[0]["ancient"], "Neow");
    }

    #[test]
    fn neow_start_surfaces_offers_and_event_option_infos() {
        let mut rm = new_run_manager_for_character(0, CharacterId::Ironclad)
            .with_neow_start()
            .with_act1(ActVariant::Overgrowth);
        initialize_run_setup(&mut rm);
        assert_eq!(rm.phase(), Phase::Event);
        let offers = neow_offers(&rm).expect("neow offers rolled");
        assert!(!offers.0.is_empty() && !offers.1.is_empty() && !offers.2.is_empty());
        let opts = event_options(&rm).expect("Neow is bound");
        assert!(!opts.is_empty());
        let infos = event_option_infos(&rm);
        let infos = infos.as_array().expect("list");
        assert_eq!(infos.len(), opts.len());
        for (info, (id, label)) in infos.iter().zip(&opts) {
            assert_eq!(info["option_id"], *id);
            assert_eq!(info["label"], *label);
            for k in ["description", "enabled", "loc_key", "title_vars", "vars"] {
                assert!(info.get(k).is_some(), "{k} missing from {info}");
            }
            assert!(info["vars"].is_object());
        }
        let room = current_room(&rm);
        assert_eq!(room["room_type"], "event");
    }
}
