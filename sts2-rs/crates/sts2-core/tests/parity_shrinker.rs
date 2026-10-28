//! Parity fuzz-shrinker (bead `sts2-rs-1a6`).
//!
//! Implements Plan v2.1 §Failure-shrinking-loop step 2 — the
//! `--features shrink` entry point that consumes a raw-failure JSON
//! captured by the fuzz harness and produces a minimized failing
//! sequence under `tests/parity/shrunk/`.
//!
//! The entire body is gated under `#[cfg(feature = "shrink")]` so that
//! the default build (and `ci-fast`) compiles this file as a no-op.
//! Invoke with:
//!
//! ```fish
//! # Primary form — env var survives libtest's argv filtering.
//! STS2_SHRINK_INPUT=<path-to-raw-failure.json> \
//!   cargo test -p sts2-core --features shrink --test parity_shrinker
//!
//! # Legacy form preserved for the bead-spec invocation. Works iff a
//! # `--exact`-disabled wrapper passes the args through; otherwise
//! # libtest treats `failing-case` as a substring filter and the test
//! # is silently skipped. Prefer the env-var form.
//! cargo test -p sts2-core --features shrink --test parity_shrinker \
//!   -- failing-case <path-to-raw-failure.json>
//! ```
//!
//! Raw-failure JSON schema (recorder writes; shrinker reads):
//!
//! ```jsonc
//! {
//!   "seed": 42,
//!   "encounter": "cultist",          // optional, used for output filename
//!   "symptom": "block-off-by-one",   // optional, used for output filename
//!   "actions": [
//!     {"kind": "EndTurn"},
//!     {"kind": "PlayCard", "hand_index": 0, "target": 0},
//!     ...
//!   ],
//!   "expected_hash": 12345678901234,  // real mode: replay-and-compare signal
//!   "smoke_test_target_len": 2        // smoke-only escape hatch (see below)
//! }
//! ```
//!
//! ## Divergence signal
//!
//! For real captured failures, the recorder emits `expected_hash` =
//! `sts2_core::state_hash(state)` of the Rust replay's final state at
//! capture time. The shrinker re-runs the same `(seed, actions)`
//! sequence under the *current* Rust HEAD; if `state_hash` differs from
//! the recorded value, the input is considered "still divergent" and
//! shrinking proceeds. This is honest, deterministic, and requires no
//! Python bridge call from the shrinker itself — the cross-language
//! comparison happened upstream in the fuzz harness that captured the
//! raw failure.
//!
//! ## Smoke-only escape hatch
//!
//! The `smoke_test_target_len: K` field switches `diverges()` into a
//! pure-length predicate (`actions.len() >= K`). This lets the
//! `_smoke-test.json` fixture exercise the shrink loop end-to-end
//! without needing a real divergence. **Smoke-only**: real raw-failure
//! files MUST NOT carry this field. The recorder script asserts it is
//! absent before writing. See `tests/parity/README.md`.

#![allow(clippy::pedantic)]

#[cfg(feature = "shrink")]
mod shrinker {
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};

    use rand::SeedableRng;
    use rand_xoshiro::Xoshiro256PlusPlus;
    use serde::{Deserialize, Serialize};
    use smallvec::SmallVec;

    use sts2_core::combat_loop::{start_combat, start_player_turn};
    use sts2_core::{
        apply, state_hash, Action, CardUid, CombatPhase, CombatState, Creature, CreatureUid,
        IntentHistory, MonsterId, MonsterKind, PlayerCombatState, Side, TurnEvents,
    };

    // ── Raw-failure schema ──────────────────────────────────────────────

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(tag = "kind")]
    enum RawAction {
        PlayCard {
            hand_index: u8,
            #[serde(default)]
            target: Option<u8>,
        },
        EndTurn,
        UsePotion {
            slot: u8,
            #[serde(default)]
            target: Option<u8>,
        },
        ResolveChoice {
            selection: Vec<u8>,
        },
    }

    impl RawAction {
        fn to_action(&self) -> Action {
            match self {
                Self::PlayCard { hand_index, target } => Action::PlayCard {
                    hand_index: *hand_index,
                    target: *target,
                },
                Self::EndTurn => Action::EndTurn,
                Self::UsePotion { slot, target } => Action::UsePotion {
                    slot: *slot,
                    target: *target,
                },
                Self::ResolveChoice { selection } => Action::ResolveChoice {
                    selection: SmallVec::from_slice(selection),
                },
            }
        }
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct RawFailure {
        seed: u64,
        actions: Vec<RawAction>,
        #[serde(default)]
        encounter: Option<String>,
        #[serde(default)]
        symptom: Option<String>,
        /// Recorded `state_hash` of the final state at capture time
        /// (real mode). Absent for smoke fixtures.
        #[serde(default)]
        expected_hash: Option<u64>,
        /// **Smoke-only** escape hatch — see module docs.
        #[serde(default)]
        smoke_test_target_len: Option<usize>,
    }

    // ── Argv parsing ────────────────────────────────────────────────────

    /// Resolves the input path from (in order):
    ///   1. The `STS2_SHRINK_INPUT` env var (recommended — survives
    ///      libtest's argv filtering).
    ///   2. A `failing-case <path>` pair anywhere in argv (works if you
    ///      manage to get past libtest, e.g. via a wrapper binary).
    ///
    /// Returns `None` if neither is supplied so the test no-ops cleanly
    /// instead of panicking on accidental invocation.
    fn parse_failing_case_arg() -> Option<PathBuf> {
        if let Ok(p) = env::var("STS2_SHRINK_INPUT") {
            if !p.is_empty() {
                return Some(PathBuf::from(p));
            }
        }
        let args: Vec<String> = env::args().collect();
        let mut iter = args.iter();
        while let Some(a) = iter.next() {
            if a == "failing-case" {
                return iter.next().map(PathBuf::from);
            }
        }
        None
    }

    /// Resolve a user-supplied path against the most useful search roots.
    /// Accepts:
    ///   * absolute paths
    ///   * paths relative to CWD
    ///   * bare filenames under `tests/parity/raw-failures/`
    ///   * bare filenames under `tests/parity/shrunk/` (for idempotency
    ///     re-runs against an already-shrunk file)
    fn resolve_input(p: &Path) -> PathBuf {
        if p.is_absolute() && p.exists() {
            return p.to_path_buf();
        }
        if p.exists() {
            return p.to_path_buf();
        }
        let workspace_root = env!("CARGO_MANIFEST_DIR"); // crates/sts2-core
        let repo_root = Path::new(workspace_root)
            .parent()
            .and_then(Path::parent)
            .expect("repo root");
        // Try the path as repo-root-relative (e.g. user passed
        // `tests/parity/shrunk/foo.json` from repo root).
        let rooted = repo_root.join(p);
        if rooted.exists() {
            return rooted;
        }
        // Then as a bare filename under the standard search dirs.
        for sub in &["tests/parity/raw-failures", "tests/parity/shrunk"] {
            let candidate = repo_root.join(sub).join(p);
            if candidate.exists() {
                return candidate;
            }
        }
        // Fall through; caller will hit a clear "file not found" error.
        p.to_path_buf()
    }

    // ── Combat construction (mirrors random_playout_fuzz.rs) ────────────

    const IRONCLAD_STARTER_DECK_SIZE: u32 = 10;

    fn make_draw_pile() -> SmallVec<[CardUid; 32]> {
        let mut draw: SmallVec<[CardUid; 32]> = SmallVec::new();
        for idx in 1..=IRONCLAD_STARTER_DECK_SIZE {
            draw.push(CardUid::new(idx).expect("non-zero"));
        }
        draw
    }

    fn make_player() -> PlayerCombatState {
        PlayerCombatState {
            hand: SmallVec::new(),
            draw: make_draw_pile(),
            discard: SmallVec::new(),
            exhaust: SmallVec::new(),
            play: SmallVec::new(),
            energy: 0,
            max_energy: 3,
            stars: 0,
            current_hp: 80,
            max_hp: 80,
            block: 0,
            powers: SmallVec::new(),
            character: None,
            potions: SmallVec::new(),
            potion_slot_indices: SmallVec::new(),
            pinned_top: smallvec::SmallVec::new(),
            pinned_bottom: SmallVec::new(),
            deferred_draw_order: false,
        }
    }

    fn make_creature(
        uid: CreatureUid,
        kind: MonsterKind,
        hp: i32,
        history: IntentHistory,
    ) -> Creature {
        Creature {
            id: uid.0,
            current_hp: hp,
            max_hp: hp,
            block: 0,
            powers: SmallVec::new(),
            species: Some(kind),
            monster_id: MonsterId(kind.python_name()),
            intent: None,
            intent_history: history,
            ai: None,
        }
    }

    fn build_combat(seed: u64) -> CombatState {
        let enemy_uid = CreatureUid::new(101).expect("non-zero");
        let cultist = make_creature(enemy_uid, MonsterKind::Nibbit, 50, IntentHistory::default());
        let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
        enemy_uids.push(enemy_uid);
        let mut state = CombatState {
            phase: CombatPhase::Setup,
            round: 0,
            side: Side::Player,
            rng: sts2_core::dotnet_rng::Rng::new((seed) as i32),
            creatures: vec![cultist],
            cards: Vec::new(),
            powers: Vec::new().into(),
            relics: Vec::new(),
            potions: Vec::new(),
            player: make_player(),
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
        };
        let _ = start_combat(&mut state);
        let _ = start_player_turn(&mut state);
        state
    }

    /// Replay `actions` on a fresh combat seeded with `seed`. Skips
    /// actions that produce `StepResult::InvalidTarget`. Stops early on
    /// combat-end. Returns the final `state_hash`.
    fn replay_hash(seed: u64, actions: &[Action]) -> u64 {
        let mut state = build_combat(seed);
        for a in actions {
            if state.is_over {
                break;
            }
            let _ = apply(&mut state, a.clone());
        }
        state_hash(&state)
    }

    /// Divergence predicate. **Smoke-only hack**: when
    /// `smoke_test_target_len = K` is set, returns `actions.len() >= K`
    /// — this is the escape hatch documented in the module header and
    /// MUST NOT appear in real raw-failure files.
    fn diverges(seed: u64, actions: &[Action], raw: &RawFailure) -> bool {
        if let Some(k) = raw.smoke_test_target_len {
            return actions.len() >= k;
        }
        match raw.expected_hash {
            Some(h) => replay_hash(seed, actions) != h,
            None => {
                // No signal available — be conservative: report no
                // divergence so the shrinker bails out instead of
                // returning a misleading minimum.
                false
            }
        }
    }

    // ── Three shrink rounds ─────────────────────────────────────────────

    /// Round 1 — recursive halving. Tries dropping the back half then
    /// the front half; recurses on the smaller half if it still
    /// diverges. Returns the smallest still-diverging subsequence found.
    fn halve(seed: u64, actions: &[Action], raw: &RawFailure) -> Vec<Action> {
        let mut current = actions.to_vec();
        loop {
            if current.len() < 2 {
                return current;
            }
            let mid = current.len() / 2;
            let front: Vec<Action> = current[..mid].to_vec();
            let back: Vec<Action> = current[mid..].to_vec();
            if diverges(seed, &front, raw) {
                current = front;
                continue;
            }
            if diverges(seed, &back, raw) {
                current = back;
                continue;
            }
            return current;
        }
    }

    /// Round 2 — greedy single-action deletion. One linear pass; keeps
    /// each deletion if the result still diverges.
    fn delete_one(seed: u64, actions: &[Action], raw: &RawFailure) -> Vec<Action> {
        let mut current = actions.to_vec();
        let mut i = 0;
        while i < current.len() {
            let mut candidate = current.clone();
            candidate.remove(i);
            if diverges(seed, &candidate, raw) {
                current = candidate;
                // Don't advance `i` — re-check the new action at this slot.
            } else {
                i += 1;
            }
        }
        current
    }

    /// Round 3 — argument shrinking. For each `PlayCard` action with a
    /// `target`, try every smaller index ≥ 0; keep the smallest that
    /// still diverges.
    fn shrink_args(seed: u64, actions: &[Action], raw: &RawFailure) -> Vec<Action> {
        let mut current = actions.to_vec();
        for i in 0..current.len() {
            if let Action::PlayCard {
                hand_index,
                target: Some(t),
            } = current[i].clone()
            {
                for try_t in 0..t {
                    let mut candidate = current.clone();
                    candidate[i] = Action::PlayCard {
                        hand_index,
                        target: Some(try_t),
                    };
                    if diverges(seed, &candidate, raw) {
                        current = candidate;
                        break;
                    }
                }
            }
        }
        current
    }

    /// Run all three rounds repeatedly until a full pass produces no
    /// further reduction.
    fn shrink_loop(seed: u64, actions: &[Action], raw: &RawFailure) -> Vec<Action> {
        let mut current = actions.to_vec();
        loop {
            let before_len = current.len();
            current = halve(seed, &current, raw);
            current = delete_one(seed, &current, raw);
            let before_args = current.clone();
            current = shrink_args(seed, &current, raw);
            if current.len() == before_len && current == before_args {
                return current;
            }
        }
    }

    // ── Output ──────────────────────────────────────────────────────────

    fn output_path(raw: &RawFailure) -> PathBuf {
        let encounter = raw.encounter.as_deref().unwrap_or("unknown-encounter");
        let symptom = raw.symptom.as_deref().unwrap_or("unknown-symptom");
        let name = format!("{encounter}-{symptom}-seed{}.json", raw.seed);
        let workspace_root = env!("CARGO_MANIFEST_DIR");
        let repo_root = Path::new(workspace_root)
            .parent()
            .and_then(Path::parent)
            .expect("repo root");
        repo_root.join("tests/parity/shrunk").join(name)
    }

    fn write_output(raw: &RawFailure, shrunk: &[Action]) -> PathBuf {
        let raw_actions: Vec<RawAction> = shrunk
            .iter()
            .map(|a| match a {
                Action::PlayCard { hand_index, target } => RawAction::PlayCard {
                    hand_index: *hand_index,
                    target: *target,
                },
                Action::EndTurn => RawAction::EndTurn,
                Action::UsePotion { slot, target } => RawAction::UsePotion {
                    slot: *slot,
                    target: *target,
                },
                Action::ResolveChoice { selection } => RawAction::ResolveChoice {
                    selection: selection.to_vec(),
                },
                // Channel / Evoke (bead sts2-rs-spy P7-7) are not
                // produced by the fuzz harness — they are card-driven
                // effects, not direct actions. Round-trip them as
                // EndTurn placeholders for the shrinker so the
                // exhaustive `match` continues to compile; if the
                // shrinker ever emits these, the dedicated raw variants
                // can be added without breaking the schema.
                Action::Channel { .. } | Action::Evoke { .. } => RawAction::EndTurn,
            })
            .collect();
        let out = RawFailure {
            seed: raw.seed,
            actions: raw_actions,
            encounter: raw.encounter.clone(),
            symptom: raw.symptom.clone(),
            expected_hash: raw.expected_hash,
            smoke_test_target_len: raw.smoke_test_target_len,
        };
        let path = output_path(raw);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir shrunk/");
        }
        let body = serde_json::to_string_pretty(&out).expect("serialize shrunk");
        fs::write(&path, format!("{body}\n")).expect("write shrunk file");
        path
    }

    pub fn run() {
        let Some(input) = parse_failing_case_arg() else {
            eprintln!(
                "[parity_shrinker] no `failing-case <path>` arg supplied; \
                 shrinker is a no-op. Invoke with: \
                 `cargo test -p sts2-core --features shrink --test parity_shrinker \
                 -- failing-case <path>`",
            );
            return;
        };
        let path = resolve_input(&input);
        let body =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let raw: RawFailure =
            serde_json::from_str(&body).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));

        let actions: Vec<Action> = raw.actions.iter().map(RawAction::to_action).collect();
        let initial_len = actions.len();

        // Sanity: the input must itself diverge. If not, we cannot shrink.
        if !diverges(raw.seed, &actions, &raw) {
            panic!(
                "[parity_shrinker] input {} does not diverge under the current \
                 Rust HEAD. Nothing to shrink. (expected_hash mismatch is \
                 required; smoke_test_target_len missing.)",
                path.display(),
            );
        }

        let shrunk = shrink_loop(raw.seed, &actions, &raw);
        let out_path = write_output(&raw, &shrunk);
        eprintln!(
            "[parity_shrinker] {} actions -> {} actions; wrote {}",
            initial_len,
            shrunk.len(),
            out_path.display(),
        );
    }
}

#[cfg(feature = "shrink")]
#[test]
fn parity_shrinker_entry() {
    shrinker::run();
}

#[cfg(not(feature = "shrink"))]
#[test]
fn parity_shrinker_disabled() {
    // Compiled-but-empty when the `shrink` feature is off so the file
    // never breaks `ci-fast`. Real entry point lives behind the feature
    // flag; see module docs.
}
