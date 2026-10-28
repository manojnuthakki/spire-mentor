//! Scripted parity harness — loads `tests/parity/scripted/*.json` fixtures
//! recorded from the Python sim and validates them against the
//! `BridgeReplayTrace` shape documented in
//! `sts2-rl-agent@01c094d:sts2_env/parity/bridge_replay.py`.
//!
//! Bead: sts2-rs-yr4 (P1-5).
//!
//! **Phase 1 scope**: load fixtures, validate envelope shape + metadata,
//! and exercise the `pretty_diff` helper. The actual transition
//! assertions (run the Rust sim through each step and compare to the
//! recorded `resulting_state`) are deferred to Phase 2/4 because the
//! Rust simulator cannot yet reproduce a full Python combat — `Card` /
//! `Creature` PODs are still placeholder shape (no HP/intent/cost
//! wiring), `combat_state_to_bridge_state` emits a Phase-0 empty
//! placeholder, and there is no Ironclad starter deck or Cultist enemy
//! yet (those are P4-1 sts2-rs-zcl and P4-3 sts2-rs-pg5). When those
//! beads land, the assertion bodies inside `assert_step_matches` /
//! `assert_replay_matches` plug straight into this harness.
//!
//! Run with: `cargo test -p sts2-core --test parity_harness`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Fixture decode types
// ---------------------------------------------------------------------------
//
// These are intentionally a parallel decode struct rather than reusing
// `sts2_core::parity::Trace` — the production type owns `version: u32` /
// `mode: String` directly (good for emit), but the harness wants to be
// permissive about field absence (e.g. an early Python recorder may omit
// keys we later add). Keeping the test-side decode separate also lets us
// evolve the fixture format without touching the production crate.

/// Top-level replay envelope. Mirrors Python `BridgeReplayTrace.from_dict`
/// at `sts2-rl-agent@01c094d:sts2_env/parity/bridge_replay.py:91-104`.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct FixtureTrace {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default = "default_mode")]
    mode: String,
    #[serde(default)]
    metadata: BTreeMap<String, Value>,
    #[serde(default)]
    initial_state: Value,
    #[serde(default)]
    steps: Vec<FixtureStep>,
}

fn default_version() -> u32 {
    1
}

fn default_mode() -> String {
    "combat".to_string()
}

/// One recorded `(action, resulting_state)` pair.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct FixtureStep {
    #[serde(default)]
    action: Value,
    #[serde(default)]
    resulting_state: Value,
}

// ---------------------------------------------------------------------------
// Pretty diff helper
// ---------------------------------------------------------------------------

/// Compare two `serde_json::Value`s and return `Some(human_readable_diff)`
/// if they differ, `None` if they are structurally equal.
///
/// The diff walks both trees in parallel and reports differences as one
/// line per leaf path, e.g.:
///
/// ```text
///   /player/hp                  expected=80   actual=72
///   /enemies/0/intent           expected="AttackDefend"   actual="Attack"
///   /hand/2                     missing in actual
/// ```
///
/// Used by Phase 2/4 parity beads to surface step-by-step divergence.
#[must_use]
pub fn pretty_diff(expected: &Value, actual: &Value) -> Option<String> {
    pretty_diff_excluding(expected, actual, &[])
}

/// Variant of [`pretty_diff`] that skips JSON-pointer paths listed in
/// `exclude`. A path matches when the current walk path (e.g.
/// `/enemies/0/intent`) equals one of the entries in `exclude`; subtree
/// exclusion falls out for free because matching a parent path returns
/// before recursing into its children. The fixture schema and exclusion
/// policy are documented in `tests/parity/FIXTURE_FORMAT.md`. Used by P3-8 (sts2-rs-qct) to
/// honor a fixture's `compare.exclude_fields` annotation when comparing
/// post-end-turn states whose intent triple might diverge due to
/// Mersenne-Twister-vs-Xoshiro256++ RNG differences
/// (Python rolls per-monster move via Mersenne; Rust rolls intents via
/// Xoshiro256++ — even with `IntentHistory` persisted on `Creature`
/// per bead sts2-rs-o7r, future fixtures whose AI consumes RNG bits
/// would diverge byte-for-byte).
///
/// Returns `None` when every non-excluded leaf is structurally equal.
#[must_use]
pub fn pretty_diff_excluding(
    expected: &Value,
    actual: &Value,
    exclude: &[String],
) -> Option<String> {
    let mut out = String::new();
    diff_walk("", expected, actual, &mut out, exclude);
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn diff_walk(path: &str, expected: &Value, actual: &Value, out: &mut String, exclude: &[String]) {
    // Skip excluded paths early — covers leaf scalars and entire
    // subtrees (e.g. excluding `/enemies/0/intent` skips a string
    // leaf; excluding `/enemies/0` would skip the whole enemy object).
    if exclude.iter().any(|p| p == path) {
        return;
    }
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            // Iterate alphabetically — `serde_json::Map` is BTreeMap-backed
            // by default, so its iteration order is already sorted, but we
            // recompute the key union explicitly to handle missing keys.
            let mut keys: Vec<&String> = e.keys().chain(a.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                let child_path = format!("{path}/{k}");
                match (e.get(k), a.get(k)) {
                    (Some(ev), Some(av)) => diff_walk(&child_path, ev, av, out, exclude),
                    (Some(ev), None) => {
                        if exclude.iter().any(|p| p == &child_path) {
                            continue;
                        }
                        out.push_str(&format!(
                            "  {child_path:<40} expected={ev}  actual=<missing>\n"
                        ));
                    }
                    (None, Some(av)) => {
                        if exclude.iter().any(|p| p == &child_path) {
                            continue;
                        }
                        out.push_str(&format!(
                            "  {child_path:<40} expected=<missing>  actual={av}\n"
                        ));
                    }
                    (None, None) => {}
                }
            }
        }
        (Value::Array(e), Value::Array(a)) => {
            let max_len = e.len().max(a.len());
            for i in 0..max_len {
                let child_path = format!("{path}/{i}");
                match (e.get(i), a.get(i)) {
                    (Some(ev), Some(av)) => diff_walk(&child_path, ev, av, out, exclude),
                    (Some(ev), None) => {
                        if exclude.iter().any(|p| p == &child_path) {
                            continue;
                        }
                        out.push_str(&format!(
                            "  {child_path:<40} expected={ev}  actual=<missing>\n"
                        ));
                    }
                    (None, Some(av)) => {
                        if exclude.iter().any(|p| p == &child_path) {
                            continue;
                        }
                        out.push_str(&format!(
                            "  {child_path:<40} expected=<missing>  actual={av}\n"
                        ));
                    }
                    (None, None) => {}
                }
            }
        }
        _ => {
            if expected != actual {
                out.push_str(&format!(
                    "  {path:<40} expected={expected}  actual={actual}\n"
                ));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Fixture loading
// ---------------------------------------------------------------------------

/// Absolute path to the workspace `tests/parity/scripted/` directory.
///
/// Resolved at test time relative to `CARGO_MANIFEST_DIR` (which points
/// at `crates/sts2-core/`), so we walk up two levels to reach the
/// workspace root and then into `tests/parity/scripted/`.
fn scripted_dir() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent() // crates/
        .and_then(Path::parent) // workspace root
        .map(|root| root.join("tests").join("parity").join("scripted"))
        .expect("workspace root must be ancestor of CARGO_MANIFEST_DIR")
}

fn load_fixture(path: &Path) -> FixtureTrace {
    let raw = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("parse {} as BridgeReplayTrace: {e}", path.display()))
}

fn collect_fixtures() -> Vec<PathBuf> {
    let dir = scripted_dir();
    if !dir.exists() {
        return Vec::new();
    }
    let mut out: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect();
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// Metadata validation
// ---------------------------------------------------------------------------

/// Required metadata keys per plan lines 187-200. A fixture missing any
/// of these is considered malformed.
const REQUIRED_METADATA_KEYS: &[&str] =
    &["seed", "encounter", "character", "rust_pin", "python_pin"];

fn validate_metadata(trace: &FixtureTrace, fixture_path: &Path) {
    for &key in REQUIRED_METADATA_KEYS {
        assert!(
            trace.metadata.contains_key(key),
            "fixture {} missing required metadata key {key:?}; got keys: {:?}",
            fixture_path.display(),
            trace.metadata.keys().collect::<Vec<_>>()
        );
    }
    assert_eq!(
        trace.version,
        1,
        "fixture {} has unsupported version {}",
        fixture_path.display(),
        trace.version
    );
    assert!(
        trace.mode == "combat" || trace.mode == "run",
        "fixture {} has unsupported mode {:?}",
        fixture_path.display(),
        trace.mode
    );
}

// ---------------------------------------------------------------------------
// Step shape validation
// ---------------------------------------------------------------------------

fn validate_step_shape(step: &FixtureStep, fixture: &Path, idx: usize) {
    // Action must be either null or an object with at least an "action" key.
    match &step.action {
        Value::Null => {}
        Value::Object(o) => {
            assert!(
                o.contains_key("action"),
                "fixture {} step {idx} action object missing 'action' key: {step:?}",
                fixture.display()
            );
        }
        other => panic!(
            "fixture {} step {idx} action must be null or object, got {other:?}",
            fixture.display()
        ),
    }

    // resulting_state must be an object with a "type" key.
    let Value::Object(obj) = &step.resulting_state else {
        panic!(
            "fixture {} step {idx} resulting_state must be an object, got {:?}",
            fixture.display(),
            step.resulting_state
        );
    };
    assert!(
        obj.contains_key("type"),
        "fixture {} step {idx} resulting_state missing 'type' key",
        fixture.display()
    );
}

// ---------------------------------------------------------------------------
// Phase 1 driver — loads + validates every fixture under scripted/
// ---------------------------------------------------------------------------

#[test]
fn parity_scripted_fixtures_load_and_validate_shape() {
    let fixtures = collect_fixtures();
    if fixtures.is_empty() {
        // No fixtures recorded yet — the harness still passes. Phase 1's
        // ≥10 fixtures across characters target accrues as P2-x and P4-x
        // beads land (they need real Card / Creature PODs and the
        // bridge-state exporter populated to record meaningful traces).
        // Rather than failing, we emit a trace and return.
        eprintln!(
            "parity harness: no fixtures under {} (Phase 1 scope — assertions deferred)",
            scripted_dir().display()
        );
        return;
    }

    for path in &fixtures {
        let trace = load_fixture(path);
        validate_metadata(&trace, path);
        // Initial state must be an object (combat or card_select).
        assert!(
            matches!(trace.initial_state, Value::Object(_)),
            "fixture {} initial_state must be a JSON object",
            path.display()
        );
        for (idx, step) in trace.steps.iter().enumerate() {
            validate_step_shape(step, path, idx);
        }
        // Phase 1 envelope-only validation stops here. The first canonical
        // replay (Vulnerable + apply_damage) lives in
        // `vthin2_vulnerable_damage::vthin2_vulnerable_damage_matches_python`
        // below — it loads the matching fixture, runs `apply_damage` against
        // a hand-constructed Rust state, and asserts byte-equality on
        // initial_state + steps[0].resulting_state.
    }
}

// ---------------------------------------------------------------------------
// Bead `sts2-rs-5gvy4` (ohxoq.1) — focused arm for the Ironclad-strike-vs-
// Cultist starter fixtures. The auto-discovery driver above validates
// shape on all `.json` files under `tests/parity/scripted/`; this arm
// adds two targeted invariants per `ohxoq.1`'s "byte-equality on the
// setup snapshot and per-step shape" acceptance:
//
//   - initial Cultist HP = 48, player HP = 80, hand contains a single
//     STRIKE_IRONCLAD; metadata.character = "Ironclad".
//   - after the `play_card` action: Cultist HP drops by exactly the
//     Strike base_damage = 6 (→ 42), player energy drops by 1 (→ 2).
//   - A20 sibling differs from A0 only in metadata.ascension (combat-
//     side ascension is identity at python pin 01c094d, see
//     `docs/reality-checks/2026-05-12-ascension-audit.md`).
//
// These pins would catch a Strike base-damage regression or an
// ascension-side semantic drift before the broader auto-discovery
// noticed.
// ---------------------------------------------------------------------------

#[test]
fn ohxoq_1_ironclad_strike_seed0_setup_and_step_pin() {
    use serde_json::Value;
    use std::path::PathBuf;
    let load = |name: &str| -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests/parity/scripted")
            .join(format!("{name}.json"));
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture missing at {}: {e}", path.display()));
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("fixture parse error at {}: {e}", path.display()))
    };
    for name in ["ironclad-strike-seed0", "ironclad-strike-seed0-a20"] {
        let trace = load(name);
        assert_eq!(
            trace["metadata"]["character"], "Ironclad",
            "{name}: metadata.character"
        );
        assert_eq!(
            trace["metadata"]["encounter"], "Cultist",
            "{name}: metadata.encounter"
        );
        let init = &trace["initial_state"];
        assert_eq!(init["player"]["hp"], 80, "{name}: player.hp");
        assert_eq!(init["player"]["max_hp"], 80, "{name}: player.max_hp");
        assert_eq!(init["player"]["energy"], 3, "{name}: player.energy");
        assert_eq!(init["enemies"][0]["hp"], 48, "{name}: enemy.hp");
        assert_eq!(init["enemies"][0]["max_hp"], 48, "{name}: enemy.max_hp");
        assert_eq!(
            init["hand"].as_array().unwrap().len(),
            1,
            "{name}: hand len"
        );
        assert_eq!(
            init["hand"][0]["id"], "STRIKE_IRONCLAD",
            "{name}: hand[0].id"
        );
        assert_eq!(
            init["hand"][0]["base_damage"], 6,
            "{name}: STRIKE base_damage"
        );
        // Per-step shape: single play_card action; cultist HP drops by 6.
        let steps = trace["steps"].as_array().expect("steps array");
        assert_eq!(steps.len(), 1, "{name}: steps len");
        assert_eq!(steps[0]["action"]["action"], "play_card", "{name}: action");
        let post = &steps[0]["resulting_state"];
        assert_eq!(post["enemies"][0]["hp"], 42, "{name}: post-Strike enemy.hp");
        assert_eq!(post["player"]["energy"], 2, "{name}: post player.energy");
    }
    // A20 sibling differs only in metadata.ascension.
    let a20 = load("ironclad-strike-seed0-a20");
    assert_eq!(
        a20["metadata"]["ascension"], 20,
        "A20 fixture must pin ascension=20 in metadata"
    );
}

/// Bead `sts2-rs-kumxn` (ohxoq.3): Defect mirror of the ohxoq.1
/// Ironclad-strike arm. Same shape, same numerics — STRIKE_DEFECT
/// also has base_damage = 6 at python pin 01c094d, so the post-play
/// Cultist HP drop is identical (48→42, player energy 3→2).
#[test]
fn ohxoq_3_defect_strike_seed0_setup_and_step_pin() {
    use serde_json::Value;
    use std::path::PathBuf;
    let load = |name: &str| -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests/parity/scripted")
            .join(format!("{name}.json"));
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture missing at {}: {e}", path.display()));
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("fixture parse error at {}: {e}", path.display()))
    };
    for name in ["defect-strike-seed0", "defect-strike-seed0-a20"] {
        let trace = load(name);
        assert_eq!(
            trace["metadata"]["character"], "Defect",
            "{name}: metadata.character"
        );
        assert_eq!(
            trace["metadata"]["encounter"], "Cultist",
            "{name}: metadata.encounter"
        );
        let init = &trace["initial_state"];
        assert_eq!(init["player"]["hp"], 80, "{name}: player.hp");
        assert_eq!(init["enemies"][0]["hp"], 48, "{name}: enemy.hp");
        assert_eq!(
            init["hand"].as_array().unwrap().len(),
            1,
            "{name}: hand len"
        );
        assert_eq!(init["hand"][0]["id"], "STRIKE_DEFECT", "{name}: hand[0].id");
        assert_eq!(
            init["hand"][0]["base_damage"], 6,
            "{name}: STRIKE_DEFECT base_damage"
        );
        let steps = trace["steps"].as_array().expect("steps array");
        assert_eq!(steps.len(), 1, "{name}: steps len");
        assert_eq!(steps[0]["action"]["action"], "play_card", "{name}: action");
        let post = &steps[0]["resulting_state"];
        assert_eq!(post["enemies"][0]["hp"], 42, "{name}: post-Strike enemy.hp");
        assert_eq!(post["player"]["energy"], 2, "{name}: post player.energy");
    }
    let a20 = load("defect-strike-seed0-a20");
    assert_eq!(
        a20["metadata"]["ascension"], 20,
        "A20 fixture must pin ascension=20 in metadata"
    );
}

/// Bead `sts2-rs-2150u` (ohxoq.2): Silent mirror of ohxoq.1's
/// Ironclad-strike arm. Same shape — STRIKE_SILENT also has
/// base_damage = 6 at python pin 01c094d, so the post-play Cultist
/// HP drop is identical (48→42, player energy 3→2).
#[test]
fn ohxoq_2_silent_strike_seed0_setup_and_step_pin() {
    use serde_json::Value;
    use std::path::PathBuf;
    let load = |name: &str| -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests/parity/scripted")
            .join(format!("{name}.json"));
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture missing at {}: {e}", path.display()));
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("fixture parse error at {}: {e}", path.display()))
    };
    for name in ["silent-strike-seed0", "silent-strike-seed0-a20"] {
        let trace = load(name);
        assert_eq!(
            trace["metadata"]["character"], "Silent",
            "{name}: metadata.character"
        );
        assert_eq!(
            trace["metadata"]["encounter"], "Cultist",
            "{name}: metadata.encounter"
        );
        let init = &trace["initial_state"];
        assert_eq!(init["player"]["hp"], 80, "{name}: player.hp");
        assert_eq!(init["enemies"][0]["hp"], 48, "{name}: enemy.hp");
        assert_eq!(
            init["hand"].as_array().unwrap().len(),
            1,
            "{name}: hand len"
        );
        assert_eq!(init["hand"][0]["id"], "STRIKE_SILENT", "{name}: hand[0].id");
        assert_eq!(
            init["hand"][0]["base_damage"], 6,
            "{name}: STRIKE_SILENT base_damage"
        );
        let steps = trace["steps"].as_array().expect("steps array");
        assert_eq!(steps.len(), 1, "{name}: steps len");
        assert_eq!(steps[0]["action"]["action"], "play_card", "{name}: action");
        let post = &steps[0]["resulting_state"];
        assert_eq!(post["enemies"][0]["hp"], 42, "{name}: post-Strike enemy.hp");
        assert_eq!(post["player"]["energy"], 2, "{name}: post player.energy");
    }
    let a20 = load("silent-strike-seed0-a20");
    assert_eq!(
        a20["metadata"]["ascension"], 20,
        "A20 fixture must pin ascension=20 in metadata"
    );
}

/// Bead `sts2-rs-3xwws` (ohxoq.8): 4th-tier per-character cards.
/// Streamline (Defect) doesn't exist on the Rust side yet, so the
/// Defect slot uses Claw (cost 0, dmg 3) instead — same single-effect
/// shape as the other characters' picks in this tier.
///   - PommelStrike (Ironclad): cost 1, Attack, dmg 9. Draw 1 from
///     an empty draw pile is a no-op at this fixture's setup, so hand
///     stays empty.
///   - Acrobatics (Silent): cost 1, Skill/Self, no damage/block.
///     Draw 3 + discard 1 from empty piles are no-ops; energy delta
///     is the observable invariant.
///   - Claw (Defect): cost 0, Attack, dmg 3. The Claw-bonus power
///     ramp is multi-card; this single-play fixture pins the base hit.
#[test]
fn xwws_fourth_tier_basics_seed0_setup_and_step_pin() {
    use serde_json::Value;
    use std::path::PathBuf;
    let load = |name: &str| -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests/parity/scripted")
            .join(format!("{name}.json"));
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture missing at {}: {e}", path.display()));
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("fixture parse error at {}: {e}", path.display()))
    };
    let cases: &[(&str, &str, &str, i64, i64, i64, i64)] = &[
        (
            "ironclad-pommel-strike-seed0",
            "Ironclad",
            "POMMEL_STRIKE",
            1,
            39,
            0,
            2,
        ),
        (
            "ironclad-pommel-strike-seed0-a20",
            "Ironclad",
            "POMMEL_STRIKE",
            1,
            39,
            0,
            2,
        ),
        (
            "silent-acrobatics-seed0",
            "Silent",
            "ACROBATICS",
            1,
            48,
            0,
            2,
        ),
        (
            "silent-acrobatics-seed0-a20",
            "Silent",
            "ACROBATICS",
            1,
            48,
            0,
            2,
        ),
        ("defect-claw-seed0", "Defect", "CLAW", 0, 45, 0, 3),
        ("defect-claw-seed0-a20", "Defect", "CLAW", 0, 45, 0, 3),
    ];
    for (name, character, card_id, cost, post_hp, post_block, post_energy) in cases {
        let trace = load(name);
        assert_eq!(
            trace["metadata"]["character"], *character,
            "{name}: metadata.character"
        );
        let init = &trace["initial_state"];
        assert_eq!(init["hand"][0]["id"], *card_id, "{name}: hand[0].id");
        assert_eq!(init["hand"][0]["cost"], *cost, "{name}: card cost");
        let steps = trace["steps"].as_array().expect("steps array");
        assert_eq!(steps.len(), 1, "{name}: steps len");
        let post = &steps[0]["resulting_state"];
        assert_eq!(post["enemies"][0]["hp"], *post_hp, "{name}: post enemy.hp");
        assert_eq!(
            post["player"]["block"], *post_block,
            "{name}: post player.block"
        );
        assert_eq!(
            post["player"]["energy"], *post_energy,
            "{name}: post player.energy",
        );
    }
    for name in [
        "ironclad-pommel-strike-seed0-a20",
        "silent-acrobatics-seed0-a20",
        "defect-claw-seed0-a20",
    ] {
        let a20 = load(name);
        assert_eq!(
            a20["metadata"]["ascension"], 20,
            "{name}: A20 fixture must pin ascension=20 in metadata"
        );
    }
}

/// Bead `sts2-rs-0a9n4` (ohxoq.7): 3rd-tier per-character cards
/// covering an Attack-with-block hybrid (IronWave), a Power card
/// (Footwork), and an attack-plus-orb-channel (ColdSnap):
///   - IronWave (Ironclad): cost 1, Attack/AnyEnemy, dmg 5 + block 5 → enemy 48→43, block 0→5, energy 3→2
///   - Footwork (Silent): cost 1, Power/Self → enemy unchanged, block 0, energy 3→2
///     (Dexterity power application not exposed in `combat_action` shape; energy is observable.)
///   - ColdSnap (Defect): cost 1, Attack/AnyEnemy, dmg 6 → enemy 48→42, energy 3→2
///     (Frost orb channel not exposed; energy + enemy delta are observable.)
#[test]
fn aon4_third_tier_basics_seed0_setup_and_step_pin() {
    use serde_json::Value;
    use std::path::PathBuf;
    let load = |name: &str| -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests/parity/scripted")
            .join(format!("{name}.json"));
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture missing at {}: {e}", path.display()));
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("fixture parse error at {}: {e}", path.display()))
    };
    // (name, character, card_id, expected_cost, post_enemy_hp, post_player_block, post_player_energy)
    let cases: &[(&str, &str, &str, i64, i64, i64, i64)] = &[
        (
            "ironclad-iron-wave-seed0",
            "Ironclad",
            "IRON_WAVE",
            1,
            43,
            5,
            2,
        ),
        (
            "ironclad-iron-wave-seed0-a20",
            "Ironclad",
            "IRON_WAVE",
            1,
            43,
            5,
            2,
        ),
        ("silent-footwork-seed0", "Silent", "FOOTWORK", 1, 48, 0, 2),
        (
            "silent-footwork-seed0-a20",
            "Silent",
            "FOOTWORK",
            1,
            48,
            0,
            2,
        ),
        ("defect-cold-snap-seed0", "Defect", "COLD_SNAP", 1, 42, 0, 2),
        (
            "defect-cold-snap-seed0-a20",
            "Defect",
            "COLD_SNAP",
            1,
            42,
            0,
            2,
        ),
    ];
    for (name, character, card_id, cost, post_hp, post_block, post_energy) in cases {
        let trace = load(name);
        assert_eq!(
            trace["metadata"]["character"], *character,
            "{name}: metadata.character"
        );
        let init = &trace["initial_state"];
        assert_eq!(init["hand"][0]["id"], *card_id, "{name}: hand[0].id");
        assert_eq!(init["hand"][0]["cost"], *cost, "{name}: card cost");
        let steps = trace["steps"].as_array().expect("steps array");
        assert_eq!(steps.len(), 1, "{name}: steps len");
        let post = &steps[0]["resulting_state"];
        assert_eq!(post["enemies"][0]["hp"], *post_hp, "{name}: post enemy.hp");
        assert_eq!(
            post["player"]["block"], *post_block,
            "{name}: post player.block"
        );
        assert_eq!(
            post["player"]["energy"], *post_energy,
            "{name}: post player.energy",
        );
    }
    for name in [
        "ironclad-iron-wave-seed0-a20",
        "silent-footwork-seed0-a20",
        "defect-cold-snap-seed0-a20",
    ] {
        let a20 = load(name);
        assert_eq!(
            a20["metadata"]["ascension"], 20,
            "{name}: A20 fixture must pin ascension=20 in metadata"
        );
    }
}

/// Bead `sts2-rs-nxa29` (ohxoq.6): second-unique-basic per character.
/// Companion to ppqao (ohxoq.5):
///   - Anger (Ironclad): cost 0, Attack/AnyEnemy, base_damage 6 → enemy 48→42, energy 3→3
///   - Survivor (Silent): cost 1, Skill/Self, base_block 8 → enemy 48 (unchanged), block 0→8, energy 3→2
///   - Dualcast (Defect): cost 1, Skill/Self, no damage/block → enemy unchanged, energy 3→2
///     (orb evoke not exposed in `combat_action` bridge shape; energy delta is the
///     observable invariant.)
#[test]
fn nxa29_second_unique_basics_seed0_setup_and_step_pin() {
    use serde_json::Value;
    use std::path::PathBuf;
    let load = |name: &str| -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests/parity/scripted")
            .join(format!("{name}.json"));
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture missing at {}: {e}", path.display()));
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("fixture parse error at {}: {e}", path.display()))
    };
    // (name, character, card_id, expected_cost, post_enemy_hp, post_player_block, post_player_energy)
    let cases: &[(&str, &str, &str, i64, i64, i64, i64)] = &[
        ("ironclad-anger-seed0", "Ironclad", "ANGER", 0, 42, 0, 3),
        ("ironclad-anger-seed0-a20", "Ironclad", "ANGER", 0, 42, 0, 3),
        ("silent-survivor-seed0", "Silent", "SURVIVOR", 1, 48, 8, 2),
        (
            "silent-survivor-seed0-a20",
            "Silent",
            "SURVIVOR",
            1,
            48,
            8,
            2,
        ),
        ("defect-dualcast-seed0", "Defect", "DUALCAST", 1, 48, 0, 2),
        (
            "defect-dualcast-seed0-a20",
            "Defect",
            "DUALCAST",
            1,
            48,
            0,
            2,
        ),
    ];
    for (name, character, card_id, cost, post_hp, post_block, post_energy) in cases {
        let trace = load(name);
        assert_eq!(
            trace["metadata"]["character"], *character,
            "{name}: metadata.character"
        );
        let init = &trace["initial_state"];
        assert_eq!(init["hand"][0]["id"], *card_id, "{name}: hand[0].id");
        assert_eq!(init["hand"][0]["cost"], *cost, "{name}: card cost");
        let steps = trace["steps"].as_array().expect("steps array");
        assert_eq!(steps.len(), 1, "{name}: steps len");
        let post = &steps[0]["resulting_state"];
        assert_eq!(post["enemies"][0]["hp"], *post_hp, "{name}: post enemy.hp");
        assert_eq!(
            post["player"]["block"], *post_block,
            "{name}: post player.block"
        );
        assert_eq!(
            post["player"]["energy"], *post_energy,
            "{name}: post player.energy",
        );
    }
    for name in [
        "ironclad-anger-seed0-a20",
        "silent-survivor-seed0-a20",
        "defect-dualcast-seed0-a20",
    ] {
        let a20 = load(name);
        assert_eq!(
            a20["metadata"]["ascension"], 20,
            "{name}: A20 fixture must pin ascension=20 in metadata"
        );
    }
}

/// Bead `sts2-rs-ppqao` (ohxoq.5): per-character unique basic-card
/// fixtures complementing the Strike/Defend coverage. Each character
/// has one basic-deck card unique to them:
///   - Bash (Ironclad): cost 2, Attack, base_damage 8 → enemy 48→40, energy 3→1
///   - Neutralize (Silent): cost 0, Attack, base_damage 3 → enemy 48→45, energy 3→3
///   - Zap (Defect): cost 1, Skill/Self, no damage → enemy 48 (unchanged), energy 3→2
///     (orb channel is not exposed in the bridge state's `combat_action` shape
///     at python pin 01c094d; the energy delta is the observable invariant.)
#[test]
fn ppqao_unique_basics_seed0_setup_and_step_pin() {
    use serde_json::Value;
    use std::path::PathBuf;
    let load = |name: &str| -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests/parity/scripted")
            .join(format!("{name}.json"));
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture missing at {}: {e}", path.display()));
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("fixture parse error at {}: {e}", path.display()))
    };
    // (name, character, card_id, expected_cost, expected_base_damage,
    //  post_enemy_hp, post_player_energy)
    let cases: &[(&str, &str, &str, i64, i64, i64, i64)] = &[
        ("ironclad-bash-seed0", "Ironclad", "BASH", 2, 8, 40, 1),
        ("ironclad-bash-seed0-a20", "Ironclad", "BASH", 2, 8, 40, 1),
        (
            "silent-neutralize-seed0",
            "Silent",
            "NEUTRALIZE",
            0,
            3,
            45,
            3,
        ),
        (
            "silent-neutralize-seed0-a20",
            "Silent",
            "NEUTRALIZE",
            0,
            3,
            45,
            3,
        ),
        ("defect-zap-seed0", "Defect", "ZAP", 1, 0, 48, 2),
        ("defect-zap-seed0-a20", "Defect", "ZAP", 1, 0, 48, 2),
    ];
    for (name, character, card_id, cost, base_damage, post_hp, post_energy) in cases {
        let trace = load(name);
        assert_eq!(
            trace["metadata"]["character"], *character,
            "{name}: metadata.character"
        );
        assert_eq!(
            trace["metadata"]["encounter"], "Cultist",
            "{name}: metadata.encounter"
        );
        let init = &trace["initial_state"];
        assert_eq!(init["player"]["hp"], 80, "{name}: player.hp");
        assert_eq!(init["enemies"][0]["hp"], 48, "{name}: enemy.hp");
        assert_eq!(
            init["hand"].as_array().unwrap().len(),
            1,
            "{name}: hand len"
        );
        assert_eq!(init["hand"][0]["id"], *card_id, "{name}: hand[0].id");
        assert_eq!(init["hand"][0]["cost"], *cost, "{name}: card cost");
        if *base_damage > 0 {
            assert_eq!(
                init["hand"][0]["base_damage"], *base_damage,
                "{name}: base_damage",
            );
        }
        let steps = trace["steps"].as_array().expect("steps array");
        assert_eq!(steps.len(), 1, "{name}: steps len");
        assert_eq!(steps[0]["action"]["action"], "play_card", "{name}: action");
        let post = &steps[0]["resulting_state"];
        assert_eq!(post["enemies"][0]["hp"], *post_hp, "{name}: post enemy.hp");
        assert_eq!(
            post["player"]["energy"], *post_energy,
            "{name}: post player.energy",
        );
    }
    // A20 siblings pin ascension=20 metadata.
    for name in [
        "ironclad-bash-seed0-a20",
        "silent-neutralize-seed0-a20",
        "defect-zap-seed0-a20",
    ] {
        let a20 = load(name);
        assert_eq!(
            a20["metadata"]["ascension"], 20,
            "{name}: A20 fixture must pin ascension=20 in metadata"
        );
    }
}

// ---------------------------------------------------------------------------
// Bead `sts2-rs-jhe60` (ohxoq.4) — focused arm for the {ironclad, silent,
// defect}-defend-seed0{,-a20} fixtures. Sibling to ohxoq.1's strike pin.
// For each of the 6 fixtures: validates the pre/post play_card invariants:
//   - hand[0].id matches the expected DEFEND_<CHARACTER> token.
//   - hand[0].base_block = 5; cost = 1; type = "Skill"; target = "Self".
//   - post-state: enemy HP unchanged (Defend doesn't deal damage); player
//     block 0 -> 5; energy 3 -> 2.
//   - A20 sibling pins ascension=20 (combat-side ascension is identity at
//     python pin 01c094d, same rationale as ohxoq.1's A20 arm).
// ---------------------------------------------------------------------------

#[test]
fn jhe60_defend_seed0_setup_and_step_pin() {
    use serde_json::Value;
    use std::path::PathBuf;
    let load = |name: &str| -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests/parity/scripted")
            .join(format!("{name}.json"));
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture missing at {}: {e}", path.display()));
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("fixture parse error at {}: {e}", path.display()))
    };
    let cases: &[(&str, &str, &str)] = &[
        ("ironclad-defend-seed0", "Ironclad", "DEFEND_IRONCLAD"),
        ("ironclad-defend-seed0-a20", "Ironclad", "DEFEND_IRONCLAD"),
        ("silent-defend-seed0", "Silent", "DEFEND_SILENT"),
        ("silent-defend-seed0-a20", "Silent", "DEFEND_SILENT"),
        ("defect-defend-seed0", "Defect", "DEFEND_DEFECT"),
        ("defect-defend-seed0-a20", "Defect", "DEFEND_DEFECT"),
    ];
    for (name, character, card_id) in cases {
        let trace = load(name);
        assert_eq!(
            trace["metadata"]["character"], *character,
            "{name}: metadata.character"
        );
        assert_eq!(
            trace["metadata"]["encounter"], "Cultist",
            "{name}: metadata.encounter"
        );
        let init = &trace["initial_state"];
        assert_eq!(init["player"]["hp"], 80, "{name}: player.hp");
        assert_eq!(init["player"]["energy"], 3, "{name}: player.energy");
        assert_eq!(init["enemies"][0]["hp"], 48, "{name}: enemy.hp");
        assert_eq!(
            init["hand"].as_array().unwrap().len(),
            1,
            "{name}: hand len"
        );
        assert_eq!(init["hand"][0]["id"], *card_id, "{name}: hand[0].id");
        assert_eq!(
            init["hand"][0]["base_block"], 5,
            "{name}: DEFEND base_block"
        );
        assert_eq!(init["hand"][0]["cost"], 1, "{name}: DEFEND cost");
        assert_eq!(init["hand"][0]["type"], "Skill", "{name}: DEFEND type");
        assert_eq!(init["hand"][0]["target"], "Self", "{name}: DEFEND target");
        let steps = trace["steps"].as_array().expect("steps array");
        assert_eq!(steps.len(), 1, "{name}: steps len");
        assert_eq!(steps[0]["action"]["action"], "play_card", "{name}: action");
        let post = &steps[0]["resulting_state"];
        assert_eq!(
            post["enemies"][0]["hp"], 48,
            "{name}: post-Defend enemy.hp unchanged"
        );
        assert_eq!(
            post["player"]["block"], 5,
            "{name}: post-Defend player.block"
        );
        assert_eq!(post["player"]["energy"], 2, "{name}: post player.energy");
    }
    // A20 siblings pin ascension=20 in metadata.
    for name in [
        "ironclad-defend-seed0-a20",
        "silent-defend-seed0-a20",
        "defect-defend-seed0-a20",
    ] {
        let a20 = load(name);
        assert_eq!(
            a20["metadata"]["ascension"], 20,
            "{name}: A20 fixture must pin ascension=20 in metadata"
        );
    }
}

// ---------------------------------------------------------------------------
// Sanity tests — exercise the harness primitives independent of fixtures
// ---------------------------------------------------------------------------

#[test]
fn pretty_diff_returns_none_for_equal_values() {
    let a = serde_json::json!({"hp": 80, "block": 0, "powers": []});
    let b = serde_json::json!({"hp": 80, "block": 0, "powers": []});
    assert_eq!(pretty_diff(&a, &b), None);
}

#[test]
fn pretty_diff_reports_scalar_mismatch() {
    let expected = serde_json::json!({"hp": 80});
    let actual = serde_json::json!({"hp": 72});
    let diff = pretty_diff(&expected, &actual).expect("diff");
    assert!(diff.contains("/hp"), "diff should reference /hp: {diff}");
    assert!(
        diff.contains("80"),
        "diff should contain expected=80: {diff}"
    );
    assert!(diff.contains("72"), "diff should contain actual=72: {diff}");
}

#[test]
fn pretty_diff_reports_missing_keys() {
    let expected = serde_json::json!({"hp": 80, "block": 5});
    let actual = serde_json::json!({"hp": 80});
    let diff = pretty_diff(&expected, &actual).expect("diff");
    assert!(
        diff.contains("/block"),
        "diff should reference /block: {diff}"
    );
    assert!(
        diff.contains("missing"),
        "diff should mention missing: {diff}"
    );
}

#[test]
fn pretty_diff_reports_array_length_mismatch() {
    let expected = serde_json::json!({"hand": [1, 2, 3]});
    let actual = serde_json::json!({"hand": [1, 2]});
    let diff = pretty_diff(&expected, &actual).expect("diff");
    assert!(
        diff.contains("/hand/2"),
        "diff should reference /hand/2: {diff}"
    );
}

#[test]
fn pretty_diff_walks_nested_objects() {
    let expected = serde_json::json!({"player": {"hp": 80, "block": 0}});
    let actual = serde_json::json!({"player": {"hp": 80, "block": 5}});
    let diff = pretty_diff(&expected, &actual).expect("diff");
    assert!(
        diff.contains("/player/block"),
        "diff should reference /player/block: {diff}"
    );
}

#[test]
fn fixture_decode_round_trips_minimal_envelope() {
    // Lock in that the FixtureTrace serde shape is compatible with a
    // hand-rolled minimal envelope. This is a smoke test against
    // accidental schema drift.
    let raw = serde_json::json!({
        "version": 1,
        "mode": "combat",
        "metadata": {
            "seed": 42,
            "encounter": "ShrinkerBeetle",
            "character": "Ironclad",
            "rust_pin": "abc123",
            "python_pin": "01c094d",
        },
        "initial_state": {"type": "combat_action"},
        "steps": [
            {
                "action": {"action": "end_turn"},
                "resulting_state": {"type": "combat_action"},
            }
        ],
    });
    let serialized = serde_json::to_string(&raw).expect("serialize");
    let trace: FixtureTrace = serde_json::from_str(&serialized).expect("decode");
    assert_eq!(trace.version, 1);
    assert_eq!(trace.mode, "combat");
    assert_eq!(trace.steps.len(), 1);
    assert_eq!(trace.metadata.get("seed"), Some(&serde_json::json!(42)));
}

#[test]
fn fixture_decode_tolerates_missing_optional_fields() {
    // A trace with no metadata / no steps still decodes — validate_metadata
    // is what enforces the required keys, not the deserializer.
    let raw = serde_json::json!({"version": 1, "mode": "combat"});
    let trace: FixtureTrace = serde_json::from_value(raw).expect("decode");
    assert_eq!(trace.version, 1);
    assert!(trace.metadata.is_empty());
    assert!(trace.steps.is_empty());
}

#[test]
fn scripted_dir_resolves_to_workspace_path() {
    let dir = scripted_dir();
    // Sanity: the path ends in tests/parity/scripted.
    let tail: Vec<_> = dir
        .components()
        .rev()
        .take(3)
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    assert_eq!(tail, vec!["scripted", "parity", "tests"]);
}

// ---------------------------------------------------------------------------
// sts2-rs-6ynt.3.3 — ETHEREAL end-of-turn expel parity guard
// ---------------------------------------------------------------------------
//
// Locks the canonical Python answer in
// `tests/parity/scripted/ethereal-dazed-expel-seed0.json`: with a single
// Dazed (UNPLAYABLE + ETHEREAL) in hand and the player ending the turn,
// `_resolve_end_of_turn_hand` (combat.py:992-999) MUST move Dazed into the
// exhaust pile (NOT discard, NOT remain in hand). The recorded fixture
// captures `exhaust_pile_count: 1` and `hand: []` at
// `steps[0].resulting_state` — this test re-asserts that shape directly so
// the fixture cannot silently drift to the pre-3.3 (no-op) answer if a
// future re-record pulls in a regressed Python pin or spec.
//
// A full Rust↔Python byte-equality test for the EoT pipeline (which would
// also exercise enemy-turn intent advancement, SHRINK debuff application,
// and the next-turn draw machinery) requires substantially more harness
// scaffolding than 6ynt.3.3's surface warrants; that is deferred to a
// follow-up bead. The unit-level coverage in `combat_loop::tests` already
// pins the engine-side behavior for all four canonical ETHEREAL cards
// (Dazed / AscendersBane / Clumsy / Void).

#[test]
fn ethereal_dazed_expel_fixture_pins_python_ground_truth() {
    let path = scripted_dir().join("ethereal-dazed-expel-seed0.json");
    let raw = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let trace: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));

    // Initial: one Dazed in hand, exhaust empty.
    let init = &trace["initial_state"];
    assert_eq!(
        init["hand"].as_array().expect("initial hand").len(),
        1,
        "initial hand must contain exactly one card",
    );
    assert_eq!(
        init["hand"][0]["id"], "DAZED",
        "initial hand[0] must be Dazed",
    );
    assert_eq!(
        init["exhaust_pile_count"], 0,
        "initial exhaust pile must be empty",
    );

    // After end_turn: Dazed expelled to exhaust, hand empty.
    let res = &trace["steps"][0]["resulting_state"];
    assert_eq!(
        res["hand"].as_array().expect("resulting hand").len(),
        0,
        "after end_turn, hand must be empty (Dazed was expelled, no \
         redraw because Ironclad starter deck is not seeded in spec)",
    );
    assert_eq!(
        res["exhaust_pile_count"], 1,
        "after end_turn, exhaust pile must contain exactly one card \
         (the expelled Dazed)",
    );
    assert_eq!(
        res["discard_pile_count"], 0,
        "after end_turn, discard pile must be empty (ETHEREAL cards \
         do NOT flush to discard)",
    );
}

// ---------------------------------------------------------------------------
// Bead `sts2-rs-8z3fl.6` — Multi-target Vulnerable scaling on Whirlwind's
// X-loop. Pins the recorded Python ground truth from
// `ironclad-whirlwind-multi-vulnerable-seed0.json` (recorded against
// sts2-rl-agent@01c094d via the since-deleted scripts/record_fixture.py —
// recorder retired, fixture frozen; see PYTHON_SIM_PIN.md). Sibling Rust unit
// test `whirlwind_multi_target_vulnerable_scaling` in
// `crates/sts2-content/src/cards/ironclad.rs` covers the engine-side
// behaviour; this arm locks Python's recorded numerics behind a
// fixture-content check so a Python-side semantic drift on
// `VulnerablePower.modify_damage_multiplicative` /
// `core/constants.py::VULNERABLE_MULTIPLIER` / `calculate_damage`'s
// `math.floor` rounding would surface here.
//
// Setup: Whirlwind (X cost, 5 base damage, AllEnemies) at X=3 vs three
// ShrinkerBeetle enemies with mixed Vulnerable state (stacks=2/0/1).
// Vulnerable multiplier is presence-gated (any stack ≥ 1 → 1.5×, NOT
// stack-scaled), applied per damage instance:
//   - enemy[0] (Vuln=2): floor(5 × 1.5) × 3 = 7 × 3 = 21  → 30 → 9
//   - enemy[1] (Vuln=0): 5 × 3 = 15                         → 30 → 15
//   - enemy[2] (Vuln=1): floor(5 × 1.5) × 3 = 7 × 3 = 21  → 30 → 9
// Player energy: 3 → 0 (X-cost consumes all energy).
// ---------------------------------------------------------------------------

#[test]
fn whirlwind_multi_vulnerable_seed0_setup_and_step_pin() {
    let path = scripted_dir().join("ironclad-whirlwind-multi-vulnerable-seed0.json");
    let raw = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let trace: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));

    // Metadata.
    assert_eq!(trace["metadata"]["character"], "Ironclad");
    assert_eq!(trace["metadata"]["encounter"], "ShrinkerBeetle");
    assert_eq!(trace["metadata"]["python_pin"], "01c094d");
    assert_eq!(trace["metadata"]["seed"], 0);

    // Initial state.
    let init = &trace["initial_state"];
    assert_eq!(init["player"]["hp"], 80, "player.hp");
    assert_eq!(init["player"]["energy"], 3, "X=3 starting energy");
    assert_eq!(init["round"], 1, "mid-combat round=1");

    let init_enemies = init["enemies"].as_array().expect("enemies array");
    assert_eq!(init_enemies.len(), 3, "3 enemies in fixture");
    for (i, hp) in [30, 30, 30].iter().enumerate() {
        assert_eq!(init_enemies[i]["hp"], *hp, "enemy[{i}] init hp");
        assert_eq!(init_enemies[i]["id"], "ShrinkerBeetle", "enemy[{i}] id");
    }
    // Vulnerable presence and stack counts.
    let vuln_amount = |idx: usize| -> Option<i64> {
        init_enemies[idx]["powers"].as_array().and_then(|powers| {
            powers
                .iter()
                .find(|p| p["id"] == "VULNERABLE")
                .and_then(|p| p["amount"].as_i64())
        })
    };
    assert_eq!(vuln_amount(0), Some(2), "enemy[0] Vulnerable=2");
    assert_eq!(vuln_amount(1), None, "enemy[1] no Vulnerable");
    assert_eq!(vuln_amount(2), Some(1), "enemy[2] Vulnerable=1");

    // Hand: single Whirlwind, AllEnemies / Attack / base_damage=5 / cost=0
    // (X-cost is encoded via `has_energy_cost_x` on the card; the recorder
    // exports the raw `cost` field which is 0 at this pin).
    assert_eq!(
        init["hand"].as_array().expect("hand array").len(),
        1,
        "hand len"
    );
    assert_eq!(init["hand"][0]["id"], "WHIRLWIND");
    assert_eq!(init["hand"][0]["base_damage"], 5, "Whirlwind base_damage");
    assert_eq!(init["hand"][0]["target"], "AllEnemies");

    // Single play_card action.
    let steps = trace["steps"].as_array().expect("steps array");
    assert_eq!(steps.len(), 1, "steps len");
    assert_eq!(steps[0]["action"]["action"], "play_card");

    // Resulting state — the Vulnerable scaling pin.
    let post = &steps[0]["resulting_state"];
    let post_enemies = post["enemies"].as_array().expect("post enemies array");
    assert_eq!(
        post_enemies[0]["hp"], 9,
        "enemy[0] (Vuln=2): 3 ticks × floor(5×1.5)=7 → 30-21 = 9"
    );
    assert_eq!(
        post_enemies[1]["hp"], 15,
        "enemy[1] (no Vuln): 3 ticks × 5 → 30-15 = 15"
    );
    assert_eq!(
        post_enemies[2]["hp"], 9,
        "enemy[2] (Vuln=1): same as Vuln=2 — multiplier is presence-gated → 30-21 = 9"
    );
    assert_eq!(
        post["player"]["energy"], 0,
        "Whirlwind X-cost zeroes energy"
    );
    assert_eq!(post["discard_pile_count"], 1, "Whirlwind moves to discard");
    assert_eq!(post["hand"].as_array().expect("post hand").len(), 0);
}

// ---------------------------------------------------------------------------
// Bead `sts2-rs-8z3fl.11` — Status-card unplayable + end-turn-in-hand
// enforcement. First fixture in the family: BURN. Pin
// `sts2-rl-agent@01c094d:sts2_env/cards/status.py:1373-1383`
// (`register_turn_end_in_hand_hook(CardId.BURN)`) + `BURN_DAMAGE=2`.
// Companion to `ethereal-dazed-expel-seed0` (ethereal/exhaust path) and
// the remaining 3 status cards (Slimed/Wound/Void) tracked under
// 8z3fl.11 follow-ups.
//
// What this pins:
//   - Initial state: BURN is in hand with `cost: -1`, `playable: false`,
//     `type: "Status"` — engine surfaces unplayability through the
//     bridge-state shape (a regression that flipped `playable` to true
//     would silently let the player burn energy on a no-op).
//   - end-turn tick: player HP 80 → 78 (BURN_DAMAGE=2 unblockable),
//     Burn STAYS in hand (cost=-1 status cards aren't discarded by
//     end-of-turn; non-ETHEREAL means they don't exhaust either).
//   - Pile invariants: hand still len=1 (still Burn), discard=0,
//     exhaust=0 — distinct from the Dazed fixture where hand=0 +
//     exhaust=1.
// ---------------------------------------------------------------------------

#[test]
fn status_burn_in_hand_end_turn_seed0_pin() {
    let path = scripted_dir().join("status-burn-in-hand-end-turn-seed0.json");
    let raw = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let trace: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));

    assert_eq!(trace["metadata"]["character"], "Ironclad");
    assert_eq!(trace["metadata"]["python_pin"], "01c094d");

    let init = &trace["initial_state"];
    assert_eq!(init["player"]["hp"], 80, "player.hp before tick");
    assert_eq!(
        init["hand"].as_array().expect("initial hand").len(),
        1,
        "initial hand len"
    );
    assert_eq!(init["hand"][0]["id"], "BURN", "hand[0].id");
    assert_eq!(init["hand"][0]["cost"], -1, "BURN cost=-1 (unplayable)");
    assert_eq!(
        init["hand"][0]["playable"], false,
        "BURN must surface playable=false on the bridge state",
    );
    assert_eq!(init["hand"][0]["type"], "Status");

    let res = &trace["steps"][0]["resulting_state"];
    assert_eq!(
        res["player"]["hp"], 78,
        "BURN end-turn-in-hand tick: 80 - BURN_DAMAGE(=2) = 78",
    );
    assert_eq!(
        res["hand"].as_array().expect("resulting hand").len(),
        1,
        "BURN stays in hand after end-turn (not ethereal, not discarded by tick)",
    );
    assert_eq!(res["hand"][0]["id"], "BURN", "BURN still in hand");
    assert_eq!(
        res["discard_pile_count"], 0,
        "BURN does NOT flush to discard at end-turn",
    );
    assert_eq!(
        res["exhaust_pile_count"], 0,
        "BURN does NOT exhaust at end-turn (no ETHEREAL keyword)",
    );
}

// ---------------------------------------------------------------------------
// Bead `sts2-rs-8z3fl.13` — WOUND in-hand end-turn no-op. Pin
// `sts2-rl-agent@01c094d:sts2_env/cards/status.py:142-147` (make_wound:
// keywords={"unplayable"}, no turn_end_in_hand_hook registered). The
// negative-space companion to status-burn (active tick) and
// ethereal-dazed-expel (ethereal exhaust): Wound just sits in hand
// across end-turn — no HP loss, no movement to discard, no exhaust.
// ---------------------------------------------------------------------------

#[test]
fn status_wound_in_hand_end_turn_seed0_pin() {
    let path = scripted_dir().join("status-wound-in-hand-end-turn-seed0.json");
    let raw = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let trace: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));

    assert_eq!(trace["metadata"]["python_pin"], "01c094d");

    let init = &trace["initial_state"];
    assert_eq!(init["hand"][0]["id"], "WOUND");
    assert_eq!(init["hand"][0]["cost"], -1, "WOUND cost=-1 (unplayable)");
    assert_eq!(init["hand"][0]["playable"], false);
    assert_eq!(init["hand"][0]["type"], "Status");

    let res = &trace["steps"][0]["resulting_state"];
    assert_eq!(
        res["player"]["hp"], 80,
        "WOUND has no end-turn-in-hand hook: HP unchanged",
    );
    assert_eq!(
        res["hand"].as_array().expect("resulting hand").len(),
        1,
        "WOUND stays in hand (not ethereal, no tick)",
    );
    assert_eq!(res["hand"][0]["id"], "WOUND");
    assert_eq!(
        res["discard_pile_count"], 0,
        "WOUND does NOT flush to discard",
    );
    assert_eq!(
        res["exhaust_pile_count"], 0,
        "WOUND does NOT exhaust (no ETHEREAL keyword)",
    );
}

// ---------------------------------------------------------------------------
// VTHIN-1 (sts2-rs-cy8) — first end-to-end Rust↔Python parity check
// ---------------------------------------------------------------------------
//
// Compares Rust's `combat_state_to_bridge_state` output to a Python-recorded
// `BridgeReplayTrace.initial_state` for a setup-only scenario (no actions,
// no powers, no cards, no enemies — Ironclad player at 50/50 HP, energy
// 0/3, round 0).
//
// **Why no enemy fields yet?** At the VTHIN-1 pin the Python recorder
// labels the encounter in metadata but emits an empty `enemies: []`
// array because the Rust `Creature` POD was `{ id: NonZeroU32 }` only
// (no hp, block, name, or intent fields). VTHIN-2 (sts2-rs-5if) and
// Phase 4 (sts2-rs-pg5) wired the enemy hp/intent through; this fixture
// validates the player-side scalar shape *first* so the integration
// loop is proven before V2 layers on the damage step.
//
// **Source.** `tests/parity/scripted/setup-only-seed0.json`, recorded by
// the (since-deleted) `scripts/record_setup_only_fixture.py` against the
// archived Python sim — recorder retired, fixture frozen; see
// PYTHON_SIM_PIN.md.

mod vthin1_setup_only {
    use super::pretty_diff;

    use serde_json::Value;
    use smallvec::SmallVec;
    use std::path::PathBuf;

    use sts2_core::{
        parity::combat_state_to_bridge_state, CombatPhase, CombatState, PlayerCombatState, Side,
        TurnEvents,
    };

    /// Build a Rust `CombatState` analogous to the Python setup-only
    /// fixture: Ironclad at 50/50 HP, 0 block, energy 0, max energy 3,
    /// no enemies, all piles empty, no powers, round 0.
    ///
    /// Kept as a free function so VTHIN-2 can extend it (add a Cultist
    /// once `Creature` grows hp/block fields).
    fn construct_setup_only_state() -> CombatState {
        let player = PlayerCombatState {
            current_hp: 50,
            max_hp: 50,
            block: 0,
            energy: 0,
            max_energy: 3,
            // SmallVec defaults are empty piles + empty powers; explicit
            // for clarity / VTHIN-2 extension target.
            hand: SmallVec::new(),
            draw: SmallVec::new(),
            discard: SmallVec::new(),
            exhaust: SmallVec::new(),
            play: SmallVec::new(),
            stars: 0,
            powers: SmallVec::new(),
            character: None,
            potions: smallvec::SmallVec::new(),
            potion_slot_indices: smallvec::SmallVec::new(),
            potion_slots: 0,
            orbs: sts2_core::OrbSlots::default(),
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
        CombatState {
            belt_value: None,
            phase: CombatPhase::Setup,
            round: 0,
            side: Side::Player,
            rng: sts2_core::dotnet_rng::Rng::new(0_i32),

            run_rng: sts2_core::run_rng_set::RunRngSet::new(0_i32),
            creatures: Vec::new(),
            cards: Vec::new().into(),
            powers: Vec::new().into(),
            relics: Vec::new().into(),
            potions: Vec::new(),
            player,
            allies: SmallVec::new(),
            enemy_uids: SmallVec::new(),
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

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests/parity/scripted/setup-only-seed0.json")
    }

    /// VTHIN-1: setup-only state from Python is byte-identical to what
    /// Rust's `combat_state_to_bridge_state` produces for the analogous
    /// state. Proves the exporter shape is parity-correct on a no-action
    /// scenario before VTHIN-2 layers on damage steps.
    #[test]
    fn vthin1_setup_only_state_matches_python() {
        let path = fixture_path();
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "VTHIN-1 fixture missing at {}: {e} — recorder retired, fixture frozen \
                 (see PYTHON_SIM_PIN.md); restore the file from git history",
                path.display()
            )
        });
        let fixture: Value = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("VTHIN-1 fixture parse error at {}: {e}", path.display()));
        let expected = fixture
            .get("initial_state")
            .unwrap_or_else(|| panic!("fixture missing initial_state key: {}", path.display()));

        let rust_state = construct_setup_only_state();
        let actual = combat_state_to_bridge_state(&rust_state);

        if &actual != expected {
            let diff = pretty_diff(expected, &actual)
                .unwrap_or_else(|| "(values differ but pretty_diff produced no output)".into());
            panic!(
                "VTHIN-1 parity mismatch (fixture {}):\n{diff}\n\nexpected: {}\nactual:   {}",
                path.display(),
                serde_json::to_string_pretty(expected).unwrap_or_default(),
                serde_json::to_string_pretty(&actual).unwrap_or_default(),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// VTHIN-2 (sts2-rs-5if) — Vulnerable + apply_damage parity check
// ---------------------------------------------------------------------------
//
// Builds on VTHIN-1's setup-only fixture by adding:
//   * A Cultist enemy at 48/48 HP (real `Creature` body — bead 5if grew the
//     POD from `{ id }` to include current_hp/max_hp/block/powers).
//   * A `VulnerablePower(amount=2)` attached to the cultist.
//   * One direct-damage step: 6 raw damage → Vulnerable (×3/2) → 9 final →
//     Cultist hp 48 - 9 = 39.
//
// The fixture was recorded by the since-deleted
// `scripts/record_vulnerable_damage_fixture.py`
// against `sts2-rl-agent@01c094d`. This test asserts byte-equality on both
// `initial_state` and `steps[0].resulting_state`, locking the
// rounding-audit invariant under real conditions: a single non-identity
// rational composed by exactly one Vulnerable listener walked via
// `iter_power_listeners`'s newly-lit-up enemy segment.
//
// **Source.** `tests/parity/scripted/vulnerable-damage-seed0.json`,
// recorded by the (since-deleted) `scripts/record_vulnerable_damage_fixture.py`
// — recorder retired, fixture frozen; see PYTHON_SIM_PIN.md.

mod vthin2_vulnerable_damage {
    use super::pretty_diff;

    use serde_json::Value;
    use smallvec::SmallVec;
    use std::path::PathBuf;

    use sts2_core::{
        damage::apply_damage,
        hooks::{CreatureRef, ValueProps},
        parity::combat_state_to_bridge_state,
        CombatPhase, CombatState, Creature, CreatureUid, IntentHistory, MonsterKind,
        PlayerCombatState, Power, PowerKind, PowerUid, Side, TurnEvents,
    };

    /// Build a Rust `CombatState` analogous to the Python VTHIN-2 fixture:
    /// Ironclad at 50/50 HP + Cultist enemy at 48/48 HP with a
    /// `VulnerablePower(amount=2)` attached. Returns the state plus the
    /// cultist's `CreatureUid` (load-bearing for `apply_damage`'s target
    /// argument).
    fn construct_vulnerable_scenario() -> (CombatState, CreatureUid) {
        let player = PlayerCombatState {
            current_hp: 50,
            max_hp: 50,
            block: 0,
            energy: 0,
            max_energy: 3,
            hand: SmallVec::new(),
            draw: SmallVec::new(),
            discard: SmallVec::new(),
            exhaust: SmallVec::new(),
            play: SmallVec::new(),
            stars: 0,
            powers: SmallVec::new(),
            character: None,
            potions: smallvec::SmallVec::new(),
            potion_slot_indices: smallvec::SmallVec::new(),
            potion_slots: 0,
            orbs: sts2_core::OrbSlots::default(),
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

        // VTHIN-2 power instance: VulnerablePower(amount=2) on the cultist.
        let power_uid = PowerUid::new(1).expect("non-zero");
        let power = Power {
            id: power_uid.0,
            kind: PowerKind::Vulnerable,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        };

        let cultist_uid = CreatureUid::new(1).expect("non-zero");
        let mut cultist_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
        cultist_powers.push(power_uid);
        let cultist = Creature {
            id: cultist_uid.0,
            current_hp: 48,
            max_hp: 48,
            block: 0,
            powers: cultist_powers,
            // VTHIN-2 fixture has Cultist with `ai is None` (the
            // recorder skipped `combat.add_enemy(cultist, ai)` —
            // see CASS finding b-motyob3q-wtg5g0 (3)). Setting
            // `kind: MonsterKind::ShrinkerBeetle` matches the parity
            // exporter's `id` emission ("Cultist"); `intent: None`
            // makes the exporter omit the intent / intent_damage /
            // intent_hits fields, preserving byte-equality.
            species: Some(MonsterKind::ShrinkerBeetle),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: sts2_core::MonsterId("ShrinkerBeetle"),
            ai: None,
        };

        let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
        enemy_uids.push(cultist_uid);

        let state = CombatState {
            belt_value: None,
            phase: CombatPhase::Setup,
            round: 0,
            side: Side::Player,
            rng: sts2_core::dotnet_rng::Rng::new(0_i32),

            run_rng: sts2_core::run_rng_set::RunRngSet::new(0_i32),
            creatures: vec![cultist],
            cards: Vec::new().into(),
            powers: vec![power].into(),
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
        (state, cultist_uid)
    }

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests/parity/scripted/vulnerable-damage-seed0.json")
    }

    fn load_fixture_value() -> Value {
        let path = fixture_path();
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "VTHIN-2 fixture missing at {}: {e} — recorder retired, fixture frozen \
                 (see PYTHON_SIM_PIN.md); restore the file from git history",
                path.display()
            )
        });
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("VTHIN-2 fixture parse error at {}: {e}", path.display()))
    }

    /// **VTHIN-2 parity test:** Rust's `combat_state_to_bridge_state` +
    /// `apply_damage` produce byte-identical output to the Python
    /// recorder's initial + step-0 states. Locks the rounding-audit
    /// invariant (`floor(6 * 3/2) = 9`) under real conditions.
    #[test]
    fn vthin2_vulnerable_damage_matches_python() {
        let path = fixture_path();
        let fixture = load_fixture_value();

        // ── Initial state parity ────────────────────────────────────────
        let (mut state, cultist_uid) = construct_vulnerable_scenario();
        let actual_initial = combat_state_to_bridge_state(&state);
        let expected_initial = fixture
            .get("initial_state")
            .unwrap_or_else(|| panic!("fixture missing initial_state: {}", path.display()));
        if &actual_initial != expected_initial {
            let diff = pretty_diff(expected_initial, &actual_initial)
                .unwrap_or_else(|| "(values differ but pretty_diff produced no output)".into());
            panic!(
                "VTHIN-2 initial_state mismatch (fixture {}):\n{diff}\n\nexpected: {}\nactual:   {}",
                path.display(),
                serde_json::to_string_pretty(expected_initial).unwrap_or_default(),
                serde_json::to_string_pretty(&actual_initial).unwrap_or_default(),
            );
        }

        // ── Apply 6 raw damage → 9 after Vulnerable → cultist 48 - 9 = 39
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            cultist_uid,
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(
            hp_loss, 9,
            "expected 6×1.5=9 damage with single-floor; got {hp_loss}"
        );

        // ── Resulting state parity ──────────────────────────────────────
        let actual_resulting = combat_state_to_bridge_state(&state);
        let expected_resulting = fixture
            .get("steps")
            .and_then(|v| v.get(0))
            .and_then(|v| v.get("resulting_state"))
            .unwrap_or_else(|| {
                panic!(
                    "fixture missing steps[0].resulting_state: {}",
                    path.display()
                )
            });
        if &actual_resulting != expected_resulting {
            let diff = pretty_diff(expected_resulting, &actual_resulting)
                .unwrap_or_else(|| "(values differ but pretty_diff produced no output)".into());
            panic!(
                "VTHIN-2 resulting_state mismatch (fixture {}):\n{diff}\n\nexpected: {}\nactual:   {}",
                path.display(),
                serde_json::to_string_pretty(expected_resulting).unwrap_or_default(),
                serde_json::to_string_pretty(&actual_resulting).unwrap_or_default(),
            );
        }

        // Sanity: cultist hp dropped exactly 9.
        let cultist = &state.creatures[0];
        assert_eq!(
            cultist.current_hp,
            48 - 9,
            "expected cultist hp=39 after 6×1.5 single-floor; got {}",
            cultist.current_hp
        );
        assert_eq!(cultist.block, 0);
    }
}

// ---------------------------------------------------------------------------
// Wave-A scripted fixtures (sts2-rs-ayn / P1-5b)
// ---------------------------------------------------------------------------
//
// Generalizes VTHIN-1 / VTHIN-2 into 11 scripted fixtures recorded by
// the since-deleted `scripts/record_fixture.py` from spec files in
// `tests/parity/scripted/specs/`. Each fixture pins a single hook-chain
// arm under real conditions:
//
//   1. setup-only-seed0 (VTHIN-1)             — exporter shape, no actions
//   2. vulnerable-damage-seed0 (VTHIN-2)       — multiplicative arm (3/2)
//   3. weak-attacker-damage-seed0              — multiplicative arm (3/4) on dealer
//   4. strength-damage-seed0                   — additive arm (+amount)
//   5. frail-block-seed0                       — block multiplicative (3/4)
//   6. dexterity-block-seed0                   — block additive (+amount)
//   7. intangible-cap-seed0                    — damage cap arm (1)
//   8. vulnerable-weak-stack-seed0             — multipler composition
//   9. block-absorbs-damage-seed0              — block-then-hp absorption
//  10. zero-damage-clamp-seed0                 — final clamp at 0
//  11. double-vulnerable-stack-seed0           — amount round-trips, mul=3/2
//
// **Player-power emit divergence:** The Rust parity exporter at this pin
// hardcodes `player.powers: []` (see `crates/sts2-core/src/parity.rs:182`),
// while the Python recorder emits the real player-powers array. Fixtures
// that put a power on the *player* (Strength/Weak/Frail/Dexterity) are
// recorded faithfully but the Rust harness cannot byte-equal them until
// parity.rs grows real player-powers emission. Those tests are
// `#[ignore]`d with the divergence cited; documented in
// `tests/parity/known_divergences.md`. The Rust *numeric* result is still
// asserted (HP/block changes), so the hook-chain wiring is checked even
// where the exporter parity is not.

#[allow(clippy::too_many_lines)]
mod wave_a_fixtures {
    use super::pretty_diff;

    use serde_json::Value;
    use smallvec::SmallVec;
    use std::path::PathBuf;

    use sts2_core::{
        damage::apply_damage,
        hooks::{self, BlockCtx, CreatureRef, ValueProps},
        parity::combat_state_to_bridge_state,
        CombatPhase, CombatState, Creature, CreatureUid, IntentHistory, MonsterKind,
        PlayerCombatState, Power, PowerKind, PowerUid, Side, TurnEvents,
    };

    // ── Scenario builder ─────────────────────────────────────────────────

    /// Compact description of a power instance for scenario construction.
    /// Mirrors the `{id, amount}` pairs in the JSON spec files.
    struct PowerSpec {
        kind: PowerKind,
        amount: i32,
    }

    /// Compact description of an enemy for scenario construction.
    struct EnemySpec {
        kind: MonsterKind,
        max_hp: i32,
        current_hp: i32,
        block: i32,
        powers: Vec<PowerSpec>,
    }

    impl EnemySpec {
        fn cultist(max_hp: i32, current_hp: i32, block: i32, powers: Vec<PowerSpec>) -> Self {
            Self {
                kind: MonsterKind::ShrinkerBeetle,
                max_hp,
                current_hp,
                block,
                powers,
            }
        }

        /// Generic ctor for the N-enemy harness extensions shipped by
        /// P5-8 (sts2-rs-2nx). Lets a caller specify any `MonsterKind`
        /// variant (not just `Cultist`) so multi-enemy encounters can
        /// be built via the same `build_state` path. The multi-enemy
        /// DSL harness for the new `MonsterAi` ctors lives in
        /// `crates/sts2-content/tests/parity_multi_enemy_dsl.rs`
        /// (cross-crate because the new ctors live in `sts2-content`
        /// and `sts2-core` cannot depend on it without cycling); this
        /// `with_kind` ctor remains the in-crate entry point for
        /// `MonsterKind`-typed multi-enemy fixtures.
        #[allow(dead_code)]
        fn with_kind(
            kind: MonsterKind,
            max_hp: i32,
            current_hp: i32,
            block: i32,
            powers: Vec<PowerSpec>,
        ) -> Self {
            Self {
                kind,
                max_hp,
                current_hp,
                block,
                powers,
            }
        }
    }

    /// P5-8 (sts2-rs-2nx) smoke test: multi-enemy `build_state`
    /// constructs a valid 3-enemy encounter without panic and the
    /// resulting `CombatState` exports a 3-element `enemies` array via
    /// `combat_state_to_bridge_state`. Locks the multi-enemy code
    /// path in the `EnemySpec` generalization.
    #[test]
    fn p5_8_multi_enemy_build_state_smoke() {
        let (state, uids) = build_state(
            50,
            50,
            0,
            vec![],
            vec![
                EnemySpec::with_kind(MonsterKind::ShrinkerBeetle, 48, 48, 0, vec![]),
                EnemySpec::with_kind(MonsterKind::FuzzyWurmCrawler, 42, 42, 0, vec![]),
                EnemySpec::with_kind(MonsterKind::LeafSlimeS, 10, 10, 0, vec![]),
            ],
        );
        assert_eq!(state.creatures.len(), 3);
        assert_eq!(state.enemy_uids.len(), 3);
        assert_eq!(uids.len(), 3);
        let bridge = sts2_core::parity::combat_state_to_bridge_state(&state);
        let enemies = bridge["enemies"].as_array().expect("enemies array");
        assert_eq!(enemies.len(), 3);
    }

    /// Build a Rust [`CombatState`] mirroring a fixture's setup. Returns
    /// the state plus per-enemy `CreatureUid` handles in spec order.
    ///
    /// **Arena layout:** powers at indices 1..=N (the 1-based id matches
    /// VTHIN-2's pattern). Player powers come first (uids 1..=p), then
    /// per-enemy powers in spec order. Each enemy's `Creature.id` =
    /// `enemy_index + 1` so `creature_index_for` (in `apply_damage`) can
    /// resolve targets reliably.
    fn build_state(
        player_max_hp: i32,
        player_current_hp: i32,
        player_block: i32,
        player_powers: Vec<PowerSpec>,
        enemies: Vec<EnemySpec>,
    ) -> (CombatState, Vec<CreatureUid>) {
        let mut state = CombatState {
            belt_value: None,
            phase: CombatPhase::Setup,
            round: 0,
            side: Side::Player,
            rng: sts2_core::dotnet_rng::Rng::new(0_i32),

            run_rng: sts2_core::run_rng_set::RunRngSet::new(0_i32),
            creatures: Vec::new(),
            cards: Vec::new().into(),
            powers: Vec::new().into(),
            relics: Vec::new().into(),
            potions: Vec::new(),
            player: PlayerCombatState {
                current_hp: player_current_hp,
                max_hp: player_max_hp,
                block: player_block,
                energy: 0,
                max_energy: 3,
                hand: SmallVec::new(),
                draw: SmallVec::new(),
                discard: SmallVec::new(),
                exhaust: SmallVec::new(),
                play: SmallVec::new(),
                stars: 0,
                powers: SmallVec::new(),
                character: None,
                potions: smallvec::SmallVec::new(),
                potion_slot_indices: smallvec::SmallVec::new(),
                potion_slots: 0,
                orbs: sts2_core::OrbSlots::default(),
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
            },
            allies: SmallVec::new(),
            enemy_uids: SmallVec::new(),
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

        // Power id counter — 1-based to keep `Option<PowerUid>` niche-packed.
        let mut next_power_id: u32 = 1;
        let mut alloc_power = |state: &mut CombatState, kind: PowerKind, amount: i32| -> PowerUid {
            let uid = PowerUid::new(next_power_id).expect("non-zero");
            next_power_id += 1;
            state.powers.push(Power {
                id: uid.0,
                kind,
                amount,
                counter: 0,
                applier: None,
                skip_next_tick: false,
            });
            uid
        };

        // Player powers.
        for p in player_powers {
            let uid = alloc_power(&mut state, p.kind, p.amount);
            state.player.powers.push(uid);
        }

        // Enemies.
        let mut enemy_uids: Vec<CreatureUid> = Vec::new();
        for (idx, espec) in enemies.into_iter().enumerate() {
            let creature_uid = CreatureUid::new(u32::try_from(idx + 1).expect("fits")).unwrap();
            let mut creature_powers: SmallVec<[PowerUid; 8]> = SmallVec::new();
            for p in espec.powers {
                let puid = alloc_power(&mut state, p.kind, p.amount);
                creature_powers.push(puid);
            }
            state.creatures.push(Creature {
                id: creature_uid.0,
                current_hp: espec.current_hp,
                max_hp: espec.max_hp,
                block: espec.block,
                powers: creature_powers,
                species: Some(espec.kind),
                intent: None,
                intent_history: IntentHistory::default(),
                monster_id: sts2_core::MonsterId("ShrinkerBeetle"),
                ai: None,
            });
            state.enemy_uids.push(creature_uid);
            enemy_uids.push(creature_uid);
        }

        (state, enemy_uids)
    }

    // ── Fixture loader ───────────────────────────────────────────────────

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests/parity/scripted")
            .join(format!("{name}.json"))
    }

    fn load_fixture(name: &str) -> Value {
        let path = fixture_path(name);
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "fixture missing at {}: {e} — recorder retired, fixtures frozen \
                 (see PYTHON_SIM_PIN.md); restore the file from git history",
                path.display()
            )
        });
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("fixture parse error at {}: {e}", path.display()))
    }

    /// Assert byte-equality between Rust's exporter and the fixture's
    /// `initial_state`. Panics with a pretty diff on mismatch.
    fn assert_initial_matches(state: &CombatState, fixture: &Value, name: &str) {
        let actual = combat_state_to_bridge_state(state);
        let expected = fixture
            .get("initial_state")
            .unwrap_or_else(|| panic!("fixture {name} missing initial_state"));
        if &actual != expected {
            let diff = pretty_diff(expected, &actual)
                .unwrap_or_else(|| "(values differ but pretty_diff produced no output)".into());
            panic!(
                "fixture {name} initial_state mismatch:\n{diff}\n\n\
                 expected: {}\n\nactual:   {}",
                serde_json::to_string_pretty(expected).unwrap_or_default(),
                serde_json::to_string_pretty(&actual).unwrap_or_default(),
            );
        }
    }

    /// Assert byte-equality on `steps[step_idx].resulting_state`.
    fn assert_resulting_matches(state: &CombatState, fixture: &Value, step_idx: usize, name: &str) {
        let actual = combat_state_to_bridge_state(state);
        let expected = fixture
            .get("steps")
            .and_then(|s| s.get(step_idx))
            .and_then(|s| s.get("resulting_state"))
            .unwrap_or_else(|| panic!("fixture {name} missing steps[{step_idx}].resulting_state"));
        if &actual != expected {
            let diff = pretty_diff(expected, &actual)
                .unwrap_or_else(|| "(values differ but pretty_diff produced no output)".into());
            panic!(
                "fixture {name} step {step_idx} resulting_state mismatch:\n{diff}\n\n\
                 expected: {}\n\nactual:   {}",
                serde_json::to_string_pretty(expected).unwrap_or_default(),
                serde_json::to_string_pretty(&actual).unwrap_or_default(),
            );
        }
    }

    // ── Per-fixture tests (no player powers — full byte-equality) ────────

    /// Fixture 1: setup-only. Already covered in `vthin1_setup_only`,
    /// but re-exercised here through the generalized scenario builder
    /// to verify the builder reproduces the same shape.
    #[test]
    fn setup_only_matches_python() {
        let (state, _) = build_state(50, 50, 0, vec![], vec![]);
        let fixture = load_fixture("setup-only-seed0");
        assert_initial_matches(&state, &fixture, "setup-only-seed0");
    }

    /// Fixture 2: vulnerable-damage. Already covered in
    /// `vthin2_vulnerable_damage`; re-exercised through the generalized
    /// scenario builder.
    #[test]
    fn vulnerable_damage_matches_python() {
        let (mut state, enemies) = build_state(
            50,
            50,
            0,
            vec![],
            vec![EnemySpec::cultist(
                48,
                48,
                0,
                vec![PowerSpec {
                    kind: PowerKind::Vulnerable,
                    amount: 2,
                }],
            )],
        );
        let fixture = load_fixture("vulnerable-damage-seed0");
        assert_initial_matches(&state, &fixture, "vulnerable-damage-seed0");
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            enemies[0],
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 9);
        assert_resulting_matches(&state, &fixture, 0, "vulnerable-damage-seed0");
    }

    /// Fixture 7: intangible-cap. Cultist has Intangible(2) → 6 raw caps
    /// at 1 → cultist HP 48 - 1 = 47.
    #[test]
    fn intangible_cap_matches_python() {
        let (mut state, enemies) = build_state(
            50,
            50,
            0,
            vec![],
            vec![EnemySpec::cultist(
                48,
                48,
                0,
                vec![PowerSpec {
                    kind: PowerKind::Intangible,
                    amount: 2,
                }],
            )],
        );
        let fixture = load_fixture("intangible-cap-seed0");
        assert_initial_matches(&state, &fixture, "intangible-cap-seed0");
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            enemies[0],
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 1, "Intangible should cap at 1");
        assert_resulting_matches(&state, &fixture, 0, "intangible-cap-seed0");
    }

    /// Fixture 9: block-absorbs-damage. Cultist has 5 block; player
    /// attacks for 8 → 5 absorbed → 3 hp loss → cultist HP 48-3=45.
    #[test]
    fn block_absorbs_damage_matches_python() {
        let (mut state, enemies) = build_state(
            50,
            50,
            0,
            vec![],
            vec![EnemySpec::cultist(48, 48, 5, vec![])],
        );
        let fixture = load_fixture("block-absorbs-damage-seed0");
        assert_initial_matches(&state, &fixture, "block-absorbs-damage-seed0");
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            enemies[0],
            8,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 3, "5 block absorbs, 3 hp loss");
        assert_resulting_matches(&state, &fixture, 0, "block-absorbs-damage-seed0");
    }

    /// Fixture 11: double-vulnerable-stack. Cultist has Vulnerable(4)
    /// (representing 4 stacks); multiplier is still 1.5x (Python's
    /// Vulnerable returns the same `VULNERABLE_MULTIPLIER` regardless of
    /// stack count — duration vs strength). 6 raw → 9 → cultist HP 39.
    #[test]
    fn double_vulnerable_stack_matches_python() {
        let (mut state, enemies) = build_state(
            50,
            50,
            0,
            vec![],
            vec![EnemySpec::cultist(
                48,
                48,
                0,
                vec![PowerSpec {
                    kind: PowerKind::Vulnerable,
                    amount: 4,
                }],
            )],
        );
        let fixture = load_fixture("double-vulnerable-stack-seed0");
        assert_initial_matches(&state, &fixture, "double-vulnerable-stack-seed0");
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            enemies[0],
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(
            hp_loss, 9,
            "Vulnerable(amount=4) still applies 1.5x — stacks are duration, not strength"
        );
        assert_resulting_matches(&state, &fixture, 0, "double-vulnerable-stack-seed0");
    }

    // ── Per-fixture tests (player powers — numeric only; exporter
    // ── divergence ignored)
    //
    // These tests run apply_damage / modify_block in Rust and assert the
    // numeric outcome against the fixture's recorded HP/block. Byte-
    // equality on the player.powers array is `#[ignore]`d via marker
    // tests below until the Rust parity exporter (parity.rs:182) emits
    // real player powers. See tests/parity/known_divergences.md.

    /// Fixture 3: weak-attacker-damage. Player has Weak(2); player
    /// attacks for 6 → 6 * 0.75 = 4.5 → floor 4 → cultist HP 44.
    #[test]
    fn weak_attacker_damage_numeric_matches_python() {
        let (mut state, enemies) = build_state(
            50,
            50,
            0,
            vec![PowerSpec {
                kind: PowerKind::Weak,
                amount: 2,
            }],
            vec![EnemySpec::cultist(48, 48, 0, vec![])],
        );
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            enemies[0],
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(
            hp_loss, 4,
            "Weak(2) on dealer: floor(6 * 3/4) = 4 — single-floor invariant"
        );
        assert_eq!(state.creatures[0].current_hp, 44);

        // Document the player.powers exporter divergence by spot-checking
        // the resulting cultist HP in the fixture (numeric parity), even
        // though full state byte-equality is blocked.
        let fixture = load_fixture("weak-attacker-damage-seed0");
        let expected_hp = fixture["steps"][0]["resulting_state"]["enemies"][0]["hp"]
            .as_i64()
            .expect("hp");
        assert_eq!(expected_hp, 44, "fixture cultist HP after Weak");
    }

    /// Fixture 4: strength-damage. Player has Strength(3); player
    /// attacks for 6 → 6 + 3 = 9 → cultist HP 39.
    #[test]
    fn strength_damage_numeric_matches_python() {
        let (mut state, enemies) = build_state(
            50,
            50,
            0,
            vec![PowerSpec {
                kind: PowerKind::Strength,
                amount: 3,
            }],
            vec![EnemySpec::cultist(48, 48, 0, vec![])],
        );
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            enemies[0],
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 9, "Strength(3): 6 + 3 = 9");
        assert_eq!(state.creatures[0].current_hp, 39);

        let fixture = load_fixture("strength-damage-seed0");
        let expected_hp = fixture["steps"][0]["resulting_state"]["enemies"][0]["hp"]
            .as_i64()
            .expect("hp");
        assert_eq!(expected_hp, 39);
    }

    /// Fixture 5: frail-block. Player has Frail(2); player gains 5
    /// block → 5 * 0.75 = 3.75 → floor 3 → player.block = 3.
    #[test]
    fn frail_block_numeric_matches_python() {
        let (mut state, _enemies) = build_state(
            50,
            50,
            0,
            vec![PowerSpec {
                kind: PowerKind::Frail,
                amount: 2,
            }],
            vec![],
        );
        let modified = hooks::modify_block(
            &state,
            5,
            &BlockCtx {
                target: CreatureRef::Player,
                props: ValueProps::MOVE,
                card_source: None,
            },
        );
        assert_eq!(modified, 3, "Frail(2): floor(5 * 3/4) = 3");
        state.player.block = modified;
        assert_eq!(state.player.block, 3);

        let fixture = load_fixture("frail-block-seed0");
        let expected_block = fixture["steps"][0]["resulting_state"]["player"]["block"]
            .as_i64()
            .expect("block");
        assert_eq!(expected_block, 3);
    }

    /// Fixture 6: dexterity-block. Player has Dexterity(2); player
    /// gains 5 block → 5 + 2 = 7 → player.block = 7.
    #[test]
    fn dexterity_block_numeric_matches_python() {
        let (mut state, _enemies) = build_state(
            50,
            50,
            0,
            vec![PowerSpec {
                kind: PowerKind::Dexterity,
                amount: 2,
            }],
            vec![],
        );
        let modified = hooks::modify_block(
            &state,
            5,
            &BlockCtx {
                target: CreatureRef::Player,
                props: ValueProps::MOVE,
                card_source: None,
            },
        );
        assert_eq!(modified, 7, "Dexterity(2): 5 + 2 = 7");
        state.player.block = modified;
        assert_eq!(state.player.block, 7);

        let fixture = load_fixture("dexterity-block-seed0");
        let expected_block = fixture["steps"][0]["resulting_state"]["player"]["block"]
            .as_i64()
            .expect("block");
        assert_eq!(expected_block, 7);
    }

    /// Fixture 8: vulnerable-weak-stack. Cultist has Vulnerable(2),
    /// player has Weak(2); attack for 6 → composed mul = (3/2)*(3/4) =
    /// 9/8 → floor(6 * 9/8) = floor(6.75) = 6 → cultist HP 48-6=42.
    /// **Load-bearing single-floor-invariant test.**
    #[test]
    fn vulnerable_weak_stack_numeric_matches_python() {
        let (mut state, enemies) = build_state(
            50,
            50,
            0,
            vec![PowerSpec {
                kind: PowerKind::Weak,
                amount: 2,
            }],
            vec![EnemySpec::cultist(
                48,
                48,
                0,
                vec![PowerSpec {
                    kind: PowerKind::Vulnerable,
                    amount: 2,
                }],
            )],
        );
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            enemies[0],
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(
            hp_loss, 6,
            "Vulnerable(3/2) * Weak(3/4): floor(6 * 9/8) = floor(6.75) = 6"
        );
        assert_eq!(state.creatures[0].current_hp, 42);

        let fixture = load_fixture("vulnerable-weak-stack-seed0");
        let expected_hp = fixture["steps"][0]["resulting_state"]["enemies"][0]["hp"]
            .as_i64()
            .expect("hp");
        assert_eq!(expected_hp, 42, "single-floor invariant locked");
    }

    /// Fixture 10: zero-damage-clamp. Player has Strength(-10); player
    /// attacks for 6 → 6 + (-10) = -4 → max(0, ...) → 0. Cultist HP
    /// unchanged (48).
    #[test]
    fn zero_damage_clamp_numeric_matches_python() {
        let (mut state, enemies) = build_state(
            50,
            50,
            0,
            vec![PowerSpec {
                kind: PowerKind::Strength,
                amount: -10,
            }],
            vec![EnemySpec::cultist(48, 48, 0, vec![])],
        );
        let hp_loss = apply_damage(
            &mut state,
            CreatureRef::Player,
            enemies[0],
            6,
            ValueProps::MOVE,
            None,
        );
        assert_eq!(hp_loss, 0, "Strength(-10): max(0, 6 + -10) = 0");
        assert_eq!(state.creatures[0].current_hp, 48);

        let fixture = load_fixture("zero-damage-clamp-seed0");
        let expected_hp = fixture["steps"][0]["resulting_state"]["enemies"][0]["hp"]
            .as_i64()
            .expect("hp");
        assert_eq!(expected_hp, 48);
    }

    // ── Marker tests for the player.powers exporter divergence ───────────
    //
    // These are `#[ignore]`d but kept compile-checked so they fail loudly
    // once parity.rs grows real player.powers emission. They do NOT run
    // in the default test pass; run with
    //   `cargo test -p sts2-core --test parity_harness -- --ignored`.

    #[test]
    // [unblocked by parity.rs player_powers fix]
    fn weak_attacker_damage_byte_equal_blocked() {
        let (mut state, enemies) = build_state(
            50,
            50,
            0,
            vec![PowerSpec {
                kind: PowerKind::Weak,
                amount: 2,
            }],
            vec![EnemySpec::cultist(48, 48, 0, vec![])],
        );
        let fixture = load_fixture("weak-attacker-damage-seed0");
        assert_initial_matches(&state, &fixture, "weak-attacker-damage-seed0");
        let _ = apply_damage(
            &mut state,
            CreatureRef::Player,
            enemies[0],
            6,
            ValueProps::MOVE,
            None,
        );
        assert_resulting_matches(&state, &fixture, 0, "weak-attacker-damage-seed0");
    }

    #[test]
    // [unblocked by parity.rs player_powers fix]
    fn strength_damage_byte_equal_blocked() {
        let (mut state, enemies) = build_state(
            50,
            50,
            0,
            vec![PowerSpec {
                kind: PowerKind::Strength,
                amount: 3,
            }],
            vec![EnemySpec::cultist(48, 48, 0, vec![])],
        );
        let fixture = load_fixture("strength-damage-seed0");
        assert_initial_matches(&state, &fixture, "strength-damage-seed0");
        let _ = apply_damage(
            &mut state,
            CreatureRef::Player,
            enemies[0],
            6,
            ValueProps::MOVE,
            None,
        );
        assert_resulting_matches(&state, &fixture, 0, "strength-damage-seed0");
    }

    #[test]
    // [unblocked by parity.rs player_powers fix]
    fn frail_block_byte_equal_blocked() {
        let (mut state, _) = build_state(
            50,
            50,
            0,
            vec![PowerSpec {
                kind: PowerKind::Frail,
                amount: 2,
            }],
            vec![],
        );
        let fixture = load_fixture("frail-block-seed0");
        assert_initial_matches(&state, &fixture, "frail-block-seed0");
        state.player.block = hooks::modify_block(
            &state,
            5,
            &BlockCtx {
                target: CreatureRef::Player,
                props: ValueProps::MOVE,
                card_source: None,
            },
        );
        assert_resulting_matches(&state, &fixture, 0, "frail-block-seed0");
    }

    #[test]
    // [unblocked by parity.rs player_powers fix]
    fn dexterity_block_byte_equal_blocked() {
        let (mut state, _) = build_state(
            50,
            50,
            0,
            vec![PowerSpec {
                kind: PowerKind::Dexterity,
                amount: 2,
            }],
            vec![],
        );
        let fixture = load_fixture("dexterity-block-seed0");
        assert_initial_matches(&state, &fixture, "dexterity-block-seed0");
        state.player.block = hooks::modify_block(
            &state,
            5,
            &BlockCtx {
                target: CreatureRef::Player,
                props: ValueProps::MOVE,
                card_source: None,
            },
        );
        assert_resulting_matches(&state, &fixture, 0, "dexterity-block-seed0");
    }

    #[test]
    // [unblocked by parity.rs player_powers fix]
    fn vulnerable_weak_stack_byte_equal_blocked() {
        let (mut state, enemies) = build_state(
            50,
            50,
            0,
            vec![PowerSpec {
                kind: PowerKind::Weak,
                amount: 2,
            }],
            vec![EnemySpec::cultist(
                48,
                48,
                0,
                vec![PowerSpec {
                    kind: PowerKind::Vulnerable,
                    amount: 2,
                }],
            )],
        );
        let fixture = load_fixture("vulnerable-weak-stack-seed0");
        assert_initial_matches(&state, &fixture, "vulnerable-weak-stack-seed0");
        let _ = apply_damage(
            &mut state,
            CreatureRef::Player,
            enemies[0],
            6,
            ValueProps::MOVE,
            None,
        );
        assert_resulting_matches(&state, &fixture, 0, "vulnerable-weak-stack-seed0");
    }

    #[test]
    // [unblocked by parity.rs player_powers fix]
    fn zero_damage_clamp_byte_equal_blocked() {
        let (mut state, enemies) = build_state(
            50,
            50,
            0,
            vec![PowerSpec {
                kind: PowerKind::Strength,
                amount: -10,
            }],
            vec![EnemySpec::cultist(48, 48, 0, vec![])],
        );
        let fixture = load_fixture("zero-damage-clamp-seed0");
        assert_initial_matches(&state, &fixture, "zero-damage-clamp-seed0");
        let _ = apply_damage(
            &mut state,
            CreatureRef::Player,
            enemies[0],
            6,
            ValueProps::MOVE,
            None,
        );
        assert_resulting_matches(&state, &fixture, 0, "zero-damage-clamp-seed0");
    }

    // The `cultist_attack_intent_matches_python` test (P3-5 / sts2-rs-hrz)
    // and its fixture (`cultist-attack-intent-seed0.json`) were removed
    // in bead `sts2-rs-v1u5` along with the underlying STS1 Cultist stub.
}

// ---------------------------------------------------------------------------
// Wave-B fuzz fixtures (sts2-rs-m2r / P2-6) — removed in sts2-rs-v1u5
// ---------------------------------------------------------------------------
//
// The fuzz_cultist module and its 220 `tests/parity/fuzz/cultist-fuzz-*.json`
// fixtures were removed alongside the underlying STS1 Cultist stub in
// bead `sts2-rs-v1u5`. The fixtures were generated against the Rust
// engine's own STS1 stubs (Phase-4 scaffolding), not against the Python
// pin's STS2-native enemies, so they were regression tests for code that
// no longer exists.

// ---------------------------------------------------------------------------
// sts2-rs-zizk — relic-install slot infrastructure
// ---------------------------------------------------------------------------
//
// Validates the `relics` top-level slot added to the (since-deleted)
// recorder `scripts/record_fixture.py`. Pairs with
// `tests/parity/scripted/specs/burning-blood-setup-only-seed0.spec.json`:
// an Ironclad at 80/80 HP with `BurningBlood` installed and no actions.
//
// **Why no `combat_state_to_bridge_state` diff for relics?** Python's
// exporter at pin `01c094d` omits relics from the bridge state
// (`bridge_replay.py:344-374`), and the Rust mirror in
// `crates/sts2-core/src/parity.rs` follows suit — so byte-equality on
// the bridge envelope holds *with or without* relics installed. The
// purpose of this fixture is to prove the pipeline end-to-end:
//
//   1. Python recorder accepts the `relics` slot, resolves the
//      string via `create_relic_by_name`, and writes a real
//      `RelicInstance` into `combat.relics` before serializing
//      `initial_state`.
//   2. Rust replay reconstructs the analogous `CombatState` (HP shape
//      from the spec) AND pushes a `Relic { kind: BurningBlood }`
//      onto `state.relics`, asserts non-empty, and asserts byte-equal
//      bridge state against the recorded fixture.
//
// Downstream beads (`sts2-rs-46ms` + the rest of `sts2-rs-01o` wave 2)
// build on this slot to record fixtures that drive event-reward
// listeners (HP gain, max-HP gain, etc.) where the bridge-state diff
// *will* be observable through player.hp / player.max_hp.

mod zizk_relic_install {
    use super::pretty_diff;

    use serde_json::Value;
    use smallvec::SmallVec;
    use std::num::NonZeroU32;
    use std::path::PathBuf;

    use sts2_core::{
        parity::combat_state_to_bridge_state, CombatPhase, CombatState, PlayerCombatState, Relic,
        RelicKind, Side, TurnEvents,
    };

    /// Build a Rust `CombatState` analogous to the recorded
    /// `burning-blood-setup-only-seed0` fixture: Ironclad at 80/80 HP,
    /// no enemies, no piles, no actions — but with `BurningBlood`
    /// installed on `state.relics`. Mirrors the Python recorder's
    /// `combat.relics = [create_relic_by_name("BurningBlood")]` line.
    fn construct_burning_blood_setup_only_state() -> CombatState {
        let player = PlayerCombatState {
            current_hp: 80,
            max_hp: 80,
            block: 0,
            energy: 0,
            max_energy: 3,
            hand: SmallVec::new(),
            draw: SmallVec::new(),
            discard: SmallVec::new(),
            exhaust: SmallVec::new(),
            play: SmallVec::new(),
            stars: 0,
            powers: SmallVec::new(),
            character: None,
            potions: SmallVec::new(),
            potion_slot_indices: SmallVec::new(),
            potion_slots: 0,
            orbs: sts2_core::OrbSlots::default(),
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
        let mut state = CombatState {
            belt_value: None,
            phase: CombatPhase::Setup,
            round: 0,
            side: Side::Player,
            rng: sts2_core::dotnet_rng::Rng::new(0_i32),

            run_rng: sts2_core::run_rng_set::RunRngSet::new(0_i32),
            creatures: Vec::new(),
            cards: Vec::new().into(),
            powers: Vec::new().into(),
            relics: Vec::new().into(),
            potions: Vec::new(),
            player,
            allies: SmallVec::new(),
            enemy_uids: SmallVec::new(),
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
        // Mirror the recorder's `combat.relics = [create_relic_by_name(...)]`
        // call. The Phase-7 dispatcher attribution machinery
        // (`hooks::iter_relic_listeners`) walks `state.relics` in
        // acquisition order; pushing one entry here keeps that contract.
        state.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind: RelicKind::BurningBlood,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        state
    }

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests/parity/scripted/burning-blood-setup-only-seed0.json")
    }

    /// Bridge-state byte-equality with the recorded fixture. Because
    /// neither exporter surfaces relics, this passes when relics are
    /// installed on both sides — the test's *real* coverage is the
    /// recorder-side install (asserted implicitly by the fixture
    /// re-record succeeding via the registry) plus the Rust-side
    /// install (asserted below in `rust_state_relics_contains_burning_blood`).
    #[test]
    fn zizk_burning_blood_setup_only_matches_python() {
        let path = fixture_path();
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "zizk fixture missing at {}: {e} — recorder retired, fixture frozen \
                 (see PYTHON_SIM_PIN.md); restore the file from git history",
                path.display()
            )
        });
        let fixture: Value = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("zizk fixture parse error at {}: {e}", path.display()));
        let expected = fixture
            .get("initial_state")
            .unwrap_or_else(|| panic!("zizk fixture missing initial_state: {}", path.display()));

        let rust_state = construct_burning_blood_setup_only_state();
        let actual = combat_state_to_bridge_state(&rust_state);

        if &actual != expected {
            let diff = pretty_diff(expected, &actual)
                .unwrap_or_else(|| "(values differ but pretty_diff produced no output)".into());
            panic!(
                "zizk parity mismatch (fixture {}):\n{diff}\n\nexpected: {}\nactual:   {}",
                path.display(),
                serde_json::to_string_pretty(expected).unwrap_or_default(),
                serde_json::to_string_pretty(&actual).unwrap_or_default(),
            );
        }
    }

    /// The constructed Rust state's `relics` arena contains exactly one
    /// entry, of kind `BurningBlood`. Mirrors the recorder's invariant:
    /// `spec.relics == ["BurningBlood"]` → `len(combat.relics) == 1`
    /// and `combat.relics[0]` resolves to the `BurningBlood` class.
    #[test]
    fn rust_state_relics_contains_burning_blood() {
        let state = construct_burning_blood_setup_only_state();
        assert_eq!(state.relics.len(), 1);
        assert_eq!(state.relics[0].kind, RelicKind::BurningBlood);
    }
}

// ---------------------------------------------------------------------------
// sts2-rs-s16y — scripted parity fixtures for the 4 remaining starter relics
// (CrackedCore / RingOfTheSnake / DivineRight / BoundPhylactery)
// ---------------------------------------------------------------------------
//
// Builds on the zizk relics-install slot. Each fixture pairs a `.spec.json`
// under `tests/parity/scripted/specs/` with a generated `.json` under
// `tests/parity/scripted/`. The recorder validates the Python-side install
// (registry lookup + RelicInstance instantiation); these tests validate the
// Rust-side install (state.relics carries the analogous Relic entry) and
// byte-equality on the bridge envelope.
//
// **Behavioral parity for the actual relic effects** (BurningBlood heal,
// CrackedCore Lightning orb, RingOfTheSnake +2 cards, DivineRight +3 stars,
// BoundPhylactery revive) is NOT asserted here — the bridge schema at pin
// `01c094d` omits relics by design, the orb-emit count is not yet in the
// wire schema (CrackedCore), and the post-victory hook + revive surface
// require run-builder integration. Per the bead's own scope note:
// "setup-only fixtures only; behavioral fixtures depend on run-builder".
//
// Pin map (Python source-of-truth):
//   - CrackedCore     `relics/starter.py:83-93`   pool=DEFECT
//   - RingOfTheSnake  `relics/starter.py:53-62`   pool=SILENT
//   - DivineRight     `relics/starter.py:149-157` pool=REGENT
//   - BoundPhylactery `relics/starter.py:113-125` pool=NECROBINDER

mod s16y_starter_relics {
    use super::pretty_diff;

    use serde_json::Value;
    use smallvec::SmallVec;
    use std::num::NonZeroU32;
    use std::path::PathBuf;

    use sts2_core::{
        parity::combat_state_to_bridge_state, CombatPhase, CombatState, PlayerCombatState, Relic,
        RelicKind, Side, TurnEvents,
    };

    /// Build a Rust `CombatState` analogous to a starter-relic
    /// setup-only fixture: player at `max_hp` HP, no enemies, no piles,
    /// no actions — with the named relic installed on `state.relics`.
    /// Mirrors the zizk `construct_burning_blood_setup_only_state`
    /// pattern with `max_hp` + `RelicKind` parameterized.
    fn construct_relic_setup_state(max_hp: i32, kind: RelicKind) -> CombatState {
        let player = PlayerCombatState {
            current_hp: max_hp,
            max_hp,
            block: 0,
            energy: 0,
            max_energy: 3,
            hand: SmallVec::new(),
            draw: SmallVec::new(),
            discard: SmallVec::new(),
            exhaust: SmallVec::new(),
            play: SmallVec::new(),
            stars: 0,
            powers: SmallVec::new(),
            character: None,
            potions: SmallVec::new(),
            potion_slot_indices: SmallVec::new(),
            potion_slots: 0,
            orbs: sts2_core::OrbSlots::default(),
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
        let mut state = CombatState {
            belt_value: None,
            phase: CombatPhase::Setup,
            round: 0,
            side: Side::Player,
            rng: sts2_core::dotnet_rng::Rng::new(0_i32),

            run_rng: sts2_core::run_rng_set::RunRngSet::new(0_i32),
            creatures: Vec::new(),
            cards: Vec::new().into(),
            powers: Vec::new().into(),
            relics: Vec::new().into(),
            potions: Vec::new(),
            player,
            allies: SmallVec::new(),
            enemy_uids: SmallVec::new(),
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
        state.relics.push(Relic {
            id: NonZeroU32::new(1).expect("non-zero"),
            kind,
            counter: 0,
            is_wax: false,
            is_melted: false,
        });
        state
    }

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join(format!("tests/parity/scripted/{name}.json"))
    }

    fn assert_byte_equality(fixture_name: &str, state: &CombatState) {
        let path = fixture_path(fixture_name);
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "s16y fixture missing at {}: {e} — recorder retired, fixture frozen \
                 (see PYTHON_SIM_PIN.md); restore the file from git history",
                path.display()
            )
        });
        let fixture: Value = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("s16y fixture parse error at {}: {e}", path.display()));
        let expected = fixture
            .get("initial_state")
            .unwrap_or_else(|| panic!("s16y fixture missing initial_state: {}", path.display()));
        let actual = combat_state_to_bridge_state(state);
        if &actual != expected {
            let diff = pretty_diff(expected, &actual)
                .unwrap_or_else(|| "(values differ but pretty_diff produced no output)".into());
            panic!(
                "s16y parity mismatch (fixture {}):\n{diff}\n\nexpected: {}\nactual:   {}",
                path.display(),
                serde_json::to_string_pretty(expected).unwrap_or_default(),
                serde_json::to_string_pretty(&actual).unwrap_or_default(),
            );
        }
    }

    #[test]
    fn s16y_cracked_core_setup_only_matches_python() {
        let state = construct_relic_setup_state(75, RelicKind::CrackedCore);
        assert_byte_equality("cracked-core-setup-only-seed0", &state);
        assert_eq!(state.relics.len(), 1);
        assert_eq!(state.relics[0].kind, RelicKind::CrackedCore);
    }

    #[test]
    fn s16y_ring_of_the_snake_setup_only_matches_python() {
        let state = construct_relic_setup_state(70, RelicKind::RingOfTheSnake);
        assert_byte_equality("ring-of-the-snake-setup-only-seed0", &state);
        assert_eq!(state.relics.len(), 1);
        assert_eq!(state.relics[0].kind, RelicKind::RingOfTheSnake);
    }

    #[test]
    fn s16y_divine_right_setup_only_matches_python() {
        let state = construct_relic_setup_state(72, RelicKind::DivineRight);
        assert_byte_equality("divine-right-setup-only-seed0", &state);
        assert_eq!(state.relics.len(), 1);
        assert_eq!(state.relics[0].kind, RelicKind::DivineRight);
    }

    #[test]
    fn s16y_bound_phylactery_setup_only_matches_python() {
        let state = construct_relic_setup_state(75, RelicKind::BoundPhylactery);
        assert_byte_equality("bound-phylactery-setup-only-seed0", &state);
        assert_eq!(state.relics.len(), 1);
        assert_eq!(state.relics[0].kind, RelicKind::BoundPhylactery);
    }
}

// ---------------------------------------------------------------------------
// vmko: HeavenlyDrill X-cost surface parity (follow-up to sts2-rs-n4wd)
// ---------------------------------------------------------------------------
//
// Pins the four-branch X-cost doubling-threshold behaviour of HeavenlyDrill
// (Regent rare, base_damage=8 unupgraded) against the Python pin at
// `sts2-rl-agent@01c094d:sts2_env/cards/regent.py:730-744`. The handler
// reads `energy_spent` off the played CardInstance (lifted onto the POD
// by bead sts2-rs-n4wd in `combat_loop::play_card`) and deals
// `base_damage` damage `x` times, where:
//
//   x = energy_spent
//   if energy_spent >= 4 (effect_vars.get('energy', 4)):
//       x *= 2
//
// Four branches exercised, one per fixture step:
//
//   energy_spent=0 → 0 hits → 0 damage (no-op; below threshold)
//   energy_spent=3 → 3 hits → 24 damage (below threshold; no doubling)
//   energy_spent=4 → 8 hits → 64 damage (threshold hit; first doubling)
//   energy_spent=5 → 10 hits → 80 damage (above threshold; linear doubling)
//
// **Why a hand-authored fixture?** The Python sim's existing scripted
// `play_card` recorder does not yet support seeding `card.energy_spent`
// independently of an actual energy spend, so a recorded trace would
// require a multi-step turn cycle (start_player_turn → spend energy →
// play X-cost card) that conflates the X-cost surface with the
// energy-pool / turn-state machinery. The fixture pins the
// hand-authored bridge-state shape directly. (The Python recorder never
// grew a `set_energy_spent` action before it was retired — recorder
// retired, fixtures frozen; see PYTHON_SIM_PIN.md.)
//
// **Why `dispatch` instead of `play_card`?** The full `play_card`
// wrapper would consume energy + move the card from Hand to Play, but
// `state.player.energy` only goes up to `max_energy = 3` here — the
// `energy_spent=4` / `5` branches need to bypass that ceiling.
// Calling `dispatch` directly with the card pre-loaded into Play (via
// `push_play_card`) sets the same Play-pile top-of-stack invariant
// that `play_heavenly_drill` reads, without the Hand→Play move or
// energy bookkeeping.

mod vmko_heavenly_drill {
    use super::pretty_diff;

    use serde_json::Value;
    use smallvec::SmallVec;
    use std::path::PathBuf;

    use sts2_core::{
        card_effects::dispatch,
        cards::{CardId, CardType, TargetType},
        parity::combat_state_to_bridge_state,
        Card, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory, MonsterKind,
        PlayerCombatState, Side, TurnEvents,
    };

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests/parity/scripted/heavenly-drill-xcost-seed0.json")
    }

    fn load_fixture() -> Value {
        let path = fixture_path();
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
    }

    /// Build the per-step state: player + 1 ShrinkerBeetle (100/100 HP),
    /// HeavenlyDrill loaded onto the Play pile with the requested
    /// `energy_spent`. Mirrors the fixture's `initial_state` plus the
    /// per-step `energy_spent` override the bridge exporter cannot see
    /// (`energy_spent` is a Card POD field, not a bridge-state field).
    fn build_state_for_x(energy_spent: i32) -> (CombatState, CreatureUid) {
        let mut state = CombatState {
            belt_value: None,
            phase: CombatPhase::PlayerPlayPhase,
            round: 0,
            side: Side::Player,
            rng: sts2_core::dotnet_rng::Rng::new(0_i32),

            run_rng: sts2_core::run_rng_set::RunRngSet::new(0_i32),
            creatures: Vec::new(),
            cards: Vec::new().into(),
            powers: Vec::new().into(),
            relics: Vec::new().into(),
            potions: Vec::new(),
            player: PlayerCombatState {
                current_hp: 50,
                max_hp: 50,
                block: 0,
                energy: 0,
                max_energy: 3,
                hand: SmallVec::new(),
                draw: SmallVec::new(),
                discard: SmallVec::new(),
                exhaust: SmallVec::new(),
                play: SmallVec::new(),
                stars: 0,
                powers: SmallVec::new(),
                character: None,
                potions: smallvec::SmallVec::new(),
                potion_slot_indices: smallvec::SmallVec::new(),
                potion_slots: 0,
                orbs: sts2_core::OrbSlots::default(),
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
            },
            allies: SmallVec::new(),
            enemy_uids: SmallVec::new(),
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

        // Enemy: ShrinkerBeetle at 100/100 HP — large enough that the
        // 80-damage branch (energy_spent=5) leaves it alive at 20, so
        // the break-on-death short-circuit doesn't bias any branch's
        // hit count below the expected total.
        let enemy_uid = CreatureUid::new(1).expect("non-zero");
        state.creatures.push(Creature {
            id: enemy_uid.0,
            current_hp: 100,
            max_hp: 100,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::ShrinkerBeetle),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: sts2_core::MonsterId("ShrinkerBeetle"),
            ai: None,
        });
        state.enemy_uids.push(enemy_uid);

        // Load HeavenlyDrill onto the Play pile with the per-step
        // `energy_spent`. The handler reads `state.cards[idx].energy_spent`
        // off the top of `state.player.play`, so this is the only knob
        // the four branches differ on.
        let card_uid = sts2_core::CardUid::new(1).expect("non-zero");
        state.cards.push(Card {
            uid: card_uid,
            card_id: CardId::HeavenlyDrill,
            cost: -1,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            upgraded: false,
            base_damage: 8,
            base_block: 0,
            energy_spent,
            ..Card::pod_defaults()
        });
        state.player.play.push(card_uid);

        (state, enemy_uid)
    }

    /// Initial-state byte-equality: the fixture's `initial_state`
    /// describes the post-setup-pre-dispatch bridge state. Because
    /// HeavenlyDrill is in the Play pile (not Hand) and the bridge
    /// exporter does not surface Play-pile contents, the `hand` field
    /// is empty even though the card is loaded — that's intentional
    /// and matches the Python pin's bridge_replay shape.
    #[test]
    fn initial_state_matches_fixture() {
        // energy_spent=0 chosen arbitrarily — the initial-state
        // snapshot is identical across all four branches; only the
        // post-dispatch resulting_state diverges. (Note: even though
        // the four steps carry different `energy_spent` values, the
        // bridge exporter omits that field, so the per-step
        // `initial_state` would round-trip the same JSON. We share a
        // single `initial_state` block in the fixture for clarity.)
        let (state, _) = build_state_for_x(0);
        let actual = combat_state_to_bridge_state(&state);
        let fixture = load_fixture();
        let expected = fixture
            .get("initial_state")
            .expect("fixture missing initial_state");
        if &actual != expected {
            let diff = pretty_diff(expected, &actual)
                .unwrap_or_else(|| "(values differ but pretty_diff produced no output)".into());
            panic!(
                "heavenly-drill initial_state mismatch:\n{diff}\n\n\
                 expected: {}\n\nactual:   {}",
                serde_json::to_string_pretty(expected).unwrap_or_default(),
                serde_json::to_string_pretty(&actual).unwrap_or_default(),
            );
        }
    }

    /// Per-branch dispatch + byte-equality against
    /// `steps[idx].resulting_state`. Drives all four X-cost branches
    /// (0 / 3 / 4 / 5) through `card_effects::dispatch(HeavenlyDrill,
    /// Some(enemy))` and compares the resulting bridge state.
    fn assert_branch(step_idx: usize, energy_spent: i32, expected_enemy_hp: i32) {
        sts2_core::card_effects::ensure_installed();
        sts2_content::cards::regent::install_pool();

        let (mut state, enemy_uid) = build_state_for_x(energy_spent);

        let _ = dispatch(&mut state, CardId::HeavenlyDrill, Some(enemy_uid));

        // Quick invariant before byte-equality: per-branch enemy HP
        // matches the documented table (catches a wrong doubling
        // threshold or a swapped break-on-death short-circuit faster
        // than a full JSON diff).
        let enemy = state
            .creatures
            .iter()
            .find(|c| c.id == enemy_uid.0)
            .expect("enemy survives state");
        assert_eq!(
            enemy.current_hp, expected_enemy_hp,
            "x={energy_spent}: expected enemy_hp={expected_enemy_hp}, got {}",
            enemy.current_hp,
        );

        // Byte-equality against fixture step.
        let actual = combat_state_to_bridge_state(&state);
        let fixture = load_fixture();
        let expected = fixture
            .get("steps")
            .and_then(|s| s.get(step_idx))
            .and_then(|s| s.get("resulting_state"))
            .unwrap_or_else(|| panic!("fixture missing steps[{step_idx}].resulting_state"));
        if &actual != expected {
            let diff = pretty_diff(expected, &actual)
                .unwrap_or_else(|| "(values differ but pretty_diff produced no output)".into());
            panic!(
                "heavenly-drill step {step_idx} (x={energy_spent}) mismatch:\n{diff}\n\n\
                 expected: {}\n\nactual:   {}",
                serde_json::to_string_pretty(expected).unwrap_or_default(),
                serde_json::to_string_pretty(&actual).unwrap_or_default(),
            );
        }
    }

    #[test]
    fn x0_noop_no_damage() {
        assert_branch(0, 0, 100);
    }

    #[test]
    fn x3_below_threshold_no_doubling() {
        assert_branch(1, 3, 76);
    }

    #[test]
    fn x4_at_threshold_first_doubling() {
        assert_branch(2, 4, 36);
    }

    #[test]
    fn x5_above_threshold_linear_doubling() {
        assert_branch(3, 5, 20);
    }
}

// ---------------------------------------------------------------------------
// rggx: Acts 2+3 parity fixtures (one per tier per act minimum)
// ---------------------------------------------------------------------------
//
// First fixture under sts2-rs-rggx: Act 2 weak tier representative
// (TUNNELER_WEAK). Tunneler is single-creature with no slot args and
// no start-of-combat power, so it does NOT depend on the harness
// extensions called out in `sts2-rs-69u4` (multi-creature spawn +
// start-of-combat power application). The remaining act2/act3 tiers
// land as sibling fixtures once 69u4 closes; for now this lands the
// weak-tier slot per the bead's "one per tier per act minimum" floor.
//
// **Why no actions on this fixture?** Tunneler's BURROW ↔ BITE AI
// is deterministic given an initial state, but threading the intent
// roll through the parity exporter requires `Creature.intent` to be
// rolled to `Some(_)` — which would tie the fixture's
// `resulting_state` to whichever side of the RNG-implementation gap
// is fired first (Python Mersenne vs Rust Xoshiro256++). The
// setup-only shape avoids that gap entirely: it pins the bridge
// exporter's monster_id / hp / max_hp / block / is_alive / powers
// shape for an act2 monster, which is the smallest meaningful unit
// of "the encounter table works end-to-end". When 69u4 lands the
// recorded-Python-trace path, follow-up fixtures will exercise the
// action-driven side.

mod rggx_act2_tunneler {
    use super::pretty_diff;

    use serde_json::Value;
    use smallvec::SmallVec;
    use std::path::PathBuf;

    use sts2_core::{
        parity::combat_state_to_bridge_state, CombatPhase, CombatState, Creature, CreatureUid,
        IntentHistory, MonsterId, MonsterKind, PlayerCombatState, Side, TurnEvents,
    };

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests/parity/scripted/act2-tunneler-setup-only-seed0.json")
    }

    fn load_fixture() -> Value {
        let path = fixture_path();
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
    }

    /// Build a Rust `CombatState` mirroring the fixture's
    /// `initial_state`: Ironclad at 80/80 HP, 0 block, 0 energy, no
    /// piles, no powers — vs a single Tunneler at 26/26 HP, no
    /// powers, intent unrolled (None). `MonsterId` is set to
    /// "Tunneler" explicitly (the shared `build_state` helper in
    /// `wave_a_fixtures` hard-codes "ShrinkerBeetle"; this fixture's
    /// canonical encounter id is "Tunneler" so we set it directly).
    fn construct_tunneler_setup_state() -> CombatState {
        let player = PlayerCombatState {
            current_hp: 80,
            max_hp: 80,
            block: 0,
            energy: 0,
            max_energy: 3,
            hand: SmallVec::new(),
            draw: SmallVec::new(),
            discard: SmallVec::new(),
            exhaust: SmallVec::new(),
            play: SmallVec::new(),
            stars: 0,
            powers: SmallVec::new(),
            character: None,
            potions: SmallVec::new(),
            potion_slot_indices: SmallVec::new(),
            potion_slots: 0,
            orbs: sts2_core::OrbSlots::default(),
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

        let tunneler_uid = CreatureUid::new(1).expect("non-zero");
        let tunneler = Creature {
            id: tunneler_uid.0,
            current_hp: 26,
            max_hp: 26,
            block: 0,
            powers: SmallVec::new(),
            species: Some(MonsterKind::Tunneler),
            intent: None,
            intent_history: IntentHistory::default(),
            monster_id: MonsterId("Tunneler"),
            ai: None,
        };

        let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
        enemy_uids.push(tunneler_uid);

        CombatState {
            belt_value: None,
            phase: CombatPhase::Setup,
            round: 0,
            side: Side::Player,
            rng: sts2_core::dotnet_rng::Rng::new(0_i32),

            run_rng: sts2_core::run_rng_set::RunRngSet::new(0_i32),
            creatures: vec![tunneler],
            cards: Vec::new().into(),
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

    /// Pins the bridge-state initial shape for an act2 weak-tier
    /// representative. Catches drift in the bridge exporter's
    /// per-enemy field set / ordering when a non-Cultist `monster_id`
    /// flows through.
    #[test]
    fn initial_state_byte_equal_to_fixture() {
        let state = construct_tunneler_setup_state();
        let actual = combat_state_to_bridge_state(&state);
        let fixture = load_fixture();
        let expected = fixture
            .get("initial_state")
            .expect("fixture missing initial_state");
        if &actual != expected {
            let diff = pretty_diff(expected, &actual)
                .unwrap_or_else(|| "(values differ but pretty_diff produced no output)".into());
            panic!(
                "act2-tunneler initial_state mismatch:\n{diff}\n\n\
                 expected: {}\n\nactual:   {}",
                serde_json::to_string_pretty(expected).unwrap_or_default(),
                serde_json::to_string_pretty(&actual).unwrap_or_default(),
            );
        }
    }

    /// The Rust state carries `monster_id = "Tunneler"` (NOT the
    /// legacy "ShrinkerBeetle" fallback used by the shared
    /// `wave_a_fixtures` builder). Locks in that an act2 fixture can
    /// surface the canonical Python id even when the shared builder
    /// doesn't.
    #[test]
    fn enemy_monster_id_is_tunneler() {
        let state = construct_tunneler_setup_state();
        let bridge = combat_state_to_bridge_state(&state);
        let enemies = bridge["enemies"].as_array().expect("enemies array");
        assert_eq!(enemies.len(), 1);
        assert_eq!(enemies[0]["id"], "Tunneler");
        assert_eq!(enemies[0]["hp"], 26);
        assert_eq!(enemies[0]["max_hp"], 26);
        assert_eq!(enemies[0]["is_alive"], true);
    }
}
