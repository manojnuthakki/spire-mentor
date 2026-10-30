//! `PpoRunAgent` + `ppo_observe_json` — Task 9 of the 2026-07-07
//! ppo-rust-inference plan. Cloned from the (since-removed) ranker agent's PyO3
//! pattern: a stepwise agent that drives the deployed
//! `MctsCombatPolicy<PpoPolicy>` one decision at a time on a live
//! `PyRunManager`, plus a free function that exposes the Rust
//! observation builder directly for Python-side parity assertions.
//!
//! These two surfaces back the Task 9 end-to-end Python gates:
//! - **Gate A2** (`ppo_observe_json` vs `ppo.env.observe()`): obs-dict
//!   parity along a policy-mode (`env.reset()`/`env.step()`) walk.
//! - **Gate C** (`PpoRunAgent.evaluate` vs `NetPolicy.distribution`):
//!   argmax/π/V parity along the same walk, without the Rust agent
//!   actually acting (the Python harness drives every step so both
//!   sides see identical trajectories).

use std::path::PathBuf;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use sts2_core::full_run::RunPolicy;
use sts2_core::run_manager::Phase;
use sts2_mcts::RootOutcome;
use sts2_runai::policy::{CombatBudget, MctsCombatPolicy};
use sts2_runai::ppo::net::onnx_model_path;
use sts2_runai::ppo::policy::{build_obs, PpoPolicy};

use crate::run_manager::{phase_str, run_action_to_pydict, PyRunManager};

/// `sts2_rs.ppo_observe_json(rm, phase, surface, seed, ascension)` — the
/// Rust-side twin of `ppo.env.observe()`, exercised by Gate A2.
///
/// Materializes the pending event (idempotent, RNG-free — the same call
/// `PpoPolicy::prepare` makes) when `phase == "event"` so the built
/// observation sees a bound `current_event`/`event_options`, exactly
/// like Python's `observe()` does via `rm.materialize_event()`. The
/// caller-supplied `seed`/`ascension` are stamped onto the result
/// (rather than trusted from `rm`'s own fields), matching the
/// interface's documented contract.
#[pyfunction]
pub fn ppo_observe_json(
    rm: &mut PyRunManager,
    phase: &str,
    surface: Option<&str>,
    seed: i64,
    ascension: i64,
) -> PyResult<String> {
    if phase == "event" && rm.inner.run_state.current_event.is_none() {
        if let Some(id) = sts2_content::events::dispatch::pick_event(&mut rm.inner.run_state, None)
        {
            rm.inner.run_state.bind_event(id);
        }
    }
    let mut obs = build_obs(&rm.inner, phase, surface);
    obs.seed = seed;
    obs.ascension = ascension;
    serde_json::to_string(&obs)
        .map_err(|e| PyRuntimeError::new_err(format!("ppo_observe_json: serialize failed: {e}")))
}

/// Python-facing stepwise agent that plays the deployed
/// `MctsCombatPolicy<PpoPolicy>` on a live `RunManager`, mirroring
/// `PyHeuristicRunAgent`. Combat stays MCTS's (eval-cheap by default,
/// matching `run_arena --policy ppo --combat eval-cheap`); the PPO net
/// only ever scores non-combat decisions.
#[pyclass(name = "PpoRunAgent", module = "sts2_rs")]
#[derive(Debug)]
pub struct PyPpoRunAgent {
    policy: MctsCombatPolicy<PpoPolicy>,
}

#[pymethods]
impl PyPpoRunAgent {
    /// `model_path` overrides `STS2_RUNAI_PPO`/the sibling `sts2-pycli`
    /// snapshot default ([`onnx_model_path`]); `sims`/`boss_sims`
    /// override `CombatBudget::eval_cheap()`'s defaults (1000/4000)
    /// exactly like `HeuristicRunAgent`.
    ///
    /// `features` is the encoder schema spec the net was TRAINED at
    /// (`champions.toml`'s `features`, `STS2_PPO_FEATURES` syntax). With an
    /// explicit `model_path` the policy cannot look the schema up itself
    /// (no manifest entry describes an arbitrary path), so a caller serving
    /// a manifest champion passes it here; `None` keeps the process-wide
    /// `STS2_PPO_FEATURES`.
    #[new]
    #[pyo3(signature = (seed = 0, sims = None, boss_sims = None, model_path = None, features = None))]
    fn new(
        seed: i32,
        sims: Option<u32>,
        boss_sims: Option<u32>,
        model_path: Option<String>,
        features: Option<String>,
    ) -> PyResult<Self> {
        let path = model_path
            .map(PathBuf::from)
            .or_else(onnx_model_path)
            .ok_or_else(|| {
                PyRuntimeError::new_err(
                    "PpoRunAgent requires model_path=... or STS2_RUNAI_PPO=<path/to/model.onnx> \
                 (or the sibling sts2-pycli snapshot)",
                )
            })?;
        let meta = match features {
            Some(spec) => {
                let parsed = sts2_runai::ppo::features::Features::try_parse(&spec)
                    .map_err(pyo3::exceptions::PyValueError::new_err)?;
                PpoPolicy::from_path_with(&path, parsed)
            }
            None => PpoPolicy::from_path(&path),
        };

        let mut budget = CombatBudget::eval_cheap();
        if let Some(s) = sims {
            budget.sims = s;
        }
        if let Some(b) = boss_sims {
            budget.boss_sims = b;
        }

        let policy = MctsCombatPolicy::new(meta, budget, seed);
        Ok(Self { policy })
    }

    /// Apply one decision to `rm` and return a step record dict, or
    /// `None` if there is no legal action to take. Same shape as
    /// `HeuristicRunAgent.step` — see that method's doc comment for the
    /// key list.
    fn step<'py>(
        &mut self,
        py: Python<'py>,
        rm: &mut PyRunManager,
    ) -> PyResult<Option<Bound<'py, PyDict>>> {
        // `prepare` (event materialization) MUST run before
        // `get_available_actions`, mirroring `drive_full_run`'s call
        // order — a not-yet-materialized event can present a different
        // (or empty) action surface than the materialized one.
        self.policy.prepare(&mut rm.inner);
        let actions = rm.inner.get_available_actions();
        if actions.is_empty() {
            return Ok(None);
        }

        let phase = rm.inner.phase();
        let is_combat = phase == Phase::Combat;

        let Some(idx) = self.policy.choose(&rm.inner, &actions) else {
            return Ok(None);
        };
        let action = actions[idx];

        // Capture the combat search record AFTER `choose` (which runs
        // the MCTS and caches it) but BEFORE `take_action` (which
        // advances the state) — same rationale as `HeuristicRunAgent::step`.
        let combat_search: Option<Bound<'py, PyDict>> = if is_combat {
            self.policy
                .last_combat_search()
                .map(|rec| -> PyResult<Bound<'py, PyDict>> {
                    let d = PyDict::new(py);
                    let best_idx = rec
                        .outcomes
                        .iter()
                        .position(|o| o.action == rec.chosen)
                        .unwrap_or(0);
                    let list = PyList::empty(py);
                    for (i, o) in rec.outcomes.iter().enumerate() {
                        let is_best = i == best_idx;
                        let entry = root_outcome_to_pydict(py, o, is_best)?;
                        list.append(entry)?;
                    }
                    d.set_item("actions", list)?;
                    d.set_item("recommended", crate::action_to_pydict(py, &rec.chosen)?)?;
                    Ok(d)
                })
                .transpose()?
        } else {
            None
        };

        rm.ensure_play_log_flag();
        let result = rm.inner.take_action(action);

        let over = rm.inner.is_over();
        let won = rm.inner.run_state.player_won;

        let out = PyDict::new(py);
        out.set_item("phase", phase_str(phase))?;
        out.set_item("is_combat", is_combat)?;
        out.set_item("over", over)?;
        out.set_item("won", won)?;
        out.set_item("action", run_action_to_pydict(py, &action)?)?;
        out.set_item("combat_search", combat_search)?;
        out.set_item("result", format!("{result:?}"))?;
        Ok(Some(out))
    }

    /// Score the CURRENT decision without taking any action: `prepare`
    /// (event materialization) then `PpoPolicy::evaluate` on the
    /// (masked) candidate list. Returns `(probs, value, argmax)` indexed
    /// into the FILTERED candidate list — same convention as
    /// `PpoPolicy::evaluate` — or `None` for `Phase::Combat`/
    /// `Phase::RunOver`/an already-finished run/a single-option
    /// decision (no inference needed).
    fn evaluate(&mut self, rm: &mut PyRunManager) -> Option<(Vec<f32>, f32, usize)> {
        self.policy.prepare(&mut rm.inner);
        let actions = rm.inner.get_available_actions();
        if actions.is_empty() {
            return None;
        }
        self.policy.meta().evaluate(&rm.inner, &actions)
    }

    /// Like [`Self::evaluate`] but also returns the masked candidate
    /// action dicts (same shape `HeuristicRunAgent.step` emits per action),
    /// so a caller can label each probability and dispatch the argmax
    /// without re-deriving the mask. `actions[i]` corresponds to
    /// `probs[i]`; `argmax` indexes both. `None` for
    /// `Phase::Combat`/`Phase::RunOver`/a finished run/a single-option
    /// decision — identical to [`Self::evaluate`]'s `None` contract.
    fn evaluate_with_actions<'py>(
        &mut self,
        py: Python<'py>,
        rm: &mut PyRunManager,
    ) -> Option<(Bound<'py, PyList>, Vec<f32>, f32, usize)> {
        self.policy.prepare(&mut rm.inner);
        let actions = rm.inner.get_available_actions();
        if actions.is_empty() {
            return None;
        }
        let (masked, probs, value, argmax) = self
            .policy
            .meta()
            .evaluate_with_actions(&rm.inner, &actions)?;
        let list = PyList::empty(py);
        for a in &masked {
            // run_action_to_pydict can only fail on an unknown action kind;
            // a masked non-combat RunAction is always a known kind, so unwrap
            // the serialization (a failure here is a genuine bug, not input).
            let d = run_action_to_pydict(py, a).expect("run_action_to_pydict on masked action");
            list.append(d).expect("append to fresh PyList");
        }
        Some((list, probs, value, argmax))
    }
}

/// Serialise one [`RootOutcome`] into the same per-action dict shape
/// `mcts_recommend` emits — byte-copy of `heuristic_agent.rs`'s helper.
fn root_outcome_to_pydict<'py>(
    py: Python<'py>,
    o: &RootOutcome,
    is_best: bool,
) -> PyResult<Bound<'py, PyDict>> {
    crate::action_score_pydict(
        py,
        &o.action,
        o.visits,
        o.wins,
        o.losses,
        o.ties,
        o.value_sum,
        /* low_confidence */ false,
        /* q_saturated */ false,
        is_best,
    )
}
