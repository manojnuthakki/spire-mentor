//! `sts2_rs.mcts_recommend` / `sts2_rs.mcts_search_tree` — the search result
//! dicts, built once here for both the `PyO3` module and the browser
//! workbench (`sts2-workbench::advisors`).
//!
//! The Python hint adapter (`sts2_hint.recommender.RecommendConfig`) and the
//! workbench's `reasoning.py` feed these from a loose `config` dict; the
//! browser feeds the same dict straight from the SPA. [`RecommendParams`]
//! is the one parser (same defaults, unknown keys dropped, the same
//! rejections for a bad `rollout_policy` / `hidden_info`), so a config
//! produces the same [`MctsConfig`] on every host.
//!
//! Everything here is single-process; the `threads > 1` root-parallel path
//! is `sts2_mcts::par_search_with_policy` (rayon) and callers that cannot
//! spawn threads — the wasm build — clamp `threads` to 1 before calling.

use serde_json::{json, Map, Value};
use sts2_core::{apply, legal_actions, Action, CombatState};
use sts2_mcts::{
    dpw_outcome_cap, hint, par_search_with_policy, EdgeAction, HiddenInfoMode, LeafEval, Mcts,
    MctsConfig, NodeKind, RolloutPolicyKind, RootOutcome, SharedPolicy,
};

use super::combat::combat_action_to_value;

/// Q-saturation epsilon: if the spread of root-child mean values is below
/// this, the rollout policy can't distinguish actions and any ranking is
/// essentially noise. Matches the threshold used by the CLI hint formatter
/// (`sts2-cli/src/agent/human.rs`, `q_saturated`).
pub const Q_SATURATION_EPSILON: f32 = 1e-3;

/// Low-confidence visit-fraction threshold: any root child with visits
/// below `LOW_CONFIDENCE_VISIT_FRACTION * max_visits` is flagged. Mirrors
/// `LOW_CONFIDENCE_VISIT_FRACTION` in `sts2-cli/src/agent/human.rs`.
pub const LOW_CONFIDENCE_VISIT_FRACTION: f64 = 0.05;

/// Hard cap on the number of actions reported in the principal variation
/// returned by [`recommend_value`]. Matches the spec ("depth ≤ 5").
pub const PV_MAX_DEPTH: usize = 5;

/// `RecommendConfig` (`sts2_hint/recommender.py`): the knobs of one
/// recommend / search-tree call, with that class's defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecommendParams {
    pub sims: u32,
    pub depth: u32,
    pub c_puct: f32,
    pub seed: u64,
    /// Root-parallel workers (`> 1` needs a thread pool).
    pub threads: u32,
    /// Per-action independent searches instead of one root search.
    pub fair: bool,
    pub rollout_policy: RolloutPolicyKind,
    pub hidden_info: HiddenInfoMode,
    /// `None` keeps the mode default (`MctsConfig::default().draw_widening_c`).
    pub draw_widening_c: Option<f32>,
    /// Explicit EndTurn DPW coefficient (an ablation knob). `None` keeps what
    /// the mode deploys ([`MctsConfig::apply_mode_dialect`]).
    pub endturn_widening_c: Option<f32>,
    /// Explicit EndTurn DPW exponent; `None` keeps the mode's.
    pub endturn_widening_alpha: Option<f32>,
    pub common_random_numbers: bool,
}

impl Default for RecommendParams {
    fn default() -> Self {
        Self {
            sims: 300,
            depth: 200,
            c_puct: sts2_mcts::belief_draw_c_puct(300),
            seed: 0,
            threads: 1,
            fair: false,
            rollout_policy: RolloutPolicyKind::Heuristic,
            // The DEPLOYED honest search, not the leaky one. These three are
            // one setting: see `resolve_mode_constants`.
            hidden_info: HiddenInfoMode::BeliefDraw,
            draw_widening_c: Some(sts2_mcts::BELIEF_DRAW_DRAW_WIDENING_C),
            endturn_widening_c: None,
            endturn_widening_alpha: None,
            common_random_numbers: false,
        }
    }
}

/// `rollout_policy` string → kind; the same two spellings `mcts_recommend`
/// accepts, the same rejection for anything else.
pub fn parse_rollout_policy(s: &str, fn_name: &str) -> Result<RolloutPolicyKind, String> {
    match s {
        "heuristic" => Ok(RolloutPolicyKind::Heuristic),
        "uniform" => Ok(RolloutPolicyKind::Uniform),
        other => Err(format!(
            "{fn_name}: rollout_policy must be 'heuristic' or 'uniform', got {other:?}"
        )),
    }
}

/// `hidden_info` string → mode. Rejects an unrecognized value rather than
/// falling back: a typo in an A/B arm's config must fail loudly, not
/// silently run the wrong search.
pub fn parse_hidden_info(s: &str, fn_name: &str) -> Result<HiddenInfoMode, String> {
    match s {
        "clairvoyant" => Ok(HiddenInfoMode::Clairvoyant),
        "beliefdraw" => Ok(HiddenInfoMode::BeliefDraw),
        "drawchance" => Ok(HiddenInfoMode::DrawChance),
        "rootscramble" => Ok(HiddenInfoMode::RootScramble),
        other => Err(format!(
            "{fn_name}: hidden_info must be one of \
             'clairvoyant'|'beliefdraw'|'drawchance'|'rootscramble', got {other:?}"
        )),
    }
}

fn num_u32(v: &Value) -> Option<u32> {
    v.as_u64().and_then(|n| u32::try_from(n).ok()).or_else(|| {
        v.as_f64()
            .filter(|f| f.is_finite() && *f >= 0.0)
            .map(|f| f as u32)
    })
}

fn num_u64(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_i64().map(|n| n as u64))
        .or_else(|| v.as_f64().filter(|f| f.is_finite()).map(|f| f as u64))
}

fn num_f32(v: &Value) -> Option<f32> {
    v.as_f64().map(|f| f as f32)
}

impl RecommendParams {
    /// `RecommendConfig.from_dict`: read the known keys of a loose config
    /// dict, keep the defaults for the rest, ignore everything unknown
    /// (older frontends still send `chance_nodes`, `log_path`, …).
    /// `fn_name` labels the error for a bad `rollout_policy` / `hidden_info`.
    pub fn from_config(config: &Value, fn_name: &str) -> Result<Self, String> {
        let mut p = Self::default();
        let Some(obj) = config.as_object() else {
            return Ok(p);
        };
        if let Some(v) = obj.get("sims").and_then(num_u32) {
            p.sims = v;
        }
        if let Some(v) = obj.get("depth").and_then(num_u32) {
            p.depth = v;
        }
        if let Some(v) = obj.get("c_puct").and_then(num_f32) {
            p.c_puct = v;
        }
        if let Some(v) = obj.get("seed").and_then(num_u64) {
            p.seed = v;
        }
        if let Some(v) = obj.get("threads").and_then(num_u32) {
            p.threads = v;
        }
        if let Some(v) = obj.get("fair").and_then(Value::as_bool) {
            p.fair = v;
        }
        if let Some(v) = obj.get("rollout_policy").and_then(Value::as_str) {
            p.rollout_policy = parse_rollout_policy(v, fn_name)?;
        }
        if let Some(v) = obj.get("hidden_info").and_then(Value::as_str) {
            p.hidden_info = parse_hidden_info(v, fn_name)?;
        }
        let explicit_draw_widening = matches!(obj.get("draw_widening_c"), Some(v) if !v.is_null());
        match obj.get("draw_widening_c") {
            Some(Value::Null) | None => {}
            Some(v) => p.draw_widening_c = num_f32(v),
        }
        if let Some(v) = obj.get("endturn_widening_c").and_then(num_f32) {
            p.endturn_widening_c = Some(v);
        }
        if let Some(v) = obj.get("endturn_widening_alpha").and_then(num_f32) {
            p.endturn_widening_alpha = Some(v);
        }
        if let Some(v) = obj.get("common_random_numbers").and_then(Value::as_bool) {
            p.common_random_numbers = v;
        }
        p.resolve_mode_constants(
            obj.get("c_puct").and_then(num_f32).is_some(),
            explicit_draw_widening,
        );
        Ok(p)
    }

    /// Re-derive the constants that TRAVEL WITH `hidden_info`, unless the
    /// caller pinned them.
    ///
    /// `c_puct` and `draw_widening_c` are not free knobs: the deployed
    /// BeliefDraw honest search sets them from the budget
    /// ([`MctsConfig::apply_mode_dialect`] — the F2 retune, bead
    /// `sts2-rs-09x5n`). Pairing BeliefDraw with the engine's `SQRT_2` /
    /// no-widening numbers is a MISTUNED search, not a neutral one, and the
    /// constant depends on `sims`, which callers set independently. So a
    /// config that names a mode or a budget without naming the constants gets
    /// the constants that mode deploys at that budget. (The EndTurn pair
    /// needs no resolving here: `None` defers to the dialect in
    /// [`Self::mcts_config`].)
    ///
    /// Every other mode keeps the historical 1.4 / no widening byte-identical.
    fn resolve_mode_constants(&mut self, keep_c_puct: bool, keep_draw_widening: bool) {
        let honest = self.hidden_info == HiddenInfoMode::BeliefDraw;
        let mut dialect = MctsConfig {
            hidden_info: self.hidden_info,
            max_simulations: self.sims,
            ..MctsConfig::default()
        };
        dialect.apply_mode_dialect();
        if !keep_c_puct {
            self.c_puct = if honest { dialect.c_puct } else { 1.4 };
        }
        if !keep_draw_widening {
            self.draw_widening_c = honest.then_some(dialect.draw_widening_c);
        }
    }

    /// The search config `mcts_recommend` / `mcts_search_tree` build: the
    /// mode's deployed dialect ([`MctsConfig::apply_mode_dialect`], the same
    /// call the run agent's `sts2-runai::policy::mcts_config_for` makes), the
    /// deployed-search knobs pinned (no transposition table, regularized π
    /// temperature 0.1, uniform PUCT prior), then this call's parameters —
    /// an explicit `c_puct` / widening value wins over the dialect.
    #[must_use]
    pub fn mcts_config(&self) -> MctsConfig {
        let mut cfg = MctsConfig {
            max_simulations: self.sims,
            max_rollout_depth: self.depth,
            seed: self.seed,
            rollout_policy: self.rollout_policy,
            use_transposition_table: false,
            regularized_pi_temperature: 0.1,
            use_puct_prior: false,
            hidden_info: self.hidden_info,
            common_random_numbers: self.common_random_numbers,
            ..MctsConfig::default()
        };
        cfg.apply_mode_dialect();
        cfg.c_puct = self.c_puct;
        cfg.draw_widening_c = self
            .draw_widening_c
            .unwrap_or(MctsConfig::default().draw_widening_c);
        if let Some(c) = self.endturn_widening_c {
            cfg.endturn_widening_c = c;
        }
        if let Some(alpha) = self.endturn_widening_alpha {
            cfg.endturn_widening_alpha = alpha;
        }
        cfg
    }
}

/// Snapshot of one root child outcome used by both the `fair=False`
/// (single search) and `fair=True` (per-action independent search) paths
/// of [`recommend_value`]. Decoupled from [`RootOutcome`] so the fair-eval
/// branch can synthesize terminal outcomes (e.g. a lethal playable already
/// kills the only enemy) as if they had been rolled out once.
#[derive(Clone, Debug)]
pub struct RecommendScore {
    pub action: Action,
    pub visits: u32,
    pub wins: u32,
    pub losses: u32,
    pub ties: u32,
    /// Sum of continuous leaf values; `value_sum / visits` is the Q used
    /// for action ranking.
    pub value_sum: f32,
    /// This action ends the combat in a WIN immediately (fair-eval only).
    /// Ranked above a merely equal-valued continuation (GH#296).
    pub terminal_win: bool,
}

impl From<RootOutcome> for RecommendScore {
    fn from(o: RootOutcome) -> Self {
        Self {
            action: o.action,
            visits: o.visits,
            wins: o.wins,
            losses: o.losses,
            ties: o.ties,
            value_sum: o.value_sum,
            terminal_win: false,
        }
    }
}

/// Q = `value_sum / visits` from the player's perspective (0 for an
/// unvisited action).
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn q_of(s: &RecommendScore) -> f32 {
    if s.visits == 0 {
        0.0
    } else {
        s.value_sum / s.visits as f32
    }
}

/// Sum `(visits, wins, losses, ties, value_sum)` across every root child of
/// a post-action search. The per-child counters sum to the root node's own
/// counters because every simulation backpropagates through the root.
#[must_use]
pub fn aggregate_root(outs: &[RootOutcome]) -> (u32, u32, u32, u32, f32) {
    let mut v = 0u32;
    let mut w = 0u32;
    let mut l = 0u32;
    let mut t = 0u32;
    let mut vs = 0.0_f32;
    for o in outs {
        v += o.visits;
        w += o.wins;
        l += o.losses;
        t += o.ties;
        vs += o.value_sum;
    }
    (v, w, l, t, vs)
}

/// One per-action score dict (`mcts_recommend()["actions"][i]`).
#[must_use]
#[allow(clippy::too_many_arguments, clippy::cast_precision_loss)]
pub fn action_score_value(
    action: &Action,
    visits: u32,
    wins: u32,
    losses: u32,
    ties: u32,
    value_sum: f32,
    low_confidence: bool,
    q_saturated: bool,
    is_best: bool,
) -> Value {
    let vdenom = f64::from(visits.max(1));
    let p_win = f64::from(wins) / vdenom;
    let p_tie = f64::from(ties) / vdenom;
    let p_loss = f64::from(losses) / vdenom;
    // Legacy `value` field — bucket-derived mean, kept for the CLI's
    // `LIKELY-WIN n%` display + parity with pre-v2oh consumers.
    let q_bucket = if visits == 0 {
        0.0_f32
    } else {
        (wins as f32 - losses as f32) / visits as f32
    };
    // Continuous Q from the lightspeed-style reward: what action ranking
    // actually consumes.
    let q_cont = if visits == 0 {
        0.0_f32
    } else {
        value_sum / visits as f32
    };
    #[allow(clippy::cast_possible_truncation)]
    let bucket = hint::win_prob_bucket(p_win as f32);
    json!({
        "action": combat_action_to_value(action),
        "visits": visits,
        "value": q_bucket,
        "value_sum": value_sum,
        "q": q_cont,
        "wins": wins,
        "losses": losses,
        "ties": ties,
        "p_win": p_win,
        "p_tie": p_tie,
        "p_loss": p_loss,
        "bucket": bucket,
        "low_confidence": low_confidence,
        "q_saturated": q_saturated,
        "is_best": is_best,
    })
}

/// Rank `scores` and build the `mcts_recommend` result dict:
/// `{actions, pv, q_saturated, fair, sims, threads, intent_damage_known}`.
///
/// Sort: Q desc, then a decisive WIN, then visits desc (matches
/// `human.rs` plus the GH#296 terminal-win tie-break). The top row is
/// `is_best`; rows below 5% of the max visits are `low_confidence`;
/// `q_saturated` when the Q spread across >1 rows is under
/// [`Q_SATURATION_EPSILON`].
#[must_use]
pub fn recommend_output_value(
    mut scores: Vec<RecommendScore>,
    pv: &[Action],
    fair: bool,
    sims: u32,
    threads: u32,
    intent_damage_known: bool,
) -> Value {
    scores.sort_by(|a, b| {
        let qa = q_of(a);
        let qb = q_of(b);
        qb.partial_cmp(&qa)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.terminal_win.cmp(&a.terminal_win))
            .then(b.visits.cmp(&a.visits))
    });
    let q_saturated = if scores.len() > 1 {
        let q_min = scores.iter().map(q_of).fold(f32::INFINITY, f32::min);
        let q_max = scores.iter().map(q_of).fold(f32::NEG_INFINITY, f32::max);
        (q_max - q_min).abs() < Q_SATURATION_EPSILON
    } else {
        false
    };
    let max_visits = scores.iter().map(|s| s.visits).max().unwrap_or(0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let low_visit_threshold =
        ((f64::from(max_visits) * LOW_CONFIDENCE_VISIT_FRACTION).ceil() as u32).max(1);
    let actions: Vec<Value> = scores
        .iter()
        .enumerate()
        .map(|(idx, s)| {
            action_score_value(
                &s.action,
                s.visits,
                s.wins,
                s.losses,
                s.ties,
                s.value_sum,
                s.visits < low_visit_threshold,
                q_saturated,
                idx == 0,
            )
        })
        .collect();
    let pv: Vec<Value> = pv.iter().map(combat_action_to_value).collect();
    json!({
        "actions": actions,
        "pv": pv,
        "q_saturated": q_saturated,
        "fair": fair,
        "sims": sims,
        "threads": threads,
        "intent_damage_known": intent_damage_known,
    })
}

/// How many root determinizations a fair evaluation spreads each action's
/// budget over under an honest hidden-information mode (sts2-agent fork).
pub const FAIR_ROOT_SAMPLES: u32 = 16;

/// Salt deriving a fair evaluation's per-determinization seeds from the
/// search seed (distinct from every other splitmix64 salt in the engine).
const FAIR_ROOT_SALT: u64 = 0xF00D_BEEF_CAFE_00F1;

/// The post-action states a fair evaluation searches for `action`, each with
/// its search seed and budget. Under `BeliefDraw`: [`FAIR_ROOT_SAMPLES`]
/// determinizations of the root (each a clone made a concrete sampled world
/// by [`sts2_core::blindfold::blindfold_root`]: the unknown draw middle
/// shuffled, pins kept, the combat streams reseeded, from a seed derived from
/// the search seed and the sample's index alone), the action applied to each,
/// the budget split evenly. Otherwise the action applied to
/// the state itself, with the whole budget.
#[must_use]
pub fn fair_root_samples(state: &CombatState, action: &Action, params: &RecommendParams) -> Vec<(CombatState, u64, u32)> {
    let honest = params.hidden_info == HiddenInfoMode::BeliefDraw;
    let samples = if honest { FAIR_ROOT_SAMPLES.min(params.sims.max(1)) } else { 1 };
    (0..samples)
        .map(|k| {
            let budget = params.sims / samples + u32::from(k < params.sims % samples);
            let mut post = state.clone();
            let mut seed = params.seed;
            if honest {
                seed = sts2_core::run_rng_set::splitmix64(
                    params.seed ^ FAIR_ROOT_SALT ^ u64::from(k).wrapping_mul(0x9E37_79B9_7F4A_7C15),
                );
                // A concrete sampled world: the unknown draw middle canonicalized, then shuffled
                // from the sample's seed alone, pins kept, every combat stream reseeded
                // (`blindfold_root`). `cards::shuffle` keeps a belief pile's order, so the belief
                // flag goes first: an action that reads the pile's physical top (Havoc's autoplay)
                // then plays a sampled card, not the canonically first (review F3 of A4d, round 2).
                // The search after the action keeps its own belief root.
                post.player.deferred_draw_order = false;
                sts2_core::blindfold::blindfold_root(&mut post, seed);
            }
            let _ = apply(&mut post, action.clone());
            (post, seed, budget)
        })
        .collect()
}

/// One action's fair score: an independent search after the action.
///
/// Under an honest mode (`BeliefDraw`, sts2-agent fork) the action itself
/// must not see hidden information either: a draw it makes, a target or a
/// generated card it rolls, are outcomes the player can't know. So the
/// action's budget is split over [`FAIR_ROOT_SAMPLES`] determinizations of
/// the ROOT: each a clone made a concrete sampled world
/// ([`sts2_core::blindfold::blindfold_root`]: the unknown draw-pile middle
/// shuffled, pins kept, every combat RNG stream reseeded, from the search
/// seed and the sample's index alone; see [`fair_root_samples`]), the action
/// applied to it, then its own search with its share of the budget. The
/// samples are the same for every action (common random numbers across
/// actions). An action that ends the combat in a sample counts as that
/// sample's share of outcomes, so terminal and continuing samples weigh by
/// their probability. Before this the action was applied to the state
/// itself: a root draw took the true next card, a root roll the state's
/// real dice (review F3 of A4d). Other modes keep the single application.
fn fair_action_score(
    state: &CombatState,
    action: &Action,
    params: &RecommendParams,
    cfg: &MctsConfig,
    policy: &SharedPolicy,
) -> RecommendScore {
    let honest = params.hidden_info == HiddenInfoMode::BeliefDraw;
    let mut score = RecommendScore {
        action: action.clone(),
        visits: 0,
        wins: 0,
        losses: 0,
        ties: 0,
        value_sum: 0.0,
        terminal_win: true,
    };
    for (post, seed, budget) in fair_root_samples(state, action, params) {
        let mut kcfg = *cfg;
        if honest {
            kcfg.max_simulations = budget.max(1);
            kcfg.seed = seed;
        }
        if legal_actions(&post).is_empty() {
            // Terminal in this sample: one outcome weighted by the sample's
            // share of the budget, on the SAME scale as the sibling searches
            // (`cfg.terminal_eval_mode`, GH#296).
            let v = sts2_mcts::terminal_state_value(
                &post,
                kcfg.terminal_eval_mode,
                &LeafEval::default(),
                kcfg.loss_gradient,
            );
            let n = if honest { budget.max(1) } else { 1 };
            score.visits += n;
            if v > 0.5 {
                score.wins += n;
            } else if v < -0.5 {
                score.losses += n;
            } else {
                score.ties += n;
            }
            #[allow(clippy::cast_precision_loss)]
            {
                score.value_sum += v * n as f32;
            }
            score.terminal_win &= post.player_won;
            continue;
        }
        score.terminal_win = false;
        let (visits, wins, losses, ties, value_sum) = if params.threads <= 1 {
            let mut m = Mcts::with_policy(post, kcfg, policy.clone());
            let _ = m.search();
            aggregate_root(&m.root_outcomes())
        } else {
            let r = par_search_with_policy(post, kcfg, params.threads, policy.clone(), LeafEval::default());
            aggregate_root(&r.outcomes)
        };
        score.visits += visits;
        score.wins += wins;
        score.losses += losses;
        score.ties += ties;
        score.value_sum += value_sum;
    }
    score
}

/// Run the search `mcts_recommend` runs and return its result dict.
///
/// `fair=false` (default): ONE search from `state`, root child counters per
/// action, PV from the live tree (single-threaded) or just the best action
/// (root-parallel). `fair=true`: per legal action, apply it and run an
/// INDEPENDENT search from the post-action state; an already-terminal
/// post-action state is scored as one synthesized outcome. `threads > 1`
/// uses the rayon root-parallel search — never on wasm.
///
/// `intent_damage_known` is the wire-side trust flag the `PyO3` state
/// carries (whether the enemy intents' damage was known when the state was
/// built); a native engine state passes `true`.
#[must_use]
pub fn recommend_value(
    state: &CombatState,
    params: &RecommendParams,
    intent_damage_known: bool,
) -> Value {
    let cfg = params.mcts_config();
    let policy = SharedPolicy::uniform();
    let legal = legal_actions(state);
    let (scores, pv): (Vec<RecommendScore>, Vec<Action>) = if legal.is_empty() {
        (Vec::new(), Vec::new())
    } else if params.fair {
        let mut scores: Vec<RecommendScore> = Vec::with_capacity(legal.len());
        for action in &legal {
            scores.push(fair_action_score(state, action, params, &cfg, &policy));
        }
        // No PV for the fair case — each action has its own tree.
        (scores, Vec::new())
    } else if params.threads <= 1 {
        let mut m = Mcts::with_policy(state.clone(), cfg, policy);
        let _ = m.search();
        let outs = m.root_outcomes();
        let pv: Vec<Action> = m.get_pv().into_iter().take(PV_MAX_DEPTH).collect();
        (outs.into_iter().map(RecommendScore::from).collect(), pv)
    } else {
        let r = par_search_with_policy(
            state.clone(),
            cfg,
            params.threads,
            policy,
            LeafEval::default(),
        );
        let scores = r.outcomes.into_iter().map(RecommendScore::from).collect();
        // Merged counters but no shared tree — the PV is just the best action.
        (scores, vec![r.best_action])
    };
    recommend_output_value(
        scores,
        &pv,
        params.fair,
        params.sims,
        params.threads,
        intent_damage_known,
    )
}

/// Terse description of the action on a tree edge, derived purely from the
/// stable [`EdgeAction`] (no per-node state needed). Returns
/// `(kind, label, target_index)`. `PlayCard` labels by card identity (the
/// `CardId` debug name + `+` when upgraded); `Other` variants map to the same
/// `kind` strings [`combat_action_to_value`] uses, with a compact human label.
#[must_use]
pub fn edge_descr(edge: &EdgeAction) -> (String, String, Option<u8>) {
    match edge {
        EdgeAction::PlayCard {
            card_id,
            upgraded,
            target,
            ..
        } => {
            let mark = if *upgraded { "+" } else { "" };
            ("play".to_string(), format!("{card_id:?}{mark}"), *target)
        }
        EdgeAction::Other(a) => match a {
            Action::EndTurn => ("end_turn".to_string(), "End turn".to_string(), None),
            Action::PlayCard { hand_index, target } => {
                ("play".to_string(), format!("card[{hand_index}]"), *target)
            }
            Action::UsePotion { slot, target } => {
                ("potion".to_string(), format!("Potion s{slot}"), *target)
            }
            Action::DrinkPotion {
                potion_slot,
                target,
            } => (
                "drink_potion".to_string(),
                format!("Potion s{potion_slot}"),
                *target,
            ),
            Action::Channel { orb, target } => {
                ("channel".to_string(), format!("Channel {orb:?}"), *target)
            }
            Action::Evoke { times } => ("evoke".to_string(), format!("Evoke x{times}"), None),
            Action::ResolveChoice { selection } => (
                "choose".to_string(),
                format!("Choose {}", selection.first().copied().unwrap_or(0)),
                None,
            ),
        },
    }
}

/// Run a single (non-fair, single-tree) search and serialize the **entire
/// search tree** for the inspector: `{nodes, root: 0, pv, sims, c_puct}`.
///
/// Each node carries `id, parent, visits, q (= value_sum/visits),
/// value_sum, wins/losses/ties, is_terminal, proven_win, proven_loss,
/// children[], the incoming edge (kind/label/target_index)`, the chance-node
/// metadata (`node_kind ∈ {"decision","chance"}`, `chance_action`,
/// `outcome_cap`, `chance_draws`, `chance_closed`, `outcome_ordinal`) and a
/// `summary` of the state after the node's action. For a chance node
/// `children` are its sampled outcomes (probability = `child.visits /
/// node.visits`) and `q` is the expectimax value. `threads` / `fair` in
/// `params` are ignored: the tree is single by construction.
#[must_use]
pub fn search_tree_value(state: &CombatState, params: &RecommendParams) -> Value {
    let cfg = params.mcts_config();
    let mut nodes: Vec<Value> = Vec::new();
    let mut pv: Vec<Action> = Vec::new();
    if !legal_actions(state).is_empty() {
        let mut m = Mcts::with_policy(state.clone(), cfg, SharedPolicy::uniform());
        let _ = m.search();
        pv = m.get_pv().into_iter().take(PV_MAX_DEPTH).collect();
        let arena = &m.tree.arena;
        // Incoming-edge labels, stamped from each parent: decision parents
        // label the edge with the chosen action; chance parents label it as
        // an "outcome" carrying the DPW ordinal.
        let mut edges: Vec<(Option<String>, Option<String>, Option<u8>, Option<u32>)> =
            vec![(None, None, None, None); arena.len()];
        for n in arena {
            for (edge, cid) in &n.children {
                let (kind, label, tgt) = edge_descr(edge);
                edges[cid.0 as usize] = (Some(kind), Some(label), tgt, None);
            }
            for outcome in &n.chance_outcomes {
                edges[outcome.child.0 as usize] = (
                    Some("outcome".to_string()),
                    Some(format!("outcome #{}", outcome.n)),
                    None,
                    Some(outcome.n),
                );
            }
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        for (i, n) in arena.iter().enumerate() {
            let is_chance = matches!(n.kind, NodeKind::Chance);
            // A chance node's subtree lives in `chance_outcomes`, NOT
            // `children` (empty for chance nodes); flatten both so the
            // serialized tree stays connected.
            let children: Vec<u32> = if is_chance {
                n.chance_outcomes.iter().map(|o| o.child.0).collect()
            } else {
                n.children.iter().map(|(_, cid)| cid.0).collect()
            };
            // DPW outcome cap at the node's current visits — the engine's
            // own class selection (EndTurn > Draw > Default) and formula.
            let outcome_cap = if is_chance {
                dpw_outcome_cap(&cfg, n)
            } else {
                0
            };
            let q = if n.visits > 0 {
                n.value_sum / n.visits as f32
            } else {
                0.0
            };
            let (kind, label, target_index, outcome_ordinal) = edges[i].clone();
            let enemies: Vec<Value> = n
                .snapshot
                .enemies
                .iter()
                .map(|e| json!({"uid": e.uid, "hp": e.current_hp, "max_hp": e.max_hp}))
                .collect();
            let mut d = Map::new();
            d.insert("id".into(), json!(i as u32));
            d.insert("parent".into(), json!(n.parent.map(|p| p.0)));
            d.insert("visits".into(), json!(n.visits));
            d.insert("q".into(), json!(q));
            d.insert("value_sum".into(), json!(n.value_sum));
            d.insert("wins".into(), json!(n.wins));
            d.insert("losses".into(), json!(n.losses));
            d.insert("ties".into(), json!(n.ties));
            d.insert("is_terminal".into(), json!(n.is_terminal));
            d.insert("proven_win".into(), json!(n.proven_win));
            d.insert("proven_loss".into(), json!(n.proven_loss));
            d.insert("children".into(), json!(children));
            d.insert("kind".into(), json!(kind));
            d.insert("label".into(), json!(label));
            d.insert("target_index".into(), json!(target_index));
            d.insert(
                "node_kind".into(),
                json!(if is_chance { "chance" } else { "decision" }),
            );
            d.insert(
                "chance_action".into(),
                json!(n.chance_action.as_ref().map(|a| edge_descr(a).1)),
            );
            d.insert("outcome_cap".into(), json!(outcome_cap));
            d.insert("chance_draws".into(), json!(n.chance_draws));
            d.insert("chance_closed".into(), json!(n.chance_closed));
            d.insert("outcome_ordinal".into(), json!(outcome_ordinal));
            d.insert(
                "summary".into(),
                json!({
                    "player_hp": n.snapshot.player_hp,
                    "player_max_hp": n.snapshot.player_max_hp,
                    "player_block": n.snapshot.player_block,
                    "energy": n.snapshot.energy,
                    "round": n.snapshot.round,
                    "enemies": enemies,
                }),
            );
            nodes.push(Value::Object(d));
        }
    }
    let pv: Vec<Value> = pv.iter().map(combat_action_to_value).collect();
    json!({
        "nodes": nodes,
        "root": 0u32,
        "pv": pv,
        "sims": params.sims,
        "c_puct": params.c_puct,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_config_keeps_defaults_and_drops_unknown_keys() {
        let p = RecommendParams::from_config(
            &json!({"chance_nodes": true, "log_path": "/x", "sims": 64}),
            "t",
        )
        .unwrap();
        assert_eq!(p.sims, 64);
        assert_eq!(p.depth, 200);
        assert_eq!(p.threads, 1);
        assert!(!p.fair);
        // The DEPLOYED honest search is the default, and its two paired
        // constants follow the mode and the budget rather than being pinned:
        // 64 sims sits below the 500 step, so `c_puct` is 0.8, not 2.5.
        assert_eq!(p.hidden_info, HiddenInfoMode::BeliefDraw);
        assert!((p.c_puct - sts2_mcts::belief_draw_c_puct(64)).abs() < 1e-6);
        assert_eq!(p.draw_widening_c, Some(2.0));
        // A non-object config is the defaults.
        assert_eq!(
            RecommendParams::from_config(&Value::Null, "t").unwrap(),
            RecommendParams::default()
        );
    }

    #[test]
    fn from_config_reads_the_deployed_honest_triple() {
        let p = RecommendParams::from_config(
            &json!({"sims": 2000, "c_puct": 2.5, "hidden_info": "beliefdraw",
                    "draw_widening_c": 2.0, "threads": 1, "seed": 7}),
            "t",
        )
        .unwrap();
        assert_eq!(p.hidden_info, HiddenInfoMode::BeliefDraw);
        assert_eq!(p.draw_widening_c, Some(2.0));
        assert_eq!(p.seed, 7);
        let cfg = p.mcts_config();
        assert_eq!(cfg.max_simulations, 2000);
        assert!((cfg.draw_widening_c - 2.0).abs() < 1e-6);
        assert!(!cfg.use_puct_prior);
        assert!(!cfg.use_transposition_table);
        // `null` keeps the MODE default, which is now 2.0 because the default
        // mode is BeliefDraw and the widening constant travels with it.
        let p = RecommendParams::from_config(&json!({"draw_widening_c": null}), "t").unwrap();
        assert_eq!(p.draw_widening_c, Some(2.0));
        // Naming a non-honest mode without the constants restores the engine
        // defaults byte-identically — the F2 retune is BeliefDraw-only.
        let p = RecommendParams::from_config(&json!({"hidden_info": "clairvoyant"}), "t").unwrap();
        assert_eq!(p.draw_widening_c, None);
        assert!((p.c_puct - 1.4).abs() < 1e-6);
        assert!(
            (p.mcts_config().draw_widening_c - MctsConfig::default().draw_widening_c).abs() < 1e-6
        );
    }

    /// GH #758: the EndTurn pair follows the mode's deployed dialect (the run
    /// agent's PR #467 retune under BeliefDraw), an explicit value wins, and
    /// every other mode keeps the engine's 0.5 / 0.4.
    #[test]
    fn endturn_widening_follows_the_mode_unless_pinned() {
        let (dc, dalpha) = sts2_mcts::BELIEF_DRAW_ENDTURN_WIDENING;
        let cfg = RecommendParams::default().mcts_config();
        assert_eq!(cfg.endturn_widening_c.to_bits(), dc.to_bits());
        assert_eq!(cfg.endturn_widening_alpha.to_bits(), dalpha.to_bits());

        let p = RecommendParams::from_config(
            &json!({"endturn_widening_c": 0.5, "endturn_widening_alpha": 0.4}),
            "t",
        )
        .unwrap();
        assert_eq!(p.endturn_widening_c, Some(0.5));
        let cfg = p.mcts_config();
        assert!((cfg.endturn_widening_c - 0.5).abs() < 1e-6);
        assert!((cfg.endturn_widening_alpha - 0.4).abs() < 1e-6);
        // Pinning the EndTurn pair leaves the rest of the dialect alone.
        assert!((cfg.draw_widening_c - 2.0).abs() < 1e-6);

        let cfg = RecommendParams::from_config(&json!({"hidden_info": "clairvoyant"}), "t")
            .unwrap()
            .mcts_config();
        let engine = MctsConfig::default();
        assert_eq!(
            cfg.endturn_widening_c.to_bits(),
            engine.endturn_widening_c.to_bits()
        );
        assert_eq!(
            cfg.endturn_widening_alpha.to_bits(),
            engine.endturn_widening_alpha.to_bits()
        );
    }

    #[test]
    fn bad_modes_are_rejected_loudly() {
        let e = RecommendParams::from_config(&json!({"hidden_info": "psychic"}), "mcts_recommend")
            .unwrap_err();
        assert!(e.starts_with("mcts_recommend: hidden_info"), "{e}");
        let e =
            RecommendParams::from_config(&json!({"rollout_policy": "greedy"}), "x").unwrap_err();
        assert!(e.contains("rollout_policy"), "{e}");
    }

    #[test]
    fn output_ranks_by_q_then_terminal_win_then_visits() {
        let s =
            |visits: u32, wins: u32, value_sum: f32, terminal_win: bool, idx: u8| RecommendScore {
                action: Action::PlayCard {
                    hand_index: idx,
                    target: None,
                },
                visits,
                wins,
                losses: visits - wins,
                ties: 0,
                value_sum,
                terminal_win,
            };
        let out = recommend_output_value(
            vec![
                s(100, 50, 50.0, false, 0), // q 0.5
                s(1, 1, 1.0, true, 1),      // q 1.0, terminal win
                s(10, 10, 10.0, false, 2),  // q 1.0, more visits but no terminal win
                s(2, 0, 0.0, false, 3),     // low confidence (below 5% of 100)
            ],
            &[Action::EndTurn],
            false,
            113,
            1,
            true,
        );
        let acts = out["actions"].as_array().unwrap();
        let idx = |i: usize| acts[i]["action"]["card_index"].as_u64().unwrap();
        assert_eq!([idx(0), idx(1), idx(2), idx(3)], [1, 2, 0, 3]);
        assert_eq!(acts[0]["is_best"], json!(true));
        assert_eq!(acts[1]["is_best"], json!(false));
        assert_eq!(acts[3]["low_confidence"], json!(true));
        assert_eq!(acts[0]["low_confidence"], json!(true)); // 1 visit < ceil(5)
        assert_eq!(out["q_saturated"], json!(false));
        assert_eq!(out["pv"][0]["action"], json!("end_turn"));
        assert_eq!(out["sims"], json!(113));
        assert_eq!(acts[0]["bucket"], json!(hint::win_prob_bucket(1.0)));
    }

    #[test]
    fn empty_scores_are_a_valid_result() {
        let out = recommend_output_value(Vec::new(), &[], false, 0, 1, false);
        assert_eq!(out["actions"], json!([]));
        assert_eq!(out["q_saturated"], json!(false));
        assert_eq!(out["intent_damage_known"], json!(false));
    }
}
