//! Live PPO run policy — Task 8 of the 2026-07-07 ppo-rust-inference
//! plan. Builds `sts2-pycli/python/ppo/env.py::observe()`-equivalent
//! observations straight from a live [`RunManager`] (the same core
//! accessors the PyO3 getters in `sts2-py::run_manager` wrap), runs
//! them through the [`super::encoder`]/[`super::net`] pipeline (Tasks
//! 3-7), and picks actions with the exact decision-ownership order of
//! `env.py::_advance` (porting authority; never modify that file).
//!
//! The PPO net never plays combat — `Phase::Combat` stays
//! `MctsCombatPolicy`'s (see `arena.rs::build_policy`'s
//! `(PolicyKind::Ppo, Some(budget))` arm, the only cell this policy is
//! ever constructed for). `MctsCombatPolicy` can still hand a rare
//! combat decision BACK to this policy on its documented fallback paths
//! (`policy::mod`'s `FallbackCounts`); `PpoPolicy::choose` covers that
//! by delegating to an embedded `HeuristicPolicy` rather than asserting
//! the net is never asked (see the `Phase::Combat` branch there).

#[cfg(feature = "ppo")]
use std::sync::Arc;

#[cfg(feature = "ppo")]
use sts2_core::full_run::RunPolicy;
use sts2_core::map::MapPointType;
use sts2_core::run_manager::RunManager;
#[cfg(feature = "ppo")]
use sts2_core::run_manager::{Phase, RunAction};
use sts2_core::shop::{is_shop_entry_available, ShopInventory};
use sts2_core::ChoiceKind;

#[cfg(feature = "ppo")]
use crate::policy::{DecisionTelemetry, HeuristicPolicy, MetaEvalRecord, VScoreSource};

#[cfg(feature = "ppo")]
use super::encoder;
#[cfg(feature = "ppo")]
use super::features::Features;
#[cfg(feature = "ppo")]
use super::net::PpoNet;
#[cfg(feature = "ppo")]
use super::obs::ActionDesc;
use super::obs::{
    BossNodeObs, Coord, EventOptionObs, FullMapObs, MapNodeObs, PpoObs, ShopCardObs, ShopEntryObs,
    ShopObs,
};

/// `sts2-py::run_manager::phase_str` mirror. Kept local (rather than
/// depending on `sts2-py`, which would invert the crate dependency
/// direction — `sts2-py` already depends on `sts2-runai`) — see the
/// plan's design decision 6 for the analogous `vocab::registry_names`
/// single-sourcing; this one is small enough that a duplicate mapping
/// is the pragmatic call.
#[cfg(feature = "ppo")]
pub fn phase_str(p: Phase) -> &'static str {
    match p {
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

/// `(name, EventVar)` pairs -> a JSON-value map, mirroring the `PyDict` the
/// PyO3 `event_option_infos()` getter builds for `vars`/`title_vars` — int
/// stays a JSON number (the template DSL's `:plural:` form compares it to 1;
/// a string would always take the plural branch), text stays a string.
fn event_vars_to_map(
    vars: &[(&'static str, sts2_core::EventVar)],
) -> std::collections::BTreeMap<String, serde_json::Value> {
    vars.iter()
        .map(|(name, value)| {
            let v = match value {
                sts2_core::EventVar::Int(i) => serde_json::Value::from(*i),
                sts2_core::EventVar::Text(s) => serde_json::Value::from(s.clone()),
            };
            ((*name).to_string(), v)
        })
        .collect()
}

/// `sts2-py::run_manager::map_point_type_ui_str` mirror (same rationale
/// as [`phase_str`]) — `full_map()`'s PascalCase UI strings, which
/// `mapfeat::pascal_to_snake` converts back to the vocab's snake form.
fn map_point_type_ui_str(t: MapPointType) -> &'static str {
    match t {
        MapPointType::Unassigned => "Unassigned",
        MapPointType::Unknown => "Unknown",
        MapPointType::Monster => "Monster",
        MapPointType::Elite => "Elite",
        MapPointType::Boss => "Boss",
        MapPointType::Shop => "Shop",
        MapPointType::RestSite => "RestSite",
        MapPointType::Treasure => "Treasure",
        MapPointType::Ancient => "Ancient",
    }
}

/// `env.py::classify_surface` port: smith-first or phase-agnostic
/// deck-mutation lists are card picks; otherwise the phase decides.
/// Mirrors `sts2-runai::policy::ablation::classify` (the same rule,
/// ported independently here per the brief — `env.py` is the porting
/// authority for THIS module, not the ablation policy).
#[cfg(feature = "ppo")]
pub fn classify_surface(phase: Phase, actions: &[RunAction]) -> Option<&'static str> {
    if matches!(actions.first(), Some(RunAction::SmithCard { .. })) {
        return Some("card-pick");
    }
    if !actions.is_empty()
        && actions.iter().all(|a| {
            matches!(
                a,
                RunAction::RemoveDeckCard { .. }
                    | RunAction::UpgradeDeckCard { .. }
                    | RunAction::TransformDeckCard { .. }
                    | RunAction::EnchantDeckCard { .. }
                    | RunAction::GrantDeckCard { .. }
            )
        })
    {
        return Some("card-pick");
    }
    // A full-belt potion swap preempts the action surface in ANY phase
    // (`run_manager.rs:1010`), so it has to be claimed before the phase
    // match — otherwise it would be reported as whatever room it happened
    // to interrupt. GH #252/#379.
    if !actions.is_empty()
        && actions
            .iter()
            .all(|a| matches!(a, RunAction::PotionSwap { .. }))
    {
        return Some("potion-swap");
    }
    match phase {
        Phase::MapChoice => Some("path"),
        Phase::CardReward | Phase::BossRelic => Some("card-pick"),
        Phase::Event => Some("event"),
        Phase::Shop => Some("shop"),
        Phase::RestSite => Some("rest"),
        Phase::Treasure => Some("treasure"),
        Phase::Combat | Phase::RunOver => None,
    }
}

/// `env.py::mask_rejectable` port: drop a belt-full `shop_buy_potion`
/// (a C#-lockstep graceful engine no-op that would deadlock a greedy
/// argmax policy), keeping every other action's relative order and
/// original identity. Never returns an empty list (falls back to the
/// unfiltered `actions`, matching Python's `masked or actions`).
#[cfg(feature = "ppo")]
pub fn mask_rejectable(rm: &RunManager, phase: Phase, actions: &[RunAction]) -> Vec<RunAction> {
    if phase != Phase::Shop {
        return actions.to_vec();
    }
    if rm.run_state.potions.len() < rm.run_state.max_potion_slots as usize {
        return actions.to_vec();
    }
    let masked: Vec<RunAction> = actions
        .iter()
        .copied()
        .filter(|a| !matches!(a, RunAction::ShopBuyPotion { .. }))
        .collect();
    if masked.is_empty() {
        actions.to_vec()
    } else {
        masked
    }
}

/// `single_boss_for_act` mirror (`sts2-py::run_manager.rs` ~:138): Act 1
/// is intentionally treated as multi-boss (returns `None`), Acts 2/3
/// return the pool's sole entry when it has exactly one.
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

/// `RunManager.full_map()`'s boss `kind`/`id` resolution chain mirror
/// (`sts2-py::run_manager.rs` ~:1893-1926): Gate A2 (Task 9) proved this
/// is NOT encoder-dead — `observe()`'s obs dict carries it verbatim and
/// the gate compares the whole dict, not just what the encoder reads —
/// so despite `mapfeat::compute` only reading a boss node's `room_type`,
/// this must still match byte-for-byte. Priority: pinned boss (display
/// only for `kind`; `id` prefers the generated-room roll, falling back
/// to the pin's display name) > generated-room roll alone > the current
/// pin's single-boss-pool fallback.
fn resolve_boss_display(rm: &RunManager, act_index: u8) -> (Option<String>, Option<String>) {
    let gen_boss_id = act_index
        .checked_sub(1)
        .and_then(|i| rm.run_state.generated_rooms.get(i as usize))
        .map(|g| g.boss_id);
    if let Some(pin) = rm.run_state.pinned_act_boss.as_ref() {
        let kind = Some(pin.lead_kind.python_name().to_string());
        let id = Some(gen_boss_id.unwrap_or(pin.display_name).to_string());
        (kind, id)
    } else if let Some(id) = gen_boss_id {
        (None, Some(id.to_string()))
    } else if let Some(spec) = single_boss_for_act(act_index) {
        match spec.members.first() {
            Some(lead) => (
                Some(lead.kind.python_name().to_string()),
                Some(spec.name.to_string()),
            ),
            None => (None, None),
        }
    } else {
        (None, None)
    }
}

/// `PyRunManager::effective_master_deck_indices` mirror
/// (`sts2-py::run_manager.rs` ~:516): drop master-deck entries an
/// unreclaimed `SwipePower` theft is still holding.
///
/// Gated on `rm.combat.is_some()`, **not** `Phase::Combat`, to stay
/// byte-identical with the sts2-py mirror (whose serialization applies
/// the same subtraction whenever a `CombatState` is present, e.g. on
/// the reward screen). Since `d1e957f92`,
/// `exit_combat_and_transition` clears `combat.swipe_stolen` after the
/// exit callback flushes un-reclaimed steals into `master_deck`
/// (`sts2-core/src/run_manager.rs` ~:1969, pinned by
/// `exit_combat_clears_settled_swipe_ledger`), so out of combat this
/// filter is the *identity* whenever `callbacks.exit_combat` is wired
/// — all production paths — and `obs.deck` indices coincide with raw
/// `master_deck` indices on non-combat surfaces. (The pre-fix Gate A2
/// finding — a stale ledger lingering in `rm.combat` excluding cards
/// well after combat — no longer applies; see git history.)
fn effective_master_deck_indices(rm: &RunManager) -> Vec<usize> {
    let all = 0..rm.run_state.master_deck.len();
    let Some(combat) = rm.combat.as_ref() else {
        return all.collect();
    };
    // sts2-agent fork: each stolen card by its deck link
    // (`CombatState::unreclaimed_stolen_deck_indices`), not its combat state.
    let stolen = combat.unreclaimed_stolen_deck_indices(&rm.run_state.master_deck);
    all.filter(|i| !stolen.contains(i)).collect()
}

/// `RunManager.full_map()` mirror (`sts2-py::run_manager.rs` ~:1842),
/// built from the same [`sts2_core::map::ActMap::full_map_view`] the
/// PyO3 getter wraps, including its boss `kind`/`id` resolution chain
/// ([`resolve_boss_display`]) — Gate A2 (Task 9) proved that chain
/// affects the obs dict Python compares byte-for-byte, so it can't be
/// left `None` as originally assumed (see git history for the prior
/// doc comment this superseded).
fn build_full_map_obs(rm: &RunManager) -> Option<FullMapObs> {
    let act_map = rm.run_state.current_act_map.as_ref()?;
    let current = rm.run_state.current_room.as_ref().and_then(|r| r.map_coord);
    let view = act_map.full_map_view(current, &rm.run_state.visited_map_coords);

    let rows = view
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|n| MapNodeObs {
                    col: i64::from(n.coord.col),
                    row: i64::from(n.coord.row),
                    room_type: map_point_type_ui_str(n.point_type).to_string(),
                    children: n
                        .children
                        .iter()
                        .map(|c| Coord {
                            col: i64::from(c.col),
                            row: i64::from(c.row),
                        })
                        .collect(),
                    visited: n.visited,
                    current: n.current,
                })
                .collect()
        })
        .collect();

    let boss = view.boss.map(|b| {
        let (kind, id) = resolve_boss_display(rm, rm.run_state.act_index);
        BossNodeObs {
            col: i64::from(b.coord.col),
            row: i64::from(b.coord.row),
            room_type: map_point_type_ui_str(b.point_type).to_string(),
            kind,
            id,
        }
    });
    // A10 DoubleBoss identity, mirroring the PyO3 getter's `second_boss`
    // arm exactly: `col`/`row`/`type` off the map point, plus the PREVIOUS
    // act's `second_boss_id` (`act_index - 1`, the act whose rooms were
    // generated for this map). It never sets `kind`, so leave it `None` —
    // Gate A2 compares the two dicts key-for-key.
    let second_boss = act_map.second_boss.map(|sb_idx| {
        let p = act_map.point(sb_idx);
        BossNodeObs {
            col: i64::from(p.coord.col),
            row: i64::from(p.coord.row),
            room_type: map_point_type_ui_str(p.point_type).to_string(),
            kind: None,
            id: rm
                .run_state
                .act_index
                .checked_sub(1)
                .and_then(|i| rm.run_state.generated_rooms.get(i as usize))
                .and_then(|g| g.second_boss_id)
                .map(str::to_string),
        }
    });
    let current_coord = view.current_coord.map(|c| Coord {
        col: i64::from(c.col),
        row: i64::from(c.row),
    });

    Some(FullMapObs {
        rows,
        boss,
        second_boss,
        current_coord,
    })
}

/// `RunManager.shop_inventory()` mirror (`sts2-py::run_manager.rs`
/// ~:1184).
fn build_shop_obs(inv: &ShopInventory) -> ShopObs {
    let cards: Vec<ShopCardObs> = inv
        .cards
        .iter()
        .chain(inv.colorless_cards.iter())
        .enumerate()
        .map(|(idx, e)| ShopCardObs {
            idx: idx as i64,
            id: e.card.as_ref().map(|c| format!("{:?}", c.card_id)),
            price: i64::from(e.price),
            on_sale: e.on_sale,
            is_colorless: e.is_colorless,
            available: is_shop_entry_available(e.price) && e.card.is_some(),
        })
        .collect();
    let relics: Vec<ShopEntryObs> = inv
        .relics
        .iter()
        .enumerate()
        .map(|(idx, e)| ShopEntryObs {
            idx: idx as i64,
            id: e.relic_id.map(|r| format!("RELIC.{}", r.to_python_name())),
            price: i64::from(e.price),
            available: is_shop_entry_available(e.price) && e.relic_id.is_some(),
        })
        .collect();
    let potions: Vec<ShopEntryObs> = inv
        .potions
        .iter()
        .enumerate()
        .map(|(idx, e)| ShopEntryObs {
            idx: idx as i64,
            id: e.potion_kind.map(|k| format!("{k:?}")),
            price: i64::from(e.price),
            available: is_shop_entry_available(e.price) && e.potion_kind.is_some(),
        })
        .collect();
    ShopObs {
        cards,
        relics,
        potions,
        removal_cost: i64::from(inv.removal_cost),
        removal_used: inv.removal_used,
    }
}

/// `env.py::observe` + `metrics_snapshot` port: builds the observation
/// dict the encoder consumes straight from a live [`RunManager`], via
/// the same core accessors the PyO3 getters wrap (file:line refs below
/// match the plan's File Map).
///
/// `PpoPolicy` never runs mid-`Phase::Combat` (combat stays
/// `MctsCombatPolicy`'s — see the module doc comment), so unlike
/// `sts2-py`'s `potions` getter (gated on `Phase::Combat` specifically)
/// this never needs a live-combat-belt special case. `master_deck`'s
/// twin filter is a different story: [`effective_master_deck_indices`]
/// gates on `rm.combat.is_some()`, which can stay `true` well past
/// `Phase::Combat` (the finished fight's state lingers until the next
/// one starts) — see that function's doc comment for how Gate A2 (Task
/// 9) caught this.
#[must_use]
pub fn build_obs(rm: &RunManager, phase: &str, surface: Option<&str>) -> PpoObs {
    let rs = &rm.run_state;

    let deck: Vec<(String, bool)> = effective_master_deck_indices(rm)
        .into_iter()
        .map(|i| {
            let c = &rs.master_deck[i];
            (format!("{:?}", c.card_id), c.upgraded)
        })
        .collect();
    let relics: Vec<String> = rs
        .relics
        .iter()
        .map(|r| format!("RELIC.{}", r.to_python_name()))
        .collect();
    let potions: Vec<String> = rs.potions.iter().map(|k| format!("{k:?}")).collect();
    let map = build_full_map_obs(rm);
    let map_coord = rs
        .current_room
        .as_ref()
        .and_then(|r| r.map_coord)
        .map(|c| (i64::from(c.col), i64::from(c.row)));

    // `deck_size`/`num_upgraded` (Python's `metrics_snapshot`) are derived
    // from the SAME filtered `deck` view, not raw `rs.master_deck` — see
    // `effective_master_deck_indices`'s doc comment.
    let deck_size = deck.len() as i64;
    let num_upgraded = deck.iter().filter(|(_, up)| *up).count() as i64;

    let mut obs = PpoObs {
        phase: phase.to_string(),
        surface: surface.map(str::to_string),
        seed: i64::from(rm.seed),
        ascension: i64::from(rs.ascension_level),
        deck,
        relics,
        potions,
        map,
        map_coord,
        total_floor: i64::from(rs.total_floor),
        act_index: i64::from(rs.act_index),
        act_floor: i64::from(rs.act_floor),
        hp: i64::from(rs.current_hp),
        max_hp: i64::from(rs.max_hp),
        gold: i64::from(rs.gold),
        deck_size,
        num_upgraded,
        num_relics: rs.relics.len() as i64,
        num_potions: rs.potions.len() as i64,
        card_reward_option_ids: None,
        card_reward_option_upgraded: None,
        card_reward_bundles: None,
        relic_reward_ids: None,
        boss_relic_options: None,
        smith_options: None,
        card_removal_options: None,
        card_grant_options: None,
        transform_select_deck_indices: None,
        enchant_select_deck_indices: None,
        pending_enchantment: None,
        upgrade_select_deck_indices: None,
        shop: None,
        pending_potion_swap: None,
        event_name: None,
        event_options: None,
        rest_option_labels: None,
    };

    match surface {
        Some("card-pick") => {
            obs.card_reward_option_ids = Some(
                rm.card_reward_options()
                    .iter()
                    .map(|cid| format!("CARD.{}", cid.to_python_name()))
                    .collect(),
            );
            obs.card_reward_option_upgraded = Some(rm.card_reward_upgraded_flags().to_vec());
            obs.card_reward_bundles = Some(rm.card_reward_bundles().map(|packs| {
                packs
                    .iter()
                    .map(|pack| {
                        pack.iter()
                            .map(|cid| format!("CARD.{}", cid.to_python_name()))
                            .collect()
                    })
                    .collect()
            }));
            obs.relic_reward_ids = Some(
                rs.relic_rewards
                    .iter()
                    .rev()
                    .map(|r| format!("{:?}", r.relic_id))
                    .collect(),
            );
            obs.boss_relic_options = Some(
                rm.boss_relic_options()
                    .iter()
                    .map(|r| format!("{r:?}"))
                    .collect(),
            );
            obs.smith_options = Some(
                rs.pending_choice
                    .as_ref()
                    .filter(|pc| pc.kind == ChoiceKind::CardSelect)
                    .map(|pc| {
                        pc.options
                            .iter()
                            .map(|opt| format!("{:?}", opt.card_id))
                            .collect()
                    })
                    .unwrap_or_default(),
            );
            obs.card_removal_options = Some(
                rs.removal_offer_order()
                    .into_iter()
                    .map(|i| {
                        let c = &rs.master_deck[i];
                        (format!("{:?}", c.card_id), c.upgraded)
                    })
                    .collect(),
            );
            obs.card_grant_options = Some(
                rs.pending_card_grants
                    .first()
                    .map(|g| {
                        g.options
                            .iter()
                            .map(|c| (format!("{:?}", c.card_id), c.upgraded))
                            .collect()
                    })
                    .unwrap_or_default(),
            );
            obs.transform_select_deck_indices = Some(
                rm.transform_eligible_deck_indices()
                    .into_iter()
                    .map(|i| i as i64)
                    .collect(),
            );
            obs.enchant_select_deck_indices = Some(
                rm.enchant_eligible_deck_indices()
                    .into_iter()
                    .map(|i| i as i64)
                    .collect(),
            );
            // GH #545: WHICH enchantment the front queued pick stamps — the
            // `enchant_select_deck_indices` list above says WHERE, this says
            // WHAT. `python_name()` is the canonical registry id, not the
            // localized `wire_name()` display title.
            // Outer `Some` unconditionally: this arm IS the `"card-pick"`
            // surface, and `env.py::observe()` sets the key there regardless
            // of whether an enchant pick is actually queued (double-`Option`
            // — see the field doc on `PpoObs::pending_enchantment`).
            obs.pending_enchantment = Some(
                rs.pending_deck_enchants
                    .first()
                    .map(|p| p.enchantment.python_name().to_string()),
            );
            // GH #399 Task 5b: the `UpgradeDeckCard` index space, taken from
            // the ONE helper that already carries the engine's own
            // `is_upgradable()` filter (a807f36a9) — never re-derived here,
            // and never the `!upgraded` subset the raw deck would give.
            obs.upgrade_select_deck_indices = Some(
                crate::policy::surfaces::picks::upgradable_deck_indices(rm)
                    .into_iter()
                    .map(|i| i as i64)
                    .collect(),
            );
        }
        Some("shop") => {
            obs.shop = rm.shop_inventory().map(build_shop_obs);
        }
        Some("event") => {
            obs.event_name = rs.current_event.map(|id| id.python_name().to_string());
            obs.event_options = sts2_content::events::dispatch::present(rs).map(|opts| {
                opts.into_iter()
                    .enumerate()
                    .map(|(idx, opt)| {
                        // Structured effect payload (GH #237): mirror the
                        // PyO3 `event_option_infos()` so `ppo_observe_json`
                        // and `env.observe()` stay byte-identical (Gate A2).
                        let delta = sts2_content::events::dispatch::preview_option_effect(rs, idx);
                        EventOptionObs {
                            option_id: opt.option_id.to_string(),
                            label: opt.label,
                            description: opt.description,
                            enabled: opt.enabled,
                            loc_key: opt.loc_key.to_string(),
                            vars: event_vars_to_map(&opt.vars),
                            title_vars: event_vars_to_map(&opt.title_vars),
                            gold_delta: delta.map(|d| i64::from(d.gold_delta)),
                            hp_delta: delta.map(|d| i64::from(d.hp_delta)),
                            max_hp_delta: delta.map(|d| i64::from(d.max_hp_delta)),
                            relics_gained: delta.map(|d| i64::from(d.relics_gained)),
                            relics_lost: delta.map(|d| i64::from(d.relics_lost)),
                            potions_gained: delta.map(|d| i64::from(d.potions_gained)),
                            potions_lost: delta.map(|d| i64::from(d.potions_lost)),
                            cards_gained: delta.map(|d| i64::from(d.cards_gained)),
                            cards_lost: delta.map(|d| i64::from(d.cards_lost)),
                        }
                    })
                    .collect()
            });
        }
        Some("rest") => {
            obs.rest_option_labels = Some(vec!["rest".to_string(), "smith".to_string()]);
        }
        Some("potion-swap") => {
            // `RunManager::pending_potion_swap`'s debug-name spelling
            // (`format!("{k:?}")`), the same one `obs.potions` uses, so the
            // encoder's `vocab::potions()` lookup resolves both alike.
            obs.pending_potion_swap = rs.pending_potion_swaps.first().map(|k| format!("{k:?}"));
        }
        _ => {}
    }

    obs
}

/// Per-run high-water marks of the export budget this policy actually
/// consumed (GH #315). An overflow is fatal, so the only always-on
/// signal that a sweep is approaching the cliff is how close its worst
/// decision got — `arena.rs` reports both alongside the `FallbackCounts`
/// columns so headroom is visible in every sweep without a special build.
///
/// Atomics rather than `Cell` because [`PpoPolicy`] is scored through
/// `&self` from [`PpoPolicy::evaluate_with_actions`] and the type must
/// stay `Sync` to sit inside the `Arc`-shared cell policies.
#[cfg(feature = "ppo")]
#[derive(Debug, Default)]
pub struct TokenPeak {
    act_slots: std::sync::atomic::AtomicUsize,
    tokens: std::sync::atomic::AtomicUsize,
}

#[cfg(feature = "ppo")]
impl TokenPeak {
    fn observe(&self, obs_len: usize, n_actions: usize) {
        use std::sync::atomic::Ordering::Relaxed;
        self.act_slots.fetch_max(n_actions, Relaxed);
        self.tokens.fetch_max(obs_len + n_actions, Relaxed);
    }

    /// `(peak action slots, peak obs+act tokens)` seen so far.
    #[must_use]
    pub fn get(&self) -> (usize, usize) {
        use std::sync::atomic::Ordering::Relaxed;
        (self.act_slots.load(Relaxed), self.tokens.load(Relaxed))
    }
}

/// The live PPO run policy. Holds the loaded net plus a
/// [`HeuristicPolicy`] used ONLY as the combat-fallback delegate (see the
/// `RunPolicy::choose` impl below) — construction is
/// [`PpoPolicy::from_env`], mirroring the `build_ranker`/`build_value`
/// env-var idiom in `arena.rs`.
#[cfg(feature = "ppo")]
#[derive(Debug)]
pub struct PpoPolicy {
    net: Arc<PpoNet>,
    /// The encoder schema THIS net was trained at, resolved once at
    /// construction from `champions.toml` (or, for an explicit path, from
    /// `STS2_PPO_FEATURES`). Carried per policy rather than read from the
    /// process-wide [`Features::active`] so a process holding several
    /// characters' champions serves each at its own schema — see
    /// [`super::net::features_for`].
    features: Features,
    /// Combat-fallback delegate — see the `choose` doc comment on the
    /// `Phase::Combat` branch. Never scores a non-combat decision.
    heuristic: HeuristicPolicy,
    /// Export-budget high-water marks over this policy's lifetime (one
    /// per run). See [`TokenPeak`].
    peak: TokenPeak,
    /// Scoring of the decision `choose` just made, for the trajectory
    /// recorder ([`DecisionTelemetry`]). Consumed per decision, so it
    /// can never leak into a decision this policy did not score.
    last_eval: Option<MetaEvalRecord>,
    /// Group-mass decode of the net's distribution (A8 audit §8):
    /// candidates that differ only in TARGET are pooled before the
    /// argmax so a split "remove a card" preference isn't outvoted by
    /// a single-candidate alternative. Resolved once at construction
    /// from `STS2_RUNAI_PPO_GROUP_DECODE` (default ON, `0` restores
    /// the plain per-candidate argmax) — see [`grouped_argmax`].
    group_decode: bool,
}

#[cfg(feature = "ppo")]
impl PpoPolicy {
    /// Load the ONNX model from `STS2_RUNAI_PPO`, or the sibling
    /// `sts2-pycli` checkout's committed snapshot
    /// ([`super::net::onnx_model_path`]). Panics loudly on a
    /// missing/unloadable model — there is no policy-degradation
    /// fallback, matching `arena::build_ranker`/`build_value`.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_env_for(sts2_core::potions::CharacterId::Ironclad)
    }

    /// Load the ONNX model for `character` from `STS2_RUNAI_PPO` (an
    /// explicit override that wins for every character), or the sibling
    /// `sts2-pycli` checkout's committed per-character champion
    /// ([`super::net::onnx_model_path_for`]). Panics loudly on a
    /// missing/unloadable model — including a character with no trained
    /// champion — there is no policy-degradation fallback, matching
    /// `arena::build_ranker`/`build_value`.
    #[must_use]
    pub fn from_env_for(character: sts2_core::potions::CharacterId) -> Self {
        let path = super::net::onnx_model_path_for(character).unwrap_or_else(|| {
            panic!(
                "--policy ppo for {character:?} requires STS2_RUNAI_PPO=<model.onnx> \
                 or a committed sibling champion for this character"
            )
        });
        Self::from_path_with(&path, super::net::features_for(character))
    }

    /// Load the ONNX model from an explicit path, bypassing the
    /// `STS2_RUNAI_PPO`/sibling-checkout resolution [`Self::from_env`]
    /// does. Used by the Task 9 PyO3 `PpoRunAgent` binding, which wants
    /// to pin a specific snapshot (e.g. from a test fixture) rather than
    /// rely on the process-global env var. Panics loudly on an
    /// unloadable model, matching [`Self::from_env`].
    #[must_use]
    pub fn from_path(path: &std::path::Path) -> Self {
        // An explicit path is by definition a net no manifest entry describes,
        // so its schema comes from the environment — the same lane
        // `net::features_for` reserves for `STS2_RUNAI_PPO`.
        Self::from_path_with(path, Features::active())
    }

    /// [`Self::from_path`] with an explicit encoder feature set rather than
    /// the process-wide [`Features::active`].
    ///
    /// This is the seam that lets ONE process serve champions trained at
    /// DIFFERENT schemas. `STS2_PPO_FEATURES` is parsed once per process, so
    /// it cannot say "on for Ironclad, off for the other four" — and the
    /// mismatch is invisible in the tensor, because a gated feature rewrites
    /// an existing slot instead of adding a column. See
    /// [`super::net::features_for`].
    #[must_use]
    pub fn from_path_with(path: &std::path::Path, features: Features) -> Self {
        let net = PpoNet::load(path)
            .unwrap_or_else(|e| panic!("PPO net: failed to load {}: {e}", path.display()));
        Self::with_net(net, features)
    }

    /// [`Self::from_path_with`] over the ONNX bytes ([`PpoNet::from_bytes`]),
    /// for hosts without a filesystem (the browser workbench). Unlike the
    /// path constructors this returns the load error instead of panicking:
    /// the workbench degrades to its heuristic brain and shows the reason.
    ///
    /// # Errors
    /// tract's error text when the bytes are not a loadable export.
    pub fn from_bytes_with(bytes: &[u8], features: Features) -> Result<Self, String> {
        PpoNet::from_bytes(bytes).map(|net| Self::with_net(net, features))
    }

    fn with_net(net: PpoNet, features: Features) -> Self {
        Self {
            features,
            net: Arc::new(net),
            heuristic: HeuristicPolicy::new(),
            peak: TokenPeak::default(),
            last_eval: None,
            group_decode: group_decode_enabled(
                std::env::var("STS2_RUNAI_PPO_GROUP_DECODE").ok().as_deref(),
            ),
        }
    }

    /// `(peak action slots, peak obs+act tokens)` over this policy's
    /// lifetime — the export-budget headroom signal (GH #315). `(0, 0)`
    /// until the net scores its first decision.
    #[must_use]
    pub fn peak(&self) -> (usize, usize) {
        self.peak.get()
    }

    /// Side-effect-free decision pipeline: mask belt-full
    /// `shop_buy_potion`, classify the surface, build the observation,
    /// featurize the FILTERED candidate list, and run inference.
    ///
    /// Returns `None` for `Phase::Combat`/`Phase::RunOver` (this policy
    /// never scores those — combat is `MctsCombatPolicy`'s and a
    /// finished run has no decision to score) or when at most one
    /// candidate remains after masking (no inference needed). Used by
    /// [`Self::evaluate`] (which delegates here) — and thus transitively
    /// by [`Self::choose`] (below) and the Task 9 PyO3 binding, which
    /// wants the raw distribution/value for display without taking an
    /// action.
    ///
    /// `probs`/`argmax` are indexed into the MASKED candidate list
    /// (`probs.len()` == the number of actions actually shown to the
    /// net), not the caller's original `actions` — [`Self::choose`]
    /// maps the result back to an original-list index itself.
    ///
    /// `probs` are the RAW per-candidate probabilities; `argmax` is the
    /// FINAL pick after the group-mass decode ([`grouped_argmax`], A8
    /// audit §8 — `STS2_RUNAI_PPO_GROUP_DECODE=0` restores the plain
    /// per-candidate argmax). Telemetry ([`MetaEvalRecord`]) and the
    /// workbench panel both consume this pair, so transcripts show the
    /// true distribution and dispatch the true choice.
    ///
    /// Also returns the MASKED candidate list itself (owned) whose order
    /// matches `probs` — [`Self::evaluate`] delegates here and drops it.
    /// The workbench PPO panel needs the exact actions the net scored so
    /// it can label each probability and dispatch the argmax;
    /// re-deriving the mask on the Python side would risk a silent
    /// misalignment.
    #[must_use]
    pub fn evaluate_with_actions(
        &self,
        rm: &RunManager,
        actions: &[RunAction],
    ) -> Option<(Vec<RunAction>, Vec<f32>, f32, usize)> {
        self.try_evaluate_with_actions(rm, actions)
            .unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`Self::evaluate_with_actions`] that reports an inference failure
    /// instead of panicking — for hosts that must degrade and carry on
    /// (the browser workbench falls back to its heuristic brain and shows
    /// the reason). The `Err` text is exactly the panic message the
    /// panicking twin raises.
    ///
    /// # Errors
    /// A combat-shaped action outside combat, or a net inference failure
    /// (an export-budget overflow, named with the phase / surface / deck
    /// size — GH #315).
    pub fn try_evaluate_with_actions(
        &self,
        rm: &RunManager,
        actions: &[RunAction],
    ) -> Result<Option<(Vec<RunAction>, Vec<f32>, f32, usize)>, String> {
        let phase = rm.phase();
        if phase == Phase::Combat || phase == Phase::RunOver {
            return Ok(None);
        }
        let masked = mask_rejectable(rm, phase, actions);
        if masked.len() < 2 {
            return Ok(None);
        }
        let surface = classify_surface(phase, &masked);
        let obs = build_obs(rm, phase_str(phase), surface);
        let action_descs = masked
            .iter()
            .map(|a| {
                ActionDesc::try_from(a).map_err(|()| {
                    format!("PpoPolicy::evaluate_with_actions saw a combat-shaped action outside combat: {a:?}")
                })
            })
            .collect::<Result<Vec<ActionDesc>, String>>()?;
        let feat = encoder::featurize_with(&obs, &action_descs, self.features);
        self.peak.observe(feat.obs_cat.len(), feat.n_actions);
        let (probs, value) = self.net.infer(&feat).map_err(|e| {
            // GH #315: an export-budget overflow used to report only
            // `obs=… act=…`, and the audit-log path drops its records
            // when the cell panics — so the failing surface could not be
            // named from the outside. Name it here, where the phase,
            // surface and deck size are all in hand.
            format!(
                "PPO net inference failed: {e} \
                 [phase={phase:?} surface={surface:?} deck={} candidates={} relics={} \
                 potions={} floor={}]",
                obs.deck.len(),
                masked.len(),
                obs.relics.len(),
                obs.potions.len(),
                obs.total_floor,
            )
        })?;
        let argmax = if self.group_decode {
            grouped_argmax(&action_descs, &probs)
        } else {
            argmax_first(&probs)
        }
        .expect("probs is non-empty (masked.len() >= 2)");
        Ok(Some((masked, probs, value, argmax)))
    }

    /// Score a decision built from the **bridge wire** rather than a live
    /// `RunManager` — [`super::wire::build_decision`]'s output.
    ///
    /// The advisor lane has no `RunManager` (SpireMentor sends the wire schema
    /// and nothing else), so [`Self::evaluate_with_actions`] cannot serve it.
    /// Everything else is shared: same `featurize`, same tract inference, and
    /// — since the group decode was re-keyed off [`ActionDesc`] — the same A8
    /// §8 [`grouped_argmax`] behind the same `STS2_RUNAI_PPO_GROUP_DECODE`
    /// seam. That last part used to be a deliberate scope cut, and it made
    /// the live-game advisor recommend differently from run_arena/workbench
    /// on any split-mass screen (a shop fans removal out into one candidate
    /// per deck card, so the pooled intent and the biggest single bar are
    /// routinely different actions). The bridge must play the champion the
    /// workbench shows, so the cut is closed.
    ///
    /// Returns `None` when fewer than two candidates survived — there is no
    /// decision to make and no inference worth running, matching
    /// [`Self::evaluate_with_actions`]'s own early-out.
    ///
    /// Unlike the `RunManager` path this **never panics** on an inference
    /// failure: it is on the live-game never-crash path, where the caller
    /// must be able to fall back to its own policy for one decision and carry
    /// on. The error surfaces as `None`.
    #[must_use]
    pub fn evaluate_wire(
        &self,
        decision: &super::wire::WireDecision,
    ) -> Option<(Vec<f32>, f32, usize)> {
        if decision.actions.len() < 2 {
            return None;
        }
        let feat = encoder::featurize_with(&decision.obs, &decision.actions, self.features);
        self.peak.observe(feat.obs_cat.len(), feat.n_actions);
        let (probs, value) = self.net.infer(&feat).ok()?;
        let argmax = if self.group_decode {
            grouped_argmax(&decision.actions, &probs)
        } else {
            argmax_first(&probs)
        }?;
        Some((probs, value, argmax))
    }

    /// Side-effect-free decision pipeline: see [`Self::evaluate_with_actions`].
    /// Delegates to it and drops the masked list — the shape
    /// [`Self::choose`] and the Task 9 parity binding already consume.
    #[must_use]
    pub fn evaluate(
        &self,
        rm: &RunManager,
        actions: &[RunAction],
    ) -> Option<(Vec<f32>, f32, usize)> {
        self.evaluate_with_actions(rm, actions)
            .map(|(_, probs, value, argmax)| (probs, value, argmax))
    }
}

/// First-max argmax: the index of the largest value, ties broken toward
/// the SMALLEST index. `Iterator::max_by`/`max_by_key` return the LAST
/// maximum on ties, but the Python training env's `logp.argmax()`
/// (numpy/torch, `sts2-pycli/python/ppo/net_policy.py`) returns the
/// FIRST — Task 9's Gate C asserts Rust argmax == Python argmax along
/// real trajectories, so this scan must match numpy's tie-break exactly
/// rather than Rust's default-iterator-adaptor one. Since the A8 §8
/// group-mass decode landed, that Gate C invariant holds only under
/// `STS2_RUNAI_PPO_GROUP_DECODE=0` (which the parity suite's `_env`
/// fixture pins); with the default-ON decode the exposed argmax is
/// [`grouped_argmax`], which deliberately diverges from numpy on
/// split-mass screens.
///
/// `NaN` inputs are never expected (softmax outputs). A `NaN` after the
/// first element loses every `>` comparison and is skipped; a `NaN` at
/// index 0 seeds `best` and then wins, since nothing compares greater —
/// which happens to match numpy's argmax (it returns the first `NaN`'s
/// index). Either way a `NaN` here means featurization/inference is
/// already broken upstream.
#[cfg(feature = "ppo")]
#[must_use]
fn argmax_first(xs: &[f32]) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, &x) in xs.iter().enumerate() {
        let is_new_best = match best {
            Some((_, b)) => x > b,
            None => true,
        };
        if is_new_best {
            best = Some((i, x));
        }
    }
    best.map(|(i, _)| i)
}

/// `STS2_RUNAI_PPO_GROUP_DECODE` resolver: default ON, `"0"` restores
/// the plain per-candidate argmax. Any other value (including `"1"`)
/// keeps the group decode — the seam is a rollback switch, not a mode
/// enum, matching the `STS2_RUNAI_CHANCE_NODES=0` style of default-on
/// levers.
#[cfg(feature = "ppo")]
#[must_use]
fn group_decode_enabled(env_value: Option<&str>) -> bool {
    env_value != Some("0")
}

/// Decode key for [`grouped_argmax`]: candidates that share a key are
/// one INTENT and pool their probability mass before the argmax.
///
/// Exactly four candidate shapes are target-only within a screen — the
/// variant fixes the intent (remove / transform / enchant / smith) and
/// the payload only picks WHICH card:
/// [`ActionDesc::ShopRemoveCard`], [`ActionDesc::TransformDeckCard`],
/// [`ActionDesc::EnchantDeckCard`], [`ActionDesc::SmithCard`]. Every
/// other variant stays a singleton (keyed by its own candidate index):
/// `ShopBuyCard { idx: 1 }` vs `{ idx: 2 }` are genuinely different
/// decisions, not one preference split over targets.
///
/// Keyed off [`ActionDesc`], not `RunAction`, so BOTH lanes can decode
/// with the same rule: the `RunManager` lane converts its candidates to
/// `ActionDesc` to featurize them anyway, and the bridge wire lane
/// ([`PpoPolicy::evaluate_wire`]) has nothing else — it never sees a
/// `RunAction`. The deck-mutation kinds this deliberately leaves out
/// (`RemoveDeckCard` / `UpgradeDeckCard` / `GrantDeckCard`, the
/// event-driven select surfaces) are unchanged from the A8 §8 scope;
/// widening the set changes what the deployed champions do and wants its
/// own measurement.
#[cfg(feature = "ppo")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum GroupKey {
    ShopRemove,
    Transform,
    Enchant,
    Smith,
    Singleton(usize),
}

#[cfg(feature = "ppo")]
fn group_key(action: &ActionDesc, idx: usize) -> GroupKey {
    match action {
        ActionDesc::ShopRemoveCard { .. } => GroupKey::ShopRemove,
        ActionDesc::TransformDeckCard { .. } => GroupKey::Transform,
        ActionDesc::EnchantDeckCard { .. } => GroupKey::Enchant,
        ActionDesc::SmithCard { .. } => GroupKey::Smith,
        _ => GroupKey::Singleton(idx),
    }
}

/// Group-mass argmax (A8 audit §8, ranked recommendation 6): pool each
/// [`GroupKey`]'s probability mass, argmax over the pooled masses, then
/// pick the highest-probability member WITHIN the winning group. Both
/// argmaxes are first-max ([`argmax_first`] semantics) so the decode
/// stays deterministic and tie-breaks match the ungrouped path.
///
/// Motivation, measured on the 30-run A8 audit: shop screens emit one
/// `ShopRemoveCard` candidate PER removable card, so a 0.667 total
/// "remove a starter" preference split 5 ways (0.133 each) lost the
/// plain argmax to a single 0.142 `ShopBuyCard` — 11 of 137 scored
/// shop decisions picked an action type with strictly less total mass
/// than a competitor, 8 of them losing to `ShopRemoveCard`. The
/// ZenWeaver control (removal as a single packaged option, taken 5/5)
/// shows the preference is real and only the decode loses it.
///
/// On a screen with no groupable variant every candidate is its own
/// singleton group, so this degrades to exactly [`argmax_first`].
#[cfg(feature = "ppo")]
#[must_use]
fn grouped_argmax(actions: &[ActionDesc], probs: &[f32]) -> Option<usize> {
    debug_assert_eq!(actions.len(), probs.len());
    // First-occurrence order keeps the group argmax's tie-break aligned
    // with the candidate order (and thus with `argmax_first`).
    let mut groups: Vec<(GroupKey, f32)> = Vec::new();
    for (i, action) in actions.iter().enumerate() {
        let key = group_key(action, i);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, mass)) => *mass += probs[i],
            None => groups.push((key, probs[i])),
        }
    }
    let masses: Vec<f32> = groups.iter().map(|&(_, mass)| mass).collect();
    let winner = groups[argmax_first(&masses)?].0;
    let mut best: Option<(usize, f32)> = None;
    for (i, action) in actions.iter().enumerate() {
        // `map_or(true, …)` not `is_none_or`: clippy.toml pins msrv 1.78.
        if group_key(action, i) == winner && best.map_or(true, |(_, b)| probs[i] > b) {
            best = Some((i, probs[i]));
        }
    }
    best.map(|(i, _)| i)
}

#[cfg(feature = "ppo")]
impl RunPolicy for PpoPolicy {
    /// `env.py::observe`'s event surface calls `rm.materialize_event()`,
    /// which needs `&mut RunManager`; `choose` stays `&RunManager` (see
    /// the `RunPolicy::prepare` doc comment), so the event is bound
    /// here instead. Byte-copy of `sts2-py::run_manager.rs`'s
    /// `materialize_event` (~:1451): idempotent (no-op once an event is
    /// bound) and RNG-free (deterministic pick), so calling it on every
    /// non-event decision too costs nothing observable.
    fn prepare(&mut self, rm: &mut RunManager) {
        if rm.phase() == Phase::Event && rm.run_state.current_event.is_none() {
            if let Some(id) = sts2_content::events::dispatch::pick_event(&mut rm.run_state, None) {
                rm.run_state.enter_event_room(id);
            }
        }
    }

    fn choose(&mut self, rm: &RunManager, actions: &[RunAction]) -> Option<usize> {
        let phase = rm.phase();
        assert!(
            phase != Phase::RunOver,
            "PpoPolicy::choose called during {phase:?} — a finished run has no decision to \
             score (unreachable: drive_full_run checks rm.is_over() before calling choose)"
        );
        if phase == Phase::Combat {
            // MCTS-fallback path only. `PpoPolicy` is always deployed as
            // `MctsCombatPolicy<PpoPolicy>`'s inner `meta`
            // (`arena.rs::build_policy`'s `(PolicyKind::Ppo, Some(budget))`
            // arm) — `MctsCombatPolicy::choose` plays every ordinary combat
            // decision itself and only reaches this branch on the rare
            // `FallbackCounts` paths documented on that type (no root
            // outcome / untranslatable action — e.g. Channel, Evoke,
            // multi-pick — / translation drift). The PPO net must never
            // score combat (it was never trained on combat observations,
            // and `evaluate` returns `None` for `Phase::Combat`), so this
            // mirrors the deployed-ranker-cell pattern where the *meta*
            // policy wrapped by `MctsCombatPolicy` is itself
            // combat-capable: delegate to the same deterministic
            // `HeuristicPolicy` combat catch-all
            // (`policy::heuristic`'s "first option, never a panic" combat
            // arm) rather than panicking and corrupting the sweep with an
            // `outcome="panic"` row.
            self.last_eval = Some(MetaEvalRecord {
                source: "heuristic_fallback",
                actions: actions.to_vec(),
                probs: Vec::new(),
                value: None,
                argmax: None,
            });
            return self.heuristic.choose(rm, actions);
        }
        // GH #379: `potion_swap` used to short-circuit to the heuristic
        // here, because it had no `ActionDesc` tag and
        // `evaluate_with_actions` would have panicked. It has one now —
        // `classify_surface` claims the surface, `build_obs` ships the
        // offered potion, and the encoder tokenizes each held potion — so
        // the net owns the decision on both arms instead of the run policy
        // never seeing a surface training now exercises.
        //
        // ORDERING, and it matters: a champion promoted before this
        // surface existed carries an UNTRAINED `potion-swap` embedding row
        // (`vocab::SURFACES`' new tail), so on such a net this path ranks
        // noise. That is not obviously better than the pre-#252 silent
        // drop — swapping a good potion out for a worse one loses more
        // than declining does. Measure such a net on the rollback arm
        // (`STS2_RUNAI_POTION_SWAP=0`, `arena::run_cell`) and only compare
        // arms once a campaign has run with the flag on.
        let masked = mask_rejectable(rm, phase, actions);
        if masked.len() == 1 {
            // Single-remaining-candidate auto-take: no inference, per
            // env.py::_advance's `len(actions) == 1` branch.
            self.last_eval = Some(MetaEvalRecord {
                source: "ppo_auto_take",
                actions: masked.clone(),
                probs: Vec::new(),
                value: None,
                argmax: None,
            });
            return actions.iter().position(|a| *a == masked[0]);
        }
        let (scored, probs, value, argmax) = self
            .evaluate_with_actions(rm, actions)
            .expect("evaluate returns Some for a multi-option non-combat/non-over decision");
        let chosen = scored[argmax];
        self.last_eval = Some(MetaEvalRecord {
            source: "ppo",
            actions: scored,
            probs,
            value: Some(value),
            argmax: Some(argmax),
        });
        actions.iter().position(|a| *a == chosen)
    }
}

#[cfg(feature = "ppo")]
impl VScoreSource for PpoPolicy {}

#[cfg(feature = "ppo")]
impl DecisionTelemetry for PpoPolicy {
    fn take_meta_eval(&mut self) -> Option<MetaEvalRecord> {
        self.last_eval.take()
    }
}

#[cfg(all(test, feature = "ppo"))]
mod argmax_tests {
    use super::argmax_first;

    #[test]
    fn ties_break_to_first_index() {
        assert_eq!(argmax_first(&[0.5, 0.5, 0.1]), Some(0));
    }

    #[test]
    fn strict_max_wins_regardless_of_position() {
        assert_eq!(argmax_first(&[0.1, 0.7, 0.2]), Some(1));
    }

    #[test]
    fn empty_slice_is_none() {
        assert_eq!(argmax_first(&[]), None);
    }
}

#[cfg(all(test, feature = "ppo"))]
mod group_decode_tests {
    use sts2_core::run_manager::RunAction;

    use super::{argmax_first, group_decode_enabled, grouped_argmax, ActionDesc};

    /// The candidate shape the decode actually keys on. Both lanes reach
    /// it: the `RunManager` lane converts (this `TryFrom`, the same call
    /// `evaluate_with_actions` makes to featurize), and the bridge wire
    /// lane builds `ActionDesc` directly. Spelling the fixtures as engine
    /// actions keeps them readable — and pins that the conversion carries
    /// the grouping-relevant variant through.
    fn descs(actions: &[RunAction]) -> Vec<ActionDesc> {
        actions
            .iter()
            .map(|a| ActionDesc::try_from(a).expect("non-combat fixture action"))
            .collect()
    }

    /// The audit's measured shop scenario (A8 audit §8, seed
    /// 1520652879 f2): five `ShopRemoveCard` candidates splitting a
    /// 0.667 "remove a starter" preference lose the plain argmax to a
    /// single 0.142 `ShopBuyCard`. The group decode must pool the
    /// removal mass and pick a removal — specifically the
    /// highest-probability removal target.
    #[test]
    fn shop_removal_mass_beats_single_buy() {
        let actions = [
            RunAction::ShopBuyCard { idx: 4 },
            RunAction::ShopRemoveCard { deck_idx: 0 },
            RunAction::ShopRemoveCard { deck_idx: 1 },
            RunAction::ShopRemoveCard { deck_idx: 2 },
            RunAction::ShopRemoveCard { deck_idx: 3 },
            RunAction::ShopRemoveCard { deck_idx: 4 },
        ];
        let probs = [0.142, 0.130, 0.136, 0.133, 0.133, 0.126];
        // Plain argmax takes the buy — the defect under test.
        assert_eq!(argmax_first(&probs), Some(0));
        // Group decode: removal mass 0.658 > 0.142, then the best
        // member of the winning group (deck_idx 1 at 0.136).
        assert_eq!(grouped_argmax(&descs(&actions), &probs), Some(2));
    }

    /// A screen of only singleton variants (distinct buys + leave) has
    /// nothing to pool — the group decode must equal the plain argmax.
    #[test]
    fn singleton_screen_matches_plain_argmax() {
        let actions = [
            RunAction::ShopBuyCard { idx: 0 },
            RunAction::ShopBuyCard { idx: 1 },
            RunAction::ShopBuyRelic { idx: 0 },
            RunAction::LeaveShop,
        ];
        let probs = [0.2, 0.4, 0.3, 0.1];
        assert_eq!(
            grouped_argmax(&descs(&actions), &probs),
            argmax_first(&probs)
        );
        assert_eq!(grouped_argmax(&descs(&actions), &probs), Some(1));
    }

    /// Within the winning group, equal-probability members tie-break to
    /// the FIRST (matching `argmax_first`'s first-max semantics), so
    /// the decode stays deterministic.
    #[test]
    fn within_group_tie_breaks_to_first_member() {
        let actions = [
            RunAction::LeaveShop,
            RunAction::ShopRemoveCard { deck_idx: 3 },
            RunAction::ShopRemoveCard { deck_idx: 7 },
        ];
        let probs = [0.2, 0.4, 0.4];
        assert_eq!(grouped_argmax(&descs(&actions), &probs), Some(1));
    }

    /// The other three groupable variants pool exactly like
    /// `ShopRemoveCard`: a split transform preference must not lose to
    /// a single competitor with less total mass.
    #[test]
    fn transform_mass_pools_like_removal() {
        let actions = [
            RunAction::EventChoice { idx: 0 },
            RunAction::TransformDeckCard { idx: 0 },
            RunAction::TransformDeckCard { idx: 1 },
            RunAction::TransformDeckCard { idx: 2 },
        ];
        let probs = [0.31, 0.24, 0.23, 0.22];
        assert_eq!(argmax_first(&probs), Some(0));
        assert_eq!(grouped_argmax(&descs(&actions), &probs), Some(1));
    }

    /// `STS2_RUNAI_PPO_GROUP_DECODE` resolver: default (unset) is ON,
    /// `"0"` is the rollback to the plain argmax, anything else stays
    /// ON. The flag routes `evaluate_with_actions` between
    /// `grouped_argmax` and `argmax_first`, whose divergence on the
    /// audit scenario is asserted above.
    #[test]
    fn seam_defaults_on_and_zero_disables() {
        assert!(group_decode_enabled(None));
        assert!(!group_decode_enabled(Some("0")));
        assert!(group_decode_enabled(Some("1")));
    }
}

#[cfg(all(test, feature = "ppo"))]
mod combat_fallback_tests {
    use std::sync::Arc;

    use sts2_content::run_callbacks::new_run_manager_for_character;
    use sts2_core::full_run::RunPolicy;
    use sts2_core::potions::CharacterId;

    use super::{HeuristicPolicy, Phase, PpoNet, PpoPolicy};

    /// Regression for the critical fallback-panic finding: when
    /// `MctsCombatPolicy`'s combat search can't translate a chosen action
    /// (its `FallbackCounts` paths — see `policy::mod`'s doc comment), it
    /// hands the decision to the wrapped meta policy EVEN mid-combat.
    /// `PpoPolicy::choose` used to `assert!(phase != Combat)` there, which
    /// would panic and corrupt a sweep row with `outcome="panic"`. It must
    /// now delegate to the embedded `HeuristicPolicy` and return a valid
    /// action index instead.
    #[test]
    fn combat_phase_delegates_to_heuristic_without_panicking() {
        let Some(path) = super::super::net::onnx_model_path() else {
            eprintln!("skip: no ONNX model on this checkout");
            return;
        };
        let net = PpoNet::load(&path).unwrap_or_else(|e| panic!("load onnx: {e}"));
        let mut policy = PpoPolicy {
            net: Arc::new(net),
            // The committed champions are all at the default schema; pinning
            // it here (rather than reading the environment) keeps this test
            // independent of whatever STS2_PPO_FEATURES the runner exports.
            features: crate::ppo::features::Features::default(),
            heuristic: HeuristicPolicy::new(),
            peak: super::TokenPeak::default(),
            last_eval: None,
            group_decode: true,
        };

        // Warm up to Phase::Combat with the heuristic (never the policy
        // under test, so we're not begging the question) — the same
        // idiom `policy::heuristic`'s `combat_choice_exhausts_curse_*`
        // test uses.
        let mut rm = new_run_manager_for_character(3, CharacterId::Ironclad);
        let mut warmup = HeuristicPolicy::new();
        let mut guard = 0;
        while rm.phase() != Phase::Combat {
            let actions = rm.get_available_actions();
            let idx = warmup.choose(&rm, &actions).expect("warmup pick");
            let _ = rm.take_action(actions[idx]);
            guard += 1;
            assert!(guard < 300, "never reached combat");
        }

        let actions = rm.get_available_actions();
        assert!(!actions.is_empty(), "combat must surface actions");
        let idx = policy
            .choose(&rm, &actions)
            .expect("combat fallback must return a pick, not None");
        assert!(
            idx < actions.len(),
            "returned index {idx} out of bounds for {} actions",
            actions.len()
        );
    }
}
