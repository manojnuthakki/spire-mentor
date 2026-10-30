//! `RunManager` accessors the workbench reads, in the exact shapes the PyO3
//! binding (`sts2-py/src/run_manager.rs`) hands the Python workbench.
//!
//! The Python port drove `sts2_rs.RunManager` through ~40 small getters;
//! this module is their Rust twin over the core `RunManager`, so `game.rs`
//! reads like the Python it ports. Dict-shaped results come from
//! `sts2_wire::emit` (the single serde source the PyO3 crate itself now
//! serializes through); scalar / list results are computed here the way the
//! binding computes them.

use serde_json::Value;
use sts2_core::run_manager::{ActionResult, Phase, RunAction, RunManager};
use sts2_core::CombatState;
use sts2_wire::emit;

// ── strings ───────────────────────────────────────────────────────────────

/// `phase()` — the snake_case phase string the Python matched on.
#[must_use]
pub fn phase_str(rm: &RunManager) -> &'static str {
    phase_str_of(rm.phase())
}

/// [`phase_str`] for a phase value.
#[must_use]
pub fn phase_str_of(phase: Phase) -> &'static str {
    match phase {
        Phase::MapChoice => "map_choice",
        Phase::Combat => "combat",
        Phase::CardReward => "card_reward",
        Phase::BossRelic => "boss_relic",
        Phase::Shop => "shop",
        Phase::RestSite => "rest_site",
        Phase::Event => "event",
        Phase::Treasure => "treasure",
        Phase::RunOver => "run_over",
    }
}

/// `take_action` result string (`"advanced"` / `"run_over"` / …).
#[must_use]
pub fn result_str(r: &ActionResult) -> &'static str {
    match r {
        ActionResult::Advanced => "advanced",
        ActionResult::InvalidForPhase => "invalid_for_phase",
        ActionResult::NotImplemented => "not_implemented",
        ActionResult::RunOver => "run_over",
        ActionResult::ShopPurchaseFailed(_) => "shop_purchase_failed",
        ActionResult::ShopRemoveCardFailed(_) => "shop_remove_card_failed",
        _ => "unknown",
    }
}

/// `"Ironclad"` etc. from a `CharacterId`.
#[must_use]
pub fn character_str(c: sts2_core::potions::CharacterId) -> &'static str {
    use sts2_core::potions::CharacterId;
    match c {
        CharacterId::Ironclad => "Ironclad",
        CharacterId::Silent => "Silent",
        CharacterId::Defect => "Defect",
        CharacterId::Regent => "Regent",
        CharacterId::Necrobinder => "Necrobinder",
    }
}

/// Parse a character name (`"Ironclad"` / `"ironclad"`).
pub fn character_from_str(s: &str) -> Result<sts2_core::potions::CharacterId, String> {
    emit::names::character_from_str(s)
}

// ── actions ───────────────────────────────────────────────────────────────

/// `available_actions()` as engine values.
#[must_use]
pub fn available_actions(rm: &RunManager) -> Vec<RunAction> {
    rm.get_available_actions()
}

/// `run_action_to_pydict` twin: `{"action": ..., ...}`.
#[must_use]
pub fn action_value(a: &RunAction) -> Value {
    emit::run_action_to_value(a)
}

/// `action_from_pydict` twin.
pub fn action_from_value(v: &Value) -> Result<RunAction, String> {
    emit::run_action_from_value(v).map_err(|e| e.to_string())
}

/// The `"action"` kind string of an engine action, via its wire form.
#[must_use]
pub fn action_kind(a: &RunAction) -> String {
    action_value(a)
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string()
}

/// Integer field of an engine action's wire form (`idx`, `col`, `deck_idx`).
#[must_use]
pub fn action_int(a: &RunAction, key: &str) -> Option<i64> {
    action_value(a).get(key).and_then(Value::as_i64)
}

/// `take_action(dict)` → result string, applying the play-log flags first
/// exactly as `PyRunManager::ensure_play_log_flag` does.
pub fn take_action(
    rm: &mut RunManager,
    action: RunAction,
    record_card_plays: bool,
    record_combat_log: bool,
) -> &'static str {
    ensure_play_log_flag(rm, record_card_plays, record_combat_log);
    result_str(&rm.take_action(action))
}

/// `take_action({"action": "combat_resolve_choice_selection", "selection": [..]})`.
pub fn resolve_choice_selection(
    rm: &mut RunManager,
    selection: &[u8],
    record_card_plays: bool,
    record_combat_log: bool,
) -> &'static str {
    ensure_play_log_flag(rm, record_card_plays, record_combat_log);
    let sel: smallvec::SmallVec<[u8; 4]> = selection.iter().copied().collect();
    result_str(&rm.resolve_combat_choice_selection(sel))
}

/// `PyRunManager::ensure_play_log_flag`: the binding re-arms the two display
/// logs on the CURRENT combat before every action (a combat created BY the
/// action is armed by the next call — same timing as Python, which the
/// golden tests depend on).
pub fn ensure_play_log_flag(rm: &mut RunManager, record_card_plays: bool, record_combat_log: bool) {
    if record_card_plays {
        if let Some(c) = rm.combat.as_mut() {
            c.combat_log.record_card_play_log = true;
        }
    }
    if record_combat_log {
        if let Some(c) = rm.combat.as_mut() {
            c.combat_log.event_log.set_recording(true);
        }
    }
}

/// `set_record_card_plays(on)`.
pub fn set_record_card_plays(rm: &mut RunManager, on: bool) {
    if let Some(c) = rm.combat.as_mut() {
        c.combat_log.record_card_play_log = on;
    }
}

/// `set_record_combat_log(on)`.
pub fn set_record_combat_log(rm: &mut RunManager, on: bool) {
    if let Some(c) = rm.combat.as_mut() {
        c.combat_log.event_log.set_recording(on);
    }
}

// ── dict-shaped views (from sts2_wire::emit) ──────────────────────────────

/// `state_to_dict(cs)`.
#[must_use]
pub fn wire(cs: &CombatState) -> Value {
    // The only failure is a schema invariant the engine itself upholds (an
    // enemy uid missing from `creatures`); Python raised there too.
    emit::combat_state_to_value(cs).unwrap_or_else(|e| panic!("combat wire: {e}"))
}

/// `cs.card_damage_preview()` — one `{enemy_index: damage}` dict per hand card.
#[must_use]
pub fn damage_preview(cs: &CombatState) -> Vec<Value> {
    emit::card_damage_preview(cs)
        .into_iter()
        .map(|per_enemy| {
            let mut m = serde_json::Map::new();
            for (enemy_index, dmg) in per_enemy {
                m.insert(enemy_index.to_string(), Value::from(dmg));
            }
            Value::Object(m)
        })
        .collect()
}

/// `cs.card_block_preview()` — one `int | None` per hand card.
#[must_use]
pub fn block_preview(cs: &CombatState) -> Vec<Value> {
    emit::card_block_preview(cs)
        .into_iter()
        .map(|b| b.map_or(Value::Null, Value::from))
        .collect()
}

/// `cs.pending_choice()`.
#[must_use]
pub fn pending_choice(cs: &CombatState) -> Option<Value> {
    emit::pending_choice(cs)
}

/// `full_map()` (`None` → `Value::Null`).
#[must_use]
pub fn full_map(rm: &RunManager) -> Value {
    emit::run_manager::full_map(rm)
}

/// `path_plan(first_col)` (`None` → `Value::Null`).
#[must_use]
pub fn path_plan(rm: &RunManager, first_col: Option<i32>) -> Value {
    use std::collections::BTreeMap;
    use sts2_runai::eval::{HeuristicV, OptionalityWeights};
    use sts2_runai::policy::surfaces::pathing;
    use sts2_runai::view::VisibleRunView;

    let view = VisibleRunView::from_run_manager(rm);
    if view.map.is_none() {
        return Value::Null;
    }
    let v = HeuristicV::default();
    let Some((best_col, opt_cols)) = pathing::scored_first_cols(&view, &v) else {
        return Value::Null;
    };
    // Baseline arm: the pre-feature hard-max aggregator (no flexibility
    // valuation) — surfaced so the UI can show what optionality changed.
    let baseline = pathing::scored_first_cols_opt(
        &view,
        &v,
        &BTreeMap::new(),
        &OptionalityWeights::hard_max(),
    );
    let baseline_col = baseline.as_ref().map_or(best_col, |(c, _)| *c);
    let baseline_scores: BTreeMap<i32, f64> = baseline
        .map(|(_, cols)| cols.into_iter().collect())
        .unwrap_or_default();
    let stats: BTreeMap<i32, pathing::ColumnOptionality> = pathing::column_optionality(&view)
        .unwrap_or_default()
        .into_iter()
        .collect();
    let route: Vec<Value> = pathing::planned_route_from(&view, &v, first_col)
        .into_iter()
        .map(|c| serde_json::json!({"col": c.col, "row": c.row}))
        .collect();
    let columns: Vec<Value> = opt_cols
        .into_iter()
        .map(|(col, score)| {
            let st = stats.get(&col).copied().unwrap_or_default();
            serde_json::json!({
                "col": col,
                "score": score,
                "baseline_score": baseline_scores.get(&col).copied(),
                "n_paths": st.n_paths,
                "min_elites": st.min_elites,
                "max_elites": st.max_elites,
                "can_skip_elite": st.can_skip_elite(),
                "is_pick": col == best_col,
            })
        })
        .collect();
    serde_json::json!({
        "best_col": best_col,
        "first_col": first_col,
        "baseline_col": baseline_col,
        "diverged": best_col != baseline_col,
        "route": route,
        "columns": columns,
    })
}

/// `shop_inventory()` (`None` → `Value::Null`).
#[must_use]
pub fn shop_inventory(rm: &RunManager) -> Value {
    emit::run_manager::shop_inventory(rm)
}

/// `event_option_infos()` (`None` → `Value::Null`).
#[must_use]
pub fn event_option_infos(rm: &RunManager) -> Value {
    emit::run_manager::event_option_infos(rm)
}

/// `current_room()` (`None` → `Value::Null`).
#[must_use]
pub fn current_room(rm: &RunManager) -> Value {
    emit::run_manager::current_room(rm)
}

/// `combat_log()` — raw event rows, oldest first (empty outside combat).
#[must_use]
pub fn combat_log(rm: &RunManager) -> Vec<Value> {
    to_list(emit::run_manager::combat_log(rm))
}

/// `card_play_log()`.
#[must_use]
pub fn card_play_log(rm: &RunManager) -> Vec<Value> {
    to_list(emit::run_manager::card_play_log(rm))
}

fn to_list(v: Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a,
        _ => Vec::new(),
    }
}

// ── scalars / id lists (computed the way the binding computes them) ───────

/// `gold` — the combat-time mirror while a fight is ACTIVE, else run gold.
#[must_use]
pub fn gold(rm: &RunManager) -> i32 {
    emit::run_manager::gold(rm)
}

/// `relics()` — `RELIC.<PYTHON_NAME>` ids.
#[must_use]
pub fn relics(rm: &RunManager) -> Vec<String> {
    emit::run_manager::relics(rm)
}

/// `potions()` — the combat belt in `Phase::Combat`, else the run belt.
#[must_use]
pub fn potions(rm: &RunManager) -> Vec<String> {
    emit::run_manager::potions(rm)
}

/// `effective_master_deck_indices`: master-deck positions NOT currently
/// stolen by an in-progress `SwipePower` thief.
#[must_use]
pub fn effective_master_deck_indices(rm: &RunManager) -> Vec<usize> {
    emit::run_manager::effective_master_deck_indices(rm)
}

/// `master_deck_detailed()` — `(PascalCase id, upgraded)` per deck card.
#[must_use]
pub fn master_deck_detailed(rm: &RunManager) -> Vec<(String, bool)> {
    emit::run_manager::master_deck_detailed(rm)
}

/// `card_removal_options()` — removable cards in C#'s offer order.
#[must_use]
pub fn card_removal_options(rm: &RunManager) -> Vec<(String, bool)> {
    emit::run_manager::card_removal_options(rm)
}

/// `card_grant_options()`.
#[must_use]
pub fn card_grant_options(rm: &RunManager) -> Vec<(String, bool)> {
    emit::run_manager::card_grant_options(rm)
}

/// `pending_card_grants` (picks the FRONT grant still requires).
#[must_use]
pub fn pending_card_grants(rm: &RunManager) -> u32 {
    rm.run_state
        .pending_card_grants
        .first()
        .map_or(0, |g| u32::try_from(g.pick).unwrap_or(u32::MAX))
}

/// `pending_deck_removals`.
#[must_use]
pub fn pending_deck_removals(rm: &RunManager) -> u32 {
    rm.run_state.pending_deck_removals
}

/// `pending_deck_upgrades`.
#[must_use]
pub fn pending_deck_upgrades(rm: &RunManager) -> u32 {
    rm.run_state.pending_deck_upgrades
}

/// `pending_deck_transforms`.
#[must_use]
pub fn pending_deck_transforms(rm: &RunManager) -> u32 {
    u32::try_from(rm.run_state.pending_deck_transforms.len()).unwrap_or(u32::MAX)
}

/// `pending_deck_enchants`.
#[must_use]
pub fn pending_deck_enchants(rm: &RunManager) -> u32 {
    u32::try_from(rm.run_state.pending_deck_enchants.len()).unwrap_or(u32::MAX)
}

/// `transform_select_deck_indices()`.
#[must_use]
pub fn transform_select_deck_indices(rm: &RunManager) -> Vec<usize> {
    rm.transform_eligible_deck_indices()
}

/// `enchant_select_deck_indices()`.
#[must_use]
pub fn enchant_select_deck_indices(rm: &RunManager) -> Vec<usize> {
    rm.enchant_eligible_deck_indices()
}

/// `upgrade_select_deck_indices()` — C# `FromDeckForUpgrade`'s
/// `Where(IsUpgradable)` subset.
#[must_use]
pub fn upgrade_select_deck_indices(rm: &RunManager) -> Vec<usize> {
    rm.run_state
        .master_deck
        .iter()
        .enumerate()
        .filter(|(_, c)| c.is_upgradable())
        .map(|(i, _)| i)
        .collect()
}

/// `card_offer_choose_screen`.
#[must_use]
pub fn card_offer_choose_screen(rm: &RunManager) -> bool {
    rm.card_offer_choose_screen()
}

/// `relic_reward_ids()` — head of the queue first.
#[must_use]
pub fn relic_reward_ids(rm: &RunManager) -> Vec<String> {
    emit::run_manager::relic_reward_ids(rm)
}

/// `boss_relic_options()`.
#[must_use]
pub fn boss_relic_options(rm: &RunManager) -> Vec<String> {
    emit::run_manager::boss_relic_options(rm)
}

/// `rest_site_options()` — `(option_id, enabled)`; empty outside `RestSite`.
#[must_use]
pub fn rest_site_options(rm: &RunManager) -> Vec<(String, bool)> {
    emit::run_manager::rest_site_options(rm)
}

/// `smith_options()` — the open Smith `CardSelect`'s candidates.
#[must_use]
pub fn smith_options(rm: &RunManager) -> Vec<String> {
    emit::run_manager::smith_options(rm)
}

/// `card_reward_option_ids()` — `CARD.<PYTHON_NAME>`.
#[must_use]
pub fn card_reward_option_ids(rm: &RunManager) -> Vec<String> {
    emit::run_manager::card_reward_option_ids(rm)
}

/// `card_reward_bundles()`.
#[must_use]
pub fn card_reward_bundles(rm: &RunManager) -> Option<Vec<Vec<String>>> {
    emit::run_manager::card_reward_bundles(rm)
}

/// `materialize_event()` — bind the room's event if none is bound yet, then
/// name it (`LostWisp`).
pub fn materialize_event(rm: &mut RunManager) -> Option<String> {
    if rm.run_state.current_event.is_none() {
        let id = sts2_content::events::dispatch::pick_event(&mut rm.run_state, None)?;
        rm.run_state.bind_event(id);
    }
    current_event_name(rm)
}

/// `current_event_name()`.
#[must_use]
pub fn current_event_name(rm: &RunManager) -> Option<String> {
    rm.run_state
        .current_event
        .map(|id| id.python_name().to_string())
}

/// `act1` — the RESOLVED Act-1 model this run walks.
#[must_use]
pub fn act1_str(rm: &RunManager) -> &'static str {
    use sts2_core::act_selection::ActVariant;
    match rm.run_state.act1 {
        ActVariant::Overgrowth => "overgrowth",
        ActVariant::Underdocks => "underdocks",
        ActVariant::Hive => "hive",
        ActVariant::Glory => "glory",
    }
}

/// `run_seed` — the hashed master seed as `u32`.
#[must_use]
pub fn run_seed(rm: &RunManager) -> u32 {
    emit::run_manager::run_seed(rm)
}

/// `map_coord()` — `(col, row)` of the current room, if it sits on the map.
#[must_use]
pub fn map_coord(rm: &RunManager) -> Option<(i32, i32)> {
    emit::run_manager::map_coord(rm)
}

/// `max_potion_slots()`.
#[must_use]
pub fn max_potion_slots(rm: &RunManager) -> i32 {
    rm.run_state.max_potion_slots
}

/// `total_floor()`.
#[must_use]
pub fn total_floor(rm: &RunManager) -> i32 {
    rm.run_state.total_floor
}

/// Whether an in-combat `pending_choice` (Headbutt etc.) is open.
#[must_use]
pub fn combat_choice_open(rm: &RunManager) -> bool {
    rm.combat
        .as_ref()
        .is_some_and(|c| c.pending_choice.is_some())
}
