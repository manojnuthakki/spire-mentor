//! `sts2-corpus-gen` — MCTS self-play corpus generator for Texel
//! weight tuning. Bead `sts2-rs-ulbl` (vsqc.1).
//!
//! Drives MCTS-policy self-play across a diverse Act-1 encounter
//! set, records the 12-column feature vector
//! ([`sts2_core::eval_features`]) at every player decision point,
//! and labels each captured position with the eventual combat
//! outcome (1 = won, 0 = lost). Filters positions to `round >=
//! min_round` (default 2) and writes the result as NPZ via
//! [`sts2_mcts::npz::write_npz`].
//!
//! ## Sister-bead interface
//!
//! The 12-column layout is pinned by `sts2-rs-uxyf` (vsqc.2):
//! `sts2_core::EVAL_FEATURE_NAMES`. The downstream Texel harness
//! (`sts2-rs-tutm` / vsqc.3) consumes the NPZ output and fits
//! per-column weights against `EVAL_FEATURE_WEIGHTS`. Re-ordering
//! columns MUST flow through all three beads in lockstep.
//!
//! ## Usage
//!
//! ```text
//! sts2-corpus-gen \
//!     --n-positions 100000 \
//!     --output /tmp/texel-corpus.npz \
//!     --sims-per-move 300 \
//!     --max-rollout-depth 10 \
//!     --min-round 2 \
//!     --seed 42
//! ```
//!
//! See `docs/perf/2026-05-17-texel-corpus.md` for the feature
//! column order + encounter-sampler rationale.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, ValueEnum};
use glob::glob;
use rayon::prelude::*;
use rayon::ThreadPoolBuilder;
use serde::Deserialize;
use smallvec::SmallVec;
use sts2_cli::scenario_to_combat_state;
use sts2_cli::sim::scenario::toml_file::{load_scenario_file, ScenarioFile};
use sts2_content::cards::defect::helpers::defect_starter_deck;
use sts2_content::cards::ironclad_basic::ironclad_starter_deck;
use sts2_content::cards::necrobinder::necrobinder_starter_deck;
use sts2_content::cards::regent::regent_starter_deck;
use sts2_content::cards::silent::helpers::silent_starter_deck;
use sts2_content::cards::spec_index::{
    DEFECT_SPECS, IRONCLAD_SPECS, NECROBINDER_SPECS, REGENT_SPECS, SILENT_SPECS,
};
use sts2_content::cards::CardSpec;
use sts2_content::encounters::act1_overgrowth::ACT1_ENCOUNTERS;
use sts2_content::encounters::EncounterTier;
use sts2_content::monster_dispatch::{build_ai, SpawnPosition};
use sts2_content::names::{resolve_card_name, PlayerClassHint};
use sts2_core::cards::Card;
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::WIDE_FEATURE_DIM;
use sts2_core::{
    apply, is_terminal, Action, CardUid, CombatPhase, CombatState, Creature, CreatureUid,
    IntentHistory, MonsterId, OrbSlots, PlayerCombatState, RelicKind, Side, TurnEvents,
};
use sts2_mcts::corpus::{
    fnv1a_64, CorpusCollector, CorpusCollectorWide, PolicyActionSpace, TIER_BOSS, TIER_COMMON,
    TIER_ELITE, TIER_UNKNOWN,
};
use sts2_mcts::corpus::{
    CARD_FEAT_DIM, ENEMY_SLOT_MAX, HAND_SLOT_MAX, PILE_SLOT_MAX, RELIC_SLOT_MAX,
};
use sts2_mcts::expert::RolloutPolicyKind;
use sts2_mcts::npz::{
    write_npz_narrow_with_provenance, write_npz_with_embeddings,
    write_npz_with_extras_and_provenance,
};
use sts2_mcts::{
    sample_rollout_leaf_state, EmbeddingValueNet, HiddenInfoMode, LeafEval, LearnedPolicyPrior,
    LearnedPolicyPriorStats, LearnedValueNet, Mcts, MctsConfig, CANONICAL_ACTION_DIM,
};
use sts2_runai::policy::COMBAT_ROLLOUT_DEPTH;

/// Polymorphic wrapper so `run_self_play` can drive either schema
/// without code duplication. Both arms expose the same surface
/// (record / mark / filter / len / is_empty / into_arrays); the
/// only schema-aware split is the column count handed to
/// [`write_npz`] at the end of `main`.
enum Collector {
    Narrow(CorpusCollector),
    Wide(CorpusCollectorWide),
}

impl Collector {
    /// Wave-6 visit-aware capture. Wide arm forwards to
    /// `CorpusCollectorWide::record_position_with_visits`; Narrow
    /// arm ignores the visits (no policy-head column to populate)
    /// and the `root_q` TD target (no value-head TD column either —
    /// bead `sts2-rs-wrip2`).
    fn record_position_with_visits(
        &mut self,
        state: &CombatState,
        visits: &[(sts2_core::Action, u32)],
        root_q: Option<f32>,
    ) {
        match self {
            Collector::Narrow(c) => c.record_position(state),
            Collector::Wide(c) => c.record_position_with_visits(state, visits, root_q),
        }
    }

    /// Wave-6 HP-aware backfill. Wide arm forwards to
    /// `CorpusCollectorWide::mark_combat_outcome_with_hp`; Narrow
    /// arm ignores the HP fraction (no aux-HP column to populate).
    fn mark_combat_outcome_with_hp(&mut self, won: bool, final_hp_norm: f32) {
        match self {
            Collector::Narrow(c) => c.mark_combat_outcome(won),
            Collector::Wide(c) => c.mark_combat_outcome_with_hp(won, final_hp_norm),
        }
    }

    fn filter_round_at_least(&mut self, min: u16) -> usize {
        match self {
            Collector::Narrow(c) => c.filter_round_at_least(min),
            Collector::Wide(c) => c.filter_round_at_least(min),
        }
    }

    fn len(&self) -> usize {
        match self {
            Collector::Narrow(c) => c.len(),
            Collector::Wide(c) => c.len(),
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Collector::Narrow(c) => c.is_empty(),
            Collector::Wide(c) => c.is_empty(),
        }
    }

    fn columns(&self) -> usize {
        match self {
            // Pinned by `sts2_core::EVAL_FEATURE_NAMES.len()` — bumped
            // 12 → 16 across the Wave-1+2+3 leaf-eval-term beads (ns1s
            // / 5l4y / gzbh / ot78). Source the count via
            // `EVAL_FEATURE_NAMES` instead of a magic number so future
            // schema bumps don't desync the NPZ writer assertion.
            Collector::Narrow(_) => sts2_core::EVAL_FEATURE_NAMES.len(),
            Collector::Wide(_) => WIDE_FEATURE_DIM,
        }
    }

    /// sts2-rs-3b0p: merge a per-worker collector into `self`. Both
    /// arms must be the same variant — mixing Narrow + Wide is a
    /// programmer error (the corpus generator constructs all workers
    /// via the same `make_collector` helper, so the variants always
    /// match in practice).
    fn merge(&mut self, other: Self) {
        match (self, other) {
            (Collector::Narrow(a), Collector::Narrow(b)) => a.merge(b),
            (Collector::Wide(a), Collector::Wide(b)) => a.merge(b),
            _ => panic!("Collector::merge: variant mismatch between driver and worker collectors"),
        }
    }

    /// sts2-rs-3b0p: truncate to at most `target_len` entries.
    fn truncate(&mut self, target_len: usize) {
        match self {
            Collector::Narrow(c) => c.truncate(target_len),
            Collector::Wide(c) => c.truncate(target_len),
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "sts2-corpus-gen",
    about = "Generate a Texel-tuning corpus via MCTS self-play (bead sts2-rs-ulbl / vsqc.1)"
)]
struct Args {
    /// Target number of filtered positions (rows in features.npy).
    #[arg(long, default_value_t = 100_000)]
    n_positions: usize,

    /// Output `.npz` path. Will be overwritten if it exists.
    #[arg(long, default_value = "/tmp/texel-corpus.npz")]
    output: PathBuf,

    /// MCTS simulation budget per move. The plan §10.4 suggests
    /// 200-500; default 300 splits the range.
    #[arg(long, default_value_t = 300)]
    sims_per_move: u32,

    /// MCTS rollout depth cap. Lower values keep self-play turns
    /// short — `evaluate_in_progress` (the leaf-eval under Texel)
    /// is consulted whenever the rollout hits this cap, so this
    /// is the dominant trigger for the leaf-eval cost centers
    /// the corpus is supposed to inform.
    #[arg(long, default_value_t = 10)]
    max_rollout_depth: u32,

    /// Rollout policy to use during self-play corpus generation
    /// (GitHub #24). Historically the corpus generator hard-coded
    /// `Uniform`, while production MCTS defaults to `Heuristic` —
    /// that mismatch meant policy-head training targets reflected a
    /// different search process than the one running at inference.
    /// Default is now `heuristic` (matches `MctsConfig::default()`);
    /// set to `uniform` only if you specifically want to reproduce the
    /// pre-#24 corpus.
    #[arg(long, value_enum, default_value_t = CliRolloutPolicy::Heuristic)]
    rollout_policy: CliRolloutPolicy,

    /// Search dialect for the self-play teacher — see [`CliSearchDialect`].
    /// `legacy` (default) keeps every existing corpus recipe byte-stable;
    /// `deployed` matches the production combat search, which is what a
    /// policy prior destined for that search has to be distilled from.
    #[arg(long, value_enum, default_value_t = CliSearchDialect::Legacy)]
    search_dialect: CliSearchDialect,

    /// **EXPERIMENTAL** (bead `sts2-rs-2arxw` / v4wjd.1, scaffold
    /// only): record additional mid-rollout state snapshots per
    /// combat, labelled by the eventual terminal_win (KataGo-style).
    /// At this commit the flag is parsed + validated + plumbed into
    /// the per-combat collector path, but the actual snapshot
    /// emission is a no-op pending the follow-up bead that hooks
    /// into the MCTS rollout. `--mid-rollout-samples 0` (default) is
    /// the only value that's bit-for-bit deterministic with the
    /// pre-`2arxw` corpus; other values warn-and-no-op until the
    /// emit logic lands.
    ///
    /// Acceptance for the follow-up bead: the snapshot site is
    /// wherever `MctsRollout` is about to call the leaf evaluator;
    /// see `docs/perf/2026-05-21-v4wjd-mid-rollout-decomposition.md`.
    #[arg(long, default_value_t = 0)]
    mid_rollout_samples: u32,

    /// ExIt (sts2-rs-1bcx): path to a wide-172 `value_policy_net.py`
    /// weights JSON (`LearnedValueNet::from_weights_path`). When set,
    /// self-play uses PUCT + `LearnedPolicyPrior` (the trained policy
    /// head) to drive search instead of UCB + uniform, so the recorded
    /// visit-count targets reflect the *improved* searcher — the data
    /// half of one Expert-Iteration step. Absent, generation is
    /// byte-identical to the pre-1bcx path.
    #[arg(long)]
    policy_prior_weights: Option<PathBuf>,

    /// Loaded form of `policy_prior_weights`. Not a CLI arg — populated
    /// once in `main` after parse so the net loads a single time and
    /// rides the existing `&Args` borrow across the rayon self-play
    /// boundary (clap `skip` defaults this to `None`).
    #[arg(skip)]
    prior_net: Option<Arc<LearnedValueNet>>,

    /// Shared stats handle for the PUCT prior (sts2-rs-1bcx). Populated
    /// alongside `prior_net` in `main`; `run_self_play` builds each
    /// per-move prior with `with_stats(this)` so fallback/collision
    /// counters aggregate across the whole run for an end-of-run summary.
    #[arg(skip)]
    prior_stats: Option<Arc<LearnedPolicyPriorStats>>,

    /// ExIt (sts2-rs-x9hyn): path to a `value_policy_net_generalist.py`
    /// weights JSON (`EmbeddingValueNet::from_weights_path`). When set,
    /// self-play uses PUCT + `LearnedPolicyPrior<EmbeddingValueNet>` as
    /// the prior AND `LeafEval::LearnedEmbed` as the leaf eval — the
    /// embedding-net data half of one Expert-Iteration step. Mutually
    /// exclusive with `--policy-prior-weights`. Requires
    /// `--with-embeddings` so the generated corpus carries ids for the
    /// next iteration's trainer.
    #[arg(long, conflicts_with = "policy_prior_weights")]
    embed_prior_weights: Option<PathBuf>,

    /// ExIt (sts2-rs-x9hyn, Phase 3): use the `--embed-prior-weights` net
    /// as the PUCT prior ONLY, leaving the leaf eval at the default
    /// `LeafEval::Texel` instead of `LeafEval::LearnedEmbed`. This is the
    /// "policy-only" recipe — it keeps the dead learned value head out of
    /// the self-play loop (Texel is a strictly better leaf eval; see
    /// `docs/perf/2026-06-06-deepsets-exit-iteration.md`). No effect unless
    /// `--embed-prior-weights` is also set. Default false preserves the
    /// iteration-1 (prior + learned leaf) behavior byte-for-byte.
    #[arg(long, requires = "embed_prior_weights")]
    embed_prior_texel_leaf: bool,

    /// Loaded form of `embed_prior_weights` (not a CLI arg).
    #[arg(skip)]
    embed_prior_net: Option<Arc<EmbeddingValueNet>>,

    /// Phase 2 ExpI (leaf co-improvement): path to a
    /// `texel_tune.py --target value` weights JSON (`fitted_weights_ordered`).
    /// When set, self-play drives the rollout depth-cap with the champion's
    /// runtime wide leaf (`LeafEval::TexelWideRuntime`) instead of the baked-in
    /// `TexelWide` const, so each round's `mcts_root_q` value targets reflect
    /// the *improving* leaf. Composes with `--embed-prior-texel-leaf` (the
    /// embedding net stays the PUCT prior); rejected together with a
    /// `LearnedEmbed` leaf (`--embed-prior-weights` without
    /// `--embed-prior-texel-leaf`) since the leaf source would be ambiguous.
    #[arg(long)]
    leaf_weights_path: Option<PathBuf>,

    /// Loaded form of `leaf_weights_path` (not a CLI arg).
    #[arg(skip)]
    leaf_weights: Option<Arc<[f32; sts2_core::WIDE_FEATURE_DIM]>>,

    /// Drop captured positions with `round < min_round`. Texel
    /// default is 2 — first-turn positions are dominated by the
    /// random starting-hand draw and contribute noise.
    #[arg(long, default_value_t = 2)]
    min_round: u16,

    /// RNG seed for encounter sampling + MCTS root seed.
    #[arg(long, default_value_t = 42)]
    seed: u64,

    /// Optional cap on the number of self-play combats to attempt
    /// before bailing — guards against pathological non-terminating
    /// fixtures. Default 50× n_positions / 8 (assumes ~8 captures
    /// per combat).
    #[arg(long)]
    max_combats: Option<usize>,

    /// Emit the Wave-6 wide-schema feature vector
    /// (`sts2_core::eval_features_wide`, 80 columns pinned by
    /// `EVAL_FEATURE_NAMES_WIDE`) instead of the 12-feature Texel
    /// schema. Bead `sts2-rs-4vo6` (qctu.2). The downstream
    /// AlphaZero-style trainer (qctu.3) consumes the wide
    /// schema; the Texel chain stays on the narrow 12-column
    /// output.
    #[arg(long, default_value_t = false)]
    wide: bool,

    /// Wave-6 generalist-net id-array extension (bead `sts2-rs-uqnh`).
    /// When set together with `--wide`, the NPZ also contains the six
    /// per-slot id arrays consumed by
    /// `scripts/value_policy_net_generalist.py` (card_ids_{hand,draw,
    /// discard,exhaust}, monster_slot_ids, relic_set_ids — all `<u2`
    /// row-major; `0` is NULL). Has no effect without `--wide`.
    #[arg(long, default_value_t = false)]
    with_embeddings: bool,

    /// Worker count for the rayon-parallel combat loop
    /// (`sts2-rs-3b0p`). `0` (default) uses rayon's global pool
    /// size. Set explicitly to pin the worker count — the integration
    /// test in `crates/sts2-cli/tests/corpus_gen_determinism.rs`
    /// asserts that `--workers 1` and `--workers 8` produce
    /// byte-identical NPZ output for the same `--seed`.
    #[arg(long, default_value_t = 0)]
    workers: usize,

    /// Character to self-play (Regent-combat-leaf plan, Task 2). Default
    /// `ironclad` is fully backward compatible — byte-identical starter
    /// deck / card pool / sts2log fixture default to every invocation
    /// that predates this flag. `regent` swaps in `regent_starter_deck()`
    /// (`--deck-source starter`), `REGENT_SPECS`
    /// (`--deck-source random-full-pool`), and the
    /// `/tmp/run-fixtures-full/the-regent` fixture default
    /// (`--deck-source sts2log-perfect`). Act-1 encounter sampling is
    /// shared across characters (`ACT1_ENCOUNTERS` has no per-character
    /// gating).
    #[arg(long, value_enum, default_value_t = CliCharacter::Ironclad)]
    character: CliCharacter,

    /// Initial deck composition source (bead `sts2-rs-2vwf`). Per
    /// `feedback_corpus_starter_deck_too_simple`, the default
    /// `starter` produces 99.7%-win training corpora that saturate
    /// learned value heads (root-cause of the 7njh +0.00pp FAIL).
    /// `sts2log-perfect` pulls human-played decks from
    /// `--sts2log-fixtures-dir`; `random-full-pool` samples N cards
    /// from the full `--character` registry. Encounter sampling is
    /// unchanged across all three.
    #[arg(long, value_enum, default_value_t = DeckSource::Starter)]
    deck_source: DeckSource,

    /// Directory of `.toml` fixtures used when
    /// `--deck-source sts2log-perfect`. When unset (default), resolved
    /// from `--character`: `/tmp/run-fixtures-full/the-ironclad` or
    /// `.../the-regent` (the `eh83` scrape landing points).
    #[arg(long)]
    sts2log_fixtures_dir: Option<PathBuf>,

    /// Deck size when `--deck-source random-full-pool`. 15 matches
    /// the floor-2 deck length on most sts2log fixtures.
    #[arg(long, default_value_t = 15)]
    random_full_pool_size: usize,

    /// Scenario source (bead `sts2-rs-ctdr`). `synthetic` (default)
    /// keeps the existing behavior — sample encounter from
    /// `ACT1_ENCOUNTERS`, pull deck from `--deck-source`, build
    /// state inline via `build_combat_state`. `toml` switches to
    /// real-run fixtures: each combat pulls a `.toml` ScenarioFile
    /// via `--fixture-glob`, runs the relic blocklist, and
    /// materializes the full starting `CombatState` via
    /// `sts2_cli::scenario_to_combat_state`. The toml path
    /// supersedes `--deck-source` (whole scenario, not just deck).
    #[arg(long, value_enum, default_value_t = Source::Synthetic)]
    source: Source,

    /// Glob pattern of `.toml` scenario fixtures (only used when
    /// `--source toml`). Example:
    /// `/tmp/run-fixtures-combined/ironclad/*.toml`. Matched paths
    /// are loaded eagerly at startup; fixtures whose `[player].relics`
    /// list references any relic *not* in the runtime `RelicKind`
    /// enum are skipped (auto-derived blocklist). Pool is sorted by
    /// path for cross-host reproducibility; per-combat indexing is
    /// `combat_idx % pool.len()`, identical to the `--deck-source
    /// sts2log-perfect` shape.
    #[arg(long)]
    fixture_glob: Option<String>,

    // ── Hard-fixture-seeding flags (bead `sts2-rs-t4tn`) ──────────
    // Approach (A) from the bead: filter the loaded fixture pool by
    // provenance metadata so the corpus skews toward harder runs.
    // All four are no-ops when --source != toml.
    /// Drop fixtures whose `[provenance].floor` is below this value.
    /// Deeper-floor fixtures pair larger decks with harder
    /// encounters, which `sts2-rs-2dgj` showed pushes the win-rate
    /// distribution downward into the balanced-corpus band. `0`
    /// disables (default, preserves prior behavior).
    #[arg(long, default_value_t = 0)]
    floor_min: u32,
    /// Drop fixtures whose `[player].ascension` is below this value.
    /// Higher-ascension runs face harder enemies / smaller HP pools
    /// (in the source game) and thus contribute more loss-tail
    /// states. Engine is ascension-identity at this pin so the
    /// filter affects fixture *selection*, not in-engine difficulty.
    /// `0` disables (default).
    #[arg(long, default_value_t = 0)]
    ascension_min: u32,
    /// Filter fixtures by `[provenance].player_outcome` field.
    /// `any` (default) keeps everything; `won` keeps only fixtures
    /// the human won; `lost` keeps only fixtures from death-floors.
    /// Note: per `os46`'s structural diagnosis, sts2log's capture
    /// pipeline drops most `lost` fixtures — the `lost` setting
    /// currently filters to a near-empty pool. Re-evaluate once the
    /// scraper's loss-capture path lands.
    #[arg(long, value_enum, default_value_t = PlayerOutcomeFilter::Any)]
    player_outcome: PlayerOutcomeFilter,
    /// Hard ceiling on the fraction of combats that may abort via
    /// `catch_unwind` (content-gap panics — unported relic / card
    /// dispatch). When the running rate exceeds this, the binary
    /// exits with code 3 and a forensics line listing the top
    /// content gaps in the dropped pool. Surfaces content gaps as
    /// build-blockers rather than silent corpus-shrinkers (per
    /// `os46`'s diagnosis: hard fixtures are exactly the ones that
    /// drop, biasing the survivor pool toward easier states).
    /// `1.0` (default) disables the gate.
    #[arg(long, default_value_t = 1.0)]
    max_aborts_fraction: f32,

    /// Bead `sts2-rs-acbj` (step 3): drop fixtures whose
    /// `[provenance].deck_perfect` flag is not `true`. The scraper
    /// round-tripped ~93% of .run-source decks losslessly; the
    /// remaining ~7% have incomplete decks (the event log was
    /// missing draw / purge / transform events). Playing those
    /// fixtures with wrong decks pollutes the win-rate label.
    /// Default `false` (back-compat); recommended `true` for any
    /// acbj training run.
    #[arg(long, default_value_t = false)]
    require_deck_perfect: bool,

    /// Policy-target action-slot scheme. `canonical` (640, card-id only;
    /// default, byte-stable) or `keyed` (4672, card × target-class) —
    /// sts2-rs-1bcx follow-up. Only meaningful with `--wide`.
    #[arg(long, value_enum, default_value_t = CliPolicyActionSpace::Canonical)]
    policy_action_space: CliPolicyActionSpace,

    /// Combats per simulated run when `--source run-manager` (Phase 0a).
    /// `combat_idx` decomposes into `run_ordinal = idx / N` and
    /// `target_ordinal = idx % N`; the run for `run_ordinal` is replayed
    /// with the heuristic up to its `target_ordinal`-th combat, which is
    /// then snapshotted + MCTS-self-played. `target_ordinal`s the run
    /// never reaches (it died earlier) yield an empty collector that is
    /// dropped as an abort, so `N` should be set near the heuristic's
    /// realistic survival depth to avoid wasted idx slots (Act-1 is
    /// ~5-6 combats; default 6). Raising `N` widens floor coverage at
    /// the cost of more aborts. No-op for other `--source` values.
    #[arg(long, default_value_t = 6)]
    runmanager_combats_per_run: usize,

    /// Drive the run's *non-target* (fast-forward) combats with MCTS
    /// instead of the cheap heuristic, so runs survive deeper and the
    /// sampled entry states reach elite/boss tiers. Uses the loaded
    /// embedding prior as a PUCT prior when `--embed-prior-weights` is
    /// set, else plain UCB. Costs ≈ `target_ordinal` extra low-sim
    /// searches per recorded combat. Required to get past the heuristic's
    /// ≈95 %-common / 0 %-boss distribution (Gate-0 FAIL). No-op unless
    /// `--source run-manager`.
    #[arg(long, default_value_t = false)]
    runmanager_mcts_progress: bool,

    /// MCTS sims/move for run-progression combats when
    /// `--runmanager-mcts-progress`. Kept low (these combats only advance
    /// the run; only the *target* combat is recorded at full
    /// `--sims-per-move`). No-op otherwise.
    #[arg(long, default_value_t = 64)]
    runmanager_progress_sims: u32,

    /// God-mode HP for run *progression* when `--source run-manager`
    /// (`0` = off, default). When > 0, the run starts at this HP (and is
    /// re-topped to it after every fast-forward combat) so the run cannot
    /// die and traverses to the deep floors — Act-1/2/3 elites and bosses
    /// — accumulating a real deck + relics. This is the "cheat survival,
    /// keep the deck/relic/floor context real" lever that breaks the
    /// deckbuilding deadlock (the heuristic wins every fight by attrition
    /// once it cannot die). Use a large value (e.g. 100000) so a single
    /// long Act-3 fight can't drain it. The recorded *target* combat's HP
    /// is reset to `--runmanager-snapshot-hp` first, so only progression
    /// is cheated — the recorded fight + its win/loss label are faithful.
    /// No-op unless `--source run-manager`.
    #[arg(long, default_value_t = 0)]
    runmanager_godmode_hp: i32,

    /// Realistic HP the recorded combat snapshot is reset to when
    /// `--runmanager-godmode-hp` is on (both max + current; default 80 =
    /// full Ironclad). Set < 80 to record boss/elite fights from an
    /// "arrived hurt" HP, biasing the outcome distribution toward the
    /// loss tail the value head needs. No-op when god-mode is off (the
    /// snapshot keeps the run's real HP).
    #[arg(long, default_value_t = 80)]
    runmanager_snapshot_hp: i32,

    /// Carry the god-run's real potion belt into each exported fixture
    /// (`[player].potions`). Default OFF, and deliberately so: the belt
    /// changes the realized `CombatState`, hence its `state_hash`, and the
    /// committed decision-quality suites (`combat_position_book*.json`,
    /// `dominance_pairs.json`) pin the hash of a belt-less pool. Turn this on
    /// for a *new* pool (`harvest.sh` with `EXPORT_POTIONS=1` writes it next
    /// to the old one) rather than regenerating the one the frozen suites
    /// verify against. GH #446: the v2 book had zero positions where a potion
    /// was available, because every fixture carried `potions = []`.
    #[arg(long, default_value_t = false)]
    runmanager_export_potions: bool,

    /// Phase 1a reference-search value targets. When > 0, each recorded
    /// position's value target (`mcts_root_q`) is relabeled from a SECOND,
    /// stronger search at this sim budget on the same pre-decision state
    /// (the action-selection search still runs at `--sims-per-move` and
    /// supplies the visit/policy target). Set above `--sims-per-move` so
    /// the net distills "leaf + a stronger search's lookahead" instead of
    /// mimicking the inference-time leaf (the Texel-Q circularity). Pairs
    /// with the trainer's `--value-td-lambda 1.0`. `0` (default) keeps the
    /// action-search Q. Source-agnostic (works for synthetic/toml/
    /// run-manager). Roughly doubles per-position search cost.
    #[arg(long, default_value_t = 0)]
    reference_sims: u32,

    /// Rollout depth for the reference value search (`--reference-sims`).
    /// Defaults to 10 (the usual inference depth); raise to capture more
    /// lookahead in the value target. No-op unless `--reference-sims > 0`.
    #[arg(long, default_value_t = 10)]
    reference_depth: u32,

    /// Phase 0b: export a held-out on-manifold EVAL set instead of an NPZ
    /// training corpus. Drives god-mode `--source run-manager` runs and
    /// writes one `.toml` `ScenarioFile` fixture per sampled combat entry
    /// (deck incl. upgrades + monster roster + HP reset to
    /// `--runmanager-snapshot-hp`) plus a `corpus.json` (`RealisticCorpus`
    /// shape) into this directory, which the A/B harness loads natively
    /// via `--corpus-path <dir>/corpus.json`. Use a seed DISJOINT from the
    /// training corpus's. Fixtures carry the god-run's real accumulated
    /// relics (filtered to ids with a runtime `RelicKind`) so combat-entry
    /// relic effects fire on load (e.g. Regent DivineRight +3 stars);
    /// potions are still emitted empty (belt not mirrored yet). Implies
    /// `--source run-manager`; all NPZ flags are ignored in this mode.
    #[arg(long)]
    runmanager_toml_export: Option<PathBuf>,

    /// Number of `.toml` fixtures to emit when `--runmanager-toml-export`
    /// (default 200, matching realistic_v2's size). Runs that die before
    /// their target combat are skipped, so more runs may be driven.
    #[arg(long, default_value_t = 200)]
    runmanager_eval_count: usize,

    /// Ascension for the god-mode fixture-export runs (0-10). The run
    /// is driven at this ascension so encounter composition, A8
    /// ToughEnemies HP rolls, and accumulated relics are all
    /// on-distribution for it, and the emitted `[player]` block
    /// records it so ascension-aware loaders (Task 2) can rebuild
    /// A9 DeadlyEnemies attack scaling at realize time. Default 0 =
    /// byte-identical to the pre-flag export.
    #[arg(long, default_value_t = 0)]
    runmanager_ascension: u8,
}

/// CLI mirror of [`RolloutPolicyKind`] — clap's `ValueEnum` impl can't
/// be derived for the upstream enum from this crate. GitHub #24.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum CliRolloutPolicy {
    /// Uniform-random rollout — the pre-#24 corpus generator behavior.
    Uniform,
    /// Softmax over expert play priority. Matches
    /// `MctsConfig::default()` and the production search.
    Heuristic,
}

impl From<CliRolloutPolicy> for RolloutPolicyKind {
    fn from(value: CliRolloutPolicy) -> Self {
        match value {
            CliRolloutPolicy::Uniform => RolloutPolicyKind::Uniform,
            CliRolloutPolicy::Heuristic => RolloutPolicyKind::Heuristic,
        }
    }
}

/// Which search dialect the self-play *teacher* runs in.
///
/// The policy head is trained on the teacher's root visit distribution, so
/// the teacher's dialect is baked into every target. `--rollout-policy`
/// already exists for exactly this reason (GH #24: "that mismatch meant
/// policy-head training targets reflected a different search process than
/// the one running at inference") — but it only fixed the rollout, and the
/// rest of the config has drifted much further since.
///
/// `Legacy` is `MctsConfig::default()`: **Clairvoyant** hidden info and
/// `c_puct = 1.41`. Production has been honest-by-default since the
/// 2026-07-12 G4 bless, so a Legacy teacher's visit counts depend on the
/// draw-pile order that the student at inference time cannot see. Distilling
/// that gives the student a target it has no way to reproduce.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum CliSearchDialect {
    /// `MctsConfig::default()` — Clairvoyant, `c_puct` 1.41. Byte-stable
    /// against every corpus generated before this flag existed.
    Legacy,
    /// The deployed combat dialect (`sts2-runai::mcts_config_for` under
    /// `STS2_RUNAI_HONEST` unset): `BeliefDraw` plus
    /// [`MctsConfig::apply_mode_dialect`] — `c_puct` from
    /// `belief_draw_c_puct` of the budget (2.5 at >= 1000 sims),
    /// `draw_widening_c` 2.0 and the PR #467 EndTurn widening 5.787 / 0.436.
    /// Before GH #758 this stamped only the first two, so a "deployed"
    /// corpus searched EndTurn at the pre-#467 0.5 / 0.4 and a flat
    /// `c_puct` 2.5 at every budget. Pair with `--max-rollout-depth 200`
    /// (`COMBAT_ROLLOUT_DEPTH`) — the depth keeps its own flag, and a
    /// mismatch is warned about rather than silently corrected.
    Deployed,
}

impl CliSearchDialect {
    /// Stamp the dialect onto a config built from `MctsConfig::default()`
    /// with its `max_simulations` already set (the `c_puct` is
    /// budget-keyed). `Legacy` is a no-op, so every pre-existing call site
    /// is unchanged.
    fn apply(self, cfg: &mut MctsConfig) {
        if self == Self::Deployed {
            cfg.hidden_info = HiddenInfoMode::BeliefDraw;
            cfg.apply_mode_dialect();
        }
    }
}

/// CLI mirror of [`PolicyActionSpace`] — clap's `ValueEnum` impl can't
/// be derived for the upstream enum from this crate (sts2-rs-1bcx).
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum CliPolicyActionSpace {
    /// Card-id only (640 slots); default, byte-stable vs pre-1bcx output.
    Canonical,
    /// Card × target-class (4672 slots) — ExIt Lever-A follow-up.
    Keyed,
}

impl From<CliPolicyActionSpace> for PolicyActionSpace {
    fn from(value: CliPolicyActionSpace) -> Self {
        match value {
            CliPolicyActionSpace::Canonical => PolicyActionSpace::Canonical,
            CliPolicyActionSpace::Keyed => PolicyActionSpace::Keyed,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum PlayerOutcomeFilter {
    /// Keep all fixtures regardless of `[provenance].player_outcome`.
    Any,
    /// Keep only fixtures where `player_outcome = "won"`.
    Won,
    /// Keep only fixtures where `player_outcome = "lost"`.
    /// **Currently a near-empty pool** — sts2log captures drop
    /// dead-run fixtures (per `os46`). Plumbing is here for when
    /// the scraper's loss-capture path lands.
    Lost,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum Source {
    /// Existing synthetic-encounter + sampled-deck path. The
    /// `--deck-source` flag controls the deck pool; encounter is
    /// sampled from `ACT1_ENCOUNTERS` per-combat.
    Synthetic,
    /// New (bead `sts2-rs-ctdr`) full-scenario path. Pulls real-run
    /// fixtures via `--fixture-glob`, filters by the auto-derived
    /// relic blocklist, realizes via
    /// `sts2_cli::scenario_to_combat_state`. `--deck-source` and
    /// the encounter sampler are ignored in this mode.
    Toml,
    /// On-manifold self-play path (Phase 0a of the AlphaZero bet,
    /// `docs/plans/2026-06-07-alphazero-onmanifold-selfplay-bet.md`).
    /// Instead of synthetic encounters or scraped fixtures, drive a
    /// real `RunManager` headlessly with a cheap heuristic
    /// navigation+combat policy and snapshot the `CombatState` the
    /// run-loop actually constructs at a combat *entry* (real deck
    /// reached via real card rewards, real HP after prior fights, real
    /// potions, real act/floor context). The snapshot is then
    /// MCTS-self-played to terminal exactly like the synthetic path, so
    /// the captured rows are on-manifold rather than drawn from a
    /// static fixture distribution. `combat_idx` maps to
    /// `(run_seed, target_ordinal)` via `--runmanager-combats-per-run`;
    /// each idx replays a run with the heuristic up to its
    /// `target_ordinal`-th combat, snapshots, and records that one
    /// combat. `--deck-source` / `--fixture-glob` are ignored.
    /// Floor/tier coverage is bounded by the heuristic's survival depth
    /// (boss-tier coverage is thin until the run-progression policy is
    /// strengthened — see the plan doc); the per-row `encounter_tier_u8`
    /// provenance column lets `scripts/corpus_tier_histogram.py` check
    /// the realized distribution (Gate 0).
    RunManager,
}

/// CLI `--character` selector (Regent-combat-leaf plan, Task 2).
///
/// All five playable characters are selectable. The wide-leaf
/// featurization that gated this list historically only applies to the
/// NPZ training path; `--runmanager-toml-export` ignores every NPZ flag,
/// so a character needs nothing beyond a starter deck, a spec registry
/// and an engine `CharacterId` to be harvestable. `silent` / `defect`
/// were added for the combat position book (#446), where character
/// coverage is load-bearing — Silent carries ~3x Ironclad's rate of
/// transposition-merged root children (13.1% vs 4.6%, measured
/// 2026-08-12), so an Ironclad-only book is not representative.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum CliCharacter {
    Ironclad,
    Silent,
    Defect,
    Regent,
    Necrobinder,
}

impl CliCharacter {
    /// `--deck-source starter` deck for this character.
    fn starter_deck(self) -> Vec<CardSpec> {
        match self {
            CliCharacter::Ironclad => ironclad_starter_deck(),
            CliCharacter::Silent => silent_starter_deck(),
            CliCharacter::Defect => defect_starter_deck(),
            CliCharacter::Regent => regent_starter_deck(),
            CliCharacter::Necrobinder => necrobinder_starter_deck(),
        }
    }

    /// `--deck-source random-full-pool` sampling registry for this
    /// character.
    fn specs(self) -> &'static [fn() -> CardSpec] {
        match self {
            CliCharacter::Ironclad => IRONCLAD_SPECS,
            CliCharacter::Silent => SILENT_SPECS,
            CliCharacter::Defect => DEFECT_SPECS,
            CliCharacter::Regent => REGENT_SPECS,
            CliCharacter::Necrobinder => NECROBINDER_SPECS,
        }
    }

    /// Default `--sts2log-fixtures-dir` when the flag is unset — the
    /// `eh83` scrape landing point for this character.
    fn default_sts2log_fixtures_dir(self) -> &'static str {
        match self {
            CliCharacter::Ironclad => "/tmp/run-fixtures-full/the-ironclad",
            CliCharacter::Silent => "/tmp/run-fixtures-full/the-silent",
            CliCharacter::Defect => "/tmp/run-fixtures-full/the-defect",
            CliCharacter::Regent => "/tmp/run-fixtures-full/the-regent",
            CliCharacter::Necrobinder => "/tmp/run-fixtures-full/the-necrobinder",
        }
    }

    /// `[player].class` string fixtures are filtered on in
    /// `load_sts2log_perfect_pool`.
    fn fixture_class_name(self) -> &'static str {
        match self {
            CliCharacter::Ironclad => "Ironclad",
            CliCharacter::Silent => "Silent",
            CliCharacter::Defect => "Defect",
            CliCharacter::Regent => "Regent",
            CliCharacter::Necrobinder => "Necrobinder",
        }
    }

    /// `resolve_card_name` class hint. Only Ironclad has a
    /// `PlayerClassHint` variant today (`Regent`/`Necrobinder`
    /// aren't in that enum yet — see `names.rs`); `None` still resolves
    /// every full Regent card name via the merged `deck_card_spec_registry`,
    /// it just loses bare `"Strike"`/`"Defend"` disambiguation, which
    /// sts2log fixtures don't emit (they store the concrete card name).
    fn class_hint(self) -> Option<PlayerClassHint> {
        match self {
            CliCharacter::Ironclad => Some(PlayerClassHint::Ironclad),
            CliCharacter::Silent => Some(PlayerClassHint::Silent),
            CliCharacter::Defect => Some(PlayerClassHint::Defect),
            CliCharacter::Regent | CliCharacter::Necrobinder => None,
        }
    }

    /// Character starting max HP. Per the C# decomp
    /// (`MegaCrit.Sts2.Core.Models.Characters/{Ironclad,Regent,
    /// Necrobinder}.cs` `StartingHp`): Ironclad 80 (unchanged from
    /// pre-`--character` behavior), Regent 75, Necrobinder 66.
    fn max_hp(self) -> i32 {
        match self {
            CliCharacter::Ironclad => 80,
            CliCharacter::Silent => 70,
            CliCharacter::Defect => 75,
            CliCharacter::Regent => 75,
            CliCharacter::Necrobinder => 66,
        }
    }

    /// Engine-side character id, for the `--source run-manager` path
    /// (drives a real `RunManager` — starter relics like Regent's
    /// DivineRight fire through their actual hooks there, so no
    /// synthetic star seeding applies on that path).
    fn character_id(self) -> sts2_core::potions::CharacterId {
        match self {
            CliCharacter::Ironclad => sts2_core::potions::CharacterId::Ironclad,
            CliCharacter::Silent => sts2_core::potions::CharacterId::Silent,
            CliCharacter::Defect => sts2_core::potions::CharacterId::Defect,
            CliCharacter::Regent => sts2_core::potions::CharacterId::Regent,
            CliCharacter::Necrobinder => sts2_core::potions::CharacterId::Necrobinder,
        }
    }

    /// Starting `stars` resource for the synthetic combat bootstrap.
    /// The synthetic path models zero starting relics for every
    /// character (see `build_combat_state`), but Regent's starter
    /// relic DivineRight (Starter rarity;
    /// `MegaCrit.Sts2.Core.Models.Relics/DivineRight.cs`) fires
    /// `AfterRoomEntered` → `PlayerCmd.GainStars(3)` on *every*
    /// `CombatRoom` entry, so it's the one starting-relic effect
    /// that's actually combat-relevant at fight start. Seeding
    /// `stars = 3` reproduces that effect without wiring a full
    /// relic object. Ironclad has no starting stars.
    fn starting_stars(self) -> i8 {
        match self {
            CliCharacter::Ironclad
            | CliCharacter::Silent
            | CliCharacter::Defect
            | CliCharacter::Necrobinder => 0,
            CliCharacter::Regent => 3,
        }
    }

    /// Fight-start Osty summon amount for the synthetic combat
    /// bootstrap. Necrobinder's starter relic BoundPhylactery
    /// (`MegaCrit.Sts2.Core.Models.Relics/BoundPhylactery.cs`) summons
    /// Osty 1 at combat start; like Regent's DivineRight stars above,
    /// the synthetic path has no relic lifecycle to fire it through, so
    /// the net fight-start effect is reproduced directly. The relic's
    /// OTHER effect — +1 summon per turn after turn 1
    /// (`AfterEnergyResetLate`) — is a recurring in-combat hook the
    /// synthetic path deliberately forgoes (zero-relic discipline; the
    /// `--source run-manager` path drives the real relic and gets both
    /// effects faithfully).
    fn starting_osty_summon(self) -> i32 {
        match self {
            CliCharacter::Ironclad
            | CliCharacter::Silent
            | CliCharacter::Defect
            | CliCharacter::Regent => 0,
            CliCharacter::Necrobinder => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum DeckSource {
    /// `--character`'s starter deck (`ironclad_starter_deck()` /
    /// `regent_starter_deck()`) — the 10-card pre-curse pile.
    Starter,
    /// Pull from `--sts2log-fixtures-dir`, filtering to `--character`'s
    /// fixtures whose `[player].deck` resolves through
    /// `sts2_content::names::resolve_card_name`. Skips fixtures with
    /// any unknown card name (character-dependent unknown-name counts
    /// per eh83 scrape numbers, mostly content-gap).
    Sts2logPerfect,
    /// Sample `--random-full-pool-size` cards uniformly from
    /// `--character.specs()` (e.g. `IRONCLAD_SPECS`, 86 entries at pin
    /// 01c094d, or `REGENT_SPECS`). Per-combat deck is keyed on
    /// `combat_seed`, so worker count is irrelevant for determinism.
    RandomFullPool,
}

/// Per-combat deterministic seed. The combat index alone determines
/// the seed; the rayon thread layout has zero influence. Splitting
/// the base seed through the SplitMix-style golden-ratio multiplier
/// avoids the trivial collision `combat_seed(0) == base_seed`.
#[inline]
fn combat_seed_from_index(base_seed: u64, combat_idx: u64) -> u64 {
    base_seed.wrapping_add(combat_idx.wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

// ── Deck-source loaders (sts2-rs-2vwf) ──────────────────────────────────

#[derive(Deserialize)]
struct FixturePlayer {
    class: String,
    deck: Vec<String>,
}

#[derive(Deserialize)]
struct FixtureRoot {
    player: FixturePlayer,
}

/// Eagerly load every `character`-class fixture in `dir` whose deck
/// resolves cleanly through `resolve_card_name`. Skipped fixtures
/// are tallied and logged once. Order is filesystem-iteration order
/// — `Combat_idx % pool.len()` keys deterministically off that
/// order, so the pool is sorted by filename for reproducibility
/// across hosts.
fn load_sts2log_perfect_pool(
    dir: &Path,
    character: CliCharacter,
) -> Result<Vec<Vec<CardSpec>>, String> {
    let class_name = character.fixture_class_name();
    let class_hint = character.class_hint();

    let entries = std::fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("toml"))
        .collect();
    paths.sort();

    let mut pool: Vec<Vec<CardSpec>> = Vec::new();
    let mut skipped_class = 0usize;
    let mut skipped_card = 0usize;
    let mut skipped_parse = 0usize;
    for p in &paths {
        let Ok(text) = std::fs::read_to_string(p) else {
            skipped_parse += 1;
            continue;
        };
        let Ok(parsed): Result<FixtureRoot, _> = toml::from_str(&text) else {
            skipped_parse += 1;
            continue;
        };
        if !parsed.player.class.eq_ignore_ascii_case(class_name) {
            skipped_class += 1;
            continue;
        }
        let mut deck = Vec::with_capacity(parsed.player.deck.len());
        let mut any_unknown = false;
        for name in &parsed.player.deck {
            if let Ok(spec) = resolve_card_name(name, class_hint) {
                deck.push(spec);
            } else {
                any_unknown = true;
                break;
            }
        }
        if any_unknown {
            skipped_card += 1;
            continue;
        }
        if !deck.is_empty() {
            pool.push(deck);
        }
    }
    eprintln!(
        "[corpus-gen] sts2log-perfect: loaded {} decks from {} (skipped {} non-{class_name}, \
         {} unknown-card, {} parse-error; pool order = sorted filename)",
        pool.len(),
        dir.display(),
        skipped_class,
        skipped_card,
        skipped_parse,
    );
    if pool.is_empty() {
        return Err(format!(
            "no usable sts2log-perfect {class_name} decks under {}",
            dir.display()
        ));
    }
    Ok(pool)
}

// ── Full-scenario fixture loader (sts2-rs-ctdr) ─────────────────────────

/// Minimal Deserialize shim used by the relic-blocklist pre-filter.
/// We only need to read `[player].relics` to decide whether to even
/// attempt the full `ScenarioFile` parse — most blocklist failures
/// are cheap to catch before `scenario_to_combat_state` complains.
#[derive(Deserialize)]
struct FixtureRelicsOnly {
    #[serde(default)]
    player: Option<FixtureRelicPlayer>,
}

#[derive(Deserialize, Default)]
struct FixtureRelicPlayer {
    #[serde(default)]
    relics: Vec<String>,
}

/// Build the set of `RelicKind::python_name()` strings the runtime
/// supports. The compile-time `python_name` arm-table is exhaustive
/// (`every_variant_has_python_name` regression-tests it), so this set
/// always matches the engine's relic surface — no manual blocklist
/// maintenance needed. Each new `RelicKind` variant automatically
/// becomes eligible the next time corpus_gen is rebuilt.
#[allow(
    clippy::enum_glob_use,
    clippy::unnecessary_wraps,
    reason = "exhaustive 290-variant RelicKind enumeration reads cleaner with glob import"
)]
fn supported_relic_names() -> std::collections::BTreeSet<&'static str> {
    use RelicKind::*;
    // `enum_iterator` isn't a workspace dep; enumerate inline via the
    // same exhaustive match `python_name` uses. We can't iterate the
    // discriminants directly, so call `python_name()` on every variant
    // explicitly. If a future variant is added without a row here, the
    // blocklist will silently treat it as unsupported — but the next
    // `corpus_gen --source toml` run will print the unsupported-relic
    // tally and a maintainer can patch this list in one minute.
    //
    // For 2026-05-17, every `RelicKind` variant gets a row. Sourced
    // from the `python_name` match arms in
    // `crates/sts2-core/src/relics.rs:543`.
    // Exhaustive 290-variant list — sourced via
    //   grep -oE 'RelicKind::\w+ =>' crates/sts2-core/src/relics.rs \
    //   | sed 's/RelicKind:://;s/ =>//' | sort -u
    // The `python_name` arm-table this mirrors is itself exhaustive
    // (regressed by `every_variant_has_python_name`), so this set
    // equals "every relic the engine knows about." A future
    // `RelicKind::all() -> &'static [Self]` helper would let us
    // derive this without the inline list. Empirically (2026-05-17):
    // 40592/44651 = 90.9% of Ironclad sts2log fixtures pass this
    // filter.
    [
        Akabeko,
        AlchemicalCoffer,
        AmethystAubergine,
        Anchor,
        ArcaneScroll,
        ArchaicTooth,
        ArtOfWar,
        Astrolabe,
        BagOfMarbles,
        BagOfPreparation,
        BeatingRemnant,
        BeautifulBracelet,
        Bellows,
        BeltBuckle,
        BigHat,
        BigMushroom,
        BiiigHug,
        BingBong,
        BlackBlood,
        BlackStar,
        BlessedAntler,
        BloodSoakedRose,
        BloodVial,
        BoneFlute,
        BoneTea,
        Bookmark,
        BookOfFiveRings,
        BookRepairKnife,
        BoomingConch,
        BoundPhylactery,
        BowlerHat,
        Bread,
        BrilliantScarf,
        Brimstone,
        BronzeScales,
        BurningBlood,
        BurningSticks,
        Byrdpip,
        CallingBell,
        Candelabra,
        CaptainsWheel,
        Cauldron,
        CentennialPuzzle,
        Chandelier,
        CharonsAshes,
        ChemicalX,
        ChoicesParadox,
        ChosenCheese,
        Circlet,
        Claws,
        CloakClasp,
        CrackedCore,
        Crossbow,
        CursedPearl,
        DarkstonePeriapt,
        DataDisk,
        DaughterOfTheWind,
        DelicateFrond,
        DemonTongue,
        DeprecatedRelic,
        DiamondDiadem,
        DingyRug,
        DistinguishedCape,
        DivineDestiny,
        DivineRight,
        DollysMirror,
        DragonFruit,
        DreamCatcher,
        Driftwood,
        DustyTome,
        Ectoplasm,
        ElectricShrymp,
        EmberTea,
        EmotionChip,
        EmptyCage,
        EternalFeather,
        FakeAnchor,
        FakeBloodVial,
        FakeHappyFlower,
        FakeLeesWaffle,
        FakeMango,
        FakeMerchantsRug,
        FakeOrichalcum,
        FakeSneckoEye,
        FakeStrikeDummy,
        FakeVenerableTeaSet,
        FencingManual,
        FestivePopper,
        Fiddle,
        FishingRod,
        ForgottenSoul,
        FragrantMushroom,
        FresnelLens,
        FrozenEgg,
        FuneraryMask,
        FurCoat,
        GalacticDust,
        GamblingChip,
        GamePiece,
        GhostSeed,
        Girya,
        GlassEye,
        Glitter,
        GnarledHammer,
        GoldenCompass,
        GoldenPearl,
        GoldPlatedCables,
        Gorget,
        GremlinHorn,
        HandDrill,
        HappyFlower,
        HelicalDart,
        HistoryCourse,
        HornCleat,
        IceCream,
        InfusedCore,
        IntimidatingHelmet,
        IronClub,
        IvoryTile,
        JeweledMask,
        JewelryBox,
        JossPaper,
        JuzuBracelet,
        Kifuda,
        Kunai,
        Kusarigama,
        Lantern,
        LargeCapsule,
        LastingCandy,
        LavaLamp,
        LavaRock,
        LeadPaperweight,
        LeafyPoultice,
        LeesWaffle,
        LetterOpener,
        LizardTail,
        LoomingFruit,
        LordsParasol,
        LostCoffer,
        LostWisp,
        LuckyFysh,
        LunarPastry,
        Mango,
        MassiveScroll,
        MawBank,
        MealTicket,
        MeatCleaver,
        MeatOnTheBone,
        MembershipCard,
        MercuryHourglass,
        Metronome,
        MiniatureCannon,
        MiniatureTent,
        MiniRegent,
        MoltenEgg,
        MrStruggles,
        MummifiedHand,
        MusicBox,
        MysticLighter,
        NeowsTorment,
        NewLeaf,
        NinjaScroll,
        Nunchaku,
        NutritiousOyster,
        NutritiousSoup,
        OddlySmoothStone,
        OldCoin,
        OrangeDough,
        Orichalcum,
        OrnamentalFan,
        Orrery,
        PaelsBlood,
        PaelsClaw,
        PaelsEye,
        PaelsFlesh,
        PaelsGrowth,
        PaelsHorn,
        PaelsLegion,
        PaelsTears,
        PaelsTooth,
        PaelsWing,
        PandorasBox,
        Pantograph,
        PaperKrane,
        PaperPhrog,
        ParryingShield,
        Pear,
        Pendulum,
        PenNib,
        Permafrost,
        PetrifiedToad,
        PhilosophersStone,
        PhylacteryUnbound,
        Planisphere,
        Pocketwatch,
        PollinousCore,
        Pomander,
        PotionBelt,
        PowerCell,
        PrayerWheel,
        PrecariousShears,
        PreciseScissors,
        PreservedFog,
        PrismaticGem,
        PumpkinCandle,
        PunchDagger,
        RadiantPearl,
        RainbowRing,
        RazorTooth,
        RedMask,
        RedSkull,
        Regalite,
        RegalPillow,
        ReptileTrinket,
        RingingTriangle,
        RingOfTheDrake,
        RingOfTheSnake,
        RippleBasin,
        RoyalPoison,
        RoyalStamp,
        RuinedHelmet,
        RunicCapacitor,
        RunicPyramid,
        Sai,
        SandCastle,
        ScreamingFlagon,
        ScrollBoxes,
        SeaGlass,
        SealOfGold,
        SelfFormingClay,
        SereTalon,
        Shovel,
        Shuriken,
        SignetRing,
        SilverCrucible,
        SlingOfCourage,
        SmallCapsule,
        SneckoEye,
        SneckoSkull,
        Sozu,
        SparklingRouge,
        SpikedGauntlets,
        StoneCalendar,
        StoneCracker,
        StoneHumidifier,
        Storybook,
        Strawberry,
        StrikeDummy,
        SturdyClamp,
        SwordOfJade,
        SwordOfStone,
        SymbioticVirus,
        TanxsWhistle,
        TeaOfDiscourtesy,
        TheAbacus,
        TheBoot,
        TheCourier,
        ThrowingAxe,
        Tingsha,
        TinyMailbox,
        ToastyMittens,
        Toolbox,
        TouchOfOrobas,
        ToughBandages,
        ToxicEgg,
        ToyBox,
        TriBoomerang,
        TungstenRod,
        TuningFork,
        TwistedFunnel,
        UnceasingTop,
        UndyingSigil,
        UnsettlingLamp,
        Vajra,
        Vambrace,
        VelvetChoker,
        VenerableTeaSet,
        VeryHotCocoa,
        VexingPuzzlebox,
        VitruvianMinion,
        WarHammer,
        WarPaint,
        Whetstone,
        WhisperingEarring,
        WhiteBeastStatue,
        WhiteStar,
        WingCharm,
        WongoCustomerAppreciationBadge,
        WongosMysteryTicket,
        YummyCookie,
    ]
    .iter()
    .map(|k| k.python_name())
    .collect()
}

/// Pre-filter check: does `path`'s `[player].relics` list reference
/// only relics in `supported`? Returns `Ok(true)` when the fixture
/// passes; `Ok(false)` when at least one relic is unsupported (with
/// the first offending name in the second tuple slot for diagnostic
/// logging); `Err` on parse failure.
fn fixture_passes_relic_blocklist(
    path: &Path,
    supported: &std::collections::BTreeSet<&'static str>,
) -> Result<(bool, Option<String>), String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let parsed: FixtureRelicsOnly =
        toml::from_str(&text).map_err(|e| format!("toml {}: {e}", path.display()))?;
    let relics = parsed.player.unwrap_or_default().relics;
    for r in &relics {
        if !supported.contains(r.as_str()) {
            return Ok((false, Some(r.clone())));
        }
    }
    Ok((true, None))
}

/// Bead `sts2-rs-acbj` (step 1): resolve an encounter id string to
/// the 3-bucket tier code emitted into the corpus NPZ. Accepts
/// both the bare `"SLUDGE_SPINNER_WEAK"` form and the prefixed
/// `"ENCOUNTER.SLUDGE_SPINNER_WEAK"` form that the `.run-source`
/// fixtures use under `[provenance].encounter`. Searches ACT1,
/// ACT2, then ACT3 rosters; returns [`TIER_UNKNOWN`] if the id
/// isn't found in any. `EncounterTier::Weak` and `Normal` both
/// fold into [`TIER_COMMON`].
fn resolve_encounter_tier(raw: &str) -> u8 {
    let id = raw.strip_prefix("ENCOUNTER.").unwrap_or(raw);
    let found = sts2_content::encounters::act1_overgrowth::find(id)
        .or_else(|| sts2_content::encounters::act2_hive::find(id))
        .or_else(|| sts2_content::encounters::act3_glory::find(id));
    match found {
        Some(def) => match def.tier {
            EncounterTier::Weak | EncounterTier::Normal => TIER_COMMON,
            EncounterTier::Elite => TIER_ELITE,
            EncounterTier::Boss => TIER_BOSS,
            // EncounterTier is #[non_exhaustive]; any future variant
            // falls through to UNKNOWN until this match is updated.
            _ => TIER_UNKNOWN,
        },
        None => TIER_UNKNOWN,
    }
}

/// Bead `sts2-rs-t4tn` approach (A): post-load fixture filter on
/// `[provenance].floor`, `[player].ascension`, and
/// `[provenance].player_outcome`. Returns the filtered pool with
/// the same ordering as the input. Logs aggregate counts to
/// stderr so an out-of-band filter (e.g. `--floor-min 50` when the
/// pool has zero floor-50+ entries) surfaces immediately.
fn apply_t4tn_provenance_filters(
    pool: Vec<ScenarioFile>,
    floor_min: u32,
    ascension_min: u32,
    outcome: PlayerOutcomeFilter,
    require_deck_perfect: bool,
) -> Vec<ScenarioFile> {
    if floor_min == 0
        && ascension_min == 0
        && matches!(outcome, PlayerOutcomeFilter::Any)
        && !require_deck_perfect
    {
        return pool;
    }
    let total = pool.len();
    let mut dropped_floor = 0usize;
    let mut dropped_ascension = 0usize;
    let mut dropped_outcome = 0usize;
    let mut dropped_deck_imperfect = 0usize;
    let kept: Vec<ScenarioFile> = pool
        .into_iter()
        .filter(|s| {
            // Bead `sts2-rs-acbj` (step 3): deck_perfect filter.
            // ~7% of the .run-source pool is deck-imperfect (scraper
            // couldn't recover the exact deck from the event log).
            // Those fixtures produce systematically wrong starting
            // states — playing them with the correct decks is the
            // wrong experiment.
            if require_deck_perfect {
                let perfect = s
                    .provenance
                    .as_ref()
                    .and_then(|p| p.deck_perfect)
                    .unwrap_or(false);
                if !perfect {
                    dropped_deck_imperfect += 1;
                    return false;
                }
            }
            if floor_min > 0 {
                let f = s.provenance.as_ref().and_then(|p| p.floor).unwrap_or(0);
                if f < floor_min {
                    dropped_floor += 1;
                    return false;
                }
            }
            if ascension_min > 0 {
                let a = s.player.as_ref().and_then(|p| p.ascension).unwrap_or(0);
                if a < ascension_min {
                    dropped_ascension += 1;
                    return false;
                }
            }
            match outcome {
                PlayerOutcomeFilter::Any => {}
                PlayerOutcomeFilter::Won => {
                    let o = s
                        .provenance
                        .as_ref()
                        .and_then(|p| p.player_outcome.as_deref())
                        .unwrap_or("");
                    if o != "won" {
                        dropped_outcome += 1;
                        return false;
                    }
                }
                PlayerOutcomeFilter::Lost => {
                    let o = s
                        .provenance
                        .as_ref()
                        .and_then(|p| p.player_outcome.as_deref())
                        .unwrap_or("");
                    if o != "lost" {
                        dropped_outcome += 1;
                        return false;
                    }
                }
            }
            true
        })
        .collect();
    eprintln!(
        "[corpus-gen] t4tn: provenance filters kept {}/{} fixtures \
         (deck_perfect-only dropped {dropped_deck_imperfect}, \
         floor_min={floor_min} dropped {dropped_floor}, \
         ascension_min={ascension_min} dropped {dropped_ascension}, \
         outcome={outcome:?} dropped {dropped_outcome})",
        kept.len(),
        total,
    );
    kept
}

/// Eagerly load every fixture matching `pattern` whose
/// `[player].relics` list is fully supported. The returned pool is
/// sorted by path for cross-host reproducibility (per-combat indexing
/// is `combat_idx % pool.len()`).
fn load_toml_fixture_pool(pattern: &str) -> Result<Vec<ScenarioFile>, String> {
    let supported = supported_relic_names();
    let mut entries: Vec<PathBuf> = glob(pattern)
        .map_err(|e| format!("bad glob {pattern}: {e}"))?
        .filter_map(Result::ok)
        .collect();
    entries.sort();
    if entries.is_empty() {
        return Err(format!("no fixtures matched glob {pattern}"));
    }

    let mut pool: Vec<ScenarioFile> = Vec::new();
    let mut skipped_relic_blocklist = 0usize;
    let mut skipped_parse = 0usize;
    let mut blocklist_relic_counter: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();

    for p in &entries {
        match fixture_passes_relic_blocklist(p, &supported) {
            Ok((true, _)) => {}
            Ok((false, Some(bad))) => {
                skipped_relic_blocklist += 1;
                *blocklist_relic_counter.entry(bad).or_insert(0) += 1;
                continue;
            }
            Ok((false, None)) => {
                skipped_relic_blocklist += 1;
                continue;
            }
            Err(_) => {
                skipped_parse += 1;
                continue;
            }
        }
        match load_scenario_file(p) {
            Ok(s) => pool.push(s),
            Err(_) => skipped_parse += 1,
        }
    }

    eprintln!(
        "[corpus-gen] ctdr: loaded {} fixtures from {} (skipped {} relic-blocklist, \
         {} parse-error; survival = {:.1}%)",
        pool.len(),
        pattern,
        skipped_relic_blocklist,
        skipped_parse,
        100.0 * pool.len() as f64 / entries.len().max(1) as f64,
    );
    if !blocklist_relic_counter.is_empty() {
        let mut top: Vec<(&String, &usize)> = blocklist_relic_counter.iter().collect();
        top.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
        eprintln!("[corpus-gen] ctdr: top blocklist-blocking relics:");
        for (name, count) in top.iter().take(10) {
            eprintln!("  {count:>6}  {name}");
        }
    }

    if pool.is_empty() {
        return Err(format!(
            "no fixtures from {pattern} passed relic blocklist + scenario parse"
        ));
    }
    Ok(pool)
}

/// Sample `size` cards from `specs` (`--character`'s registry, e.g.
/// `IRONCLAD_SPECS` / `REGENT_SPECS`) keyed deterministically off `seed`
/// (typically the combat_seed). Allows repeats — STS2 decks routinely
/// run 5× Strike or 3× Defend.
fn random_full_pool_deck(
    seed: u64,
    size: usize,
    specs: &'static [fn() -> CardSpec],
) -> Vec<CardSpec> {
    let mut rng = sts2_core::dotnet_rng::Rng::new((seed ^ 0xDEC0_DEC0_DEC0_DEC0) as i32);
    (0..size)
        .map(|_| {
            let idx = rng.next_int_exclusive(0, specs.len() as i32) as usize;
            specs[idx]()
        })
        .collect()
}

fn make_collector(args: &Args, log_embedding_warning: bool) -> Collector {
    if args.wide {
        let mut wide =
            CorpusCollectorWide::new().with_policy_action_space(args.policy_action_space.into());
        if args.with_embeddings {
            wide = wide.with_embeddings();
        }
        Collector::Wide(wide)
    } else {
        if args.with_embeddings && log_embedding_warning {
            eprintln!("[corpus-gen] --with-embeddings has no effect without --wide; ignoring");
        }
        Collector::Narrow(CorpusCollector::new())
    }
}

fn main() -> ExitCode {
    sts2_content::cards::dispatch::ensure_installed();
    let mut args = Args::parse();
    if args.search_dialect == CliSearchDialect::Deployed {
        eprintln!(
            "[corpus-gen] --search-dialect deployed: teacher runs BeliefDraw + \
             the deployed dialect (budget-keyed c_puct, draw_widening_c 2.0, \
             EndTurn widening 5.787/0.436)"
        );
        // Depth keeps its own flag, so `deployed` can be half-applied. Warn
        // rather than override: silently rewriting a value the caller passed
        // explicitly is how a "matched" corpus ends up unmatched in a way
        // nothing downstream can detect.
        if args.max_rollout_depth != COMBAT_ROLLOUT_DEPTH {
            eprintln!(
                "[corpus-gen] WARNING: --search-dialect deployed with \
                 --max-rollout-depth {} — production runs {COMBAT_ROLLOUT_DEPTH} \
                 (sts2-runai::COMBAT_ROLLOUT_DEPTH). The teacher is only \
                 partially matched to the search this corpus is meant to train for.",
                args.max_rollout_depth,
            );
        }
    }
    if let Some(path) = &args.policy_prior_weights {
        match LearnedValueNet::from_weights_path(path) {
            Ok(net) => {
                eprintln!(
                    "[corpus-gen] ExIt prior loaded from {} — self-play uses PUCT + LearnedPolicyPrior",
                    path.display()
                );
                args.prior_net = Some(Arc::new(net));
                args.prior_stats = Some(Arc::new(LearnedPolicyPriorStats::default()));
            }
            Err(e) => {
                eprintln!(
                    "[corpus-gen] error: --policy-prior-weights {} failed to load: {e}",
                    path.display()
                );
                return ExitCode::FAILURE;
            }
        }
    }
    if let Some(p) = args.embed_prior_weights.clone() {
        assert!(
            args.wide && args.with_embeddings,
            "[corpus-gen] --embed-prior-weights requires --wide --with-embeddings \
             (next-iter trainer needs the per-slot ids, which are only emitted by \
             the wide embeddings collector)"
        );
        let net = match EmbeddingValueNet::from_weights_path(&p) {
            Ok(net) => net,
            Err(e) => {
                eprintln!(
                    "[corpus-gen] error: --embed-prior-weights {} failed to load: {e}",
                    p.display()
                );
                return ExitCode::FAILURE;
            }
        };
        eprintln!(
            "[corpus-gen] ExIt embed prior loaded from {} — self-play uses PUCT + {}",
            p.display(),
            if args.embed_prior_texel_leaf {
                "LearnedPolicyPrior (Texel leaf — policy-only)"
            } else {
                "LearnedPolicyPrior + LearnedEmbed leaf"
            }
        );
        args.embed_prior_net = Some(Arc::new(net));
    }
    if let Some(p) = args.leaf_weights_path.clone() {
        // Reject the ambiguous combo: a LearnedEmbed leaf (embed prior as
        // BOTH prior and leaf) plus a runtime wide leaf — two leaf sources.
        if args.embed_prior_net.is_some() && !args.embed_prior_texel_leaf {
            eprintln!(
                "[corpus-gen] error: --leaf-weights-path conflicts with the LearnedEmbed \
                 leaf recipe; pass --embed-prior-texel-leaf so the embed net is the PUCT \
                 prior only and the runtime wide leaf is the leaf"
            );
            return ExitCode::FAILURE;
        }
        match sts2_mcts::value_net::load_wide_leaf_weights(&p) {
            Ok(w) => {
                eprintln!(
                    "[corpus-gen] Phase-2 runtime wide leaf loaded from {} — self-play \
                     depth-cap uses TexelWideRuntime (champion leaf co-improvement)",
                    p.display()
                );
                args.leaf_weights = Some(Arc::new(w));
            }
            Err(e) => {
                eprintln!(
                    "[corpus-gen] error: --leaf-weights-path {} failed to load: {e}",
                    p.display()
                );
                return ExitCode::FAILURE;
            }
        }
    }

    // Silence per-combat panic spew when --deck-source pulls fixtures
    // that exercise content-gap cards (Rampage/Headbutt/etc. without
    // a target context). The catch_unwind in run_one_combat handles
    // the unwind; this just drops the noisy default-handler output.
    // Restored to the default handler if anyone else needs panics
    // surfaced is a future-bead concern.
    std::panic::set_hook(Box::new(|_| {}));

    // Phase 0b: on-manifold eval-set export (god-mode states → .toml
    // fixtures + a RealisticCorpus corpus.json the A/B harness loads
    // natively). Bypasses the whole NPZ self-play flow.
    if let Some(dir) = args.runmanager_toml_export.clone() {
        return export_onmanifold_eval(&args, &dir);
    }

    // Bead `sts2-rs-1h8xz` (kwuj4.A): the mid-rollout snapshot emit
    // path is wired through `sample_rollout_leaf_state` (see the
    // per-sample loop in `run_self_play`). The earlier
    // sts2-rs-2arxw scaffold's warn-loud message is dropped; N > 0
    // now produces real rows.
    if args.mid_rollout_samples > 0 {
        eprintln!(
            "[corpus-gen] --mid-rollout-samples={} active: emitting up to N \
             synthetic-rollout leaf-state rows per player-decision node, \
             labelled by the eventual terminal_win.",
            args.mid_rollout_samples,
        );
    }

    let max_combats = args.max_combats.unwrap_or(args.n_positions * 50 / 8);

    // Deck-source pool (bead 2vwf). For RandomFullPool, the pool stays
    // empty — each combat generates its own deck off combat_seed.
    // sts2-rs-ctdr: when --source toml is set, load the scenario
    // pool now and bypass the synthetic deck_pool entirely. The two
    // pools are mutually exclusive — toml mode supersedes --deck-source.
    let scenario_pool: Vec<ScenarioFile> = if matches!(args.source, Source::Toml) {
        let Some(pattern) = args.fixture_glob.as_deref() else {
            eprintln!("[corpus-gen] --source toml requires --fixture-glob <pattern>");
            return ExitCode::from(2);
        };
        let raw = match load_toml_fixture_pool(pattern) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[corpus-gen] {e}");
                return ExitCode::from(2);
            }
        };
        // Bead `sts2-rs-t4tn`: hard-fixture-seeding filters applied
        // on top of the relic-blocklist survivors. All three
        // provenance gates are no-ops at their default values.
        let pool = apply_t4tn_provenance_filters(
            raw,
            args.floor_min,
            args.ascension_min,
            args.player_outcome,
            args.require_deck_perfect,
        );
        if pool.is_empty() {
            eprintln!(
                "[corpus-gen] t4tn: provenance filters left zero fixtures \
                 (--floor-min={} --ascension-min={} --player-outcome={:?}). \
                 Relax the filters or check the source pool's coverage.",
                args.floor_min, args.ascension_min, args.player_outcome,
            );
            return ExitCode::from(2);
        }
        pool
    } else {
        Vec::new()
    };

    // sts2log-perfect fixture dir: explicit `--sts2log-fixtures-dir`
    // wins; otherwise resolved from `--character` so `regent` doesn't
    // silently pull Ironclad fixtures. Computed unconditionally (cheap)
    // so it's available whichever `--deck-source` is chosen; it's only
    // read when `--deck-source sts2log-perfect` is actually selected, so
    // `starter` / `random-full-pool` never touch the filesystem here.
    let sts2log_fixtures_dir: PathBuf = args
        .sts2log_fixtures_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(args.character.default_sts2log_fixtures_dir()));

    let deck_pool: Vec<Vec<CardSpec>> = if matches!(args.source, Source::Toml) {
        // Unused in toml mode — each combat realizes its deck from
        // the per-fixture ScenarioFile.
        Vec::new()
    } else {
        match args.deck_source {
            DeckSource::Starter => vec![args.character.starter_deck()],
            DeckSource::Sts2logPerfect => {
                match load_sts2log_perfect_pool(&sts2log_fixtures_dir, args.character) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("[corpus-gen] {e}");
                        return ExitCode::from(2);
                    }
                }
            }
            DeckSource::RandomFullPool => Vec::new(),
        }
    };

    // For `--deck-source starter` (vsqc.1 default) we filter to
    // non-boss encounters — the 10-card starter pile drags bosses
    // into endless 50+ turn combats that hit HARD_STEP_CAP without
    // producing useful gradient. For the 2vwf modes (sts2log-perfect
    // / random-full-pool) we include bosses so the outcome
    // distribution has a real loss tail; better decks can actually
    // finish those fights, which is the win/loss gradient the value
    // head needs to learn from.
    let allow_bosses = !matches!(args.deck_source, DeckSource::Starter);
    let encounter_pool: Vec<usize> = ACT1_ENCOUNTERS
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            if e.randomize.is_some() {
                return false;
            }
            allow_bosses || !matches!(e.tier, EncounterTier::Boss)
        })
        .map(|(i, _)| i)
        .collect();
    assert!(
        !encounter_pool.is_empty(),
        "encounter pool empty — sts2-content/encounters/act1 layout drifted"
    );

    // sts2-rs-3b0p: process combats in deterministic batches via
    // rayon. Each combat is keyed solely by combat_idx → fixed
    // combat_seed; the per-combat output is merged in combat_idx order,
    // so the resulting collector is byte-identical regardless of
    // worker count. Batches are sized to amortize rayon dispatch
    // overhead (each batch is a single par_iter) while still letting
    // the outer loop short-circuit once n_positions filtered rows are
    // collected.
    //
    // Cap batch_size so progress lines tick on a human-friendly
    // cadence (~5-15 s/tick at typical throughput). Pre-cap the
    // first batch was `n_positions` combats — a 500k run would emit
    // a single progress line at the very end after ~3 h wall, which
    // is what the `feedback_long_running_tasks_need_progress_logs`
    // auto-memory flagged. Per-batch dispatch overhead is sub-ms;
    // shrinking the batch from 500k → 1024 trades ~0.01 % overhead
    // for a 3-h-vs-15-s feedback gap. Output is still byte-identical
    // (combat_seed is keyed on `combat_idx`; merge happens in
    // combat_idx order regardless of batch boundary).
    let batch_size = args.n_positions.clamp(64, 1024);

    // Custom thread pool only when --workers is set explicitly;
    // otherwise rayon's global pool (sized by RAYON_NUM_THREADS or
    // core count) is used.
    let pool = if args.workers > 0 {
        Some(
            ThreadPoolBuilder::new()
                .num_threads(args.workers)
                .build()
                .expect("rayon thread pool build"),
        )
    } else {
        None
    };

    let mut driver = make_collector(&args, /*log_embedding_warning=*/ true);
    let mut combats_run: usize = 0;
    let mut combats_won: usize = 0;
    let mut combats_aborted: usize = 0;
    let mut combat_idx_next: u64 = 0;

    // Per-combat closure: build state, run self-play, return a fully
    // labeled local Collector + win flag. Runs inside rayon workers
    // (must be Send + deterministic in combat_idx).
    // Returns `None` if the combat panicked (content-gap card dispatch
    // typically — Rampage / Headbutt expect a target the deck-context
    // doesn't supply). Aborted combats are dropped, not counted toward
    // wins or losses.
    let run_one_combat = |combat_idx: u64| -> Option<(Collector, bool)> {
        let combat_seed = combat_seed_from_index(args.seed, combat_idx);

        // sts2-rs-ctdr: toml-source combats realize via the a2fh
        // library function; deck/encounter/relic/potion all come from
        // the fixture rather than the synthetic samplers.
        if matches!(args.source, Source::Toml) {
            let fixture_index = (combat_idx as usize) % scenario_pool.len();
            let scenario = &scenario_pool[fixture_index];
            return std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_single_combat_from_scenario(
                    scenario,
                    combat_seed,
                    combat_idx,
                    fixture_index as u64,
                    &args,
                )
            }))
            .ok();
        }

        // Phase 0a (AlphaZero on-manifold self-play bet): drive a real
        // RunManager to a combat entry and snapshot the on-manifold
        // CombatState, then MCTS-self-play it. `combat_idx` →
        // (run_ordinal, target_ordinal); the run is keyed solely on
        // run_ordinal so output is worker-count-independent. An empty
        // collector (target combat never reached — run died earlier)
        // is filtered to `None` so it counts as an abort, not a
        // zero-row merge.
        if matches!(args.source, Source::RunManager) {
            let per_run = args.runmanager_combats_per_run.max(1) as u64;
            let run_ordinal = combat_idx / per_run;
            let target_ordinal = (combat_idx % per_run) as usize;
            let run_seed = combat_seed_from_index(args.seed, run_ordinal);
            return std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_single_combat_from_runmanager(run_seed, target_ordinal, combat_idx, &args)
            }))
            .ok()
            .filter(|(local, _)| !local.is_empty());
        }

        let mut enc_rng =
            sts2_core::dotnet_rng::Rng::new((combat_seed ^ 0x5151_5151_5151_5151) as i32);
        let enc_idx =
            encounter_pool[enc_rng.next_int_exclusive(0, encounter_pool.len() as i32) as usize];
        let enc = &ACT1_ENCOUNTERS[enc_idx];

        let deck_specs: Vec<CardSpec> = match args.deck_source {
            DeckSource::RandomFullPool => random_full_pool_deck(
                combat_seed,
                args.random_full_pool_size,
                args.character.specs(),
            ),
            // Pool-indexed: `combat_idx % pool.len()` is deterministic in
            // (seed, idx) and worker-count-independent.
            _ => deck_pool[(combat_idx as usize) % deck_pool.len()].clone(),
        };

        // Wrap the per-combat body in catch_unwind so a content-gap
        // dispatch panic on one fixture doesn't poison the rayon pool.
        // Engine state is owned within the closure, so unwind safety
        // is local to this combat.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_single_combat(enc, combat_seed, combat_idx, enc_idx, &deck_specs, &args)
        }))
        .ok()
    };

    /// sts2-rs-ctdr per-combat body: realize the full starting state
    /// from a real-run scenario, then run MCTS self-play as the
    /// synthetic path does. Encounter / deck / relics / potions all
    /// come from the fixture; only the MCTS RNG seed comes from
    /// `combat_seed`. Win/loss is labeled from MCTS self-play, not
    /// the fixture's `player_outcome` (per the bead description).
    fn run_single_combat_from_scenario(
        scenario: &ScenarioFile,
        combat_seed: u64,
        combat_idx: u64,
        fixture_index: u64,
        args: &Args,
    ) -> (Collector, bool) {
        // a2fh's library fn: full state materialization (clap-free).
        // Realization can fail if the fixture references a card or
        // potion that isn't ported yet — this is the same fail mode
        // the relic blocklist catches earlier, just for non-relic
        // surfaces. Panic-unwind at the rayon layer drops the combat.
        let mut state = scenario_to_combat_state(scenario, combat_seed)
            .expect("scenario_to_combat_state: pre-filtered fixture should realize");
        let _ = start_combat(&mut state);
        let _ = start_player_turn(&mut state);

        let mut local = make_collector(args, /*log_embedding_warning=*/ false);
        // Bead sts2-rs-acbj (step 1): plumb per-combat provenance
        // (run_id + encounter tier) into the local collector so
        // every captured row carries the group key + tier. FNV-1a
        // of an empty/missing run_id is non-zero, so the "0 =
        // unset" sentinel only fires when provenance is absent
        // entirely. Tier defaults to UNKNOWN when the encounter
        // id doesn't resolve in ACT{1,2,3}_ENCOUNTERS — that path
        // currently fires on STS2-specific encounters not yet in
        // the static rosters and should be tracked separately.
        // Bead sts2-rs-e2bjk: also stamp the narrow collector so
        // `--group-split` has a real per-run group key on the
        // narrow Texel-tuning corpus path.
        {
            let prov = scenario.provenance.as_ref();
            let run_id_str = prov.and_then(|p| p.run_id.as_deref()).unwrap_or("");
            let run_id_u64 = if run_id_str.is_empty() {
                // No provenance `run_id` — which is the case for the whole
                // on-manifold pool (`--runmanager-toml-export` writes no
                // `[provenance]` block). Leaving the 0 sentinel gives every
                // row the SAME group key, so `--split-mode group` sees one
                // group, puts all of it on one side of the split, and hands
                // the trainer an empty validation set: every metric comes
                // back NaN and best-checkpointing silently keeps epoch 0.
                //
                // Fall back to the fixture's pool index. The pool is sorted
                // by path and indexed `combat_idx % pool.len()`, so this is
                // stable across hosts and groups rows by the *deck* they were
                // played from — which is the correlation the group split
                // exists to break. Shift-then-`| 1` keeps every index distinct
                // while never producing the 0 "unset" sentinel.
                (fixture_index << 1) | 1
            } else {
                fnv1a_64(run_id_str.as_bytes())
            };
            let tier = prov
                .and_then(|p| p.encounter.as_deref())
                .map_or(TIER_UNKNOWN, resolve_encounter_tier);
            match &mut local {
                Collector::Narrow(c) => c.set_pending_combat_provenance(run_id_u64, tier),
                Collector::Wide(c) => c.set_pending_combat_provenance(run_id_u64, tier),
            }
        }
        const HARD_TURN_CAP: u16 = 50;
        const HARD_STEP_CAP: u64 = 500;
        // Toml scenarios don't share the synthetic act1 encounter
        // index space; pass a sentinel that downstream code treats as
        // "unknown encounter". The Wide collector's monster-slot
        // capture pulls species from `state.creatures` directly, so
        // the sentinel is informational only.
        let enc_idx: usize = usize::MAX;
        let win = run_self_play(
            &mut state,
            &mut local,
            args,
            combat_seed,
            combat_idx,
            enc_idx,
            HARD_TURN_CAP,
            HARD_STEP_CAP,
        );
        let max_hp = state.player.max_hp.max(1) as f32;
        let final_hp_norm = (state.player.current_hp.max(0) as f32) / max_hp;
        local.mark_combat_outcome_with_hp(win, final_hp_norm);
        (local, win)
    }

    /// Phase 0a per-combat body: build a real Ironclad `RunManager`,
    /// drive it headlessly with the heuristic nav+combat policy to the
    /// `target_ordinal`-th combat entry, snapshot the on-manifold
    /// `CombatState` the run-loop constructed there, then MCTS-self-play
    /// that snapshot to terminal (recording every decision state) the
    /// same way the synthetic/toml paths do. Returns an empty collector
    /// when the run terminates before reaching `target_ordinal` (the
    /// caller filters that to an abort). Win/loss is labeled from the
    /// MCTS self-play of the snapshot, not from the run's own (heuristic)
    /// resolution of that combat.
    fn run_single_combat_from_runmanager(
        run_seed: u64,
        target_ordinal: usize,
        combat_idx: u64,
        args: &Args,
    ) -> (Collector, bool) {
        // Drive a god-mode run to the target combat entry (shared with the
        // Phase-0b eval export). Empty collector when the run dies first →
        // the caller filters that to an abort.
        let Some((rm, tier)) = drive_god_run_to_target(run_seed, target_ordinal, args) else {
            return (
                make_collector(args, /*log_embedding_warning=*/ false),
                false,
            );
        };
        let Some(mut state) = rm.combat else {
            return (
                make_collector(args, /*log_embedding_warning=*/ false),
                false,
            );
        };

        // Reset the snapshot from the unkillable progression HP back to a
        // realistic value so the *recorded* fight (and its win/loss label)
        // is faithful — only progression was cheated, not the corpus.
        if args.runmanager_godmode_hp > 0 {
            force_player_hp(
                &mut state,
                args.runmanager_snapshot_hp,
                args.runmanager_snapshot_hp,
            );
        }

        let mut local = make_collector(args, /*log_embedding_warning=*/ false);
        // Provenance: each on-manifold combat is its own "run" group for
        // the trainer's per-tier weighting; `combat_idx | 1` keeps the
        // group key strictly nonzero (0 is the "unset" sentinel). Tier
        // is the real room tier the run-loop selected.
        {
            let run_id_u64 = combat_idx | 1;
            match &mut local {
                Collector::Narrow(c) => c.set_pending_combat_provenance(run_id_u64, tier),
                Collector::Wide(c) => c.set_pending_combat_provenance(run_id_u64, tier),
            }
        }
        // enter_combat already ran start_combat + start_player_turn, so
        // the snapshot is play-ready; run_self_play drives from here.
        const HARD_TURN_CAP: u16 = 50;
        const HARD_STEP_CAP: u64 = 500;
        let win = run_self_play(
            &mut state,
            &mut local,
            args,
            run_seed,
            combat_idx,
            usize::MAX,
            HARD_TURN_CAP,
            HARD_STEP_CAP,
        );
        let max_hp = state.player.max_hp.max(1) as f32;
        let final_hp_norm = (state.player.current_hp.max(0) as f32) / max_hp;
        local.mark_combat_outcome_with_hp(win, final_hp_norm);
        (local, win)
    }

    // Body extracted so catch_unwind sees a clean callable.
    fn run_single_combat(
        enc: &'static sts2_content::encounters::EncounterDef,
        combat_seed: u64,
        combat_idx: u64,
        enc_idx: usize,
        deck_specs: &[CardSpec],
        args: &Args,
    ) -> (Collector, bool) {
        let mut state = build_combat_state(enc, combat_seed, deck_specs, args.character);
        let _ = start_combat(&mut state);
        // Regent's starter relic DivineRight (Starter rarity;
        // `MegaCrit.Sts2.Core.Models.Relics/DivineRight.cs`) grants +3
        // stars via `AfterRoomEntered` → `PlayerCmd.GainStars(3)` on
        // every combat-room entry — a run/map-level hook that, in the
        // real game, is not part of `CombatState` initialization and
        // so survives `start_combat`'s unconditional `player.stars = 0`
        // reset (mirrors Python's `CombatState.__init__`). The synthetic
        // bootstrap has no RunState/room-entry lifecycle to fire that
        // hook through, so reproduce the net effect directly here, AFTER
        // `start_combat` has already reset stars — seeding it inside
        // `build_combat_state` would be silently wiped by that reset.
        state.player.stars = args.character.starting_stars();
        // Necrobinder's BoundPhylactery fight-start Summon 1 — same
        // synthetic-bootstrap reasoning as the stars seeding above; see
        // `starting_osty_summon`'s doc for the per-turn trickle caveat.
        let osty_amount = args.character.starting_osty_summon();
        if osty_amount > 0 {
            let _ = state.summon_osty(osty_amount);
        }
        let _ = start_player_turn(&mut state);

        let mut local = make_collector(args, /*log_embedding_warning=*/ false);
        // Bead sts2-rs-acbj (step 1): synthetic-path provenance.
        // Each combat is its own "run" (no cross-combat deck
        // sharing the way .run-source fixtures have), so the
        // group key is just `combat_idx | 1` (the `| 1` keeps it
        // strictly nonzero, since 0 is the "unset" sentinel).
        // Tier comes directly off the EncounterDef.
        // Bead sts2-rs-e2bjk: also stamp the narrow collector.
        {
            let run_id_u64 = combat_idx | 1;
            let tier = match enc.tier {
                EncounterTier::Weak | EncounterTier::Normal => TIER_COMMON,
                EncounterTier::Elite => TIER_ELITE,
                EncounterTier::Boss => TIER_BOSS,
                _ => TIER_UNKNOWN,
            };
            match &mut local {
                Collector::Narrow(c) => c.set_pending_combat_provenance(run_id_u64, tier),
                Collector::Wide(c) => c.set_pending_combat_provenance(run_id_u64, tier),
            }
        }
        // Self-play until terminal or hard turn cap (defensive — a
        // misbehaved AI loop would otherwise hang the generator).
        // `HARD_STEP_CAP` is the per-combat step ceiling that fires
        // even when `state.round` stops advancing (e.g. a phase or
        // pending-choice loop that never re-enters `PlayerPlayPhase`
        // and never increments the round). Without it, the only bound
        // was `round <= HARD_TURN_CAP`, which a stuck combat can dodge
        // while every per-move `state.clone()` inside MCTS compounds
        // into multi-GB RSS (bead `sts2-rs-3kz2`).
        const HARD_TURN_CAP: u16 = 50;
        const HARD_STEP_CAP: u64 = 500;
        let win = run_self_play(
            &mut state,
            &mut local,
            args,
            combat_seed,
            combat_idx,
            enc_idx,
            HARD_TURN_CAP,
            HARD_STEP_CAP,
        );
        let max_hp = state.player.max_hp.max(1) as f32;
        let final_hp_norm = (state.player.current_hp.max(0) as f32) / max_hp;
        local.mark_combat_outcome_with_hp(win, final_hp_norm);
        (local, win)
    }

    while driver.len() < args.n_positions && combats_run < max_combats {
        let remaining_combats = max_combats - combats_run;
        let this_batch = batch_size.min(remaining_combats).max(1) as u64;
        let batch_start = combat_idx_next;
        let batch_end = combat_idx_next + this_batch;

        // par_iter().collect() preserves input order, so results[i] is
        // the (Collector, win) for combat_idx batch_start + i. Merging
        // in that iteration order keeps the driver collector
        // byte-identical to a sequential run.
        let body = || {
            (batch_start..batch_end)
                .into_par_iter()
                .map(&run_one_combat)
                .collect::<Vec<_>>()
        };
        let results: Vec<Option<(Collector, bool)>> = match &pool {
            Some(p) => p.install(body),
            None => body(),
        };
        for slot in results {
            match slot {
                Some((local, win)) => {
                    driver.merge(local);
                    combats_run += 1;
                    if win {
                        combats_won += 1;
                    }
                }
                None => combats_aborted += 1,
            }
        }
        combat_idx_next = batch_end;

        eprintln!(
            "[corpus-gen] combats={combats_run} captured={} won={} ({:.1}%) aborted={combats_aborted}",
            driver.len(),
            combats_won,
            100.0 * combats_won as f64 / combats_run.max(1) as f64,
        );

        // Bead `sts2-rs-t4tn`: hard-fail the run when the abort-rate
        // exceeds the configured ceiling. Surfaces content-gap drops
        // as build-blockers (per os46's structural diagnosis: the
        // dropped combats are exactly the hard fixtures that would
        // contribute the loss tail, so silent dropping skews the
        // survivor pool toward easier states). `1.0` (default)
        // disables the gate.
        if args.max_aborts_fraction < 1.0 {
            let total = combats_run + combats_aborted;
            if total > 0 {
                let frac = combats_aborted as f32 / total as f32;
                if frac > args.max_aborts_fraction {
                    eprintln!(
                        "[corpus-gen] t4tn: abort-rate {:.1}% exceeds \
                         --max-aborts-fraction={:.1}%; exiting. \
                         Investigate content gaps or raise the ceiling.",
                        100.0 * frac,
                        100.0 * args.max_aborts_fraction,
                    );
                    return ExitCode::from(3);
                }
            }
        }
    }

    let dropped = driver.filter_round_at_least(args.min_round);
    // Truncate to exactly n_positions for cross-worker-count
    // determinism: any worker count that produced ≥ n_positions
    // filtered rows yields the same output by selecting the first
    // n_positions in combat_idx order.
    if driver.len() > args.n_positions {
        driver.truncate(args.n_positions);
    }
    let collector = driver;

    if collector.is_empty() {
        eprintln!("[corpus-gen] no positions collected (max_combats reached?)");
        return ExitCode::from(2);
    }

    let final_n = collector.len();
    let columns = collector.columns();

    // GH #24 / bead `sts2-rs-9c7vr`: snapshot policy-projection
    // quality metrics before `into_arrays_*` consumes the wide
    // collector. Narrow collectors have no policy targets, so the
    // metric block is `None`.
    let policy_metrics_snapshot: Option<sts2_mcts::corpus::PolicyMetrics> = match &collector {
        Collector::Narrow(_) => None,
        Collector::Wide(c) => Some(c.policy_metrics().clone()),
    };

    // Resolve the policy dimension from the wide collector (if wide) or
    // fall back to canonical (narrow has no policy targets).
    let policy_dim: usize = match &collector {
        Collector::Wide(c) => c.policy_action_space().dim(),
        Collector::Narrow(_) => CANONICAL_ACTION_DIM,
    };

    let write_result = match collector {
        Collector::Narrow(c) => {
            // Bead sts2-rs-e2bjk: emit the 4-array narrow NPZ
            // (features + outcomes + run_id_u64 + encounter_tier_u8)
            // so `scripts/texel_tune.py --group-split` has a real
            // per-run group key to partition on. Strict superset of
            // the old 2-array NPZ — readers that ignore the extra
            // keys still see `features` + `outcomes`.
            let bundle = c.into_arrays_with_provenance();
            write_npz_narrow_with_provenance(
                &args.output,
                &bundle.features,
                final_n,
                columns,
                &bundle.outcomes,
                &bundle.run_id_u64,
                &bundle.encounter_tier_u8,
            )
        }
        Collector::Wide(c) => {
            if c.captures_embeddings() {
                // Wave-6 generalist-net path (bead sts2-rs-uqnh):
                // emit the four base arrays + six per-slot id arrays.
                // policy_dim is dynamic (canonical=640 or keyed=4672).
                let bundle = c.into_arrays_with_embeddings();
                write_npz_with_embeddings(
                    &args.output,
                    &bundle.features,
                    final_n,
                    columns,
                    &bundle.outcomes,
                    &bundle.final_hp_norm,
                    &bundle.visit_distribution,
                    policy_dim,
                    &bundle.card_ids_hand,
                    HAND_SLOT_MAX,
                    &bundle.card_ids_draw,
                    &bundle.card_ids_discard,
                    &bundle.card_ids_exhaust,
                    PILE_SLOT_MAX,
                    &bundle.monster_slot_ids,
                    ENEMY_SLOT_MAX,
                    &bundle.relic_set_ids,
                    RELIC_SLOT_MAX,
                    &bundle.card_feats_hand,
                    &bundle.card_feats_draw,
                    &bundle.card_feats_discard,
                    &bundle.card_feats_exhaust,
                    CARD_FEAT_DIM,
                    &bundle.mcts_root_q,
                    &bundle.mcts_q_valid,
                    &bundle.legal_action_mask,
                    &bundle.run_id_u64,
                    &bundle.encounter_tier_u8,
                )
            } else {
                // Bead sts2-rs-acbj (step 1): emit the
                // 4-array-plus-2-provenance NPZ so the trainer's
                // --group-col + per-tier loss-weight flags can
                // light up. Strict superset of the qctu.6.x
                // 4-array NPZ format; old trainers that don't read
                // run_id_u64 / encounter_tier_u8 still see the
                // same features+outcomes+final_hp_norm+
                // visit_distribution keys.
                // policy_dim is dynamic (canonical=640 or keyed=4672).
                let bundle = c.into_arrays_with_provenance();
                write_npz_with_extras_and_provenance(
                    &args.output,
                    &bundle.features,
                    final_n,
                    columns,
                    &bundle.outcomes,
                    &bundle.final_hp_norm,
                    &bundle.visit_distribution,
                    policy_dim,
                    &bundle.run_id_u64,
                    &bundle.encounter_tier_u8,
                    &bundle.legal_action_mask,
                    &bundle.raw_texel_score,
                    // Phase-0 combat-redesign (2026-06-09): emit the
                    // search-bootstrapped value target + mask on the
                    // non-embeddings `--wide` path so a single NPZ
                    // carries {ref value, visit dist, run_id group key}.
                    &bundle.mcts_root_q,
                    &bundle.mcts_q_valid,
                )
            }
        }
    };
    match write_result {
        Ok(bytes) => {
            let win_rate = combats_won as f64 / combats_run.max(1) as f64;
            eprintln!(
                "[corpus-gen] wrote {bytes} bytes to {} ({final_n} rows × {columns} features, \
                 {dropped} dropped by min_round={}, {combats_run} combats run, win-rate {:.1}%)",
                args.output.display(),
                args.min_round,
                100.0 * win_rate,
            );
            // ExIt (sts2-rs-1bcx): whole-run PUCT-prior health. Fires
            // whenever a prior was loaded (independent of collector
            // schema), so a degenerate net — one falling back to
            // uniform-on-legal on most moves — is caught at generation
            // time instead of after a multi-hour train+A/B cycle.
            if let Some(stats) = &args.prior_stats {
                eprintln!(
                    "[corpus-gen] puct prior: evaluations={} uniform_fallbacks={} \
                     fallback_rate={:.4} collision_rate={:.4}",
                    stats.evaluations(),
                    stats.uniform_fallbacks(),
                    stats.uniform_fallback_rate(),
                    stats.collision_rate(),
                );
            }
            // GH #24 / bead `sts2-rs-9c7vr`: policy-projection
            // validation report + MCTS-config metadata sidecar. The
            // sidecar lets downstream trainers detect drift between
            // training-time and inference-time search config (the
            // exact failure mode the issue's "trained from mismatched
            // search targets" subtitle calls out). Sidecar lives next
            // to the NPZ so byte-stable readers don't break.
            if let Some(pm) = &policy_metrics_snapshot {
                eprintln!(
                    "[corpus-gen] policy validation: count={} mean_legal_mass={:.4} \
                     mean_entropy={:.4} slot_collision_rate={:.4} key_collision_rate={:.4} \
                     uniform_fallback_rate={:.4}",
                    pm.count,
                    pm.mean_legal_mass(),
                    pm.mean_entropy(),
                    pm.slot_collision_rate(),
                    pm.key_collision_rate(),
                    pm.uniform_fallback_rate(),
                );
                let meta_path = {
                    let mut p = args.output.clone();
                    let mut name = p
                        .file_name()
                        .map(std::ffi::OsStr::to_os_string)
                        .unwrap_or_default();
                    name.push(".meta.json");
                    p.set_file_name(name);
                    p
                };
                let rollout_policy_str = match args.rollout_policy {
                    CliRolloutPolicy::Uniform => "uniform",
                    CliRolloutPolicy::Heuristic => "heuristic",
                };
                let policy_action_space_str = match args.policy_action_space {
                    CliPolicyActionSpace::Canonical => "canonical",
                    CliPolicyActionSpace::Keyed => "keyed",
                };
                // ExIt (sts2-rs-1bcx): record PUCT-prior provenance so an
                // ExIt-generated corpus is distinguishable from a
                // UCB+uniform one. JSON-escape the path via serde_json;
                // `null` when no prior was used.
                let policy_prior_weights_json = match &args.policy_prior_weights {
                    Some(path) => serde_json::to_string(&path.display().to_string())
                        .unwrap_or_else(|_| "null".to_string()),
                    None => "null".to_string(),
                };
                // Provenance for the teacher dialect: a policy head distilled
                // from a Clairvoyant teacher and one distilled from the deployed
                // honest search are different artifacts, and nothing else in the
                // NPZ distinguishes them.
                let search_dialect_str = match args.search_dialect {
                    CliSearchDialect::Legacy => "legacy",
                    CliSearchDialect::Deployed => "deployed",
                };
                let meta = format!(
                    "{{\n  \"schema_version\": 1,\n  \"mcts_config\": {{\n    \
                     \"sims_per_move\": {},\n    \"max_rollout_depth\": {},\n    \
                     \"search_dialect\": \"{}\",\n    \
                     \"rollout_policy\": \"{}\",\n    \"min_round\": {},\n    \
                     \"seed\": {},\n    \"use_puct_prior\": {},\n    \
                     \"policy_prior_weights\": {},\n    \
                     \"policy_action_space\": \"{}\",\n    \
                     \"policy_dim\": {}\n  }},\n  \"corpus\": {{\n    \"rows\": {},\n    \
                     \"feature_columns\": {},\n    \"action_dim_slot\": {},\n    \
                     \"action_dim_keyed\": {},\n    \"combats_run\": {},\n    \
                     \"combats_won\": {},\n    \"dropped_by_min_round\": {},\n    \
                     \"win_rate\": {:.6}\n  }},\n  \"policy_validation\": {{\n    \
                     \"count\": {},\n    \"mean_legal_mass\": {:.6},\n    \
                     \"mean_entropy\": {:.6},\n    \"slot_collision_rate\": {:.6},\n    \
                     \"key_collision_rate\": {:.6},\n    \"uniform_fallback_rate\": {:.6}\n  \
                     }}\n}}\n",
                    args.sims_per_move,
                    args.max_rollout_depth,
                    search_dialect_str,
                    rollout_policy_str,
                    args.min_round,
                    args.seed,
                    args.prior_net.is_some(),
                    policy_prior_weights_json,
                    policy_action_space_str,
                    policy_dim,
                    final_n,
                    columns,
                    CANONICAL_ACTION_DIM,
                    sts2_mcts::ACTION_KEY_DIM,
                    combats_run,
                    combats_won,
                    dropped,
                    win_rate,
                    pm.count,
                    pm.mean_legal_mass(),
                    pm.mean_entropy(),
                    pm.slot_collision_rate(),
                    pm.key_collision_rate(),
                    pm.uniform_fallback_rate(),
                );
                if let Err(e) = std::fs::write(&meta_path, meta) {
                    eprintln!(
                        "[corpus-gen] WARNING: failed to write metadata sidecar {}: {e}",
                        meta_path.display()
                    );
                } else {
                    eprintln!("[corpus-gen] metadata sidecar at {}", meta_path.display());
                }
            }
            // Bead 2vwf: outcome-balance warning. The 7njh acceptance
            // gate produced +0.00pp because the 99.7%-win training
            // corpus saturated the value head — flag here so any
            // downstream consumer can refuse to train against a
            // degenerate corpus instead of silently producing a
            // useless net.
            if !(0.30..=0.70).contains(&win_rate) {
                eprintln!(
                    "[corpus-gen] WARNING: degenerate outcome distribution \
                     (win_rate={:.1}%, outside [30%, 70%]). The value head will \
                     saturate near the dominant class and produce ~constant output. \
                     Pick --deck-source sts2log-perfect or random-full-pool, or \
                     bias --max-rollout-depth / --sims-per-move toward harder \
                     play. See sts2-rs-acbj for the full root-cause writeup.",
                    100.0 * win_rate,
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[corpus-gen] NPZ write failed: {e}");
            ExitCode::from(1)
        }
    }
}

/// Phase 2 ExpI: override a search's depth-cap leaf with the champion's
/// runtime wide leaf when `--leaf-weights-path` was loaded, so the captured
/// `mcts_root_q` value targets reflect the *improving* leaf rather than the
/// baked-in `TexelWide` const. No-op (returns the search unchanged) when the
/// flag is absent — byte-identical to the pre-Phase-2 path. Generic over the
/// policy so both `Mcts::new` and `Mcts::with_policy` branches reuse it. Safe
/// against the embed branch because `main` rejects a runtime leaf + a
/// `LearnedEmbed` leaf together.
fn with_runtime_leaf<P: sts2_mcts::Policy>(mcts: Mcts<P>, args: &Args) -> Mcts<P> {
    if let Some(w) = args.leaf_weights.as_ref() {
        mcts.with_leaf_eval(LeafEval::TexelWideRuntime(Arc::clone(w)))
    } else {
        mcts
    }
}

/// Run one MCTS search and snapshot the visit distribution + root Q
/// *before* the chosen action is applied. Generic over the policy prior
/// `P` so the UCB-uniform (`Mcts::new`) and PUCT-prior
/// (`Mcts::with_policy`) self-play branches in `run_self_play` reuse a
/// single capture path (sts2-rs-1bcx).
fn search_and_capture<P: sts2_mcts::Policy>(
    mut mcts: Mcts<P>,
) -> (Action, Vec<(Action, u32)>, Option<f32>) {
    let action = mcts.search();
    let visits = mcts
        .root_outcomes()
        .into_iter()
        .map(|o| (o.action, o.visits))
        .collect();
    let root_q = mcts.root_q();
    (action, visits, root_q)
}

/// Drive one combat to terminal under MCTS-policy play, recording
/// every player-decision state in the collector. Returns `true`
/// if the player won, `false` otherwise.
#[allow(clippy::too_many_arguments)]
fn run_self_play(
    state: &mut CombatState,
    collector: &mut Collector,
    args: &Args,
    combat_seed: u64,
    combat_idx: u64,
    enc_idx: usize,
    hard_turn_cap: u16,
    hard_step_cap: u64,
) -> bool {
    let mut step_index: u64 = 0;
    let mut last_action_kind: &'static str = "<none>";
    while !is_terminal(state) && state.round <= hard_turn_cap {
        // Bead `sts2-rs-3kz2`: bail before per-move `state.clone()`
        // compounds a stuck-combat loop into multi-GB RSS. Forensics
        // for the next iteration of the fix.
        if step_index >= hard_step_cap {
            eprintln!(
                "[corpus-gen] aborted combat_idx={combat_idx} enc_idx={enc_idx} \
                 combat_seed={combat_seed} phase={:?} round={} step={step_index} \
                 last_action={last_action_kind} cards={} creatures={} powers={} \
                 reason=hard_step_cap",
                state.phase,
                state.round,
                state.cards.len(),
                state.creatures.len(),
                state.powers.len(),
            );
            return false;
        }
        if matches!(state.phase, CombatPhase::PlayerPlayPhase) {
            // Snapshot the pre-decision state so the capture
            // reflects what the leaf-eval would have seen if MCTS
            // had depth-cut at this node — not the post-apply state.
            let pre_state = state.clone();

            // Bead `sts2-rs-3kz2`: TT is a 16 MiB `Box<[Entry; 1 << 20]>`
            // allocated per `Mcts::new()`. corpus_gen builds a fresh
            // tree per move (no reroot), so the TT can never be reused
            // across moves — each move pays the full 16 MiB alloc-and-
            // drop. Over thousands of combats × tens of moves, glibc
            // heap fragments and RSS climbs into tens of GB. The TT is
            // a search-quality lever, not a correctness lever; turning
            // it off bounds RSS at the cost of a few % search quality,
            // acceptable for a Texel/value-net training corpus.
            let mut cfg = MctsConfig {
                max_simulations: args.sims_per_move,
                max_rollout_depth: args.max_rollout_depth,
                seed: combat_seed.wrapping_add(step_index),
                use_transposition_table: false,
                rollout_policy: args.rollout_policy.into(),
                ..MctsConfig::default()
            };
            args.search_dialect.apply(&mut cfg);
            let cfg = cfg;
            // Wave-6 (qctu.6.x): capture the visit distribution from
            // the just-completed search BEFORE we apply the action.
            // The Wide collector consumes this as the policy-head
            // training target; the Narrow collector ignores it. The
            // visit-weighted root Q (bead `sts2-rs-wrip2`) rides along
            // as the TD-style value target.
            //
            // ExIt (sts2-rs-1bcx): with a loaded prior, drive search
            // with PUCT + LearnedPolicyPrior so the captured visits
            // reflect the improved searcher. Absent the prior this is
            // the original Mcts::new path, byte-for-byte (use_puct_prior
            // stays false; the prior touches selection only).
            let (action, visits, root_q) = if let Some(net) = args.prior_net.as_ref() {
                let mut cfg = cfg;
                cfg.use_puct_prior = true;
                // Invariant: prior_stats is set whenever prior_net is (main loader).
                let stats = args
                    .prior_stats
                    .as_ref()
                    .expect("prior_stats set alongside prior_net");
                let mcts = Mcts::with_policy(
                    pre_state.clone(),
                    cfg,
                    LearnedPolicyPrior::with_stats(Arc::clone(net), Arc::clone(stats)),
                );
                search_and_capture(with_runtime_leaf(mcts, args))
            } else if let Some(net) = args.embed_prior_net.as_ref() {
                // ExIt (sts2-rs-x9hyn): embedding-net analog of the 1bcx
                // wide path. The same `EmbeddingValueNet` drives BOTH the
                // PUCT prior (`LearnedPolicyPrior<EmbeddingValueNet>`) and
                // the leaf eval (`LeafEval::LearnedEmbed`).
                let mut cfg = cfg;
                cfg.use_puct_prior = true;
                let mcts = Mcts::with_policy(
                    pre_state.clone(),
                    cfg,
                    LearnedPolicyPrior::<EmbeddingValueNet>::new(Arc::clone(net)),
                );
                // Phase 3 (sts2-rs-x9hyn): policy-only recipe leaves the
                // leaf eval at the default Texel; otherwise route it
                // through the embedding value head (iteration-1 behavior).
                let mcts = if args.embed_prior_texel_leaf {
                    mcts
                } else {
                    mcts.with_leaf_eval(LeafEval::LearnedEmbed(Arc::clone(net)))
                };
                search_and_capture(with_runtime_leaf(mcts, args))
            } else {
                search_and_capture(with_runtime_leaf(Mcts::new(pre_state.clone(), cfg), args))
            };

            // Phase 1a (AlphaZero bet, docs/plans/2026-06-07-...): reference-
            // search value target. The action-selection search above runs at
            // --sims-per-move with the inference-time leaf; for the VALUE
            // label we optionally run a STRONGER reference search (higher
            // sims / deeper rollout) on the same pre_state and record ITS
            // root Q instead. This breaks the Texel-Q circularity — the net
            // learns to predict "leaf + a stronger search's worth of
            // lookahead" rather than the leaf it will itself replace at
            // inference. The visit distribution (policy target) still comes
            // from the action-selection search. The reference uses the SAME
            // prior/leaf wiring as the action search, so it is a stronger
            // version of the same evaluator, not a different one.
            let label_q = if args.reference_sims > 0 {
                let ref_cfg = MctsConfig {
                    max_simulations: args.reference_sims,
                    max_rollout_depth: args.reference_depth,
                    // Distinct seed so the reference search is not a
                    // duplicate of the action search; deterministic in
                    // (combat_seed, step_index).
                    seed: combat_seed
                        .wrapping_add(step_index)
                        .wrapping_mul(0xD1B5_4A32_D192_ED03),
                    use_transposition_table: false,
                    rollout_policy: args.rollout_policy.into(),
                    ..MctsConfig::default()
                };
                let mut ref_cfg = ref_cfg;
                args.search_dialect.apply(&mut ref_cfg);
                let ref_cfg = ref_cfg;
                let (_, _, ref_q) = if let Some(net) = args.embed_prior_net.as_ref() {
                    let mut ref_cfg = ref_cfg;
                    ref_cfg.use_puct_prior = true;
                    let mcts = Mcts::with_policy(
                        pre_state.clone(),
                        ref_cfg,
                        LearnedPolicyPrior::<EmbeddingValueNet>::new(Arc::clone(net)),
                    );
                    let mcts = if args.embed_prior_texel_leaf {
                        mcts
                    } else {
                        mcts.with_leaf_eval(LeafEval::LearnedEmbed(Arc::clone(net)))
                    };
                    search_and_capture(with_runtime_leaf(mcts, args))
                } else if let Some(net) = args.prior_net.as_ref() {
                    let mut ref_cfg = ref_cfg;
                    ref_cfg.use_puct_prior = true;
                    let stats = args
                        .prior_stats
                        .as_ref()
                        .expect("prior_stats set alongside prior_net");
                    search_and_capture(with_runtime_leaf(
                        Mcts::with_policy(
                            pre_state.clone(),
                            ref_cfg,
                            LearnedPolicyPrior::with_stats(Arc::clone(net), Arc::clone(stats)),
                        ),
                        args,
                    ))
                } else {
                    search_and_capture(with_runtime_leaf(
                        Mcts::new(pre_state.clone(), ref_cfg),
                        args,
                    ))
                };
                // Fall back to the action-search Q if the reference search
                // returned no root value (e.g. an immediately-terminal node).
                ref_q.or(root_q)
            } else {
                root_q
            };
            collector.record_position_with_visits(&pre_state, &visits, label_q);

            // Bead `sts2-rs-1h8xz` (kwuj4.A): mid-rollout state
            // sampling. For each player-decision node, run
            // `args.mid_rollout_samples` synthetic rollouts from
            // `pre_state` and record the leaf-eval state each lands
            // in. Each call uses a deterministic per-sample seed so
            // re-runs of the same (combat_seed, step_index) emit the
            // same rows. The recorded rows carry `visits=[]` and
            // `root_q=NaN` so the trainer can distinguish them from
            // start-of-turn snapshots; outcome-labelling falls out of
            // `mark_combat_outcome_with_hp` at combat end (every row
            // emitted in this combat shares the same terminal_win).
            if args.mid_rollout_samples > 0 {
                let policy_kind = args.rollout_policy.into();
                for sample_idx in 0..args.mid_rollout_samples {
                    // Per-sample RNG: combine combat_seed + step + sample_idx so
                    // the (combat, decision-node, sample-slot) triple maps to a
                    // unique reproducible seed.
                    let sample_seed = combat_seed
                        .wrapping_add(step_index)
                        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                        .wrapping_add(u64::from(sample_idx));
                    // Rollout policy RNG: search-side determinization
                    // (DET, not parity-critical per bead `sts2-rs-ykmk9`
                    // Stage 2 audit); keep Xoshiro for the rollout policy.
                    use rand_xoshiro::rand_core::SeedableRng;
                    let mut rng = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(sample_seed);
                    let mut scratch = pre_state.clone();
                    sample_rollout_leaf_state(
                        &mut scratch,
                        &mut rng,
                        args.max_rollout_depth,
                        policy_kind,
                        |leaf_state, _depth| {
                            collector.record_position_with_visits(leaf_state, &[], None);
                        },
                    );
                }
            }

            last_action_kind = match &action {
                Action::PlayCard { .. } => "PlayCard",
                Action::EndTurn => "EndTurn(mcts)",
                Action::UsePotion { .. } | Action::DrinkPotion { .. } => "Potion",
                Action::Channel { .. } => "Channel",
                Action::Evoke { .. } => "Evoke",
                Action::ResolveChoice { .. } => "ResolveChoice",
            };
            let result = apply(state, action);
            // If apply resolved combat (Win/Loss), break.
            if state.is_over {
                break;
            }
            let _ = result;
        } else {
            // Out of PlayerPlayPhase — let the engine drive the
            // enemy / end-turn pipeline by applying Action::EndTurn
            // (the engine's canonical "advance state machine"
            // verb when no player decision is pending).
            last_action_kind = "EndTurn(advance)";
            let _ = apply(state, Action::EndTurn);
        }
        step_index += 1;
    }
    state.is_over && state.player_won
}

/// Heuristic navigation + combat policy for driving a `RunManager`
/// headlessly in the Phase-0a on-manifold sampler. Mirrors the
/// `full_run_act1to3_ironclad` acceptance-test driver: in combat, play
/// Powers first (only if ≥1 energy would remain), then Attacks (HP > 50%)
/// or Skills, falling through to any affordable card else EndTurn;
/// out of combat, take the first available action (first map node, first
/// reward option, …). This is a *progression* policy only — it advances
/// the run cheaply to reach on-manifold combat-entry states; the entry
/// snapshot itself is MCTS-self-played for the corpus. Its strength
/// bounds how deep (which floors/tiers) the sampler can reach.
fn runmanager_pick_action(
    rm: &sts2_core::run_manager::RunManager,
) -> sts2_core::run_manager::RunAction {
    use sts2_core::cards::CardType;
    use sts2_core::run_manager::{Phase, RunAction};

    let actions = rm.get_available_actions();
    if rm.phase() != Phase::Combat {
        return actions.into_iter().next().unwrap_or(RunAction::Unhandled);
    }

    // Resolve any open combat choice first (a pending Headbutt PutBack /
    // Armaments / potion / deferred turn-setup suppresses PlayCard +
    // EndTurn in get_available_actions; the loop must clear it). Prefer
    // the skip option, else the first choice option.
    if let Some(rc) = actions
        .iter()
        .copied()
        .find(|a| matches!(a, RunAction::CombatResolveChoice { option: None }))
        .or_else(|| {
            actions
                .iter()
                .copied()
                .find(|a| matches!(a, RunAction::CombatResolveChoice { .. }))
        })
    {
        return rc;
    }

    let combat = rm.combat.as_ref();
    let prefer_attack = combat.map_or(true, |c| {
        let hp = c.player.current_hp;
        let max = c.player.max_hp.max(1);
        hp * 2 > max
    });
    let pick_by_type = |want: CardType| -> Option<usize> {
        let c = combat?;
        c.player.hand.iter().enumerate().find_map(|(i, uid)| {
            let card = c.cards.get(uid.0.get() as usize - 1)?;
            let energy = i32::from(c.player.energy);
            let cost = sts2_core::hooks::effective_card_cost(c, *uid);
            if cost >= 0 && cost <= energy && card.card_type == want {
                Some(i)
            } else {
                None
            }
        })
    };
    // Power-first only if ≥1 energy would remain (avoids burning the
    // whole turn on a Power with no block down).
    let energy = i32::from(combat.map_or(0, |c| c.player.energy));
    let power_idx = if energy >= 2 {
        pick_by_type(CardType::Power)
    } else {
        None
    };
    let idx = power_idx.or_else(|| {
        if prefer_attack {
            pick_by_type(CardType::Attack).or_else(|| pick_by_type(CardType::Skill))
        } else {
            pick_by_type(CardType::Skill).or_else(|| pick_by_type(CardType::Attack))
        }
    });
    if let Some(i) = idx {
        RunAction::PlayCard {
            hand_idx: i,
            target: None,
        }
    } else {
        actions
            .iter()
            .copied()
            .find(|a| matches!(a, RunAction::PlayCard { .. }))
            .unwrap_or(RunAction::EndTurn)
    }
}

/// Translate a combat-side [`Action`] to the [`RunAction`] the
/// `RunManager` dispatches, mirroring the `Action → RunAction` mapping in
/// `RunManager::get_available_actions` (run_manager.rs Phase::Combat arm).
/// Returns `None` for actions with no run-loop equivalent (UsePotion
/// legacy / Channel / Evoke — not emitted by `legal_actions`, so this
/// path is unreachable for Ironclad runs); callers fall back to the
/// heuristic picker.
fn action_to_runaction(a: &Action) -> Option<sts2_core::run_manager::RunAction> {
    use sts2_core::run_manager::RunAction;
    match a {
        Action::PlayCard { hand_index, target } => Some(RunAction::PlayCard {
            hand_idx: usize::from(*hand_index),
            target: target.map(u32::from),
        }),
        Action::EndTurn => Some(RunAction::EndTurn),
        Action::ResolveChoice { selection } => match selection.as_slice() {
            [] => Some(RunAction::CombatResolveChoice { option: None }),
            [i] => Some(RunAction::CombatResolveChoice {
                option: Some(usize::from(*i)),
            }),
            // Multi-pick (surfaced discard-N): first-`k` prefix → length.
            #[allow(clippy::cast_possible_truncation)]
            sel => Some(RunAction::CombatResolveChoiceTakeFirst {
                count: sel.len() as u8,
            }),
        },
        Action::DrinkPotion {
            potion_slot,
            target,
        } => Some(RunAction::DrinkPotion {
            potion_slot: usize::from(*potion_slot),
            target: target.map(u32::from),
        }),
        _ => None,
    }
}

/// Run-progression action for the Phase-0a sampler. Out of combat, or
/// when `--runmanager-mcts-progress` is off, defers to the cheap
/// heuristic [`runmanager_pick_action`]. In combat with MCTS progression
/// enabled, runs a (low-sim) MCTS search on the live combat — using the
/// loaded embedding prior as a PUCT prior when present, else plain UCB —
/// and translates its chosen [`Action`] back to a [`RunAction`]. This is
/// the "drive the run with the current best agent" policy the bet's
/// Phase 0a calls for: the heuristic dies in Act-1 (≈95 % common / 0 %
/// boss, Gate-0 FAIL), so MCTS progression is what lets runs survive deep
/// enough to sample elite/boss-tier entry states. `seed` must be
/// deterministic in (run_seed, step) so a run prefix replays identically
/// across `target_ordinal`s.
fn runmanager_progress_action(
    rm: &sts2_core::run_manager::RunManager,
    args: &Args,
    seed: u64,
) -> sts2_core::run_manager::RunAction {
    use sts2_core::run_manager::Phase;
    if rm.phase() != Phase::Combat || !args.runmanager_mcts_progress {
        return runmanager_pick_action(rm);
    }
    let Some(combat) = rm.combat.as_ref() else {
        return runmanager_pick_action(rm);
    };
    // Open choices: legal_actions returns only ResolveChoice variants;
    // let the heuristic clear them (skip / first option) rather than
    // spinning up a search over choice resolutions.
    if combat.pending_choice.is_some() {
        return runmanager_pick_action(rm);
    }
    let mut cfg = MctsConfig {
        max_simulations: args.runmanager_progress_sims,
        max_rollout_depth: args.max_rollout_depth,
        seed,
        use_transposition_table: false,
        rollout_policy: args.rollout_policy.into(),
        ..MctsConfig::default()
    };
    args.search_dialect.apply(&mut cfg);
    let cfg = cfg;
    let action = if let Some(net) = args.embed_prior_net.as_ref() {
        let mut cfg = cfg;
        cfg.use_puct_prior = true;
        Mcts::with_policy(
            combat.clone(),
            cfg,
            LearnedPolicyPrior::<EmbeddingValueNet>::new(Arc::clone(net)),
        )
        .search()
    } else {
        Mcts::new(combat.clone(), cfg).search()
    };
    action_to_runaction(&action).unwrap_or_else(|| runmanager_pick_action(rm))
}

/// Drive a god-mode `--source run-manager` run headlessly to the
/// `target_ordinal`-th combat entry and return the positioned
/// `RunManager` (with `rm.combat` = the on-manifold entry state) + the
/// room tier. Shared by the NPZ self-play path
/// (`run_single_combat_from_runmanager`) and the Phase-0b eval export
/// (`export_onmanifold_eval`). Returns `None` if the run terminates before
/// reaching `target_ordinal`. God-mode HP (set at run start + re-topped
/// after each combat) keeps the run alive to deep floors; the caller is
/// responsible for resetting the recorded/exported HP to a realistic value.
fn drive_god_run_to_target(
    run_seed: u64,
    target_ordinal: usize,
    args: &Args,
) -> Option<(sts2_core::run_manager::RunManager, u8)> {
    use sts2_content::run_callbacks::new_run_manager_for_character_with_ascension;
    use sts2_core::run_manager::{ActionResult, Phase};
    use sts2_core::runstate::RoomType;

    let mut rm = new_run_manager_for_character_with_ascension(
        run_seed as i32,
        args.character.character_id(),
        args.runmanager_ascension,
    );
    let godmode = args.runmanager_godmode_hp > 0;
    if godmode {
        rm.run_state.max_hp = args.runmanager_godmode_hp;
        rm.run_state.current_hp = args.runmanager_godmode_hp;
    }
    let mut combats_seen = 0usize;
    let mut in_combat = false;
    let mut step_budget = 200_000_i32;
    let mut progress_step: u64 = 0;

    loop {
        let phase = rm.phase();
        if phase == Phase::RunOver {
            return None;
        }
        if phase == Phase::Combat {
            if !in_combat {
                in_combat = true;
                if combats_seen == target_ordinal {
                    let tier = rm
                        .run_state
                        .current_room
                        .as_ref()
                        .map_or(TIER_UNKNOWN, |room| match room.room_type {
                            RoomType::Elite => TIER_ELITE,
                            RoomType::Boss => TIER_BOSS,
                            RoomType::Monster => TIER_COMMON,
                            _ => TIER_UNKNOWN,
                        });
                    return rm.combat.is_some().then_some((rm, tier));
                }
            }
        } else if in_combat {
            combats_seen += 1;
            in_combat = false;
            if godmode {
                rm.run_state.max_hp = args.runmanager_godmode_hp;
                rm.run_state.current_hp = args.runmanager_godmode_hp;
            }
        }
        let action = runmanager_progress_action(&rm, args, run_seed.wrapping_add(progress_step));
        progress_step = progress_step.wrapping_add(1);
        match rm.take_action(action) {
            ActionResult::Advanced => {}
            _ => return None,
        }
        step_budget -= 1;
        if step_budget <= 0 {
            return None;
        }
    }
}

/// Human-readable tier token for the `RealisticCorpus` entry + fixture
/// filename (mirrors the `parse_realistic_tier` strings the A/B harness
/// expects: "common" / "elite" / "boss").
fn tier_name(tier: u8) -> &'static str {
    match tier {
        TIER_ELITE => "elite",
        TIER_BOSS => "boss",
        _ => "common",
    }
}

/// Serialize a positioned god-mode run's combat ENTRY into a `schema = 1`
/// `ScenarioFile` TOML string. Deck card ids → `to_python_name()` (+ `"+"`
/// for upgraded), monster roster → `python_name()` + the spawned HP. HP is
/// the realistic `snapshot_hp`, NOT the god-mode progression HP.
///
/// Relics carry the god-run's real accumulated `rs.relics` (e.g. Regent's
/// DivineRight), filtered to ids with a runtime `RelicKind` so the loader's
/// `seed_relics_with_counters` never hard-errors on an unported relic. The
/// downstream loaders (`scenario_to_combat_state` + the `mcts_ab_value_net`
/// bench) already reconstruct `state.relics` and fire `start_combat` /
/// `start_player_turn`, which drive the relic-effects channels — so carrying
/// the list here is sufficient for combat-entry relic effects (DivineRight's
/// `after_side_turn_start` +3 stars) to fire in the realized eval state.
/// `potions` is the belt written to `[player].potions`: `&[]` unless
/// `--runmanager-export-potions`, which passes the god-run's real
/// `run_state.potions` (the progression heuristic takes the first reward
/// option and never drinks, so the belt fills up and stays full). `class`
/// uses the real character — a Regent fixture must not mislabel as
/// Ironclad, or the loader's class fallback silently drops the Regent
/// identity.
///
/// `ascension` is written as `[player].ascension` (the `ScenarioFile`
/// schema's existing optional field, `toml_file.rs`) ONLY when nonzero —
/// the pre-ascension-flag export never emitted this key, so omitting it
/// at ascension 0 keeps `--runmanager-ascension` unset byte-identical to
/// the old output instead of adding a new `ascension = 0` line everywhere.
fn god_entry_to_toml(
    rm: &sts2_core::run_manager::RunManager,
    snapshot_hp: i32,
    character: CliCharacter,
    ascension: u8,
    potions: &[sts2_core::potions::PotionKind],
) -> String {
    // Delegates to the shared writer (bead `sts2-rs-cws0i`, extracted so
    // `run_arena`'s champion-run fixture harvest can reuse the same
    // serialization). With `&[]` this reproduces the pre-cws0i byte shape
    // (`potions = []`) exactly, which the frozen suites' `state_hash`
    // values depend on.
    sts2_cli::sim::scenario::export::render_run_entry_toml(
        rm,
        snapshot_hp,
        character.fixture_class_name(),
        ascension,
        potions,
    )
}

/// Phase 0b: export a held-out on-manifold eval set. Drives god-mode
/// `--source run-manager` runs, writes one `.toml` fixture per sampled
/// combat entry + a `corpus.json` (`RealisticCorpus` shape) the A/B
/// harness loads via `--corpus-path <dir>/corpus.json`.
fn export_onmanifold_eval(args: &Args, out_dir: &Path) -> ExitCode {
    use std::fmt::Write as _;
    use std::panic::AssertUnwindSafe;

    if let Err(e) = std::fs::create_dir_all(out_dir) {
        eprintln!(
            "[corpus-gen] eval-export: cannot create {} ({e})",
            out_dir.display()
        );
        return ExitCode::FAILURE;
    }
    let per_run = args.runmanager_combats_per_run.max(1) as u64;
    let target = args.runmanager_eval_count;
    // RealisticCorpus.entries JSON, assembled by hand (the harness reads
    // {state_id, tier, fixture_path, floor, seed}).
    let mut entries_json: Vec<String> = Vec::with_capacity(target);
    let mut tier_counts = [0usize; 3]; // common, elite, boss
    let mut written = 0usize;
    let mut combat_idx: u64 = 0;
    let max_tries = (target as u64) * 64 + 2048;
    let mut tries: u64 = 0;

    while written < target && tries < max_tries {
        tries += 1;
        let run_ordinal = combat_idx / per_run;
        let target_ordinal = (combat_idx % per_run) as usize;
        let run_seed = combat_seed_from_index(args.seed, run_ordinal);
        combat_idx += 1;

        let driven = std::panic::catch_unwind(AssertUnwindSafe(|| {
            drive_god_run_to_target(run_seed, target_ordinal, args)
        }))
        .ok()
        .flatten();
        let Some((rm, tier)) = driven else { continue };

        let belt: &[sts2_core::potions::PotionKind] = if args.runmanager_export_potions {
            &rm.run_state.potions
        } else {
            &[]
        };
        let toml = god_entry_to_toml(
            &rm,
            args.runmanager_snapshot_hp,
            args.character,
            args.runmanager_ascension,
            belt,
        );
        let tname = tier_name(tier);
        let fname = format!("onmanifold_{written:05}_{tname}.toml");
        let fpath = out_dir.join(&fname);
        if let Err(e) = std::fs::write(&fpath, &toml) {
            eprintln!(
                "[corpus-gen] eval-export: write {} failed ({e})",
                fpath.display()
            );
            return ExitCode::FAILURE;
        }
        let mut entry = String::new();
        let _ = write!(
            entry,
            "{{\"state_id\": {written}, \"tier\": \"{tname}\", \"fixture_path\": {:?}, \
             \"floor\": {}, \"seed\": {run_seed}}}",
            fpath.display().to_string(),
            target_ordinal + 1,
        );
        entries_json.push(entry);
        match tier {
            TIER_ELITE => tier_counts[1] += 1,
            TIER_BOSS => tier_counts[2] += 1,
            _ => tier_counts[0] += 1,
        }
        written += 1;
        if written % 25 == 0 || written == target {
            eprintln!(
                "[corpus-gen] eval-export: {written}/{target} fixtures \
                 (common={} elite={} boss={}, runs driven={tries})",
                tier_counts[0], tier_counts[1], tier_counts[2],
            );
        }
    }

    if written == 0 {
        eprintln!("[corpus-gen] eval-export: 0 fixtures produced (god-mode reachable combats?)");
        return ExitCode::from(2);
    }

    let corpus = format!(
        "{{\n  \"version\": 2,\n  \"git_sha\": \"onmanifold-godmode\",\n  \
         \"generator\": \"corpus_gen --runmanager-toml-export (Phase 0b)\",\n  \
         \"bucket_globs\": {{}},\n  \"entries\": [\n    {}\n  ]\n}}\n",
        entries_json.join(",\n    "),
    );
    let corpus_path = out_dir.join("corpus.json");
    if let Err(e) = std::fs::write(&corpus_path, corpus) {
        eprintln!("[corpus-gen] eval-export: write corpus.json failed ({e})");
        return ExitCode::FAILURE;
    }
    eprintln!(
        "[corpus-gen] eval-export DONE: {written} fixtures + {} \
         (common={} elite={} boss={}). A/B via --corpus-path {}",
        corpus_path.display(),
        tier_counts[0],
        tier_counts[1],
        tier_counts[2],
        corpus_path.display(),
    );
    ExitCode::SUCCESS
}

/// Force a combat's player HP to `(max_hp, current_hp)`, updating BOTH the
/// legacy `PlayerCombatState` fields and the mirror `Creature` (player HP
/// reads route through `state.creatures[creature_uid]` once the mirror is
/// initialized — see CLAUDE.md "PlayerCombatState.creature_uid"). Used by
/// the Phase-0a god-mode sampler to reset a boss/elite snapshot from the
/// unkillable progression HP back to a realistic value before the
/// recorded self-play, so the fight (and its win/loss label) is faithful.
fn force_player_hp(state: &mut CombatState, max_hp: i32, current_hp: i32) {
    let max_hp = max_hp.max(1);
    let cur = current_hp.clamp(1, max_hp);
    state.player.max_hp = max_hp;
    state.player.current_hp = cur;
    if let Some(uid) = state.player.creature_uid {
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
            c.max_hp = max_hp;
            c.current_hp = cur;
        }
    }
}

/// Build a `CombatState` for a single starter encounter, for any
/// `--character`. Minimal inline replica of `sts2-cli::sim::scenario::
/// starter::build_state` for the Ironclad path — kept inline to avoid
/// promoting sts2-cli to a lib crate just for this binary. Models zero
/// starting relics for every character. The one exception —
/// Regent's starter relic DivineRight granting starting `stars` — is
/// NOT applied here (see the `stars` field comment below): it's
/// seeded by the caller, after `start_combat` runs (see
/// `run_single_combat` / `CliCharacter::starting_stars`).
fn build_combat_state(
    enc: &'static sts2_content::encounters::EncounterDef,
    seed: u64,
    deck_specs: &[CardSpec],
    character: CliCharacter,
) -> CombatState {
    let mut rng = sts2_core::dotnet_rng::Rng::new((seed) as i32);
    let max_hp: i32 = character.max_hp();

    let mut cards_arena: Vec<Card> = Vec::with_capacity(deck_specs.len());
    let mut draw_uids: SmallVec<[CardUid; 32]> = SmallVec::new();
    for (idx, spec) in deck_specs.iter().enumerate() {
        let raw = u32::try_from(idx + 1).expect("deck size fits u32");
        let uid = CardUid::new(raw).expect("non-zero");
        cards_arena.push(Card {
            uid,
            card_id: spec.id,
            cost: spec.cost,
            card_type: spec.card_type,
            target_type: spec.target_type,
            upgraded: false,
            base_damage: spec.base_damage,
            base_block: spec.base_block,
            ..Card::pod_defaults()
        });
        draw_uids.push(uid);
    }

    let mut creatures: Vec<Creature> = Vec::new();
    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();

    let n_monsters = enc.monsters.len();
    for (idx, (kind, spawn_args)) in enc.monsters.iter().enumerate() {
        let raw = u32::try_from(idx + 1).expect("monster count fits u32");
        let uid = CreatureUid::new(raw).expect("non-zero");
        let hp = kind.starting_hp(&mut rng, 0);
        let position = SpawnPosition {
            alone: n_monsters == 1,
            front: idx == 0,
            back: idx + 1 == n_monsters && !(idx == 0 && n_monsters == 1),
            index: u8::try_from(idx).expect("encounter < 256 monsters"),
        };
        let ai = build_ai(*kind, *spawn_args, position);
        creatures.push(Creature {
            id: uid.0,
            current_hp: hp,
            max_hp: hp,
            block: 0,
            powers: SmallVec::new(),
            species: Some(*kind),
            monster_id: MonsterId(kind.python_name()),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: Some(ai),
        });
        enemy_uids.push(uid);
    }

    let player_state = PlayerCombatState {
        current_hp: max_hp,
        max_hp,
        block: 0,
        energy: 0,
        max_energy: 3,
        hand: SmallVec::new(),
        draw: draw_uids,
        discard: SmallVec::new(),
        exhaust: SmallVec::new(),
        play: SmallVec::new(),
        // NOTE: `combat_loop::start_combat` unconditionally resets
        // `player.stars = 0` (mirrors Python's `CombatState.__init__`),
        // so whatever is set here is wiped the moment the caller runs
        // `start_combat(&mut state)` right after this fn returns. The
        // DivineRight seed is therefore applied POST-`start_combat` at
        // the call site (`run_single_combat`), not here — see the
        // comment there. Keep this at the character-agnostic baseline.
        stars: 0,
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

    CombatState {
        belt_value: None,
        phase: CombatPhase::Setup,
        round: 0,
        side: Side::Player,
        rng: sts2_core::dotnet_rng::Rng::new(seed.wrapping_add(0x9e37_79b9) as i32),

        run_rng: sts2_core::run_rng_set::RunRngSet::new(seed.wrapping_add(0x9e37_79b9) as i32),
        creatures,
        cards: cards_arena.into(),
        powers: Vec::new().into(),
        relics: Vec::new().into(),
        potions: Vec::new(),
        player: player_state,
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
        room_extra_rewards: Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture-export path (`--runmanager-toml-export`) must record
    /// the god-run's ascension in `[player].ascension` so ascension-aware
    /// loaders (Task 2 of the Phase-0 MCTS high-asc instrument) can
    /// rebuild A9 DeadlyEnemies attack scaling at realize time.
    ///
    /// Byte-stability constraint (Task 1 brief): the pre-flag export
    /// never emitted an `ascension` key at all, so `ascension = 0` must
    /// stay ABSENT rather than becoming a new `"ascension = 0"` line —
    /// otherwise every existing default-ascension export would change
    /// byte-for-byte. This intentionally diverges from the brief's
    /// original assertion (`toml0.contains("ascension = 0")`); see the
    /// Task 1 report for the byte-stability diff evidence backing this
    /// call.
    #[test]
    fn god_entry_to_toml_records_ascension_only_when_nonzero() {
        let args = Args::parse_from([
            "corpus_gen",
            "--character",
            "ironclad",
            "--runmanager-ascension",
            "9",
        ]);
        let (rm, _tier) =
            drive_god_run_to_target(42, 0, &args).expect("seed 42 must reach its first combat");

        let toml9 = god_entry_to_toml(&rm, 80, CliCharacter::Ironclad, 9, &[]);
        assert!(
            toml9.contains("ascension = 9"),
            "fixture must record its ascension, got:\n{toml9}"
        );

        let toml0 = god_entry_to_toml(&rm, 80, CliCharacter::Ironclad, 0, &[]);
        assert!(
            !toml0.contains("ascension"),
            "ascension 0 must stay byte-identical to the pre-flag export \
             (no ascension key at all), got:\n{toml0}"
        );
    }
}
