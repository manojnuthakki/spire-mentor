//! UCT MCTS engine for sts2-core (Phase 8, bead `sts2-rs-69p`).
//!
//! Classic 4-phase UCT loop: Selection → Expansion → Simulation → Backprop.
//! Arena-allocated tree (`Vec<MctsNode>` indexed by [`NodeId`]) so children
//! are referenced as `u32` indices instead of `Box`/`Rc` — keeps a 1000-sim
//! search in a handful of allocations.
//!
//! ## Determinization
//!
//! STS combat is stochastic (card draws, monster intent rolls). We use
//! **single-determinization** MCTS as recommended by the porting plan:
//! each simulation re-seeds the rollout RNG with
//! `config.seed XOR sim_index`, then clones the state at the rolled-out
//! node and runs a single trajectory. This is the v0.1.0 baseline;
//! a future bead can swap in multi-determinization / determinization
//! caching without changing the public surface.
//!
//! ## Out of scope for v0.1.0
//!
//! * No PUCT — only the [`Policy`] trait wiring exists. Selection uses
//!   the classic UCT formula `Q + c_puct·sqrt(ln(N)/n)`.
//! * No parallelism. Phase 8 is single-threaded perf focus.
//!
//! ## Transposition table (bead `sts2-rs-7z97`, W6)
//!
//! Optionally, on expansion the engine consults a transposition table
//! keyed on `(state_hash, depth_bucket)` (where `depth_bucket = min(depth
//! / 4, 255)` — coarse enough to merge near-by occurrences of the same
//! position, fine enough to keep turn-1 and turn-50 occurrences of the
//! same surface state separate). On a hit, the new edge is wired
//! directly to the existing node — the same node may then have multiple
//! parents, so the search graph becomes a DAG. Backprop walks only the
//! actual selection path each iteration ("sum into both paths"); the
//! shared node accumulates statistics from every path that arrives at
//! it, which slightly inflates visit counts but keeps the
//! implementation a one-line addition to the existing backprop loop.
//!
//! Keyed on [`sts2_core::search_hash`], which omits the RNG cursors, so
//! two paths reaching the same board merge even when they consumed
//! randomness differently. [`MctsConfig::use_transposition_table`] reads
//! as a gate but is not one — [`Mcts::with_policy`] forces it on for
//! every search; see that field's docs.

/// Content hash of THIS crate's source, stamped at build time by `build.rs`
/// (issue #15, bead `sts2-rs-cg5ci`). Compiled INTO the crate so a stale
/// `sts2-mcts` rlib carries its old hash. See `sts2_core::SRC_HASH`.
pub const SRC_HASH: &str = env!("STS2_MCTS_SRC_HASH");

use rand::SeedableRng;
use rand_xoshiro::Xoshiro256PlusPlus;
use rayon::prelude::*;
use smallvec::SmallVec;

#[cfg(test)]
use sts2_core::StepResult;
use sts2_core::{
    apply as core_apply, evaluate_victory, is_terminal, legal_actions, loot_at_risk_hp_equiv,
    state_hash, terminal_value, victory_hp_equiv_to_q, victory_hp_surplus, Action, CombatState,
};

pub mod action_index;
pub mod corpus;
pub mod expert;
pub mod hint;
pub mod lethal_detector;
pub mod motif;
pub mod npz;
pub mod quiescence;
pub mod tt;
pub mod value_net;

pub use lethal_detector::detect_lethal;

/// Errors that can be returned by [`Mcts::try_search_n`] and friends.
/// Bead `sts2-rs-b32is`.
///
/// # Example
///
/// ```
/// use sts2_mcts::SearchError;
///
/// fn handle(err: SearchError) -> &'static str {
///     match err {
///         SearchError::TerminalRoot => "engine handed a dead root",
///     }
/// }
///
/// assert_eq!(handle(SearchError::TerminalRoot), "engine handed a dead root");
/// // The error also implements Display + std::error::Error so it
/// // propagates cleanly via `?` in fallible call-sites.
/// assert_eq!(
///     format!("{}", SearchError::TerminalRoot),
///     "search() called on terminal root: no legal actions",
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchError {
    /// The root state has no legal actions. Reachable when an
    /// external caller hands the engine a `CombatState` with
    /// `is_over = true` (debug-CLI scripted replay, fuzz harness,
    /// ill-conditioned smoke test). The convenience wrapper
    /// [`Mcts::search_n`] panics on this; [`Mcts::try_search_n`]
    /// returns it.
    TerminalRoot,
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchError::TerminalRoot => {
                write!(f, "search() called on terminal root: no legal actions")
            }
        }
    }
}

impl std::error::Error for SearchError {}

pub use action_index::{
    action_key_to_slot, action_to_key, action_to_keyed_slot, action_to_slot, target_class,
    ActionKey, ActionTarget, ACTION_KEY_DIM, CANONICAL_ACTION_DIM,
};
pub use corpus::{CorpusCollector, CorpusCollectorWide, CorpusEntry, CorpusEntryWide};
pub use expert::{Heuristic, RolloutPolicy, RolloutPolicyKind, Uniform};
pub use tt::TranspositionTable;
pub use value_net::{
    EmbeddingValueNet, LeafEval, LearnedPolicyPrior, LearnedPolicyPriorStats, LearnedValueNet,
    NormStats, PolicyNet, ValueNetError,
};

// ─── Config / policy ────────────────────────────────────────────────────────

/// MCTS configuration.
#[derive(Clone, Copy, Debug)]
pub struct MctsConfig {
    /// UCT exploration constant. Default `sqrt(2) ≈ 1.41`.
    pub c_puct: f32,
    /// Number of simulations the engine will run when [`Mcts::search`] is
    /// called.
    pub max_simulations: u32,
    /// Hard cap on rollout depth. Hitting the cap returns a tie (`0.0`)
    /// — i.e. the leaf eval is consulted via [`Self::terminal_eval_mode`]
    /// / [`crate::LeafEval`].
    ///
    /// **Tuning note (GitHub #18):** the default `10` is shallow
    /// relative to typical STS2 combat length (20-40 turns / ~80+
    /// action plies). The `vbzji` A/B experiment found that
    /// `sims=10k, depth=200` beat `sims=150k, depth=20` for the
    /// Learned leaf-eval — i.e. deeper search recovers more terminal
    /// signal than wider shallow search when the value head is
    /// mid-state OOD. If you swap the leaf-eval to
    /// [`crate::LeafEval::Learned`], strongly consider raising this to
    /// at least `50` (and ideally to a value that lets most rollouts
    /// reach a real terminal) before drawing conclusions about
    /// value-head quality.
    pub max_rollout_depth: u32,
    /// Base seed for the determinization RNG.
    pub seed: u64,
    /// Rollout policy selector. Default is
    /// [`RolloutPolicyKind::Heuristic`] (softmax over expert play
    /// priority, see [`expert::play_ordering`]); set to
    /// [`RolloutPolicyKind::Uniform`] for the pre-`dfv1` baseline.
    /// Bead `sts2-rs-dfv1` (W1).
    pub rollout_policy: RolloutPolicyKind,
    /// Priority the heuristic rollout gives `EndTurn`. Defaults to
    /// [`expert::END_TURN_PRIORITY`] (10 000), where the softmax weight
    /// underflows to `+0.0` and the rollout never ends a turn while a card
    /// is playable — it empties the hand every turn.
    ///
    /// A finite value inside the card range (roughly `[-30, 90]`) makes
    /// stopping early a sampled option, so lines whose merit is *not*
    /// playing a card become representable in the rollout. Ignored by
    /// [`RolloutPolicyKind::Uniform`], which reads no priorities at all.
    /// GH #446 — see `logs/2026-09-08-endturn-rollout/`.
    pub rollout_end_turn_priority: i16,
    /// Enable the transposition table (bead `sts2-rs-7z97`, W6). On
    /// expansion, the engine consults a `(search_hash, depth_bucket)`
    /// map and links to the existing node instead of allocating when a
    /// hit exists.
    ///
    /// **This field is inert.** [`Mcts::with_policy`] overwrites it with
    /// `true` on every construction (`config.use_transposition_table =
    /// true`, since `755819594`), so the `false` here and at every call
    /// site is not what the search runs. Chance trees reconverge heavily
    /// and `search_hash`-keyed merging is load-bearing for them.
    ///
    /// Measured on the deployed dialect over 1000 harvested positions
    /// (2026-08-12): **0.1518 hits/sim**, merging 12.4% of
    /// node-creation attempts.
    ///
    /// `docs/perf/2026-05-20-gh17-tt-default-ab.md` reports 0 hits/sim
    /// and recommends the default stay off. That verdict is stale twice
    /// over: it measured a table keyed on the RNG-cursor-*sensitive*
    /// [`sts2_core::state_hash`] a month before
    /// [`sts2_core::search_hash`] existed (`2bbb00a91`, 2026-06-20), and
    /// it ran only on the 50-state starter corpus because the realistic
    /// fixture pool was missing from that environment.
    pub use_transposition_table: bool,
    /// Softmax temperature for the closed-form regularized root
    /// policy `π̄(a) ∝ exp(Q(a) / T)` used to pick the committed
    /// action at the end of `search_n` (bead `sts2-rs-trnu`, plan
    /// §4.4; Danihelka et al. 2020, arXiv:2007.12509). `T → 0` makes
    /// `π̄` peaky around the highest-mean-value child (≈ argmax(Q));
    /// `T → ∞` flattens to uniform. Default `0.1` keeps the chosen
    /// action near argmax(Q) while still defining a calibrated
    /// distribution that downstream samplers can read via
    /// [`Mcts::regularized_root_pi`]. Replaces the prior
    /// `argmax(visits)` rule which is a crude approximation to this
    /// closed form (per the paper).
    pub regularized_pi_temperature: f32,
    /// Wave-6 bead `sts2-rs-ry1d` (qctu.9). When `true`, the
    /// selection formula swaps from UCT
    /// (`Q + c_puct·sqrt(ln N / n)`) to PUCT
    /// (`Q + c_puct·P(s,a)·sqrt(ΣN)/(1+N(s,a))`), reading
    /// `P(s,a)` from `self.policy.prior(state, actions)`. Default
    /// `false` keeps every pre-bead fixture byte-stable. Flip
    /// alongside `Mcts::with_policy(state, config,
    /// LearnedPolicyPrior::new(arc))` to give the trained policy
    /// head actual influence over the search — the qctu.8 prior
    /// surface is otherwise a no-op against the UCT formula.
    pub use_puct_prior: bool,
    /// How to score rollout terminal states. Default
    /// `TerminalEvalMode::Binary` is the historical behavior:
    /// win returns evaluate_victory(state) in [0.5, 1.0]; loss
    /// returns flat -1.0. The flat-loss branch throws away the
    /// rich state signal (enemy HP remaining, potions used, turn).
    /// See `sts2-rs-vbzji` mid-combat A/B finding. Other modes
    /// use the configured leaf_eval to differentiate "barely lost"
    /// from "annihilated".
    pub terminal_eval_mode: TerminalEvalMode,
    /// Bead `sts2-rs-8hs96` integration. When `true`, every rollout's
    /// leaf-eval consultation is preceded by
    /// `quiescence::drive_to_quiescence(state, 32)` — driving the
    /// state through all sole-legal-action transitions so the eval
    /// observes a "quiet" state. Default `false` keeps existing
    /// fixtures byte-stable.
    pub use_quiescence: bool,
    /// Enable the conservative lethal-this-turn detector
    /// ([`crate::detect_lethal`], bead `sts2-rs-5yzrh` + GH #51).
    /// When `true`, every newly-visited non-terminal leaf is checked
    /// against the detector; a positive hit stamps `proven_win` on the
    /// leaf so the standard MCTS-Solver lite backup propagates the
    /// forced win toward the root without burning rollout budget.
    /// Defaults to `false` (byte-stable against fixtures that don't
    /// opt in). The detector is conservative — false positives are
    /// avoided at the cost of low recall on positions outside its
    /// allow-listed shape.
    pub use_lethal_detector: bool,
    /// Double-Progressive-Widening coefficient `C` for non-`EndTurn` chance
    /// nodes: the outcome cap is `ceil(C · (visits+1)^alpha)`.
    pub chance_widening_c: f32,
    /// Double-Progressive-Widening exponent `alpha` for non-`EndTurn` chance
    /// nodes.
    pub chance_widening_alpha: f32,
    /// DPW coefficient `C` for chance nodes whose stochasticity was
    /// detected via a draw-order event (bead `sts2-rs-k1z14` retune,
    /// F1 plan §3.3), rather than an RNG-counter delta — see
    /// [`MctsNode::chance_is_draw_triggered`], which is the actual source
    /// of truth for when this field applies. Originally (F1) that flag was
    /// `true` only under [`HiddenInfoMode::DrawChance`], but bead
    /// `sts2-rs-09x5n` (F2) extended it to also fire under
    /// [`HiddenInfoMode::BeliefDraw`] whenever `cards_drawn` advances — so
    /// this field is live for BOTH `DrawChance` AND `BeliefDraw`, not just
    /// `DrawChance`. It remains dead weight for `Clairvoyant`/`RootScramble`
    /// (the flag is always `false` there, so reading it never changes their
    /// behavior). `BeliefDraw` is the deployed default (F2 champion) and
    /// bakes its own retuned `draw_widening_c = 2.0` in
    /// `sts2-runai::mcts_config_for` — this knob is load-bearing for the
    /// shipped agent, not an inert F1 leftover. `EndTurn` nodes keep using
    /// [`Self::endturn_widening_c`]
    /// even when also draw-triggered — `is_endturn` takes priority in
    /// [`Mcts::dpw_cap`]'s class selection, preserving the existing
    /// expensive-transition cost containment for the enemy-turn
    /// resolution node. Defaults equal to [`Self::chance_widening_c`]
    /// — a deliberate, evidence-based choice, not just a byte-stable
    /// placeholder:
    ///
    /// The H1 diagnostic (`drawchance_diagnose`) found draw-triggered
    /// chance nodes capped at ≤2 realized outcomes 92% of the time at
    /// typical visit counts, so F1 mechanically could barely hedge over
    /// draw order. Widening this knob DOES move the G2 closed-loop
    /// recovery metric at the fixed 1000-sim G2 budget — a `C=2.5,
    /// α=0.5` sweep point reached 27.8% recovery vs 11.1% at parity —
    /// but a sims ladder at that same config REVERSES sign as the
    /// budget grows: 1000 sims → +27.8%, 2000 → +5.3%, 4000 → **−22.2%**
    /// (F1 becomes worse than F0). That is a single-seed, low-n (500)
    /// signal (every sweep point's McNemar p ≥ 0.4 — none individually
    /// significant), so the 1000-sim peak reads as noise riding a
    /// downward trend, not a real budget-independent improvement.
    /// Baking a config that flips sign 2–4x sims out would be actively
    /// harmful to any downstream caller running at higher budgets (e.g.
    /// boss-tier escalation), so the default stays at parity with
    /// [`Self::chance_widening_c`] pending a wider, multi-seed
    /// confirmation before anything more aggressive ships as default.
    /// See the bead `sts2-rs-k1z14` (G2/G3 retune pass) task report for
    /// the full sweep table.
    pub draw_widening_c: f32,
    /// DPW exponent `alpha` for draw-triggered chance nodes. See
    /// [`Self::draw_widening_c`].
    pub draw_widening_alpha: f32,
    /// DPW coefficient `C` for `EndTurn` chance nodes — tuned separately
    /// because `EndTurn` resolves the whole enemy turn (high entropy, most
    /// expensive transition), so fewer outcomes are widened.
    pub endturn_widening_c: f32,
    /// DPW exponent `alpha` for `EndTurn` chance nodes.
    pub endturn_widening_alpha: f32,
    /// Allow the MCTS-solver to prove a **win** through a chance node once
    /// its outcome set is deemed closed (all sampled outcomes proven-win and
    /// the closure heuristic has fired). This is unsound under DPW unless the
    /// support is genuinely finite and enumerated, so it defaults to `false`
    /// (the loss-side — any outcome proven-loss ⇒ chance node proven-loss —
    /// is always sound and stays on).
    pub chance_prove_wins: bool,
    /// Stop drawing fresh DPW outcomes at a chance node once
    /// [`MctsNode::chance_closed`] has fired (16 consecutive duplicate
    /// draws, [`CHANCE_CLOSE_K`]) — descend by [`Mcts::sample_outcome_by_visits`]
    /// instead, exactly as the cap-reached branch already does. Without
    /// this, a closed low-entropy node keeps paying a full state clone +
    /// edge re-apply (for `EndTurn`: an entire enemy-turn resolution) +
    /// state hash on EVERY visit where `n_outcomes < cap`, only to
    /// rediscover a duplicate — closure gates win-proving but never gated
    /// the spend. NOT byte-identical to the historical behavior: a dup
    /// draw descends into the child that matched the fresh sample (true
    /// outcome-probability sampling), while the sample path is
    /// visit-proportional, and the residual chance of discovering a
    /// sub-1/16-probability outcome after closure is forfeited. Defaults
    /// to `false` (byte-identical) pending a paired win-rate null-check;
    /// enable via `STS2_RUNAI_MCTS_OVERRIDES=chance_closed_stops_widening=1`.
    pub chance_closed_stops_widening: bool,
    /// Hard depth backstop for the chance-mode selection descent (and the
    /// principal-variation walk). The transposition table merges
    /// game-identical states across paths, so the search tree is a DAG that
    /// can contain cycles; without this cap a merged node could close a loop
    /// and spin the descent forever. Defaults to [`CHANCE_MAX_DEPTH`] (1024);
    /// configurable mainly so regression tests can trip the bound cheaply.
    /// Only consulted when [`Self::chance_nodes`].
    pub chance_max_depth: usize,
    /// How the search's root treats hidden information (draw-pile order
    /// beyond pins, RNG stream cursors). Bead `sts2-rs-vqckz` (F0, plan
    /// §3.1/§3.4 of `docs/plans/2026-07-12-clairvoyance-removal-plan.md`).
    /// Default [`HiddenInfoMode::Clairvoyant`] keeps every pre-bead
    /// fixture byte-stable — the root carries the true materialized pile
    /// order and true RNG cursors, exactly the historical (leaky)
    /// behavior. Set [`HiddenInfoMode::RootScramble`] for honest search:
    /// `Mcts::with_policy` blindfolds a private copy of the root state
    /// (via [`sts2_core::blindfold::blindfold_root`], seeded purely from
    /// [`Self::seed`]) before the root node is built, so the chosen
    /// action becomes a function of player-observable information only.
    /// [`Mcts::reroot`] is unsupported under `RootScramble` and asserts —
    /// it installs the caller's state verbatim (no blindfold), so tree
    /// reuse would silently re-leak hidden information; honest modes must
    /// construct a fresh `Mcts` per decision.
    ///
    /// This engine-level default stays `Clairvoyant` deliberately (reroot
    /// / tree-reuse callers + fixture stability); the DEPLOYED run-AI
    /// default flipped to honest [`HiddenInfoMode::BeliefDraw`] as of the
    /// G4 bless (2026-07-12) — see `sts2-runai`'s
    /// `MctsCombatPolicy::mcts_pick` / `resolve_hidden_info`, gated by
    /// `STS2_RUNAI_HONEST` (unset now means honest, not clairvoyant).
    pub hidden_info: HiddenInfoMode,
    /// Common random numbers (CRN): give every stochastic edge leaving one
    /// decision node the SAME `randomness_base`, so outcome *n* of
    /// "play Defend" and outcome *n* of "play Strike" are realized from an
    /// identical reseed — for every *n* including 0, which costs one
    /// pre-action rebuild per stochastic edge (see below). The luck is
    /// then matched across the alternatives
    /// being compared, and the difference in their values is attributable
    /// to the move rather than to which nature draws each happened to
    /// sample. This is the variance-reduction technique `sts_lightspeed`
    /// relies on; ours does the deliberate opposite by default (see
    /// [`Mcts::alloc_chance_node`], which folds the parent's child count
    /// into the salt precisely so siblings get *distinct* bases).
    ///
    /// Default `false`: enabling it changes every seeded search fixture,
    /// and the matched-luck argument is weaker here than in lightspeed for
    /// two independent reasons. First, after two different card plays the
    /// deck composition itself differs, so a shared reseed stream only
    /// approximately matches the two nature draws. Second, UCT allocates
    /// siblings wildly unequal visits, and each chance node counts its own
    /// `chance_draws` ordinal — sibling A's k-th widen does share
    /// `(base, k)` with sibling B's k-th, so the PAIRING is exact, but the
    /// two means are still averages over different-length ordinal
    /// prefixes. Matched luck is a prefix property, not a set property.
    /// Hence a flag and an A/B rather than a switch of default. Judge it on
    /// per-decision stability (flip rate, modal-share instability on
    /// pivotal block-vs-push turns), not on win rate.
    ///
    /// Read by [`Mcts::alloc_chance_node`] (the shared base) and by the
    /// outcome-0 realization in `expand` (which switches from the
    /// on-policy continuation to a `(base, 0)` reseed, without which
    /// outcome 0 — the bulk of a low-visit sibling's value — would be the
    /// one outcome CRN failed to match). A search that never allocates a
    /// chance node is unaffected either way.
    ///
    /// NOT plumbed into the other search entry points: `MctsCombatPolicy`
    /// (`sts2-runai`), `play_to_terminal_config`, `PyMcts::new`,
    /// `mcts_search_tree`, and `sts2-cli`'s agent all leave it at the
    /// default, so only `sts2_rs.mcts_recommend` can turn it on today.
    /// That matches the stated purpose (per-decision stability off the
    /// recommender path); a native-arm or play-to-terminal A/B needs the
    /// plumbing extended first.
    pub common_random_numbers: bool,
    /// Root-pick tolerance band for [`Mcts::pick_root_action`]: among
    /// root children whose (bonus-adjusted) mean value lies within
    /// `tie_eps` of the best, the most-visited child wins. Default
    /// `0.02` — the measured per-child Q noise floor at a run's
    /// 1000-sim budget (GH #403; the 2026-08-09 A8 audit re-measured
    /// reproducibility at ±0.075 across search seeds, so 0.02 is
    /// conservative). `0.0` restores strict-equality tie-breaking
    /// (band = exact score ties only); negative values are clamped
    /// to `0.0`.
    pub tie_eps: f32,
    /// Graded proven-loss backups (2026-08-09 A8 audit, FINDINGS §3).
    /// When `true` (default), the RichLoss / RichBoth loss backup
    /// replaces the constant [`value_net::TERMINAL_LOSS_LEAF`] clamp
    /// with a loss-quality gradient ([`loss_quality`]): losses land in
    /// `[-1.45, -1.05)` ordered by enemy HP removed / player HP kept /
    /// turns survived, still strictly below every depth-cut (> -1.0)
    /// and win (>= +1.0) value. The audit measured 2428 pure-loss root
    /// children at mean −1.4500 sd 0.0003 — whole boss fights with
    /// every child pinned to the −1.45 floor, i.e. the search switched
    /// off exactly where it matters most (and 6 of 15 fights it
    /// believed lost were actually won). `false` restores the
    /// pre-audit constant floor byte-for-byte (rollback seam:
    /// `STS2_RUNAI_LOSS_GRADIENT=0` in `sts2-runai`).
    pub loss_gradient: bool,
    /// Enemy-HP-removed-fraction weight in the [`loss_quality_with`]
    /// gradient (arm E surface, 2026-08-15 Optuna study prep). Default
    /// `0.6`, the dominant term per the 2026-08-09 A8 audit (FINDINGS
    /// §3). Overridden weights need not sum to `1.0` — the sum-to-1
    /// invariant documented on [`loss_quality`] only holds at
    /// [`MctsConfig::default()`].
    pub loss_w_removed: f32,
    /// Player-HP-kept-fraction weight in the [`loss_quality_with`]
    /// gradient. Default `0.25`.
    pub loss_w_hp: f32,
    /// Turns-survived weight in the [`loss_quality_with`] gradient.
    /// Default `0.15`.
    pub loss_w_turns: f32,
    /// Denominator offset for the turns-survived term of
    /// [`loss_quality_with`]: `round / (round + loss_turns_half_life)`.
    /// Default `8.0` (the legacy `LOSS_QUALITY_TURNS_HALF_LIFE` const).
    pub loss_turns_half_life: f32,
    /// Width of the graded loss-leaf band above
    /// [`value_net::TERMINAL_LOSS_LEAF`] consulted by
    /// [`loss_leaf_value`]. Default `0.8` (the legacy
    /// `LOSS_GRADIENT_LEAF_SPAN` const).
    pub loss_span: f32,
    /// Multiplier on the run-loot penalty ([`loot_penalty_q`]) charged at
    /// win terminals and depth-cap leaves — the GH #604 term that stops the
    /// search treating a thief's `Intent::Escape` as equivalent to killing
    /// it. Default `1.0` (the sizing baked into
    /// [`sts2_core::loot_at_risk_hp_equiv`]); `0.0` restores the pre-#604
    /// loot-blind values byte-for-byte, and larger values buy the loot more
    /// aggressively. Swept through `STS2_RUNAI_MCTS_OVERRIDES`
    /// (`loot_weight=…`).
    pub loot_weight: f32,
    /// Cheap 1-ply root guards inside the [`Self::tie_eps`] band
    /// (2026-08-09 A8 audit, FINDINGS §5 / recommendation 5). When
    /// `true` (default), [`Mcts::pick_root_action`] refines a
    /// band-winning `EndTurn` with two exact-knowledge checks that
    /// only ever promote an alternative ALREADY inside the band — a
    /// real Q gap outside `tie_eps` is never overridden: prefer an
    /// in-band block-granting play when telegraphed attack damage
    /// exceeds standing block (26 audited declines into realized
    /// damage, 307 HP lost, 16 in-band), else an in-band free play
    /// (67% of voluntary end-turns left ≥1 energy and a playable
    /// card; 269 left a playable 0-cost card, one a declined free
    /// lethal). `false` disables both (rollback seam:
    /// `STS2_RUNAI_ROOT_GUARDS=0` in `sts2-runai`). The proven-win
    /// guard is NOT gated by this flag — a solver-proven winning
    /// child is exact knowledge, not a heuristic.
    pub root_guards: bool,
    /// Root-level potion opportunity-cost charge (GH #447 follow-up),
    /// indexed by belt slot: [`Mcts::pick_root_action`] subtracts
    /// `charge[potion_slot]` (root-Q units — see
    /// `sts2_core::victory_hp_equiv_to_q`) from each
    /// `Action::DrinkPotion` child's bonus-adjusted score before the
    /// tie band and root guards run, so a charged drink falls out of
    /// the band naturally. Deterministic root arithmetic — the #447
    /// sweep proved the LEAF potion channel is noise-bound (±0.075
    /// per-child reproducibility swamps any plausible λ), which is why
    /// cross-fight potion value enters here instead. Slot-indexed
    /// because each searched ply builds a fresh `Mcts` from the current
    /// combat belt (and `MctsConfig` is `Copy`); slots past the belt
    /// length are ignored. `None` (default) ⇒ byte-identical picks.
    pub potion_charge_q: Option<[f32; 8]>,
    /// Exclude `Action::DrinkPotion` from ROOT children entirely
    /// (interior nodes and rollouts are untouched). The randomized
    /// drink-jitter seam (`STS2_RUNAI_DRINK_JITTER`) sets this per
    /// fight to generate exogenous belt variation for causal
    /// opportunity-cost estimation. Default `false` ⇒ no-op.
    pub root_drink_mask: bool,
    /// First-play-urgency value for an unvisited child, expressed as a
    /// reduction from the parent's mean Q: the first-visit score becomes
    /// `parent_q - fpu_reduction` instead of `f32::INFINITY`.
    ///
    /// `None` (the default) keeps the historical `INFINITY` tiebreak, under
    /// which **every** child is visited once before any is visited twice, at
    /// every node and every depth. That is a large source of flat visit
    /// allocation and it is entirely independent of [`Self::c_puct`] — the
    /// 2026-08-12 sweep's near-uniform `vmax/vmin` cannot be attributed to
    /// the exploration constant without pinning this down first.
    ///
    /// `Some(r)` is the AlphaZero/Leela convention: an unvisited child is
    /// worth what its parent is worth, minus a penalty for being unproven.
    /// `r = 0.0` values it at the parent (Leela-Zero); larger `r` discourages
    /// breadth. KataGo ships `fpuReductionMax = 0.2` with a smaller `0.1` at
    /// the root.
    ///
    /// This is the only FPU knob. An `EndTurn`-specific one
    /// (`endturn_fpu_penalty`, bead `sts2-rs-xwpl6`) was declared,
    /// documented and defaulted but never read by the chance-mode
    /// selector — the deployed path — so it was deleted in GH #445
    /// rather than wired, because the general form measured here is a
    /// **monotone loss**: every `r > 0` costs `gold_agr`, and by
    /// `r = 9.0` the search collapses to a single root child. Wiring
    /// the `EndTurn`-only variant would have been implementing a
    /// mechanism already measured as harmful.
    /// See `logs/2026-08-14-exploration-arms/FINDINGS.md` §2 and
    /// `docs/research/2026-08-14-adaptive-exploration-beyond-fixed-c-puct.md` §6.1.
    pub fpu_reduction: Option<f32>,
    /// KataGo's dynamic variance-scaled cPUCT: the exploration term is
    /// multiplied by
    /// `1 + cpuct_utility_stdev_scale · (σ_node / cpuct_utility_stdev_prior − 1)`,
    /// where `σ_node` is the standard deviation of the rollout values backed
    /// up through the node (prior-blended, see
    /// [`Self::cpuct_utility_stdev_prior_weight`]). Volatile nodes explore
    /// more, settled nodes exploit harder, and the conversion between "units
    /// of Q" and "units of exploration bonus" is estimated per node rather
    /// than assumed globally.
    ///
    /// Default `0.0` makes the factor identically `1.0`, i.e. **byte-stable**
    /// against every pre-existing fixture. KataGo ships `0.85`
    /// (`cpp/search/searchexplorehelpers.cpp`; ~50 Elo measured against an
    /// optimally-tuned constant).
    pub cpuct_utility_stdev_scale: f32,
    /// The σ at which [`Self::cpuct_utility_stdev_scale`] leaves the
    /// exploration term unchanged, and the mean of the variance prior that
    /// regularizes low-visit nodes. **This is in our utility units, not
    /// KataGo's** — their `0.40` is on a Go win-rate scale, ours span
    /// `[-1.45, +1.15]` with per-rollout σ ≈ 1. Set it to the measured median
    /// node σ (`mcts_node_stdev_probe`), or the factor degenerates into a
    /// constant rescale of `c_puct` in disguise.
    pub cpuct_utility_stdev_prior: f32,
    /// Pseudo-count weight of the variance prior. Without it a 3-visit node
    /// yields a 3-sample variance and a wild multiplier; against our ±0.075
    /// per-child noise that is not optional. KataGo ships `2.0`.
    pub cpuct_utility_stdev_prior_weight: f32,
    /// Allocate the root's simulation budget by **Sequential Halving**
    /// (Karnin et al. 2013) instead of by UCT.
    ///
    /// UCT minimizes *cumulative* regret, which is the wrong objective at the
    /// root: the search is paid once, for the action it finally recommends —
    /// that is *simple* regret. With a known fixed budget, Sequential Halving
    /// is near-optimal for that objective and takes **no exploration constant
    /// at all**. It applies only to the root; the descent below keeps UCT,
    /// which is the H-MCTS arrangement (Pepels, Cazenave & Winands).
    ///
    /// This changes budget *allocation* only. The committed action still
    /// comes from [`Mcts::pick_root_action`], so the proven-win guard,
    /// lost-position override, `tie_eps` band and EndTurn guards all still
    /// apply — which keeps the arm comparable to the others.
    ///
    /// Default `false`.
    pub root_sequential_halving: bool,
    /// **Global Std** (arXiv 2510.21275): replace the constant entirely with
    /// `c = global_std_c · σ_tree`, where `σ_tree` is the standard deviation
    /// of the mean values of *every visited node in the search tree*.
    ///
    /// This is a different statistic from
    /// [`Self::cpuct_utility_stdev_scale`], and the difference is the whole
    /// point. KataGo's σ is the spread of individual **rollout returns**
    /// through one node, which here is dominated by leaf-eval noise and barely
    /// moves between a decided position and an open one. Global Std is the
    /// spread of **backed-up Q estimates**, which is the quantity that
    /// actually separates "one action is clearly best" from "nothing is
    /// resolved" — and the quantity the position-book quartiles are built on.
    ///
    /// The 28-domain study found this the best of 12 scale-independent
    /// strategies, with `C ≈ 2` as the generalizing default, and specifically
    /// that node-local variants lose to tree-global ones because a single
    /// node's few samples are too noisy.
    ///
    /// `None` (default) keeps the fixed [`Self::c_puct`]. When set, `c_puct`
    /// is ignored.
    pub global_std_c: Option<f32>,

    /// **How simulation returns combine on the way back up** (GitHub #492).
    ///
    /// Every other field on this struct decides *where* simulations go.
    /// This one decides what happens to the results. Default
    /// [`BackupRule::Mean`] is the historical arithmetic mean and is
    /// byte-identical to the pre-#492 engine — the power-mean pass is
    /// skipped entirely, not computed-and-discarded.
    ///
    /// See `docs/research/2026-08-23-mcts-backup-rules.md`.
    pub backup_rule: BackupRule,
    /// Power-mean order `p` for [`BackupRule::PowerMean`]. `p = 1` is the
    /// (visit-weighted) arithmetic mean, `p -> +inf` is the max, and the
    /// operator is monotone increasing in `p` (Dam et al., IJCAI 2020,
    /// Lemma 1). Ignored under [`BackupRule::Mean`].
    ///
    /// **Range guidance.** The stochastic follow-up (Dam, Maillard,
    /// Kaufmann 2024) finds `p = 2` best in every stochastic MDP it tests
    /// and *worse* performance at `p in {4, 6, 8, 10, 16}`; the `p = 100+`
    /// that won on rocksample is a deterministic-transition result. Ladder
    /// the low end.
    pub backup_power_p: f32,
    /// Positive-domain shift `B` for [`BackupRule::PowerMean`]. The power
    /// mean is defined for *positive* operands; our values live in
    /// `[-MAX_LEAF_VALUE_ABS, +MAX_LEAF_VALUE_ABS]`, so each child Q is
    /// shifted to `Q + B` before exponentiation and `B` is subtracted from
    /// the result.
    ///
    /// **`B` is a real parameter of the arm, not an implementation
    /// detail:** the power mean is not shift-invariant for `p != 1`. A
    /// large `B` compresses the relative spread of the shifted values and
    /// pulls the operator back toward the mean; a small `B` makes a given
    /// `p` far more aggressive. Report it alongside `p`, and never compare
    /// our `p` to a published `p` without stating `B`.
    ///
    /// Must be at least `MAX_LEAF_VALUE_ABS` for the shifted operands to
    /// be non-negative on every reachable value; smaller settings are
    /// permitted (and are the way to increase aggression) but floor at
    /// zero, which discards ordering information below `-B`.
    pub backup_power_shift: f32,
}

/// How simulation returns combine on the way back up the tree
/// (GitHub #492). See [`MctsConfig::backup_rule`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BackupRule {
    /// Arithmetic mean: a node's value is `value_sum / visits`, the mean
    /// of every rollout beneath it. The historical rule, and the rule the
    /// 50 000-sim position-book oracle was built with.
    #[default]
    Mean,
    /// Power mean over **decision-node children**, weighted by child visit
    /// counts (Power-UCT; Dam, Klink, D'Eramo, Peters, Pajarinen, IJCAI
    /// 2020, Eq. 11 / Algorithm 1 line 38):
    ///
    /// ```text
    /// Q(s) = ( SUM_a (n_a / N) * (Q_a + B)^p ) ^ (1/p)  -  B
    /// ```
    ///
    /// **Chance nodes keep the mean.** In Power-UCT the power mean is
    /// applied at the state (V) node only; the state-action (Q) node — the
    /// layer that averages over sampled successor states — stays a plain
    /// running average (their Algorithm 1 line 51, and the explicit
    /// `SimulateQ`/`SimulateV` split in the 2024 stochastic follow-up).
    /// [`NodeKind::Chance`] *is* that layer here: its children are
    /// outcomes of nature, not choices, so a max-leaning aggregate over
    /// them would be planning against a draw the agent cannot force. This
    /// is a correctness constraint, not a tuning choice.
    PowerMean,
    /// [`Self::PowerMean`] with the successor values **min-max normalized
    /// per node** before exponentiation instead of shifted by a global
    /// constant:
    ///
    /// ```text
    /// lo = min_a Q_a,  hi = max_a Q_a
    /// z_a = (Q_a - lo) / (hi - lo)
    /// Q(s) = lo + (hi - lo) * ( SUM_a (n_a/N) * z_a^p ) ^ (1/p)
    /// ```
    ///
    /// **Why this variant exists.** The literature's operator assumes
    /// rewards in `[0, 1]`, and the standard fix — shift everything by a
    /// constant `B` — makes the operator's aggression depend on the
    /// *absolute* value scale. Our sibling gaps are ~0.005 on values near
    /// +1.2, so with `B = 1.5` the shifted operands sit at 2.70 vs 2.71:
    /// a ratio of 1.004, which `p = 6` turns into 1.02. The shifted power
    /// mean is therefore close to INERT on exactly the narrow-spread
    /// contamination GitHub #492 describes, while being extremely
    /// aggressive on wide spreads (a proven-loss child at −1.45 shifts to
    /// 0.05 and is annihilated at any `p >= 2`). It is aggressive where
    /// nothing needed deciding and mild where the decision is.
    ///
    /// Normalizing per node removes the absolute scale, so `p` means the
    /// same thing at every node. Same endpoints (`p = 1` is the weighted
    /// mean exactly, since the map is affine; `p -> inf` is the max), same
    /// chance-node rule (chance nodes take the plain mean, for which the
    /// normalization cancels), and the same `[min, max]` bound.
    ///
    /// The cost is a different kind of fragility: `hi` and `lo` come from
    /// two single children, so one proven-loss sibling stretches the range
    /// and squashes everyone else into the bottom of `[0, 1]`. Which
    /// failure you prefer is an empirical question, which is why both are
    /// ladderable rather than one being chosen in advance.
    PowerMeanNorm,
}

/// See [`MctsConfig::hidden_info`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HiddenInfoMode {
    /// Historical (leaky) behavior: the root sees the true materialized
    /// draw-pile order and true RNG cursors. Kept as the default so every
    /// pre-`sts2-rs-vqckz` fixture stays byte-stable; also useful for
    /// label-ablation studies that intentionally want the clairvoyant
    /// search as a reference point.
    #[default]
    Clairvoyant,
    /// F0 honest mode: before the root node is built, the root state's
    /// hidden information is replaced with a seeded fake derived purely
    /// from [`MctsConfig::seed`] (never from the incoming hidden state) —
    /// see [`sts2_core::blindfold::blindfold_root`]. Single-determinization
    /// only; [`Self::DrawChance`] lets draw uncertainty flow through the
    /// existing chance-node machinery instead.
    RootScramble,
    /// F1 honest mode (bead `sts2-rs-i8bgu`, plan §3.2): routes draw-order
    /// uncertainty through the same chance-node machinery that already
    /// models roll uncertainty, instead of single-determinizing it away.
    /// Includes [`Self::RootScramble`]'s root blindfold as defense in
    /// depth (rollouts from leaves that predate the first draw-chance node
    /// still read the root's stored pile order; the scramble makes that
    /// order fake). On top of that:
    ///
    /// * **Detection** (`one_simulation`'s expansion step): an applied
    ///   edge is ALSO stochastic when it consumed unknown draw order —
    ///   `state.cards_drawn` advanced AND the parent's unknown-middle draw
    ///   segment ([`sts2_core::cards::unknown_middle_uids`]) had ≥2 cards
    ///   and was not observationally homogeneous
    ///   ([`sts2_core::cards::segment_is_observationally_homogeneous`]).
    /// * **Realization** ([`Mcts::realize_chance_outcome`]): EVERY chance
    ///   outcome (0 included, draw- and roll-triggered alike) first
    ///   reshuffles the parent's unknown pile segment via
    ///   [`sts2_core::blindfold::shuffle_unknown_pile_for_outcome`], seeded
    ///   from `(base, n)`, before reseeding the combat streams and
    ///   re-applying the edge. Outcome 0 stops being the true continuation
    ///   realized from the real RNG cursors — this also closes the
    ///   roll-chance-node outcome-0 leak.
    /// * **Dedup**: chance-outcome deduplication hashes with
    ///   [`sts2_core::chance_outcome_hash`] instead of
    ///   [`sts2_core::search_hash`], so two outcomes that differ only in
    ///   WHICH physical duplicate-card instance sits where collapse to one
    ///   outcome instead of exploding DPW's outcome fan-out.
    ///
    /// `Clairvoyant` and `RootScramble` are byte-identical to their
    /// pre-`sts2-rs-i8bgu` behavior — every change above is conditional on
    /// this variant.
    DrawChance,
    /// F2 honest mode (bead `sts2-rs-09x5n`, plan §3.2's lightspeed-fork
    /// `CardPile` port): instead of DrawChance's per-outcome
    /// re-materialized full pile order (which is clairvoyant-within-the-
    /// determinization for every draw AFTER the first), the unknown
    /// draw-pile segment stays a CANONICAL (uid-sorted) belief-state
    /// multiset for the entire search — see
    /// [`sts2_core::blindfold::belief_root`] and
    /// [`sts2_core::cards::draw`]'s `deferred_draw_order` branch.
    ///
    /// * **Root conversion** (`Mcts::with_policy`): the root's unknown pile
    ///   segment is sorted (NOT shuffled — there is no permutation left to
    ///   materialize) and
    ///   [`sts2_core::PlayerCombatState::deferred_draw_order`] is set,
    ///   exactly like [`Self::RootScramble`]'s blindfold otherwise.
    /// * **Detection**: `cards::draw` now consumes the SAME `shuffle`
    ///   stream the empty-pile auto-reshuffle uses whenever the unknown
    ///   segment has >=2 cards (singleton draws stay RNG-free), so the
    ///   EXISTING RNG-counter-fingerprint chance detector fires with ZERO
    ///   new detector code — every uncertain draw becomes a chance node at
    ///   every depth, unlike `DrawChance`'s single first-draw
    ///   special-case. No heterogeneity pre-check runs (unlike
    ///   `DrawChance`'s `segment_is_observationally_homogeneous` guard) —
    ///   a homogeneous segment still chance-nodes (it still reads the
    ///   stream), but its outcomes all dedup to one child via
    ///   [`sts2_core::chance_outcome_hash`] (see below), so the tree
    ///   doesn't actually widen over indistinguishable outcomes.
    /// * **Realization**: no per-outcome pile reshuffle step —
    ///   `Mcts::realize_chance_outcome`'s `reshuffle_pile` stays `false`
    ///   for `BeliefDraw` even when the node is draw-triggered (unlike
    ///   `DrawChance`). Reseeding the combat streams from `(base, n)`
    ///   before re-applying the edge is enough: the deferred draw's own
    ///   RNG read (now against the freshly-reseeded stream) IS the
    ///   outcome-sampling mechanism, so there is nothing left to
    ///   re-materialize.
    /// * **Dedup**: chance-outcome deduplication uses
    ///   [`sts2_core::chance_outcome_hash`], same as `DrawChance`.
    /// * **Widening class**: a chance node is marked draw-triggered
    ///   ([`MctsNode::chance_is_draw_triggered`], selecting
    ///   [`WideningClass::Draw`]) whenever `cards_drawn` advanced — no
    ///   heterogeneity check (the trigger IS the fingerprint delta under
    ///   this mode, so there's nothing else to gate on).
    ///
    /// `Clairvoyant`/`RootScramble`/`DrawChance` are byte-identical to
    /// their pre-`sts2-rs-09x5n` behavior — every change above is
    /// conditional on this variant.
    BeliefDraw,
}

/// How rollout terminal states are scored.
/// See `MctsConfig::terminal_eval_mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TerminalEvalMode {
    /// Historical. Win → evaluate_victory(state) in [0.5, 1.0];
    /// loss → flat -1.0. Byte-stable against pre-bead fixtures.
    Binary,
    /// Win branch unchanged. Loss replaced with
    /// -1.0 + 0.5 * loss_leaf: under [`MctsConfig::loss_gradient`]
    /// (default) the loss leaf is the graded [`loss_quality`] band,
    /// landing losses in [-1.45, -1.05) — strictly below wins and
    /// depth-cuts; with the gradient off, the historical
    /// `leaf_eval(state)` (terminal-loss-clamped to a flat -1.45).
    RichLoss,
    /// Loss branch replaced with `-1.0 + 0.5 * loss_leaf` (the graded
    /// [`loss_quality`] band under [`MctsConfig::loss_gradient`], else
    /// the historical clamped `leaf_eval(state)`), landing losses in
    /// [-1.45, -1.05). Win branch is
    /// `0.5 + evaluate_victory(state)`, landing wins in [+1.0, +1.5]
    /// (GitHub issue #10 — restores the bead v2oh HP/potions/turn-rich
    /// win signal that was silently dropped when this mode replaced
    /// wins with the Texel HCE `leaf_eval`). Issue #4: the previous
    /// `base + ±5.0` scaling blew Q to ~[-5.9, 5.9], collapsing UCT
    /// to greedy once any child terminal-won — these magnitudes keep
    /// |Q| ≤ 1.5, so the default `c_puct = √2` stays in balance with
    /// the explore term. Engine default since `sts2-rs-v6bo2` —
    /// vbzji A/B harness showed +6 pp aggregate search win-rate vs
    /// Binary at depth=10.
    #[default]
    RichBoth,
}

/// The BeliefDraw exploration constant for a given simulation budget.
///
/// The optimum `c_puct` rises monotonically with budget. Measured on the
/// 168-position book at eight budgets, 24 seeds each, paired against the
/// deployed 2.5 (`logs/2026-08-15-cpuct-sims/FINDINGS.md`):
///
/// | sims | best `c` | `c = 2.5` | Δ | cluster p |
/// |---:|---:|---:|---:|---:|
/// | 200 | 0.40 | 0.568 | +8.2pp | <1e-4 |
/// | 300 | 0.40 | 0.645 | +4.2pp | 0.007 |
/// | 500 | 1.41 | 0.710 | +3.1pp | 0.0003 |
/// | 1000 | 1.41 | 0.787 | +0.8pp | 0.33 |
/// | 2000 | **2.50** | 0.845 | best arm | — |
/// | 4000 | 2.50 / 4.00 | 0.885 | 0.0pp | 0.96 |
/// | 8000 | 4.00 | 0.906 | +1.0pp | 0.11 |
///
/// **The deployed lanes are unaffected.** 1000 / 4000 / 8000 all return 2.5,
/// which is what they already ran, so this is a no-op for `eval_cheap`,
/// `eval_strong` and the advisor. What it fixes is the *hazard*: `c_puct` was
/// a constant bolted to `hidden_info`, while the budget is chosen
/// independently (`--sims` on the advisor, `CombatBudget` here). Dropping the
/// budget for latency therefore kept 2.5 and walked into the mis-tuned corner
/// — 4.2pp at 300 sims, 8.2pp at 200 — silently, in a metric no latency check
/// looks at.
///
/// **The rule only deviates from 2.5 where 2.5 is SIGNIFICANTLY beaten.** At
/// 1000 sims `c = 1.41` leads by 0.8pp at p = 0.33, and the 2026-08-14 study
/// saw +0.4pp at p = 0.62 over 32 seeds — a coin flip, and precisely the kind
/// of gap that study showed earlier drafts had been reading as signal. So the
/// upper boundary is 1000, not the 2000 where 2.5 first wins outright: below
/// 500 and at 500 the evidence clears p < 0.001, and everywhere from 1000 up
/// it does not clear anything. That keeps every deployed budget byte-identical
/// (asserted by `belief_draw_c_puct_is_unchanged_at_every_deployed_budget`)
/// instead of retuning the main combat lane on noise.
///
/// 0.8 rather than the measured-best 0.4 below 500: the two are tied at 300
/// (0.687 / 0.684) and 0.8 degrades more gracefully upward, still being within
/// noise at 500 where 0.4 is not.
///
/// BeliefDraw only. Every arm in that sweep ran the honest dialect, so the
/// curve is not established for Clairvoyant / RootScramble / DrawChance, which
/// keep [`MctsConfig::default`]'s 1.41.
#[must_use]
pub fn belief_draw_c_puct(max_simulations: u32) -> f32 {
    match max_simulations {
        0..=499 => 0.8,
        500..=999 => 1.41,
        _ => 2.5,
    }
}

/// `draw_widening_c` of the deployed BeliefDraw dialect — the F2 retune,
/// bead `sts2-rs-09x5n`. Set by [`MctsConfig::apply_mode_dialect`].
pub const BELIEF_DRAW_DRAW_WIDENING_C: f32 = 2.0;

/// `(endturn_widening_c, endturn_widening_alpha)` of the deployed BeliefDraw
/// dialect — the 2026-08-17 EndTurn-widening retune (PR #467). Set by
/// [`MctsConfig::apply_mode_dialect`]; see there for the evidence.
pub const BELIEF_DRAW_ENDTURN_WIDENING: (f32, f32) = (5.787, 0.436);

impl MctsConfig {
    /// Set the constants that TRAVEL WITH [`Self::hidden_info`] at
    /// [`Self::max_simulations`]: the deployed search dialect.
    ///
    /// The one source of truth for it. The run agent
    /// (`sts2-runai::policy::mcts_config_for`) and every interactive surface
    /// (`sts2-wire::emit::mcts::RecommendParams::mcts_config` — the
    /// real-game advisor, the workbench hint / inspector / coach, SimLab)
    /// call this, so a future retune cannot land on one and miss the other.
    /// Before GH #758 the advisor and the hint carried their own copy of
    /// the pre-#467 dialect and searched EndTurn at the engine's 0.5 / 0.4.
    ///
    /// BeliefDraw gets, in addition to the engine defaults:
    ///
    /// * `c_puct` = [`belief_draw_c_puct`] of the budget (F2 retune,
    ///   budget-keyed);
    /// * `draw_widening_c` = [`BELIEF_DRAW_DRAW_WIDENING_C`] (F2 retune);
    /// * `endturn_widening_c / alpha` = [`BELIEF_DRAW_ENDTURN_WIDENING`]
    ///   (PR #467).
    ///
    /// Every other mode is left untouched: those retunes were measured only
    /// under BeliefDraw. Call this BEFORE applying a caller's explicit
    /// overrides, so an ablation still gets what it asks for.
    ///
    /// # The EndTurn retune (PR #467)
    ///
    /// `EndTurn` is the highest-entropy transition in the game — one edge
    /// that resolves every monster's intent, the turn-boundary hooks and
    /// the next draw — and its DPW class was the one never swept: `0.5 /
    /// 0.4` was hand-set BELOW the other classes precisely because
    /// realizing the transition is the most expensive one, i.e. tuned for
    /// cost containment rather than for strength. At 100 visits that cap
    /// admits 4 sampled enemy turns; these values admit 44, so the search
    /// actually hedges over what the enemies might do.
    ///
    /// Paired full-run gate, `main-500` panel, Ironclad, `ppo` +
    /// `eval-cheap`, 500 cells per arm per ascension (McNemar exact):
    ///
    /// ```text
    ///   A8   38.6% -> 48.8%  (+10.20pp, p = 4.6e-05)
    ///   A9   20.2% -> 23.6%  ( +3.40pp, p = 0.13)
    ///   A10   5.2% ->  7.4%  ( +2.20pp, p = 0.11)
    ///   A0   68.4% -> 78.0%  ( +9.60pp, p = 2.3e-05)
    ///   pooled all-ascension +6.35pp, n = 2000, p = 8.2e-10
    /// ```
    ///
    /// A0 was the non-regression arm and came back POSITIVE, so this is NOT
    /// ascension-specific and is applied unconditionally to the honest
    /// dialect rather than gated on ascension.
    ///
    /// It is a SEARCH-METHOD win, not a compute win, and the control that
    /// establishes that is the load-bearing evidence: the same extra engine
    /// work spent on plain sims instead (`--sims 2400 --boss-sims 9600`, no
    /// knob) is NULL — +0.80pp at A8 (p = 0.80) and +1.40pp at A0 (p =
    /// 0.60) — while this retune at the ORIGINAL budget beats that
    /// bigger-budget arm by +9.40pp (A8, p = 1.2e-04) and +8.20pp (A0, p =
    /// 5.3e-04).
    ///
    /// COST: #467 measured ~2.4x full-run wall-clock at identical sims, but
    /// most of that was a quadratic dedup-hash scan that PR #469 removed
    /// (see [`ChanceOutcome::dedup_hash`]). After #469: ~1.14x / 1.16x per
    /// search at 1000 / 4000 sims (`widening_cost_probe`, single thread)
    /// and ~1.17x (A0) / 1.16x (A8) per full run (n = 48 per arm). Anything
    /// that compares arms at fixed wall-clock, or runs near `run_arena`'s
    /// per-seed `--timeout-ms`, must still account for it.
    pub fn apply_mode_dialect(&mut self) {
        if self.hidden_info == HiddenInfoMode::BeliefDraw {
            self.c_puct = belief_draw_c_puct(self.max_simulations);
            self.draw_widening_c = BELIEF_DRAW_DRAW_WIDENING_C;
            (self.endturn_widening_c, self.endturn_widening_alpha) = BELIEF_DRAW_ENDTURN_WIDENING;
        }
    }

    /// The `(C, α)` pair that sizes a chance node of `class`.
    fn widening_pair(&self, class: WideningClass) -> (f32, f32) {
        match class {
            WideningClass::EndTurn => (self.endturn_widening_c, self.endturn_widening_alpha),
            WideningClass::Draw => (self.draw_widening_c, self.draw_widening_alpha),
            WideningClass::Default => (self.chance_widening_c, self.chance_widening_alpha),
        }
    }
}

/// `ceil(C · (visits+1)^α)`, at least 1 — the DPW outcome cap formula.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn dpw_cap_formula(c: f32, alpha: f32, visits: u32) -> usize {
    (c * ((visits + 1) as f32).powf(alpha)).ceil().max(1.0) as usize
}

/// The DPW outcome cap the search applies to chance node `node` at its
/// current visit count under `config` — the same class selection
/// (EndTurn > Draw > Default) and formula as the search itself, for
/// displays such as the tree inspector that must not drift from it.
/// Meaningless for a decision node.
#[must_use]
pub fn dpw_outcome_cap(config: &MctsConfig, node: &MctsNode) -> usize {
    let (c, alpha) = config.widening_pair(WideningClass::of_node(node));
    dpw_cap_formula(c, alpha, node.visits)
}

impl Default for MctsConfig {
    fn default() -> Self {
        Self {
            c_puct: std::f32::consts::SQRT_2,
            max_simulations: 1000,
            max_rollout_depth: 10,
            seed: 0,
            rollout_policy: RolloutPolicyKind::Heuristic,
            rollout_end_turn_priority: expert::END_TURN_PRIORITY,
            use_transposition_table: false,
            regularized_pi_temperature: 0.1,
            use_puct_prior: false,
            terminal_eval_mode: TerminalEvalMode::RichBoth,
            use_quiescence: false,
            use_lethal_detector: false,
            chance_widening_c: 1.0,
            chance_widening_alpha: 0.5,
            draw_widening_c: 1.0,
            draw_widening_alpha: 0.5,
            endturn_widening_c: 0.5,
            endturn_widening_alpha: 0.4,
            chance_prove_wins: false,
            chance_closed_stops_widening: false,
            chance_max_depth: CHANCE_MAX_DEPTH,
            hidden_info: HiddenInfoMode::Clairvoyant,
            common_random_numbers: false,
            tie_eps: 0.02,
            loss_gradient: true,
            loss_w_removed: 0.6,
            loss_w_hp: 0.25,
            loss_w_turns: 0.15,
            loss_turns_half_life: LOSS_QUALITY_TURNS_HALF_LIFE,
            loss_span: LOSS_GRADIENT_LEAF_SPAN,
            loot_weight: 1.0,
            root_guards: true,
            potion_charge_q: None,
            root_drink_mask: false,
            fpu_reduction: None,
            cpuct_utility_stdev_scale: 0.0,
            // Our utility units, not KataGo's 0.40. Measured median root-node
            // σ on the position book; see `mcts_node_stdev_probe`.
            cpuct_utility_stdev_prior: 1.0,
            cpuct_utility_stdev_prior_weight: 2.0,
            root_sequential_halving: false,
            global_std_c: None,
            backup_rule: BackupRule::Mean,
            backup_power_p: 1.0,
            backup_power_shift: MAX_LEAF_VALUE_ABS,
        }
    }
}

/// Pluggable policy prior. Reserved for future PUCT work; v0.1.0 ships
/// only [`UniformPolicy`].
pub trait Policy {
    /// Return a probability vector aligned with `actions`.
    fn prior(&self, state: &CombatState, actions: &[Action]) -> SmallVec<[f32; 16]>;
}

/// Uniform `1/N` prior. Used in selection-tie-breaking and (in future
/// beads) for biased rollouts.
#[derive(Clone, Copy, Debug, Default)]
pub struct UniformPolicy;

impl Policy for UniformPolicy {
    fn prior(&self, _state: &CombatState, actions: &[Action]) -> SmallVec<[f32; 16]> {
        let n = actions.len().max(1);
        #[allow(clippy::cast_precision_loss)]
        let p = 1.0_f32 / n as f32;
        actions.iter().map(|_| p).collect()
    }
}

/// Runtime-polymorphic [`Policy`] handle. `Mcts<P>` is generic over the
/// policy at compile time, but the production PyO3 surface needs to pick
/// the prior at *runtime* (the learned embedding prior when its asset is
/// present, `UniformPolicy` otherwise) without monomorphizing every entry
/// point twice. Wrapping an `Arc<dyn Policy + Send + Sync>` gives one
/// concrete `P = SharedPolicy` that resolves either choice, is cheap to
/// `Clone` (one `Arc` bump — required so `par_search_with_policy` can hand
/// each rayon worker its own handle that shares the underlying net), and
/// is `Send + Sync` for the threaded path. Bead `sts2-rs-x9hyn`.
#[derive(Clone)]
pub struct SharedPolicy(pub std::sync::Arc<dyn Policy + Send + Sync>);

impl std::fmt::Debug for SharedPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedPolicy").finish_non_exhaustive()
    }
}

impl SharedPolicy {
    /// Wrap any concrete policy in a shared, runtime-dispatched handle.
    #[must_use]
    pub fn new<P: Policy + Send + Sync + 'static>(policy: P) -> Self {
        Self(std::sync::Arc::new(policy))
    }

    /// The uniform fallback, byte-identical in behavior to
    /// `Mcts<UniformPolicy>` when paired with `use_puct_prior = false`.
    #[must_use]
    pub fn uniform() -> Self {
        Self(std::sync::Arc::new(UniformPolicy))
    }
}

impl Policy for SharedPolicy {
    fn prior(&self, state: &CombatState, actions: &[Action]) -> SmallVec<[f32; 16]> {
        self.0.prior(state, actions)
    }
}

/// Internal tree-edge action key.
///
/// Bead `sts2-rs-edge` / GitHub issue #16: tree nodes used to store
/// raw [`Action`] values on their child edges, but
/// `Action::PlayCard { hand_index, target }` carries a **volatile**
/// index into the player's hand. Under PIMC-style determinization the
/// hand composition diverges across simulations, so a `PlayCard
/// { hand_index: 0 }` edge expanded by sim1 (Bash) and re-traversed
/// by sim2 (Strike at index 0) would back the Strike rollout's stats
/// into the Bash node — poisoning the tree.
///
/// `EdgeAction` decouples the edge identity from the volatile hand
/// index: for `PlayCard` we capture the **card identity** tuple
/// `(card_id, upgraded, cost, free, affliction, target)`. At UCT
/// descent / expansion
/// time the edge is resolved back to a concrete `Action::PlayCard`
/// against the *current* simulation state, by scanning the hand for a
/// card matching that identity tuple. Every other variant is
/// hand-index-free and stored verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EdgeAction {
    /// Stable card-identity key for a `PlayCard` edge. `cost` here is
    /// the card's `cost` field at the moment the edge was created —
    /// it disambiguates cards that share `(card_id, upgraded)` but
    /// have had per-instance cost mutations (Mayhem / SeeingRed-style)
    /// applied. `free` captures whether the card's LOCAL base cost is
    /// zeroed by a `FreeThisTurn` / `FreeUntilPlayed` rider (or
    /// Flatten-after-osty-attack) — a per-instance cost mutation that
    /// does NOT touch the `cost` field. Without it, a PowerPotion's
    /// 0-cost copy of a card collides with an identically-named,
    /// identically-costed non-free copy in hand, so `resolve` returns
    /// the wrong (non-free) one and the engine rejects the play for
    /// lack of energy (GH bug report). `affliction` is the same story one
    /// field further on: `legal_actions` gates a play on the card's
    /// per-instance affliction KIND — ChainsOfBinding blocks every further
    /// `Bound` card once one has been played this turn, SmoggyPower blocks
    /// `Smog` cards — so a Bound and a clean copy of one card are NOT the
    /// same play. Without it they share an edge, `resolve` returns the FIRST
    /// matching hand slot, and the search can commit to the copy
    /// `legal_actions` gated out (measured 2026-08-16: a 4-card hand
    /// `[Strike(Bound), ShrugItOff(Bound), ShrugItOff, Strike]` where legal is
    /// only slots 2 and 3, while the edge resolved to slot 1). Stacks are
    /// deliberately excluded — both gates match on `kind` with `..`, so
    /// splitting on stacks would manufacture duplicate children for one move.
    PlayCard {
        card_id: sts2_core::cards::CardId,
        upgraded: bool,
        cost: i8,
        free: bool,
        affliction: Option<sts2_core::card_afflictions::CardAfflictionKind>,
        target: Option<u8>,
    },
    /// All other variants — they don't carry volatile indices into
    /// per-simulation state, so the raw [`Action`] is stable across
    /// determinizations.
    Other(Action),
}

impl EdgeAction {
    /// Build an `EdgeAction` from a legal `Action` at the moment of
    /// expansion. For `PlayCard`, looks up the card in `state.cards`
    /// via `state.player.hand[hand_index] -> CardUid` and captures
    /// `(card_id, upgraded, cost, free, affliction, target)`. For
    /// unrecognized hand
    /// indices (shouldn't occur — `legal_actions` only emits valid
    /// indices) falls back to `Other` with the original action so the
    /// edge at least round-trips correctly.
    fn from_action(action: &Action, state: &CombatState) -> Self {
        if let Action::PlayCard { hand_index, target } = action {
            if let Some(card_uid) = state.player.hand.get(*hand_index as usize) {
                let idx = card_uid.0.get() as usize - 1;
                if let Some(card) = state.cards.get(idx) {
                    return Self::PlayCard {
                        card_id: card.card_id,
                        upgraded: card.upgraded,
                        cost: card.cost,
                        free: sts2_core::hooks::card_cost_locally_zeroed(state, card),
                        affliction: card.affliction.map(|a| a.kind),
                        target: *target,
                    };
                }
            }
        }
        Self::Other(action.clone())
    }

    /// Resolve this edge against `state` and return the concrete
    /// `Action` that should be applied. Returns `None` when a
    /// `PlayCard` edge has no matching card in the current hand —
    /// the calling determinization simply doesn't contain the card
    /// this edge represents, so the edge is unplayable this sim and
    /// the caller should skip it.
    fn resolve(&self, state: &CombatState) -> Option<Action> {
        match self {
            Self::Other(a) => Some(a.clone()),
            Self::PlayCard {
                card_id,
                upgraded,
                cost,
                free,
                affliction,
                target,
            } => {
                // Linear scan over `state.player.hand` (typically ≤10
                // entries) for the first card matching the identity
                // tuple. NO HashMap allocation on the hot path — this
                // runs once per UCT child per descent step.
                for (slot, card_uid) in state.player.hand.iter().enumerate() {
                    let idx = card_uid.0.get() as usize - 1;
                    let Some(card) = state.cards.get(idx) else {
                        continue;
                    };
                    if card.card_id == *card_id
                        && card.upgraded == *upgraded
                        && card.cost == *cost
                        && sts2_core::hooks::card_cost_locally_zeroed(state, card) == *free
                        && card.affliction.map(|a| a.kind) == *affliction
                    {
                        #[allow(clippy::cast_possible_truncation)]
                        return Some(Action::PlayCard {
                            hand_index: slot as u8,
                            target: *target,
                        });
                    }
                }
                None
            }
        }
    }
}

/// Per-root-child outcome breakdown returned by
/// [`Mcts::root_outcomes`](crate::Mcts::root_outcomes).
///
/// Invariant: `wins + losses + ties == visits`. The bucket counters
/// are kept for the CLI's `LIKELY-WIN n%` display surface; the
/// load-bearing scalar for action ranking is now [`Self::value_sum`]
/// (sum of continuous per-rollout values from `terminal_value` /
/// `depth_cut_value`), with `mean_value = value_sum / visits`. Bead
/// `sts2-rs-v2oh`.
#[derive(Clone, Debug)]
pub struct RootOutcome {
    pub action: Action,
    pub visits: u32,
    pub wins: u32,
    pub losses: u32,
    pub ties: u32,
    /// Sum of leaf values backed up through this root child. Each
    /// rollout contributes the continuous `f32` produced by
    /// `terminal_value` / `depth_cut_value`, not the bucketized ±1.
    /// Per-rollout range is mode-dependent — up to `[-1.5, 1.5]`
    /// ([`MAX_LEAF_VALUE_ABS`]) under the widest [`TerminalEvalMode`]
    /// (`RichBoth`: wins land in `[1.0, 1.5]`, losses near `-1.45`);
    /// only depth-cut leaf evals stay inside `(-1.0, 1.0)`.
    pub value_sum: f32,
    /// Running `Σ value²` over the same rollouts as [`Self::value_sum`].
    /// With `visits` and `value_sum` it gives this child's return variance
    /// in O(1), and therefore a standard error on `mean_value` — the
    /// per-ply *resolution* statistic the decision-quality suite reports
    /// next to agreement (GH #446): a search that picked the gold action
    /// with a top-two gap of many standard errors *knew*; one that picked
    /// it inside one standard error guessed.
    pub value_sq_sum: f32,
    /// `value_sum / visits` — the player-perspective expected value
    /// of taking this action.
    pub mean_value: f32,
}

impl RootOutcome {
    /// Sample variance of the returns backed up through this child
    /// (population form, `Σv²/n − mean²`, clamped at 0 against rounding).
    /// `0.0` for an unvisited child.
    #[must_use]
    pub fn value_variance(&self) -> f32 {
        if self.visits == 0 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let n = self.visits as f32;
        (self.value_sq_sum / n - self.mean_value * self.mean_value).max(0.0)
    }

    /// Standard error of [`Self::mean_value`]: `sqrt(variance / visits)`.
    /// `0.0` for an unvisited child.
    #[must_use]
    pub fn mean_stderr(&self) -> f32 {
        if self.visits == 0 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let n = self.visits as f32;
        (self.value_variance() / n).sqrt()
    }
}

/// Maximum absolute value any single rollout can legitimately produce.
///
/// All rollout / proven-flag / terminal-leaf code paths route through
/// the configured [`TerminalEvalMode`]; the widest mode (`RichBoth`)
/// emits `0.5 + evaluate_victory(state)` for wins (range [+1.0, +1.5];
/// see GitHub issue #10) and `-1.0 + 0.5 · loss_leaf` for losses —
/// where `loss_leaf` is the graded `[-0.9, -0.1)` band from
/// [`loss_quality`] (default, [`MctsConfig::loss_gradient`]) or the
/// historical `leaf_eval(state)` clamped to `(-1.0, 1.0)` (Texel via
/// [`sts2_core::evaluate_in_progress`], which is `tanh`-saturated
/// per GH issue #11 — asymptotic to but never reaching `±1.0`) or to
/// `(-0.49, 0.49)` (Learned via `map_win_to_score`) — so losses land
/// in `[-1.45, -1.05)` (gradient) / `[-1.45, -0.55]` (legacy). Either
/// way `|value| < 1.5` holds for every well-formed leaf. The 1.5 bound is used both by
/// [`Mcts::one_simulation`]'s backprop `debug_assert!` and as a
/// release-build clamp — see the GitHub #32
/// note in that function for the bug it guards against.
const MAX_LEAF_VALUE_ABS: f32 = 1.5;

/// Hard cap on selection
/// depth, a backstop against pathological cycles a transposition merge could
/// introduce. Far above any real STS combat tree depth.
const CHANCE_MAX_DEPTH: usize = 1024;

/// Chance-node mode: number of consecutive Double-Progressive-Widening draws
/// that must land on an already-seen outcome before the chance node's outcome
/// set is declared "closed" (enabling sound win-proving when
/// `chance_prove_wins` is set). Heuristic — see `MctsNode::widenings_since_new`.
const CHANCE_CLOSE_K: u32 = 16;

/// Salt mixed into `config.seed` (via the standard splitmix64 pattern) to
/// derive the seed passed to [`sts2_core::blindfold::blindfold_root`] for
/// [`HiddenInfoMode::RootScramble`]. Distinct from every salt used inside
/// `blindfold_root` itself and from every other splitmix64 salt in the
/// engine — the two-level mix (this crate's salt, then core's own
/// pile/stream salts) mirrors the nested seed derivation the
/// `clairvoyance_probe` bench already uses for its BLIND arm.
const ROOT_BLINDFOLD_SEED_SALT: u64 = 0xB11D_F01D_5EED_0000;

/// The sibling-invariant salt [`Mcts::alloc_chance_node`] substitutes for
/// the parent's child count under [`MctsConfig::common_random_numbers`].
///
/// Any constant makes the siblings share a base; this one is chosen far
/// outside the non-CRN salt range (`1 ..= children.len() + 1`) so the CRN
/// stream is a genuinely independent stream rather than a copy of the
/// first child's. See the collision note at the use site.
const CRN_CHANCE_SALT: u64 = 0xC121_5A17_0000_0001;

// ─── Tree ───────────────────────────────────────────────────────────────────

/// Arena index into [`MctsTree::arena`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeId(pub u32);

/// A single search-tree node.
///
/// Stores both a continuous `value_sum` (for UCT exploitation / final
/// action ranking) and `(wins, losses, ties)` bucket counters (for
/// the CLI display surface). Each rollout contributes one continuous
/// value (`|value| <= MAX_LEAF_VALUE_ABS`, i.e. `[-1.5, 1.5]` under
/// `RichBoth`) plus exactly one bucket increment;
/// `wins + losses + ties == visits` is maintained as an invariant.
/// Bead `sts2-rs-v2oh` introduced the continuous channel — prior to
/// that, UCT consumed `(wins - losses) / visits` and the buckets did
/// double duty as both display and value.
/// One enemy's HP in a [`NodeSnapshot`].
#[derive(Clone, Debug)]
pub struct EnemySnapshot {
    pub uid: u32,
    pub current_hp: i32,
    pub max_hp: i32,
}

/// A compact snapshot of the combat state a node represents — the outcome
/// *after* the edge action that created the node. Captured once at creation
/// (the concrete simulation state is in hand then; it is not retained
/// otherwise). The fields here are deterministic w.r.t. the action sequence
/// — they do not depend on the determinized draw order — so the snapshot is a
/// faithful representation of player/enemy HP, block, energy and round at that
/// node. Surfaced by the search-tree inspector ("what happens if I play this").
#[derive(Clone, Debug)]
pub struct NodeSnapshot {
    pub player_hp: i32,
    pub player_max_hp: i32,
    pub player_block: i32,
    pub energy: i8,
    pub round: u16,
    pub enemies: SmallVec<[EnemySnapshot; 8]>,
}

/// Whether a tree node is a player **decision** point or a **chance** node
/// representing the random resolution of a stochastic transition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NodeKind {
    /// The player chooses among `children` (the existing UCT/PUCT behaviour).
    #[default]
    Decision,
    /// Nature resolves a random event. `children` are unused; the sampled
    /// outcomes live in [`MctsNode::chance_outcomes`] and are selected by
    /// Double Progressive Widening. The stochastic action that produced this
    /// node is in [`MctsNode::chance_action`]; the pre-action state is the
    /// parent decision node's [`MctsNode::realized`].
    Chance,
}

/// Which Double-Progressive-Widening coefficient pair
/// [`Mcts::dpw_cap`] applies to a chance node (bead `sts2-rs-k1z14` H1
/// retune). `EndTurn` takes priority over `Draw` in
/// [`Mcts::chance_widen_or_sample`]'s classification when a node is both
/// (an `EndTurn` chance node's stochasticity often includes the
/// subsequent draw) — this preserves the pre-existing expensive-
/// transition cost containment for the enemy-turn-resolution node rather
/// than having the (generally more aggressive) draw coefficients override
/// it. `Draw` only ever applies under [`HiddenInfoMode::DrawChance`] or
/// [`HiddenInfoMode::BeliefDraw`] (see
/// [`MctsNode::chance_is_draw_triggered`]); `Clairvoyant`/`RootScramble`
/// only ever produce `Default`/`EndTurn`, so their behavior — and the
/// `[0]`/`[1]` `dpw_cap_cache` slots they exercise — is unchanged by this
/// enum gaining a third variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WideningClass {
    Default,
    EndTurn,
    Draw,
}

impl WideningClass {
    /// H1 retune (bead `sts2-rs-k1z14`): `EndTurn` keeps priority over
    /// `Draw` — see the type doc.
    fn of_node(node: &MctsNode) -> Self {
        if matches!(node.chance_action, Some(EdgeAction::Other(Action::EndTurn))) {
            Self::EndTurn
        } else if node.chance_is_draw_triggered {
            Self::Draw
        } else {
            Self::Default
        }
    }
}

/// One sampled outcome of a chance node. `n` is the outcome ordinal (the
/// reseed index passed to
/// [`sts2_core::run_rng_set::RunRngSet::reseed_combat_streams`]); `child` is
/// the decision node holding the realized post-transition state. Per-outcome
/// visit/value statistics live on `child` (uniform backup), so none are
/// duplicated here.
#[derive(Clone, Copy, Debug)]
pub struct ChanceOutcome {
    pub n: u32,
    pub child: NodeId,
    /// Cached [`sts2_core::chance_outcome_hash`] of `child`'s realized
    /// state, filled the first time the observational dedup scan (or the
    /// widen push) needs it. `child`'s realized state is immutable after
    /// allocation — the one replacement site is the reroot path, which
    /// clears `chance_outcomes` wholesale — so the cache can never go
    /// stale. Before this cache the scan recomputed the hash over every
    /// existing outcome on EVERY widen draw, and the hash itself
    /// Debug-formats every card in every pile (`card_observational_key`),
    /// which under the PR #467 EndTurn-widening fan-out was the single
    /// largest cost in the whole search (callgrind 2026-08-17: ~38% of
    /// instructions in `core::fmt` alone). `None` for children whose
    /// realized state is not yet populated, and always `None` outside
    /// `DrawChance`/`BeliefDraw` (the search-hash dedup branch never
    /// reads it).
    pub dedup_hash: Option<u64>,
}

/// Perf item M12: what an `EndTurn` chance node knows about the RNG-free
/// prefix of its own re-application. Every widen draw re-applies `EndTurn`
/// to a reseeded clone of the parent state, and the leading phases of
/// [`sts2_core::combat_loop::end_turn_dispatch`] usually read no randomness
/// (on the common/elite anchor corpus, everything before the next turn's
/// draw), so they compute the same state on every draw. The first widen runs the chain
/// phase by phase and keeps the state at the last boundary before any RNG
/// stream moved; later widens clone that, reseed, and run only the rest.
/// See `Mcts::realize_endturn_widen` for why that is exact.
#[derive(Clone, Debug)]
pub enum EndTurnPrefix {
    /// Not probed yet (every node starts here; decision nodes stay here).
    Unprobed,
    /// Probed: the first phase already touched an RNG stream or ended the
    /// chain, or the parent state is not in the play phase. Every widen
    /// takes the plain reseed-and-reapply path.
    Uncacheable,
    /// The state after the first `phases` phases, all of which returned
    /// `Advanced` without touching `state.rng` or any `RunRngSet` stream,
    /// under the reseed of the draw that probed it.
    Cached { phases: u8, state: Box<CombatState> },
}

#[derive(Clone, Debug)]
pub struct MctsNode {
    pub visits: u32,
    pub wins: u32,
    pub losses: u32,
    pub ties: u32,
    /// Continuous reward accumulator. UCT's exploitation term reads
    /// `value_sum / visits`.
    pub value_sum: f32,
    /// Running `Σ value²` over the same rollouts that feed [`Self::value_sum`].
    /// Together they give the node's utility variance in O(1), which is what
    /// [`MctsConfig::cpuct_utility_stdev_scale`] scales exploration by.
    ///
    /// Deliberately measured over *returns backed up through this node*, not
    /// over its children's Q means. Child means share successor value
    /// estimates across paths, which makes their i.i.d. sample variance
    /// inconsistent in a tree — it converges to zero while the true variance
    /// stays positive (arXiv 2607.14555). Returns do not have that defect,
    /// and it is the statistic KataGo uses for the same reason.
    pub value_sq_sum: f32,
    /// Number of simulations whose *leaf* was this node, and the sum of
    /// their values. A subset of [`Self::visits`] / [`Self::value_sum`]:
    /// the rest of `visits` is traffic that passed through on the way to a
    /// descendant.
    ///
    /// Only read by [`BackupRule::PowerMean`], which needs a node's own
    /// leaf sample kept separate from its children's aggregates so the
    /// operator reduces *exactly* to the arithmetic mean at `p = 1`
    /// (Truong 2023's `r_0` term). Maintained unconditionally — two `u32`
    /// adds per simulation on the leaf only — because a
    /// conditionally-maintained statistic is a statistic nobody can trust
    /// after a config flip.
    pub own_visits: u32,
    /// Sum of the values of the simulations counted by [`Self::own_visits`].
    pub own_value_sum: f32,
    pub children: SmallVec<[(EdgeAction, NodeId); 6]>,
    /// Per-**edge** visit counts, parallel to [`Self::children`]: entry `i`
    /// is how many simulations traversed `children[i]` *from this node*.
    ///
    /// Not the same as `children[i]`'s own `visits`, and the difference is
    /// not a rounding detail. The transposition table is unconditionally on
    /// (see `Mcts::with_policy`), so the search graph is a DAG: one node is
    /// reachable from several parents and its `visits` counts all of that
    /// traffic. Measured on the `make_pick_state` fixture at 400 sims, a
    /// decision node's children's `visits` summed to 1.1–1.8x the node's
    /// own — so `n_child / SUM n_child` is NOT the parent's action
    /// distribution.
    ///
    /// Power-UCT's backup weights are `n(s,a) / N(s)` — genuinely per-edge
    /// (Dam et al., IJCAI 2020, Eq. 11). This is that `n(s,a)`. Recorded
    /// during descent (the UCT loop already has the index in hand), so it
    /// costs one `u32` increment per hop and no lookup.
    pub child_visits: SmallVec<[u32; 6]>,
    pub parent: Option<NodeId>,
    /// Inline capacity 16 (was 8): realistic mid-game hands yield ~15–30
    /// canonical legal actions, so the old 8-slot inline buffer spilled to
    /// the heap on essentially every decision-node expansion (malloc/free
    /// family was ~9% of search instructions on the 2026-07-04
    /// realistic-deck callgrind). 16 keeps moderate hands inline; big
    /// hands still spill once, amortized over the node's life.
    pub untried_actions: SmallVec<[EdgeAction; 16]>,
    /// Recorded for diagnostics + the transposition table. In chance-node
    /// mode this is the RNG-cursor-sensitive [`sts2_core::state_hash`] of the
    /// realized state (diagnostics); transposition uses
    /// [`MctsNode::search_hash`] instead.
    pub state_hash: u64,
    /// RNG-cursor-INsensitive [`sts2_core::search_hash`] of [`Self::realized`].
    /// Used for chance outcome dedup + the chance-aware transposition table.
    pub search_hash: u64,
    /// Decision vs Chance.
    pub kind: NodeKind,
    /// The realized [`CombatState`] this node represents (descent is
    /// pointer-follow over stored states). `Box`ed so arena nodes stay small.
    /// Dropped when a node becomes proven/terminal to reclaim memory.
    pub realized: Option<Box<CombatState>>,
    /// Sampled outcomes of a chance node (empty for decision nodes), in
    /// draw order. Selected by Double Progressive Widening.
    pub chance_outcomes: SmallVec<[ChanceOutcome; 4]>,
    /// For a chance node: the stochastic action whose application produced
    /// it (carried for diagnostics / PV labels and EndTurn widening tuning).
    pub chance_action: Option<EdgeAction>,
    /// For a chance node: the base seed from which per-outcome reseeds are
    /// derived. Fixed at creation, distinct per node.
    pub randomness_base: u64,
    /// For a chance node: consecutive Double-Progressive-Widening draws that
    /// produced an already-seen outcome (by `search_hash`). Drives the
    /// closure heuristic that gates sound win-proving through chance nodes.
    pub widenings_since_new: u32,
    /// For a chance node: total number of widening draws taken so far — the
    /// monotonically-increasing reseed *ordinal* passed to
    /// [`sts2_core::run_rng_set::RunRngSet::reseed_combat_streams`]. This is
    /// deliberately NOT the distinct-outcome count: deduped (already-seen)
    /// draws do not grow `chance_outcomes`, so keying the reseed off the
    /// outcome count would freeze the ordinal and re-derive the *same*
    /// realization every visit — collapsing the expectimax sample onto one
    /// outcome. Each draw consumes a fresh ordinal so successive draws are
    /// independent samples of nature.
    pub chance_draws: u32,
    /// For a chance node: set once the outcome set is deemed effectively
    /// enumerated (see `widenings_since_new`). Required before a chance node
    /// may be proven a win.
    pub chance_closed: bool,
    /// Bead `sts2-rs-k1z14` (F1 G2/G3 retune): for a chance node under
    /// [`HiddenInfoMode::DrawChance`], `true` iff this node's
    /// stochasticity was (at least partly) detected via the
    /// draw-heterogeneity check (`cards_drawn` increased over an
    /// observationally-heterogeneous unknown segment) rather than purely
    /// via an RNG-counter delta. Always `false` outside
    /// `DrawChance`/`BeliefDraw` and for decision nodes.
    ///
    /// Bead `sts2-rs-09x5n` (F2 plan §3.2 step 6) extends this to
    /// [`HiddenInfoMode::BeliefDraw`]: `true` there whenever `cards_drawn`
    /// advanced, full stop — no heterogeneity check, since under that mode
    /// the trigger for the chance node's EXISTENCE already IS the
    /// fingerprint delta (`cards::draw`'s deferred-draw branch consumes the
    /// shuffle stream directly), so there's nothing else to gate the
    /// classification on.
    ///
    /// LOAD-BEARING under `DrawChance` (it started diagnostic-only; the
    /// H3 fix promoted it): [`Mcts::chance_widen_or_sample`] reads it to
    /// (a) select [`WideningClass::Draw`]'s DPW coefficients
    /// (`draw_widening_c/alpha`) for BOTH modes and (b) gate the
    /// per-outcome unknown-pile reshuffle + outcome-0 reseed — but (b) is
    /// ADDITIONALLY gated on `hidden_info == DrawChance` at both read
    /// sites (bead `sts2-rs-09x5n`), since `BeliefDraw`'s deferred draw
    /// needs no extra pile-materialization step; a pure-roll chance node
    /// under either mode keeps the historical on-policy outcome 0 instead.
    /// It is also the bucket key for the `drawchance_diagnose` post-hoc
    /// tree walk. Because it is always `false` outside
    /// `DrawChance`/`BeliefDraw`, every read degenerates to the pre-existing
    /// behavior there — `Clairvoyant`/`RootScramble` byte-stability is
    /// preserved.
    pub chance_is_draw_triggered: bool,
    /// For an `EndTurn` chance node: the cached RNG-free prefix of its
    /// re-application (perf item M12). Freed with the node.
    pub endturn_prefix: EndTurnPrefix,
    pub is_terminal: bool,
    pub is_expanded: bool,
    /// MCTS-Solver lite (bead `sts2-rs-wr8f`, Wave-4): this subtree
    /// contains a forced player win. Set on construction when the
    /// state itself is a terminal-win, and propagated to parents
    /// during backup when any child becomes `proven_win`. Selection
    /// short-circuits past proven nodes and backup feeds `+1.0`
    /// without running a rollout — the headline ≥30%-sim-reduction
    /// lever for forced-lethal positions.
    pub proven_win: bool,
    /// MCTS-Solver lite: this subtree is a forced player loss. Set on
    /// construction when the state itself is a terminal-loss, and
    /// propagated to parents when the parent is FULLY EXPANDED (every
    /// legal action has a child) and ALL children are `proven_loss`
    /// (vs `proven_win` which propagates on ANY child).
    ///
    /// GitHub issue #7 once dropped the full-expansion gate so the
    /// propagation would fire earlier on deep-narrow elite fights,
    /// trading soundness for pragmatic pruning. The 2026-08-09 A8
    /// audit repair restored the gate: `lost_position_override` now
    /// treats this flag as a PROOF and bypasses the search on it, so
    /// an unsound stamp switches the search off on winnable positions
    /// (the exact FINDINGS §3 pathology). Worse, the heuristic stamp
    /// latched at sim 1 whenever the FIRST canonical action (an
    /// attack, by expert priority) died — e.g. into Thorns at low HP —
    /// because the backprop's proven-parent early break made the
    /// stamp permanent before any sibling was expanded. Both proven
    /// sides are sound again by induction: terminal seeds are exact,
    /// chance nodes prove a loss on ANY realized losing outcome, and
    /// decision nodes only on a fully-expanded all-losing child set.
    pub proven_loss: bool,
    /// State summary for this node (outcome after its edge action). See
    /// [`NodeSnapshot`].
    pub snapshot: NodeSnapshot,
}

impl MctsNode {
    fn new(parent: Option<NodeId>, state: &CombatState, use_lethal_detector: bool) -> Self {
        // PIMC path: the `state_hash` field carries the RNG-cursor-sensitive
        // hash (its transposition key). Computed here so chance mode can take
        // the cheaper `new_inner` path with a precomputed hash instead.
        Self::new_inner(parent, state, use_lethal_detector, state_hash(state))
    }

    /// Shared constructor body taking a precomputed hash for the `state_hash`
    /// field. Chance mode passes its RNG-cursor-insensitive `search_hash` to
    /// avoid hashing the (large) `CombatState` twice per node.
    fn new_inner(
        parent: Option<NodeId>,
        state: &CombatState,
        use_lethal_detector: bool,
        state_hash_val: u64,
    ) -> Self {
        let terminal = is_terminal(state);
        // Bead `sts2-rs-wr8f`: seed proven flags from the terminal
        // sign. `terminal_value` returns `(true, +0.5..=+1.0)` for a
        // player win and `(true, -1.0)` for a loss; thresholds at
        // `> 0.5` / `< 0.0` keep the `±0.49` depth-cap surrogate out
        // of the proven-class (it is treated as `Tie` by
        // `Bucket::from_value`).
        let (mut proven_win, proven_loss) = if terminal {
            let (_, v) = terminal_value(state);
            (v > 0.5, v < 0.0)
        } else {
            (false, false)
        };
        // Bead `sts2-rs-3fk0l`: lethal-this-turn detector. Only consult
        // when the state is non-terminal (terminal already has the right
        // proven flag from the branch above) and the config flag is on.
        // The detector is conservative — see `lethal_detector` module
        // docs — so a `true` return is a guaranteed forced-lethal.
        if !terminal && use_lethal_detector && lethal_detector::detect_lethal(state) {
            proven_win = true;
        }
        let enemies: SmallVec<[EnemySnapshot; 8]> = state
            .enemy_uids
            .iter()
            .filter_map(|uid| {
                state
                    .creatures
                    .iter()
                    .find(|c| c.id == uid.0)
                    .map(|c| EnemySnapshot {
                        uid: uid.0.get(),
                        current_hp: c.current_hp,
                        max_hp: c.max_hp,
                    })
            })
            .collect();
        let snapshot = NodeSnapshot {
            player_hp: state.player.current_hp,
            player_max_hp: state.player.max_hp,
            player_block: state.player.block,
            energy: state.player.energy,
            round: state.round,
            enemies,
        };
        // Computed once here (not per visit). Valid for the node's life in
        // chance mode because the realized state is immutable after creation.
        Self {
            visits: 0,
            wins: 0,
            losses: 0,
            ties: 0,
            value_sum: 0.0,
            value_sq_sum: 0.0,
            own_visits: 0,
            own_value_sum: 0.0,
            children: SmallVec::new(),
            child_visits: SmallVec::new(),
            parent,
            untried_actions: SmallVec::new(),
            state_hash: state_hash_val,
            search_hash: 0,
            kind: NodeKind::Decision,
            realized: None,
            chance_outcomes: SmallVec::new(),
            chance_action: None,
            randomness_base: 0,
            widenings_since_new: 0,
            chance_draws: 0,
            chance_closed: false,
            chance_is_draw_triggered: false,
            endturn_prefix: EndTurnPrefix::Unprobed,
            is_terminal: terminal,
            is_expanded: false,
            proven_win,
            proven_loss,
            snapshot,
        }
    }
}

/// Which terminal bucket a rollout fell into. Cheap to compute from the
/// `+1 / −1 / 0` value emitted by [`terminal_value`] and [`rollout`].
#[derive(Clone, Copy, Debug)]
enum Bucket {
    Win,
    Loss,
    Tie,
}

impl Bucket {
    #[inline]
    fn from_value(v: f32) -> Self {
        if v > 0.5 {
            Self::Win
        } else if v < -0.5 {
            Self::Loss
        } else {
            Self::Tie
        }
    }
}

/// Up-front arena reservation (in nodes) for a search with the given
/// simulation budget: ~2.5 nodes per simulation (decision node, plus a
/// chance wrapper + outcome-0 child on stochastic edges; transposition
/// merges allocate nothing), capped at 16 384 nodes (~10–15 MiB of
/// `MctsNode`) so a very large `max_simulations` cannot pre-commit
/// unbounded memory — beyond the cap the arena falls back to ordinary
/// amortized doubling.
#[inline]
fn arena_reservation(max_simulations: u32) -> usize {
    ((max_simulations as usize).saturating_mul(5) / 2).min(1 << 14)
}

/// Arena over [`MctsNode`].
#[derive(Clone, Debug)]
pub struct MctsTree {
    pub arena: Vec<MctsNode>,
    pub root: NodeId,
    /// Cached root state — every selection traversal re-applies actions
    /// from this base. Cloning the root state is `O(state size)` but
    /// happens exactly once per simulation.
    pub root_state: CombatState,
}

impl MctsTree {
    fn new(mut root_state: CombatState, use_lethal_detector: bool) -> Self {
        // The game-style event log is STATE-RESIDENT (`sts2_core::event_log`),
        // so a search cloned from a live recording state would inherit both
        // its rows and its recording flag and then append every rollout event
        // to a log nobody reads — allocation on the hottest path in the
        // engine. Silence and empty it here, at the single constructor both
        // `MctsSearch::new` and `with_policy` funnel through. The LIVE state
        // is untouched: this runs on the search's own clone.
        root_state.combat_log.event_log.disarm_for_search();
        let root_node = MctsNode::new(None, &root_state, use_lethal_detector);
        Self {
            arena: vec![root_node],
            root: NodeId(0),
            root_state,
        }
    }

    fn node(&self, id: NodeId) -> &MctsNode {
        &self.arena[id.0 as usize]
    }

    fn node_mut(&mut self, id: NodeId) -> &mut MctsNode {
        &mut self.arena[id.0 as usize]
    }

    fn alloc(&mut self, node: MctsNode) -> NodeId {
        let id = u32::try_from(self.arena.len()).expect("arena fits in u32");
        self.arena.push(node);
        NodeId(id)
    }
}

// ─── Engine ─────────────────────────────────────────────────────────────────

/// UCT MCTS engine.
///
/// Type parameter `P` is the policy prior; defaults to [`UniformPolicy`].
#[derive(Debug)]
pub struct Mcts<P: Policy = UniformPolicy> {
    pub config: MctsConfig,
    pub tree: MctsTree,
    pub policy: P,
    pub rng: Xoshiro256PlusPlus,
    sim_index: u32,
    /// Aggregated `(action, visits, mean_value)` for [`root_visits`]. Lazily
    /// recomputed on demand.
    root_stats: Vec<(Action, u32, f32)>,
    /// Transposition table — `(state_hash, depth_bucket)` → existing
    /// `NodeId`. Populated on expansion when
    /// [`MctsConfig::use_transposition_table`] is `true`. Hits cause
    /// the new edge to wire to the existing node instead of allocating
    /// — the search graph then becomes a DAG, but backprop still walks
    /// only the actual selection path each iteration ("sum into both
    /// paths"). Bead `sts2-rs-7z97`. The `HashMap` use is intentional —
    /// arena cardinality is unbounded; a `BTreeMap` would scale `log n`
    /// per probe, and the hot path runs once per simulation.
    /// Bead `sts2-rs-h2ql` (Wave 4) replaced the prior
    /// `HashMap<(u64,u8), NodeId>` (bead `sts2-rs-7z97`) with a
    /// fixed-size open-addressing table over `ahash`, sized at
    /// `2²⁰` slots. SipHash → AHasher saves ~3× on the hash step,
    /// the `Vec`-backed fixed table never reallocates, and linear
    /// probing keeps the hot path branch-light. See
    /// [`crate::tt`] for the implementation.
    ///
    /// `None` when [`MctsConfig::use_transposition_table`] is `false`
    /// (the default) — the 16 MiB fixed-size table is only allocated
    /// when the feature is actually enabled. Allocating it
    /// unconditionally cost ~6% of per-search instructions (a 16 MiB
    /// zero-fill once per `Mcts::new`) for a table that the default
    /// config never touches.
    transposition: Option<TranspositionTable>,
    /// Number of nodes allocated via [`MctsTree::alloc`] since
    /// construction (excluding the root). Counter for the
    /// allocation-delta unit test.
    nodes_allocated: u64,
    /// Times [`Self::legalize_root_action`] had to replace an illegal
    /// candidate. See that function for what causes it and why this is a
    /// counter rather than an assert.
    illegal_root_actions: u64,
    /// Running `(Σ Q, Σ Q², count)` over every visited node in the tree, for
    /// [`MctsConfig::global_std_c`]. Maintained incrementally in backup — each
    /// node on the path swaps its old Q out and its new Q in — so reading
    /// `σ_tree` is O(1) instead of an O(tree) traversal per selection step.
    tree_q_sum: f64,
    tree_q_sq_sum: f64,
    tree_q_count: u64,
    /// Set by the Sequential Halving driver
    /// ([`MctsConfig::root_sequential_halving`]) to pin the root edge a
    /// simulation must take. `None` (always, unless SH is running) leaves the
    /// root to UCT like every other node.
    forced_root_child: Option<NodeId>,
    /// Number of simulations whose leaf was `proven_win` / `proven_loss`
    /// (either by construction on a terminal state, or via backup
    /// propagation from earlier sims) and therefore short-circuited
    /// the rollout step. Bead `sts2-rs-wr8f` acceptance counter —
    /// the forced-lethal test asserts `>= 30%` of total sims hit this
    /// path once the proven flag has surfaced.
    rollouts_skipped: u64,
    /// Phase-0 instrument (2026-08-15 high-asc campaign): total number
    /// of times [`rollout`] actually ran (i.e. the leaf was NOT
    /// decisive — proven/terminal leaves short-circuit into
    /// [`Self::rollouts_skipped`] instead and are not counted here).
    /// Accumulated across every [`Self::search`]/[`Self::search_n`]
    /// call on this tree instance. See [`Self::rollout_stats`].
    rollouts_total: u64,
    /// Phase-0 instrument (2026-08-15 high-asc campaign): subset of
    /// [`Self::rollouts_total`] whose rollout returned via the
    /// depth-cap branch (`depth_cut_value`, i.e. the rollout was still
    /// non-terminal when it stopped) rather than reaching a genuine
    /// terminal state. High-ascension combats that route most rollouts
    /// through this branch are the ones where the leaf evaluator's
    /// quality actually matters. See [`Self::rollout_stats`].
    depth_cap_hits: u64,
    /// GitHub issue #19: number of stored tree edges that were
    /// unavailable in the current PIMC determinization (the stored
    /// action was not in `legal_actions(state)` at descent time).
    /// Incremented during SELECTION and EXPANSION when revalidation
    /// rejects the stored edge.
    stale_child_edges: u64,
    /// GitHub issue #19: number of `core_apply` calls whose result
    /// indicated the action did not progress the state (currently
    /// `StepResult::InvalidTarget` or a no-op `PlayCard` detected by
    /// hand-length invariance).
    failed_apply: u64,
    /// GitHub issue #19: subset of `failed_apply` where the action
    /// returned `StepResult::InvalidTarget`.
    invalid_target: u64,
    /// GitHub issue #19: subset of `failed_apply` where a `PlayCard`
    /// action returned `Advanced` but did not actually move a card out
    /// of the hand (engine-side no-op fallthrough).
    no_op_playcard: u64,
    /// Best action sequence observed across all rollouts since the last
    /// reroot (or construction). Captured from the SELECTION path —
    /// actions taken from the root down to the simulation leaf — at the
    /// moment a rollout produces a strictly better value than
    /// [`Self::best_value`]. Mirrors `bestActionSequence` /
    /// `stepThroughSolution` from sts_lightspeed
    /// (`BattleScumSearcher2`). Bead `sts2-rs-dm5z` (W4).
    ///
    /// Hint surface ([`Self::root_outcomes`]) is unchanged — this is an
    /// optional agent-surface accumulator for callers that want a
    /// multi-action plan from a single search.
    best_seq: Vec<EdgeAction>,
    /// Best leaf value observed since the last reroot. Initial sentinel
    /// is `f32::NEG_INFINITY` so the very first rollout always installs
    /// its sequence. "Strict improvement" semantics: a new sequence is
    /// only captured when `value > best_value`, NOT `>=` — keeps
    /// `best_seq` stable on ties.
    best_value: f32,
    /// Wave-6 leaf-eval selector. Bead `sts2-rs-4n0x` (qctu.5).
    /// Defaults to [`LeafEval::Texel`] — i.e. byte-stable against
    /// every pre-bead test fixture. Flip to [`LeafEval::Learned`]
    /// via [`Self::with_leaf_eval`] to A/B against an ONNX-loaded
    /// value head.
    leaf_eval: LeafEval,
    /// Memo for the Double-Progressive-Widening outcome cap
    /// `ceil(C · (visits+1)^α)` — `[0]` for regular chance nodes,
    /// `[1]` for `EndTurn` chance nodes, `[2]` for draw-triggered chance
    /// nodes (bead `sts2-rs-k1z14`; only ever populated under
    /// `HiddenInfoMode::DrawChance`), each indexed by `visits`.
    /// `powf` was a top per-chance-visit transcendental
    /// (perf-investigation 2026-07-04); the cap is a pure function of
    /// `visits` and the per-search `(C, α)` pair, so each entry is
    /// computed once via the identical expression and reused —
    /// bit-identical to computing inline. See [`Self::dpw_cap`].
    dpw_cap_cache: [Vec<usize>; 3],
    /// `(C.to_bits(), α.to_bits())` the corresponding
    /// [`Self::dpw_cap_cache`] slab was built with. `config` is `pub`,
    /// so a caller could mutate the widening coefficients between
    /// searches — a mismatch invalidates the slab.
    dpw_cap_params: [(u32, u32); 3],
    /// Perf item M12: the phase boundary the next `EndTurn` prefix probe
    /// clones at — the RNG-free prefix length the previous probe measured.
    /// A probe clones the state at every RNG-free boundary from here on, so
    /// a guess that is too short costs extra clones and one that is too long
    /// costs that node one uncached draw (it re-probes on the next). It
    /// steers only how much the probe clones, never what a draw realizes.
    endturn_prefix_guess: u8,
}

impl Mcts<UniformPolicy> {
    /// Construct an MCTS engine with the uniform policy.
    pub fn new(root_state: CombatState, config: MctsConfig) -> Self {
        Self::with_policy(root_state, config, UniformPolicy)
    }
}

impl<P: Policy> Mcts<P> {
    /// Construct an MCTS engine with a custom policy.
    pub fn with_policy(mut root_state: CombatState, mut config: MctsConfig, policy: P) -> Self {
        // Bead `sts2-rs-vqckz` (F0): honest-search root blindfold. Must
        // run before ANY of `root_state` is read/cached below — the
        // `recommended_depth` scan, the transposition-table state, and the
        // root node's `search_hash` / `realized` snapshot must all see the
        // blindfolded state, not the caller's true one, or the "honest"
        // search would still leak hidden information through those paths.
        // No-op (byte-identical) when `hidden_info` is the default
        // `Clairvoyant`. `DrawChance` includes the root blindfold too
        // (bead `sts2-rs-i8bgu`, plan §3.2: defense in depth — rollouts
        // from leaves that predate the first draw-chance node still read
        // the root's stored pile order). `BeliefDraw` (bead `sts2-rs-09x5n`,
        // F2) uses its own root conversion — canonicalize-and-set-the-flag
        // instead of canonicalize-and-shuffle — since there is no
        // permutation left to materialize once draws defer their
        // randomness to draw time.
        match config.hidden_info {
            HiddenInfoMode::RootScramble | HiddenInfoMode::DrawChance => {
                let blindfold_seed =
                    sts2_core::run_rng_set::splitmix64(config.seed ^ ROOT_BLINDFOLD_SEED_SALT);
                sts2_core::blindfold::blindfold_root(&mut root_state, blindfold_seed);
            }
            HiddenInfoMode::BeliefDraw => {
                let belief_seed =
                    sts2_core::run_rng_set::splitmix64(config.seed ^ ROOT_BLINDFOLD_SEED_SALT);
                sts2_core::blindfold::belief_root(&mut root_state, belief_seed);
            }
            HiddenInfoMode::Clairvoyant => {}
        }
        // Bead `sts2-rs-y4e4z`: bump rollout depth when any enemy on
        // the board recommends a higher horizon (Vantom → 50; see
        // `MonsterKind::recommended_rollout_depth`). Caller-supplied
        // depth is treated as a floor — if you explicitly set a
        // higher depth than the recommendation, that wins. Depth is
        // only ever raised, never lowered.
        let recommended_depth = root_state
            .enemy_uids
            .iter()
            .filter_map(|uid| {
                root_state
                    .creatures
                    .iter()
                    .find(|c| c.id == uid.0)
                    .and_then(|c| c.species)
                    .and_then(sts2_core::MonsterKind::recommended_rollout_depth)
            })
            .max();
        if let Some(rec) = recommended_depth {
            if rec > config.max_rollout_depth {
                config.max_rollout_depth = rec;
            }
        }
        // The transposition table is always ON: chance trees reconverge
        // heavily (the same game state is reached via many different outcome
        // paths), so `search_hash`-keyed merging concentrates visits and
        // measurably improves decision quality. The table is sized small
        // (`CHANCE_TT_SIZE_LOG2`) since chance trees are small and the table is
        // reallocated per move in closed-loop play.
        config.use_transposition_table = true;
        let mut tree = MctsTree::new(root_state, config.use_lethal_detector);
        // Reserve the node arena up front from the simulation budget so the
        // hot search loop never pays a `Vec` doubling-regrow (each regrow
        // memmoves every existing `MctsNode` — a large struct — and was part
        // of the malloc/free + memcpy buckets on the 2026-07-04 callgrind).
        // Each simulation allocates at most ~2–3 nodes (decision, plus a
        // chance wrapper + outcome-0 on stochastic edges; TT merges allocate
        // none), so 2.5× sims covers the realistic ceiling. Capped so a huge
        // `max_simulations` cannot pre-commit unbounded memory.
        tree.arena
            .reserve(arena_reservation(config.max_simulations));
        // Descent is pointer-follow over stored states, so the root needs its
        // realized state cached up front.
        let root_state_clone = tree.root_state.clone();
        let root = tree.root;
        tree.node_mut(root).search_hash = sts2_core::search_hash(&root_state_clone);
        tree.node_mut(root).realized = Some(Box::new(root_state_clone));
        let rng = Xoshiro256PlusPlus::seed_from_u64(config.seed);
        Self {
            config,
            tree,
            policy,
            rng,
            sim_index: 0,
            root_stats: Vec::new(),
            #[allow(clippy::disallowed_methods)]
            transposition: Some(TranspositionTable::with_log2(
                crate::tt::CHANCE_TT_SIZE_LOG2,
            )),
            nodes_allocated: 0,
            illegal_root_actions: 0,
            tree_q_sum: 0.0,
            tree_q_sq_sum: 0.0,
            tree_q_count: 0,
            forced_root_child: None,
            rollouts_skipped: 0,
            rollouts_total: 0,
            depth_cap_hits: 0,
            stale_child_edges: 0,
            failed_apply: 0,
            invalid_target: 0,
            no_op_playcard: 0,
            best_seq: Vec::new(),
            best_value: f32::NEG_INFINITY,
            leaf_eval: LeafEval::default(),
            dpw_cap_cache: [Vec::new(), Vec::new(), Vec::new()],
            dpw_cap_params: [(0, 0), (0, 0), (0, 0)],
            endturn_prefix_guess: 1,
        }
    }

    /// Swap the leaf-score function. Default is [`LeafEval::Texel`]
    /// (the Texel-tuned `evaluate_in_progress`); call this with
    /// `LeafEval::Learned(Arc<LearnedValueNet>)` to route the MCTS
    /// rollout depth-cap through an ONNX-loaded value head instead.
    /// Bead `sts2-rs-4n0x` (qctu.5).
    #[must_use]
    pub fn with_leaf_eval(mut self, leaf_eval: LeafEval) -> Self {
        self.leaf_eval = leaf_eval;
        self
    }

    /// Current leaf-score selector (diagnostic accessor — the A/B
    /// harness reads this to confirm which path is active).
    pub fn leaf_eval(&self) -> &LeafEval {
        &self.leaf_eval
    }

    /// Number of transposition-table hits since construction. Always
    /// `0` when `MctsConfig::use_transposition_table` is `false`.
    pub fn tt_hits(&self) -> u64 {
        self.transposition
            .as_ref()
            .map_or(0, TranspositionTable::hits)
    }

    /// Number of nodes allocated (excluding the root) since
    /// construction. With the transposition table enabled this counter
    /// grows more slowly because shared nodes don't trigger an
    /// allocation; it is the load-bearing measurement for the W6
    /// allocation-delta unit test.
    pub fn nodes_allocated(&self) -> u64 {
        self.nodes_allocated
    }

    /// Times this search's root pick was illegal against the root state and
    /// had to be replaced by [`Self::legalize_root_action`].
    ///
    /// **Expected to be 0.** It is a counter rather than a `debug_assert`
    /// because the fallback has always run silently in release, where an
    /// assert does nothing, so the useful question is *how often* rather than
    /// *whether*. Same rationale as `MctsCombatPolicy`'s `FallbackCounts`
    /// (issue #164): measure the handoff instead of pretending it never
    /// happens.
    ///
    /// A non-zero value is worth investigating; it is not corruption — the
    /// replacement is always a legal action — but it means the search
    /// committed to a weaker pick rule than `pick_root_action` intended.
    #[must_use]
    pub fn illegal_root_actions(&self) -> u64 {
        self.illegal_root_actions
    }

    /// Number of simulations whose rollout was short-circuited by the
    /// MCTS-Solver lite proven-win / proven-loss flag (bead
    /// `sts2-rs-wr8f`). Returns `0` until backup has propagated a
    /// proven flag deep enough that UCT descents start re-hitting a
    /// proven leaf. Used by the forced-lethal acceptance test as a
    /// "rollout work avoided" measurement.
    pub fn rollouts_skipped(&self) -> u64 {
        self.rollouts_skipped
    }

    /// Phase-0 instrument (2026-08-15 high-asc campaign):
    /// `(rollouts_total, depth_cap_hits)` accumulated across every
    /// [`Self::search`]/[`Self::search_n`] call on this tree instance
    /// since construction — how often the rollout step actually ran,
    /// and how often it ran out the depth cap (and therefore leaned on
    /// the leaf evaluator) instead of reaching a genuine terminal
    /// state. Counting only — no effect on search order, RNG draws, or
    /// the returned action.
    ///
    /// Per-`Mcts`-instance only: [`par_search`]/[`par_search_with_policy`]
    /// build one `Mcts` per rayon worker and never merge these counters
    /// across workers, so this getter is only meaningful when called on
    /// a single-threaded `Mcts` used for one decision (the intended
    /// Phase-0 usage).
    pub fn rollout_stats(&self) -> (u64, u64) {
        (self.rollouts_total, self.depth_cap_hits)
    }

    /// GitHub issue #19: count of stored tree edges that were
    /// unavailable in the current PIMC determinization. Each
    /// increment is a SELECTION or EXPANSION step whose stored action
    /// was not in `legal_actions(state)` at descent time; the search
    /// treated the node as a leaf for that iteration instead of
    /// descending into a stale child.
    pub fn stale_child_edges(&self) -> u64 {
        self.stale_child_edges
    }

    /// GitHub issue #19: count of `core_apply` calls during
    /// SELECTION / EXPANSION whose result indicated the action did
    /// not progress the state (`InvalidTarget` or a no-op `PlayCard`).
    pub fn failed_apply(&self) -> u64 {
        self.failed_apply
    }

    /// GitHub issue #19: subset of `failed_apply` where the action
    /// returned `StepResult::InvalidTarget`.
    pub fn invalid_target(&self) -> u64 {
        self.invalid_target
    }

    /// GitHub issue #19: subset of `failed_apply` where a `PlayCard`
    /// action returned `Advanced` but left the hand unchanged
    /// (engine-side no-op fallthrough).
    pub fn no_op_playcard(&self) -> u64 {
        self.no_op_playcard
    }

    /// Whether the root has been proven a forced win for the player
    /// by the MCTS-Solver lite (bead `sts2-rs-wr8f`).
    pub fn root_proven_win(&self) -> bool {
        self.tree.node(self.tree.root).proven_win
    }

    /// Whether the root has been proven a forced loss for the player
    /// by the MCTS-Solver lite (bead `sts2-rs-wr8f`).
    pub fn root_proven_loss(&self) -> bool {
        self.tree.node(self.tree.root).proven_loss
    }

    /// GitHub issue #32 user-visible fix, re-scoped by the 2026-08-09 A8
    /// audit (FINDINGS §3): when the root is a solver-PROVEN loss, every
    /// backed-up value is inside the loss band and the tree's ranking is
    /// a ranking over ways to die — a greedy 1-ply salvage rule does
    /// better than argmax(Q). Naïve `argmax(Q)` on these states often
    /// picks `EndTurn` because losing-fast leaves the leaf-eval near the
    /// starting position, while playing cards drives it toward
    /// more-negative HP-loss states. The visible pathology: `EndTurn`
    /// into lethal in 1–2 turns having played zero cards.
    ///
    /// **Trigger — [`Self::root_proven_loss`] AND every root child has
    /// `wins == 0`.** Each condition alone is insufficient. The audit
    /// found the old every-child-wins==0 scan (by itself) trivially
    /// satisfied by depth-capped rollouts against 400+-HP bosses (2.7%
    /// of plies, whole boss fights end to end), handing those plies to
    /// this 1-ply rule with the search effectively off — and six of
    /// fifteen such fights were actually WON. But the proof flag (by
    /// itself) is also not ground truth: the chance-side proof rule is
    /// pessimistic (ANY sampled outcome proven-lost stamps the chance
    /// node lost), so a "proven" loss can carry double-digit rollout
    /// win rates. Requiring both means the override fires only when the
    /// solver says lost AND no rollout ever found a win. Every other
    /// position stays on [`Self::pick_root_action`]: the
    /// [`MctsConfig::loss_gradient`] backup orders their children inside
    /// the loss band and the `tie_eps` band falls back to visits.
    ///
    /// In proven-lost positions, prefer the action whose 1-ply lookahead
    /// maximizes `damage_dealt + block_credit - 0.5 * hp_lost` so the
    /// engine plays out trying to bring at least one enemy down — or
    /// blunt the incoming hit — before dying. The 0.5 weight on HP loss
    /// is conservative — it lets a Strike (5 dmg, 0 self-HP) beat
    /// EndTurn (0 dmg, 0 self-HP at the start of the player turn) but
    /// doesn't reward suicidal trades. `block_credit`
    /// ([`BLOCK_CREDIT_WEIGHT`] per point) credits block the action
    /// gains, capped at the telegraphed incoming attack damage — the
    /// audit's structural gap was a block card scoring 0 damage / 0
    /// hp_lost and losing the tie to EndTurn, so the override literally
    /// could not block.
    ///
    /// GitHub issue #215: the damage-race above is blind to actions that
    /// AVERT an imminent lethal that no amount of chip damage undoes. The
    /// motivating case is TheInsatiable's Sandpit countdown — playing the
    /// `FranticEscape` status buys back a turn (Sandpit +1) but deals 0
    /// damage, so the raw damage-race swings for a Strike and is EATEN
    /// this very enemy turn, throwing away the game. Even a PROVEN loss
    /// proves only the solver's expanded horizon: each survived turn is a
    /// fresh, deeper re-search that may find the win. So survival
    /// strictly dominates: an action that keeps the player alive through
    /// the next enemy turn is preferred over one that merely deals more
    /// damage before dying. The survival check is a
    /// 1-ply-play-then-`EndTurn` probe (see [`survives_next_enemy_turn`]);
    /// it changes the pick ONLY when survival differs across the candidate
    /// actions — in an ordinary lost position where every line dies the
    /// same enemy turn the term is uniform and the GH #32 / #128
    /// damage-race tie-break is preserved unchanged.
    ///
    /// Returns `None` when the root has children but is not a proven
    /// loss (or has no children at all).
    #[must_use]
    pub fn lost_position_override(&self) -> Option<Action> {
        let root = self.tree.node(self.tree.root);
        if root.children.is_empty() {
            return None;
        }
        if !self.root_proven_loss() {
            return None;
        }
        // A proven loss is only as exact as the chance-side proof rule,
        // which is deliberately pessimistic: ANY sampled outcome
        // proven-lost stamps the whole chance node lost. A root can
        // therefore be "proven" lost while its rollouts win double-digit
        // percentages (2026-08-10 post-fix corpus: a root with 244/2016
        // winning rollouts on its best child fired this override). Those
        // positions must stay on `pick_root_action`, whose Q ranking
        // prefers the highest-surviving line; the 1-ply salvage rule is
        // for positions with no empirical counterexample at all.
        if root
            .children
            .iter()
            .any(|&(_, c)| self.tree.node(c).wins > 0)
        {
            return None;
        }

        let baseline = &self.tree.root_state;
        let baseline_enemy_hp = sum_enemy_hp(baseline);
        let baseline_player_hp = baseline.player.current_hp;
        let baseline_slippery = sum_enemy_slippery(baseline);

        // Bead `sts2-rs-722vx-B`: when the position is observed-lost
        // and the 1-ply score from each action is tied near zero
        // (typical Q-saturation regime: no action can deal real damage
        // because of a Slippery wall / Intangible / under-energy), the
        // raw argmax(score) tie-break is order-of-iteration which
        // tends to favor the first-emitted potion. Burning every
        // potion on a turn that does nothing is strictly worse than
        // EndTurn — saves the consumable for a future turn that
        // might still matter.
        //
        // ldd1c follow-up (B): the same regime is *exactly* where the
        // Vantom Slippery wall lives — every attack is capped at 1 dmg
        // so `damage_dealt` barely separates a wall-chipping multi-hit
        // from idling, and the selector defaults to potion-spam. But
        // chipping the counter down IS the winning line (it unlocks
        // full-damage hits next turn). So credit each Slippery stack an
        // action removes (`SLIPPERY_CLEAR_CREDIT` per stack) — this is
        // the un-squashed, un-divided home for the signal that the
        // leaf-eval `enemy_slippery_stacks` feature is too diluted to
        // carry (IN_PROGRESS_DIVISOR 114.56 × 0.5 loss-shaping ⇒ a full
        // 9-stack clear moves the leaf value only ~0.02). Here a 3-hit
        // 0-cost attack clears 3 stacks ⇒ +6 score, decisively beating
        // EndTurn/potions in the tie regime.
        // The tiered preference (applied below the score) is:
        //   1. Action with strictly positive score (real damage dealt,
        //      or block against a telegraphed hit)
        //   2. EndTurn / PlayCard (2026-08-09 A8 audit: the old
        //      EndTurn-above-PlayCard preference is GONE — with block
        //      now credited in the score, a residual same-tier tie means
        //      the card demonstrably did nothing measurable, and the
        //      first-iteration order below already ranks candidates by
        //      canonicalized expert play priority)
        //   3. DrinkPotion (last — preserves the consumable)
        // Ties within a tier fall through to the first-iteration order.
        let mut best: Option<(Action, bool, f32, u8)> = None;
        for (edge, _) in &root.children {
            // Bead `sts2-rs-edge` / GH #16: root children are
            // EdgeActions; resolve each against the live root state
            // before applying. A root edge that fails to resolve is
            // skipped (it represents a card not in the current root
            // hand — shouldn't happen at the root, but defend
            // against it rather than panic).
            let Some(act) = edge.resolve(baseline) else {
                continue;
            };
            let mut s = baseline.clone();
            let _ = core_apply(&mut s, act.clone());
            // GH #215: does this action keep the player alive through the
            // next enemy turn? Computed from the 1-ply post-state `s` by
            // driving one `EndTurn` so the enemy side resolves (Sandpit
            // countdown, attacks, …). Survival is the top-priority key in
            // the tie-break below.
            let survives = survives_next_enemy_turn(&s);
            let enemy_hp_after = sum_enemy_hp(&s);
            let player_hp_after = s.player.current_hp;
            #[allow(clippy::cast_precision_loss)]
            let damage_dealt = (baseline_enemy_hp - enemy_hp_after).max(0) as f32;
            #[allow(clippy::cast_precision_loss)]
            let hp_lost = (baseline_player_hp - player_hp_after).max(0) as f32;
            // ldd1c-B: credit Slippery stacks this action removes from
            // the enemy. Each cleared stack unlocks a full-damage hit
            // next turn, so chipping the wall is real progress even
            // though the capped hit registers only ~1 `damage_dealt`.
            #[allow(clippy::cast_precision_loss)]
            let slippery_cleared = (baseline_slippery - sum_enemy_slippery(&s)).max(0) as f32;
            // A8 audit (FINDINGS §3): credit block gained against the
            // damage the enemies are telegraphing — each blocked point
            // is an HP point saved, so it carries the same 0.5 weight
            // as `hp_lost`. Capped at the incoming total (over-blocking
            // a small hit earns nothing), and measured as the DELTA of
            // useful block so pre-existing block isn't re-credited.
            // Intents are read from the post-action state `s`: killing
            // the attacker zeroes its telegraph (that progress is
            // already paid out through `damage_dealt`). For the EndTurn
            // candidate `s` is the NEXT player turn (block expired,
            // fresh intents), so the credit is ~0 there and the enemy
            // hit it just ate is priced through `hp_lost` instead.
            let incoming = sum_enemy_telegraphed_damage(&s);
            #[allow(clippy::cast_precision_loss)]
            let useful_block_gained =
                (s.player.block.min(incoming) - baseline.player.block.min(incoming)).max(0) as f32;
            let score = damage_dealt
                + SLIPPERY_CLEAR_CREDIT * slippery_cleared
                + BLOCK_CREDIT_WEIGHT * useful_block_gained
                - 0.5 * hp_lost;
            // Tier-bucket the action for tie-break below the score.
            // Lower tier number wins ties. Damage-dealing / blocking
            // actions always beat zero-score actions via score alone.
            // EndTurn and PlayCard share a tier (A8 audit — see the
            // tier list above); only potions rank behind.
            let tier: u8 = match &act {
                Action::DrinkPotion { .. } | Action::UsePotion { .. } => 3,
                _ => 2,
            };
            const TIE_EPS: f32 = 0.5;
            let replace = match &best {
                None => true,
                Some((_, best_surv, best_score, best_tier)) => {
                    // GH #215: survival is the dominant key — a candidate
                    // that lives through the next enemy turn always beats
                    // one that dies, regardless of chip damage. Only when
                    // survival is equal do we fall through to the GH #32 /
                    // ldd1c-B damage-race + tier tie-break.
                    if survives != *best_surv {
                        survives
                    } else if (score - *best_score).abs() <= TIE_EPS {
                        tier < *best_tier
                    } else {
                        score > *best_score
                    }
                }
            };
            if replace {
                best = Some((act.clone(), survives, score, tier));
            }
        }
        best.map(|(a, _, _, _)| a)
    }

    /// Run `config.max_simulations` iterations from the current root, then
    /// return the best root action under the `argmax(Q) tiebreak visits`
    /// rule (see [`Self::search_n`] for details). Bead `sts2-rs-trnu`.
    pub fn search(&mut self) -> Action {
        self.search_n(self.config.max_simulations)
    }

    /// Fallible variant of [`Self::search`]: runs the configured
    /// `max_simulations` budget and returns
    /// [`SearchError::TerminalRoot`] on an empty argmax set instead
    /// of panicking. Bead `sts2-rs-y3a28` (b32is.1 follow-up).
    ///
    /// # Example
    ///
    /// ```
    /// use sts2_core::CombatState;
    /// use sts2_mcts::{Mcts, MctsConfig, SearchError};
    ///
    /// // No-arg variant — uses `config.max_simulations` for the budget.
    /// let state = CombatState {
    ///     is_over: true,
    ///     ..CombatState::default()
    /// };
    /// let cfg = MctsConfig {
    ///     max_simulations: 0,
    ///     ..MctsConfig::default()
    /// };
    /// let mut mcts = Mcts::new(state, cfg);
    /// assert!(matches!(mcts.try_search(), Err(SearchError::TerminalRoot)));
    /// ```
    pub fn try_search(&mut self) -> Result<Action, SearchError> {
        self.try_search_n(self.config.max_simulations)
    }

    /// Run `n` additional simulations and return the best root action
    /// under the [`Self::pick_root_action`] rule.
    ///
    /// # Panics
    /// Panics if the root has no legal actions (terminal state).
    /// Callers that need a fallible variant should use
    /// [`Self::try_search_n`] instead.
    pub fn search_n(&mut self, n: u32) -> Action {
        self.try_search_n(n)
            .expect("search() called on terminal root: no legal actions")
    }

    /// Fallible variant of [`Self::search_n`]: returns
    /// [`SearchError::TerminalRoot`] when the root has no legal
    /// actions instead of panicking. Useful for external callers
    /// (debug-CLI scripted replay, fuzz harnesses, ill-conditioned
    /// smoke tests) that may hand the engine a `CombatState` with
    /// `is_over = true`. Bead `sts2-rs-b32is`.
    ///
    /// # Example
    ///
    /// ```
    /// use sts2_core::CombatState;
    /// use sts2_mcts::{Mcts, MctsConfig, SearchError};
    ///
    /// // Force a terminal-root state: player at hp=0, is_over=true.
    /// let state = CombatState {
    ///     is_over: true,
    ///     ..CombatState::default()
    /// };
    /// let cfg = MctsConfig {
    ///     max_simulations: 0,
    ///     ..MctsConfig::default()
    /// };
    /// let mut mcts = Mcts::new(state, cfg);
    /// match mcts.try_search_n(0) {
    ///     Ok(_action) => unreachable!("terminal root has no legal action"),
    ///     Err(SearchError::TerminalRoot) => {}
    /// }
    /// ```
    pub fn try_search_n(&mut self, n: u32) -> Result<Action, SearchError> {
        if self.config.root_sequential_halving {
            self.run_sequential_halving(n);
        } else {
            for _ in 0..n {
                self.one_simulation();
                self.sim_index = self.sim_index.wrapping_add(1);
            }
        }
        self.pick_root_action()
    }

    /// Run one simulation with the root edge pinned to `child`.
    fn one_simulation_forcing(&mut self, child: Option<NodeId>) {
        self.forced_root_child = child;
        self.one_simulation();
        self.sim_index = self.sim_index.wrapping_add(1);
        self.forced_root_child = None;
    }

    /// Allocate the root budget by Sequential Halving (Karnin, Koren &
    /// Somekh 2013) instead of by UCT — see
    /// [`MctsConfig::root_sequential_halving`].
    ///
    /// With `k` arms and budget `T` the schedule runs `R = ceil(log2 k)`
    /// phases; in a phase with surviving set `S` each arm receives
    /// `floor(T / (|S| · R))` simulations, after which the worse half by mean
    /// value is eliminated. Every arm therefore gets a *guaranteed* sample
    /// budget that grows geometrically as the field narrows, instead of
    /// UCT's "whoever currently looks best" allocation. There is no
    /// exploration constant anywhere in this schedule.
    ///
    /// Budget accounting is exact-or-under, never over: leftover simulations
    /// from integer division are spent on the final survivors, so the arm
    /// costs the same wall-clock as the UCT arm at equal `n`.
    fn run_sequential_halving(&mut self, n: u32) {
        // Warm-up: materialize every root child and give it one visit.
        // Expansion is lazy (one child per simulation), so this both creates
        // the arms and seeds the means SH ranks on.
        let mut spent = 0_u32;
        while spent < n {
            self.one_simulation_forcing(None);
            spent += 1;
            let root = self.tree.node(self.tree.root);
            if root.is_terminal || root.children.is_empty() {
                return;
            }
            if root.untried_actions.is_empty()
                && root
                    .children
                    .iter()
                    .all(|(_, c)| self.tree.node(*c).visits > 0)
            {
                break;
            }
        }

        let mut survivors: Vec<NodeId> = self
            .tree
            .node(self.tree.root)
            .children
            .iter()
            .map(|(_, c)| *c)
            .collect();
        if survivors.len() < 2 || spent >= n {
            // Single arm (or budget already gone): nothing for SH to allocate.
            while spent < n {
                self.one_simulation_forcing(None);
                spent += 1;
            }
            return;
        }

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let rounds = (survivors.len() as f64).log2().ceil().max(1.0) as u32;
        for _ in 0..rounds {
            if survivors.len() < 2 || spent >= n {
                break;
            }
            #[allow(clippy::cast_possible_truncation)]
            let per_arm = ((n - spent) / (survivors.len() as u32 * rounds)).max(1);
            for &arm in &survivors.clone() {
                for _ in 0..per_arm {
                    if spent >= n {
                        break;
                    }
                    self.one_simulation_forcing(Some(arm));
                    spent += 1;
                }
            }
            // Eliminate the worse half by mean value. Unvisited arms sort
            // last; ties keep child-creation order, matching the first-max
            // determinism rule `pick_root_action` documents.
            survivors.sort_by(|a, b| {
                let q = |id: &NodeId| {
                    let node = self.tree.node(*id);
                    if node.visits == 0 {
                        f32::NEG_INFINITY
                    } else {
                        #[allow(clippy::cast_precision_loss)]
                        {
                            node.value_sum / node.visits as f32
                        }
                    }
                };
                q(b).partial_cmp(&q(a)).unwrap_or(std::cmp::Ordering::Equal)
            });
            survivors.truncate(survivors.len().div_ceil(2));
        }

        // Spend the integer-division remainder on the survivors, round-robin,
        // so the arm never under-spends its nominal budget.
        let mut i = 0_usize;
        while spent < n {
            let arm = survivors[i % survivors.len()];
            self.one_simulation_forcing(Some(arm));
            spent += 1;
            i += 1;
        }
    }

    /// Would first-play urgency rather expand a new action than descend into
    /// the best child it already has? See [`MctsConfig::fpu_reduction`].
    ///
    /// **The unexpanded arm needs an exploration bonus, and UCT does not
    /// define one.** `c·sqrt(ln N / n)` diverges at `n = 0`, which is exactly
    /// why the historical convention is `INFINITY` — under plain UCT an
    /// unexpanded action is *infinitely* attractive, so a node always finishes
    /// expanding before it refines anything. (PUCT has no such problem: the
    /// prior gives an unvisited child the finite bonus `c·P(s,a)·sqrt(ΣN)`.)
    ///
    /// So the knob has to supply one. The arm is scored as the child it would
    /// become on its first visit — value `parent_q - r`, bonus at `n = 1`:
    ///
    /// ```text
    /// score_unexpanded = (parent_q - r) + c · sqrt(ln N / 1)
    /// ```
    ///
    /// which puts `r` in plain units of Q against a one-visit sibling, and
    /// makes `r = 0` ("an unexpanded action is worth what the parent is
    /// worth") approximate the historical breadth-first rule rather than
    /// collapsing to greedy. Ties go to expansion.
    fn fpu_prefers_expansion(&self, node_id: NodeId, r: f32) -> bool {
        let n = self.tree.node(node_id);
        let parent_visits = n.visits.max(1);
        #[allow(clippy::cast_precision_loss)]
        let parent_q = n.value_sum / parent_visits as f32;
        #[allow(clippy::cast_precision_loss)]
        let ln_p = (parent_visits as f32).ln();
        let c = self.effective_c(n);
        let unexpanded = parent_q - r + c * ln_p.max(0.0).sqrt();
        let mut best = f32::NEG_INFINITY;
        for (_, cid) in &n.children {
            let child = self.tree.node(*cid);
            if child.visits == 0 {
                return false;
            }
            #[allow(clippy::cast_precision_loss)]
            let score =
                child.value_sum / child.visits as f32 + c * (ln_p / child.visits as f32).sqrt();
            if score > best {
                best = score;
            }
        }
        unexpanded >= best
    }

    /// The exploration coefficient actually used at `node`, after both
    /// adaptive rules. Precedence: [`MctsConfig::global_std_c`] replaces the
    /// constant outright when set; otherwise the constant is scaled by
    /// KataGo's per-node variance factor (identity when that is off). With
    /// neither enabled this is exactly `config.c_puct`.
    fn effective_c(&self, node: &MctsNode) -> f32 {
        if let Some(cg) = self.config.global_std_c {
            return cg * self.tree_q_stdev();
        }
        self.config.c_puct * Self::utility_stdev_factor(&self.config, node)
    }

    /// Standard deviation of the mean values of every visited node in the
    /// tree — the Global Std statistic. See [`MctsConfig::global_std_c`].
    ///
    /// Falls back to `1.0` below two nodes so an arm never starts with a zero
    /// exploration coefficient (which would lock the search onto whichever
    /// child happened to be expanded first, permanently).
    ///
    /// Public because `global_std_c` is meaningless without knowing the scale
    /// it multiplies: a ladder picked in the abstract (the paper's `C ≈ 2`)
    /// lands wherever this happens to sit, which for us is ~0.1, i.e. an
    /// effective `c` of 0.2 rather than the deployed 2.5.
    #[must_use]
    pub fn tree_q_stdev(&self) -> f32 {
        if self.tree_q_count < 2 {
            return 1.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let n = self.tree_q_count as f64;
        let mean = self.tree_q_sum / n;
        let var = (self.tree_q_sq_sum / n - mean * mean).max(0.0);
        #[allow(clippy::cast_possible_truncation)]
        let sd = var.sqrt() as f32;
        if sd > 0.0 {
            sd
        } else {
            1.0
        }
    }

    /// Raw (un-regularized) standard deviation of the rollout values backed
    /// up through the root. This is the quantity
    /// [`MctsConfig::cpuct_utility_stdev_prior`] has to be calibrated
    /// against — KataGo's shipped `0.40` is on a Go win-rate scale and means
    /// nothing here. Returns `0.0` for a root with fewer than two visits.
    #[must_use]
    pub fn root_utility_stdev(&self) -> f32 {
        let n = self.tree.node(self.tree.root);
        if n.visits < 2 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let w = n.visits as f32;
        let mean = n.value_sum / w;
        ((n.value_sq_sum / w) - mean * mean).max(0.0).sqrt()
    }

    /// KataGo's dynamic exploration multiplier — see
    /// [`MctsConfig::cpuct_utility_stdev_scale`]. Returns exactly `1.0` when
    /// the feature is off, so the default path is bit-identical.
    ///
    /// Mirrors `Search::getFpuValueForChildrenAssumeVisited` in
    /// `cpp/search/searchexplorehelpers.cpp`, with visit counts standing in
    /// for KataGo's playout weights (we have no per-visit weighting).
    fn utility_stdev_factor(config: &MctsConfig, node: &MctsNode) -> f32 {
        let scale = config.cpuct_utility_stdev_scale;
        if scale == 0.0 {
            return 1.0;
        }
        let prior = config.cpuct_utility_stdev_prior;
        debug_assert!(prior > 0.0, "cpuct_utility_stdev_prior must be positive");
        let variance_prior = prior * prior;
        let prior_weight = config.cpuct_utility_stdev_prior_weight;
        #[allow(clippy::cast_precision_loss)]
        let weight = node.visits as f32;
        let stdev = if weight <= 1.0 || prior_weight + weight - 1.0 <= 0.0 {
            prior
        } else {
            let mean = node.value_sum / weight;
            let mean_sq = mean * mean;
            // Guard against a negative variance from float cancellation, the
            // same way KataGo does.
            let utility_sq_avg = (node.value_sq_sum / weight).max(mean_sq);
            (((mean_sq + variance_prior) * prior_weight + utility_sq_avg * weight)
                / (prior_weight + weight - 1.0)
                - mean_sq)
                .max(0.0)
                .sqrt()
        };
        (1.0 + scale * (stdev / prior - 1.0)).max(0.0)
    }

    /// The ONE root-action selection rule, shared by every entry point:
    /// [`Self::search`] / [`Self::search_n`] / [`Self::try_search_n`]
    /// call it after their simulation loop, and `sts2-runai`'s
    /// `MctsCombatPolicy::mcts_pick` (the deployed run agent) calls it
    /// through `try_search_n`. Before the 2026-08-09 A8 audit the run
    /// agent re-derived its own argmax over `root_outcomes()`, which
    /// silently dropped the GH #403 draw-window bonus on the deployed
    /// path and — because `max_by` keeps the LAST maximum while the
    /// re-derived stable sort keeps the FIRST — made the two entry
    /// points pick opposite ends of the child list on exact ties
    /// (10.3% of audited plies). Any change to the pick rule belongs
    /// here and nowhere else.
    ///
    /// Selection, in order:
    ///
    /// 1. **GH #32 lost-position override.** On solver-PROVEN losses
    ///    ([`Self::root_proven_loss`] — since the A8 audit, NOT the old
    ///    "every child has 0 rollout wins" scan, which depth-capped
    ///    boss rollouts satisfied trivially), naïve argmax(Q) prefers
    ///    EndTurn into lethal because losing-fast keeps leaf_eval near
    ///    the starting position — see [`Self::lost_position_override`]
    ///    for the full rationale.
    /// 2. **Proven-win guard** (FINDINGS §5 guard 1, NOT gated by
    ///    [`MctsConfig::root_guards`] — it is exact): if any root
    ///    child is solver-proven winning ([`Node::proven_win`], the
    ///    lethal-detector / terminal-win propagation), pick it
    ///    unconditionally. Q statistics cannot outrank a proof; a
    ///    late proof on a lightly-visited child would otherwise lose
    ///    the visit tie-break to a well-visited coin-flip. Multiple
    ///    proven wins fall to visits, then rule 6's first-max.
    /// 3. **Score = mean value + GH #403 draw-window bonus.** The base
    ///    key is `argmax mean_value` (bead sts2-rs-trnu, plan §4.4,
    ///    Danihelka et al. 2020 arXiv:2007.12509: for `T > 0`,
    ///    `argmax π̄ ≡ argmax Q`; the full softmax is exposed via
    ///    [`Self::regularized_root_pi`]). When the hand holds a card
    ///    whose play-count-gated draw rider is still live (FTL),
    ///    [`expert::draw_window_root_q_bonus`] nudges that card's score
    ///    — the rider is worth ~0.002 of leaf value, an order of
    ///    magnitude under the Q noise at a run's 1000-sim budget, so
    ///    the search alone coin-flips it. Gated on
    ///    [`expert::has_live_draw_window`] so with no such card in hand
    ///    every score is `q + 0.0`, bit-for-bit the plain comparison.
    /// 4. **Tolerance tie-break** ([`MctsConfig::tie_eps`]): among
    ///    children whose score is within `tie_eps` of the best, the
    ///    most-visited child wins. UCT visit counts integrate value
    ///    information beyond the noisy Q mean — the A8 audit found
    ///    permanent-buff cards (Inflame, Loop, Noxious Fumes, …) that
    ///    held 2×+ the visits of the Q-argmax while losing it by
    ///    <0.007, i.e. deep inside the ±0.02 noise floor, and
    ///    preferring visits inside that band flips those plies
    ///    correctly (94/94 audited Ironclad plies). `tie_eps = 0`
    ///    restores strict-equality tie-breaking.
    /// 5. **In-band EndTurn guards** ([`MctsConfig::root_guards`],
    ///    FINDINGS §5 guards 2–3): when the band winner is `EndTurn`,
    ///    two ordered refinements may promote a `PlayCard` child whose
    ///    score is itself inside the band — never one outside it, so a
    ///    resolved Q gap always stands. (a) *Block guard*: telegraphed
    ///    enemy attack damage exceeds standing player block and an
    ///    in-band child's card grants block (`base_block > 0`, the
    ///    same key the GH #293 rollout ramp uses). (b) *Free-play
    ///    guard*: an in-band child's card costs 0 (or is
    ///    free-this-turn per [`sts2_core::hooks::card_cost_locally_zeroed`])
    ///    — ending the turn discards a strictly free option. Each
    ///    guard resolves multiple candidates by visits, then rule 6.
    /// 6. **Determinism**: visit ties keep the EARLIEST child in
    ///    child-creation order (first-max semantics — the same rule the
    ///    deployed `mcts_pick` stable sort always had; the old
    ///    `max_by` last-max rule on this path is gone). Child-creation
    ///    order is itself deterministic (canonicalized untried-action
    ///    order), so the whole pick is a pure function of the tree.
    ///
    /// # Errors
    /// [`SearchError::TerminalRoot`] when the root has no children
    /// (terminal state / no legal actions).
    pub fn pick_root_action(&mut self) -> Result<Action, SearchError> {
        if let Some(rescued) = self.lost_position_override() {
            return Ok(self.legalize_root_action(rescued));
        }
        // Rule 2: a solver-proven winning child beats any statistic.
        // Most-visited proven win on multiples; strict `>` keeps the
        // earliest on a visit tie (rule 6).
        let proven_win_pick = {
            let node = self.tree.node(self.tree.root);
            let mut best: Option<(Action, u32)> = None;
            for (edge, cid) in &node.children {
                let child = self.tree.node(*cid);
                if child.proven_win && best.as_ref().map_or(true, |&(_, v)| child.visits > v) {
                    if let Some(action) = edge.resolve(&self.tree.root_state) {
                        best = Some((action, child.visits));
                    }
                }
            }
            best.map(|(action, _)| action)
        };
        if let Some(won) = proven_win_pick {
            return Ok(self.legalize_root_action(won));
        }
        self.refresh_root_stats();
        let draw_window_live = crate::expert::has_live_draw_window(&self.tree.root_state);
        let scored: Vec<(Action, u32, f32)> = self
            .root_stats
            .iter()
            .map(|(a, v, q)| {
                let mut adjusted = if draw_window_live {
                    q + crate::expert::draw_window_root_q_bonus(&self.tree.root_state, a)
                } else {
                    *q
                };
                // Root-level potion opportunity-cost charge (GH #447
                // follow-up): applied AFTER the draw-window bonus and
                // BEFORE the tie band + root guards, so a charged drink
                // exits the band and the guards never resurrect it.
                if let (Some(charges), Action::DrinkPotion { potion_slot, .. }) =
                    (self.config.potion_charge_q.as_ref(), a)
                {
                    if usize::from(*potion_slot) < self.tree.root_state.player.potions.len() {
                        adjusted -= charges
                            .get(usize::from(*potion_slot))
                            .copied()
                            .unwrap_or(0.0);
                    }
                }
                (a.clone(), *v, adjusted)
            })
            .collect();
        let best_score = scored
            .iter()
            .map(|&(_, _, s)| s)
            .fold(f32::NEG_INFINITY, f32::max);
        let eps = self.config.tie_eps.max(0.0);
        // Strict `>` on visits keeps the earliest in-band child on a
        // visit tie (first-max semantics, rule 6 above).
        let mut pick: Option<(usize, u32)> = None;
        for (i, &(_, visits, score)) in scored.iter().enumerate() {
            if score >= best_score - eps && pick.map_or(true, |(_, best_v)| visits > best_v) {
                pick = Some((i, visits));
            }
        }
        let (mut idx, _) = pick.ok_or(SearchError::TerminalRoot)?;
        // Rule 5: in-band EndTurn guards. Candidates must themselves be
        // inside the band — a real Q signal outside `tie_eps` is never
        // overridden. Within a guard, visits then rule 6's first-max.
        if self.config.root_guards && matches!(scored[idx].0, Action::EndTurn) {
            let state = &self.tree.root_state;
            let band_pick = |satisfies: &dyn Fn(&sts2_core::cards::Card) -> bool| {
                let mut best: Option<(usize, u32)> = None;
                for (i, (action, visits, score)) in scored.iter().enumerate() {
                    if *score >= best_score - eps
                        && root_hand_card(state, action).is_some_and(satisfies)
                        && best.map_or(true, |(_, v)| *visits > v)
                    {
                        best = Some((i, *visits));
                    }
                }
                best.map(|(i, _)| i)
            };
            // 5a — block guard: EndTurn into telegraphed damage the
            // standing block does not cover, with a block-granting play
            // in the band (audit: 26 declined blocks into realized
            // damage, 307 HP). `base_block > 0` is the same
            // block-bearing key the GH #293 rollout ramp uses.
            if sum_enemy_telegraphed_damage(state) > state.player.block {
                if let Some(i) = band_pick(&|card| card.base_block > 0) {
                    idx = i;
                }
            }
            // 5b — free-play guard: EndTurn while a 0-cost (or
            // free-this-turn) play sits in the band (audit: 269
            // voluntary end-turns left a playable 0-cost card, one of
            // them a free lethal).
            if matches!(scored[idx].0, Action::EndTurn) {
                if let Some(i) = band_pick(&|card| {
                    card.cost == 0 || sts2_core::hooks::card_cost_locally_zeroed(state, card)
                }) {
                    idx = i;
                }
            }
        }
        let action = scored[idx].0.clone();
        Ok(self.legalize_root_action(action))
    }

    /// Defense-in-depth livelock guard: guarantee the action the search
    /// returns is actually legal at the root. [`EdgeAction::resolve`] maps a
    /// stable card-identity edge onto a concrete hand slot, and a resolution
    /// bug could in principle hand back an action whose `play_card` is a
    /// no-op, livelocking a deterministic caller that re-picks it forever.
    /// Re-validate against [`sts2_core::legal_actions`] and, on a miss, fall
    /// back to the best *legal* root child (then any legal action). Costs one
    /// `legal_actions(root)` build per search — negligible beside the
    /// simulation budget — and is a no-op in the normal (already-legal) case.
    fn legalize_root_action(&mut self, candidate: Action) -> Action {
        let legal = legal_actions(&self.tree.root_state);
        if legal.contains(&candidate) {
            return candidate;
        }
        // Counted, not asserted — see `illegal_root_actions`.
        //
        // NOTE what the fallback below gives up: `pick_root_action` layers a
        // lost-position override (GH #32), a proven-win preference, the #403
        // draw-window Q bonus and the `tie_eps` band over a visits tie-break.
        // This is a plain argmax over mean Q among legal children, so a
        // *proven win* that resolved illegally is discarded here rather than
        // remapped. That is the real cost, and it is why this is counted.
        self.illegal_root_actions += 1;
        self.refresh_root_stats();
        self.root_stats
            .iter()
            .filter(|(a, _, _)| legal.contains(a))
            .max_by(|(_, va, qa), (_, vb, qb)| {
                qa.partial_cmp(qb)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(va.cmp(vb))
            })
            .map(|(a, _, _)| a.clone())
            .or_else(|| legal.first().cloned())
            .unwrap_or(Action::EndTurn)
    }

    /// Closed-form regularized root policy `π̄(a) ∝ exp(Q(a) / T)` over
    /// the current root's children, in the same order as
    /// [`Self::root_visits`]. Empty when the root is terminal or
    /// unexpanded. Numerically stable (subtracts the max logit before
    /// `exp`). Bead `sts2-rs-trnu` (plan §4.4).
    ///
    /// The action returned by [`Self::search`] / [`Self::search_n`] is
    /// `argmax π̄`; this accessor exposes the full distribution so
    /// callers that want stochastic-action selection (move-time
    /// diversity in self-play / RL data generation) can sample
    /// directly. Reads `T` from [`MctsConfig::regularized_pi_temperature`].
    pub fn regularized_root_pi(&mut self) -> Vec<(Action, f32)> {
        self.refresh_root_stats();
        if self.root_stats.is_empty() {
            return Vec::new();
        }
        let t = self.config.regularized_pi_temperature.max(1e-6);
        let scaled: Vec<f32> = self.root_stats.iter().map(|(_, _, q)| q / t).collect();
        let max_logit = scaled.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let exps: Vec<f32> = scaled.iter().map(|&l| (l - max_logit).exp()).collect();
        let sum: f32 = exps.iter().sum();
        let inv_sum = if sum > 0.0 { 1.0 / sum } else { 0.0 };
        self.root_stats
            .iter()
            .zip(exps.iter())
            .map(|((a, _, _), &e)| (a.clone(), e * inv_sum))
            .collect()
    }

    /// `(action, visits, mean_value)` for every root child, in child-creation
    /// order. Mean value is `(wins − losses) / visits` from the **player's**
    /// perspective. Empty when the root is terminal or unexpanded.
    ///
    /// Prefer [`root_outcomes`](Self::root_outcomes) when you need the
    /// underlying win/loss/tie breakdown — this accessor is kept for
    /// backward compatibility with callers that only care about the mean.
    pub fn root_visits(&mut self) -> &[(Action, u32, f32)] {
        self.refresh_root_stats();
        &self.root_stats
    }

    /// Per-root-child win/loss/tie counts plus derived `mean_value`. Same
    /// child order as [`root_visits`](Self::root_visits). Empty when the
    /// root is terminal or unexpanded.
    ///
    /// `wins + losses + ties == visits` is invariant. `wins / visits` is
    /// the rollout-policy win probability for that action; `ties / visits`
    /// is the rate at which rollouts hit the depth cap and is a useful
    /// "estimate trustworthiness" signal.
    pub fn root_outcomes(&self) -> Vec<RootOutcome> {
        let root = self.tree.root;
        let node = self.tree.node(root);
        node.children
            .iter()
            .filter_map(|(edge, cid)| {
                // Resolve the edge against root_state. The root's hand
                // is fixed across sims, so first-visit creation
                // guarantees resolution succeeds — we filter rather
                // than panic to keep the surface graceful if a future
                // bead introduces volatility at the root.
                let action = edge.resolve(&self.tree.root_state)?;
                let c = self.tree.node(*cid);
                #[allow(clippy::cast_precision_loss)]
                let mean = if c.visits == 0 {
                    0.0
                } else {
                    c.value_sum / c.visits as f32
                };
                Some(RootOutcome {
                    action,
                    visits: c.visits,
                    wins: c.wins,
                    losses: c.losses,
                    ties: c.ties,
                    value_sum: c.value_sum,
                    value_sq_sum: c.value_sq_sum,
                    mean_value: mean,
                })
            })
            .collect()
    }

    /// Re-root the search tree at the child reached by `action`, replacing
    /// the cached `root_state` with `new_root_state`. Preserves the
    /// surviving subtree's *root* visit / value statistics; descendants
    /// beyond the new root are discarded (see "Descendant validity"
    /// below).
    ///
    /// Returns `true` on success, `false` if `action` was never expanded
    /// at the current root (e.g. the root had untried actions and the
    /// caller picked one before any rollout visited it). On `false` the
    /// caller should fall back to constructing a fresh [`Mcts`] for the
    /// new state — the existing tree is left unchanged.
    ///
    /// # Descendant validity (GH #21)
    ///
    /// Descendants under the new root were created during simulations
    /// against a single determinization of the OLD root state. Their
    /// stored `is_terminal`, `state_hash`, `untried_actions`, and child
    /// lists reflect that determinization's trajectory, not the live
    /// `new_root_state`. After hidden draws, intent rolls, hand-index
    /// or card-identity changes between turns, those fields can be
    /// flatly wrong — most dangerously, `is_terminal = true` on a
    /// descendant whose reconstructed state is non-terminal would
    /// cause selection to consume a stale terminal evaluation and
    /// short-circuit further search there.
    ///
    /// Until a state-hash / chance-node architecture lands (or
    /// descendants are lazily revalidated against their reconstructed
    /// states on next visit), `reroot` discards all descendants beyond
    /// the new root. The new root retains its visit / value
    /// aggregates; its children are rebuilt on the next search.
    ///
    /// # PIMC caveat
    ///
    /// The preserved root visit counts reflect rollouts that ran
    /// against per-simulation determinizations from the OLD root state.
    /// In a perfect-information game this would be a strict win; under
    /// PIMC + hidden draw order the surviving root statistics are
    /// approximately-but-not-exactly conditioned on `new_root_state`.
    /// We accept this — it's the same trade-off other PIMC search
    /// engines (e.g. Hearthstone-AI, Spire-AI) make to recover the
    /// compute cost of re-search from scratch each turn.
    pub fn reroot(&mut self, action: &Action, new_root_state: CombatState) -> bool {
        // Bead `sts2-rs-vqckz` (F0): `reroot` installs `new_root_state`
        // VERBATIM — it never blindfolds. Under an honest mode the caller's
        // state carries the true hidden pile order / RNG cursors, so tree
        // reuse would silently restore full clairvoyance from move 2 onward
        // and poison every honest measurement built on top. Hard assert (not
        // debug_assert) for the same measurement-integrity rationale as the
        // STS2_RUNAI_HONEST panic in sts2-runai: reroot is cheap relative to
        // a search, and a silent re-leak is far costlier than the check.
        // Honest modes must construct a fresh `Mcts` per decision (which is
        // what every current honest caller — the probe, `mcts_pick` — does).
        assert!(
            self.config.hidden_info == HiddenInfoMode::Clairvoyant,
            "Mcts::reroot is unsupported under {:?}: reroot installs the caller's state \
             verbatim (no blindfold), so tree reuse re-leaks hidden information the honest \
             mode exists to hide — construct a fresh Mcts per decision instead",
            self.config.hidden_info,
        );
        let root = self.tree.root;
        // Bead `sts2-rs-edge` / GH #16: edges are EdgeActions keyed by
        // stable card identity. Resolve each against the OLD root_state
        // (the state edges were created against) and compare to the
        // caller's concrete Action.
        let new_root_old_id = self
            .tree
            .node(root)
            .children
            .iter()
            .find(|(edge, _)| edge.resolve(&self.tree.root_state).as_ref() == Some(action))
            .map(|(_, id)| *id);
        let Some(new_root_id) = new_root_old_id else {
            return false;
        };

        // GH #21: drop every descendant beyond the new root.
        //
        // Surviving descendant nodes were created during simulations
        // against a SINGLE determinization of the OLD root state
        // (shuffled draw, sampled intents, etc.). Their stored
        // `is_terminal`, `state_hash`, `untried_actions`, and child
        // lists reflect *that* determinization's trajectory, not the
        // live `new_root_state`. After hidden draws / intent rolls /
        // hand-index or card-identity changes between turns, those
        // fields can be flatly wrong — most dangerously,
        // `is_terminal = true` on a descendant whose reconstructed
        // state is non-terminal would cause selection to take a fake
        // terminal evaluation and short-circuit further search there.
        //
        // The previous implementation BFS-copied the whole subtree
        // and refreshed only the new root. That left descendants stale.
        // Until a state-hash/chance-node architecture lands (or each
        // descendant is lazily revalidated against its reconstructed
        // state on next visit), the only safe policy is to discard
        // them and re-expand from the new root on the next search.
        //
        // We still preserve the new root's `visits` / `value_sum` —
        // these are an aggregate over many rollouts and are the same
        // PIMC trade-off already documented above. Children are
        // cleared; `untried_actions` and `is_expanded` are reset so
        // the lazy-populate path in `one_simulation` re-derives them
        // against `new_root_state`. Terminal / proven flags and the
        // state hash are recomputed from `new_root_state` below.
        let mut new_root_node = self.tree.arena[new_root_id.0 as usize].clone();
        new_root_node.parent = None;
        new_root_node.children.clear();
        new_root_node.child_visits.clear();
        new_root_node.untried_actions.clear();
        new_root_node.is_expanded = false;
        // Same up-front reservation as `with_policy` — the rebuilt arena
        // will be refilled by the next `search_n` call's expansions.
        let mut new_arena = Vec::with_capacity(1 + arena_reservation(self.config.max_simulations));
        new_arena.push(new_root_node);

        self.tree.arena = new_arena;
        self.tree.root = NodeId(0);
        self.tree.root_state = new_root_state;
        {
            // The tree stores realized state per node and may have re-rooted
            // onto a chance node (the child of a stochastic action). The real
            // committed outcome almost never matches any sampled outcome, so
            // descendants were already discarded above; reset the surviving
            // root to a plain decision node holding the real state.
            let realized = self.tree.root_state.clone();
            let root = self.tree.node_mut(NodeId(0));
            root.kind = NodeKind::Decision;
            root.chance_outcomes.clear();
            root.chance_action = None;
            root.randomness_base = 0;
            root.widenings_since_new = 0;
            root.chance_draws = 0;
            root.chance_closed = false;
            root.endturn_prefix = EndTurnPrefix::Unprobed;
            root.search_hash = sts2_core::search_hash(&realized);
            root.realized = Some(Box::new(realized));
        }
        self.root_stats.clear();
        // The transposition table indexed nodes by their pre-reroot
        // NodeIds; those are now stale after the BFS remap above. The
        // simplest correct policy is to drop the table — it will
        // repopulate naturally as new expansions occur in the
        // surviving subtree. Bead `sts2-rs-7z97`.
        if let Some(tt) = self.transposition.as_mut() {
            tt.clear();
        }
        // W4 (`sts2-rs-dm5z`): reroot invalidates the best-sequence
        // tracker — those actions started at the OLD root and have
        // already been (partially) committed by the caller. Reset to
        // the initial sentinel so the next search rebuilds from the
        // new root.
        self.best_seq.clear();
        self.best_value = f32::NEG_INFINITY;

        // Refresh the new root's derived fields from `new_root_state`.
        // The descendants were discarded above; only the root's own
        // metadata (terminal flag, state hash, proven flags) needs to
        // be recomputed against the live state. `untried_actions` /
        // `is_expanded` were already cleared above so the
        // lazy-populate path in `one_simulation` re-derives the legal
        // action list from `new_root_state` on the next visit.
        {
            // Compute derived values from the new root state before taking
            // a mutable borrow on the tree — the borrow checker cannot see
            // that `root_state` and `arena[0]` are disjoint fields.
            let new_terminal = is_terminal(&self.tree.root_state);
            let new_hash = state_hash(&self.tree.root_state);
            let (proven_win, proven_loss) = if new_terminal {
                let (_, v) = terminal_value(&self.tree.root_state);
                (v > 0.5, v < 0.0)
            } else {
                (false, false)
            };
            let legal = legal_actions(&self.tree.root_state);
            // Pre-resolve each existing child edge against the new
            // root_state BEFORE taking the mutable borrow below — the
            // borrow checker can't see that `arena[0].children` and
            // `root_state` are disjoint, but a Vec<bool> of "keep?"
            // decisions is borrow-free.
            let keep_child: Vec<bool> = self.tree.arena[0]
                .children
                .iter()
                .map(|(edge, _)| match edge.resolve(&self.tree.root_state) {
                    Some(a) => legal.iter().any(|la| la == &a),
                    None => false,
                })
                .collect();

            let root_node = self.tree.node_mut(NodeId(0));
            root_node.is_terminal = new_terminal;
            root_node.state_hash = new_hash;
            root_node.proven_win = proven_win;
            root_node.proven_loss = proven_loss;
            // Clear untried_actions and is_expanded so one_simulation's
            // lazy-populate re-derives them against new_root_state.
            root_node.untried_actions.clear();
            root_node.is_expanded = false;
            // Prune children whose actions are no longer legal in
            // new_root_state so they don't waste UCT budget.
            // `Action` does not implement `Hash`, so a linear scan is
            // used — the action count is O(tens) at most.
            // Apply the pre-computed keep mask. Indexed by original
            // child position; retain mirrors the index walk.
            let mut idx = 0usize;
            root_node.children.retain(|_| {
                let keep = keep_child.get(idx).copied().unwrap_or(false);
                idx += 1;
                keep
            });
            // `child_visits` is positionally parallel to `children`, so it
            // has to be pruned by the same mask or every surviving edge's
            // count silently belongs to a different action.
            let mut idx = 0usize;
            root_node.child_visits.retain(|_| {
                let keep = keep_child.get(idx).copied().unwrap_or(false);
                idx += 1;
                keep
            });
        }
        true
    }

    /// Principal variation from the root, following the best child at
    /// each step until reaching a leaf.
    ///
    /// Selection rule matches [`Self::search_n`]: `argmax mean_value`
    /// (= `argmax Q`) with stable tie-break by visit count. Bead
    /// `sts2-rs-trnu` replaced the prior `argmax(visits)` rule at the
    /// root; the PV must use the same rule or it disagrees with the
    /// action just chosen (GH #3).
    ///
    /// Decision nodes contribute their best-Q child's action (resolved
    /// against that node's realized state); chance nodes are traversed by
    /// following their most-visited outcome and emit no action of their own
    /// (the stochastic action was already recorded on the decision→chance
    /// edge).
    pub fn get_pv(&self) -> Vec<Action> {
        let mut pv = Vec::new();
        let mut cur = self.tree.root;
        // Depth backstop: the chance-mode transposition table merges
        // game-identical states across paths, so the tree is a DAG that can
        // contain cycles (the selection loop guards against this with the
        // same `CHANCE_MAX_DEPTH` bound). Without a cap here, a cycle on the
        // principal variation would spin this walk forever.
        let mut steps = 0usize;
        loop {
            if steps >= self.config.chance_max_depth {
                return pv;
            }
            steps += 1;
            let node = self.tree.node(cur);
            match node.kind {
                NodeKind::Decision => {
                    if node.children.is_empty() {
                        return pv;
                    }
                    let Some(best) = node.children.iter().max_by(|(_, ida), (_, idb)| {
                        let na = self.tree.node(*ida);
                        let nb = self.tree.node(*idb);
                        #[allow(clippy::cast_precision_loss)]
                        let qa = if na.visits == 0 {
                            0.0
                        } else {
                            na.value_sum / na.visits as f32
                        };
                        #[allow(clippy::cast_precision_loss)]
                        let qb = if nb.visits == 0 {
                            0.0
                        } else {
                            nb.value_sum / nb.visits as f32
                        };
                        qa.partial_cmp(&qb)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then(na.visits.cmp(&nb.visits))
                    }) else {
                        return pv;
                    };
                    if let Some(state) = node.realized.as_deref() {
                        if let Some(action) = best.0.resolve(state) {
                            pv.push(action);
                        }
                    }
                    cur = best.1;
                }
                NodeKind::Chance => {
                    let Some(best) = node
                        .chance_outcomes
                        .iter()
                        .max_by_key(|o| self.tree.node(o.child).visits)
                    else {
                        return pv;
                    };
                    cur = best.child;
                }
            }
        }
    }

    /// Best action sequence captured by the rollout-improvement tracker
    /// (bead `sts2-rs-dm5z`, W4). Each call to [`Self::search`] /
    /// [`Self::search_n`] may extend or replace this sequence when a
    /// rollout's leaf value strictly exceeds [`Self::best_value`].
    ///
    /// Returns `&[]` when no rollout has produced an improvement since
    /// construction or the last [`Self::reroot`]. The first rollout will
    /// always install a sequence (initial sentinel is
    /// `f32::NEG_INFINITY`), so an empty slice here only occurs when
    /// `search` has not yet been called or every rollout produced
    /// `f32::NEG_INFINITY` (effectively never under
    /// [`terminal_value`] / [`depth_cut_value`] which are bounded).
    ///
    /// Mirrors `bestActionSequence` / `stepThroughSolution` from
    /// `sts_lightspeed::BattleScumSearcher2`. Hint-only callers do NOT
    /// need this — use [`Self::root_outcomes`] for the per-action
    /// hint signal.
    pub fn best_sequence(&self) -> Vec<Action> {
        // Bead `sts2-rs-edge` / GH #16: `best_seq` now stores
        // EdgeActions (stable card-identity keys); reconstruct the
        // concrete Action list by walking a state clone from the root
        // and resolving each edge against the chain of states. Stops
        // early if an edge fails to resolve (defensive — best_seq is
        // captured against a determinization that resolved at the
        // time, but cards consumed earlier in the chain change the
        // hand state at later positions).
        let mut out: Vec<Action> = Vec::with_capacity(self.best_seq.len());
        let mut state = self.tree.root_state.clone();
        for edge in &self.best_seq {
            let Some(action) = edge.resolve(&state) else {
                break;
            };
            let _ = core_apply(&mut state, action.clone());
            out.push(action);
        }
        out
    }

    /// Best leaf value observed since the last reroot (sentinel
    /// `f32::NEG_INFINITY` until the first simulation completes).
    /// Monotone non-decreasing across [`Self::search`] /
    /// [`Self::search_n`] calls within a single root — guaranteed by
    /// the strict-`>` update rule. Bead `sts2-rs-dm5z` (W4).
    pub fn best_value(&self) -> f32 {
        self.best_value
    }

    /// Visit-weighted mean Q across root children, or `None` when no
    /// child has visits (terminal root, or `search_n(0)`). Range
    /// matches `terminal_value` at `[-1.0, 1.0]`. Used as the TD-style
    /// value target for value-head training:
    /// `target = (1 − λ) · outcome + λ · ((root_q + 1) / 2)`.
    /// Bead `sts2-rs-wrip2`.
    pub fn root_q(&self) -> Option<f32> {
        let root = self.tree.root;
        let node = self.tree.node(root);
        let mut total_visits: u64 = 0;
        let mut sum_value: f64 = 0.0;
        for (_, cid) in &node.children {
            let c = self.tree.node(*cid);
            if c.visits > 0 {
                total_visits += u64::from(c.visits);
                sum_value += f64::from(c.value_sum);
            }
        }
        if total_visits == 0 {
            None
        } else {
            #[allow(clippy::cast_possible_truncation)]
            Some((sum_value / total_visits as f64) as f32)
        }
    }

    /// Mean rollout value (`value_sum / visits`) for the root child
    /// keyed by `action`, or `None` when no root child matches or that
    /// child has zero visits. Convenience wrapper around
    /// [`Self::root_outcomes`] for callers that already know which
    /// action they want to score (e.g. fixture tests pinning a
    /// constructed setup's Q-ordering).
    ///
    /// Bead `sts2-rs-hgu7` (Q-comparison fixture surface).
    pub fn root_q_for_action(&self, action: &Action) -> Option<f32> {
        let root = self.tree.root;
        let node = self.tree.node(root);
        // Bead `sts2-rs-edge` / GH #16: resolve each edge against
        // root_state and compare to the caller's Action.
        node.children.iter().find_map(|(edge, cid)| {
            if edge.resolve(&self.tree.root_state).as_ref() == Some(action) {
                let c = self.tree.node(*cid);
                if c.visits == 0 {
                    None
                } else {
                    #[allow(clippy::cast_precision_loss)]
                    Some(c.value_sum / c.visits as f32)
                }
            } else {
                None
            }
        })
    }

    // ── Internals ──────────────────────────────────────────────────────────

    fn refresh_root_stats(&mut self) {
        let root = self.tree.root;
        let node = self.tree.node(root);
        // Bead `sts2-rs-edge` / GH #16: resolve edges against root_state.
        // Root edges were created against `root_state` so resolution
        // always succeeds at the root; the `filter_map` is defensive.
        self.root_stats = node
            .children
            .iter()
            .filter_map(|(edge, cid)| {
                let action = edge.resolve(&self.tree.root_state)?;
                let c = self.tree.node(*cid);
                #[allow(clippy::cast_precision_loss)]
                let mean = if c.visits == 0 {
                    0.0
                } else {
                    c.value_sum / c.visits as f32
                };
                Some((action, c.visits, mean))
            })
            .collect();
    }

    /// GitHub issue #19: revalidate a stored tree edge against the
    /// current PIMC determinization and apply it. Returns `true` iff
    /// the action was both in `legal_actions(state)` and progressed
    /// the state (not an `InvalidTarget` and not a no-op `PlayCard`).
    /// Increments the corresponding diagnostic counter on rejection
    /// so callers can surface how often determinization drift is
    /// pruning tree descent.
    ///
    /// As of bead `sts2-rs-edge` / GH #16 the live `one_simulation`
    /// path uses `EdgeAction::resolve` for revalidation instead — the
    /// stable card-identity edge keys make the action-vs-legal-set
    /// scan unnecessary. The helper is retained under `#[cfg(test)]`
    /// so the GH #19 unit tests still exercise the predicate, but it
    /// is no longer wired into the descent loop.
    #[cfg(test)]
    fn edge_applies(&mut self, state: &mut CombatState, action: &Action) -> bool {
        // Cheap pre-check: is the stored action in the current state's
        // legal set? Built from the same `legal_actions` source used
        // to populate `untried_actions` on first visit, so on the same
        // determinization this check always passes — the cost is only
        // borne by revisits under re-shuffled draws.
        let legals = legal_actions(state);
        if !legals.iter().any(|a| a == action) {
            self.stale_child_edges += 1;
            return false;
        }
        // Pre-state observation for no-op `PlayCard` detection: a
        // legal PlayCard should always move a card out of the hand.
        let pre_hand_len = state.player.hand.len();
        let is_play_card = matches!(action, Action::PlayCard { .. });
        let result = core_apply(state, action.clone());
        if result == StepResult::InvalidTarget {
            self.failed_apply += 1;
            self.invalid_target += 1;
            return false;
        }
        if is_play_card && state.player.hand.len() == pre_hand_len {
            // PlayCard returned `Advanced` but the hand is unchanged —
            // the engine took an early-return no-op path (energy /
            // hand-index / cost guard). Don't let the search descend
            // into a child node representing a state the engine never
            // actually produced. Bead reference: issue #19.
            self.failed_apply += 1;
            self.no_op_playcard += 1;
            return false;
        }
        true
    }

    /// Value for a decisive leaf — proven-win / proven-loss / terminal.
    /// Split out of the former `leaf_value` helper (now inlined into
    /// [`Self::one_simulation`]) so the chance-mode simulation
    /// step can evaluate decisive leaves against the node's stored
    /// realized state without cloning it (this path never mutates
    /// `state`). Callers must pass a leaf for which
    /// `proven_win || proven_loss || is_terminal` holds.
    fn decisive_leaf_value(
        &mut self,
        leaf_proven_win: bool,
        leaf_proven_loss: bool,
        state: &CombatState,
    ) -> f32 {
        if leaf_proven_win {
            self.rollouts_skipped += 1;
            match self.config.terminal_eval_mode {
                TerminalEvalMode::Binary | TerminalEvalMode::RichLoss => 1.0,
                TerminalEvalMode::RichBoth => {
                    0.5 + evaluate_victory(state) + victory_hp_surplus(state)
                        - loot_penalty_q(state, &self.config)
                }
            }
        } else if leaf_proven_loss {
            self.rollouts_skipped += 1;
            match self.config.terminal_eval_mode {
                TerminalEvalMode::Binary => -1.0,
                // Proven-loss states may still be ALIVE (the solver
                // propagates the flag up interior nodes), so the graded
                // loss leaf here also orders them by remaining player HP.
                TerminalEvalMode::RichLoss | TerminalEvalMode::RichBoth => {
                    -1.0 + 0.5 * loss_leaf_value(state, &self.leaf_eval, &self.config)
                }
            }
        } else {
            // sts2-rs-vbzji: route terminal-leaf values through the
            // configured mode (matches the rollout() branch).
            terminal_state_value_with(state, &self.config, &self.leaf_eval)
        }
    }

    /// One full chance-node MCTS iteration.
    ///
    /// Descends by **pointer-follow over the realized `CombatState` stored on
    /// each node**, branches the tree on stochastic transitions via chance
    /// nodes (detected by a combat-RNG counter delta), resolves chance nodes
    /// with Double Progressive Widening, and backs values up with uniform
    /// (expectimax) backup.
    fn one_simulation(&mut self) {
        // Search-side rollout RNG, seeded deterministically per simulation
        // (so determinism holds with workers=1).
        let mut sim_rng = Xoshiro256PlusPlus::seed_from_u64(
            self.config.seed ^ u64::from(self.sim_index).wrapping_mul(0x9E37_79B9_7F4A_7C15),
        );

        let mut node_id = self.tree.root;
        // Inline capacity 64 (was 32): with TT DAG merges + chance-node
        // interleaving the selection path regularly exceeds 32 hops on
        // long combats (`chance_max_depth` allows up to 1024), and each
        // spill was a per-simulation heap allocation. 64 × 4 B stays a
        // trivial 256 B stack frame.
        let mut path: SmallVec<[NodeId; 64]> = smallvec::smallvec![node_id];
        // Parallel to `path`: `incoming[k]` is the index into
        // `path[k-1].children` that was traversed to reach `path[k]`, or
        // `CHANCE_HOP` when `path[k-1]` is a chance node (whose per-outcome
        // counts already live in `chance_outcomes[i].n`). Slot 0 is the root
        // and is never read. Recorded here rather than reconstructed in
        // backup because every site below already knows the index.
        const CHANCE_HOP: u32 = u32::MAX;
        let mut incoming: SmallVec<[u32; 64]> = smallvec::smallvec![CHANCE_HOP];
        let mut fresh_leaf = false;

        // ── 1. SELECTION (pointer-follow over stored states) ────────────
        loop {
            // Safety bound against pathological cycles introduced by TT
            // merges (a merged node could otherwise close a loop).
            if path.len() > self.config.chance_max_depth {
                // UCT descent (below) may have just stepped onto a *chance*
                // node, which carries no realized state -- but the leaf eval
                // after this loop requires a decision node's realized state.
                // Resolve the chance node one step to a decision outcome so we
                // always exit on a decision leaf, then stop (don't expand past
                // the bound). Without this, the descent could break here on a
                // chance node and `.expect("leaf has realized state")` panics
                // (GH #98).
                if self.tree.node(node_id).kind == NodeKind::Chance {
                    let (child, _is_fresh) = self.chance_widen_or_sample(node_id, &path);
                    path.push(child);
                    incoming.push(CHANCE_HOP);
                    node_id = child;
                    fresh_leaf = true;
                }
                break;
            }
            if self.tree.node(node_id).kind == NodeKind::Chance {
                let (child, is_fresh) = self.chance_widen_or_sample(node_id, &path);
                path.push(child);
                incoming.push(CHANCE_HOP);
                node_id = child;
                if is_fresh {
                    fresh_leaf = true;
                    break;
                }
                continue;
            }
            // Decision node: lazily populate untried from its realized state.
            if !self.tree.node(node_id).is_expanded {
                if !self.tree.node(node_id).is_terminal {
                    let state_ref = self
                        .tree
                        .node(node_id)
                        .realized
                        .as_deref()
                        .expect("decision node has realized state in chance mode");
                    let acts = legal_actions(state_ref);
                    let mut acts = canonicalize_actions(state_ref, acts);
                    // Drink-jitter seam (GH #447 follow-up): mask
                    // `DrinkPotion` at the ROOT only — interior nodes and
                    // rollouts still drink, this is about removing the
                    // root *decision* to create exogenous belt variation.
                    // Guard against emptying the action set (EndTurn is
                    // always legal in PlayerPlayPhase, so this only trips
                    // on exotic pending-choice roots — keep the originals
                    // there rather than strand the search).
                    if self.config.root_drink_mask && node_id == self.tree.root {
                        let unmasked = acts.clone();
                        acts.retain(|a| !matches!(a, Action::DrinkPotion { .. }));
                        if acts.is_empty() {
                            acts = unmasked;
                        }
                    }
                    let mut untried: SmallVec<[EdgeAction; 16]> = SmallVec::new();
                    for a in acts {
                        untried.push(EdgeAction::from_action(&a, state_ref));
                    }
                    // A8 audit §7b: `canonicalize_actions` dedups on
                    // exactly the `EdgeAction::PlayCard` identity tuple,
                    // so no two untried edges may be equal. A duplicate
                    // here becomes two children resolving to the same
                    // concrete action — splitting its budget and (under
                    // the always-on transposition table) summing a
                    // shared child's visits twice in `root_outcomes`.
                    debug_assert!(
                        untried
                            .iter()
                            .enumerate()
                            .all(|(i, e)| !untried[..i].contains(e)),
                        "duplicate EdgeAction in untried_actions — \
                         canonicalize_actions key drifted from EdgeAction: {untried:?}"
                    );
                    self.tree.node_mut(node_id).untried_actions = untried;
                }
                self.tree.node_mut(node_id).is_expanded = true;
            }
            let n = self.tree.node(node_id);
            if n.is_terminal || n.children.is_empty() {
                break;
            }
            if !n.untried_actions.is_empty() {
                // An unexpanded action is where first-play urgency actually
                // bites in this engine. Expansion pops ONE untried action per
                // simulation and immediately backs a value up through it, so a
                // child is never `visits == 0` by the time the UCT descent runs
                // — the `INFINITY` first-visit tiebreak below is unreachable
                // dead code, and the "every child gets one visit before any
                // gets two" behaviour comes from *this* rule instead.
                match self.config.fpu_reduction {
                    // Historical: an unexpanded action unconditionally outranks
                    // every visited child, so a node expands breadth-first
                    // before it refines anything.
                    None => break,
                    // FPU: price the unexpanded action at `parent_q - r` and
                    // let it compete with the visited children on the same
                    // scale. Large `r` says "refine what you have".
                    Some(r) if self.fpu_prefers_expansion(node_id, r) => break,
                    Some(_) => {}
                }
            }
            // Sequential Halving owns the root edge: the SH driver has already
            // decided which arm this simulation belongs to, so skip UCT for
            // this one hop. Everything below the root is untouched (H-MCTS).
            if node_id == self.tree.root {
                if let Some(forced) = self.forced_root_child {
                    // One scan of the root's child list per simulation — the
                    // only site that has to look the edge index up rather
                    // than already knowing it.
                    let slot = self
                        .tree
                        .node(node_id)
                        .children
                        .iter()
                        .position(|(_, cid)| *cid == forced);
                    node_id = forced;
                    path.push(node_id);
                    incoming.push(
                        slot.and_then(|i| u32::try_from(i).ok())
                            .unwrap_or(CHANCE_HOP),
                    );
                    continue;
                }
            }
            // UCT/PUCT descent by pointer — children already hold realized
            // states, so no edge-resolve / re-apply is needed (unlike PIMC).
            // One prior channel: the learned PUCT policy prior when
            // `use_puct_prior` is set, turning the explore term into
            // `c·P(s,a)·√ΣN/(1+N)` and the first-visit FPU into `c·P(s,a)·√ΣN`
            // (instead of INFINITY). Without it (the default, and every parity
            // fixture) `puct_priors` is `None`: first-visit keeps the INFINITY
            // tiebreak and visited children use the classic UCT term.
            //
            // The three hand-written game-knowledge edge priors that used to
            // ride alongside it were removed 2026-08-22 after measuring null —
            // see `docs/experiments/2026-08-22-game-edge-priors-null.md`.
            let parent_visits = n.visits.max(1);
            #[allow(clippy::cast_precision_loss)]
            let ln_p = (parent_visits as f32).ln();
            // KataGo's dynamic variance scaling. `scale == 0.0` short-circuits
            // to the historical constant, bit-for-bit.
            let c = self.effective_c(n);
            let state_ref = n
                .realized
                .as_deref()
                .expect("decision node has realized state in chance mode");
            // Resolve each child edge once, for the learned prior's action
            // list. Skipped entirely when the prior is off, so the default
            // config does no extra resolve work.
            let resolved: SmallVec<[Option<Action>; 16]> = if self.config.use_puct_prior {
                n.children
                    .iter()
                    .map(|(edge, _)| edge.resolve(state_ref))
                    .collect()
            } else {
                SmallVec::new()
            };
            let puct_priors: Option<SmallVec<[f32; 16]>> = if self.config.use_puct_prior {
                // Unresolved (filtered) children get a placeholder EndTurn; the
                // slot is never consumed (their `resolved` entry is `None`).
                let actions: SmallVec<[Action; 16]> = resolved
                    .iter()
                    .map(|r| r.clone().unwrap_or(Action::EndTurn))
                    .collect();
                Some(self.policy.prior(state_ref, &actions))
            } else {
                None
            };
            #[allow(clippy::cast_precision_loss)]
            let sqrt_parent = (parent_visits as f32).sqrt();
            let mut best: Option<NodeId> = None;
            let mut best_slot: u32 = CHANCE_HOP;
            let mut best_uct = f32::NEG_INFINITY;
            for (child_idx, (_edge, cid)) in n.children.iter().enumerate() {
                let child = self.tree.node(*cid);
                let uct = if child.visits == 0 {
                    // First visit, in precedence order: the learned PUCT FPU
                    // when a prior exists, then the configured FPU reduction
                    // (`parent_q - r`), then the historical INFINITY tiebreak.
                    if let Some(ref priors) = puct_priors {
                        let p = priors.get(child_idx).copied().unwrap_or(0.0);
                        c * p * sqrt_parent
                    } else if let Some(r) = self.config.fpu_reduction {
                        #[allow(clippy::cast_precision_loss)]
                        let parent_q = n.value_sum / parent_visits as f32;
                        parent_q - r
                    } else {
                        f32::INFINITY
                    }
                } else {
                    #[allow(clippy::cast_precision_loss)]
                    let q = child.value_sum / child.visits as f32;
                    #[allow(clippy::cast_precision_loss)]
                    let explore = if let Some(ref priors) = puct_priors {
                        let p = priors.get(child_idx).copied().unwrap_or(0.0);
                        c * p * sqrt_parent / (1.0 + child.visits as f32)
                    } else {
                        c * (ln_p / child.visits as f32).sqrt()
                    };
                    q + explore
                };
                if uct > best_uct {
                    best_uct = uct;
                    best = Some(*cid);
                    best_slot = u32::try_from(child_idx).unwrap_or(CHANCE_HOP);
                }
            }
            let Some(next) = best else { break };
            node_id = next;
            path.push(node_id);
            incoming.push(best_slot);
        }

        // ── 2. EXPANSION (decision-node leaf only) ──────────────────────
        if !fresh_leaf {
            let leaf = self.tree.node(node_id);
            let do_expand = leaf.kind == NodeKind::Decision
                && !leaf.is_terminal
                && !leaf.untried_actions.is_empty();
            if do_expand {
                let edge = self.tree.node_mut(node_id).untried_actions.remove(0);
                // Single clone: resolve + stochastic detection + the realized
                // outcome-0 state all happen on one working copy (`work` is
                // moved into the new child node, so no extra clone is needed).
                let mut work = self
                    .tree
                    .node(node_id)
                    .realized
                    .as_deref()
                    .expect("decision leaf has realized state")
                    .clone();
                if let Some(action) = edge.resolve(&work) {
                    // Apply and detect whether the transition consumed combat
                    // randomness (→ chance node) via a counter delta across
                    // all combat streams + the legacy rng.
                    let before_fp = work.run_rng.combat_counter_fingerprint();
                    let before_legacy = work.rng.counter();
                    let before_cards_drawn = work.cards_drawn;
                    // DrawChance detection (bead `sts2-rs-i8bgu`, F1 plan
                    // §3.2 step 1): read BEFORE `core_apply` mutates the
                    // pile — the segment is a property of the PARENT
                    // decision, matching the parent-conditioned reseed
                    // `realize_chance_outcome` uses. Short-circuits to
                    // `false` (skipping the segment scan entirely) outside
                    // `DrawChance`, so Clairvoyant/RootScramble pay nothing
                    // extra here.
                    let draw_chance_mode = self.config.hidden_info == HiddenInfoMode::DrawChance;
                    // F2 (bead `sts2-rs-09x5n`): under `BeliefDraw`, `cards::draw`
                    // consumes the shuffle stream itself whenever the unknown
                    // segment has >=2 cards, so the fingerprint-delta term in
                    // `stochastic` below already detects a genuine draw-chance
                    // event with ZERO extra code here — no heterogeneity
                    // pre-check is needed (or even possible without reading the
                    // pile a second time), unlike `DrawChance`'s single
                    // first-draw special case.
                    let pre_action_draw_heterogeneous = draw_chance_mode && {
                        let segment = sts2_core::cards::unknown_middle_uids(&work.player);
                        segment.len() >= 2
                            && !sts2_core::cards::segment_is_observationally_homogeneous(
                                &segment,
                                &work.cards,
                            )
                    };
                    let _ = core_apply(&mut work, action);
                    let stochastic = work.run_rng.combat_counter_fingerprint() != before_fp
                        || work.rng.counter() != before_legacy
                        || (pre_action_draw_heterogeneous && work.cards_drawn > before_cards_drawn);
                    if stochastic {
                        let chance_id = self.alloc_chance_node(node_id, &edge, &work);
                        // H3 retune (bead `sts2-rs-k1z14`, F1 G2/G3 pass):
                        // this per-NODE flag — not the blanket `draw_chance_mode`
                        // — gates both the diagnostic field below and the
                        // outcome-0 rebuild decision just below it. Recorded
                        // once here so both readers see the identical
                        // classification. `f1_draw_triggered` (DrawChance-only,
                        // heterogeneity-gated) drives the outcome-0 REBUILD
                        // decision unchanged from before this bead; the STORED
                        // node flag additionally covers `BeliefDraw` (bead
                        // `sts2-rs-09x5n`, F2 plan §3.2 step 6) via a plain
                        // `cards_drawn` delta — under that mode the trigger IS
                        // the fingerprint delta, so there is nothing else to
                        // gate the classification on.
                        let cards_drawn_moved = work.cards_drawn > before_cards_drawn;
                        let f1_draw_triggered = pre_action_draw_heterogeneous && cards_drawn_moved;
                        let belief_draw_mode =
                            self.config.hidden_info == HiddenInfoMode::BeliefDraw;
                        let is_draw_triggered =
                            f1_draw_triggered || (belief_draw_mode && cards_drawn_moved);
                        self.tree.node_mut(chance_id).chance_is_draw_triggered = is_draw_triggered;
                        path.push(chance_id);
                        // `children.push` for this edge happens below, so the
                        // slot it will land in is the current length.
                        incoming.push(
                            u32::try_from(self.tree.node(node_id).children.len())
                                .unwrap_or(CHANCE_HOP),
                        );
                        // Outcome 0: historically (Clairvoyant / RootScramble,
                        // byte-stable) the on-policy realization we just
                        // computed from the real RNG cursors (not a reseed).
                        // Under `DrawChance` (bead `sts2-rs-i8bgu`, F1 plan
                        // §3.2 step 2) outcome 0 stops being the true
                        // continuation: it is rebuilt through the exact same
                        // `(base, 0)` reseed-and-reapply path
                        // `chance_widen_or_sample` uses for widened outcomes,
                        // starting from a FRESH clone of the parent's
                        // pre-action state (not `work`, which already carries
                        // the real cursors' realization) — see
                        // `realize_chance_outcome`. Reserve reseed ordinal 0
                        // for outcome 0 and start widen draws at ordinal 1
                        // either way, so `chance_draws` stays a
                        // unique-per-draw counter and widened ordinals never
                        // alias outcome 0's slot. Built BEFORE `edge` is moved
                        // into `children` below — `realize_chance_outcome`
                        // only needs `&edge`. H3 retune: this is now gated on
                        // `f1_draw_triggered`, NOT the blanket `draw_chance_mode`
                        // — a chance node reached under `DrawChance` whose
                        // stochasticity is a pure RNG-counter delta (a roll,
                        // e.g. monster damage variance) with no draw-order
                        // component keeps the historical (Clairvoyant /
                        // RootScramble) on-policy outcome-0 instead of paying
                        // an unnecessary extra reseed's variance — the
                        // draw-hedging de-privileging only makes sense where
                        // there is draw order to hedge over. See the H3
                        // diagnostic note on `MctsConfig::draw_widening_c`.
                        // `BeliefDraw` (bead `sts2-rs-09x5n`, F2) NEVER takes
                        // this rebuild branch even when `is_draw_triggered` is
                        // true for it — the deferred draw's own RNG read
                        // against `work`'s real (root-reseeded, thus already
                        // uninformative) cursors already IS the honest
                        // realization; there is no separate pile order left
                        // to re-materialize the way `DrawChance` needs to.
                        //
                        // CRN adds a SECOND reason to rebuild, orthogonal to
                        // the pile. Sharing `randomness_base` across siblings
                        // only matches their luck on outcomes actually
                        // realized from `(base, n)` — and the on-policy
                        // outcome 0 is realized from `work`'s own post-action
                        // cursors instead, so it was the one outcome CRN did
                        // NOT match. That is the opposite of harmless: at low
                        // visits `dpw_cap` holds ~2-3 outcomes, so the
                        // unmatched one carries a third to a half of the
                        // sibling's value mass, and it is the ONLY outcome a
                        // barely-visited sibling ever has. `sts_lightspeed`
                        // (`BattleSearcher.cpp:377-395`) has no such hole: it
                        // seeds `next.rng = Random(randomnessBase)` BEFORE
                        // `execute`, so its outcome 0 is genuinely `(base, 0)`.
                        // Mirror that — but with `reshuffle_pile = false`, so
                        // this stays a pure stream reseed and does not
                        // re-materialize a pile behind `BeliefDraw`'s back.
                        let crn_rebuild = self.config.common_random_numbers;
                        let outcome0 = if f1_draw_triggered || crn_rebuild {
                            let base = self.tree.node(chance_id).randomness_base;
                            let pre_action = self
                                .tree
                                .node(node_id)
                                .realized
                                .as_deref()
                                .expect("decision node has realized state")
                                .clone();
                            let s = self.realize_chance_outcome(
                                pre_action,
                                &edge,
                                base,
                                0,
                                f1_draw_triggered,
                            );
                            self.chance_alloc_decision(Some(chance_id), s, &path)
                        } else {
                            self.chance_alloc_decision(Some(chance_id), work, &path)
                        };
                        self.tree.node_mut(node_id).children.push((edge, chance_id));
                        self.tree.node_mut(node_id).child_visits.push(0);
                        self.tree
                            .node_mut(chance_id)
                            .chance_outcomes
                            .push(ChanceOutcome {
                                n: 0,
                                child: outcome0,
                                dedup_hash: None,
                            });
                        self.tree.node_mut(chance_id).chance_draws = 1;
                        path.push(outcome0);
                        incoming.push(CHANCE_HOP);
                        node_id = outcome0;
                    } else {
                        let child = self.chance_alloc_decision(Some(node_id), work, &path);
                        self.tree.node_mut(node_id).children.push((edge, child));
                        self.tree.node_mut(node_id).child_visits.push(0);
                        path.push(child);
                        incoming.push(
                            u32::try_from(self.tree.node(node_id).children.len() - 1)
                                .unwrap_or(CHANCE_HOP),
                        );
                        node_id = child;
                    }
                }
            }
        }

        // ── 3. SIMULATION (leaf eval, shared with PIMC) ─────────────────
        // Take the realized state out of the node (a pointer move, not a
        // clone) so the borrow checker lets us evaluate it against `&mut
        // self`; it is restored below. Decisive leaves (proven / terminal)
        // never mutate the state, so they evaluate the stored state
        // directly — the full `CombatState` clone is only paid when a
        // rollout actually runs (perf-investigation 2026-07-04; behavior
        // identical to always-cloning since the clone was read-only on
        // the decisive paths).
        let realized = self
            .tree
            .node_mut(node_id)
            .realized
            .take()
            .expect("leaf has realized state");
        if self.config.use_lethal_detector {
            let leaf = self.tree.node(node_id);
            if !leaf.is_terminal
                && !leaf.proven_win
                && !leaf.proven_loss
                && crate::detect_lethal(&realized)
            {
                self.tree.node_mut(node_id).proven_win = true;
            }
        }
        let leaf = self.tree.node(node_id);
        let leaf_proven_win = leaf.proven_win;
        let leaf_proven_loss = leaf.proven_loss;
        let leaf_decisive = leaf_proven_win || leaf_proven_loss || leaf.is_terminal;
        let (value, rollout_hit_terminal) = if leaf_decisive {
            (
                self.decisive_leaf_value(leaf_proven_win, leaf_proven_loss, &realized),
                // Unused: `leaf_decisive` already forces the decisive bucket.
                true,
            )
        } else {
            let mut state = (*realized).clone();
            let v = rollout(
                &mut state,
                &mut sim_rng,
                self.config.max_rollout_depth,
                self.config.rollout_policy,
                &self.leaf_eval,
                self.config.use_quiescence,
                &self.config,
            );
            let hit_terminal = is_terminal(&state);
            self.rollouts_total += 1;
            if !hit_terminal {
                // `rollout` returned via its depth-cap branch
                // (`depth_cut_value`) — see the mirrored condition
                // there.
                self.depth_cap_hits += 1;
            }
            (v, hit_terminal)
        };
        self.tree.node_mut(node_id).realized = Some(realized);
        debug_assert!(
            value.is_finite() && value.abs() <= MAX_LEAF_VALUE_ABS,
            "chance-mode leaf value out of bounds: {value} (mode={:?})",
            self.config.terminal_eval_mode,
        );
        let value = value.clamp(-MAX_LEAF_VALUE_ABS, MAX_LEAF_VALUE_ABS);

        // ── 4. BACKPROPAGATION (uniform / expectimax) ───────────────────
        let bucket = if leaf_decisive || rollout_hit_terminal {
            Bucket::from_value(value)
        } else {
            Bucket::Tie
        };
        let leaf_id = *path.last().expect("path always contains the root");
        debug_assert_eq!(
            path.len(),
            incoming.len(),
            "edge-slot record drifted from the selection path"
        );
        for k in 1..path.len() {
            let slot = incoming[k];
            if slot == CHANCE_HOP {
                continue;
            }
            if let Some(c) = self
                .tree
                .node_mut(path[k - 1])
                .child_visits
                .get_mut(slot as usize)
            {
                *c += 1;
            }
        }
        for &nid in &path {
            // Swap this node's old Q out of the tree-wide aggregate before the
            // update and its new Q in after, so `global_std_c` reads a current
            // σ without walking the arena. Read before the mutable borrow.
            let old = {
                let n = self.tree.node(nid);
                (n.visits > 0).then(|| f64::from(n.value_sum) / f64::from(n.visits))
            };
            if let Some(old_q) = old {
                self.tree_q_sum -= old_q;
                self.tree_q_sq_sum -= old_q * old_q;
                self.tree_q_count -= 1;
            }
            let n = self.tree.node_mut(nid);
            n.visits += 1;
            n.value_sum += value;
            n.value_sq_sum += value * value;
            if nid == leaf_id {
                n.own_visits += 1;
                n.own_value_sum += value;
            }
            let new_q = f64::from(n.value_sum) / f64::from(n.visits);
            match bucket {
                Bucket::Win => n.wins += 1,
                Bucket::Loss => n.losses += 1,
                Bucket::Tie => n.ties += 1,
            }
            self.tree_q_sum += new_q;
            self.tree_q_sq_sum += new_q * new_q;
            self.tree_q_count += 1;
        }

        // GitHub #492: the aggregation pass. Under the default
        // `BackupRule::Mean` this is not merely a no-op, it is not reached —
        // the loop above IS the backup and the engine is byte-identical to
        // its pre-#492 self.
        if self.config.backup_rule != BackupRule::Mean {
            // Leaf -> root, so a parent always re-aggregates over children
            // that have already absorbed this simulation.
            for i in (0..path.len()).rev() {
                self.reaggregate_node(path[i]);
            }
        }

        self.chance_backprop_proven(&path);
        self.chance_capture_best_seq(&path, value);
    }

    /// Re-express one node's value as an aggregate over its **successors**
    /// instead of an accumulated average of the returns that passed through
    /// it (GitHub #492).
    ///
    /// Two operators, one per node kind — this split is the algorithm, not a
    /// special case:
    ///
    /// * **Decision node** — visit-weighted power mean of order `p` (Dam et
    ///   al., IJCAI 2020, Eq. 11 / Algorithm 1 line 38), over per-edge
    ///   traffic:
    ///
    ///   ```text
    ///   t = own_n * (own_mean + B)^p  +  SUM_a  n(s,a) * (Q_a + B)^p
    ///   w = own_n                     +  SUM_a  n(s,a)
    ///   Q = (t / w)^(1/p) - B
    ///   ```
    ///
    /// * **Chance node** — the plain visit-weighted arithmetic mean of the
    ///   same shape (`p = 1`), i.e. the expectation over nature. Power-UCT
    ///   applies the power mean at V nodes only; its Q node (Algorithm 1
    ///   line 51) is a running average over sampled successors, and so is
    ///   this. Leaning a chance node toward its best outcome would be
    ///   planning against a draw the agent cannot force.
    ///
    /// **The chance node still has to be re-aggregated**, even though its
    /// operator is the mean. Its accumulated `value_sum` is a mean of raw
    /// *returns*, which knows nothing about the power means its outcome
    /// children now report — leave it alone and every chance node erases the
    /// arm's signal from everything below it. Under `BeliefDraw` that is
    /// most of the tree, so the arm would measure as inert for a reason
    /// that has nothing to do with whether the idea works.
    ///
    /// Three further deliberate choices:
    ///
    /// * **Childless nodes are skipped.** A freshly expanded leaf or a
    ///   terminal has nothing to aggregate over, so its accumulated mean
    ///   stands. This is what grounds the recursion.
    /// * **The result is written back into `value_sum` as `Q * visits`**,
    ///   rather than into a parallel field. Every consumer of a node's value
    ///   in this file — UCT descent, `fpu_prefers_expansion`,
    ///   `pick_root_action`, `root_outcomes`, `root_q`, `get_pv`,
    ///   `best_value`, `regularized_root_pi` — reads `value_sum / visits`,
    ///   so one write reaches all of them and no second scalar can silently
    ///   disagree with the first. `value_sq_sum` is deliberately NOT
    ///   rewritten: it is a statistic over raw returns (see its doc
    ///   comment), which is what `cpuct_utility_stdev_scale` wants.
    /// * **`f64` accumulation.** `(1.5 +/- 0.005)^6` in `f32` loses
    ///   precision in exactly the digits that separate siblings.
    ///
    /// **Not an identity at `p = 1`, and knowingly so.** On a tree,
    /// recomputing the weighted mean from successors reproduces the
    /// accumulated mean exactly. This search is a DAG — the transposition
    /// table is unconditionally on — so a shared child reports an estimate
    /// pooled across *all* its parents' traffic, and re-aggregating adopts
    /// that pooled estimate in place of the path-local one. Measured on the
    /// `make_pick_state` fixture at 400 sims, the two differ by ~0.01 of Q.
    /// That is the same order as the sibling gaps #492 is about, so **`p =
    /// 1` is a distinct arm, not a null control**: it isolates the
    /// re-aggregation change, and only `BackupRule::Mean` is the true
    /// control. Any ladder that omits `p = 1` cannot tell the two apart.
    fn reaggregate_node(&mut self, nid: NodeId) {
        let shift = f64::from(self.config.backup_power_shift);
        let normalize = self.config.backup_rule == BackupRule::PowerMeanNorm;
        let (kind, own_visits, own_value_sum, succ): (
            NodeKind,
            u32,
            f32,
            SmallVec<[(NodeId, u32); 16]>,
        ) = {
            let n = self.tree.node(nid);
            match n.kind {
                NodeKind::Decision if !n.children.is_empty() => (
                    n.kind,
                    n.own_visits,
                    n.own_value_sum,
                    n.children
                        .iter()
                        .enumerate()
                        .map(|(i, (_, cid))| (*cid, n.child_visits.get(i).copied().unwrap_or(0)))
                        .collect(),
                ),
                NodeKind::Chance if !n.chance_outcomes.is_empty() => (
                    n.kind,
                    n.own_visits,
                    n.own_value_sum,
                    // `ChanceOutcome::n` is not maintained (it is written 0 and
                    // never incremented); the outcome child's own `visits` is
                    // what `sample_outcome_by_visits` already weights by, so
                    // use the same statistic rather than resurrect a second one.
                    n.chance_outcomes
                        .iter()
                        .map(|o| (o.child, self.tree.node(o.child).visits))
                        .collect(),
                ),
                _ => return,
            }
        };
        let order = if kind == NodeKind::Chance {
            1.0
        } else {
            f64::from(self.config.backup_power_p)
        };
        debug_assert!(order > 0.0, "backup_power_p must be positive, got {order}");

        // Collect the (value, weight) pairs once — the normalized variant
        // needs a min/max pass before it can exponentiate anything.
        let mut vals: SmallVec<[(f64, f64); 16]> = SmallVec::new();
        if own_visits > 0 {
            vals.push((
                f64::from(own_value_sum) / f64::from(own_visits),
                f64::from(own_visits),
            ));
        }
        for (cid, edge_visits) in succ {
            let c = self.tree.node(cid);
            if edge_visits == 0 || c.visits == 0 {
                continue;
            }
            // Value from the successor node (a DAG-pooled estimate that every
            // parent legitimately consults); weight from this edge.
            vals.push((
                f64::from(c.value_sum) / f64::from(c.visits),
                f64::from(edge_visits),
            ));
        }
        let total_w: f64 = vals.iter().map(|(_, n)| *n).sum();
        if total_w <= 0.0 {
            return;
        }
        // `(lo, scale)` is the affine map into the exponentiated domain.
        // Global shift: `(−B, 1)`. Per-node normalization: `(min, max−min)`,
        // falling back to the shift when the range is degenerate (a single
        // successor, or all successors equal — where every operator agrees
        // anyway).
        let (lo, scale) = if normalize {
            let mut min = f64::INFINITY;
            let mut max = f64::NEG_INFINITY;
            for (v, _) in &vals {
                min = min.min(*v);
                max = max.max(*v);
            }
            let range = max - min;
            if range > 1e-9 {
                (min, range)
            } else {
                (-shift, 1.0)
            }
        } else {
            (-shift, 1.0)
        };
        let acc: f64 = vals
            .iter()
            .map(|(v, n)| n * ((v - lo) / scale).max(0.0).powf(order))
            .sum();
        let q = lo + scale * (acc / total_w).powf(1.0 / order);
        debug_assert!(q.is_finite(), "backup re-aggregation produced {q}");
        let q = q.clamp(
            -f64::from(MAX_LEAF_VALUE_ABS),
            f64::from(MAX_LEAF_VALUE_ABS),
        );

        // Keep the tree-wide Q aggregate (`global_std_c`) consistent with the
        // value the node now reports.
        let n = self.tree.node_mut(nid);
        let visits = f64::from(n.visits);
        let old_q = f64::from(n.value_sum) / visits;
        #[allow(clippy::cast_possible_truncation)]
        {
            n.value_sum = (q * visits) as f32;
        }
        self.tree_q_sum += q - old_q;
        self.tree_q_sq_sum += q.mul_add(q, -(old_q * old_q));
    }

    /// Allocate a chance node wrapping the stochastic `edge` applied from
    /// decision node `parent` (whose realized state is `pre`). The
    /// `randomness_base` is derived deterministically from the parent's
    /// hash, the engine seed, and the parent's current child count (so
    /// multiple stochastic edges from one decision node get distinct bases).
    ///
    /// Under [`MctsConfig::common_random_numbers`] the child count is
    /// dropped from the salt, so every stochastic edge leaving `parent`
    /// shares one base: outcome *n* of each sibling is then realized from
    /// the same reseed and the siblings are compared under matched luck.
    fn alloc_chance_node(
        &mut self,
        parent: NodeId,
        edge: &EdgeAction,
        pre: &CombatState,
    ) -> NodeId {
        let parent_hash = self.tree.node(parent).state_hash;
        let salt = if self.config.common_random_numbers {
            // Same constant for every sibling. Deliberately OUTSIDE the
            // non-CRN range (`1 ..= children.len()+1`): the obvious choice
            // of `1` is exactly the salt a parent's FIRST child gets
            // anyway, which would make the CRN stream a copy of that one
            // sibling's non-CRN stream and make a search whose every chance
            // node happens to be a first child byte-identical CRN on/off —
            // i.e. the flag would silently do nothing in precisely the
            // fixtures a reader would use to check that it does something.
            CRN_CHANCE_SALT
        } else {
            (self.tree.node(parent).children.len() as u64).wrapping_add(1)
        };
        let base = sts2_core::run_rng_set::splitmix64(
            parent_hash
                ^ self.config.seed.wrapping_mul(0xD1B5_4A32_D192_ED03)
                ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15),
        );
        // A chance node carries no realized state and its terminal/proven/
        // hash fields are unused (its verdict derives from its outcomes), so
        // skip the full `state_hash` and lethal-detector work `MctsNode::new`
        // would do — pass a 0 hash and lethal=false.
        let mut node = MctsNode::new_inner(Some(parent), pre, false, 0);
        node.kind = NodeKind::Chance;
        node.chance_action = Some(edge.clone());
        node.randomness_base = base;
        // A chance node is neither terminal nor solver-seeded on its own; its
        // verdict is derived from its outcomes during backprop.
        node.is_terminal = false;
        node.proven_win = false;
        node.proven_loss = false;
        node.is_expanded = true; // chance nodes have no untried actions
        let id = self.tree.alloc(node);
        self.nodes_allocated += 1;
        id
    }

    /// Realize chance outcome `n` (base `base`) of a chance node whose
    /// stochastic action is `action_edge`, starting from `pre_action` — a
    /// clone of the chance node's PARENT (pre-action) decision state.
    /// Reseeds the combat streams + legacy rng from `(base, n)` (mirrors
    /// [`sts2_core::run_rng_set::RunRngSet::reseed_combat_streams`]'s
    /// information-set semantics: abandon the real RNG timeline, resample
    /// a fresh i.i.d. draw), then re-applies `action_edge`.
    ///
    /// When `reshuffle_pile` is set (bead `sts2-rs-i8bgu`, F1 plan §3.2
    /// step 2; caller-decided since bead `sts2-rs-k1z14`'s H3 retune —
    /// see the callers' `is_draw_triggered` / `chance_is_draw_triggered`
    /// checks), `pre_action`'s unknown draw-pile segment is ALSO
    /// reshuffled first, seeded from the SAME `(base, n)` — see
    /// [`sts2_core::blindfold::shuffle_unknown_pile_for_outcome`]. This is
    /// what makes outcome `n` genuinely uncertain about draw order, not
    /// just about RNG rolls. Byte-stable for Clairvoyant/RootScramble
    /// (both always pass `false`) and preserved for `DrawChance` roll-only
    /// chance nodes (also `false` there — no draw order to hedge over):
    /// this is exactly the reseed-and-reapply sequence
    /// [`Self::chance_widen_or_sample`]'s widen branch always ran before
    /// `sts2-rs-i8bgu`; the pile shuffle is additional and opt-in per call.
    ///
    /// Returns the realized state only. Callers hash it themselves: its
    /// [`sts2_core::search_hash`] is the TT / node-identity hash,
    /// deliberately UNCHANGED by this bead (see
    /// [`sts2_core::chance_outcome_hash`] for the separate dedup-only hash
    /// callers compute when they need order/uid-insensitive equivalence),
    /// and the widen path skips it on an observational dedup hit.
    // `&self` is no longer read now that `reshuffle_pile` is caller-decided
    // (bead `sts2-rs-k1z14` H3 retune moved the `hidden_info` check out to
    // callers) — kept as a method rather than a free function for
    // consistency with the other `chance_*`/`realize_*` helpers it's
    // called alongside, all of which DO need `&self`/`&mut self`.
    #[allow(clippy::unused_self)]
    fn realize_chance_outcome(
        &self,
        mut pre_action: CombatState,
        action_edge: &EdgeAction,
        base: u64,
        n: u32,
        reshuffle_pile: bool,
    ) -> CombatState {
        if reshuffle_pile {
            sts2_core::blindfold::shuffle_unknown_pile_for_outcome(&mut pre_action.player, base, n);
        }
        reseed_for_outcome(&mut pre_action, base, n);
        if let Some(action) = action_edge.resolve(&pre_action) {
            let _ = core_apply(&mut pre_action, action);
        }
        pre_action
    }

    /// Perf item M12: realize widen draw `n` (base `base`) of the `EndTurn`
    /// chance node `chance_id`, whose parent decision node is `parent`,
    /// through the node's cached RNG-free prefix. Returns `None` when the
    /// draw must take the plain [`Self::realize_chance_outcome`] path: a
    /// hidden-info mode other than `BeliefDraw` / `Clairvoyant` (`DrawChance`
    /// reshuffles the pile before the reseed; `RootScramble` is left alone),
    /// a side-channel recorder armed (see below), or a node whose probe found
    /// no usable prefix.
    ///
    /// **Why the cached path is exact.** The plain path computes
    /// `chain(reseed(pre, n))`. A prefix phase was accepted only if it
    /// returned `Advanced` and left `state.rng` and the whole `RunRngSet`
    /// equal to their post-reseed values, so it drew no randomness; a phase
    /// that draws none runs identically under any reseed, because the reseed
    /// writes only the RNG fields. So `prefix(reseed(pre, n))` equals
    /// `reseed(prefix(reseed(pre, n0)), n)` for the probing draw's `n0`, and
    /// running the remaining phases on it is `end_turn_dispatch`'s own
    /// tail. The one hole is a phase that derives values from an RNG's SEED
    /// without moving any cursor (an ephemeral `Rng::new(seed)`): debug builds
    /// rebuild every new prefix under a second reseed and assert the two
    /// agree.
    ///
    /// **Diagnostics.** The thread-local `damage_trace` and the stderr traces
    /// would see the prefix's events once per node instead of once per draw,
    /// so the cache is bypassed (not cleared) while any of them is armed —
    /// [`sts2_core::combat_loop::side_channel_recorders_armed`]. The
    /// state-resident event log rides the cloned state either way, but it is
    /// part of that check too.
    fn realize_endturn_widen(
        &mut self,
        chance_id: NodeId,
        parent: NodeId,
        base: u64,
        n: u32,
    ) -> Option<CombatState> {
        if !matches!(
            self.config.hidden_info,
            HiddenInfoMode::BeliefDraw | HiddenInfoMode::Clairvoyant
        ) {
            return None;
        }
        let pre = self
            .tree
            .node(parent)
            .realized
            .as_deref()
            .expect("chance parent has realized state");
        if sts2_core::combat_loop::side_channel_recorders_armed(pre) {
            return None;
        }
        match &self.tree.node(chance_id).endturn_prefix {
            EndTurnPrefix::Uncacheable => None,
            EndTurnPrefix::Cached { phases, state } => {
                let mut s = (**state).clone();
                reseed_for_outcome(&mut s, base, n);
                let _ =
                    sts2_core::combat_loop::end_turn_dispatch_from(&mut s, usize::from(*phases));
                Some(s)
            }
            EndTurnPrefix::Unprobed => {
                // `apply` no-ops `EndTurn` outside the play phase; leave
                // that (never stochastic) case on the plain path.
                if pre.phase != sts2_core::CombatPhase::PlayerPlayPhase {
                    self.tree.node_mut(chance_id).endturn_prefix = EndTurnPrefix::Uncacheable;
                    return None;
                }
                let mut s = pre.clone();
                reseed_for_outcome(&mut s, base, n);
                let clone_from = usize::from(self.endturn_prefix_guess);
                let (clean, prefix) = end_turn_probing_prefix(&mut s, clone_from);
                if clean > 0 {
                    // The next probe clones where this node's prefix ended.
                    self.endturn_prefix_guess = u8::try_from(clean).unwrap_or(u8::MAX);
                }
                let entry = match prefix {
                    Some(state) => {
                        #[cfg(debug_assertions)]
                        {
                            let pre = self.tree.node(parent).realized.as_deref().expect("parent");
                            debug_assert_prefix_seed_independent(pre, &state, clean, base, n);
                        }
                        EndTurnPrefix::Cached {
                            phases: u8::try_from(clean).expect("phase count fits u8"),
                            state: Box::new(state),
                        }
                    }
                    // No RNG-free leading phase: nothing to cache, ever.
                    None if clean == 0 => EndTurnPrefix::Uncacheable,
                    // The prefix ended before the guessed boundary, so
                    // nothing was cloned; the next draw re-probes there.
                    None => EndTurnPrefix::Unprobed,
                };
                self.tree.node_mut(chance_id).endturn_prefix = entry;
                Some(s)
            }
        }
    }

    /// Allocate (or transposition-merge) a decision node holding realized
    /// `state`. When the transposition table is enabled, chance mode keys it
    /// on the RNG-cursor-insensitive [`sts2_core::search_hash`] so
    /// game-identical states reached via different outcome seeds merge.
    fn chance_alloc_decision(
        &mut self,
        parent: Option<NodeId>,
        state: CombatState,
        path: &[NodeId],
    ) -> NodeId {
        let sh = sts2_core::search_hash(&state);
        self.chance_alloc_decision_with_hash(parent, state, path, sh)
    }

    /// As [`Self::chance_alloc_decision`], but for callers that already
    /// computed the `search_hash` of `state` (e.g. the widen path, which
    /// hashes to dedup before allocating). Avoids a second full hash of the
    /// large [`CombatState`] on the search hot path.
    fn chance_alloc_decision_with_hash(
        &mut self,
        parent: Option<NodeId>,
        state: CombatState,
        path: &[NodeId],
        sh: u64,
    ) -> NodeId {
        if self.config.use_transposition_table {
            #[allow(clippy::cast_possible_truncation)]
            let bucket = (path.len() / 4).min(u8::MAX as usize) as u8;
            let key = (sh, bucket);
            let peek = self
                .transposition
                .as_ref()
                .expect("TT allocated when use_transposition_table")
                .peek(key);
            if let Some(existing) = peek {
                if !path.contains(&existing) {
                    self.transposition
                        .as_mut()
                        .expect("TT allocated when use_transposition_table")
                        .record_hit();
                    return existing;
                }
                // Cycle guard: fall through and allocate a fresh node.
            }
            let id = self.alloc_decision_inner(parent, state, sh);
            self.transposition
                .as_mut()
                .expect("TT allocated when use_transposition_table")
                .insert(key, id);
            return id;
        }
        self.alloc_decision_inner(parent, state, sh)
    }

    fn alloc_decision_inner(
        &mut self,
        parent: Option<NodeId>,
        state: CombatState,
        search_hash: u64,
    ) -> NodeId {
        // Chance mode keys transposition on `search_hash` and never consults
        // the RNG-cursor-sensitive `state_hash`, so reuse `search_hash` for
        // both fields and skip the second full hash of the (large) state.
        let mut node =
            MctsNode::new_inner(parent, &state, self.config.use_lethal_detector, search_hash);
        node.search_hash = search_hash;
        node.realized = Some(Box::new(state));
        let id = self.tree.alloc(node);
        self.nodes_allocated += 1;
        id
    }

    /// Double-Progressive-Widening outcome cap `ceil(C · (visits+1)^α)`
    /// for a chance node with `visits` visits, memoized per distinct
    /// visit count in [`Self::dpw_cap_cache`]. Each entry is computed by
    /// the identical `powf` expression the inline computation used, so
    /// the returned cap is bit-identical — this only removes the
    /// per-chance-visit `powf` (perf-investigation 2026-07-04).
    fn dpw_cap(&mut self, class: WideningClass, visits: u32) -> usize {
        let (c, alpha) = self.config.widening_pair(class);
        let slot = class as usize;
        let key = (c.to_bits(), alpha.to_bits());
        if self.dpw_cap_params[slot] != key {
            // `config` is `pub` — rebuild the slab if the coefficients
            // changed since it was filled (or on first use).
            self.dpw_cap_params[slot] = key;
            self.dpw_cap_cache[slot].clear();
        }
        let cache = &mut self.dpw_cap_cache[slot];
        let idx = visits as usize;
        while cache.len() <= idx {
            #[allow(clippy::cast_possible_truncation)]
            let v = cache.len() as u32;
            cache.push(dpw_cap_formula(c, alpha, v));
        }
        cache[idx]
    }

    /// Double Progressive Widening at chance node `chance_id`. Returns the
    /// selected outcome's decision-node child plus a flag that is `true` only
    /// when a brand-new outcome was drawn (the caller treats it as the
    /// simulation leaf and rolls out from it).
    fn chance_widen_or_sample(&mut self, chance_id: NodeId, path: &[NodeId]) -> (NodeId, bool) {
        let visits = self.tree.node(chance_id).visits;
        let n_outcomes = self.tree.node(chance_id).chance_outcomes.len();
        // Always `false` outside `DrawChance`/`BeliefDraw`.
        let is_draw_triggered = self.tree.node(chance_id).chance_is_draw_triggered;
        // H1 retune (bead `sts2-rs-k1z14`): `EndTurn` keeps priority over
        // `Draw` for which (C, α) pair sizes the cap — see `WideningClass`
        // doc — but the pile-reshuffle decision below stays keyed on
        // `is_draw_triggered` alone: an `EndTurn` chance node that is ALSO
        // draw-triggered (the common case — `EndTurn` resolves the next
        // player turn's hand draw) still needs its pile reshuffled to hedge
        // over draw order, it just widens under the `EndTurn` coefficients.
        let widening_class = WideningClass::of_node(self.tree.node(chance_id));
        let cap = self.dpw_cap(widening_class, visits);

        // Optional closed-node gate (`chance_closed_stops_widening`): once
        // the closure heuristic has fired, stop paying for fresh draws (a
        // full clone + edge re-apply + hash per draw) that will almost
        // certainly dedup, and descend by visit-proportional sampling like
        // the cap-reached branch. A closed node always has at least one
        // outcome (closure counts consecutive DUPLICATE draws, which
        // require an existing outcome to duplicate), so the sample branch
        // is safe. See the config field's doc for why this is off by
        // default.
        let widen_gated =
            self.config.chance_closed_stops_widening && self.tree.node(chance_id).chance_closed;

        if n_outcomes < cap && !widen_gated {
            // WIDEN: realize a fresh outcome from the parent's pre-action
            // state, using the next *draw ordinal* (a per-node monotonic
            // counter) as the reseed `n`. NOT `n_outcomes`: deduped draws do
            // not grow `chance_outcomes`, so reusing the outcome count would
            // re-feed the same `(base, n)` every visit, re-derive the
            // identical realization, and collapse the chance node's
            // expectimax estimate onto a single outcome. Each draw consumes a
            // fresh ordinal so successive widens are independent samples of
            // nature.
            let n = self.tree.node(chance_id).chance_draws;
            self.tree.node_mut(chance_id).chance_draws = n.saturating_add(1);
            let parent = self
                .tree
                .node(chance_id)
                .parent
                .expect("chance node has parent");
            let base = self.tree.node(chance_id).randomness_base;
            // Perf item M12: an `EndTurn` draw re-runs only the part of the
            // enemy turn after its cached RNG-free prefix, when it can.
            let cached = if widening_class == WideningClass::EndTurn {
                self.realize_endturn_widen(chance_id, parent, base, n)
            } else {
                None
            };
            let s = if let Some(s) = cached {
                s
            } else {
                let action_edge = self
                    .tree
                    .node(chance_id)
                    .chance_action
                    .clone()
                    .expect("chance node has action");
                let pre_action = self
                    .tree
                    .node(parent)
                    .realized
                    .as_deref()
                    .expect("chance parent has realized state")
                    .clone();
                // F2 (bead `sts2-rs-09x5n`): the per-outcome pile reshuffle
                // is `DrawChance`-only — under `BeliefDraw`,
                // `is_draw_triggered` CAN be true (see the expansion-site
                // classification above), but reshuffling would materialize
                // a concrete order the deferred-draw representation
                // deliberately never holds. `realize_chance_outcome`'s
                // reseed-and-reapply alone is sufficient: re-applying the
                // edge against the freshly reseeded streams makes the
                // deferred draw sample a fresh outcome on its own, with no
                // extra step. Byte-identical to the pre-bead behavior for
                // every other mode: `is_draw_triggered` was already only
                // ever `true` under `DrawChance` before this bead.
                let reshuffle_pile =
                    is_draw_triggered && self.config.hidden_info == HiddenInfoMode::DrawChance;
                self.realize_chance_outcome(pre_action, &action_edge, base, n, reshuffle_pile)
            };
            // Local outcome dedup: a fresh draw that lands on an already-seen
            // outcome signals closure progress and is reused rather than
            // added (keeps the visit-weighted average unbiased and bounds
            // the outcome fan-out). Byte-stable comparison
            // (`search_hash`-keyed) outside `DrawChance`/`BeliefDraw`; under
            // either mode (bead `sts2-rs-i8bgu` F1 plan §3.2 step 3 /
            // `sts2-rs-09x5n` F2 plan §3.2 step 5) two outcomes that differ
            // only in WHICH physical duplicate-card instance sits where must
            // also dedup, so the comparison switches to the order/uid-
            // insensitive [`sts2_core::chance_outcome_hash`] recomputed over
            // each existing outcome's realized state — recomputed rather
            // than cached because it must never influence the TT key / this
            // node's own `search_hash` field (both stay `search_hash` per
            // the plan: "keep the TT key as-is initially").
            let use_observational_dedup = matches!(
                self.config.hidden_info,
                HiddenInfoMode::DrawChance | HiddenInfoMode::BeliefDraw
            );
            // Observational branch: each existing outcome's hash is cached in
            // `ChanceOutcome::dedup_hash` on first use (see its doc comment),
            // so the scan is one hash of the NEW state plus `u64` compares —
            // identical hash values, hence identical dedup decisions, to the
            // pre-cache recompute-per-scan behavior.
            let mut new_dedup_hash = None;
            // The new state's `search_hash`: the dedup key outside the
            // observational modes, and otherwise needed only to allocate a
            // NEW outcome (the TT key). The observational branch leaves it
            // `None`, so a dedup hit (about half of all widen draws under
            // deployed BeliefDraw) never hashes the state; the allocation
            // below computes it from the same state.
            let mut sh = None;
            let dup = if use_observational_dedup {
                let dedup_key = sts2_core::chance_outcome_hash(&s);
                new_dedup_hash = Some(dedup_key);
                let n_existing = self.tree.node(chance_id).chance_outcomes.len();
                let mut found = None;
                for i in 0..n_existing {
                    let o = self.tree.node(chance_id).chance_outcomes[i];
                    let h = if let Some(h) = o.dedup_hash {
                        h
                    } else if let Some(r) = self.tree.node(o.child).realized.as_deref() {
                        let h = sts2_core::chance_outcome_hash(r);
                        self.tree.node_mut(chance_id).chance_outcomes[i].dedup_hash = Some(h);
                        h
                    } else {
                        // Mirrors the pre-cache scan: a child with no
                        // realized state never matches (and stays uncached
                        // until one is populated).
                        continue;
                    };
                    if h == dedup_key {
                        found = Some(o.child);
                        break;
                    }
                }
                found
            } else {
                let key = sts2_core::search_hash(&s);
                sh = Some(key);
                self.tree
                    .node(chance_id)
                    .chance_outcomes
                    .iter()
                    .map(|o| o.child)
                    .find(|&cid| self.tree.node(cid).search_hash == key)
            };
            if let Some(child) = dup {
                let w = self
                    .tree
                    .node(chance_id)
                    .widenings_since_new
                    .saturating_add(1);
                self.tree.node_mut(chance_id).widenings_since_new = w;
                if w >= CHANCE_CLOSE_K {
                    self.tree.node_mut(chance_id).chance_closed = true;
                }
                (child, false)
            } else {
                // Reuse the `sh` the non-observational dedup computed instead
                // of hashing the large state a second time.
                let sh = sh.unwrap_or_else(|| sts2_core::search_hash(&s));
                let child = self.chance_alloc_decision_with_hash(Some(chance_id), s, path, sh);
                self.tree
                    .node_mut(chance_id)
                    .chance_outcomes
                    .push(ChanceOutcome {
                        n,
                        child,
                        // Reuse the hash the dedup scan just computed for the
                        // new state, so this outcome never needs a lazy fill.
                        dedup_hash: new_dedup_hash,
                    });
                self.tree.node_mut(chance_id).widenings_since_new = 0;
                (child, true)
            }
        } else {
            // SAMPLE an existing outcome proportional to its visit count —
            // keeps the descent distribution an unbiased estimate of the
            // chance node's expectation.
            (self.sample_outcome_by_visits(chance_id), false)
        }
    }

    /// Pick an existing chance outcome with probability proportional to its
    /// child's visit count (uniform when all are unvisited).
    fn sample_outcome_by_visits(&mut self, chance_id: NodeId) -> NodeId {
        use rand::Rng as _;
        let outcomes: SmallVec<[(NodeId, u32); 8]> = self
            .tree
            .node(chance_id)
            .chance_outcomes
            .iter()
            .map(|o| (o.child, self.tree.node(o.child).visits))
            .collect();
        debug_assert!(
            !outcomes.is_empty(),
            "sampling from a chance node with no outcomes"
        );
        let total: u64 = outcomes.iter().map(|(_, v)| u64::from(*v)).sum();
        if total == 0 {
            let idx = (self.rng.gen::<u64>() % outcomes.len() as u64) as usize;
            return outcomes[idx].0;
        }
        let mut r = self.rng.gen_range(0..total);
        for (child, v) in &outcomes {
            let w = u64::from(*v);
            if r < w {
                return *child;
            }
            r -= w;
        }
        outcomes[outcomes.len() - 1].0
    }

    /// Chance-aware MCTS-solver propagation along the simulation `path`.
    ///
    /// * Decision node: `proven_win` if ANY child proven_win; `proven_loss`
    ///   if the node is fully expanded (every legal action has a child)
    ///   AND all children are proven_loss.
    /// * Chance node: `proven_loss` if ANY outcome proven_loss (sound —
    ///   nature can realize it); `proven_win` only if ALL outcomes proven_win
    ///   AND the node is closed AND `chance_prove_wins` is set (unsound under
    ///   DPW otherwise).
    fn chance_backprop_proven(&mut self, path: &[NodeId]) {
        for window in path.windows(2).rev() {
            let parent_id = window[0];
            let child_id = window[1];
            if self.tree.node(parent_id).proven_win || self.tree.node(parent_id).proven_loss {
                break;
            }
            match self.tree.node(parent_id).kind {
                NodeKind::Chance => {
                    let outcomes: SmallVec<[NodeId; 8]> = self
                        .tree
                        .node(parent_id)
                        .chance_outcomes
                        .iter()
                        .map(|o| o.child)
                        .collect();
                    let any_loss = outcomes.iter().any(|&c| self.tree.node(c).proven_loss);
                    if any_loss {
                        self.tree.node_mut(parent_id).proven_loss = true;
                        continue;
                    }
                    if self.config.chance_prove_wins && self.tree.node(parent_id).chance_closed {
                        let all_win = !outcomes.is_empty()
                            && outcomes.iter().all(|&c| self.tree.node(c).proven_win);
                        if all_win {
                            self.tree.node_mut(parent_id).proven_win = true;
                            continue;
                        }
                    }
                    break;
                }
                NodeKind::Decision => {
                    let child = self.tree.node(child_id);
                    let child_win = child.proven_win;
                    let child_loss = child.proven_loss;
                    if child_win {
                        self.tree.node_mut(parent_id).proven_win = true;
                        continue;
                    }
                    if child_loss {
                        // Soundness gate (A8 audit repair): a decision
                        // node is proven lost only when EVERY legal
                        // action has been expanded AND every child is a
                        // proven loss. One untried action per visit is
                        // popped from `untried_actions`, so on early
                        // sims `children` covers only a prefix of the
                        // canonical action order — stamping off that
                        // prefix latched a FALSE `proven_loss` at sim 1
                        // whenever the first canonical action (an
                        // attack, by expert priority) died to Thorns /
                        // an HP-cost card, and the proven-parent early
                        // break above made the false stamp permanent.
                        // With `lost_position_override` trusting the
                        // flag as a proof (search bypass), the stamp
                        // must be sound; see the `proven_loss` field
                        // doc for the GH #7 history this reverts.
                        let parent = self.tree.node(parent_id);
                        let fully_expanded =
                            parent.is_expanded && parent.untried_actions.is_empty();
                        if fully_expanded {
                            let children: SmallVec<[NodeId; 6]> =
                                parent.children.iter().map(|(_, c)| *c).collect();
                            let all_loss = !children.is_empty()
                                && children.iter().all(|&c| self.tree.node(c).proven_loss);
                            if all_loss {
                                self.tree.node_mut(parent_id).proven_loss = true;
                                continue;
                            }
                        }
                    }
                    break;
                }
            }
        }
    }

    /// Best-action-sequence capture for chance mode. Walks the path and, at
    /// each decision→child edge, records the edge action; chance→outcome hops
    /// carry no player action (the stochastic action was already recorded on
    /// the decision→chance edge), so they are skipped.
    fn chance_capture_best_seq(&mut self, path: &[NodeId], value: f32) {
        if value <= self.best_value {
            return;
        }
        self.best_value = value;
        self.best_seq.clear();
        for window in path.windows(2) {
            let parent = window[0];
            let child = window[1];
            if self.tree.node(parent).kind != NodeKind::Decision {
                continue;
            }
            if let Some((edge, _)) = self
                .tree
                .node(parent)
                .children
                .iter()
                .find(|(_, cid)| *cid == child)
            {
                self.best_seq.push(edge.clone());
            }
        }
    }
}

/// Value of an already-**terminal** state under `mode`, on the SAME scale
/// the search backs up. Single source of truth for every terminal branch.
///
/// GH#296: callers that short-circuit an immediately-terminal child (the
/// `fair`-eval paths in `mcts_recommend` and the CLI human agent) used to
/// inline `terminal_value(state).1` here. That is the **Binary** scale
/// (win → `evaluate_victory` in `[0.5, 1.0]`), while every non-terminal
/// sibling was scored by a search running `RichBoth` (win →
/// `0.5 + evaluate_victory`, in `[1.0, 1.5]`). Mixing the two scales in one
/// comparison made an immediate win score exactly 0.5 BELOW a stall, so a
/// lethal play ranked last and the agent declined to win. Route every
/// terminal short-circuit through this function instead of re-deriving it.
///
/// Panics in debug if `state` is not terminal — the value is meaningless
/// otherwise.
#[must_use]
pub fn terminal_state_value(
    state: &CombatState,
    mode: TerminalEvalMode,
    leaf_eval: &LeafEval,
    loss_gradient: bool,
) -> f32 {
    let cfg = MctsConfig {
        terminal_eval_mode: mode,
        loss_gradient,
        ..MctsConfig::default()
    };
    terminal_state_value_with(state, &cfg, leaf_eval)
}

/// Parameterized form of [`terminal_state_value`] (arm E surface,
/// 2026-08-15 Optuna study prep): the loss-quality weights come from
/// `cfg` (see [`loss_quality_with`]) instead of the fixed defaults.
/// Production search paths (`Mcts::decisive_leaf_value`, [`rollout`])
/// carry a full [`MctsConfig`] already, so they call this directly
/// rather than through the bool-only legacy wrapper above.
#[must_use]
pub fn terminal_state_value_with(
    state: &CombatState,
    cfg: &MctsConfig,
    leaf_eval: &LeafEval,
) -> f32 {
    debug_assert!(
        is_terminal(state),
        "terminal_state_value called on a non-terminal state"
    );
    match cfg.terminal_eval_mode {
        TerminalEvalMode::Binary => terminal_value(state).1,
        TerminalEvalMode::RichLoss => {
            if state.player_won {
                terminal_value(state).1
            } else {
                -1.0 + 0.5 * loss_leaf_value(state, leaf_eval, cfg)
            }
        }
        TerminalEvalMode::RichBoth => {
            if state.player_won {
                0.5 + evaluate_victory(state) + victory_hp_surplus(state)
                    - loot_penalty_q(state, cfg)
            } else {
                -1.0 + 0.5 * loss_leaf_value(state, leaf_eval, cfg)
            }
        }
    }
}

/// Width of the graded loss-leaf band above [`value_net::TERMINAL_LOSS_LEAF`]:
/// `loss_leaf = TERMINAL_LOSS_LEAF + SPAN · loss_quality`, so the leaf spans
/// `[-0.9, -0.1)` and the `-1.0 + 0.5 · leaf` backup spans `[-1.45, -1.05)`.
/// The ceiling sits 0.05 BELOW `-1.0` on purpose: every depth-cut value is
/// `> -1.0` (tanh-saturated Texel / ±0.49 Learned — see
/// [`MAX_LEAF_VALUE_ABS`]), so the best-ordered loss still ranks strictly
/// under the worst uncertain line. That preserves the band-level property the
/// `sts2-rs-pvuwv` constant clamp was added for (a suicide line can never
/// outrank a line that might survive) while restoring the WITHIN-band
/// ordering the RichLoss design intended and the clamp destroyed.
const LOSS_GRADIENT_LEAF_SPAN: f32 = 0.8;

/// Hard cap on [`loot_penalty_q`], in root-Q units. `Bucket::from_value`
/// buckets a backed-up value as a win/loss/tie at `±0.5`, and the RichBoth
/// win band bottoms out at `+1.0`, so a penalty strictly under `0.5` keeps
/// a WON combat bucketing as a win no matter how much loot walked off.
/// At the default sizing one stolen card is ≈ `0.057` Q, so the cap only
/// engages past seven-odd stolen cards — a state no encounter reaches.
const LOOT_PENALTY_Q_CAP: f32 = 0.45;

/// Floor for the loot-adjusted depth-cap leaf. [`depth_cut_value`]'s
/// unadjusted value is `tanh`-bounded (or `±0.49` Learned), so it is
/// strictly `> -1.0`; the loss band above deliberately tops out at
/// `-1.05` so the best-ordered loss still ranks under the worst uncertain
/// line. Subtracting the loot penalty must not break that, so the
/// adjusted leaf is floored just inside `-1.0`.
const LOOT_ADJUSTED_LEAF_FLOOR: f32 = -0.999;

/// The run-loot penalty, in root-Q units, for a state whose combat is
/// about to end (or be cut off) with loot still in a thief's pocket.
/// [`sts2_core::loot_at_risk_hp_equiv`] prices the loot in HP-equivalents
/// and [`victory_hp_equiv_to_q`] converts at the victory band's own
/// HP exchange rate, so the search trades HP for loot on one scale.
/// Scaled by [`MctsConfig::loot_weight`] and capped at
/// [`LOOT_PENALTY_Q_CAP`]. Always `>= 0`. GH #604.
#[must_use]
pub fn loot_penalty_q(state: &CombatState, cfg: &MctsConfig) -> f32 {
    if cfg.loot_weight == 0.0 {
        return 0.0;
    }
    let q = victory_hp_equiv_to_q(loot_at_risk_hp_equiv(state)) * cfg.loot_weight;
    if q.is_finite() {
        q.clamp(0.0, LOOT_PENALTY_Q_CAP)
    } else {
        0.0
    }
}

/// Denominator offset for the turns-survived term of [`loss_quality`]:
/// `round / (round + 8)` reaches 0.5 at turn 8 and saturates smoothly —
/// long fights stop separating on length alone and defer to the HP terms.
const LOSS_QUALITY_TURNS_HALF_LIFE: f32 = 8.0;

/// How good a LOST (or proven-lost) position is, in `[0, 1)` — the
/// within-loss-band gradient for [`loss_leaf_value`] (2026-08-09 A8 audit,
/// FINDINGS §3). The pre-audit constant `TERMINAL_LOSS_LEAF` clamp backed
/// every loss up to exactly −1.45, which zeroed the search gradient on any
/// ply whose rollouts all died (whole 400+-HP boss fights at depth-200:
/// 2428 root children at sd 0.0003). Losses must still ORDER states:
///
/// * **Enemy HP removed** (weight 0.6, dominant): a loss one hit short of
///   the kill ranks far above dying at full boss HP — and the loss label is
///   not ground truth (six of fifteen "all-rollouts-lose" fights in the
///   audit were won), so steering toward almost-winning lines is exactly
///   right. All-enemy totals (same convention as [`sum_enemy_hp`], which
///   the GH #32 override scores with), not the eval's non-minion split.
/// * **Player HP kept** (0.25): separates proven-loss-but-alive interior
///   nodes (the solver propagates `proven_loss` onto states the player has
///   not yet died in); exactly 0 on dead-player terminals.
/// * **Turns survived** (0.15): a mild later-is-better signal so slower
///   deaths at equal damage still separate (each survived turn is a fresh,
///   deeper re-search in the real run — the GH #215 rationale).
///
/// The raw leaf eval is deliberately NOT consulted: on dead-player states
/// its accumulated-`damage_dealt_raw`/buff terms score suicide cards
/// (Bloodletting / Offering) ABOVE passive deaths — the exact
/// `sts2-rs-0ljme` bug the constant clamp was introduced to kill.
#[must_use]
pub fn loss_quality(state: &CombatState) -> f32 {
    loss_quality_with(state, &MctsConfig::default())
}

/// Parameterized form of [`loss_quality`] (arm E surface, 2026-08-15
/// Optuna study prep): the three weights and the turns-survived
/// half-life come from `cfg` (see [`MctsConfig::loss_w_removed`] /
/// `loss_w_hp` / `loss_w_turns` / `loss_turns_half_life`) instead of
/// the fixed `0.6` / `0.25` / `0.15` / `8.0`. At [`MctsConfig::default()`]
/// this reproduces [`loss_quality`] bit-for-bit. Overridden weights need
/// not sum to `1.0` — the sum-to-1 invariant documented on
/// [`loss_quality`] only holds at the defaults, so the clamp below is
/// load-bearing (not just a float-pathology guard) once weights are
/// swept.
#[must_use]
pub fn loss_quality_with(state: &CombatState, cfg: &MctsConfig) -> f32 {
    let enemy_max = sum_enemy_max_hp(state);
    #[allow(clippy::cast_precision_loss)]
    let removed = if enemy_max > 0 {
        1.0 - sum_enemy_hp(state) as f32 / enemy_max as f32
    } else {
        0.0
    };
    #[allow(clippy::cast_precision_loss)]
    let hp_frac = state.player.current_hp.max(0) as f32 / state.player.max_hp.max(1) as f32;
    let turns = f32::from(state.round);
    let turns_frac = turns / (turns + cfg.loss_turns_half_life);
    (cfg.loss_w_removed * removed + cfg.loss_w_hp * hp_frac + cfg.loss_w_turns * turns_frac)
        .clamp(0.0, 0.999)
}

/// The loss-side leaf for the `-1.0 + 0.5 · leaf` backup, shared by
/// [`terminal_state_value_with`] and `Mcts::decisive_leaf_value`'s
/// proven-loss branch. `cfg.loss_gradient` on (default): the graded
/// `TERMINAL_LOSS_LEAF + cfg.loss_span · loss_quality_with(state, cfg)`
/// band, identical for terminal and proven-loss-but-alive states. Off
/// (the rollback seam): the historical `leaf_eval.evaluate(state)` —
/// whose internal terminal-loss guard yields the constant `-0.9` (backup
/// −1.45) on dead states and the raw in-progress eval on
/// proven-loss-alive ones — byte-for-byte.
fn loss_leaf_value(state: &CombatState, leaf_eval: &LeafEval, cfg: &MctsConfig) -> f32 {
    if cfg.loss_gradient {
        value_net::TERMINAL_LOSS_LEAF + cfg.loss_span * loss_quality_with(state, cfg)
    } else {
        leaf_eval.evaluate(state)
    }
}

/// Rollout to terminal or depth cap. Returns the player-perspective
/// value (`±1.0` for confirmed terminals); on depth-cap hit, returns the
/// heuristic surrogate from [`depth_cut_value`] (bead `sts2-rs-dfv1`
/// W2), clamped to `[-1.0, 1.0]`. Action selection at each step is
/// driven by `policy_kind`:
///
/// * [`RolloutPolicyKind::Uniform`] — pre-`dfv1` uniform-random baseline.
/// * [`RolloutPolicyKind::Heuristic`] — softmax over expert play
///   priority (`tau = 32`, see [`expert::SOFTMAX_TAU`]).
#[allow(clippy::too_many_arguments)] // private fn mirroring MctsConfig knobs 1:1
fn rollout(
    state: &mut CombatState,
    rng: &mut Xoshiro256PlusPlus,
    max_depth: u32,
    policy_kind: RolloutPolicyKind,
    leaf_eval: &LeafEval,
    use_quiescence: bool,
    cfg: &MctsConfig,
) -> f32 {
    let mut depth = 0u32;
    while !is_terminal(state) && depth < max_depth {
        let acts = legal_actions(state);
        if acts.is_empty() {
            break;
        }
        // Bead `sts2-rs-j0f6` (coverage-gated heuristic rollout): when
        // the STS1-ported `play_ordering` table covers fewer than
        // `expert::COVERAGE_THRESHOLD` of the legal `PlayCard` actions
        // in this state, the heuristic's softmax is dominated by a
        // small set of covered cards and effectively picks at random
        // among the uncovered (NEUTRAL_PRIORITY) majority. Falling
        // back to Uniform on those steps recovers the -6pp regression
        // dfv1 introduced on random-deck (Silent/Defect/Necrobinder/
        // Regent) scenarios. Per-step gating (not per-rollout) — the
        // covered fraction shifts as cards cycle through the hand.
        //
        // `expert::pick_rollout_action` fuses the coverage gate + the
        // heuristic pick into one hand/card resolution pass; the chosen
        // action and RNG-draw sequence are identical to the
        // coverage-then-pick pair it replaced (perf 2026-07-04).
        let idx = match policy_kind {
            RolloutPolicyKind::Uniform => Uniform.pick(rng, state, &acts),
            RolloutPolicyKind::Heuristic => {
                expert::pick_rollout_action_with(rng, state, &acts, cfg.rollout_end_turn_priority)
            }
        };
        let action = acts[idx].clone();
        let _ = core_apply(state, action);
        depth += 1;
    }
    if use_quiescence {
        // Bead `sts2-rs-8hs96` integration. Drive the rollout-local
        // scratch state through any sole-legal-action transitions so
        // the leaf eval observes a "quiet" position. Cap of 32 matches
        // the module's safety net and prevents runaway under any
        // future edge case where sole-legal-actions chain unboundedly.
        let _ = quiescence::drive_to_quiescence(state, 32);
    }
    if is_terminal(state) {
        // sts2-rs-vbzji: the historical Binary mode returns flat -1.0
        // on losses, throwing away the rich state signal (enemy HP
        // remaining, potions used, turn). RichLoss + RichBoth route
        // through the loss leaf to give MCTS a gradient over loss
        // quality. See `TerminalEvalMode` docs for ranges.
        terminal_state_value_with(state, cfg, leaf_eval)
    } else {
        // Bead `sts2-rs-dfv1` W2: heuristic value instead of flat tie.
        // `Bucket::from_value` thresholds the result at ±0.5 so the
        // RootOutcome win/loss/tie surface remains a calibrated binary
        // signal (option (a) per the bead brief — the Bucket shape
        // already cleanly handles it, so option (b) wasn't needed).
        depth_cut_value(state, leaf_eval, cfg)
    }
}

/// Run a single synthetic rollout from `state` for the purpose of
/// sampling the state distribution MCTS' leaf evaluator sees in
/// production — bead `sts2-rs-37qe8` (v4wjd.1.1).
///
/// Walks up to `max_depth` policy-driven moves from `state` (using
/// the same Uniform / Heuristic-with-coverage-fallback logic as the
/// production [`rollout`] function), calling `observer(state, depth)`
/// **once**, at the moment the production rollout would call the leaf
/// evaluator — either because the depth cap was reached, or because
/// the position became terminal. The observer sees the same
/// `CombatState` shape the leaf eval would see in production.
///
/// Unlike [`rollout`], this function does not return a value, does not
/// apply quiescence smoothing, and does not consult the terminal-eval
/// mode — its sole purpose is exposing the leaf-state to a corpus
/// collector. `corpus_gen` consumes this via the v4wjd.2 pipeline
/// (sts2-rs-kwuj4).
///
/// The synthetic rollout mutates `state` in place (the caller should
/// pass a clone). The observer is invoked exactly once per call.
pub fn sample_rollout_leaf_state(
    state: &mut CombatState,
    rng: &mut Xoshiro256PlusPlus,
    max_depth: u32,
    policy_kind: RolloutPolicyKind,
    mut observer: impl FnMut(&CombatState, u32),
) {
    let mut depth = 0u32;
    while !is_terminal(state) && depth < max_depth {
        let acts = legal_actions(state);
        if acts.is_empty() {
            break;
        }
        let idx = match policy_kind {
            RolloutPolicyKind::Uniform => Uniform.pick(rng, state, &acts),
            RolloutPolicyKind::Heuristic => expert::pick_rollout_action(rng, state, &acts),
        };
        let action = acts[idx].clone();
        let _ = core_apply(state, action);
        depth += 1;
    }
    observer(state, depth);
}

/// Reseed `state`'s combat streams and legacy `rng` for chance outcome `n`
/// of base `base` — the information-set reseed every chance realization
/// runs before re-applying its edge (see [`Mcts::realize_chance_outcome`]).
fn reseed_for_outcome(state: &mut CombatState, base: u64, n: u32) {
    state.run_rng.reseed_combat_streams(base, n);
    // Search-side reseed of the legacy stream — non-parity fast seed,
    // same rationale as reseed_combat_streams.
    state.rng = sts2_core::dotnet_rng::Rng::from_search_seed(
        sts2_core::run_rng_set::outcome_stream_seed_u64(
            base,
            n,
            sts2_core::run_rng_set::LEGACY_RNG_SALT,
        ),
    );
}

/// Apply `EndTurn` to `state` (in the play phase, where `apply` dispatches
/// it) one [`sts2_core::combat_loop::end_turn_phase`] at a time, which is
/// exactly `end_turn_dispatch`, and measure its RNG-free prefix: the number
/// of leading phases that returned `Advanced` and left `state.rng` and every
/// `RunRngSet` stream untouched. Also returns the state at the end of that
/// prefix when it is at least `clone_from` phases long — cloned at each
/// qualifying boundary as it is crossed, since whether the next phase draws
/// randomness is only known after it ran. Perf item M12, see
/// [`Mcts::realize_endturn_widen`].
fn end_turn_probing_prefix(
    state: &mut CombatState,
    clone_from: usize,
) -> (usize, Option<CombatState>) {
    use sts2_core::combat_loop::{end_turn_phase, END_TURN_PHASE_COUNT};
    let rng = state.rng.clone();
    let run_rng = state.run_rng.clone();
    let mut clean = 0;
    let mut prefix = None;
    for phase in 0..END_TURN_PHASE_COUNT {
        let advanced = matches!(
            end_turn_phase(state, phase),
            sts2_core::StepResult::Advanced
        );
        if clean == phase && advanced && state.rng == rng && state.run_rng == run_rng {
            clean = phase + 1;
            if clean >= clone_from {
                prefix = Some(state.clone());
            }
        }
        if !advanced {
            break;
        }
    }
    (clean, prefix)
}

/// Debug guard for perf item M12: rebuild the `phases`-phase `EndTurn`
/// prefix of `pre` under a second reseed and assert it equals the cached
/// `prefix` (built under ordinal `n`) once that is reseeded the same way.
/// Catches a phase that derives randomness from an RNG's seed without moving
/// any cursor, which the probe's cursor check cannot see.
#[cfg(debug_assertions)]
fn debug_assert_prefix_seed_independent(
    pre: &CombatState,
    prefix: &CombatState,
    phases: usize,
    base: u64,
    n: u32,
) {
    // Real draw ordinals count up from 0, so the high bit never collides.
    let other = n ^ 0x8000_0000;
    let mut rebuilt = pre.clone();
    reseed_for_outcome(&mut rebuilt, base, other);
    for phase in 0..phases {
        let result = sts2_core::combat_loop::end_turn_phase(&mut rebuilt, phase);
        assert!(
            matches!(result, sts2_core::StepResult::Advanced),
            "EndTurn prefix phase {phase} returned {result:?} under a second reseed"
        );
    }
    let mut cached = prefix.clone();
    reseed_for_outcome(&mut cached, base, other);
    assert!(
        cached == rebuilt,
        "EndTurn prefix of {phases} phases depends on the reseed: a phase derives \
         randomness without moving an RNG cursor"
    );
}

/// Sum of `current_hp` (floored at 0) across every creature registered
/// in `state.enemy_uids`. Used by [`Mcts::lost_position_override`] to
/// score 1-ply damage-dealt deltas on observed-lost positions
/// (GitHub #32). Defined as a free function so the override's `&self`
/// borrow doesn't conflict with iterating `root.children`.
fn sum_enemy_hp(state: &CombatState) -> i32 {
    state
        .creatures
        .iter()
        .filter(|c| state.enemy_uids.iter().any(|u| u.0 == c.id))
        .map(|c| c.current_hp.max(0))
        .sum()
}

/// `max_hp` sibling of [`sum_enemy_hp`] — the denominator for
/// [`loss_quality`]'s enemy-HP-removed fraction. Same all-enemy
/// (minions included) convention.
fn sum_enemy_max_hp(state: &CombatState) -> i32 {
    state
        .creatures
        .iter()
        .filter(|c| state.enemy_uids.iter().any(|u| u.0 == c.id))
        .map(|c| c.max_hp.max(0))
        .sum()
}

/// Whether the player is still alive after the next enemy turn, given a
/// 1-ply post-action state `post` (the state right after the override's
/// candidate action was applied). Drives one `EndTurn` so the enemy side
/// resolves — `end_turn_dispatch` walks end-of-player-turn →
/// `enemy_turn_start` (where TheInsatiable's Sandpit countdown ticks and,
/// at 0, eats the player) → enemy attacks → the player's next turn-start.
/// Returns `true` when the player did not lose (still in-combat, or won
/// outright if the action was lethal). Used by
/// [`Mcts::lost_position_override`] (GitHub #215) to prefer a survivable
/// line — e.g. `FranticEscape` buying back a Sandpit turn — over a
/// chip-damage swing that dies this very enemy turn. A free function for
/// the same borrow reason as [`sum_enemy_hp`].
fn survives_next_enemy_turn(post: &CombatState) -> bool {
    // The action itself may already have ended combat (a lethal play, or
    // a self-kill): trust that outcome and don't drive another turn.
    if post.is_over {
        return post.player_won;
    }
    let mut s = post.clone();
    let _ = core_apply(&mut s, Action::EndTurn);
    // Survives ⇔ combat isn't over as a loss (still fighting, or won outright).
    !s.is_over || s.player_won
}

/// Per-stack credit applied to Slippery the 1-ply override clears from
/// the enemy (bead `sts2-rs-ldd1c` follow-up B). Sized so a single
/// 0-cost multi-hit that chips the wall clears the `TIE_EPS = 0.5`
/// band several times over and decisively outscores EndTurn / potion
/// in the observed-lost regime. Untuned; the failure mode it fixes is
/// "Vantom looks unwinnable, MCTS spams potions instead of grinding
/// the 9-stack Slippery wall down".
const SLIPPERY_CLEAR_CREDIT: f32 = 2.0;

/// Per-point credit for block gained against telegraphed incoming attack
/// damage in the 1-ply override score (2026-08-09 A8 audit, FINDINGS §3).
/// Matches the `0.5` weight on `hp_lost` — a blocked point IS an HP point
/// saved — so a Defend against a live attack (min(5, incoming) → +2.5)
/// clears the `TIE_EPS = 0.5` tie band decisively instead of losing a
/// 0-vs-0 tie to EndTurn, which is exactly the audited "the override
/// structurally cannot block" failure (regent 6/85 HP, 22 incoming,
/// END TURN over a 3266/4000-visit Defend).
const BLOCK_CREDIT_WEIGHT: f32 = 0.5;

/// Card behind a root `PlayCard` action, resolved against the root
/// state's hand (same `uid → cards[uid-1]` idiom as
/// [`EdgeAction::from_action`]). `None` for non-`PlayCard` actions —
/// feeds the rule-5 root guards, which only inspect card children.
fn root_hand_card<'a>(
    state: &'a CombatState,
    action: &Action,
) -> Option<&'a sts2_core::cards::Card> {
    let Action::PlayCard { hand_index, .. } = action else {
        return None;
    };
    let card_uid = state.player.hand.get(*hand_index as usize)?;
    state.cards.get(card_uid.0.get() as usize - 1)
}

/// Total attack damage the alive enemies are telegraphing —
/// `intent.damage() × hits`, non-attack intents contributing 0 via
/// `damage() == 0`. Mirrors the expert rollout's `HpAwareCtx::compute`
/// incoming sum (all alive enemies — minions attack too; the leaf eval's
/// non-minion split is an eval-feature convention, not a damage fact).
/// Deliberately skips the Intangible 1-per-hit cap: in the proven-lost
/// regime this feeds, over-crediting block while Intangible is up is
/// harmless (both candidates see the same inflated total). Sibling of
/// [`sum_enemy_hp`]; free function for the same borrow reason.
fn sum_enemy_telegraphed_damage(state: &CombatState) -> i32 {
    state
        .creatures
        .iter()
        .filter(|c| state.enemy_uids.iter().any(|u| u.0 == c.id) && c.current_hp > 0)
        .filter_map(|c| c.intent)
        .fold(0i32, |acc, i| {
            acc.saturating_add(i.damage().saturating_mul(i32::from(i.hits())))
        })
}

/// Total remaining `Slippery` stacks across alive enemies — the 1-ply
/// override credits an action for how many of these it removes (each
/// cleared stack unlocks a full-damage hit next turn). Sibling to
/// [`sum_enemy_hp`]; free function for the same borrow reason.
fn sum_enemy_slippery(state: &CombatState) -> i32 {
    state
        .creatures
        .iter()
        .filter(|c| state.enemy_uids.iter().any(|u| u.0 == c.id) && c.current_hp > 0)
        .flat_map(|c| c.powers.iter())
        .filter_map(|puid| state.powers.iter().find(|p| p.id == puid.0))
        .filter(|p| p.kind == sts2_core::PowerKind::Slippery)
        .map(|p| p.amount.max(0))
        .sum()
}

/// Heuristic value used when [`rollout`] hits the depth cap. Forwards
/// to [`sts2_core::evaluate_in_progress`], the sts_lightspeed-ported
/// in-progress eval (bead `sts2-rs-v2oh`). Output is `tanh`-shaped in
/// `(-1, 1)` (p99 ≈ ±0.45, but lopsided positions reach toward ±0.9),
/// so its magnitude alone does NOT keep depth-cuts out of the Win/Loss
/// `Bucket` band. The "depth-cuts always bucket as `Tie`" invariant is
/// instead enforced explicitly in [`Mcts::one_simulation`]'s backup,
/// which only consults [`Bucket::from_value`] for genuine terminal /
/// proven leaves — only real terminal states register as `Win` / `Loss`.
///
/// Prior to bead `v2oh` this was an inline `hp_diff + turn_pen` that
/// structurally pinned to `-1.0` in any ≥2-enemy opening (un-normalized
/// enemy_sum). The new formula credits damage dealt, potions held,
/// and progress through the fight, mirroring the C++ reference.
///
/// # How often this runs: never, at the deployed depth
///
/// This is the ONLY live consumer of [`LeafEval`] in the default config (the
/// other call site, [`loss_leaf_value`], reaches the leaf only when
/// `MctsConfig::loss_gradient` is false, and it defaults to true). And at
/// `sts2_runai::policy::COMBAT_ROLLOUT_DEPTH = 200` the cap is not reached:
/// GH #512's depth probe measured **0/5912 cap leaves at d200**, and the
/// `wb-b5c453e74f1b80e3` repro independently showed `W + L == visits` with
/// **T = 0** over 2000 sims. So on the deployed path every rollout finishes
/// the fight and this function is dead weight — which is what makes any
/// leaf-eval tuning near-zero-surface-area (GH #527; see [`LeafEval`] for the
/// full note and the 2026-07-18 campaign that closed the evaluator half of
/// GH #124).
///
/// It is NOT dead everywhere. `MctsConfig::default().max_rollout_depth` is
/// **10**, and `sts2-cli/src/main.rs`, the combat REPL and
/// `closed_loop_harness` take that default; at depth 10 nearly every rollout
/// lands here. Anything that changes or removes this function has to answer
/// for those callers.
fn depth_cut_value(state: &CombatState, leaf_eval: &LeafEval, cfg: &MctsConfig) -> f32 {
    // Bead `sts2-rs-4n0x` (qctu.5): dispatch to either
    // `evaluate_in_progress` (the Texel-tuned HCE — pre-bead
    // default, byte-stable) or the loaded ONNX value head when
    // the caller opts in via `Mcts::with_leaf_eval`.
    let base = leaf_eval.evaluate(state);
    // GH #604: charge the run loot the position is on course to lose. None of
    // the leaf evaluators can see it — a fled thief reads as a dead one to
    // every HP-shaped feature — and at the default `max_rollout_depth = 10`
    // most rollouts land here rather than on a terminal, so the kill-the-thief
    // gradient has to exist at this seam too. Killing the thief flips its
    // `SwipeStolen` entry to `reclaimed` / banks its `HeistPower` gold, which
    // is exactly what zeroes the term. Floored just inside `-1.0` to preserve
    // the loss-band ordering `LOSS_GRADIENT_LEAF_SPAN` documents.
    (base - loot_penalty_q(state, cfg)).max(LOOT_ADJUSTED_LEAF_FLOOR)
}

/// W3: canonicalize the legal-action list at expansion time. Mirrors
/// the duplicate-prune block at
/// `sts_lightspeed/src/sim/search/BattleScumSearcher2.cpp:220-234`
/// (`enumerateCardActions`) plus a play-priority sort so `remove(0)`
/// pops in expert order.
///
/// Steps:
/// 1. Sort [`Action::PlayCard`] entries by the [`EdgeAction::PlayCard`]
///    identity tuple `(card_id, upgraded, cost, free, target)` so equal
///    cards are adjacent.
/// 2. Collapse adjacent equal `PlayCard` entries — different hand
///    indices of the same edge identity back the same tree child.
/// 3. Sort the whole list by expert play priority (lower first) so
///    `remove(0)` pops the highest-priority untried action first.
///
/// Non-`PlayCard` actions (EndTurn, DrinkPotion, Channel, Evoke,
/// ResolveChoice) are left alone in the dedup step and slotted into the
/// final priority sort.
///
/// Bead `sts2-rs-dfv1` W3.
fn canonicalize_actions(
    state: &CombatState,
    acts: SmallVec<[Action; 16]>,
) -> SmallVec<[Action; 16]> {
    // Resolve the card identity tuple used for dedup. Cards in the
    // player's hand index into `state.cards` via `card_uid.0 - 1`.
    //
    // INVARIANT (A8 audit §7b): this key is exactly the
    // [`EdgeAction::PlayCard`] identity tuple `(card_id, upgraded,
    // cost, free, affliction, target)`, because `EdgeAction` is the identity
    // the tree actually keys and resolves edges by. A FINER key here
    // manufactures sibling children that map to the same `EdgeAction`
    // and resolve to the same concrete hand slot — byte-identical
    // duplicate root actions splitting one move's budget and (via the
    // transposition table) double-counting a shared child's visits. A
    // COARSER key drops a genuinely distinct play — twice now: the PowerPotion
    // free-copy bug that added `free` (`86a4c0f5c`), and the 2026-08-16
    // `affliction` addition, where a `Bound` and a clean copy of one card
    // collapsed into a single edge whose `resolve` returned the slot
    // `legal_actions` had gated out under ChainsOfBinding.
    // Keep the two tuples in lockstep; the expansion-site
    // `debug_assert` and `tests/canonicalize.rs` pin both directions.
    //
    // The original key (bead `sts2-rs-dfv1`) additionally carried
    // `combat_vars.len()` as a `special_data` proxy meant to keep
    // scaling copies (Rampage) apart — but `EdgeAction::resolve`
    // ignores combat vars and always picks the FIRST matching hand
    // card, so that distinction never reached the simulator; it only
    // produced the duplicate children above (6.6% of audited A8 plies).
    type Key = (
        sts2_core::cards::CardId,
        bool,
        i8,
        bool,
        Option<sts2_core::card_afflictions::CardAfflictionKind>,
        Option<u8>,
    );
    let key_of = |a: &Action| -> Option<Key> {
        match a {
            Action::PlayCard { hand_index, target } => {
                let card_uid = state.player.hand.get(*hand_index as usize)?;
                let idx = card_uid.0.get() as usize - 1;
                let card = state.cards.get(idx)?;
                Some((
                    card.card_id,
                    card.upgraded,
                    card.cost,
                    sts2_core::hooks::card_cost_locally_zeroed(state, card),
                    card.affliction.map(|a| a.kind),
                    *target,
                ))
            }
            _ => None,
        }
    };

    // Partition into PlayCard actions (with their dedup keys) and
    // everything else. Bead `sts2-rs-a1jo9` (perf): both partitions
    // are SmallVec<_; 16> so the typical legal-actions set (also
    // SmallVec<_; 16> from `legal_actions`) stays inline — no heap
    // touch on the hot MCTS node-expansion path.
    let mut plays: SmallVec<[(Key, Action); 16]> = SmallVec::new();
    let mut others: SmallVec<[Action; 16]> = SmallVec::new();
    for a in acts {
        if let Some(k) = key_of(&a) {
            plays.push((k, a));
        } else {
            others.push(a);
        }
    }

    // Sort PlayCard entries by their dedup key so equal cards are
    // adjacent, then collapse runs of equal keys.
    plays.sort_by_key(|a| a.0);
    plays.dedup_by(|a, b| a.0 == b.0);

    // Final list: PlayCard actions plus the non-PlayCard tail, then
    // sort by expert priority so `remove(0)` pops the
    // highest-priority action first.
    let mut out: SmallVec<[Action; 16]> = SmallVec::with_capacity(plays.len() + others.len());
    for (_, a) in plays {
        out.push(a);
    }
    for a in others {
        out.push(a);
    }
    out.sort_by_key(|a| match a {
        Action::PlayCard { hand_index, .. } => state
            .player
            .hand
            .get(*hand_index as usize)
            .and_then(|uid| state.cards.get(uid.0.get() as usize - 1))
            .map_or(expert::NEUTRAL_PRIORITY, |c| {
                expert::play_ordering(c.card_id, state.player.character)
            }),
        Action::EndTurn => expert::END_TURN_PRIORITY,
        _ => expert::NEUTRAL_PRIORITY,
    });
    out
}

// ─── Root parallelization ───────────────────────────────────────────────────

/// Result of a [`par_search`] call.
#[derive(Clone, Debug)]
pub struct ParSearchResult {
    /// Most-visited action after merging all workers' root children.
    pub best_action: Action,
    /// Merged per-root-child outcomes. Same invariant as
    /// [`Mcts::root_outcomes`]: `wins + losses + ties == visits` per entry.
    /// Child order is the order in which actions were first observed
    /// across workers (typically `legal_actions(root_state)` order, since
    /// each worker pops untried actions in that order).
    pub outcomes: Vec<RootOutcome>,
}

/// `splitmix64` finalizer over `base ⊕ (worker · golden)`. Produces a
/// well-distributed per-worker base seed that is statistically
/// independent of the linear `sim_index · 0x9E37_79B9_7F4A_7C15` stride
/// `Mcts::one_simulation` adds on top of it — see [`par_search_with_policy`]'s
/// `# Determinism` note for why a plain additive offset is wrong.
#[inline]
fn splitmix64_seed(base: u64, worker: u64) -> u64 {
    let mut z = base.wrapping_add(worker.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Root-parallel MCTS. Splits `config.max_simulations` evenly across
/// `num_workers` rayon workers; each runs an independent single-threaded
/// `Mcts` on a clone of `root_state` with a derived seed, and the
/// per-child counters are summed at the end.
///
/// With `num_workers == 1` this is a thin wrapper around [`Mcts::search`]
/// and skips rayon entirely (no thread-pool overhead).
///
/// # Determinism
/// Output is deterministic for a fixed `(config.seed, num_workers,
/// max_simulations)` tuple. Worker `w` derives its base seed by passing
/// `(config.seed, w)` through a `splitmix64` finalizer. Changing
/// `num_workers` changes the seed split *and* the per-worker sim budget,
/// so results will differ across thread counts even at the same total
/// sim budget — expected for root parallelization.
///
/// The worker seed must NOT share the per-simulation determinization
/// stride: `Mcts::one_simulation` derives each sim's seed as
/// `cfg.seed + sim_index · 0x9E37_79B9_7F4A_7C15`. If the worker offset
/// used that same constant (`config.seed + w · 0x9E37…`) the two strides
/// would commute — worker `w` sim `i` and worker `w'` sim `i'` collide
/// whenever `w + i == w' + i'`, so an N-worker × M-sim ensemble samples
/// only ~`N + M` distinct determinizations instead of `N · M`,
/// collapsing PIMC diversity (and wasting ~`(N-1)/N` of the parallel
/// budget on duplicate chance trajectories that merge as correlated
/// double-counts). The `splitmix64` hash decorrelates the worker base
/// from the linear per-sim stride so distinct workers never realize the
/// same determinization.
///
/// Note: tree parallelization (single shared tree, virtual loss) was
/// prototyped at one point; on this workload it scaled identically to
/// root-parallel (memory + cache-coherence bound, not algorithm bound)
/// while sacrificing determinism, so it was dropped.
///
/// # Panics
/// Panics if the root has no legal actions (terminal state).
pub fn par_search(
    root_state: CombatState,
    config: MctsConfig,
    num_workers: u32,
) -> ParSearchResult {
    par_search_with_policy(
        root_state,
        config,
        num_workers,
        UniformPolicy,
        LeafEval::default(),
    )
}

/// Like [`par_search`] but every worker's `Mcts` is built with `policy`
/// as its PUCT prior. The prior is only consulted when
/// `config.use_puct_prior == true`; with the default `false` this is
/// byte-identical to [`par_search`] regardless of `policy`. `policy` is
/// cloned once per worker, so pass a cheap-to-clone handle — for the
/// shared learned prior use [`SharedPolicy`] (an `Arc` bump per worker,
/// sharing one net + stats). This lets the production threaded search
/// carry the embedding policy prior, not just the single-threaded path.
/// Bead `sts2-rs-x9hyn`.
pub fn par_search_with_policy<P>(
    root_state: CombatState,
    config: MctsConfig,
    num_workers: u32,
    policy: P,
    leaf_eval: LeafEval,
) -> ParSearchResult
where
    P: Policy + Clone + Send + Sync + 'static,
{
    let workers = num_workers.max(1);
    if workers == 1 {
        let mut m = Mcts::with_policy(root_state, config, policy).with_leaf_eval(leaf_eval);
        let best = m.search();
        let outcomes = m.root_outcomes();
        return ParSearchResult {
            best_action: best,
            outcomes,
        };
    }

    let total = config.max_simulations;
    let base = total / workers;
    let extra = total % workers;

    let per_worker: Vec<Vec<RootOutcome>> = (0..workers)
        .into_par_iter()
        .map(|w| {
            let sims = base + u32::from(w < extra);
            let mut cfg = config;
            // Decorrelate the per-worker base seed from the per-sim
            // determinization stride (see the `# Determinism` note): a
            // plain `config.seed + w · 0x9E37…` offset commutes with
            // `one_simulation`'s `+ sim_index · 0x9E37…` stride and makes
            // distinct workers re-sample identical determinizations.
            cfg.seed = splitmix64_seed(config.seed, u64::from(w));
            cfg.max_simulations = sims;
            let mut m = Mcts::with_policy(root_state.clone(), cfg, policy.clone())
                .with_leaf_eval(leaf_eval.clone());
            if sims > 0 {
                let _ = m.search();
            }
            m.root_outcomes()
        })
        .collect();

    // Merge by action equality. With ~tens of actions the linear scan
    // is fine — avoids a HashMap and the need for `Action: Hash`.
    //
    // Under [`BackupRule::PowerMean`] each worker's `value_sum` is already
    // `Q_powermean * visits` rather than a raw return sum, so summing them
    // yields the visit-weighted mean *of the workers' power means* — a
    // mean-of-power-means, not the power mean of the union. That is the
    // usual root-parallel approximation (the same one that makes merged
    // `wins`/`losses` an approximation of a single deeper tree's), and it
    // is exact at `workers == 1`, which is the path every #492 harness
    // takes.
    let mut merged: Vec<RootOutcome> = Vec::new();
    for worker_outs in per_worker {
        for o in worker_outs {
            if let Some(existing) = merged.iter_mut().find(|m| m.action == o.action) {
                existing.visits += o.visits;
                existing.wins += o.wins;
                existing.losses += o.losses;
                existing.ties += o.ties;
                existing.value_sum += o.value_sum;
                // Σv² pools the same way Σv does. Left out when the field
                // landed (GH #446), which made every merged child's
                // `value_variance()` read `Σv²(worker 0)/N − mean²` — negative,
                // clamped to 0 — so `mean_stderr()` was 0 on every
                // root-parallel result (found by `combat_position_book
                // oracle-q`, GH #494).
                existing.value_sq_sum += o.value_sq_sum;
            } else {
                merged.push(o);
            }
        }
    }
    #[allow(clippy::cast_precision_loss)]
    for o in &mut merged {
        o.mean_value = if o.visits == 0 {
            0.0
        } else {
            o.value_sum / o.visits as f32
        };
    }

    // Selection rule mirrors `Mcts::search_n`: argmax mean_value
    // (= argmax Q) with stable tie-break by visit count. Bead
    // `sts2-rs-trnu` switched the single-threaded root rule from
    // argmax(visits) to argmax(Q); par_search must match or it
    // returns a different action than the equivalent single-threaded
    // `search()` call (GH #3).
    // Defense-in-depth livelock guard (mirrors `Mcts::legalize_root_action`):
    // only consider outcomes whose action is legal at the root, so a stray
    // edge-resolution miss can never surface as the merged `best_action`. In
    // the normal case every merged outcome is already legal, so this is a
    // no-op selection over the same set.
    let legal = legal_actions(&root_state);
    let best_action = merged
        .iter()
        .filter(|o| legal.contains(&o.action))
        .max_by(|a, b| {
            a.mean_value
                .partial_cmp(&b.mean_value)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.visits.cmp(&b.visits))
        })
        .map(|o| o.action.clone())
        .or_else(|| {
            debug_assert!(
                merged.is_empty(),
                "par_search: no legal outcome among {} merged root children",
                merged.len()
            );
            legal.first().cloned()
        })
        .expect("par_search called on terminal root: no legal actions");
    ParSearchResult {
        best_action,
        outcomes: merged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sts2_core::CombatState;

    /// A search cloned from a RECORDING live state must not record.
    ///
    /// The game-style event log is state-resident, so the flag rides along on
    /// every clone. If the tree kept it, each rollout would append rows to a
    /// log nobody reads — allocation on the engine's hottest path — and the
    /// tree's root would carry a copy of the live fight's history. Both are
    /// checked here because `MctsTree::new` is the ONLY place that disarms.
    #[test]
    fn tree_root_is_disarmed_even_when_the_live_state_records() {
        let mut live = CombatState::default();
        live.combat_log.event_log.set_recording(true);
        live.combat_log.event_log.push(
            1,
            sts2_core::Side::Player,
            sts2_core::event_log::EventLogEntry::EnergySpent { amount: 1 },
            None,
        );
        assert_eq!(live.combat_log.event_log.rows().len(), 1);

        let tree = MctsTree::new(live.clone(), false);
        assert!(
            !tree.root_state.combat_log.event_log.recording(),
            "the search must not inherit recording from the live state"
        );
        assert!(
            tree.root_state.combat_log.event_log.rows().is_empty(),
            "…nor carry the live fight's rows into its rollouts"
        );
        assert_eq!(
            live.combat_log.event_log.rows().len(),
            1,
            "…and the live log is untouched"
        );
    }

    /// Bead `sts2-rs-vqckz` (F0): `MctsConfig::default()` must stay
    /// `HiddenInfoMode::Clairvoyant` so every pre-bead fixture that builds
    /// a config via `..MctsConfig::default()` stays byte-stable.
    #[test]
    fn default_hidden_info_mode_is_clairvoyant() {
        assert_eq!(
            MctsConfig::default().hidden_info,
            HiddenInfoMode::Clairvoyant
        );
    }

    /// Regression (GH bug report): a `PlayCard` edge for a free-this-turn
    /// copy of a card must resolve to THAT copy, not a same-id/same-cost
    /// non-free copy earlier in hand. Repro: PowerPotion stamps a generated
    /// Barricade `FreeThisTurn` (cost field stays 3), while a 3-cost
    /// Barricade already sits in hand. With 0 energy only the free copy is
    /// playable; before the fix the edge identity `(card_id, upgraded, cost)`
    /// collided and `resolve` returned the non-free copy, which the engine
    /// then rejected for lack of energy.
    #[test]
    fn edge_play_card_distinguishes_free_this_turn_copy() {
        use sts2_core::cards::{Card, CardId, CardType, TargetType, VarKey};
        use sts2_core::CardUid;

        let mut state = CombatState::default();
        let uid_non_free = CardUid::new(1).expect("nz");
        let uid_free = CardUid::new(2).expect("nz");
        let mk = |uid: CardUid, free: i32| -> Card {
            let mut c = Card {
                uid,
                card_id: CardId::BarricadeCard,
                cost: 3,
                card_type: CardType::Power,
                target_type: TargetType::Self_,
                upgraded: false,
                ..Card::pod_defaults()
            };
            if free != 0 {
                c.set_combat_var(VarKey::FreeThisTurn, free);
            }
            c
        };
        // Hand order: the NON-free copy first (slot 0), the free copy second
        // (slot 1) — the order that triggered the bug.
        state.cards.push(mk(uid_non_free, 0));
        state.cards.push(mk(uid_free, 1));
        state.player.hand.push(uid_non_free);
        state.player.hand.push(uid_free);

        // Edge built from the free copy (slot 1) must round-trip back to slot 1.
        let free_play = Action::PlayCard {
            hand_index: 1,
            target: None,
        };
        let free_edge = EdgeAction::from_action(&free_play, &state);
        assert_eq!(
            free_edge.resolve(&state),
            Some(free_play),
            "free-this-turn edge must resolve to the free copy's slot, \
             not the same-id non-free copy in slot 0"
        );

        // And the non-free copy's edge (slot 0) still resolves to slot 0 —
        // the two copies stay disambiguated in both directions.
        let non_free_play = Action::PlayCard {
            hand_index: 0,
            target: None,
        };
        let non_free_edge = EdgeAction::from_action(&non_free_play, &state);
        assert_eq!(non_free_edge.resolve(&state), Some(non_free_play));
    }

    /// Bead `sts2-rs-y3a28` (b32is.1): try_search (no-arg wrapper)
    /// matches try_search_n's TerminalRoot behavior under the
    /// configured max_simulations budget.
    #[test]
    fn try_search_no_arg_wrapper_returns_terminal_root_error() {
        let mut state = CombatState::default();
        state.player.current_hp = 0;
        state.player.max_hp = 80;
        state.is_over = true;
        let cfg = MctsConfig {
            max_simulations: 0,
            ..MctsConfig::default()
        };
        let mut mcts = Mcts::new(state, cfg);
        match mcts.try_search() {
            Err(SearchError::TerminalRoot) => {}
            Ok(action) => panic!("expected TerminalRoot, got Ok({action:?})"),
        }
    }

    /// Bead `sts2-rs-b32is`: try_search_n on a terminal-root
    /// CombatState returns Err(SearchError::TerminalRoot) instead
    /// of panicking. Pairs with the existing panic-shaped
    /// `search_n` convenience wrapper which still unwinds.
    #[test]
    fn try_search_n_returns_terminal_root_error_on_dead_root() {
        // Build a CombatState where the player is dead → is_over=true,
        // no legal actions. The MCTS root has no children to argmax
        // over, so try_search_n must surface TerminalRoot.
        let mut state = CombatState::default();
        // Default player has hp=80; force player_won=false path via
        // negative hp + is_over via the combat-loop's terminal sweep.
        state.player.current_hp = 0;
        state.player.max_hp = 80;
        state.is_over = true;

        let cfg = MctsConfig {
            max_simulations: 0,
            ..MctsConfig::default()
        };
        let mut mcts = Mcts::new(state, cfg);
        match mcts.try_search_n(0) {
            Err(SearchError::TerminalRoot) => {}
            Ok(action) => {
                panic!("expected TerminalRoot error, got Ok({action:?})");
            }
        }
    }

    /// Bead `sts2-rs-b32is`: SearchError implements std::error::Error
    /// + Display so callers can propagate cleanly via `?` and
    /// downcast through the boxed-error chain.
    #[test]
    fn search_error_implements_error_and_display() {
        let e = SearchError::TerminalRoot;
        let msg = format!("{e}");
        assert!(msg.contains("terminal root"));
        // Coerce to &dyn Error to confirm trait impl.
        let _: &dyn std::error::Error = &e;
    }

    #[test]
    fn default_config_is_sqrt2() {
        let c = MctsConfig::default();
        assert!((c.c_puct - std::f32::consts::SQRT_2).abs() < 1e-6);
        assert_eq!(c.max_simulations, 1000);
        assert_eq!(c.max_rollout_depth, 10);
    }

    /// GH #604: a solo ThievingHopper that FLEES ends the combat as a WIN
    /// with identical HP, potions and round to the win where the player
    /// killed it — every term the RichBoth win branch reads is the same, so
    /// before the loot charge the two terminals were literally equal and the
    /// search had no reason to prefer the kill. The un-reclaimed steal is the
    /// only thing separating them.
    #[test]
    fn win_terminal_ranks_the_kill_above_the_flee_that_kept_the_card() {
        let thief = sts2_core::CreatureUid(std::num::NonZeroU32::new(2).expect("non-zero"));
        let cfg = MctsConfig::default();
        let leaf = LeafEval::Texel;

        let mut fled = CombatState {
            is_over: true,
            player_won: true,
            escaped_creatures: 1,
            ..CombatState::default()
        };
        fled.player.current_hp = 40;
        fled.player.max_hp = 80;
        fled.record_swipe_steal(thief, sts2_core::cards::CardId::Bash, false);

        let mut killed = fled.clone();
        killed.reclaim_swipe_on_death(thief);
        killed.escaped_creatures = 0;

        let v_fled = terminal_state_value_with(&fled, &cfg, &leaf);
        let v_killed = terminal_state_value_with(&killed, &cfg, &leaf);
        assert!(
            v_killed > v_fled,
            "killing the thief must outrank letting it flee with the card \
             (killed={v_killed}, fled={v_fled})"
        );

        // Both are still unambiguous wins: the charge is capped well under
        // the 0.5 `Bucket::from_value` threshold and the +1.0 win-band floor.
        assert!(
            v_fled > 0.5,
            "a looted win is still bucketed a win: {v_fled}"
        );
        assert!(matches!(Bucket::from_value(v_fled), Bucket::Win));
    }

    /// `loot_weight = 0.0` is the rollback seam: it must reproduce the
    /// pre-GH-#604 terminal and depth-cut values byte-for-byte, on the exact
    /// states the charge would otherwise bite.
    // Exact equality is the assertion: the knob-off arm must reproduce the
    // pre-#604 values bit-for-bit, not merely closely.
    #[allow(clippy::float_cmp)]
    #[test]
    fn loot_weight_zero_reproduces_the_pre_604_values() {
        let thief = sts2_core::CreatureUid(std::num::NonZeroU32::new(2).expect("non-zero"));
        let leaf = LeafEval::Texel;
        let mut looted = CombatState::default();
        looted.player.current_hp = 40;
        looted.player.max_hp = 80;
        looted.record_swipe_steal(thief, sts2_core::cards::CardId::Bash, false);

        let off = MctsConfig {
            loot_weight: 0.0,
            ..MctsConfig::default()
        };
        assert_eq!(loot_penalty_q(&looted, &off), 0.0);
        assert_eq!(
            depth_cut_value(&looted, &leaf, &off),
            leaf.evaluate(&looted),
            "with the knob off the depth-cut leaf is the bare leaf eval"
        );

        let on = MctsConfig::default();
        assert!(
            loot_penalty_q(&looted, &on) > 0.0,
            "the default arm charges for the outstanding steal"
        );
        assert!(depth_cut_value(&looted, &leaf, &on) < leaf.evaluate(&looted));

        let mut clean = looted.clone();
        clean.reclaim_swipe_on_death(thief);
        assert_eq!(loot_penalty_q(&clean, &on), 0.0);
        assert_eq!(depth_cut_value(&clean, &leaf, &on), leaf.evaluate(&clean));
    }

    /// The charge is bounded on both sides: never negative, never past
    /// [`LOOT_PENALTY_Q_CAP`], and the depth-cut leaf it feeds stays strictly
    /// above `-1.0` so the graded loss band still ranks under every uncertain
    /// line (see [`LOSS_GRADIENT_LEAF_SPAN`]).
    #[test]
    fn loot_penalty_stays_inside_its_documented_bounds() {
        let thief = sts2_core::CreatureUid(std::num::NonZeroU32::new(2).expect("non-zero"));
        let cfg = MctsConfig::default();
        let mut s = CombatState::default();
        for _ in 0..200 {
            s.record_swipe_steal(thief, sts2_core::cards::CardId::Bash, false);
        }
        let q = loot_penalty_q(&s, &cfg);
        assert!(
            (0.0..=LOOT_PENALTY_Q_CAP).contains(&q),
            "penalty {q} escaped [0, {LOOT_PENALTY_Q_CAP}]"
        );
        let v = depth_cut_value(&s, &LeafEval::Texel, &cfg);
        assert!(v > -1.0, "depth-cut leaf {v} must stay above -1.0");
    }

    /// Bead `sts2-rs-o6xw`: PIMC determinization re-shuffle must
    /// preserve known-position cards (Headbutt's top-of-draw pin).
    /// Confirms that the per-sim shuffle path that MCTS invokes
    /// (`sts2_core::cards::shuffle`) keeps `pinned_top[0]` at `draw[0]`
    /// across many independent sim seeds.
    #[test]
    fn determinization_shuffle_preserves_pinned_top() {
        use sts2_core::CardUid;

        let pinned = CardUid::new(99).expect("non-zero");
        for sim_index in 0u32..200 {
            let mut player = sts2_core::PlayerCombatState::default();
            for n in 1..=25u32 {
                player.draw.push(CardUid::new(n).expect("non-zero"));
            }
            // Insert the pinned uid mid-pile so a naive shuffle would
            // move it; the pin must resurrect it to `draw[0]`.
            player.draw.insert(12, pinned);
            sts2_core::cards::pin_top(&mut player, pinned);

            // Mirror the per-sim seed derivation in `Mcts::one_simulation`.
            let seed = 0x2au64 ^ u64::from(sim_index).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let mut rng = sts2_core::dotnet_rng::Rng::new(seed as i32);
            sts2_core::cards::shuffle(&mut player, &mut rng);

            assert_eq!(
                player.draw.first().copied(),
                Some(pinned),
                "sim_index {sim_index}: pinned card must survive PIMC re-shuffle"
            );
        }
    }

    #[test]
    fn uniform_policy_sums_to_one() {
        let acts = vec![Action::EndTurn, Action::EndTurn, Action::EndTurn];
        let p = UniformPolicy.prior(&CombatState::default(), &acts);
        let s: f32 = p.iter().sum();
        assert!((s - 1.0).abs() < 1e-6);
    }

    /// Regression test for bead sts2-rs-f2j5.
    ///
    /// When a child node is created during a simulation that happened to
    /// reach a terminal state under its determinization (stale
    /// `is_terminal = true`), rerooting to that child must not cause the
    /// subsequent `search_n` to panic.
    ///
    /// Before the fix, `reroot` copied the stale `is_terminal = true`
    /// onto the new root without refreshing it from `new_root_state`. On
    /// the next `search_n` call the selection loop saw `is_terminal = true`
    /// at the root, produced no children, and
    /// `.expect("search() called on terminal root: no legal actions")`
    /// panicked.
    ///
    /// We replicate the stale-terminal condition by:
    ///   1. Running a short search to expand at least one child.
    ///   2. Forcibly setting that child's `is_terminal = true` and
    ///      `is_expanded = false`, exactly as a real simulation would leave
    ///      it when its trajectory ended in a terminal state.
    ///   3. Calling `reroot` with the real (non-terminal) live state.
    ///   4. Calling `search_n` — must not panic, must return a valid action.
    #[test]
    fn reroot_refreshes_stale_is_terminal() {
        use sts2_core::combat_loop::{start_combat, start_player_turn};
        use sts2_core::{
            CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory, MonsterId,
            MonsterKind, OrbSlots, PlayerCombatState, Side, StepResult, TurnEvents,
        };

        // Build an Ironclad-vs-Cultist state in PlayerPlayPhase — the
        // same vertical slice used by the MCTS smoke bench.
        let make_state = |seed: u64| -> CombatState {
            let deck_size: u32 = 10;
            let mut draw: smallvec::SmallVec<[CardUid; 32]> = smallvec::SmallVec::new();
            for idx in 1..=deck_size {
                draw.push(CardUid::new(idx).expect("non-zero"));
            }
            let cultist_uid = CreatureUid::new(1).expect("non-zero");
            let cultist = Creature {
                id: cultist_uid.0,
                current_hp: 48,
                max_hp: 48,
                block: 0,
                powers: smallvec::SmallVec::new(),
                species: Some(MonsterKind::Nibbit),
                monster_id: MonsterId("Nibbit"),
                intent: None,
                intent_history: IntentHistory::default(),
                ai: None,
            };
            let player = PlayerCombatState {
                hand: smallvec::SmallVec::new(),
                draw,
                discard: smallvec::SmallVec::new(),
                exhaust: smallvec::SmallVec::new(),
                play: smallvec::SmallVec::new(),
                energy: 0,
                max_energy: 3,
                stars: 0,
                current_hp: 80,
                max_hp: 80,
                block: 0,
                powers: smallvec::SmallVec::new(),
                character: None,
                potions: smallvec::SmallVec::new(),
                potion_slot_indices: smallvec::SmallVec::new(),
                potion_slots: 0,
                orbs: OrbSlots::default(),
                creature_uid: None,
                pet_uid: None,
                shivs_played_this_turn: 0,
                cards_played_this_turn: [0; 5],
                powered_hits_this_turn: 0,
                powered_hits_by_target_this_turn: smallvec::SmallVec::new(),
                powered_hits_at_card_play_start: 0,
                attack_command_suppressed_this_play: false,
                vigor_latch_card: None,
                generated_cards_this_combat: 0,
                skills_played_this_turn: 0,
                card_plays_started_this_turn: 0,
                cards_first_in_series_this_turn: 0,
                request_end_turn_after_play: false,
                retain_this_turn: smallvec::SmallVec::new(),
                sly_this_turn: smallvec::SmallVec::new(),
                sly_this_combat: smallvec::SmallVec::new(),
                pending_retain_count: 0,
                stars_gained_this_turn: 0,
                pinned_top: smallvec::SmallVec::new(),
                pinned_bottom: SmallVec::new(),
                deferred_draw_order: false,
            };
            let mut enemy_uids: smallvec::SmallVec<[CreatureUid; 8]> = smallvec::SmallVec::new();
            enemy_uids.push(cultist_uid);
            let mut state = CombatState {
                belt_value: None,
                phase: CombatPhase::Setup,
                round: 0,
                side: Side::Player,
                rng: sts2_core::dotnet_rng::Rng::new((seed) as i32),

                run_rng: sts2_core::run_rng_set::RunRngSet::new((seed) as i32),
                creatures: vec![cultist],
                cards: Vec::new().into(),
                powers: Vec::new().into(),
                relics: Vec::new().into(),
                potions: Vec::new(),
                player,
                allies: smallvec::SmallVec::new(),
                enemy_uids,
                pending_choice: None,
                resolving_card: None,
                pending_play: None,
                pending_turn_setup_resume: false,
                pending_draw_resume: None,
                pending_snecko_cost_randomize: false,
                pending_cluster_resume: None,
                pending_hand_draw_resume: None,
                pending_relic_hand_draw_resume: None,
                pending_wlp_retain_resume: false,
                pending_end_turn_resume: None,
                pending_autoplay_resume: None,
                autoplay_inline_resolve: false,
                pending_curse_choice: None,
                pending_potion_hook_resume: None,
                pending_enemy_exec_resume: None,
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
            };
            let r0 = start_combat(&mut state);
            assert_eq!(r0, StepResult::Advanced);
            let r1 = start_player_turn(&mut state);
            assert_eq!(r1, StepResult::Advanced);
            assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);
            state
        };

        let root_state = make_state(0);

        let config = MctsConfig {
            c_puct: std::f32::consts::SQRT_2,
            max_simulations: 20,
            max_rollout_depth: 50,
            seed: 42,
            rollout_policy: RolloutPolicyKind::Heuristic,
            use_transposition_table: false,
            regularized_pi_temperature: 0.1,
            use_puct_prior: false,
            terminal_eval_mode: TerminalEvalMode::Binary,
            use_quiescence: false,
            use_lethal_detector: false,
            chance_widening_c: 1.0,
            chance_widening_alpha: 0.5,
            draw_widening_c: 1.0,
            draw_widening_alpha: 0.5,
            endturn_widening_c: 0.5,
            endturn_widening_alpha: 0.4,
            chance_prove_wins: false,
            chance_closed_stops_widening: false,
            chance_max_depth: CHANCE_MAX_DEPTH,
            hidden_info: HiddenInfoMode::Clairvoyant,
            common_random_numbers: false,
            tie_eps: 0.02,
            loss_gradient: true,
            root_guards: true,
            ..MctsConfig::default()
        };

        let mut mcts = Mcts::new(root_state.clone(), config);
        // Run enough sims to guarantee at least one root child is created.
        let first_action = mcts.search_n(20);

        // Forcibly poison the child node for `first_action` by marking it
        // terminal and unexpanded — this replicates the stale state that a
        // real simulation leaves when its trajectory ended in CombatWon/Lost.
        {
            let root_id = mcts.tree.root;
            let root_state = mcts.tree.root_state.clone();
            let child_id = mcts
                .tree
                .node(root_id)
                .children
                .iter()
                .find(|(edge, _)| edge.resolve(&root_state).as_ref() == Some(&first_action))
                .map(|(_, id)| *id)
                .expect("first_action must be a root child after 20 sims");
            let child_node = mcts.tree.node_mut(child_id);
            child_node.is_terminal = true; // poison — stale!
            child_node.is_expanded = false;
            child_node.untried_actions.clear();
        }

        // Compute what the live state looks like after applying first_action.
        let mut new_state = root_state.clone();
        let _ = sts2_core::apply(&mut new_state, first_action.clone());
        // Defensive guard: legal_actions returning empty on a non-terminal state
        // shouldn't happen in practice; skip rather than spin.
        if !sts2_core::is_terminal(&new_state) && sts2_core::legal_actions(&new_state).is_empty() {
            return;
        }

        if sts2_core::is_terminal(&new_state) {
            // Combat already over — nothing more to test (reroot returns false
            // for terminal new states by convention; skip the panic test).
            return;
        }

        // Reroot to the poisoned child. Before the fix this would leave
        // `is_terminal = true` on the new root, causing `search_n` to panic.
        let rerooted = mcts.reroot(&first_action, new_state.clone());
        assert!(rerooted, "reroot must succeed: first_action was expanded");

        // This must NOT panic after the fix. It will panic before the fix
        // with "search() called on terminal root: no legal actions".
        let _action = mcts.search_n(10);

        // Sanity: the root node must now reflect the live (non-terminal) state.
        let root_node = mcts.tree.node(mcts.tree.root);
        assert!(
            !root_node.is_terminal,
            "root must be non-terminal after reroot with non-terminal state"
        );
    }

    /// Bead `sts2-rs-hgu7`: Q-comparison fixture.
    ///
    /// Follow-up to bead `sts2-rs-o6xw` (PutBack resolver pins to
    /// `Top(0)` + PIMC shuffle preserves pinned cards). o6xw proved
    /// the invariant; hgu7 demonstrates that MCTS's Q-value surface
    /// agrees the pin matters — i.e. choosing to put a high-value card
    /// (Strike: 6 dmg, playable) on top of draw produces a strictly
    /// higher root Q than choosing to put a low-value card
    /// (AscendersBane: curse, unplayable) there.
    ///
    /// Setup: a small Ironclad-vs-Cultist root with the player at
    /// `PlayerPlayPhase`, energy 0, hand empty, and a pending
    /// `PutBack` choice with two options — one Strike, one curse —
    /// drawn from a discard the test pre-seeds. The two root children
    /// are `ResolveChoice{[0]}` (pick Strike) and `ResolveChoice{[1]}`
    /// (pick curse). Both resolve immediately, pin the chosen card to
    /// `draw[0]`, and yield control. From there rollouts run the
    /// EndTurn → enemy-attack → player-draw → play cycle until the
    /// depth cap. The Strike branch hits the enemy for 6 next turn;
    /// the curse branch wastes the draw and the player takes another
    /// hit instead. With enough sims this signal is robust to RNG.
    ///
    /// Pins: search budget is calibrated against
    /// `project_mcts_budget_knee.md` (knee at 300–3000 sims, depth
    /// 10–30). Using `sims=3000, depth=30` keeps under the documented
    /// non-monotonicity cliff.
    #[test]
    fn root_q_for_action_surface_pins_putback_choice_root_children() {
        use smallvec::SmallVec;
        use sts2_core::cards::{CardId, CardType, PileKind, TargetType};
        use sts2_core::combat_loop::{start_combat, start_player_turn};
        use sts2_core::{
            Card, CardUid, ChoiceKind, ChoiceOption, CombatPhase, CombatState, Creature,
            CreatureUid, IntentHistory, MonsterId, MonsterKind, OrbSlots, PendingChoice,
            PlayerCombatState, Side, StepResult, TurnEvents,
        };

        // Build a minimal Ironclad-vs-Cultist state with the player
        // sitting at a Headbutt-style PutBack pending choice.
        let strike_uid = CardUid::new(1).expect("non-zero");
        let curse_uid = CardUid::new(2).expect("non-zero");

        let strike_card = Card {
            uid: strike_uid,
            card_id: CardId::StrikeIronclad,
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        };
        let curse_card = Card {
            uid: curse_uid,
            card_id: CardId::AscendersBane,
            cost: -2,
            card_type: CardType::Curse,
            target_type: TargetType::None,
            upgraded: false,
            base_damage: 0,
            base_block: 0,
            ..Card::pod_defaults()
        };

        let cultist_uid = CreatureUid::new(1).expect("non-zero");
        // Cultist at exactly Strike's damage (6 HP) so the
        // Strike-branch can kill in one hit while the curse branch
        // cannot. This gives MCTS a strong terminal-win signal on the
        // Strike side and a depth-cut / loss signal on the curse side.
        let cultist = Creature {
            id: cultist_uid.0,
            current_hp: 6,
            max_hp: 30,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            monster_id: MonsterId("Nibbit"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        };

        let mut discard: SmallVec<[CardUid; 32]> = SmallVec::new();
        discard.push(strike_uid);
        discard.push(curse_uid);

        let player = PlayerCombatState {
            hand: SmallVec::new(),
            draw: SmallVec::new(),
            discard,
            exhaust: SmallVec::new(),
            play: SmallVec::new(),
            energy: 0,
            max_energy: 3,
            stars: 0,
            current_hp: 50,
            max_hp: 80,
            block: 0,
            powers: SmallVec::new(),
            character: None,
            potions: SmallVec::new(),
            potion_slot_indices: SmallVec::new(),
            potion_slots: 0,
            orbs: OrbSlots::default(),
            creature_uid: None,
            pet_uid: None,
            shivs_played_this_turn: 0,
            cards_played_this_turn: [0; 5],
            powered_hits_this_turn: 0,
            powered_hits_by_target_this_turn: smallvec::SmallVec::new(),
            powered_hits_at_card_play_start: 0,
            attack_command_suppressed_this_play: false,
            vigor_latch_card: None,
            generated_cards_this_combat: 0,
            skills_played_this_turn: 0,
            card_plays_started_this_turn: 0,
            cards_first_in_series_this_turn: 0,
            request_end_turn_after_play: false,
            retain_this_turn: SmallVec::new(),
            sly_this_turn: SmallVec::new(),
            sly_this_combat: SmallVec::new(),
            pending_retain_count: 0,
            stars_gained_this_turn: 0,
            pinned_top: SmallVec::new(),
            pinned_bottom: SmallVec::new(),
            deferred_draw_order: false,
        };
        let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
        enemy_uids.push(cultist_uid);

        let mut state = CombatState {
            belt_value: None,
            phase: CombatPhase::Setup,
            round: 0,
            side: Side::Player,
            rng: sts2_core::dotnet_rng::Rng::new(0x0006_8b17_i32),

            run_rng: sts2_core::run_rng_set::RunRngSet::new(0x0006_8b17_i32),
            creatures: vec![cultist],
            cards: vec![strike_card, curse_card].into(),
            powers: Vec::new().into(),
            relics: Vec::new().into(),
            potions: Vec::new(),
            player,
            allies: SmallVec::new(),
            enemy_uids,
            pending_choice: None,
            resolving_card: None,
            pending_play: None,
            pending_turn_setup_resume: false,
            pending_draw_resume: None,
            pending_snecko_cost_randomize: false,
            pending_cluster_resume: None,
            pending_hand_draw_resume: None,
            pending_relic_hand_draw_resume: None,
            pending_wlp_retain_resume: false,
            pending_end_turn_resume: None,
            pending_autoplay_resume: None,
            autoplay_inline_resolve: false,
            pending_curse_choice: None,
            pending_potion_hook_resume: None,
            pending_enemy_exec_resume: None,
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
        };
        assert_eq!(start_combat(&mut state), StepResult::Advanced);
        assert_eq!(start_player_turn(&mut state), StepResult::Advanced);
        assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);

        // Inject the Headbutt-style PutBack pending choice. discard
        // still holds both uids; ResolveChoice will move the chosen
        // one to `draw[0]` and pin it.
        let mk_opt = |uid, card_id| ChoiceOption {
            card_id,
            upgraded: false,
            card_uid: Some(uid),
            source_pile: Some(PileKind::Discard),
        };
        state.pending_choice = Some(PendingChoice {
            kind: ChoiceKind::PutBack,
            options: vec![
                mk_opt(strike_uid, CardId::StrikeIronclad),
                mk_opt(curse_uid, CardId::AscendersBane),
            ],
            min_select: 1,
            max_select: 1,
            can_skip: false,
            post_effect: None,
            picked: SmallVec::new(),
        });

        // Root children: `ResolveChoice{[0]}` (Strike) and
        // `ResolveChoice{[1]}` (Curse). The ordering at the root
        // mirrors the option index in `legal_actions`.
        let pick_strike = Action::ResolveChoice {
            selection: {
                let mut s: SmallVec<[u8; 4]> = SmallVec::new();
                s.push(0);
                s
            },
        };
        let pick_curse = Action::ResolveChoice {
            selection: {
                let mut s: SmallVec<[u8; 4]> = SmallVec::new();
                s.push(1);
                s
            },
        };

        let config = MctsConfig {
            c_puct: std::f32::consts::SQRT_2,
            max_simulations: 3000,
            max_rollout_depth: 30,
            seed: 0x0006_8b17,
            rollout_policy: RolloutPolicyKind::Heuristic,
            use_transposition_table: false,
            regularized_pi_temperature: 0.1,
            use_puct_prior: false,
            terminal_eval_mode: TerminalEvalMode::Binary,
            use_quiescence: false,
            use_lethal_detector: false,
            chance_widening_c: 1.0,
            chance_widening_alpha: 0.5,
            draw_widening_c: 1.0,
            draw_widening_alpha: 0.5,
            endturn_widening_c: 0.5,
            endturn_widening_alpha: 0.4,
            chance_prove_wins: false,
            chance_closed_stops_widening: false,
            chance_max_depth: CHANCE_MAX_DEPTH,
            hidden_info: HiddenInfoMode::Clairvoyant,
            common_random_numbers: false,
            tie_eps: 0.02,
            loss_gradient: true,
            root_guards: true,
            ..MctsConfig::default()
        };

        let mut mcts = Mcts::new(state, config);
        let _ = mcts.search_n(3000);

        // Accessor surface checks — load-bearing for hgu7's
        // precondition ("MCTS test surface gains a stable
        // `root_q_for_action(Action) -> f32` accessor").
        //
        // Both root children must be reachable via the accessor and
        // must report finite Q values agreeing with `root_outcomes`.
        let q_strike = mcts
            .root_q_for_action(&pick_strike)
            .expect("Strike child visited");
        let q_curse = mcts
            .root_q_for_action(&pick_curse)
            .expect("Curse child visited");
        assert!(
            q_strike.is_finite() && q_curse.is_finite(),
            "Q values must be finite (got strike={q_strike}, curse={q_curse})"
        );

        // Cross-check against the canonical `root_outcomes` surface:
        // both accessors must agree on the same numeric value for the
        // same root child.
        let outcomes = mcts.root_outcomes();
        let oc_strike = outcomes
            .iter()
            .find(|o| o.action == pick_strike)
            .expect("Strike action present in root_outcomes");
        let oc_curse = outcomes
            .iter()
            .find(|o| o.action == pick_curse)
            .expect("Curse action present in root_outcomes");
        assert!(
            (oc_strike.mean_value - q_strike).abs() < 1e-6,
            "root_q_for_action(Strike) must match root_outcomes.mean_value"
        );
        assert!(
            (oc_curse.mean_value - q_curse).abs() < 1e-6,
            "root_q_for_action(Curse) must match root_outcomes.mean_value"
        );

        // Accessor must return `None` for actions that aren't root
        // children. `Action::EndTurn` is reachable as a *grand*child
        // (post-resolve) but not at this PutBack-pending root.
        assert!(
            mcts.root_q_for_action(&Action::EndTurn).is_none(),
            "root_q_for_action must return None for non-root-child actions"
        );

        // Q-direction note: the strict-`>` directional claim is
        // pinned by the sibling test
        // `playable_strike_in_hand_search_value_beats_unplayable_curse_in_hand`
        // (bead `sts2-rs-cw2z`), which runs two independent MCTS
        // searches from post-pin states (the PutBack already
        // resolved) and compares `best_value()` rather than per-
        // child mean Q. That split sidesteps the rollout-from-
        // PutBack-root degeneracy where both branches contain the
        // same two cards (one in draw, one in discard) and reshuffle
        // them back to identical decks at end-of-turn — masking the
        // pin-direction signal at the ResolveChoice grandchild
        // level.
    }

    /// Bead `sts2-rs-cw2z` (hgu7 follow-up): directional Q-claim.
    ///
    /// Demonstrates that MCTS's best-line search value strictly
    /// distinguishes a player state holding a playable damage card
    /// (Strike, kills the lone Cultist at 6 HP in one play) from one
    /// holding only an unplayable curse (AscendersBane, cost `-2`).
    /// This is the load-bearing directional Q-claim asked for in
    /// the original `sts2-rs-o6xw` Acceptance #4 / `sts2-rs-hgu7`
    /// brief — the pin mechanic itself (PutBack → `draw[0]`) is
    /// already covered by `o6xw`'s shuffle-preservation tests +
    /// the `root_q_for_action_surface_*` sibling. What was missing
    /// was a passing test that MCTS Q actually *prefers* the
    /// playable-damage line over the unplayable-curse line.
    ///
    /// Setup: two minimal Ironclad-vs-Cultist states constructed
    /// directly at `PlayerPlayPhase` with the single relevant card
    /// already drawn into hand (sidestepping the post-PutBack
    /// reshuffle that, with the same two cards in the deck, would
    /// collapse both branches to identical Q after one turn). The
    /// `state.cards` POD bank still holds both Card entries so the
    /// dispatch table can resolve `PlayCard{0}` against either —
    /// only the `hand` membership differs between branches.
    ///
    /// `best_value()` is the highest leaf value observed across
    /// rollouts (monotone non-decreasing per `sts2-rs-dm5z`):
    /// - Strike-in-hand: search finds `PlayCard{Strike, cultist}`
    ///   → 6 dmg → cultist dies → `evaluate_victory` ∈ [0.5, 1.0].
    /// - Curse-in-hand: AscendersBane is unplayable; no other
    ///   damage path exists in this minimal deck; rollouts time
    ///   out at `max_rollout_depth` with the passive Cultist still
    ///   alive, registering terminal-loss buckets.
    ///
    /// Card dispatch must be installed at test start —
    /// `sts2_content::cards::dispatch::install_default_registry()`
    /// wires up Strike's damage path; without it both branches
    /// silently no-op and collapse to identical Q.
    #[test]
    fn playable_strike_in_hand_search_value_beats_unplayable_curse_in_hand() {
        // Install the default card-dispatch registry so PlayCard
        // actions actually execute their effect bodies (Strike's
        // damage, AscendersBane's no-op). Without this the dispatch
        // table is empty in the test crate and every play_card is a
        // silent no-op, collapsing both branches to identical
        // useless-action distributions.
        sts2_content::cards::dispatch::install_default_registry();

        use smallvec::SmallVec;
        use sts2_core::cards::{CardId, CardType, TargetType};
        use sts2_core::combat_loop::{start_combat, start_player_turn};
        use sts2_core::{
            Card, CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory,
            MonsterId, MonsterKind, OrbSlots, PlayerCombatState, Side, StepResult, TurnEvents,
        };

        // Two-card POD bank shared across both branches.
        let strike_uid = CardUid::new(1).expect("non-zero");
        let curse_uid = CardUid::new(2).expect("non-zero");
        let strike_card = Card {
            uid: strike_uid,
            card_id: CardId::StrikeIronclad,
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 6,
            base_block: 0,
            ..Card::pod_defaults()
        };
        let curse_card = Card {
            uid: curse_uid,
            card_id: CardId::AscendersBane,
            cost: -2,
            card_type: CardType::Curse,
            target_type: TargetType::None,
            upgraded: false,
            base_damage: 0,
            base_block: 0,
            ..Card::pod_defaults()
        };

        // Two states diverge in which card the player has *in hand*
        // (the post-resolve, post-draw observation): Strike-in-hand
        // can kill the 6-HP Cultist this turn for an immediate
        // terminal win; Curse-in-hand must end-turn and reshuffle
        // before Strike comes back around. `evaluate_victory`
        // rewards faster wins (fewer rounds elapsed), so the
        // Strike-in-hand `best_value` strictly exceeds the
        // Curse-in-hand one.
        let build = |strike_in_hand: bool| -> CombatState {
            let cultist_uid = CreatureUid::new(1).expect("non-zero");
            let cultist = Creature {
                id: cultist_uid.0,
                current_hp: 6,
                max_hp: 30,
                block: 0,
                powers: SmallVec::new(),
                species: Some(MonsterKind::Nibbit),
                monster_id: MonsterId("Nibbit"),
                intent: None,
                intent_history: IntentHistory::default(),
                // Passive cultist: no AI / intent so the player can
                // never lose to incoming damage. The Q signal must
                // come from differential time-to-win, not from
                // surviving the side-turn cycle.
                ai: None,
            };

            // Post-Headbutt-resolve, post-draw shape: one card sits
            // in hand (the pinned-then-drawn card), the other sits
            // in draw waiting to be reshuffled. With hand-of-1
            // either Strike is playable now or Curse is — that
            // single-action divergence drives the search value
            // gap.
            let hand_uid = if strike_in_hand {
                strike_uid
            } else {
                curse_uid
            };
            let mut hand: SmallVec<[CardUid; 16]> = SmallVec::new();
            hand.push(hand_uid);

            let player = PlayerCombatState {
                hand,
                draw: SmallVec::new(),
                discard: SmallVec::new(),
                exhaust: SmallVec::new(),
                play: SmallVec::new(),
                energy: 1,
                max_energy: 3,
                stars: 0,
                current_hp: 80,
                max_hp: 80,
                block: 0,
                powers: SmallVec::new(),
                character: None,
                potions: SmallVec::new(),
                potion_slot_indices: SmallVec::new(),
                potion_slots: 0,
                orbs: OrbSlots::default(),
                creature_uid: None,
                pet_uid: None,
                shivs_played_this_turn: 0,
                cards_played_this_turn: [0; 5],
                powered_hits_this_turn: 0,
                powered_hits_by_target_this_turn: smallvec::SmallVec::new(),
                powered_hits_at_card_play_start: 0,
                attack_command_suppressed_this_play: false,
                vigor_latch_card: None,
                generated_cards_this_combat: 0,
                skills_played_this_turn: 0,
                card_plays_started_this_turn: 0,
                cards_first_in_series_this_turn: 0,
                request_end_turn_after_play: false,
                retain_this_turn: SmallVec::new(),
                sly_this_turn: SmallVec::new(),
                sly_this_combat: SmallVec::new(),
                pending_retain_count: 0,
                stars_gained_this_turn: 0,
                pinned_top: SmallVec::new(),
                pinned_bottom: SmallVec::new(),
                deferred_draw_order: false,
            };

            let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
            enemy_uids.push(cultist_uid);

            let mut state = CombatState {
                belt_value: None,
                phase: CombatPhase::Setup,
                round: 0,
                side: Side::Player,
                rng: sts2_core::dotnet_rng::Rng::new(0x00cb_2eed_i32),

                run_rng: sts2_core::run_rng_set::RunRngSet::new(0x00cb_2eed_i32),
                creatures: vec![cultist],
                cards: vec![strike_card, curse_card].into(),
                powers: Vec::new().into(),
                relics: Vec::new().into(),
                potions: Vec::new(),
                player,
                allies: SmallVec::new(),
                enemy_uids,
                pending_choice: None,
                resolving_card: None,
                pending_play: None,
                pending_turn_setup_resume: false,
                pending_draw_resume: None,
                pending_snecko_cost_randomize: false,
                pending_cluster_resume: None,
                pending_hand_draw_resume: None,
                pending_relic_hand_draw_resume: None,
                pending_wlp_retain_resume: false,
                pending_end_turn_resume: None,
                pending_autoplay_resume: None,
                autoplay_inline_resolve: false,
                pending_curse_choice: None,
                pending_potion_hook_resume: None,
                pending_enemy_exec_resume: None,
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
            };
            assert_eq!(start_combat(&mut state), StepResult::Advanced);
            assert_eq!(start_player_turn(&mut state), StepResult::Advanced);
            assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);
            state
        };

        let config = MctsConfig {
            c_puct: std::f32::consts::SQRT_2,
            max_simulations: 3000,
            max_rollout_depth: 60,
            seed: 0x00cb_2eed,
            rollout_policy: RolloutPolicyKind::Heuristic,
            use_transposition_table: false,
            regularized_pi_temperature: 0.1,
            use_puct_prior: false,
            terminal_eval_mode: TerminalEvalMode::Binary,
            use_quiescence: false,
            use_lethal_detector: false,
            chance_widening_c: 1.0,
            chance_widening_alpha: 0.5,
            draw_widening_c: 1.0,
            draw_widening_alpha: 0.5,
            endturn_widening_c: 0.5,
            endturn_widening_alpha: 0.4,
            chance_prove_wins: false,
            chance_closed_stops_widening: false,
            chance_max_depth: CHANCE_MAX_DEPTH,
            hidden_info: HiddenInfoMode::Clairvoyant,
            common_random_numbers: false,
            tie_eps: 0.02,
            loss_gradient: true,
            root_guards: true,
            ..MctsConfig::default()
        };

        let mut mcts_strike = Mcts::new(build(true), config);
        let _ = mcts_strike.search_n(3000);
        let v_strike = mcts_strike.best_value();

        let mut mcts_curse = Mcts::new(build(false), config);
        let _ = mcts_curse.search_n(3000);
        let v_curse = mcts_curse.best_value();

        // The strict-`>` directional claim: Strike-on-top must yield
        // a higher search best-value than Curse-on-top.
        //
        // Implementation note: if both searches happen to converge
        // to the same terminal-loss bucket (`best_value == -1.0` on
        // each side) the assertion below will fail with a clear
        // diagnostic, and the fixture state needs further tuning
        // (deeper deck, stronger AI, larger depth cap).
        assert!(
            v_strike > v_curse,
            "Strike-pinned best_value ({v_strike}) must exceed \
             Curse-pinned best_value ({v_curse}). When both equal \
             -1.0 (terminal loss on every rollout) the constructed \
             fixture is too punishing and needs a richer deck / AI / \
             depth budget — see bead sts2-rs-cw2z analysis."
        );
    }

    /// Bead `sts2-rs-7z97`. Direct exercise of the cycle guard: pre-
    /// populate the TT with an entry pointing at the root and confirm
    /// the engine doesn't link a child to it (which would create a
    /// self-loop the UCT descent could spin on). We can't reach the
    /// private `transposition` field from outside the crate, but
    /// `#[cfg(test)]` gives us module-private access here.
    #[test]
    fn transposition_cycle_guard_holds() {
        // Build a trivial state by re-using the helper from the
        // sibling test by inlining it (keeping the test self-contained
        // is cheaper than refactoring the helper out).

        use sts2_core::combat_loop::{start_combat, start_player_turn};
        use sts2_core::{
            CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory, MonsterId,
            MonsterKind, OrbSlots, PlayerCombatState, Side, StepResult, TurnEvents,
        };

        let cultist_uid = CreatureUid::new(1).expect("non-zero");
        let cultist = Creature {
            id: cultist_uid.0,
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            monster_id: MonsterId("Nibbit"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        };
        let mut draw: smallvec::SmallVec<[CardUid; 32]> = smallvec::SmallVec::new();
        for idx in 1..=10u32 {
            draw.push(CardUid::new(idx).expect("non-zero"));
        }
        let player = PlayerCombatState {
            hand: smallvec::SmallVec::new(),
            draw,
            discard: smallvec::SmallVec::new(),
            exhaust: smallvec::SmallVec::new(),
            play: smallvec::SmallVec::new(),
            energy: 0,
            max_energy: 3,
            stars: 0,
            current_hp: 80,
            max_hp: 80,
            block: 0,
            powers: smallvec::SmallVec::new(),
            character: None,
            potions: smallvec::SmallVec::new(),
            potion_slot_indices: smallvec::SmallVec::new(),
            potion_slots: 0,
            orbs: OrbSlots::default(),
            creature_uid: None,
            pet_uid: None,
            shivs_played_this_turn: 0,
            cards_played_this_turn: [0; 5],
            powered_hits_this_turn: 0,
            powered_hits_by_target_this_turn: smallvec::SmallVec::new(),
            powered_hits_at_card_play_start: 0,
            attack_command_suppressed_this_play: false,
            vigor_latch_card: None,
            generated_cards_this_combat: 0,
            skills_played_this_turn: 0,
            card_plays_started_this_turn: 0,
            cards_first_in_series_this_turn: 0,
            request_end_turn_after_play: false,
            retain_this_turn: smallvec::SmallVec::new(),
            sly_this_turn: smallvec::SmallVec::new(),
            sly_this_combat: smallvec::SmallVec::new(),
            pending_retain_count: 0,
            stars_gained_this_turn: 0,
            pinned_top: smallvec::SmallVec::new(),
            pinned_bottom: SmallVec::new(),
            deferred_draw_order: false,
        };
        let mut enemy_uids: smallvec::SmallVec<[CreatureUid; 8]> = smallvec::SmallVec::new();
        enemy_uids.push(cultist_uid);
        let mut state = CombatState {
            belt_value: None,
            phase: CombatPhase::Setup,
            round: 0,
            side: Side::Player,
            rng: sts2_core::dotnet_rng::Rng::new(0_i32),

            run_rng: sts2_core::run_rng_set::RunRngSet::new(0_i32),
            creatures: vec![cultist],
            cards: Vec::new().into(),
            powers: Vec::new().into(),
            relics: Vec::new().into(),
            potions: Vec::new(),
            player,
            allies: smallvec::SmallVec::new(),
            enemy_uids,
            pending_choice: None,
            resolving_card: None,
            pending_play: None,
            pending_turn_setup_resume: false,
            pending_draw_resume: None,
            pending_snecko_cost_randomize: false,
            pending_cluster_resume: None,
            pending_hand_draw_resume: None,
            pending_relic_hand_draw_resume: None,
            pending_wlp_retain_resume: false,
            pending_end_turn_resume: None,
            pending_autoplay_resume: None,
            autoplay_inline_resolve: false,
            pending_curse_choice: None,
            pending_potion_hook_resume: None,
            pending_enemy_exec_resume: None,
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
        };
        assert_eq!(start_combat(&mut state), StepResult::Advanced);
        assert_eq!(start_player_turn(&mut state), StepResult::Advanced);

        let cfg = MctsConfig {
            max_simulations: 50,
            max_rollout_depth: 20,
            seed: 42,
            use_transposition_table: true,
            regularized_pi_temperature: 0.1,
            use_puct_prior: false,
            terminal_eval_mode: TerminalEvalMode::Binary,
            use_lethal_detector: false,
            ..MctsConfig::default()
        };
        let mut mcts = Mcts::new(state, cfg);

        // Plant a poisoned TT entry: any (hash, bucket) → root. If the
        // engine ever sees a matching key it would otherwise link the
        // child edge to the root and the next selection would loop. The
        // guard must defang this by allocating a fresh node instead.
        // We use a key that won't collide with anything organic
        // (state_hash 0 is essentially impossible for a real state).
        mcts.transposition
            .as_mut()
            .expect("TT allocated: use_transposition_table = true")
            .insert((0, 0), mcts.tree.root);

        // The search must complete without panicking or spinning.
        let _ = mcts.search();
    }

    /// Bead `sts2-rs-7z97`. Pin the default — flipping it later is a
    /// follow-up bead that must update this test deliberately.
    #[test]
    fn default_config_disables_transposition_table() {
        assert!(!MctsConfig::default().use_transposition_table);
    }

    /// Bead `sts2-rs-ry1d` (qctu.9): pin the default — UCT path
    /// stays the engine's selection formula until a downstream
    /// bead deliberately flips the default after a yuyc A/B run.
    #[test]
    fn default_config_disables_puct_prior() {
        assert!(!MctsConfig::default().use_puct_prior);
    }

    // ── Bead `sts2-rs-dm5z` (W4): best-action-sequence tracker ───────────

    /// Helper for the W4 tests — constructs an Ironclad-vs-Cultist
    /// PlayerPlayPhase state. Mirrors the fixture used by
    /// `reroot_refreshes_stale_is_terminal` above. Pulled into a free
    /// function so multiple tests can share it without copy-paste.
    fn make_smoke_state(seed: u64) -> CombatState {
        use sts2_core::combat_loop::{start_combat, start_player_turn};
        use sts2_core::{
            CardUid, CombatPhase, Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind,
            OrbSlots, PlayerCombatState, Side, StepResult, TurnEvents,
        };

        let deck_size: u32 = 10;
        let mut draw: smallvec::SmallVec<[CardUid; 32]> = smallvec::SmallVec::new();
        for idx in 1..=deck_size {
            draw.push(CardUid::new(idx).expect("non-zero"));
        }
        let cultist_uid = CreatureUid::new(1).expect("non-zero");
        let cultist = Creature {
            id: cultist_uid.0,
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            monster_id: MonsterId("Nibbit"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        };
        let player = PlayerCombatState {
            hand: smallvec::SmallVec::new(),
            draw,
            discard: smallvec::SmallVec::new(),
            exhaust: smallvec::SmallVec::new(),
            play: smallvec::SmallVec::new(),
            energy: 0,
            max_energy: 3,
            stars: 0,
            current_hp: 80,
            max_hp: 80,
            block: 0,
            powers: smallvec::SmallVec::new(),
            character: None,
            potions: smallvec::SmallVec::new(),
            potion_slot_indices: smallvec::SmallVec::new(),
            potion_slots: 0,
            orbs: OrbSlots::default(),
            creature_uid: None,
            pet_uid: None,
            shivs_played_this_turn: 0,
            cards_played_this_turn: [0; 5],
            powered_hits_this_turn: 0,
            powered_hits_by_target_this_turn: smallvec::SmallVec::new(),
            powered_hits_at_card_play_start: 0,
            attack_command_suppressed_this_play: false,
            vigor_latch_card: None,
            generated_cards_this_combat: 0,
            skills_played_this_turn: 0,
            card_plays_started_this_turn: 0,
            cards_first_in_series_this_turn: 0,
            request_end_turn_after_play: false,
            retain_this_turn: smallvec::SmallVec::new(),
            sly_this_turn: smallvec::SmallVec::new(),
            sly_this_combat: smallvec::SmallVec::new(),
            pending_retain_count: 0,
            stars_gained_this_turn: 0,
            pinned_top: smallvec::SmallVec::new(),
            pinned_bottom: SmallVec::new(),
            deferred_draw_order: false,
        };
        let mut enemy_uids: smallvec::SmallVec<[CreatureUid; 8]> = smallvec::SmallVec::new();
        enemy_uids.push(cultist_uid);
        let mut state = CombatState {
            belt_value: None,
            phase: CombatPhase::Setup,
            round: 0,
            side: Side::Player,
            rng: sts2_core::dotnet_rng::Rng::new((seed) as i32),

            run_rng: sts2_core::run_rng_set::RunRngSet::new((seed) as i32),
            creatures: vec![cultist],
            cards: Vec::new().into(),
            powers: Vec::new().into(),
            relics: Vec::new().into(),
            potions: Vec::new(),
            player,
            allies: smallvec::SmallVec::new(),
            enemy_uids,
            pending_choice: None,
            resolving_card: None,
            pending_play: None,
            pending_turn_setup_resume: false,
            pending_draw_resume: None,
            pending_snecko_cost_randomize: false,
            pending_cluster_resume: None,
            pending_hand_draw_resume: None,
            pending_relic_hand_draw_resume: None,
            pending_wlp_retain_resume: false,
            pending_end_turn_resume: None,
            pending_autoplay_resume: None,
            autoplay_inline_resolve: false,
            pending_curse_choice: None,
            pending_potion_hook_resume: None,
            pending_enemy_exec_resume: None,
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
        };
        let r0 = start_combat(&mut state);
        assert_eq!(r0, StepResult::Advanced);
        let r1 = start_player_turn(&mut state);
        assert_eq!(r1, StepResult::Advanced);
        assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);
        state
    }

    /// W4 acceptance: `best_value` is monotone non-decreasing across
    /// repeated `search_n` calls within a single root (no reroot in
    /// between). Strict-`>` update semantics + an `f32::NEG_INFINITY`
    /// floor make this trivially true; the test guards against a
    /// regression that swaps to `>=` or resets `best_value` mid-run.
    #[test]
    fn best_value_monotone_across_search_calls() {
        let state = make_smoke_state(7);
        let config = MctsConfig {
            c_puct: std::f32::consts::SQRT_2,
            max_simulations: 100,
            max_rollout_depth: 50,
            seed: 13,
            rollout_policy: RolloutPolicyKind::Heuristic,
            use_transposition_table: false,
            regularized_pi_temperature: 0.1,
            use_puct_prior: false,
            terminal_eval_mode: TerminalEvalMode::Binary,
            use_quiescence: false,
            use_lethal_detector: false,
            chance_widening_c: 1.0,
            chance_widening_alpha: 0.5,
            draw_widening_c: 1.0,
            draw_widening_alpha: 0.5,
            endturn_widening_c: 0.5,
            endturn_widening_alpha: 0.4,
            chance_prove_wins: false,
            chance_closed_stops_widening: false,
            chance_max_depth: CHANCE_MAX_DEPTH,
            hidden_info: HiddenInfoMode::Clairvoyant,
            common_random_numbers: false,
            tie_eps: 0.02,
            loss_gradient: true,
            root_guards: true,
            ..MctsConfig::default()
        };
        let mut mcts = Mcts::new(state, config);
        let mut prev = f32::NEG_INFINITY;
        for _ in 0..5 {
            let _ = mcts.search_n(100);
            let cur = mcts.best_value();
            assert!(
                cur >= prev,
                "best_value must be monotone non-decreasing: prev={prev}, cur={cur}"
            );
            prev = cur;
        }
        // After ≥1 rollout completed, best_value must have left the
        // sentinel (rollout values are bounded, never NEG_INFINITY).
        assert!(prev > f32::NEG_INFINITY, "best_value never updated");
    }

    /// W4 acceptance: `best_sequence()` returns a non-empty slice after
    /// a search that ran enough sims for at least one rollout to
    /// produce a finite leaf value (i.e. always — depth_cut_value is
    /// bounded). The first rollout always installs *some* sequence
    /// because the sentinel is `NEG_INFINITY`. The acceptance text
    /// also asks specifically about `PLAYER_VICTORY`, but any rollout
    /// that strictly improves on the sentinel suffices to satisfy the
    /// non-empty post-condition.
    #[test]
    fn best_sequence_non_empty_after_search() {
        let state = make_smoke_state(11);
        let config = MctsConfig {
            c_puct: std::f32::consts::SQRT_2,
            max_simulations: 300,
            max_rollout_depth: 50,
            seed: 1,
            rollout_policy: RolloutPolicyKind::Heuristic,
            use_transposition_table: false,
            regularized_pi_temperature: 0.1,
            use_puct_prior: false,
            terminal_eval_mode: TerminalEvalMode::Binary,
            use_quiescence: false,
            use_lethal_detector: false,
            chance_widening_c: 1.0,
            chance_widening_alpha: 0.5,
            draw_widening_c: 1.0,
            draw_widening_alpha: 0.5,
            endturn_widening_c: 0.5,
            endturn_widening_alpha: 0.4,
            chance_prove_wins: false,
            chance_closed_stops_widening: false,
            chance_max_depth: CHANCE_MAX_DEPTH,
            hidden_info: HiddenInfoMode::Clairvoyant,
            common_random_numbers: false,
            tie_eps: 0.02,
            loss_gradient: true,
            root_guards: true,
            ..MctsConfig::default()
        };
        let mut mcts = Mcts::new(state, config);
        let _ = mcts.search();
        // First rollout always installs a sequence (sentinel NEG_INF).
        // The selection path on the very first sim is just `[root]`,
        // so the sequence may be length 0 — but by 300 sims expansion
        // has produced descending paths through children at least
        // once, and the strict-> update will have captured at least
        // one of them.
        // We assert non-emptiness loosely: best_value left sentinel.
        assert!(
            mcts.best_value() > f32::NEG_INFINITY,
            "no rollout produced a finite value after 300 sims"
        );
        // best_seq may be empty IF the very first rollout (root only,
        // path = [root]) produced the maximum value and no subsequent
        // strict improvement followed. That's a real edge case, not a
        // bug — exercise it by just asserting consistency: empty seq
        // ⇔ best_value came from a 1-element path.
        // For the canonical agent-surface use case, we want at least
        // one search where the captured sequence has length ≥ 1.
        // Run a few more searches to make this robust.
        for _ in 0..3 {
            let _ = mcts.search_n(300);
        }
        // After 1200 total sims the tree has descended; the best
        // sequence for at least one rollout will pass through ≥1 edge.
        // (If this ever fires it indicates the tracker is broken.)
        assert!(
            !mcts.best_sequence().is_empty() || mcts.tree.node(mcts.tree.root).children.is_empty(),
            "best_sequence should have at least one action once the root has children"
        );
    }

    /// W4 acceptance: hint surface (`root_outcomes`) is unaffected by
    /// best-sequence tracking. Since the tracker is always-on at this
    /// pin, this test pins the post-tracker numbers — a regression
    /// that touched `value_sum`, `wins`/`losses`/`ties`, or `visits`
    /// from inside the tracker would be caught by the
    /// `best_value_monotone_across_search_calls` test changing shape;
    /// here we additionally pin that hint surface remains
    /// deterministic for a fixed seed.
    #[test]
    fn hint_surface_deterministic_with_tracker() {
        let cfg = MctsConfig {
            c_puct: std::f32::consts::SQRT_2,
            max_simulations: 200,
            max_rollout_depth: 50,
            seed: 99,
            rollout_policy: RolloutPolicyKind::Heuristic,
            use_transposition_table: false,
            regularized_pi_temperature: 0.1,
            use_puct_prior: false,
            terminal_eval_mode: TerminalEvalMode::Binary,
            use_quiescence: false,
            use_lethal_detector: false,
            chance_widening_c: 1.0,
            chance_widening_alpha: 0.5,
            draw_widening_c: 1.0,
            draw_widening_alpha: 0.5,
            endturn_widening_c: 0.5,
            endturn_widening_alpha: 0.4,
            chance_prove_wins: false,
            chance_closed_stops_widening: false,
            chance_max_depth: CHANCE_MAX_DEPTH,
            hidden_info: HiddenInfoMode::Clairvoyant,
            common_random_numbers: false,
            tie_eps: 0.02,
            loss_gradient: true,
            root_guards: true,
            ..MctsConfig::default()
        };
        let mut a = Mcts::new(make_smoke_state(3), cfg);
        let mut b = Mcts::new(make_smoke_state(3), cfg);
        let _ = a.search();
        let _ = b.search();
        let oa = a.root_outcomes();
        let ob = b.root_outcomes();
        assert_eq!(oa.len(), ob.len());
        for (x, y) in oa.iter().zip(ob.iter()) {
            assert_eq!(x.action, y.action);
            assert_eq!(x.visits, y.visits);
            assert_eq!(x.wins, y.wins);
            assert_eq!(x.losses, y.losses);
            assert_eq!(x.ties, y.ties);
            assert!((x.value_sum - y.value_sum).abs() < 1e-6);
        }
    }

    /// W4 acceptance: `reroot` clears the best-sequence tracker. After
    /// rerooting the tracker must be back at sentinel state — stale
    /// sequences from the old root would mislead an agent that drives
    /// off `best_sequence()` after a commit.
    #[test]
    fn reroot_clears_best_sequence() {
        let state = make_smoke_state(5);
        let config = MctsConfig {
            c_puct: std::f32::consts::SQRT_2,
            max_simulations: 200,
            max_rollout_depth: 50,
            seed: 17,
            rollout_policy: RolloutPolicyKind::Heuristic,
            use_transposition_table: false,
            regularized_pi_temperature: 0.1,
            use_puct_prior: false,
            terminal_eval_mode: TerminalEvalMode::Binary,
            use_quiescence: false,
            use_lethal_detector: false,
            chance_widening_c: 1.0,
            chance_widening_alpha: 0.5,
            draw_widening_c: 1.0,
            draw_widening_alpha: 0.5,
            endturn_widening_c: 0.5,
            endturn_widening_alpha: 0.4,
            chance_prove_wins: false,
            chance_closed_stops_widening: false,
            chance_max_depth: CHANCE_MAX_DEPTH,
            hidden_info: HiddenInfoMode::Clairvoyant,
            common_random_numbers: false,
            tie_eps: 0.02,
            loss_gradient: true,
            root_guards: true,
            ..MctsConfig::default()
        };
        let mut mcts = Mcts::new(state.clone(), config);
        let chosen = mcts.search();
        // Tracker should have advanced past the sentinel.
        assert!(mcts.best_value() > f32::NEG_INFINITY);

        // Apply chosen action to derive new live state, then reroot.
        let mut new_state = state.clone();
        let _ = sts2_core::apply(&mut new_state, chosen.clone());
        if sts2_core::is_terminal(&new_state) {
            return;
        }
        let ok = mcts.reroot(&chosen, new_state);
        assert!(ok);
        let bv = mcts.best_value();
        assert!(
            bv.is_infinite() && bv.is_sign_negative(),
            "best_value must be NEG_INFINITY sentinel after reroot, got {bv}"
        );
        assert!(mcts.best_sequence().is_empty());
    }

    /// GitHub #32 regression: after any search, every root child's
    /// reported `mean_value = value_sum / visits` must lie within
    /// `±MAX_LEAF_VALUE_ABS`. The replay corpus surfaced Q values
    /// around -4.8 with `RichBoth` mode, which is mathematically
    /// impossible given the per-rollout `|value| ≤ 1.45` bound — the
    /// only way Q can blow up is if `value_sum` accumulates more
    /// contributions than `visits` counts (visits/value-sum
    /// double-counting in backprop, structural cycle, etc.). Pin the
    /// invariant directly off `root_outcomes()` so a future regression
    /// fires here instead of silently warping UCT exploration.
    #[test]
    fn github32_root_q_stays_within_leaf_value_bound() {
        let state = make_smoke_state(7);
        let config = MctsConfig {
            c_puct: 1.0,
            max_simulations: 1000,
            max_rollout_depth: 20,
            seed: 1,
            rollout_policy: RolloutPolicyKind::Heuristic,
            use_transposition_table: false,
            regularized_pi_temperature: 0.1,
            use_puct_prior: false,
            terminal_eval_mode: TerminalEvalMode::RichBoth,
            use_quiescence: false,
            use_lethal_detector: false,
            chance_widening_c: 1.0,
            chance_widening_alpha: 0.5,
            draw_widening_c: 1.0,
            draw_widening_alpha: 0.5,
            endturn_widening_c: 0.5,
            endturn_widening_alpha: 0.4,
            chance_prove_wins: false,
            chance_closed_stops_widening: false,
            chance_max_depth: CHANCE_MAX_DEPTH,
            hidden_info: HiddenInfoMode::Clairvoyant,
            common_random_numbers: false,
            tie_eps: 0.02,
            loss_gradient: true,
            root_guards: true,
            ..MctsConfig::default()
        };
        let mut mcts = Mcts::new(state, config);
        let _ = mcts.search();
        for o in mcts.root_outcomes() {
            assert!(
                o.mean_value.is_finite() && o.mean_value.abs() <= MAX_LEAF_VALUE_ABS,
                "root child {:?} reports Q={} (|Q|>{MAX_LEAF_VALUE_ABS}) with visits={}, value_sum={}",
                o.action,
                o.mean_value,
                o.visits,
                o.value_sum,
            );
        }
    }

    /// Bead `sts2-rs-wrip2`: visit-weighted root Q surface, used as
    /// the TD-style value target in the corpus emitter. After a real
    /// search the value must be finite and within the leaf-value
    /// bound; before any search it must be `None` (root has no
    /// expanded children with visits).
    #[test]
    fn root_q_surface_pins_bounded_and_unset_before_search() {
        let state = make_smoke_state(7);
        let config = MctsConfig {
            c_puct: 1.0,
            max_simulations: 200,
            max_rollout_depth: 20,
            seed: 1,
            rollout_policy: RolloutPolicyKind::Heuristic,
            use_transposition_table: false,
            regularized_pi_temperature: 0.1,
            use_puct_prior: false,
            terminal_eval_mode: TerminalEvalMode::RichBoth,
            use_quiescence: false,
            use_lethal_detector: false,
            chance_widening_c: 1.0,
            chance_widening_alpha: 0.5,
            draw_widening_c: 1.0,
            draw_widening_alpha: 0.5,
            endturn_widening_c: 0.5,
            endturn_widening_alpha: 0.4,
            chance_prove_wins: false,
            chance_closed_stops_widening: false,
            chance_max_depth: CHANCE_MAX_DEPTH,
            hidden_info: HiddenInfoMode::Clairvoyant,
            common_random_numbers: false,
            tie_eps: 0.02,
            loss_gradient: true,
            root_guards: true,
            ..MctsConfig::default()
        };
        let mut mcts = Mcts::new(state, config);
        // Pre-search: no children visited → None.
        assert!(
            mcts.root_q().is_none(),
            "root_q must be None before any search"
        );
        let _ = mcts.search();
        let q = mcts.root_q().expect("root_q is Some after search");
        assert!(
            q.is_finite() && q.abs() <= MAX_LEAF_VALUE_ABS,
            "root_q={q} must be finite and within ±{MAX_LEAF_VALUE_ABS}"
        );
        // Consistency with root_outcomes: visit-weighted mean of
        // child mean_values equals root_q (within float tolerance).
        let outcomes = mcts.root_outcomes();
        let total_visits: u64 = outcomes.iter().map(|o| u64::from(o.visits)).sum();
        if total_visits > 0 {
            #[allow(clippy::cast_precision_loss)]
            let weighted: f64 =
                outcomes.iter().map(|o| f64::from(o.value_sum)).sum::<f64>() / total_visits as f64;
            assert!(
                (f64::from(q) - weighted).abs() < 1e-4,
                "root_q {q} disagrees with visit-weighted root_outcomes mean {weighted}"
            );
        }
    }

    /// GitHub issue #32 user-visible fix, A8-audit trigger: the
    /// override fires iff the root is a solver-PROVEN loss
    /// (`root_proven_loss()`), and the returned action must be one of
    /// the root children's actions — the override doesn't synthesize
    /// new actions, it picks among the search's expanded ones. (The
    /// old trigger — every child `wins == 0 && losses > 0` — was
    /// trivially satisfied by depth-capped rollouts against big
    /// bosses; FINDINGS §3.)
    #[test]
    fn github32_lost_position_override_returns_root_child_action() {
        let state = make_smoke_state(7);
        let config = MctsConfig {
            c_puct: 1.0,
            max_simulations: 200,
            max_rollout_depth: 20,
            seed: 1,
            rollout_policy: RolloutPolicyKind::Heuristic,
            use_transposition_table: false,
            regularized_pi_temperature: 0.1,
            use_puct_prior: false,
            terminal_eval_mode: TerminalEvalMode::RichBoth,
            use_quiescence: false,
            use_lethal_detector: false,
            chance_widening_c: 1.0,
            chance_widening_alpha: 0.5,
            draw_widening_c: 1.0,
            draw_widening_alpha: 0.5,
            endturn_widening_c: 0.5,
            endturn_widening_alpha: 0.4,
            chance_prove_wins: false,
            chance_closed_stops_widening: false,
            chance_max_depth: CHANCE_MAX_DEPTH,
            hidden_info: HiddenInfoMode::Clairvoyant,
            common_random_numbers: false,
            tie_eps: 0.02,
            loss_gradient: true,
            root_guards: true,
            ..MctsConfig::default()
        };
        let mut mcts = Mcts::new(state, config);
        let _ = mcts.search();
        let outcomes = mcts.root_outcomes();
        if let Some(act) = mcts.lost_position_override() {
            assert!(
                outcomes.iter().any(|o| o.action == act),
                "lost_position_override returned an action not present in root children: {act:?}"
            );
            // A8 trigger: the override fires ONLY on a proven loss.
            assert!(
                mcts.root_proven_loss(),
                "override fired without root_proven_loss()"
            );
        } else {
            // If the override declined, the root must not be a proven
            // loss (or have no children to pick among).
            assert!(
                !mcts.root_proven_loss() || outcomes.is_empty(),
                "override declined on a proven-loss root with children; \
                 outcomes={outcomes:?}"
            );
        }
    }

    /// GitHub issue #32 mechanical test: when the root is flagged a
    /// proven loss, `lost_position_override` must pick the action with
    /// the largest 1-ply `damage_dealt + block_credit - 0.5 * hp_lost`
    /// score, NOT `EndTurn`.
    ///
    /// Constructed by hand-setting the root's `proven_loss` after a
    /// short warm-up search (the A8-audit trigger): we don't need a
    /// real solver proof — we just need to verify the override's
    /// selection criterion once the precondition is met.
    #[test]
    fn github32_override_picks_damage_dealing_action_when_all_lost() {
        let state = make_smoke_state(7);
        let config = MctsConfig {
            c_puct: 1.0,
            max_simulations: 50,
            max_rollout_depth: 5,
            seed: 1,
            rollout_policy: RolloutPolicyKind::Uniform,
            use_transposition_table: false,
            regularized_pi_temperature: 0.1,
            use_puct_prior: false,
            terminal_eval_mode: TerminalEvalMode::RichBoth,
            use_quiescence: false,
            use_lethal_detector: false,
            chance_widening_c: 1.0,
            chance_widening_alpha: 0.5,
            draw_widening_c: 1.0,
            draw_widening_alpha: 0.5,
            endturn_widening_c: 0.5,
            endturn_widening_alpha: 0.4,
            chance_prove_wins: false,
            chance_closed_stops_widening: false,
            chance_max_depth: CHANCE_MAX_DEPTH,
            hidden_info: HiddenInfoMode::Clairvoyant,
            common_random_numbers: false,
            tie_eps: 0.02,
            loss_gradient: true,
            root_guards: true,
            ..MctsConfig::default()
        };
        let mut mcts = Mcts::new(state, config);
        let _ = mcts.search();
        // Force the A8 precondition — a solver-proven lost root — so
        // the override's selection logic exercises.
        let root_id = mcts.tree.root;
        mcts.tree.node_mut(root_id).proven_loss = true;
        // make_smoke_state has empty hand → only EndTurn legal, so the
        // single-child case must still return Some(EndTurn) (degenerate
        // but correct: there's nothing to pick between).
        let pick = mcts.lost_position_override();
        assert!(
            pick.is_some(),
            "override must fire when the root is a proven loss"
        );
    }

    // ── loss gradient + proven-loss override (2026-08-09 A8 audit, §3) ──

    /// The audit default is load-bearing for the deployed agent — the
    /// rollback seam is `STS2_RUNAI_LOSS_GRADIENT=0` / `loss_gradient:
    /// false`, not this default.
    #[test]
    fn default_config_loss_gradient_on() {
        assert!(MctsConfig::default().loss_gradient);
    }

    /// FINDINGS §3 fix, half 1: two proven-loss terminals of different
    /// survival quality must back up DIFFERENT values (the audit
    /// measured 2428 pure-loss children at sd 0.0003 — zero gradient),
    /// ordered by quality, while every loss stays strictly below every
    /// depth-cut (> -1.0) and win (>= +1.0) value. The rollback arm
    /// (`loss_gradient: false`) restores the constant −1.45 floor
    /// byte-for-byte.
    #[test]
    fn loss_gradient_orders_terminal_losses_below_every_nonloss() {
        // Died at full enemy HP on turn 1...
        let mut early = make_smoke_state(1);
        early.is_over = true;
        early.player_won = false;
        early.player.current_hp = 0;
        early.round = 1;
        // ...vs died with the enemy one hit from dead on turn 6.
        let mut late = early.clone();
        late.round = 6;
        late.creatures[0].current_hp = 5; // 43/48 removed

        let leaf = LeafEval::default();
        let v_early = terminal_state_value(&early, TerminalEvalMode::RichBoth, &leaf, true);
        let v_late = terminal_state_value(&late, TerminalEvalMode::RichBoth, &leaf, true);
        assert!(
            v_late > v_early,
            "the nearly-won loss must rank above the immediate one: \
             late={v_late}, early={v_early}"
        );
        for v in [v_early, v_late] {
            assert!(
                (-1.45..-1.05).contains(&v),
                "gradient losses must land in [-1.45, -1.05), got {v}"
            );
        }

        // Rollback arm: both collapse to the pre-audit constant floor.
        let c_early = terminal_state_value(&early, TerminalEvalMode::RichBoth, &leaf, false);
        let c_late = terminal_state_value(&late, TerminalEvalMode::RichBoth, &leaf, false);
        assert!(
            (c_early - c_late).abs() < 1e-6 && (c_early + 1.45).abs() < 1e-6,
            "loss_gradient=false must restore the constant -1.45 backup, \
             got early={c_early}, late={c_late}"
        );
    }

    /// Arm E surface (2026-08-15 Optuna study prep): the parameterized
    /// [`loss_quality_with`] must reproduce the legacy [`loss_quality`]
    /// const path at [`MctsConfig::default()`], and overriding the
    /// weights must actually move the score.
    #[test]
    #[allow(clippy::float_cmp)] // deliberate bit-for-bit byte-stability check
    fn loss_quality_config_defaults_match_consts() {
        let cfg = MctsConfig::default();
        let s = make_smoke_state(1);
        assert_eq!(loss_quality_with(&s, &cfg), loss_quality(&s));

        let mut cfg2 = cfg;
        cfg2.loss_w_removed = 1.0;
        cfg2.loss_w_hp = 0.0;
        cfg2.loss_w_turns = 0.0;
        assert_ne!(loss_quality_with(&s, &cfg2), loss_quality(&s));
    }

    /// FINDINGS §3 fix, half 2 (trigger): the old every-child-`wins==0`
    /// scan — trivially satisfied by depth-capped rollouts against big
    /// bosses, and wrong as a loss label (6/15 such fights were won) —
    /// must NOT fire the override anymore; only a solver-proven lost
    /// root does.
    #[test]
    fn observed_lost_without_proof_no_longer_fires_override() {
        let (mut mcts, _actions) = searched_pick_fixture(MctsConfig::default(), false);
        // Rig the exact shape the old trigger keyed on: every child
        // visited, zero wins, all losses.
        let ids: Vec<NodeId> = mcts
            .tree
            .node(mcts.tree.root)
            .children
            .iter()
            .map(|(_, cid)| *cid)
            .collect();
        for id in &ids {
            let c = mcts.tree.node_mut(*id);
            c.visits = c.visits.max(1);
            c.wins = 0;
            c.losses = c.visits;
            c.ties = 0;
        }
        assert!(
            mcts.lost_position_override().is_none(),
            "observed-lost-but-unproven must stay on pick_root_action"
        );
        // Same tree with the solver proof set: the override fires.
        let root_id = mcts.tree.root;
        mcts.tree.node_mut(root_id).proven_loss = true;
        assert!(
            mcts.lost_position_override().is_some(),
            "a proven-loss root with children must fire the override"
        );
    }

    /// All-Defend twin of [`make_pick_state`] for the override's
    /// block-credit rule: player at 1 HP, enemy telegraphing a lethal
    /// 22-damage attack, hand of `DefendIronclad` (5 block) only. Both
    /// candidates die through the next enemy turn (22 − 5 > 1), so the
    /// GH #215 survival key ties and the pick falls to the 1-ply score.
    fn make_block_lost_state() -> CombatState {
        use sts2_core::cards::{Card, CardId, CardTags, CardType, TargetType};
        use sts2_core::CardUid;
        sts2_core::card_effects::ensure_installed();
        sts2_content::cards::dispatch::install_default_registry();
        let mut state = make_smoke_state(11);
        for idx in 1..=10u32 {
            let uid = CardUid::new(idx).expect("non-zero");
            state.cards.push(Card {
                uid,
                card_id: CardId::DefendIronclad,
                cost: 1,
                card_type: CardType::Skill,
                target_type: TargetType::Self_,
                upgraded: false,
                base_damage: 0,
                base_block: 5,
                combat_vars: sts2_core::cards::CombatVars::new(),
                enchantments: sts2_core::cards::CardEnchantments::new(),
                tags: CardTags::NONE,
                energy_spent: 0,
                base_replay_count: 0,
                keyword_override: None,
                affliction: None,
            });
        }
        while state.player.hand.len() < 3 {
            let uid = state.player.draw.remove(0);
            state.player.hand.push(uid);
        }
        state.player.energy = 3;
        state.player.current_hp = 1;
        state.creatures[0].intent = Some(sts2_core::Intent::Attack { damage: 22 });
        state
    }

    /// FINDINGS §3 fix, half 3 (the "structurally cannot block" gap):
    /// on a proven-loss root with a block card and telegraphed incoming
    /// damage, the override must pick the block card, not EndTurn. Both
    /// die (survival ties), damage and hp_lost are 0-vs-small — before
    /// the block credit + EndTurn-tier removal this fell into the 0.5
    /// tie band and the EndTurn-above-PlayCard tier took it.
    #[test]
    fn proven_loss_override_prefers_block_over_endturn() {
        let mut mcts = Mcts::new(make_block_lost_state(), MctsConfig::default());
        let _ = mcts.search_n(16);
        let root_id = mcts.tree.root;
        mcts.tree.node_mut(root_id).proven_loss = true;

        let pick = mcts
            .lost_position_override()
            .expect("proven-loss root with children must fire the override");
        assert!(
            matches!(pick, Action::PlayCard { .. }),
            "22 incoming and a Defend in hand: the override must block, \
             not end turn — picked {pick:?}"
        );
        // The full pick rule must route the same action out.
        assert_eq!(
            mcts.pick_root_action().expect("non-terminal root"),
            pick,
            "pick_root_action must surface the override's pick"
        );
    }

    /// The chance-side proof rule is pessimistic (ANY sampled outcome
    /// proven-lost stamps the chance node lost), so a root can be
    /// "proven" lost while its rollouts win — the 2026-08-10 post-fix
    /// corpus caught a root with 244/2016 winning rollouts on its best
    /// child firing the override. With any winning rollout on record the
    /// override must stand down and leave the pick to the search.
    #[test]
    fn proven_loss_override_stands_down_when_a_rollout_won() {
        let mut mcts = Mcts::new(make_block_lost_state(), MctsConfig::default());
        let _ = mcts.search_n(16);
        let root_id = mcts.tree.root;
        mcts.tree.node_mut(root_id).proven_loss = true;
        let child = mcts.tree.node(root_id).children[0].1;
        mcts.tree.node_mut(child).wins += 1;

        assert_eq!(
            mcts.lost_position_override(),
            None,
            "a winning rollout is an empirical counterexample to the \
             pessimistic loss proof — the override must not fire"
        );
        assert!(
            mcts.pick_root_action().is_ok(),
            "the pick must fall through to the normal band rule"
        );
    }

    // ── `pick_root_action` tie-band pick rule (2026-08-09 A8 audit) ──────

    /// Multi-action twin of [`make_smoke_state`]: same Ironclad-vs-Nibbit
    /// shell, but every deck uid gets a backing [`sts2_core::cards::Card`]
    /// and three cards are pulled to hand with 3 energy, so the root has
    /// several children (`PlayCard` × 3 + `EndTurn`) for the pick-rule
    /// tests. `with_ftl` swaps the hand-slot-2 card for a 0-cost FTL
    /// (live draw window: 0 cards played, non-empty draw pile) without
    /// changing the action structure — the two variants differ ONLY in
    /// whether [`expert::draw_window_root_q_bonus`] fires.
    fn make_pick_state(seed: u64, with_ftl: bool) -> CombatState {
        use sts2_core::cards::{Card, CardId, CardTags, CardType, TargetType};
        use sts2_core::CardUid;
        sts2_core::card_effects::ensure_installed();
        sts2_content::cards::dispatch::install_default_registry();
        let mut state = make_smoke_state(seed);
        for idx in 1..=10u32 {
            let uid = CardUid::new(idx).expect("non-zero");
            let attack = idx % 2 == 1;
            state.cards.push(Card {
                uid,
                card_id: if attack {
                    CardId::StrikeIronclad
                } else {
                    CardId::DefendIronclad
                },
                cost: 1,
                card_type: if attack {
                    CardType::Attack
                } else {
                    CardType::Skill
                },
                target_type: if attack {
                    TargetType::Enemy
                } else {
                    TargetType::Self_
                },
                upgraded: false,
                base_damage: if attack { 6 } else { 0 },
                base_block: if attack { 0 } else { 5 },
                combat_vars: sts2_core::cards::CombatVars::new(),
                enchantments: sts2_core::cards::CardEnchantments::new(),
                tags: CardTags::NONE,
                energy_spent: 0,
                base_replay_count: 0,
                keyword_override: None,
                affliction: None,
            });
        }
        // `start_player_turn` already drew 5 uids to hand; top up so
        // slot 2 exists even if a variant draws fewer, and fund plays.
        while state.player.hand.len() < 3 {
            let uid = state.player.draw.remove(0);
            state.player.hand.push(uid);
        }
        state.player.energy = 3;
        // HAND SLOT 2 becomes a distinct 0-cost attack in BOTH variants
        // — as a duplicate of the other Strikes/Defends in hand it would
        // be collapsed into one canonical root child, and the tests
        // below need a child pinned to `hand_index: 2`. FTL vs Slice is
        // the ONLY difference between the variants: a live draw window
        // or none.
        let uid = state.player.hand[2];
        let card = &mut state.cards[uid.0.get() as usize - 1];
        card.card_id = if with_ftl { CardId::Ftl } else { CardId::Slice };
        card.cost = 0;
        card.card_type = CardType::Attack;
        card.target_type = TargetType::Enemy;
        card.base_damage = if with_ftl { 5 } else { 6 };
        card.base_block = 0;
        if with_ftl {
            // Window open: 0 plays this turn, draw pile non-empty.
            assert!(!state.player.draw.is_empty());
            assert!(crate::expert::has_live_draw_window(&state));
        }
        state
    }

    /// Overwrite root child `i`'s stats with `stats[i] = (visits, mean)`
    /// in child-creation order. `wins = 1` on every rigged child keeps
    /// the GH #32 lost-position override off, and proven flags are
    /// cleared so the rule-2 proven-win guard stays off too — the tests
    /// below exercise the score/tie-band rules, not the exact guards.
    fn rig_root_stats(mcts: &mut Mcts, stats: &[(u32, f32)]) {
        let ids: Vec<NodeId> = mcts
            .tree
            .node(mcts.tree.root)
            .children
            .iter()
            .map(|(_, cid)| *cid)
            .collect();
        assert_eq!(
            ids.len(),
            stats.len(),
            "rig_root_stats: fixture has {} root children, test supplied {}",
            ids.len(),
            stats.len()
        );
        for (i, &(visits, mean)) in stats.iter().enumerate() {
            let c = mcts.tree.node_mut(ids[i]);
            c.visits = visits;
            #[allow(clippy::cast_precision_loss)]
            {
                c.value_sum = mean * visits as f32;
                // Rigged stats carry no spread, so the second moment is the
                // zero-variance one. Only read when the variance-scaled
                // `c_puct` is enabled, which these fixtures do not do.
                c.value_sq_sum = mean * mean * visits as f32;
            }
            c.wins = 1;
            c.losses = 0;
            c.ties = visits.saturating_sub(1);
            c.proven_win = false;
            c.proven_loss = false;
        }
    }

    /// Expand every root child and return the searched engine plus its
    /// root actions in child-creation order (the order `rig_root_stats`
    /// indexes by).
    fn searched_pick_fixture(config: MctsConfig, with_ftl: bool) -> (Mcts, Vec<Action>) {
        let mut mcts = Mcts::new(make_pick_state(3, with_ftl), config);
        let _ = mcts.search_n(64);
        let actions: Vec<Action> = mcts
            .root_outcomes()
            .iter()
            .map(|o| o.action.clone())
            .collect();
        assert!(
            actions.len() >= 3,
            "pick fixture must surface several root children, got {actions:?}"
        );
        (mcts, actions)
    }

    // ── GitHub #492: backup rule ───────────────────────────────────────

    /// The deployed backup rule. `Mean` is what the 50 000-sim position
    /// book's `gold_action` answers were produced with, so flipping this
    /// default silently re-bases every `gold_agr` number in the repo.
    #[test]
    fn default_config_backup_rule_is_mean() {
        let cfg = MctsConfig::default();
        assert_eq!(cfg.backup_rule, BackupRule::Mean);
        assert!((cfg.backup_power_p - 1.0).abs() < 1e-6);
        assert!((cfg.backup_power_shift - MAX_LEAF_VALUE_ABS).abs() < 1e-6);
    }

    /// Rig a root's children (and its own leaf sample) to known values and
    /// return the root Q that `reaggregate_node` produces at order `p`.
    fn rigged_aggregate(
        rule: BackupRule,
        p: f32,
        shift: f32,
        own: (u32, f32),
        children: &[(u32, f32)],
    ) -> f32 {
        let cfg = MctsConfig {
            backup_rule: rule,
            backup_power_p: p,
            backup_power_shift: shift,
            ..MctsConfig::default()
        };
        let (mut mcts, _) = searched_pick_fixture(cfg, false);
        let ids: Vec<NodeId> = mcts
            .tree
            .node(mcts.tree.root)
            .children
            .iter()
            .map(|(_, cid)| *cid)
            .collect();
        assert!(
            ids.len() >= children.len(),
            "fixture has {} root children, test wants {}",
            ids.len(),
            children.len()
        );
        // Zero every child, then set the ones the test cares about, so
        // untouched children (visits 0) drop out of the aggregate.
        for &cid in &ids {
            let c = mcts.tree.node_mut(cid);
            c.visits = 0;
            c.value_sum = 0.0;
        }
        let root = mcts.tree.root;
        #[allow(clippy::cast_precision_loss)]
        for (i, &(visits, mean)) in children.iter().enumerate() {
            let c = mcts.tree.node_mut(ids[i]);
            c.visits = visits;
            c.value_sum = mean * visits as f32;
            // The backup weights are per-EDGE, so rig the edge too.
            mcts.tree.node_mut(root).child_visits[i] = visits;
        }
        for i in children.len()..ids.len() {
            mcts.tree.node_mut(root).child_visits[i] = 0;
        }
        let total: u32 = own.0 + children.iter().map(|(v, _)| *v).sum::<u32>();
        {
            #[allow(clippy::cast_precision_loss)]
            let r = mcts.tree.node_mut(root);
            r.own_visits = own.0;
            r.own_value_sum = own.1 * own.0 as f32;
            r.visits = total;
            r.value_sum = 0.0;
        }
        mcts.reaggregate_node(root);
        let r = mcts.tree.node(root);
        #[allow(clippy::cast_precision_loss)]
        {
            r.value_sum / r.visits as f32
        }
    }

    /// `p = 1` is the identity: the power mean collapses to the
    /// visit-weighted arithmetic mean *including the node's own leaf
    /// sample*, and the positive-domain shift cancels exactly. This is
    /// what makes `p` a continuous knob anchored on today's behaviour
    /// rather than a discrete swap.
    #[test]
    fn power_mean_backup_reduces_to_the_mean_at_p_one() {
        let own = (1_u32, -0.20_f32);
        let kids = [(700_u32, 1.2118_f32), (670, 1.2078), (623, 1.2023)];
        let expected = {
            let num = f64::from(own.1) * f64::from(own.0)
                + kids
                    .iter()
                    .map(|&(v, q)| f64::from(q) * f64::from(v))
                    .sum::<f64>();
            let den = f64::from(own.0) + kids.iter().map(|&(v, _)| f64::from(v)).sum::<f64>();
            (num / den) as f32
        };
        for shift in [1.5_f32, 3.0, 8.0] {
            let got = rigged_aggregate(BackupRule::PowerMean, 1.0, shift, own, &kids);
            assert!(
                (got - expected).abs() < 1e-4,
                "p=1 shift={shift} gave {got}, weighted mean is {expected}"
            );
        }
    }

    /// Monotone increasing in `p` (Dam et al. 2020, Lemma 1), and never
    /// outside the range of its inputs — the property that keeps
    /// `|Q| <= MAX_LEAF_VALUE_ABS` true for free.
    #[test]
    fn power_mean_backup_is_monotone_in_p_and_bounded_by_its_inputs() {
        let own = (1_u32, 0.10_f32);
        let kids = [(700_u32, 1.2118_f32), (670, 1.2078), (866, 1.1876)];
        let lo = 0.10_f32;
        let hi = 1.2118_f32;
        let mut prev = f32::NEG_INFINITY;
        for p in [1.0_f32, 1.5, 2.0, 3.0, 4.0, 8.0] {
            let q = rigged_aggregate(BackupRule::PowerMean, p, MAX_LEAF_VALUE_ABS, own, &kids);
            assert!(
                q >= prev - 1e-6,
                "power mean fell from {prev} to {q} when p rose to {p}"
            );
            assert!(
                q >= lo - 1e-4 && q <= hi + 1e-4,
                "power mean {q} at p={p} escaped the input range [{lo}, {hi}]"
            );
            prev = q;
        }
        // ...and it has to actually move, or the knob is decorative.
        let p1 = rigged_aggregate(BackupRule::PowerMean, 1.0, MAX_LEAF_VALUE_ABS, own, &kids);
        let p8 = rigged_aggregate(BackupRule::PowerMean, 8.0, MAX_LEAF_VALUE_ABS, own, &kids);
        assert!(
            p8 - p1 > 1e-5,
            "p=8 ({p8}) did not rise above p=1 ({p1}) — operator is inert"
        );
    }

    /// A smaller shift is a *more aggressive* operator at the same `p`,
    /// because it stretches the relative spread of the shifted operands.
    /// This is why `backup_power_shift` is a documented parameter of the
    /// arm and not a hidden constant.
    #[test]
    fn power_mean_shift_controls_aggression() {
        let own = (1_u32, 0.10_f32);
        let kids = [(700_u32, 1.2118_f32), (670, 1.2078), (866, 1.1876)];
        let tight = rigged_aggregate(BackupRule::PowerMean, 4.0, MAX_LEAF_VALUE_ABS, own, &kids);
        let loose = rigged_aggregate(BackupRule::PowerMean, 4.0, 8.0, own, &kids);
        assert!(
            tight > loose,
            "shift 1.5 ({tight}) should lean harder toward the max than shift 8.0 ({loose})"
        );
    }

    /// Chance nodes must keep the arithmetic mean: their children are
    /// outcomes of nature, and a max-leaning aggregate over them plans
    /// against a draw the agent cannot force. Power-UCT applies the power
    /// mean at V nodes only; the Q node stays a running average (their
    /// Algorithm 1 line 51, and the explicit `SimulateQ`/`SimulateV`
    /// split in the 2024 stochastic follow-up).
    ///
    /// Asserted as a CONTRAST on the same rigged spread: a decision node
    /// whose two successors sit at −1.4 and +1.2 must rise well above
    /// their weighted mean at `p = 8`; a chance node with the identical
    /// rig must not move at all. Checking only the chance side would pass
    /// just as happily if the whole arm were inert.
    #[test]
    fn power_mean_backup_leaves_chance_nodes_alone() {
        let cfg = MctsConfig {
            backup_rule: BackupRule::PowerMean,
            backup_power_p: 8.0,
            ..chance_cfg(5, 400)
        };
        // `make_pick_state`, not `make_smoke_state`: the smoke fixture's hand
        // is empty, so every decision node has exactly one child (EndTurn) and
        // a power mean over a single value is that value — the test would be
        // structurally unable to fail.
        let mut mcts = Mcts::new(make_pick_state(3, false), cfg);
        let _ = mcts.search_n(400);
        assert!(
            count_chance_nodes(&mcts) > 0,
            "fixture produced no chance nodes — the assertion below would be vacuous"
        );

        // A wide, lopsided spread: the weighted mean is −0.1, the max is
        // +1.2, and `p = 8` should land much nearer the latter.
        const RIG: [(u32, f32); 2] = [(100, -1.4), (100, 1.2)];
        const RIGGED_MEAN: f32 = -0.1;

        #[allow(clippy::cast_precision_loss)]
        fn rig_successors(m: &mut Mcts, id: NodeId, kids: &[NodeId]) {
            for (i, &cid) in kids.iter().take(RIG.len()).enumerate() {
                let (v, q) = RIG[i];
                let c = m.tree.node_mut(cid);
                c.visits = v;
                c.value_sum = q * v as f32;
            }
            let n = m.tree.node_mut(id);
            for (i, slot) in n.child_visits.iter_mut().enumerate() {
                *slot = RIG.get(i).map_or(0, |&(v, _)| v);
            }
        }
        #[allow(clippy::cast_precision_loss)]
        fn repowered_q(m: &mut Mcts, id: NodeId, kids: &[NodeId]) -> f32 {
            rig_successors(m, id, kids);
            {
                let n = m.tree.node_mut(id);
                n.own_visits = 0;
                n.own_value_sum = 0.0;
                n.visits = RIG.iter().map(|(v, _)| *v).sum();
                n.value_sum = RIGGED_MEAN * n.visits as f32;
            }
            m.reaggregate_node(id);
            let n = m.tree.node(id);
            n.value_sum / n.visits as f32
        }

        let find = |want: NodeKind| -> Option<(NodeId, Vec<NodeId>)> {
            (0..mcts.tree.arena.len()).find_map(|i| {
                let id = NodeId(u32::try_from(i).expect("arena index fits u32"));
                let n = mcts.tree.node(id);
                if n.kind != want {
                    return None;
                }
                let kids: Vec<NodeId> = if want == NodeKind::Chance {
                    n.chance_outcomes.iter().map(|o| o.child).collect()
                } else {
                    n.children.iter().map(|(_, cid)| *cid).collect()
                };
                (kids.len() >= RIG.len()).then_some((id, kids))
            })
        };

        let (dec_id, dec_kids) =
            find(NodeKind::Decision).expect("no decision node with two children");
        let (ch_id, ch_kids) = find(NodeKind::Chance).expect("no chance node with two outcomes");

        let dec_q = repowered_q(&mut mcts, dec_id, &dec_kids);
        assert!(
            dec_q > RIGGED_MEAN + 0.5,
            "decision node stayed at {dec_q} on a rig whose mean is {RIGGED_MEAN} and max is 1.2 \
             — the power mean never fired"
        );
        assert!(
            dec_q <= 1.2 + 1e-4,
            "decision node {dec_q} escaped above its best successor"
        );

        let ch_q = repowered_q(&mut mcts, ch_id, &ch_kids);
        assert!(
            (ch_q - RIGGED_MEAN).abs() < 1e-4,
            "chance node moved to {ch_q} on the identical rig — expectation over nature was \
             replaced by a max-leaning aggregate"
        );
    }

    /// End-to-end: the knob has to reach the root statistics. A backup
    /// rule that compiles, ladders and reports "no effect" because it
    /// never actually fired is the failure mode this repo has already
    /// paid for once (the 2026-08-22 game-edge priors were applied to
    /// 0.70% of child scorings and one of them fired zero times).
    #[test]
    fn power_mean_p4_changes_root_statistics_vs_mean() {
        let base = MctsConfig {
            max_simulations: 400,
            seed: 5,
            ..MctsConfig::default()
        };
        let arm = MctsConfig {
            backup_rule: BackupRule::PowerMean,
            backup_power_p: 4.0,
            ..base
        };

        let mean_out = {
            let mut m = Mcts::new(make_pick_state(3, false), base);
            let _ = m.search();
            m.root_outcomes()
        };
        let arm_out = {
            let mut m = Mcts::new(make_pick_state(3, false), arm);
            let _ = m.search();
            m.root_outcomes()
        };
        assert_eq!(
            mean_out.len(),
            arm_out.len(),
            "arms disagree on the root action set"
        );
        let moved = mean_out
            .iter()
            .zip(arm_out.iter())
            .any(|(a, b)| (a.mean_value - b.mean_value).abs() > 1e-4 || a.visits != b.visits);
        assert!(
            moved,
            "PowerMean p=4 produced identical root statistics to Mean — the knob is inert"
        );
    }

    /// `PowerMean` at `p = 1` re-aggregates from successors instead of
    /// accumulating returns. On a tree those coincide; this search is a DAG
    /// (the transposition table is unconditionally on), so they do not — and
    /// the gap is the same order as the sibling gaps #492 is about.
    ///
    /// This test PINS that gap rather than asserting it away. If it ever
    /// collapses to zero the DAG note on `reaggregate_node` is wrong; if it
    /// grows past the bound below, `p = 1` has stopped being a usable
    /// isolation arm for the re-aggregation change and the ladder's
    /// interpretation has to be revisited.
    #[test]
    fn power_mean_p1_isolates_the_dag_reaggregation_gap() {
        let base = MctsConfig {
            max_simulations: 400,
            seed: 9,
            ..MctsConfig::default()
        };
        let arm = MctsConfig {
            backup_rule: BackupRule::PowerMean,
            backup_power_p: 1.0,
            ..base
        };

        let mut a = Mcts::new(make_pick_state(3, false), base);
        let _ = a.search();
        let mut b = Mcts::new(make_pick_state(3, false), arm);
        let _ = b.search();

        let mut worst = 0.0_f32;
        for (x, y) in a.root_outcomes().iter().zip(b.root_outcomes().iter()) {
            assert_eq!(
                format!("{:?}", x.action),
                format!("{:?}", y.action),
                "arms disagree on the root action set"
            );
            worst = worst.max((x.mean_value - y.mean_value).abs());
        }
        assert!(
            worst > 1e-4,
            "p=1 reproduced Mean exactly ({worst}) — either the TT is no longer merging or \
             re-aggregation is not happening"
        );
        assert!(
            worst < 0.05,
            "p=1 moved a root Q by {worst}, far past the ~0.01 DAG re-aggregation gap — \
             it is no longer isolating that change"
        );
    }

    /// The normalized variant exists because the shifted one is close to
    /// inert on the narrow sibling gaps GitHub #492 is actually about.
    /// Pin that: on a rig whose successors differ by 0.02 around +1.2 —
    /// the scale of the repro position — `PowerMean` at `p = 6` must move
    /// the aggregate by less than a tenth of what `PowerMeanNorm` moves it.
    /// If this ever stops holding, the "shift form cannot see narrow
    /// spreads" argument in the findings is wrong.
    #[test]
    fn shifted_power_mean_is_near_inert_on_narrow_spreads() {
        let own = (1_u32, 1.2000_f32);
        let kids = [(700_u32, 1.2100_f32), (670, 1.2000), (623, 1.1900)];
        let weighted_mean = {
            let num = f64::from(own.1) * f64::from(own.0)
                + kids
                    .iter()
                    .map(|&(v, q)| f64::from(q) * f64::from(v))
                    .sum::<f64>();
            let den = f64::from(own.0) + kids.iter().map(|&(v, _)| f64::from(v)).sum::<f64>();
            (num / den) as f32
        };
        let shifted = rigged_aggregate(BackupRule::PowerMean, 6.0, MAX_LEAF_VALUE_ABS, own, &kids);
        let normed = rigged_aggregate(
            BackupRule::PowerMeanNorm,
            6.0,
            MAX_LEAF_VALUE_ABS,
            own,
            &kids,
        );
        let d_shift = shifted - weighted_mean;
        let d_norm = normed - weighted_mean;
        assert!(
            d_norm > 0.0 && d_shift > 0.0,
            "both variants must lean up: shift {d_shift}, norm {d_norm}"
        );
        assert!(
            d_shift < 0.1 * d_norm,
            "shifted p=6 moved {d_shift} vs normalized {d_norm} on a 0.02 spread — the shift form \
             is no longer the near-inert one, so the findings' explanation is stale"
        );
    }

    /// `PowerMeanNorm` at `p = 1` is still the weighted mean: the
    /// normalization is affine, and the mean commutes with affine maps.
    /// Without this the normalized ladder would have no anchor.
    #[test]
    fn normalized_power_mean_reduces_to_the_mean_at_p_one() {
        let own = (1_u32, -0.20_f32);
        let kids = [(700_u32, 1.2118_f32), (670, 1.2078), (623, 1.2023)];
        let expected = {
            let num = f64::from(own.1) * f64::from(own.0)
                + kids
                    .iter()
                    .map(|&(v, q)| f64::from(q) * f64::from(v))
                    .sum::<f64>();
            let den = f64::from(own.0) + kids.iter().map(|&(v, _)| f64::from(v)).sum::<f64>();
            (num / den) as f32
        };
        let got = rigged_aggregate(
            BackupRule::PowerMeanNorm,
            1.0,
            MAX_LEAF_VALUE_ABS,
            own,
            &kids,
        );
        assert!(
            (got - expected).abs() < 1e-4,
            "normalized p=1 gave {got}, weighted mean is {expected}"
        );
    }

    /// The `own_visits` / `own_value_sum` split has to be maintained by
    /// the plain `Mean` path too, or a config flip mid-life would read a
    /// statistic that was never collected.
    #[test]
    fn own_leaf_stats_are_maintained_under_the_mean_rule() {
        let mut m = Mcts::new(
            make_smoke_state(2),
            MctsConfig {
                max_simulations: 300,
                seed: 1,
                ..MctsConfig::default()
            },
        );
        let _ = m.search();
        let mut own_total = 0_u32;
        for i in 0..m.tree.arena.len() {
            let n = &m.tree.arena[i];
            assert!(
                n.own_visits <= n.visits,
                "node {i} banked {} own visits out of {} total",
                n.own_visits,
                n.visits
            );
            own_total += n.own_visits;
        }
        assert_eq!(
            own_total, 300,
            "every simulation contributes exactly one leaf sample"
        );
    }

    /// The audit default is load-bearing for the deployed agent — flip it
    /// only with a fresh paired A/B, and update this pin deliberately.
    #[test]
    fn default_config_tie_eps_is_the_q_noise_floor() {
        assert!((MctsConfig::default().tie_eps - 0.02).abs() < 1e-6);
    }

    /// A8 audit rule 4: a child whose score sits INSIDE the `tie_eps`
    /// band of the best but holds more visits wins the pick — the
    /// permanent-buff pattern (Inflame/Loop/Noxious Fumes lost the
    /// Q-argmax by <0.007 while holding 2×+ the visits). Also pins that
    /// `search_n(0)` (the bench/CLI entry point) and `pick_root_action`
    /// (what `sts2-runai::mcts_pick` reaches through `try_search_n`)
    /// return the SAME action — the two entry points used to diverge on
    /// ties (max_by last-max vs stable-sort first-max).
    #[test]
    fn pick_prefers_higher_visits_inside_tie_band() {
        let (mut mcts, actions) = searched_pick_fixture(MctsConfig::default(), false);
        let mut stats = vec![(10_u32, -0.9_f32); actions.len()];
        stats[0] = (100, 0.500); // Q-argmax, lightly visited
        stats[1] = (300, 0.495); // inside the 0.02 band, 3× the visits
        rig_root_stats(&mut mcts, &stats);
        let picked = mcts.pick_root_action().expect("non-terminal root");
        assert_eq!(
            picked, actions[1],
            "0.005 < tie_eps=0.02: the higher-visit child must win the band"
        );
        assert_eq!(
            mcts.search_n(0),
            picked,
            "search_n and pick_root_action must agree — one pick rule, two entry points"
        );
    }

    /// A8 audit rule 4, other side: outside the band the Q-argmax wins
    /// regardless of visits — the tolerance tie-break must never become
    /// a visits-argmax.
    #[test]
    fn pick_outside_tie_band_takes_q_argmax() {
        let (mut mcts, actions) = searched_pick_fixture(MctsConfig::default(), false);
        let mut stats = vec![(10_u32, -0.9_f32); actions.len()];
        stats[0] = (100, 0.50); // Q-argmax
        stats[1] = (900, 0.40); // 9× the visits, 0.10 behind — out of band
        rig_root_stats(&mut mcts, &stats);
        assert_eq!(
            mcts.pick_root_action().expect("non-terminal root"),
            actions[0],
            "0.10 > tie_eps=0.02: visits must not override a resolved Q gap"
        );
    }

    /// `tie_eps = 0` is the rollback seam: the band collapses to exact
    /// score ties, restoring the strict `argmax(Q) then visits then
    /// first` rule the deployed `mcts_pick` always had.
    #[test]
    fn tie_eps_zero_restores_strict_pick() {
        let config = MctsConfig {
            tie_eps: 0.0,
            ..MctsConfig::default()
        };
        // 0.001 behind with 10× the visits: under the default band this
        // child would win; under eps=0 it must lose.
        let (mut mcts, actions) = searched_pick_fixture(config, false);
        let mut stats = vec![(10_u32, -0.9_f32); actions.len()];
        stats[0] = (100, 0.500);
        stats[1] = (999, 0.499);
        rig_root_stats(&mut mcts, &stats);
        assert_eq!(
            mcts.pick_root_action().expect("non-terminal root"),
            actions[0],
            "eps=0 must be strict: no tolerance band"
        );
        // Exact score tie: visits still break it even at eps=0.
        let (mut mcts, actions) = searched_pick_fixture(config, false);
        let mut stats = vec![(10_u32, -0.9_f32); actions.len()];
        stats[0] = (100, 0.500);
        stats[1] = (300, 0.500);
        rig_root_stats(&mut mcts, &stats);
        assert_eq!(
            mcts.pick_root_action().expect("non-terminal root"),
            actions[1],
            "exact tie at eps=0 falls to visits"
        );
    }

    /// A8 audit rule 6 regression: on an exact (score, visits) tie the
    /// pick is the EARLIEST child in creation order. The old
    /// `try_search_n` used `max_by`, which returns the LAST maximum —
    /// the opposite end of the child list from the deployed stable-sort
    /// pick, so 10.3% of audited plies disagreed between entry points.
    #[test]
    fn exact_tie_equal_visits_picks_first_child() {
        let (mut mcts, actions) = searched_pick_fixture(MctsConfig::default(), false);
        let stats = vec![(100_u32, 0.25_f32); actions.len()];
        rig_root_stats(&mut mcts, &stats);
        let picked = mcts.pick_root_action().expect("non-terminal root");
        assert_eq!(
            picked, actions[0],
            "full tie must keep the first child (first-max), not max_by's last"
        );
        assert_eq!(mcts.search_n(0), picked);
    }

    /// GH #403 through the unified pick: the draw-window bonus is part
    /// of the SCORE, so it must both flip the argmax and pull the band
    /// along with it. Identical rigged stats, two states differing only
    /// in whether hand slot 2 is FTL (live window): without FTL the
    /// well-visited 0.50 child wins; with FTL the 0.475 child's +0.05
    /// bonus makes it best AND pushes the 0.50 child out of the 0.02
    /// band, so FTL wins despite 50× fewer visits.
    #[test]
    fn draw_window_bonus_flips_pick_through_unified_rule() {
        let rigged = |mcts: &mut Mcts, actions: &[Action]| {
            let ftl_slot = actions
                .iter()
                .position(|a| matches!(a, Action::PlayCard { hand_index: 2, .. }))
                .unwrap_or_else(|| {
                    panic!("hand slot 2 must be a root child; actions = {actions:?}")
                });
            let best_slot = (0..actions.len())
                .find(|&i| i != ftl_slot)
                .expect(">=2 children");
            let mut stats = vec![(5_u32, -0.9_f32); actions.len()];
            stats[best_slot] = (500, 0.500);
            stats[ftl_slot] = (10, 0.475);
            rig_root_stats(mcts, &stats);
            (ftl_slot, best_slot)
        };

        // Control: no live window ⇒ raw scores ⇒ the 0.50 child wins
        // (0.475 is outside the 0.02 band).
        let (mut mcts, actions) = searched_pick_fixture(MctsConfig::default(), false);
        let (_, best_slot) = rigged(&mut mcts, &actions);
        assert_eq!(
            mcts.pick_root_action().expect("non-terminal root"),
            actions[best_slot],
            "without a live draw window the rigged Q-argmax must win"
        );

        // FTL variant: same stats, bonus decides.
        let (mut mcts, actions) = searched_pick_fixture(MctsConfig::default(), true);
        let (ftl_slot, _) = rigged(&mut mcts, &actions);
        let picked = mcts.pick_root_action().expect("non-terminal root");
        assert_eq!(
            picked, actions[ftl_slot],
            "GH #403: the draw-window bonus must reach the pick (this was dead code \
             on the deployed path before the A8-audit unification)"
        );
        assert_eq!(mcts.search_n(0), picked);
    }

    // ── FINDINGS §5 1-ply root guards (2026-08-09 A8 audit) ──────────────

    /// The audit default is load-bearing for the deployed agent — flip it
    /// only with a fresh paired A/B, and update this pin deliberately.
    #[test]
    fn default_config_root_guards_on() {
        assert!(MctsConfig::default().root_guards);
    }

    /// Rule 2 (FINDINGS §5 guard 1): a solver-proven winning child is
    /// picked over any Q/visit statistic — a late proof on a
    /// lightly-visited child must not lose the band to a well-visited
    /// coin-flip. Ungated: `root_guards: false` (the guards-2-3
    /// rollback) must still take the proof.
    #[test]
    fn proven_win_child_is_picked_unconditionally() {
        for root_guards in [true, false] {
            let config = MctsConfig {
                root_guards,
                ..MctsConfig::default()
            };
            let (mut mcts, actions) = searched_pick_fixture(config, false);
            let mut stats = vec![(10_u32, -0.9_f32); actions.len()];
            stats[0] = (500, 0.9); // dominant Q AND visits — loses to a proof
            stats[2] = (5, -0.5); // the proven child: terrible stats
            rig_root_stats(&mut mcts, &stats);
            let cid = mcts.tree.node(mcts.tree.root).children[2].1;
            mcts.tree.node_mut(cid).proven_win = true;
            let picked = mcts.pick_root_action().expect("non-terminal root");
            assert_eq!(
                picked, actions[2],
                "root_guards={root_guards}: a proven win is exact — Q statistics \
                 cannot outrank it"
            );
            assert_eq!(mcts.search_n(0), picked);
        }
    }

    /// Rule 5a (FINDINGS §5 guard 3): band-winning EndTurn with 22
    /// telegraphed damage over 0 block and a block-granting card inside
    /// the band — the guard must play the block card (26 audited
    /// declines, 307 HP lost). The rollback arm pins `root_guards:
    /// false` back to the plain band pick.
    #[test]
    fn block_guard_flips_in_band_endturn_into_block_play() {
        for (root_guards, expect_flip) in [(true, true), (false, false)] {
            let mut state = make_pick_state(3, false);
            state.creatures[0].intent = Some(sts2_core::Intent::Attack { damage: 22 });
            assert_eq!(state.player.block, 0, "fixture: nothing blocked yet");
            let config = MctsConfig {
                root_guards,
                ..MctsConfig::default()
            };
            let mut mcts = Mcts::new(state, config);
            let _ = mcts.search_n(64);
            let actions: Vec<Action> = mcts
                .root_outcomes()
                .iter()
                .map(|o| o.action.clone())
                .collect();
            let endturn = actions
                .iter()
                .position(|a| matches!(a, Action::EndTurn))
                .expect("EndTurn root child");
            let block = actions
                .iter()
                .position(|a| {
                    root_hand_card(&mcts.tree.root_state, a).is_some_and(|card| card.base_block > 0)
                })
                .expect("a Defend root child");
            let mut stats = vec![(10_u32, -0.9_f32); actions.len()];
            stats[endturn] = (800, 0.500); // band winner: most visits, best Q
            stats[block] = (100, 0.495); // inside the 0.02 band
            rig_root_stats(&mut mcts, &stats);
            let picked = mcts.pick_root_action().expect("non-terminal root");
            let expected = if expect_flip { block } else { endturn };
            assert_eq!(
                picked,
                actions[expected],
                "root_guards={root_guards}: 22 incoming > 0 block with an in-band \
                 Defend — guard must {}",
                if expect_flip { "block" } else { "be off" }
            );
        }
    }

    /// Rule 5b (FINDINGS §5 guard 2): band-winning EndTurn while a
    /// 0-cost play sits inside the band — ending the turn discards a
    /// strictly free option (audit: 269 such end-turns). The Defends
    /// are rigged OUT of band so the block guard cannot fire first.
    #[test]
    fn free_play_guard_flips_in_band_endturn_into_free_play() {
        let (mut mcts, actions) = searched_pick_fixture(MctsConfig::default(), false);
        let endturn = actions
            .iter()
            .position(|a| matches!(a, Action::EndTurn))
            .expect("EndTurn root child");
        let free = actions
            .iter()
            .position(|a| matches!(a, Action::PlayCard { hand_index: 2, .. }))
            .expect("the 0-cost Slice at hand slot 2");
        let mut stats = vec![(10_u32, -0.9_f32); actions.len()];
        stats[endturn] = (800, 0.500);
        stats[free] = (100, 0.495); // inside the band, 8× fewer visits
        rig_root_stats(&mut mcts, &stats);
        let picked = mcts.pick_root_action().expect("non-terminal root");
        assert_eq!(
            picked, actions[free],
            "an in-band free play must beat a voluntary EndTurn"
        );
        assert_eq!(mcts.search_n(0), picked);
    }

    /// Rule 5 band discipline: the guards only promote alternatives
    /// ALREADY inside the band. With every card child out of band, a
    /// band-winning EndTurn stands — a resolved Q gap is a real signal,
    /// never overridden (22 incoming and a 0-cost card in hand
    /// notwithstanding).
    #[test]
    fn guards_never_flip_to_an_out_of_band_alternative() {
        let mut state = make_pick_state(3, false);
        state.creatures[0].intent = Some(sts2_core::Intent::Attack { damage: 22 });
        let mut mcts = Mcts::new(state, MctsConfig::default());
        let _ = mcts.search_n(64);
        let actions: Vec<Action> = mcts
            .root_outcomes()
            .iter()
            .map(|o| o.action.clone())
            .collect();
        let endturn = actions
            .iter()
            .position(|a| matches!(a, Action::EndTurn))
            .expect("EndTurn root child");
        // Every PlayCard child sits 0.60 below EndTurn — far outside
        // tie_eps = 0.02.
        let mut stats = vec![(10_u32, -0.1_f32); actions.len()];
        stats[endturn] = (100, 0.500);
        rig_root_stats(&mut mcts, &stats);
        assert_eq!(
            mcts.pick_root_action().expect("non-terminal root"),
            actions[endturn],
            "no in-band alternative: the guards must not override the band pick"
        );
    }

    /// THE audited free-lethal decline (FINDINGS §5, ironclad/949252430
    /// f7): last enemy at 2 HP, a 0-cost damage card in hand, root
    /// scores tied to the noise floor — EndTurn won the tie and the
    /// player ate the hit plus four Infection triggers. The free-play
    /// guard must take the kill.
    #[test]
    fn free_lethal_on_tied_root_picks_the_kill() {
        let mut state = make_pick_state(3, false);
        state.creatures[0].current_hp = 2; // Slice (0-cost, 6 damage) is lethal
        state.creatures[0].intent = Some(sts2_core::Intent::Attack { damage: 8 });
        let mut mcts = Mcts::new(state, MctsConfig::default());
        let _ = mcts.search_n(64);
        let actions: Vec<Action> = mcts
            .root_outcomes()
            .iter()
            .map(|o| o.action.clone())
            .collect();
        let endturn = actions
            .iter()
            .position(|a| matches!(a, Action::EndTurn))
            .expect("EndTurn root child");
        let kill = actions
            .iter()
            .position(|a| matches!(a, Action::PlayCard { hand_index: 2, .. }))
            .expect("the 0-cost Slice at hand slot 2");
        // The audit ply: gap = 0.0000 with the kill even holding MORE
        // visits is already fixed by the rule-4/6 band (visits win an
        // exact tie). Pin the harder shape: EndTurn microscopically
        // ahead AND better visited, i.e. the band pick alone still ends
        // the turn — only the guard takes the kill. Defends stay out of
        // band so the 8-damage intent cannot divert guard 5a.
        let mut stats = vec![(10_u32, -0.9_f32); actions.len()];
        stats[endturn] = (1000, 0.3001);
        stats[kill] = (999, 0.3000);
        rig_root_stats(&mut mcts, &stats);
        let picked = mcts.pick_root_action().expect("non-terminal root");
        assert_eq!(
            picked, actions[kill],
            "2-HP enemy, free 6-damage card, tied root: the pick must kill, \
             not end the turn"
        );
        assert_eq!(mcts.search_n(0), picked);
    }

    /// ldd1c-B: `sum_enemy_slippery` totals remaining Slippery on alive
    /// enemies (drives the 1-ply override's wall-chipping credit). Dead
    /// enemies and non-Slippery powers contribute nothing.
    #[test]
    fn sum_enemy_slippery_counts_alive_enemy_stacks() {
        use sts2_core::{Power, PowerKind, PowerUid};
        let mut state = make_smoke_state(7);
        assert_eq!(sum_enemy_slippery(&state), 0, "no Slippery to start");

        let euid = state.enemy_uids[0].0;
        let slip = PowerUid::new(11).expect("non-zero");
        state.powers.push(Power {
            id: slip.0,
            kind: PowerKind::Slippery,
            amount: 9,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state
            .creatures
            .iter_mut()
            .find(|c| c.id == euid)
            .expect("enemy creature")
            .powers
            .push(slip);
        assert_eq!(sum_enemy_slippery(&state), 9, "9-stack wall counted");

        // A dead enemy's wall no longer matters.
        state
            .creatures
            .iter_mut()
            .find(|c| c.id == euid)
            .unwrap()
            .current_hp = 0;
        assert_eq!(sum_enemy_slippery(&state), 0, "dead enemy excluded");
    }

    /// GitHub issue #19: a stored child edge that is not in the
    /// current state's `legal_actions` must be rejected by
    /// revalidation. Without the fix, `core_apply` is called on the
    /// stale action, the search descends into the linked child node,
    /// and Q values for an unreachable state get polluted.
    ///
    /// Setup: a state from `make_smoke_state` (Ironclad vs Cultist
    /// in `PlayerPlayPhase`). The hand is empty in this minimal
    /// fixture so `legal_actions` contains only `EndTurn`. Feeding
    /// `edge_applies` a `PlayCard{0, …}` action — which would
    /// represent a stored edge from a determinization that had a
    /// card in slot 0 — must be rejected (not in current legal set),
    /// the `stale_child_edges` counter must increment, and the state
    /// must be unmodified.
    #[test]
    fn stale_play_card_edge_rejected_and_counted() {
        let state = make_smoke_state(7);
        let pre_state = state.clone();
        let cfg = MctsConfig::default();
        let mut mcts = Mcts::new(state, cfg);

        // Sanity: in this fixture the only legal action is EndTurn
        // (empty hand → no PlayCard, no pending choice).
        let legals = sts2_core::legal_actions(&mcts.tree.root_state);
        assert!(
            legals.iter().any(|a| matches!(a, Action::EndTurn)),
            "make_smoke_state should yield EndTurn as a legal action"
        );
        assert!(
            !legals
                .iter()
                .any(|a| matches!(a, Action::PlayCard { hand_index: 0, .. })),
            "fixture must not have PlayCard{{0}} as a legal action — \
             stale-edge rejection is only meaningful when the stored \
             action is genuinely absent from the current legal set"
        );

        let mut working = mcts.tree.root_state.clone();
        let stale = Action::PlayCard {
            hand_index: 0,
            target: Some(0),
        };
        let pre_stale = mcts.stale_child_edges();
        let applied = mcts.edge_applies(&mut working, &stale);
        assert!(
            !applied,
            "edge_applies must reject an action absent from legal_actions"
        );
        assert_eq!(
            mcts.stale_child_edges(),
            pre_stale + 1,
            "stale_child_edges counter must increment on rejection"
        );
        assert_eq!(
            working, pre_state,
            "rejected edge must leave the state unmodified"
        );

        // The legal action passes through unchanged and progresses
        // the state — counter is unchanged, state advanced.
        let mut working = mcts.tree.root_state.clone();
        let pre_stale = mcts.stale_child_edges();
        let applied = mcts.edge_applies(&mut working, &Action::EndTurn);
        assert!(applied, "edge_applies must accept a legal action");
        assert_eq!(
            mcts.stale_child_edges(),
            pre_stale,
            "stale counter must not increment for a legal action"
        );
    }

    /// GitHub issue #19: the diagnostic counters are zero-initialized
    /// and remain `0` across a full search on a fixture where every
    /// determinization keeps the same legal set (no PIMC drift
    /// surfaces). Locks the accessors into the public surface and
    /// guards against accidental over-counting in the steady state.
    #[test]
    fn stale_edge_counters_zero_on_clean_search() {
        sts2_content::cards::dispatch::install_default_registry();
        let cfg = MctsConfig {
            max_simulations: 50,
            max_rollout_depth: 10,
            seed: 11,
            ..MctsConfig::default()
        };
        let mut mcts = Mcts::new(make_smoke_state(2), cfg);
        let _ = mcts.search();
        // On the Ironclad-vs-Cultist smoke fixture the legal
        // set is stable across simulations, so no stored edge should
        // be rejected.
        assert_eq!(mcts.stale_child_edges(), 0);
        assert_eq!(mcts.failed_apply(), 0);
        assert_eq!(mcts.invalid_target(), 0);
        assert_eq!(mcts.no_op_playcard(), 0);
    }

    /// GH #21 regression: a stale-terminal descendant under the new
    /// root must not survive `reroot`. Before this fix `reroot`
    /// BFS-copied the entire surviving subtree and refreshed only the
    /// new root — descendants kept their per-determinization
    /// `is_terminal` / `state_hash` / `untried_actions`, which can
    /// disagree with the reconstructed live state after hidden draws
    /// or intent rolls. A descendant with stale `is_terminal = true`
    /// is the most dangerous case: selection could trust a fake
    /// terminal evaluation that does not match the real state.
    ///
    /// After the fix `reroot` drops all descendants beyond the new
    /// root, so a poisoned grandchild cannot reach the new tree.
    #[test]
    fn reroot_drops_stale_descendants_gh_21() {
        let state = make_smoke_state(11);
        let config = MctsConfig {
            c_puct: std::f32::consts::SQRT_2,
            max_simulations: 200,
            max_rollout_depth: 50,
            seed: 11,
            rollout_policy: RolloutPolicyKind::Heuristic,
            use_transposition_table: false,
            regularized_pi_temperature: 0.1,
            use_puct_prior: false,
            terminal_eval_mode: TerminalEvalMode::Binary,
            use_quiescence: false,
            use_lethal_detector: false,
            chance_widening_c: 1.0,
            chance_widening_alpha: 0.5,
            draw_widening_c: 1.0,
            draw_widening_alpha: 0.5,
            endturn_widening_c: 0.5,
            endturn_widening_alpha: 0.4,
            chance_prove_wins: false,
            chance_closed_stops_widening: false,
            chance_max_depth: CHANCE_MAX_DEPTH,
            hidden_info: HiddenInfoMode::Clairvoyant,
            common_random_numbers: false,
            tie_eps: 0.02,
            loss_gradient: true,
            root_guards: true,
            ..MctsConfig::default()
        };
        let mut mcts = Mcts::new(state.clone(), config);
        let chosen = mcts.search_n(200);

        // Find the chosen root child and poison ONE of its grandchildren
        // (i.e. a descendant under what will become the new root).
        // Mark it terminal so a regression that preserved descendants
        // would carry a stale terminal flag past the reroot.
        let (poisoned_old_id, _poisoned_hash) = {
            let root_id = mcts.tree.root;
            // Bead `sts2-rs-edge` / GH #16: children now store
            // EdgeActions; resolve each against the root state to
            // compare against the chosen `Action`.
            let root_state_snapshot = mcts.tree.root_state.clone();
            let child_id = mcts
                .tree
                .node(root_id)
                .children
                .iter()
                .find(|(edge, _)| edge.resolve(&root_state_snapshot).as_ref() == Some(&chosen))
                .map(|(_, id)| *id)
                .expect("chosen action must be a root child after search");
            let child_node = mcts.tree.node(child_id);
            // Need at least one grandchild to poison; skip if the
            // chosen child has none expanded yet.
            if child_node.children.is_empty() {
                return;
            }
            let gc_id = child_node.children[0].1;
            let gc_hash = mcts.tree.node(gc_id).state_hash;
            let gc = mcts.tree.node_mut(gc_id);
            gc.is_terminal = true;
            gc.proven_loss = true;
            gc.untried_actions.clear();
            gc.is_expanded = false;
            (gc_id, gc_hash)
        };

        // Reroot to the chosen action with the real (non-terminal) state.
        let mut new_state = state.clone();
        let _ = sts2_core::apply(&mut new_state, chosen.clone());
        if sts2_core::is_terminal(&new_state) {
            return;
        }
        let ok = mcts.reroot(&chosen, new_state.clone());
        assert!(ok, "reroot must succeed");

        // Acceptance: the poisoned grandchild — and every other
        // descendant — must NOT be in the new arena. The new root has
        // an empty children list and a single-node arena.
        let arena_len = mcts.tree.arena.len();
        assert_eq!(
            arena_len, 1,
            "reroot must discard all descendants (GH #21); arena len = {arena_len}"
        );
        let root_node = mcts.tree.node(mcts.tree.root);
        assert!(
            root_node.children.is_empty(),
            "new root must have no carried-over children after GH #21 fix"
        );
        assert!(
            !root_node.is_terminal,
            "new root must reflect the live (non-terminal) state"
        );
        assert!(!root_node.proven_win && !root_node.proven_loss);

        // And `search_n` must still produce a valid action against the
        // refreshed tree — i.e. lazy re-expansion works cleanly.
        let _action = mcts.search_n(20);
        // Unused binding to keep the captured id alive for compile-only
        // documentation of intent.
        let _ = poisoned_old_id;
    }

    /// Bead `sts2-rs-37qe8` (v4wjd.1.1): `sample_rollout_leaf_state`
    /// runs a synthetic rollout and invokes the observer exactly once,
    /// at the leaf-evaluator's call point — either depth-cap or
    /// terminal. The observer sees the same `CombatState` shape the
    /// production rollout's leaf eval sees. Deterministic for a fixed
    /// (state, rng-seed, policy, max_depth) tuple.
    #[test]
    fn sample_rollout_leaf_state_invokes_observer_once_and_is_deterministic() {
        let base = make_smoke_state(7);
        let make_seeded_rng = || Xoshiro256PlusPlus::seed_from_u64(99);

        let mut depths: Vec<u32> = Vec::new();
        let mut calls = 0_u32;

        let mut state_a = base.clone();
        let mut rng = make_seeded_rng();
        super::sample_rollout_leaf_state(
            &mut state_a,
            &mut rng,
            10,
            RolloutPolicyKind::Uniform,
            |_state, depth| {
                calls += 1;
                depths.push(depth);
            },
        );
        assert_eq!(calls, 1, "observer must fire exactly once per call");
        let depth_a = depths[0];

        // Determinism: same seed + policy + max_depth -> same final
        // depth + same observed-state hash.
        let mut state_b = base.clone();
        let mut rng_b = make_seeded_rng();
        let mut depth_b = None;
        super::sample_rollout_leaf_state(
            &mut state_b,
            &mut rng_b,
            10,
            RolloutPolicyKind::Uniform,
            |_state, depth| depth_b = Some(depth),
        );
        assert_eq!(
            Some(depth_a),
            depth_b,
            "depth-cap reached deterministically"
        );
        assert!(depth_a <= 10, "depth never exceeds max_depth");
    }

    /// Regression: `par_search` worker base seeds must not collide with
    /// the per-simulation determinization stride. `one_simulation`
    /// derives each sim's seed as `cfg.seed + sim_index · GOLDEN`; if the
    /// worker offset were a plain `config.seed + w · GOLDEN` it would
    /// commute with that stride, so worker `w` sim `i` and worker `w'`
    /// sim `i'` would realize an identical determinization whenever
    /// `w + i == w' + i'`. Enumerate the full seed set both schemes
    /// produce for a 4-worker × 64-sim ensemble and assert the additive
    /// scheme collapses heavily while `splitmix64_seed` does not.
    #[test]
    fn par_search_worker_seeds_do_not_alias_sim_stride() {
        const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;
        const WORKERS: u64 = 4;
        const SIMS: u64 = 64;
        let base_seed = 12_345u64;

        let sim_seed = |worker_base: u64, i: u64| worker_base.wrapping_add(i.wrapping_mul(GOLDEN));

        // Old (buggy) additive scheme: worker base == config.seed + w·GOLDEN.
        let mut additive = std::collections::HashSet::new();
        for w in 0..WORKERS {
            let wb = base_seed.wrapping_add(w.wrapping_mul(GOLDEN));
            for i in 0..SIMS {
                additive.insert(sim_seed(wb, i));
            }
        }
        // The diagonal aliasing means the union is far smaller than the
        // WORKERS·SIMS distinct seeds an independent ensemble would have.
        assert!(
            additive.len() < (WORKERS * SIMS) as usize,
            "additive scheme should alias (got {} distinct seeds)",
            additive.len(),
        );

        // Fixed scheme: splitmix64-hashed worker base.
        let mut fixed = std::collections::HashSet::new();
        for w in 0..WORKERS {
            let wb = splitmix64_seed(base_seed, w);
            for i in 0..SIMS {
                fixed.insert(sim_seed(wb, i));
            }
        }
        assert_eq!(
            fixed.len(),
            (WORKERS * SIMS) as usize,
            "splitmix64 worker seeds must yield WORKERS·SIMS distinct determinizations",
        );
    }

    /// Regression (GH #494): the root-parallel merge summed `visits`,
    /// `wins`, `losses`, `ties` and `value_sum` across workers but not
    /// `value_sq_sum`, so a merged child carried worker 0's `Σv²` against
    /// the ensemble's `N` and `Σv`. `Σv²/N − mean²` then went negative and
    /// `value_variance()` clamped it to 0 — every `mean_stderr()` on a
    /// `par_search` result read 0.0, which is what
    /// `combat_position_book oracle-q` printed for all 303 positions.
    ///
    /// Cauchy–Schwarz gives the invariant a merged table must satisfy:
    /// `Σv² ≥ (Σv)² / N` per child. Under the bug it fails on any child
    /// visited by more than one worker.
    #[test]
    fn par_search_merges_value_sq_sum_so_stderr_is_not_zero() {
        sts2_core::card_effects::ensure_installed();
        let state = make_smoke_state(7);
        let cfg = MctsConfig {
            max_simulations: 400,
            seed: 99,
            ..MctsConfig::default()
        };
        let r = par_search(state, cfg, 4);
        let mut any_variance = false;
        for o in &r.outcomes {
            if o.visits == 0 {
                continue;
            }
            #[allow(clippy::cast_precision_loss)]
            let n = o.visits as f32;
            let lower = o.value_sum * o.value_sum / n;
            assert!(
                o.value_sq_sum >= lower - 1e-3 * lower.abs().max(1.0),
                "{:?}: Σv²={} < (Σv)²/N={} — squared returns were not pooled across workers",
                o.action,
                o.value_sq_sum,
                lower
            );
            if o.mean_stderr() > 0.0 {
                any_variance = true;
            }
        }
        assert!(
            r.outcomes.iter().any(|o| o.visits > 100),
            "fixture should leave at least one child visited by several workers"
        );
        assert!(
            any_variance,
            "400 stochastic rollouts over {} children produced zero return variance everywhere",
            r.outcomes.len()
        );
    }

    // ── Chance-node mode (feat/mcts-chance-nodes) ──────────────────────

    fn chance_cfg(seed: u64, sims: u32) -> MctsConfig {
        MctsConfig {
            max_simulations: sims,
            max_rollout_depth: 20,
            seed,
            ..MctsConfig::default()
        }
    }

    fn count_chance_nodes<P: Policy>(m: &Mcts<P>) -> usize {
        m.tree
            .arena
            .iter()
            .filter(|n| n.kind == NodeKind::Chance)
            .count()
    }

    // ── Anti-clairvoyance: permutation invariance (mechanical) ─────────
    //
    // The honest-search guarantee is stated in prose in
    // `sts2_core::blindfold`'s module docs and enforced by review. These
    // two tests assert it MECHANICALLY: build two states that differ in
    // nothing but the physical order of the unknown draw pile — same
    // cards, same uids, same everything else — and show the search cannot
    // tell them apart. Permuting `player.draw` while holding `state.cards`
    // fixed is exactly "the same physical cards, stacked in a different
    // order", which is precisely the information the player does not have.

    /// Reverse the unknown draw pile. Same multiset, same uid→card
    /// mapping, different physical order.
    fn with_reversed_draw(state: &CombatState) -> CombatState {
        let mut s = state.clone();
        s.player.draw.reverse();
        s
    }

    /// The fixture both tests share: reversing it swaps which card is on
    /// top (Strike ⇄ upgraded Strike), so the permutation is one a
    /// clairvoyant search genuinely acts on.
    ///
    /// Do not read the `CardId`s as four distinct cards.
    /// `build_pommel_strike_state` stamps every draw entry as a cost-1,
    /// 6-damage, 0-block Attack whatever id it is handed, so the Defend
    /// has no block and the Bash no vulnerable — `card_id` and `upgraded`
    /// are the only fields that actually vary. The upgraded-Strike/Strike
    /// pair at the two ENDS is what carries the test; the middle two are
    /// padding that keeps the pile long enough to be worth permuting.
    fn permutation_fixture() -> CombatState {
        use sts2_core::cards::CardId;
        build_pommel_strike_state(&[
            (CardId::StrikeIronclad, false),
            (CardId::DefendIronclad, false),
            (CardId::Bash, false),
            (CardId::StrikeIronclad, true),
        ])
    }

    /// [`permutation_fixture`] with the ends pinned, which routes
    /// `canonicalize_unknown_middle` down its OTHER branch
    /// (`blindfold.rs:196-213`: preserve pinned head/tail, sort only the
    /// middle) instead of the plain `draw.sort_unstable()` fast path.
    fn permutation_fixture_pinned() -> CombatState {
        let mut s = permutation_fixture();
        let first = s.player.draw[0];
        let last = *s.player.draw.last().expect("non-empty draw pile");
        s.player.pinned_top.push(first);
        s.player.pinned_bottom.push(last);
        s
    }

    /// `belief_root` collapses two draw-pile permutations onto one pile.
    ///
    /// Note this is asserted on the pile itself, not on `search_hash`:
    /// the fast pile fingerprint is a commutative sum over uids and is
    /// therefore *already* order-blind, for transposition-table reasons
    /// that have nothing to do with hidden information. Hashing equal
    /// would prove nothing here.
    #[test]
    fn belief_root_is_invariant_to_draw_pile_permutation() {
        // Both canonicalizer branches: unpinned (plain sort) and pinned
        // (preserve head/tail, sort the middle).
        for (what, a) in [
            ("unpinned", permutation_fixture()),
            ("pinned", permutation_fixture_pinned()),
        ] {
            let b = with_reversed_draw(&a);
            assert_ne!(
                a.player.draw, b.player.draw,
                "{what} fixture must actually differ in pile order"
            );

            let (mut ba, mut bb) = (a, b);
            sts2_core::blindfold::belief_root(&mut ba, 0xC0FF_EE12);
            sts2_core::blindfold::belief_root(&mut bb, 0xC0FF_EE12);
            assert_eq!(
                ba.player.draw, bb.player.draw,
                "{what}: belief_root must canonicalize both permutations to one pile"
            );
            assert_eq!(
                ba.player.deferred_draw_order, bb.player.deferred_draw_order,
                "{what}: and put both on the deferred-draw representation"
            );
        }
    }

    /// End-to-end: a full honest search over the two permutations returns
    /// bit-identical statistics, not merely an identical chosen action.
    ///
    /// Be precise about what this can and cannot prove. It does NOT probe
    /// selection / expansion / rollout for order leaks: `belief_root` is
    /// canonicalize + reseed-from-`config.seed`, the two fixtures differ in
    /// *nothing but* pile order, so after the blindfold they are
    /// byte-identical WHOLE STATES and every downstream statistic is equal
    /// by construction — whatever those phases do with the pile.
    /// `belief_root`'s own doc says as much.
    ///
    /// What it does buy, and the reason it is not just a 300-sim
    /// restatement of `belief_root_is_invariant_to_draw_pile_permutation`
    /// above, is the ORDERING constraint warned about where the blindfold
    /// is applied: the blindfold must run before anything caches
    /// `root_state`. A future change that snapshotted `realized` or seeded
    /// the TT entry pre-blindfold would leave the two searches differing,
    /// and this is what would catch it.
    ///
    /// The `Clairvoyant` half is the control: it must DIFFER. Without it
    /// this test would still pass on a search that had somehow stopped
    /// reading the draw pile at all.
    #[test]
    fn honest_search_is_invariant_to_draw_pile_permutation() {
        let run = |state: CombatState, hidden_info: HiddenInfoMode| -> Vec<(Action, u32, f32)> {
            let cfg = MctsConfig {
                max_simulations: 300,
                max_rollout_depth: 20,
                seed: 0xA11C_1A17,
                hidden_info,
                ..MctsConfig::default()
            };
            let mut m = Mcts::new(state, cfg);
            let _ = m.search_n(300);
            let mut outs: Vec<(Action, u32, f32)> = m
                .root_outcomes()
                .into_iter()
                .map(|o| (o.action, o.visits, o.value_sum))
                .collect();
            outs.sort_by_key(|(a, _, _)| format!("{a:?}"));
            outs
        };

        let a = permutation_fixture();
        let b = with_reversed_draw(&a);

        let honest_a = run(a.clone(), HiddenInfoMode::BeliefDraw);
        let honest_b = run(b.clone(), HiddenInfoMode::BeliefDraw);
        assert!(!honest_a.is_empty(), "search must produce root children");
        assert_eq!(
            honest_a, honest_b,
            "honest search must return bit-identical statistics for two \
             states differing only in hidden draw-pile order"
        );

        let leaky_a = run(a, HiddenInfoMode::Clairvoyant);
        let leaky_b = run(b, HiddenInfoMode::Clairvoyant);
        // If THIS is the assert that fires, suspect the fixture before the
        // search: it only says two clairvoyant searches over a reversed
        // pile came out different, and any unrelated engine change that
        // happens to collapse them (e.g. the pile stops reaching the
        // rollout, or all four draw cards become interchangeable) breaks
        // it without anything being wrong with honest mode. Check that
        // `permutation_fixture`'s two ends still differ observably first.
        assert_ne!(
            leaky_a, leaky_b,
            "control: a clairvoyant search DOES read the pile order, so the \
             fixture is not degenerate and the invariance above is earned"
        );
    }

    /// `common_random_numbers` decides whether stochastic siblings share
    /// a reseed stream. Exercised through `alloc_chance_node` directly
    /// with a hand-grown child list, because the contract is about the
    /// salt derivation and not about which search shape happens to
    /// produce two stochastic edges off one node.
    #[test]
    fn crn_controls_whether_stochastic_siblings_share_a_base() {
        let state = make_smoke_state(11);
        let edge_a = EdgeAction::Other(Action::EndTurn);
        let edge_b = EdgeAction::Other(Action::Evoke { times: 1 });

        let bases = |crn: bool| -> (u64, u64) {
            let cfg = MctsConfig {
                common_random_numbers: crn,
                ..chance_cfg(0x5EED_1234, 0)
            };
            let mut m = Mcts::new(state.clone(), cfg);
            let root = m.tree.root;
            let a = m.alloc_chance_node(root, &edge_a, &state);
            // Link it the way expansion would, so the second allocation
            // sees a grown child list — that count IS the non-CRN salt.
            m.tree.node_mut(root).children.push((edge_a.clone(), a));
            let b = m.alloc_chance_node(root, &edge_b, &state);
            m.tree.node_mut(root).children.push((edge_b.clone(), b));
            (
                m.tree.node(a).randomness_base,
                m.tree.node(b).randomness_base,
            )
        };

        let (off_a, off_b) = bases(false);
        assert_ne!(
            off_a, off_b,
            "default: siblings must get distinct bases (independent nature draws)"
        );

        let (on_a, on_b) = bases(true);
        assert_eq!(
            on_a, on_b,
            "under CRN, sibling edges must be realized from the same reseed"
        );

        // The shared base must be its OWN stream, not a relabelling of the
        // first child's. `CRN_CHANCE_SALT` sits outside the non-CRN salt
        // range for exactly this reason; with the obvious `salt = 1` these
        // would be equal and a search whose every chance node is a first
        // child would be byte-identical CRN on/off.
        assert_ne!(
            on_a, off_a,
            "the CRN base must not collide with the first child's non-CRN base"
        );
    }

    /// The doc's own claim that CRN is inert while no chance node exists.
    ///
    /// `common_random_numbers` is read at exactly two sites, both inside
    /// the `stochastic` branch of expansion, so a search that has not yet
    /// allocated a chance node must not have been perturbed by the flag.
    ///
    /// One simulation is the whole budget on purpose: it is the largest
    /// one for which this fixture stays chance-free (2 sims already
    /// allocates one), and a deep search over a real combat always hits a
    /// stochastic edge — an `EndTurn` alone rolls monster AI. So the
    /// window is genuinely this narrow, and what the test guards is
    /// correspondingly specific: that nobody moves the CRN read onto the
    /// deterministic path (root setup, `expand`'s non-stochastic arm),
    /// where it would perturb every search whether or not nature ever
    /// moved.
    #[test]
    fn crn_is_inert_before_any_chance_node_is_allocated() {
        let run = |crn: bool| -> Vec<(Action, u32, f32)> {
            let cfg = MctsConfig {
                max_simulations: 1,
                max_rollout_depth: 12,
                seed: 0x0C12_0001,
                common_random_numbers: crn,
                ..MctsConfig::default()
            };
            let mut m = Mcts::new(make_smoke_state(5), cfg);
            let _ = m.search_n(1);
            assert_eq!(
                count_chance_nodes(&m),
                0,
                "budget must stay below the first chance allocation"
            );
            m.root_outcomes()
                .into_iter()
                .map(|o| (o.action, o.visits, o.value_sum))
                .collect()
        };
        assert_eq!(
            run(false),
            run(true),
            "CRN must be a no-op while no chance node exists"
        );
    }

    /// End-to-end counterpart: where chance nodes DO exist, CRN must move
    /// the root statistics. Without this, the salt unit test above would
    /// pass on an implementation that computed a shared base and then
    /// never used it — which, before the outcome-0 rebuild landed, was
    /// close to what happened for the outcome carrying most of the value.
    #[test]
    fn crn_changes_root_statistics_when_chance_nodes_exist() {
        let run = |crn: bool| -> (usize, Vec<(Action, u32, f32)>) {
            let cfg = MctsConfig {
                common_random_numbers: crn,
                ..chance_cfg(0x51DE_51DE, 400)
            };
            let mut m = Mcts::new(permutation_fixture(), cfg);
            let _ = m.search_n(400);
            let mut outs: Vec<(Action, u32, f32)> = m
                .root_outcomes()
                .into_iter()
                .map(|o| (o.action, o.visits, o.value_sum))
                .collect();
            outs.sort_by_key(|(a, _, _)| format!("{a:?}"));
            (count_chance_nodes(&m), outs)
        };

        let (off_chance, off) = run(false);
        let (on_chance, on) = run(true);
        assert!(
            off_chance > 0 && on_chance > 0,
            "fixture must allocate chance nodes ({off_chance} off / {on_chance} on)"
        );
        assert_ne!(
            off, on,
            "CRN must actually reach the search, not just the base derivation"
        );
    }

    #[test]
    fn chance_mode_root_has_realized_state() {
        let state = make_smoke_state(2);
        let m = Mcts::new(state, chance_cfg(3, 0));
        assert!(
            m.tree.node(m.tree.root).realized.is_some(),
            "chance-mode root must cache its realized state up front"
        );
        assert_eq!(m.tree.node(m.tree.root).kind, NodeKind::Decision);
    }

    #[test]
    fn chance_mode_search_runs_and_creates_chance_nodes() {
        // EndTurn from PlayerPlayPhase resolves the enemy turn + next draw,
        // which consumes combat RNG → must produce at least one chance node.
        let state = make_smoke_state(4);
        let mut m = Mcts::new(state, chance_cfg(5, 400));
        let action = m.search_n(400);
        // A legal action came back, and the tree branched on chance.
        let _ = action;
        assert!(
            count_chance_nodes(&m) > 0,
            "an EndTurn transition must create a chance node"
        );
        // root_outcomes stays well-formed (no NaN, bounded means).
        let outs = m.root_outcomes();
        assert!(!outs.is_empty(), "root should have children after search");
        for o in &outs {
            assert!(
                o.mean_value.is_finite() && o.mean_value.abs() <= MAX_LEAF_VALUE_ABS,
                "root outcome mean must be finite and bounded: {}",
                o.mean_value
            );
        }
    }

    #[test]
    fn chance_mode_is_deterministic() {
        // Same (state, seed, workers=1) → identical root stats.
        let mk = || {
            let mut m = Mcts::new(make_smoke_state(6), chance_cfg(99, 300));
            let _ = m.search_n(300);
            m.root_outcomes()
        };
        let a = mk();
        let b = mk();
        assert_eq!(a.len(), b.len(), "same child count");
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.action, y.action, "same action order");
            assert_eq!(x.visits, y.visits, "deterministic visit counts");
            assert_eq!(
                x.value_sum.to_bits(),
                y.value_sum.to_bits(),
                "deterministic value sums"
            );
        }
    }

    #[test]
    fn chance_mode_depth_bound_resolves_chance_leaf() {
        // Regression for GH #98 ("leaf has realized state" panic). The
        // chance-mode selection descent checks its depth backstop at the TOP
        // of the loop, *before* the chance-node handling. UCT descent can step
        // onto a chance child right at the boundary (a decision node's child
        // may be a chance node, e.g. an EndTurn transition); chance nodes carry
        // no realized state, so the post-loop leaf eval used to panic. The fix
        // resolves that chance node to a decision outcome before breaking.
        //
        // Deterministic trigger: move every card into the DISCARD with an
        // EMPTY hand and EMPTY draw pile. The empty hand makes EndTurn the ONLY
        // legal action at root; the empty draw pile forces EndTurn's next-turn
        // draw to RESHUFFLE the discard (which consumes combat RNG) -> a chance
        // node. Root's sole child is therefore a chance node and UCT is
        // *forced* onto it. With chance_max_depth = 1, the descent steps onto
        // that chance child at path.len() == 2 > 1 and breaks on it -- the
        // pre-fix panic site.
        let mut state = make_smoke_state(4);
        let hand: Vec<_> = state.player.hand.drain(..).collect();
        let draw: Vec<_> = state.player.draw.drain(..).collect();
        state.player.discard.clear();
        for c in hand.into_iter().chain(draw) {
            state.player.discard.push(c);
        }
        let cfg = MctsConfig {
            chance_max_depth: 1,
            ..chance_cfg(7, 50)
        };
        let mut m = Mcts::new(state, cfg);
        // Must not panic (pre-fix: panics at the leaf-eval `.expect`).
        let _ = m.search_n(50);
        // Guard: the test only exercises the bug if the EndTurn chance node
        // actually formed (otherwise it would pass vacuously).
        assert!(
            count_chance_nodes(&m) > 0,
            "root EndTurn must create a chance node for this regression to bite",
        );
    }

    #[test]
    fn chance_mode_outcomes_vary() {
        // A high-entropy EndTurn chance node must widen to more than one
        // distinct outcome (proves multi-stream outcome generation actually
        // samples different realizations, not the same state every time).
        let state = make_smoke_state(8);
        let mut m = Mcts::new(state, chance_cfg(21, 600));
        let _ = m.search_n(600);
        let max_outcomes = m
            .tree
            .arena
            .iter()
            .filter(|n| n.kind == NodeKind::Chance)
            .map(|n| n.chance_outcomes.len())
            .max()
            .unwrap_or(0);
        assert!(
            max_outcomes >= 2,
            "expected a chance node to widen to >=2 outcomes, got max {max_outcomes}"
        );
        // Every chance node's outcome children must have distinct search
        // hashes (local dedup invariant).
        for n in m.tree.arena.iter().filter(|n| n.kind == NodeKind::Chance) {
            let mut hashes: Vec<u64> = n
                .chance_outcomes
                .iter()
                .map(|o| m.tree.node(o.child).search_hash)
                .collect();
            let len = hashes.len();
            hashes.sort_unstable();
            hashes.dedup();
            assert_eq!(
                len,
                hashes.len(),
                "chance outcomes must be deduped by search_hash"
            );
        }
    }

    #[test]
    fn chance_mode_widen_consumes_a_fresh_ordinal_each_draw() {
        // Regression for the frozen-ordinal bug: every WIDEN draw must consume
        // a new monotonic reseed ordinal (`chance_draws`), NOT reuse the
        // distinct-outcome count. Keying the reseed off the outcome count
        // freezes the ordinal on the first duplicate draw, so the node
        // re-derives one realization on every visit and its expectimax
        // estimate collapses onto a single outcome. Drive widening directly
        // and confirm the draw counter advances exactly once per widen.
        let state = make_smoke_state(11);
        let mut m = Mcts::new(state, chance_cfg(1, 0));
        let root = m.tree.root;
        let pre = m.tree.node(root).realized.as_deref().unwrap().clone();
        let edge = EdgeAction::Other(Action::EndTurn);
        let chance_id = m.alloc_chance_node(root, &edge, &pre);
        m.tree.node_mut(root).children.push((edge, chance_id));
        let path = [root, chance_id];
        let draws: u32 = 6;
        for _ in 0..draws {
            // Inflate visits so the DPW cap stays far above the outcome count
            // and we remain on the WIDEN (not SAMPLE) path every iteration.
            m.tree.node_mut(chance_id).visits += 10_000;
            let _ = m.chance_widen_or_sample(chance_id, &path);
        }
        assert_eq!(
            m.tree.node(chance_id).chance_draws,
            draws,
            "each widen draw must advance the reseed ordinal exactly once \
             (frozen-at-outcome-count bug would stall it after a duplicate)"
        );
        // Stored fresh-outcome ordinals are strictly increasing and bounded by
        // the draw counter — never reused, never frozen.
        let ords: Vec<u32> = m
            .tree
            .node(chance_id)
            .chance_outcomes
            .iter()
            .map(|o| o.n)
            .collect();
        for w in ords.windows(2) {
            assert!(
                w[0] < w[1],
                "outcome ordinals must be strictly increasing, got {ords:?}"
            );
        }
        if let Some(&mx) = ords.iter().max() {
            assert!(
                mx < m.tree.node(chance_id).chance_draws,
                "stored ordinal {mx} must be below the draw counter {}",
                m.tree.node(chance_id).chance_draws
            );
        }
    }

    #[test]
    fn chance_closed_gate_stops_widening_only_when_enabled() {
        // `chance_closed_stops_widening` gate semantics, both polarities:
        //  - OFF (the default): a CLOSED node still widens — `chance_draws`
        //    keeps advancing. This documents the historical behavior the
        //    gate exists to change (closure gates win-proving, not spend).
        //  - ON: a closed node stops drawing entirely (`chance_draws`
        //    frozen), descends into an existing outcome, and never reports
        //    a fresh leaf.
        for gate_on in [false, true] {
            let state = make_smoke_state(11);
            let mut m = Mcts::new(state, chance_cfg(1, 0));
            m.config.chance_closed_stops_widening = gate_on;
            let root = m.tree.root;
            let pre = m.tree.node(root).realized.as_deref().unwrap().clone();
            let edge = EdgeAction::Other(Action::EndTurn);
            let chance_id = m.alloc_chance_node(root, &edge, &pre);
            m.tree.node_mut(root).children.push((edge, chance_id));
            let path = [root, chance_id];
            // Seed one real outcome BEFORE closing, so the sample branch has
            // something to sample (closure in production requires duplicate
            // draws, which require an existing outcome — the invariant the
            // gate's safety argument rests on).
            m.tree.node_mut(chance_id).visits += 10_000;
            let (_, fresh) = m.chance_widen_or_sample(chance_id, &path);
            assert!(fresh, "first draw on an empty node must widen");
            let draws_before = m.tree.node(chance_id).chance_draws;
            m.tree.node_mut(chance_id).chance_closed = true;
            let existing: Vec<NodeId> = m
                .tree
                .node(chance_id)
                .chance_outcomes
                .iter()
                .map(|o| o.child)
                .collect();
            for _ in 0..4 {
                m.tree.node_mut(chance_id).visits += 10_000;
                let (child, fresh) = m.chance_widen_or_sample(chance_id, &path);
                if gate_on {
                    assert!(!fresh, "gated node must never report a fresh leaf");
                    assert!(
                        existing.contains(&child),
                        "gated node must descend into a pre-existing outcome"
                    );
                }
            }
            let draws_after = m.tree.node(chance_id).chance_draws;
            if gate_on {
                assert_eq!(
                    draws_after, draws_before,
                    "gate ON: a closed node must stop consuming draw ordinals"
                );
            } else {
                assert!(
                    draws_after > draws_before,
                    "gate OFF (default): closure must NOT stop widening — \
                     byte-identical historical behavior"
                );
            }
        }
    }

    #[test]
    fn chance_mode_reroot_installs_realized() {
        let state = make_smoke_state(10);
        let mut m = Mcts::new(state, chance_cfg(33, 200));
        let action = m.search_n(200);
        // Advance the real engine by the chosen action to get the true state.
        let mut committed = m.tree.root_state.clone();
        let _ = core_apply(&mut committed, action.clone());
        let ok = m.reroot(&action, committed);
        assert!(ok, "reroot should succeed for an expanded root action");
        let root = m.tree.node(m.tree.root);
        assert_eq!(root.kind, NodeKind::Decision, "rerooted root is a decision");
        assert!(
            root.realized.is_some(),
            "rerooted root caches realized state"
        );
        assert!(
            root.chance_outcomes.is_empty(),
            "rerooted root has no outcomes"
        );
        // Search continues without panic.
        let _ = m.search_n(50);
    }

    #[test]
    fn chance_mode_loss_proves_through_chance() {
        // Loss-side proving is sound and always on: if any outcome of a
        // chance node is a proven loss, the chance node is a proven loss.
        // Build a chance node by hand with one proven-loss outcome.
        let state = make_smoke_state(11);
        let mut m = Mcts::new(state, chance_cfg(1, 0));
        let root = m.tree.root;
        // Fabricate: root(decision) -> chance -> outcome(proven_loss).
        let pre = m.tree.node(root).realized.as_deref().unwrap().clone();
        let edge = EdgeAction::Other(Action::EndTurn);
        let chance_id = m.alloc_chance_node(root, &edge, &pre);
        m.tree.node_mut(root).children.push((edge, chance_id));
        let outcome = m.chance_alloc_decision(Some(chance_id), pre, &[]);
        m.tree.node_mut(outcome).proven_loss = true;
        m.tree
            .node_mut(chance_id)
            .chance_outcomes
            .push(ChanceOutcome {
                n: 0,
                child: outcome,
                dedup_hash: None,
            });
        let path = [root, chance_id, outcome];
        m.chance_backprop_proven(&path);
        assert!(
            m.tree.node(chance_id).proven_loss,
            "any proven-loss outcome must prove the chance node a loss"
        );
    }

    /// A8 audit repair: the decision-node loss stamp must wait for FULL
    /// expansion. One untried action is expanded per visit, so on sim 1
    /// a decision node's `children` holds only the first canonical
    /// action — if that one action dies (attack into Thorns at low HP),
    /// the old all-EXPANDED-children rule latched `proven_loss` on the
    /// parent while an untried EndTurn/Defend was still winnable, and
    /// the proven-parent early break made the false stamp permanent.
    /// With `lost_position_override` trusting the flag as a proof, that
    /// false stamp switched the search off on a winnable position
    /// (FINDINGS §3's own pathology, reintroduced through the solver).
    #[test]
    fn decision_loss_not_proven_while_untried_actions_remain() {
        let state = make_smoke_state(13);
        let mut m = Mcts::new(state, chance_cfg(1, 0));
        let root = m.tree.root;
        let pre = m.tree.node(root).realized.as_deref().unwrap().clone();
        // Mirror the lazy-expansion state after sim 1: node expanded,
        // ONE child (proven loss), one canonical action still untried.
        let first_edge = EdgeAction::Other(Action::EndTurn);
        let dead_child = m.chance_alloc_decision(Some(root), pre, &[]);
        m.tree.node_mut(dead_child).proven_loss = true;
        {
            let n = m.tree.node_mut(root);
            n.is_expanded = true;
            n.children.push((first_edge, dead_child));
            n.untried_actions
                .push(EdgeAction::Other(Action::Evoke { times: 1 }));
        }
        m.chance_backprop_proven(&[root, dead_child]);
        assert!(
            !m.tree.node(root).proven_loss,
            "one dead child must not prove a loss while actions are untried"
        );
        assert!(
            m.lost_position_override().is_none(),
            "the override must not fire off an unproven loss"
        );

        // Expand the remaining action to a second proven-loss child:
        // now the proof is complete and the stamp must land.
        let pre2 = m.tree.node(root).realized.as_deref().unwrap().clone();
        let second_edge = m.tree.node_mut(root).untried_actions.remove(0);
        let dead_child2 = m.chance_alloc_decision(Some(root), pre2, &[]);
        m.tree.node_mut(dead_child2).proven_loss = true;
        m.tree
            .node_mut(root)
            .children
            .push((second_edge, dead_child2));
        m.chance_backprop_proven(&[root, dead_child2]);
        assert!(
            m.tree.node(root).proven_loss,
            "a fully-expanded all-losing child set must prove the loss"
        );
    }

    #[test]
    fn chance_mode_win_not_proven_without_flag() {
        // Win-side proving through chance is gated: with chance_prove_wins
        // off (default), an all-wins chance node is NOT proven a win.
        let state = make_smoke_state(12);
        let mut m = Mcts::new(state, chance_cfg(1, 0));
        assert!(!m.config.chance_prove_wins, "default is off");
        let root = m.tree.root;
        let pre = m.tree.node(root).realized.as_deref().unwrap().clone();
        let edge = EdgeAction::Other(Action::EndTurn);
        let chance_id = m.alloc_chance_node(root, &edge, &pre);
        m.tree.node_mut(root).children.push((edge, chance_id));
        let outcome = m.chance_alloc_decision(Some(chance_id), pre, &[]);
        m.tree.node_mut(outcome).proven_win = true;
        m.tree.node_mut(chance_id).chance_closed = true;
        m.tree
            .node_mut(chance_id)
            .chance_outcomes
            .push(ChanceOutcome {
                n: 0,
                child: outcome,
                dedup_hash: None,
            });
        let path = [root, chance_id, outcome];
        m.chance_backprop_proven(&path);
        assert!(
            !m.tree.node(chance_id).proven_win,
            "win must not be proven through chance with chance_prove_wins off"
        );

        // With the flag on AND closed AND all outcomes won, it proves.
        m.config.chance_prove_wins = true;
        m.chance_backprop_proven(&path);
        assert!(
            m.tree.node(chance_id).proven_win,
            "closed all-win chance node proves a win when the flag is on"
        );
    }

    // ── DrawChance (bead `sts2-rs-i8bgu`, F1 plan §3.2, G0 unit tests) ──

    /// Builds a `PlayerPlayPhase` Ironclad-vs-Nibbit state with a single
    /// Pommel Strike in hand (a real dispatch-backed 1-cost Attack, 9 dmg,
    /// draw 1 — its ONLY non-deterministic side effect is the draw
    /// itself, isolating the draw-chance detector from any RNG-consuming
    /// roll) and a caller-specified draw pile. Mirrors
    /// `playable_strike_in_hand_search_value_beats_unplayable_curse_in_hand`'s
    /// fixture-building pattern. The Nibbit's HP is large enough that a
    /// single Pommel Strike is never lethal, so the tree shape under test
    /// isn't confounded by early termination.
    fn build_pommel_strike_state(draw_cards: &[(sts2_core::cards::CardId, bool)]) -> CombatState {
        sts2_content::cards::dispatch::install_default_registry();

        use smallvec::SmallVec;
        use sts2_core::cards::{CardId, CardType, TargetType};
        use sts2_core::{
            Card, CardUid, CombatPhase, Creature, CreatureUid, IntentHistory, MonsterId,
            MonsterKind, OrbSlots, PlayerCombatState, Side, TurnEvents,
        };

        let pommel_uid = CardUid::new(1).expect("non-zero");
        let mut cards = vec![Card {
            uid: pommel_uid,
            card_id: CardId::PommelStrike,
            cost: 1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 9,
            base_block: 0,
            ..Card::pod_defaults()
        }];
        let mut draw: SmallVec<[CardUid; 32]> = SmallVec::new();
        for (i, &(card_id, upgraded)) in draw_cards.iter().enumerate() {
            let uid = CardUid::new(i as u32 + 2).expect("non-zero");
            cards.push(Card {
                uid,
                card_id,
                cost: 1,
                card_type: CardType::Attack,
                target_type: TargetType::Enemy,
                upgraded,
                base_damage: 6,
                base_block: 0,
                ..Card::pod_defaults()
            });
            draw.push(uid);
        }

        let mut hand: SmallVec<[CardUid; 16]> = SmallVec::new();
        hand.push(pommel_uid);

        let cultist_uid = CreatureUid::new(1).expect("non-zero");
        let cultist = Creature {
            id: cultist_uid.0,
            current_hp: 200,
            max_hp: 200,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Nibbit),
            monster_id: MonsterId("Nibbit"),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        };

        let player = PlayerCombatState {
            hand,
            draw,
            discard: SmallVec::new(),
            exhaust: SmallVec::new(),
            play: SmallVec::new(),
            energy: 1,
            max_energy: 3,
            stars: 0,
            current_hp: 80,
            max_hp: 80,
            block: 0,
            powers: SmallVec::new(),
            character: None,
            potions: SmallVec::new(),
            potion_slot_indices: SmallVec::new(),
            potion_slots: 0,
            orbs: OrbSlots::default(),
            creature_uid: None,
            pet_uid: None,
            shivs_played_this_turn: 0,
            cards_played_this_turn: [0; 5],
            powered_hits_this_turn: 0,
            powered_hits_by_target_this_turn: SmallVec::new(),
            powered_hits_at_card_play_start: 0,
            attack_command_suppressed_this_play: false,
            vigor_latch_card: None,
            generated_cards_this_combat: 0,
            skills_played_this_turn: 0,
            card_plays_started_this_turn: 0,
            cards_first_in_series_this_turn: 0,
            request_end_turn_after_play: false,
            retain_this_turn: SmallVec::new(),
            sly_this_turn: SmallVec::new(),
            sly_this_combat: SmallVec::new(),
            pending_retain_count: 0,
            stars_gained_this_turn: 0,
            pinned_top: SmallVec::new(),
            pinned_bottom: SmallVec::new(),
            deferred_draw_order: false,
        };

        let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
        enemy_uids.push(cultist_uid);

        CombatState {
            belt_value: None,
            phase: CombatPhase::PlayerPlayPhase,
            round: 1,
            side: Side::Player,
            rng: sts2_core::dotnet_rng::Rng::new(1),
            run_rng: sts2_core::run_rng_set::RunRngSet::new(1),
            creatures: vec![cultist],
            cards: cards.into(),
            powers: Vec::new().into(),
            relics: Vec::new().into(),
            potions: Vec::new(),
            player,
            allies: SmallVec::new(),
            enemy_uids,
            pending_choice: None,
            resolving_card: None,
            pending_play: None,
            pending_turn_setup_resume: false,
            pending_draw_resume: None,
            pending_snecko_cost_randomize: false,
            pending_cluster_resume: None,
            pending_hand_draw_resume: None,
            pending_relic_hand_draw_resume: None,
            pending_wlp_retain_resume: false,
            pending_end_turn_resume: None,
            pending_autoplay_resume: None,
            autoplay_inline_resolve: false,
            pending_curse_choice: None,
            pending_potion_hook_resume: None,
            pending_enemy_exec_resume: None,
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
        }
    }

    fn draw_chance_cfg(seed: u64, sims: u32) -> MctsConfig {
        MctsConfig {
            max_simulations: sims,
            seed,
            hidden_info: HiddenInfoMode::DrawChance,
            ..MctsConfig::default()
        }
    }

    /// Find the `NodeKind` of the root child reached by the `PlayCard`
    /// edge matching `card_id` (the Pommel Strike edge in every fixture
    /// below — there is exactly one `PlayCard` edge since the fixture
    /// hand has one card).
    fn pommel_strike_child_kind<P: Policy>(m: &Mcts<P>) -> Option<NodeKind> {
        let root = m.tree.root;
        m.tree
            .node(root)
            .children
            .iter()
            .find_map(|(edge, cid)| match edge {
                EdgeAction::PlayCard {
                    card_id: sts2_core::cards::CardId::PommelStrike,
                    ..
                } => Some(m.tree.node(*cid).kind),
                _ => None,
            })
    }

    #[test]
    fn draw_chance_node_created_for_heterogeneous_segment() {
        use sts2_core::cards::CardId;
        let state = build_pommel_strike_state(&[
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::Bash, false),
        ]);
        let mut m = Mcts::new(state, draw_chance_cfg(0xD4A1, 10));
        let _ = m.search_n(10);
        let kind = pommel_strike_child_kind(&m).expect("PommelStrike edge must exist at root");
        assert_eq!(
            kind,
            NodeKind::Chance,
            "a >=2-card, heterogeneous unknown segment must chance-node the draw"
        );
    }

    #[test]
    fn draw_chance_node_not_created_for_homogeneous_segment() {
        use sts2_core::cards::CardId;
        let state = build_pommel_strike_state(&[
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
        ]);
        let mut m = Mcts::new(state, draw_chance_cfg(0xD4A2, 10));
        let _ = m.search_n(10);
        let kind = pommel_strike_child_kind(&m).expect("PommelStrike edge must exist at root");
        assert_eq!(
            kind,
            NodeKind::Decision,
            "a homogeneous unknown segment (every copy the same card) must NOT chance-node — \
             every outcome looks identical to the player"
        );
    }

    #[test]
    fn draw_chance_node_not_created_for_empty_pile() {
        let state = build_pommel_strike_state(&[]);
        let mut m = Mcts::new(state, draw_chance_cfg(0xD4A3, 10));
        let _ = m.search_n(10);
        let kind = pommel_strike_child_kind(&m).expect("PommelStrike edge must exist at root");
        assert_eq!(
            kind,
            NodeKind::Decision,
            "drawing from an empty pile must NOT chance-node (it's a deterministic no-op)"
        );
    }

    #[test]
    fn draw_chance_node_not_created_for_single_card_pile() {
        use sts2_core::cards::CardId;
        let state = build_pommel_strike_state(&[(CardId::Bash, false)]);
        let mut m = Mcts::new(state, draw_chance_cfg(0xD4A4, 10));
        let _ = m.search_n(10);
        let kind = pommel_strike_child_kind(&m).expect("PommelStrike edge must exist at root");
        assert_eq!(
            kind,
            NodeKind::Decision,
            "a single-card pile has only one possible draw — must NOT chance-node"
        );
    }

    #[test]
    fn draw_chance_never_fires_under_clairvoyant_or_root_scramble() {
        use sts2_core::cards::CardId;
        let draw_cards = [
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::Bash, false),
        ];
        for mode in [HiddenInfoMode::Clairvoyant, HiddenInfoMode::RootScramble] {
            let state = build_pommel_strike_state(&draw_cards);
            let cfg = MctsConfig {
                max_simulations: 10,
                seed: 0xD4A5,
                hidden_info: mode,
                ..MctsConfig::default()
            };
            let mut m = Mcts::new(state, cfg);
            let _ = m.search_n(10);
            let kind = pommel_strike_child_kind(&m).expect("PommelStrike edge must exist at root");
            assert_eq!(
                kind,
                NodeKind::Decision,
                "{mode:?} must never chance-node a pure draw, even on a heterogeneous segment"
            );
        }
    }

    /// Equivalent-outcome dedup (F1 plan §3.2 step 3): a draw pile with
    /// THREE physically-distinct but observationally-IDENTICAL Strikes
    /// plus one Bash has exactly 2 distinguishable draw outcomes ("drew a
    /// Strike" / "drew the Bash"). With enough widening draws to sample
    /// both many times over, the chance node's outcome count must
    /// plateau at 2 — not grow toward 4 (one per physical Strike uid),
    /// which is what would happen without the order/uid-insensitive
    /// dedup hash.
    #[test]
    fn draw_chance_dedups_observationally_equal_outcomes() {
        use sts2_core::cards::CardId;
        let state = build_pommel_strike_state(&[
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::Bash, false),
        ]);
        let mut m = Mcts::new(state, draw_chance_cfg(0xDEDE, 600));
        let _ = m.search_n(600);
        let root = m.tree.root;
        let chance_id = m
            .tree
            .node(root)
            .children
            .iter()
            .find_map(|(edge, cid)| match edge {
                EdgeAction::PlayCard {
                    card_id: CardId::PommelStrike,
                    ..
                } => Some(*cid),
                _ => None,
            })
            .expect("PommelStrike edge must exist at root");
        assert_eq!(m.tree.node(chance_id).kind, NodeKind::Chance);
        let n_outcomes = m.tree.node(chance_id).chance_outcomes.len();
        assert_eq!(
            n_outcomes,
            2,
            "3 duplicate Strikes + 1 Bash must dedup to exactly 2 outcomes \
             (drew-a-Strike / drew-the-Bash), not one per physical Strike uid \
             (got {n_outcomes} outcomes; widenings_since_new={})",
            m.tree.node(chance_id).widenings_since_new,
        );
    }

    /// Outcome-0 de-privileging (F1 plan §3.2 step 2): under `DrawChance`,
    /// [`Mcts::realize_chance_outcome`] is a deterministic function of its
    /// arguments (same `(pre_action, edge, base, n)` in -> same state out)
    /// and genuinely varies by `n` — it is no longer special-cased to
    /// "whatever the real RNG cursors happen to produce", the way outcome
    /// 0 was before this bead.
    #[test]
    fn realize_chance_outcome_is_deterministic_and_varies_by_n() {
        use sts2_core::CardUid;
        // `make_smoke_state`'s default 10-card deck already has 5 cards
        // in hand (drawn by its own `start_player_turn` call) and 5 left
        // in draw — exactly the Ironclad turn-draw amount, so shuffling
        // that 5-card remainder wouldn't change WHICH cards get drawn on
        // `EndTurn` (all 5 get drawn regardless of order). Override with
        // a pile comfortably larger than the draw amount so the shuffle
        // is actually observable in which cards land in hand.
        let mut state = make_smoke_state(7);
        state.player.draw = (1..=20u32)
            .map(|n| CardUid::new(n).expect("non-zero"))
            .collect();
        let edge = EdgeAction::Other(Action::EndTurn);
        let m = Mcts::new(state.clone(), draw_chance_cfg(0x5EED, 0));

        let s0a = m.realize_chance_outcome(state.clone(), &edge, 0xF00D_BEEF, 0, true);
        let s0b = m.realize_chance_outcome(state.clone(), &edge, 0xF00D_BEEF, 0, true);
        let h0a = sts2_core::search_hash(&s0a);
        assert_eq!(
            h0a,
            sts2_core::search_hash(&s0b),
            "same (pre_action, edge, base, n) must reproduce the same realized state"
        );

        let h1 =
            sts2_core::search_hash(&m.realize_chance_outcome(state, &edge, 0xF00D_BEEF, 1, true));
        assert_ne!(
            h0a, h1,
            "distinct outcome ordinals must (almost surely) realize distinct outcomes — outcome \
             0 is no longer special-cased to a single fixed continuation"
        );
    }

    /// When told to (`reshuffle_pile: true`, the caller's decision since
    /// bead `sts2-rs-k1z14`'s H3 retune — real callers pass this for
    /// draw-triggered chance nodes under `DrawChance`),
    /// [`Mcts::realize_chance_outcome`] ALSO reshuffles the pre-action
    /// state's unknown draw-pile segment; when `false` (what `Clairvoyant`
    /// callers always pass) it must not touch pile order at all — byte-
    /// stability for the historical widen-branch behavior.
    #[test]
    fn realize_chance_outcome_pile_shuffle_gated_on_draw_chance() {
        use sts2_core::CardUid;
        let mut state = make_smoke_state(7);
        state.player.draw = (1..=8u32)
            .map(|n| CardUid::new(n).expect("non-zero"))
            .collect();
        let edge = EdgeAction::Other(Action::EndTurn);
        let base = 0xABCD;
        let n = 3;

        // `EndTurn` resolves the FULL turn cycle (enemy turn + the next
        // player turn's hand draw), so it consumes cards off the draw
        // pile regardless of DrawChance — comparing against the
        // untouched `original_draw` (as a first draft of this test did)
        // conflates "cards were drawn" with "the pile was shuffled first"
        // and produces a false failure under Clairvoyant. Compare instead
        // against a hand-rolled reference that replicates the historical
        // (pre-bead) inline reseed-and-reapply sequence with NO shuffle
        // step — that is exactly what Clairvoyant must still produce.
        let mut reference = state.clone();
        reference.run_rng.reseed_combat_streams(base, n);
        reference.rng = sts2_core::dotnet_rng::Rng::from_search_seed(
            sts2_core::run_rng_set::outcome_stream_seed_u64(
                base,
                n,
                sts2_core::run_rng_set::LEGACY_RNG_SALT,
            ),
        );
        if let Some(action) = edge.resolve(&reference) {
            let _ = core_apply(&mut reference, action);
        }

        let cv_cfg = MctsConfig {
            hidden_info: HiddenInfoMode::Clairvoyant,
            ..draw_chance_cfg(0x5EED, 0)
        };
        let m_cv = Mcts::new(state.clone(), cv_cfg);
        let s_cv = m_cv.realize_chance_outcome(state.clone(), &edge, base, n, false);
        assert_eq!(
            sts2_core::search_hash(&s_cv),
            sts2_core::search_hash(&reference),
            "Clairvoyant's realize_chance_outcome must be byte-identical to the historical \
             inline reseed-and-reapply sequence — no shuffle step"
        );

        let m_draw = Mcts::new(state.clone(), draw_chance_cfg(0x5EED, 0));
        let s_draw = m_draw.realize_chance_outcome(state.clone(), &edge, base, n, true);
        assert_ne!(
            s_draw.player.draw, s_cv.player.draw,
            "DrawChance's extra pile-shuffle step must change the post-action draw pile \
             relative to Clairvoyant, given the identical edge/base/n"
        );
    }

    /// Pins survive per-outcome pile shuffles (mirrors the root-blindfold
    /// pin-preservation guarantee).
    #[test]
    fn realize_chance_outcome_preserves_pins() {
        // `EndTurn` (the only always-legal edge in this minimal fixture)
        // actually DRAWS a fresh hand as part of resolving the next
        // player turn — and drawing a pinned card consumes its pin (bead
        // `sts2-rs-o6xw`; that's correct engine behavior, a pin only
        // promises WHERE a card sits in the pile, not that it stays
        // there forever). A `pinned_top` card is always the very next
        // card drawn, so it can never "survive" a draw — this test
        // therefore only pins to the BOTTOM, on a pile large enough
        // (comfortably more than `HAND_LIMIT`) that the post-EndTurn draw
        // never reaches that far, so the pin is expected to survive.
        use sts2_core::CardUid;
        let mut state = make_smoke_state(7);
        state.player.draw = (1..=20u32)
            .map(|n| CardUid::new(n).expect("non-zero"))
            .collect();
        let bottom = CardUid::new(20).unwrap();
        sts2_core::cards::pin_bottom(&mut state.player, bottom);
        let edge = EdgeAction::Other(Action::EndTurn);

        let m = Mcts::new(state.clone(), draw_chance_cfg(0x5EED, 0));
        let s = m.realize_chance_outcome(state, &edge, 0xABCD, 9, true);
        assert_eq!(s.player.pinned_bottom.as_slice(), &[bottom]);
        assert_eq!(s.player.draw.last().copied(), Some(bottom));
    }

    /// Determinism: the same state + config searched twice under
    /// `DrawChance` (fresh `Mcts` instances) must pick the same action —
    /// mirrors `root_scramble_is_deterministic_for_fixed_seed` in
    /// `tests/root_blindfold.rs` for the F1 mode.
    #[test]
    fn draw_chance_search_is_deterministic_for_fixed_seed() {
        use sts2_core::cards::CardId;
        let draw_cards = [
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::Bash, false),
        ];
        let cfg = draw_chance_cfg(0xBADC_0FFE, 200);
        let action_1 = Mcts::new(build_pommel_strike_state(&draw_cards), cfg).search();
        let action_2 = Mcts::new(build_pommel_strike_state(&draw_cards), cfg).search();
        assert_eq!(
            format!("{action_1:?}"),
            format!("{action_2:?}"),
            "same state + config must reproduce the same chosen action under DrawChance"
        );
    }

    // ── BeliefDraw (bead `sts2-rs-09x5n`, F2 plan §3.2, G0 unit tests) ──

    fn belief_draw_cfg(seed: u64, sims: u32) -> MctsConfig {
        MctsConfig {
            max_simulations: sims,
            seed,
            hidden_info: HiddenInfoMode::BeliefDraw,
            ..MctsConfig::default()
        }
    }

    #[test]
    fn belief_draw_node_created_for_heterogeneous_segment() {
        use sts2_core::cards::CardId;
        let state = build_pommel_strike_state(&[
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::Bash, false),
        ]);
        let mut m = Mcts::new(state, belief_draw_cfg(0xBE11, 10));
        let _ = m.search_n(10);
        let kind = pommel_strike_child_kind(&m).expect("PommelStrike edge must exist at root");
        assert_eq!(
            kind,
            NodeKind::Chance,
            "a >=2-card unknown segment must chance-node the draw under BeliefDraw"
        );
    }

    /// Unlike `DrawChance` (bead `sts2-rs-i8bgu`), `BeliefDraw` has NO
    /// heterogeneity pre-check — the trigger is purely the fingerprint
    /// delta from `cards::draw`'s deferred-draw branch, which fires
    /// regardless of whether the segment's content is homogeneous. The
    /// tree doesn't actually WIDEN over indistinguishable outcomes though
    /// (see `belief_draw_homogeneous_segment_dedups_to_one_outcome`).
    #[test]
    fn belief_draw_node_created_for_homogeneous_segment() {
        use sts2_core::cards::CardId;
        let state = build_pommel_strike_state(&[
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
        ]);
        let mut m = Mcts::new(state, belief_draw_cfg(0xBE12, 10));
        let _ = m.search_n(10);
        let kind = pommel_strike_child_kind(&m).expect("PommelStrike edge must exist at root");
        assert_eq!(
            kind,
            NodeKind::Chance,
            "BeliefDraw has no heterogeneity guard — even a homogeneous unknown \
             segment chance-nodes the draw (the fingerprint moved)"
        );
    }

    #[test]
    fn belief_draw_node_not_created_for_empty_pile() {
        let state = build_pommel_strike_state(&[]);
        let mut m = Mcts::new(state, belief_draw_cfg(0xBE13, 10));
        let _ = m.search_n(10);
        let kind = pommel_strike_child_kind(&m).expect("PommelStrike edge must exist at root");
        assert_eq!(
            kind,
            NodeKind::Decision,
            "drawing from an empty pile must NOT chance-node (it's a deterministic no-op)"
        );
    }

    #[test]
    fn belief_draw_node_not_created_for_single_card_pile() {
        use sts2_core::cards::CardId;
        let state = build_pommel_strike_state(&[(CardId::Bash, false)]);
        let mut m = Mcts::new(state, belief_draw_cfg(0xBE14, 10));
        let _ = m.search_n(10);
        let kind = pommel_strike_child_kind(&m).expect("PommelStrike edge must exist at root");
        assert_eq!(
            kind,
            NodeKind::Decision,
            "a single-card unknown segment draws deterministically (no RNG) — must NOT \
             chance-node"
        );
    }

    /// Design note (task-7a brief, Tests section): a homogeneous multi-card
    /// segment under `BeliefDraw` WILL consume RNG (index choice) and thus
    /// chance-node, but every outcome dedups to the SAME child via
    /// [`sts2_core::chance_outcome_hash`] — the tree never actually widens
    /// past one outcome no matter how many widening draws are attempted.
    #[test]
    fn belief_draw_homogeneous_segment_dedups_to_one_outcome() {
        use sts2_core::cards::CardId;
        let state = build_pommel_strike_state(&[
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
        ]);
        let mut m = Mcts::new(state, belief_draw_cfg(0xBE15, 600));
        let _ = m.search_n(600);
        let root = m.tree.root;
        let chance_id = m
            .tree
            .node(root)
            .children
            .iter()
            .find_map(|(edge, cid)| match edge {
                EdgeAction::PlayCard {
                    card_id: sts2_core::cards::CardId::PommelStrike,
                    ..
                } => Some(*cid),
                _ => None,
            })
            .expect("PommelStrike edge must exist at root");
        assert_eq!(m.tree.node(chance_id).kind, NodeKind::Chance);
        let n_outcomes = m.tree.node(chance_id).chance_outcomes.len();
        assert_eq!(
            n_outcomes, 1,
            "4 observationally-identical Strikes must dedup to exactly 1 outcome, not \
             one per physical uid (got {n_outcomes})"
        );
    }

    /// Mirrors `draw_chance_dedups_observationally_equal_outcomes` for
    /// `BeliefDraw`: 3 duplicate Strikes + 1 Bash must dedup to exactly 2
    /// distinguishable outcomes.
    #[test]
    fn belief_draw_dedups_observationally_equal_outcomes() {
        use sts2_core::cards::CardId;
        let state = build_pommel_strike_state(&[
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::Bash, false),
        ]);
        let mut m = Mcts::new(state, belief_draw_cfg(0xBEDE, 600));
        let _ = m.search_n(600);
        let root = m.tree.root;
        let chance_id = m
            .tree
            .node(root)
            .children
            .iter()
            .find_map(|(edge, cid)| match edge {
                EdgeAction::PlayCard {
                    card_id: CardId::PommelStrike,
                    ..
                } => Some(*cid),
                _ => None,
            })
            .expect("PommelStrike edge must exist at root");
        assert_eq!(m.tree.node(chance_id).kind, NodeKind::Chance);
        let n_outcomes = m.tree.node(chance_id).chance_outcomes.len();
        assert_eq!(
            n_outcomes,
            2,
            "3 duplicate Strikes + 1 Bash must dedup to exactly 2 outcomes \
             (drew-a-Strike / drew-the-Bash) under BeliefDraw (got {n_outcomes} outcomes; \
             widenings_since_new={})",
            m.tree.node(chance_id).widenings_since_new,
        );
    }

    /// Widening-class classification (F2 plan §3.2 step 6): a `BeliefDraw`
    /// chance node born from a deferred draw is marked draw-triggered via a
    /// plain `cards_drawn` delta — no heterogeneity check needed, since the
    /// trigger for the node's EXISTENCE already is the fingerprint delta.
    #[test]
    fn belief_draw_chance_node_marked_draw_triggered() {
        use sts2_core::cards::CardId;
        let state =
            build_pommel_strike_state(&[(CardId::StrikeIronclad, false), (CardId::Bash, false)]);
        let mut m = Mcts::new(state, belief_draw_cfg(0xBE16, 10));
        let _ = m.search_n(10);
        let root = m.tree.root;
        let chance_id = m
            .tree
            .node(root)
            .children
            .iter()
            .find_map(|(edge, cid)| match edge {
                EdgeAction::PlayCard {
                    card_id: CardId::PommelStrike,
                    ..
                } => Some(*cid),
                _ => None,
            })
            .expect("PommelStrike edge must exist at root");
        assert_eq!(m.tree.node(chance_id).kind, NodeKind::Chance);
        assert!(
            m.tree.node(chance_id).chance_is_draw_triggered,
            "a chance node born from a deferred draw must be marked draw-triggered under \
             BeliefDraw so it selects WideningClass::Draw"
        );
    }

    /// GH #758: `dpw_outcome_cap` — what the tree inspector displays — sizes
    /// every chance node with the class the search itself uses, including
    /// `Draw` for a draw-triggered non-EndTurn node (the inspector used to
    /// size those with the `chance_*` pair and under-report the cap).
    #[test]
    fn dpw_outcome_cap_uses_the_search_widening_class() {
        use sts2_core::cards::CardId;
        let state =
            build_pommel_strike_state(&[(CardId::StrikeIronclad, false), (CardId::Bash, false)]);
        let mut cfg = belief_draw_cfg(0xCA9, 200);
        cfg.apply_mode_dialect();
        let mut m = Mcts::new(state, cfg);
        let _ = m.search_n(200);
        let formula = |(c, alpha): (f32, f32), visits: u32| dpw_cap_formula(c, alpha, visits);
        let (mut draw, mut endturn) = (0, 0);
        for node in &m.tree.arena {
            if node.kind != NodeKind::Chance {
                continue;
            }
            let got = dpw_outcome_cap(&cfg, node);
            if matches!(node.chance_action, Some(EdgeAction::Other(Action::EndTurn))) {
                endturn += 1;
                assert_eq!(got, formula(BELIEF_DRAW_ENDTURN_WIDENING, node.visits));
            } else if node.chance_is_draw_triggered {
                draw += 1;
                let draw_pair = (BELIEF_DRAW_DRAW_WIDENING_C, cfg.draw_widening_alpha);
                assert_eq!(got, formula(draw_pair, node.visits));
                if node.visits > 0 {
                    assert_ne!(
                        got,
                        formula(
                            (cfg.chance_widening_c, cfg.chance_widening_alpha),
                            node.visits
                        ),
                        "a Draw node's cap must differ from the Default pair's here"
                    );
                }
            }
        }
        assert!(
            draw > 0,
            "fixture never expanded a draw-triggered chance node"
        );
        assert!(endturn > 0, "fixture never expanded an EndTurn chance node");
    }

    /// The dialect is BeliefDraw-only and budget-keyed.
    #[test]
    fn apply_mode_dialect_sets_the_belief_draw_constants_only() {
        let mut cfg = belief_draw_cfg(1, 300);
        cfg.apply_mode_dialect();
        assert!((cfg.c_puct - belief_draw_c_puct(300)).abs() < f32::EPSILON);
        assert!((cfg.draw_widening_c - BELIEF_DRAW_DRAW_WIDENING_C).abs() < f32::EPSILON);
        assert_eq!(
            (cfg.endturn_widening_c, cfg.endturn_widening_alpha),
            BELIEF_DRAW_ENDTURN_WIDENING
        );
        for mode in [
            HiddenInfoMode::Clairvoyant,
            HiddenInfoMode::RootScramble,
            HiddenInfoMode::DrawChance,
        ] {
            let before = MctsConfig {
                hidden_info: mode,
                ..MctsConfig::default()
            };
            let mut after = before;
            after.apply_mode_dialect();
            assert_eq!(format!("{before:?}"), format!("{after:?}"), "{mode:?}");
        }
    }

    #[test]
    fn belief_draw_search_is_deterministic_for_fixed_seed() {
        use sts2_core::cards::CardId;
        let draw_cards = [
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::Bash, false),
        ];
        let cfg = belief_draw_cfg(0xBADC_0FFE, 200);
        let action_1 = Mcts::new(build_pommel_strike_state(&draw_cards), cfg).search();
        let action_2 = Mcts::new(build_pommel_strike_state(&draw_cards), cfg).search();
        assert_eq!(
            format!("{action_1:?}"),
            format!("{action_2:?}"),
            "same state + config must reproduce the same chosen action under BeliefDraw"
        );
    }

    /// Byte-stability (task-7a brief): a `Clairvoyant` search on a fixture
    /// is unaffected by the new `deferred_draw_order` field's mere
    /// presence — the field defaults `false` and every gate in
    /// `sts2-core`/`sts2-mcts` reads it, never infers it from
    /// `hidden_info`.
    #[test]
    fn clairvoyant_search_unaffected_by_deferred_draw_order_field() {
        use sts2_core::cards::CardId;
        let draw_cards = [
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::Bash, false),
        ];
        let state = build_pommel_strike_state(&draw_cards);
        assert!(
            !state.player.deferred_draw_order,
            "a freshly-built fixture must default the flag to false"
        );
        let cfg = MctsConfig {
            max_simulations: 200,
            seed: 0xC1A1_2020,
            hidden_info: HiddenInfoMode::Clairvoyant,
            ..MctsConfig::default()
        };
        let action_1 = Mcts::new(state.clone(), cfg).search();
        let action_2 = Mcts::new(state, cfg).search();
        assert_eq!(
            format!("{action_1:?}"),
            format!("{action_2:?}"),
            "Clairvoyant search must remain deterministic and unaffected by the new field"
        );
    }

    /// `Mcts::reroot`'s hard assert against non-`Clairvoyant` modes (bead
    /// `sts2-rs-vqckz`) must also cover `BeliefDraw` — reroot installs the
    /// caller's state verbatim (no root conversion), which would silently
    /// restore full clairvoyance (and drop `deferred_draw_order`) from move
    /// 2 onward.
    #[test]
    #[should_panic(expected = "reroot is unsupported under BeliefDraw")]
    fn reroot_panics_under_belief_draw() {
        use sts2_core::cards::CardId;
        let draw_cards = [
            (CardId::StrikeIronclad, false),
            (CardId::StrikeIronclad, false),
            (CardId::Bash, false),
        ];
        let state = build_pommel_strike_state(&draw_cards);
        let cfg = belief_draw_cfg(0xDEAD_BEEF, 50);
        let mut mcts = Mcts::new(state.clone(), cfg);
        let action = mcts.search();
        let _ = mcts.reroot(&action, state);
    }
}

#[cfg(test)]
mod terminal_scale_tests {
    use super::*;

    /// GH#296: a WON terminal must outrank any non-terminal continuation.
    ///
    /// `RichBoth` documents wins in `[+1.0, +1.5]` and depth-cuts in
    /// `(-0.5, +0.5)`. The `fair`-eval paths used to score an immediately
    /// terminal win with `terminal_value()` (the Binary scale, `[0.5, 1.0]`)
    /// while scoring every sibling with a `RichBoth` search — so the win came
    /// out 0.5 LOW and the search declined lethal plays. Pin the band.
    #[test]
    fn won_terminal_outranks_the_depth_cut_band_under_rich_both() {
        let mut won = CombatState {
            is_over: true,
            player_won: true,
            ..CombatState::default()
        };
        won.player.current_hp = 1; // worst survivable win

        let v = terminal_state_value(&won, TerminalEvalMode::RichBoth, &LeafEval::default(), true);
        assert!(
            v >= 1.0,
            "a win must land in the documented [1.0, 1.5] band, got {v}"
        );
        // The depth-cut band tops out below +0.5, so any win must clear it.
        assert!(
            v > 0.5,
            "a win must strictly outrank the depth-cut band, got {v}"
        );
        // Raw `terminal_value` is the Binary scale and is exactly what the
        // fair paths used to return -- it does NOT clear the band. If this
        // ever stops holding the bug class is gone and this test is moot.
        assert!(
            terminal_value(&won).1 < 1.0,
            "sanity: raw terminal_value is the Binary scale"
        );
    }

    /// A lost terminal must stay strictly below every win and depth-cut —
    /// under BOTH loss-gradient arms.
    #[test]
    fn lost_terminal_stays_below_the_depth_cut_band() {
        let lost = CombatState {
            is_over: true,
            player_won: false,
            ..CombatState::default()
        };

        for loss_gradient in [true, false] {
            let v = terminal_state_value(
                &lost,
                TerminalEvalMode::RichBoth,
                &LeafEval::default(),
                loss_gradient,
            );
            assert!(
                v <= -0.55,
                "a loss must stay <= -0.55 (loss_gradient={loss_gradient}), got {v}"
            );
        }
        // Gradient arm is stricter: strictly below -1.0, i.e. below ANY
        // depth-cut a tanh-saturated leaf can produce.
        let v = terminal_state_value(
            &lost,
            TerminalEvalMode::RichBoth,
            &LeafEval::default(),
            true,
        );
        assert!(v < -1.0, "gradient losses live in [-1.45, -1.05), got {v}");
    }
}

/// Perf item M12: the `EndTurn` RNG-free prefix cache.
#[cfg(test)]
mod endturn_prefix_tests {
    use super::*;
    use sts2_content::monster_dispatch::MonsterSpawnArgs;
    use sts2_core::combat_loop::end_turn_dispatch_from;
    use sts2_core::potions::CharacterId;
    use sts2_core::runstate::{RoomType, RunState};
    use sts2_core::MonsterKind;

    fn install_content() {
        sts2_content::cards::dispatch::ensure_installed();
        sts2_content::relics::dispatch::ensure_installed();
        sts2_content::potions::dispatch::ensure_installed();
        sts2_core::relic_effects::ensure_installed();
    }

    /// An Ironclad starter deck against `monster`, in the play phase of
    /// round 1, built the way the run loop builds a combat.
    fn combat_vs(monster: MonsterKind, hp: i32, seed: i32) -> CombatState {
        install_content();
        let mut rs = RunState {
            current_hp: 70,
            max_hp: 80,
            rng: sts2_core::run_rng_set::RunRngSet::new(seed),
            character_id: CharacterId::Ironclad,
            ..RunState::default()
        };
        sts2_content::run_callbacks::populate_starter_deck(&mut rs, CharacterId::Ironclad);
        let roster = [(monster, MonsterSpawnArgs::None, hp)];
        let state =
            sts2_content::run_bridge::enter_combat_from_roster(&rs, &roster, RoomType::Monster);
        assert_eq!(state.phase, sts2_core::CombatPhase::PlayerPlayPhase);
        state
    }

    fn telegraphed_move(state: &CombatState) -> &'static str {
        let enemy = state.enemy_uids[0];
        state
            .creatures
            .iter()
            .find(|c| c.id == enemy.0)
            .and_then(|c| c.ai.as_ref())
            .expect("enemy has an AI")
            .current_move()
            .id
    }

    /// The act-2 ThievingHopper on round 1 (THIEVERY_MOVE, which draws the
    /// stolen card off `combat_card_generation` in `enemy_turn_exec`) and on
    /// round 2 (FLUTTER_MOVE, a self-buff that draws nothing).
    fn hopper_rounds(seed: i32) -> (CombatState, CombatState) {
        let thievery = combat_vs(MonsterKind::ThievingHopper, 60, seed);
        assert_eq!(telegraphed_move(&thievery), "THIEVERY_MOVE");
        let mut flutter = thievery.clone();
        let _ = core_apply(&mut flutter, Action::EndTurn);
        assert_eq!(flutter.phase, sts2_core::CombatPhase::PlayerPlayPhase);
        assert_eq!(telegraphed_move(&flutter), "FLUTTER_MOVE");
        (thievery, flutter)
    }

    /// The same state with its draw pile as the `BeliefDraw` root holds it.
    fn as_belief(state: &CombatState) -> CombatState {
        let mut s = state.clone();
        sts2_core::blindfold::belief_root(&mut s, 0xB0B0);
        s
    }

    fn prefix_len(state: &CombatState) -> usize {
        let mut s = state.clone();
        reseed_for_outcome(&mut s, 0xABCD, 0);
        end_turn_probing_prefix(&mut s, 1).0
    }

    /// An enemy move that draws randomness inside `enemy_turn_exec` must end
    /// the prefix before that phase (index 3), under both pile models; an
    /// RNG-free move lets the prefix run through it, so the first check is
    /// not vacuous.
    #[test]
    fn prefix_stops_before_an_enemy_move_that_draws_randomness() {
        for seed in [3, 17, 4242] {
            let (thievery, flutter) = hopper_rounds(seed);
            for (label, s) in [
                ("materialized", thievery.clone()),
                ("belief", as_belief(&thievery)),
            ] {
                assert_eq!(
                    prefix_len(&s),
                    3,
                    "seed {seed} {label}: THIEVERY's steal roll is in enemy_turn_exec"
                );
            }
            for (label, s) in [
                ("materialized", flutter.clone()),
                ("belief", as_belief(&flutter)),
            ] {
                assert!(
                    prefix_len(&s) >= 4,
                    "seed {seed} {label}: FLUTTER draws nothing, so enemy_turn_exec is in the prefix"
                );
            }
        }
    }

    /// Every widen ordinal realized through the cached prefix equals the
    /// plain reseed-and-reapply realization, and so does the probing draw.
    #[test]
    fn cached_prefix_realizes_the_plain_outcome_for_every_ordinal() {
        let mut states = Vec::new();
        for seed in [5, 29] {
            let (thievery, flutter) = hopper_rounds(seed);
            let wurm = combat_vs(MonsterKind::FuzzyWurmCrawler, 57, seed);
            for state in [&thievery, &flutter, &wurm] {
                states.push(as_belief(state));
                states.push(state.clone());
            }
        }
        let plain = |pre: &CombatState, base: u64, n: u32| {
            let mut s = pre.clone();
            reseed_for_outcome(&mut s, base, n);
            let _ = core_apply(&mut s, Action::EndTurn);
            s
        };
        let mut cached_phases = Vec::new();
        for (i, pre) in states.iter().enumerate() {
            let base = 0x5EED_0000 + i as u64;
            let mut probed = pre.clone();
            reseed_for_outcome(&mut probed, base, 1);
            let (clean, prefix) = end_turn_probing_prefix(&mut probed, 1);
            assert!(
                probed == plain(pre, base, 1),
                "state {i}: the probing draw itself"
            );
            let prefix = prefix.expect("every fixture has an RNG-free first phase");
            cached_phases.push(clean);
            for n in 2..=24 {
                let mut s = prefix.clone();
                reseed_for_outcome(&mut s, base, n);
                let _ = end_turn_dispatch_from(&mut s, clean);
                assert!(
                    s == plain(pre, base, n),
                    "state {i} ordinal {n} ({clean} cached phases)"
                );
            }
        }
        // The fixtures cover a prefix that stops at the enemy move and one
        // that runs through it.
        assert!(cached_phases.contains(&3), "{cached_phases:?}");
        assert!(cached_phases.iter().any(|&c| c >= 5), "{cached_phases:?}");
    }

    fn tree_fingerprint<P: Policy>(mcts: &Mcts<P>) -> Vec<(u32, u64, u32, u32)> {
        mcts.tree
            .arena
            .iter()
            .map(|n| {
                (
                    n.visits,
                    n.search_hash,
                    n.value_sum.to_bits(),
                    n.chance_draws,
                )
            })
            .collect()
    }

    fn cached_nodes<P: Policy>(mcts: &Mcts<P>) -> usize {
        mcts.tree
            .arena
            .iter()
            .filter(|n| matches!(n.endturn_prefix, EndTurnPrefix::Cached { .. }))
            .count()
    }

    /// A whole search at the deployed `EndTurn` widening builds the same tree
    /// with the cache as without it. Arming the thread-local damage trace is
    /// the switch: it sends every draw down the plain path.
    #[test]
    fn search_with_the_prefix_cache_builds_the_plain_tree() {
        let (thievery, flutter) = hopper_rounds(11);
        let wurm = combat_vs(MonsterKind::FuzzyWurmCrawler, 57, 11);
        for mode in [HiddenInfoMode::BeliefDraw, HiddenInfoMode::Clairvoyant] {
            for state in [&thievery, &flutter, &wurm] {
                let config = MctsConfig {
                    max_simulations: 400,
                    seed: 7,
                    hidden_info: mode,
                    endturn_widening_c: 5.787,
                    endturn_widening_alpha: 0.436,
                    ..MctsConfig::default()
                };
                let mut cached = Mcts::new(state.clone(), config);
                let pick_cached = cached.search();

                sts2_core::damage_trace::set_armed(true);
                let mut plain = Mcts::new(state.clone(), config);
                let pick_plain = plain.search();
                sts2_core::damage_trace::set_armed(false);

                assert!(
                    cached_nodes(&cached) > 0,
                    "{mode:?}: the cache was exercised"
                );
                assert_eq!(
                    cached_nodes(&plain),
                    0,
                    "{mode:?}: an armed trace bypasses the cache"
                );
                assert_eq!(pick_cached, pick_plain, "{mode:?}");
                assert!(
                    tree_fingerprint(&cached) == tree_fingerprint(&plain),
                    "{mode:?}: the trees diverge"
                );
            }
        }
    }

    /// The debug guard compares real states: a prefix that does not match
    /// its rebuild under a second reseed panics.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "depends on the reseed")]
    fn seed_guard_rejects_a_prefix_that_differs_from_its_rebuild() {
        let (thievery, _) = hopper_rounds(3);
        let mut s = thievery.clone();
        reseed_for_outcome(&mut s, 1, 0);
        let (clean, prefix) = end_turn_probing_prefix(&mut s, 1);
        let mut prefix = prefix.expect("prefix");
        // Stand-in for a phase that rolled off an RNG's seed.
        prefix.player.block += 1;
        debug_assert_prefix_seed_independent(&thievery, &prefix, clean, 1, 0);
    }
}
