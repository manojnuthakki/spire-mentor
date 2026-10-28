//! `PyO3` dialect shim over [`sts2_wire`], plus the wire *serializer*.
//!
//! **Bead:** `sts2-rs-92sk` (hint-T1) / `sts2-rs-osyf` (T2) / `sts2-rs-wfjn` (T3).
//!
//! The parse (`ParsedState`) and reconstruction (`CombatState`) halves used to
//! live here and read `PyDict` directly, which meant a Python interpreter was
//! a hard requirement for anything that wanted to ingest a wire dict. They now
//! live in [`sts2_wire`], generic over [`WireValue`]; what remains here is
//! everything that is genuinely *about Python*:
//!
//! * [`impl WireValue for Bound<'_, PyAny>`](WireValue) — the Python dialect
//!   of the same accessor surface `serde_json::Value` implements over there.
//!   Both hosts drive one parser, so they cannot drift.
//! * [`wire_err`] — [`sts2_wire::WireError`] → the two Python exception types.
//!   The Schema/Unsupported split is load-bearing: `tests/fail_loud.rs`
//!   asserts the exception *type* per failure class, not just the message.
//! * The `#[pyfunction]` entry points, with their historical `PyResult`
//!   signatures unchanged for every existing caller.
//! * `state_to_dict` — the inverse direction (`CombatState` → `PyDict`). The
//!   dict is built ONCE, as a `serde_json::Value`, by
//!   [`sts2_wire::emit::combat_state_to_value`]; this crate only runs that
//!   value through `pythonize`. Parity by construction: a wasm workbench and
//!   this module cannot emit different shapes because neither has its own.
//!
//! Everything else is re-exported from [`sts2_wire::state_dict`] so `use
//! super::*` in the tests below, and `crate::state_dict::…` in `lib.rs`,
//! resolve exactly as they did before the move.

use pyo3::create_exception;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyList};

use sts2_core::{CombatState as CoreCombatState, Intent};

// Test-only: the `PyDict` round-trip fixtures build states by hand.
#[cfg(test)]
use sts2_core::cards::VarKey;
#[cfg(test)]
use sts2_core::{CardUid, CreatureUid, MonsterKind, Orb, Power, PowerKind, PowerUid};

// Test-only: the `PyDict` round-trip fixtures build states by hand.
#[cfg(test)]
use smallvec::SmallVec;
#[cfg(test)]
use std::num::NonZeroU32;
#[cfg(test)]
use sts2_content::monster_dispatch::{build_ai, MonsterSpawnArgs, SpawnPosition};
#[cfg(test)]
use sts2_core::cards::{CardId, EnchantmentId};
#[cfg(test)]
use sts2_core::{Creature, IntentHistory, OrbKind};

use sts2_wire::{WireError, WireValue};

/// Everything the parse/build halves used to expose from this module. The
/// glob keeps `use super::*` (tests) and `crate::state_dict::…` (`lib.rs`)
/// pointing at the same names they always did; the `PyResult`-returning
/// wrappers defined below shadow their `WireResult` counterparts.
pub use sts2_wire::state_dict::*;

// ---------------------------------------------------------------------------
// Exception type
// ---------------------------------------------------------------------------

create_exception!(
    sts2_rs,
    UnsupportedStateError,
    PyValueError,
    "Raised when the wire-format dict references an id, power, or shape\n\
     the Rust hint engine cannot model. Message carries the offending\n\
     JSON path (e.g. `player.relics[3]: GHIBBET_MAW`)."
);

/// Map a [`WireError`] onto the Python exception the wire surface has always
/// raised for that class.
///
/// Not a `From` impl: both types are foreign to this crate, so the orphan
/// rule forbids one. Every `PyO3` boundary below calls this explicitly.
pub fn wire_err(e: WireError) -> PyErr {
    // `Display` is `"{path}: {detail}"`, byte-identical to what the old
    // `unsupported()` / `schema_err()` factories produced.
    let msg = e.to_string();
    if e.is_unsupported() {
        UnsupportedStateError::new_err(msg)
    } else {
        PyValueError::new_err(msg)
    }
}

// ---------------------------------------------------------------------------
// The Python dialect of `WireValue`
// ---------------------------------------------------------------------------

/// A `Bound<'py, PyAny>` wearing the [`WireValue`] trait.
///
/// The newtype is the orphan rule's doing, not a design choice: `WireValue`
/// belongs to `sts2-wire` and `Bound` to `pyo3`, so neither crate can carry
/// the impl and it has to hang off a type this one owns. Wrapping is free —
/// `Bound` is a refcounted handle and `Clone` is a refcount bump.
#[derive(Clone, Debug)]
pub struct PyWire<'py>(pub Bound<'py, PyAny>);

impl<'py> PyWire<'py> {
    /// Wrap a dict for the parser.
    fn from_dict(d: &Bound<'py, PyDict>) -> Self {
        Self(d.as_any().clone())
    }
}

impl WireValue for PyWire<'_> {
    fn get(&self, key: &str) -> Option<Self> {
        // `PyDict::get_item` returns `Ok(Some(v))` for a key explicitly set
        // to `None`, and `Ok(None)` only when the key is absent — the
        // absent-vs-null split the trait requires.
        Some(Self(
            self.0
                .downcast::<PyDict>()
                .ok()?
                .get_item(key)
                .ok()
                .flatten()?,
        ))
    }

    fn is_null(&self) -> bool {
        self.0.is_none()
    }

    fn is_object(&self) -> bool {
        self.0.downcast::<PyDict>().is_ok()
    }

    /// Key-sorted, per the trait contract — a `PyDict` iterates in insertion
    /// order, so without the sort this would hand back a different `Vec`
    /// order than the `serde_json` impl for the same logical dict.
    ///
    /// A non-string key collapses the whole call to `None`, which the callers
    /// report as `expected dict` at the same path the old code reported
    /// `keys must be strings` at. Same class, same path, and no producer
    /// emits one.
    fn entries(&self) -> Option<Vec<(String, Self)>> {
        let dict = self.0.downcast::<PyDict>().ok()?;
        let mut out: Vec<(String, Self)> = Vec::with_capacity(dict.len());
        for (k, v) in dict.iter() {
            out.push((k.extract::<String>().ok()?, Self(v)));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Some(out)
    }

    fn as_str(&self) -> Option<String> {
        self.0.extract::<String>().ok()
    }

    fn as_i64(&self) -> Option<i64> {
        self.0.extract::<i64>().ok()
    }

    fn as_u64(&self) -> Option<u64> {
        self.0.extract::<u64>().ok()
    }

    fn as_f64(&self) -> Option<f64> {
        self.0.extract::<f64>().ok()
    }

    fn as_bool(&self) -> Option<bool> {
        self.0.extract::<bool>().ok()
    }

    fn as_array(&self) -> Option<Vec<Self>> {
        Some(self.0.downcast::<PyList>().ok()?.iter().map(Self).collect())
    }

    fn render(&self) -> String {
        // Python's own `str(v)` — the mid-resolution guard has always named
        // the offending value this way.
        self.0.to_string()
    }
}

// ---------------------------------------------------------------------------
// PyResult-returning wrappers
// ---------------------------------------------------------------------------
//
// These shadow the glob-imported `sts2_wire` originals so every existing
// caller (`lib.rs`, `tests/fail_loud.rs`, `tests/mcts_recommend_wins.rs`,
// the test module below) keeps its exact signature and error type.

/// Parse + validate a wire-format dict into [`ParsedState`].
///
/// # Errors
///
/// `ValueError` for schema violations, `UnsupportedStateError` for content
/// the engine cannot model — both carrying the offending JSON path.
pub fn parse_state_dict(d: &Bound<'_, PyDict>) -> PyResult<ParsedState> {
    sts2_wire::parse_state_dict(&PyWire::from_dict(d)).map_err(wire_err)
}

/// Translate a [`ParsedState`] into a fully-initialised [`CoreCombatState`].
///
/// # Errors
///
/// `UnsupportedStateError` for any unresolvable runtime spec lookup.
pub fn state_from_dict_impl(parsed: &ParsedState) -> PyResult<CoreCombatState> {
    sts2_wire::state_from_dict_impl(parsed).map_err(wire_err)
}

/// Translate one wire-dict intent into a Rust [`Intent`] value.
///
/// # Errors
///
/// `ValueError` on schema violations (missing `kind`, wrong field type,
/// unknown intent kind).
pub fn parse_intent(
    raw: &Bound<'_, PyAny>,
    path: &str,
    damage_known: &mut bool,
) -> PyResult<Option<Intent>> {
    sts2_wire::state_dict::parse_intent(&PyWire(raw.clone()), path, damage_known).map_err(wire_err)
}

/// Return a sorted, deterministic list of canonical names for the
/// requested registry. The Python adapter calls this at import-time to
/// validate its `dict[str, str]` mapping tables against the Rust
/// source-of-truth (plan §Decisions row 32).
///
/// Accepted kinds: `"relic"`, `"relic_id"`, `"card"`, `"monster"`,
/// `"power"`, `"potion"`, `"event"`, `"boss"`. Unknown kinds fail loud
/// with `ValueError`.
///
/// # Errors
///
/// `ValueError` for an unknown registry kind.
pub fn list_registry_names_impl(kind: &str) -> PyResult<Vec<String>> {
    registry_names_for(kind).map_err(PyValueError::new_err)
}

// ---------------------------------------------------------------------------
// PyO3 wrappers
// ---------------------------------------------------------------------------

/// Validate a wire-format dict; return `None` on success or raise a
/// typed exception (`UnsupportedStateError` / `ValueError`) on failure.
///
/// T2 (`sts2-rs-osyf`) will replace this with `state_from_dict` that
/// returns a `PyCombatState` — at T1 we only certify the dict survives
/// the schema.
#[pyfunction]
pub fn parse_state_dict_py(d: &Bound<'_, PyDict>) -> PyResult<()> {
    parse_state_dict(d).map(|_| ())
}

/// Sorted list of canonical names for the requested registry kind.
#[pyfunction]
pub fn list_registry_names(kind: &str) -> PyResult<Vec<String>> {
    list_registry_names_impl(kind)
}

/// `(debug_form, python_name)` for every id of one of the four PPO
/// registry kinds (`"card"`, `"relic_id"`, `"potion"`, `"event"`).
///
/// The Debug form is the PascalCase enum-variant string `{:?}` dumps
/// emit (`master_deck_detailed`, inventory potion ids, …); the python
/// name is what `list_registry_names` / reward-option ids carry. One
/// entry per enum variant. Lets the GH#286 vocab round-trip test
/// enumerate the engine-emitted name universe without re-deriving it
/// from the registry it is checking.
#[pyfunction]
pub fn list_registry_id_forms(kind: &str) -> PyResult<Vec<(String, String)>> {
    sts2_runai::ppo::vocab::registry_id_forms(kind).ok_or_else(|| {
        PyValueError::new_err(format!(
            "unknown PPO registry kind `{kind}` (expected one of: card, relic_id, potion, event)"
        ))
    })
}

// ---------------------------------------------------------------------------
// state_from_dict — the PyDict entry point
// ---------------------------------------------------------------------------

/// Parse a wire-format dict and build a [`CoreCombatState`] from it.
/// Combines [`parse_state_dict`] + [`state_from_dict_impl`] into one
/// call.
///
/// # Errors
///
/// Returns the same error surface as [`parse_state_dict`] plus
/// `UnsupportedStateError` for any unresolvable runtime spec lookup
/// during state construction.
pub fn state_from_dict(d: &Bound<'_, PyDict>) -> PyResult<CoreCombatState> {
    sts2_wire::state_from_wire(&PyWire::from_dict(d)).map_err(wire_err)
}
// ---------------------------------------------------------------------------
// to_dict — CoreCombatState → wire dict (bead sts2-rs-wfjn, T3)
// ---------------------------------------------------------------------------
//
// Inverse of `state_from_dict`. The serializer itself lives in
// `sts2_wire::emit::combat` (one definition, shared with every non-Python
// consumer); what remains here is the `Value` → `PyDict` step. See that
// module for the round-trip contract and the list of intentionally-lossy
// fields.

/// Build the wire-format dict for `state`. The dict round-trips through
/// [`state_from_dict`] modulo the documented ignored fields (see
/// [`sts2_wire::emit::combat`]).
///
/// # Errors
///
/// * `ValueError` — pile contains a uid not in `state.cards`, or a
///   power uid not in `state.powers` (i.e. the state failed its own
///   `assert_invariants` before we got here).
pub fn state_to_dict<'py>(
    py: Python<'py>,
    state: &CoreCombatState,
) -> PyResult<Bound<'py, PyDict>> {
    let value = sts2_wire::emit::combat_state_to_value(state).map_err(crate::emit_err)?;
    crate::value_to_pydict(py, &value)
}

/// Test-only `PyDict` view of [`sts2_wire::emit::combat::intent_to_value`],
/// for the intent fixtures below that read the dict through `get_item`.
#[cfg(test)]
fn intent_to_pydict<'py>(
    py: Python<'py>,
    intent: Intent,
    state: &CoreCombatState,
    dealer: CreatureUid,
) -> PyResult<Bound<'py, PyDict>> {
    crate::value_to_pydict(
        py,
        &sts2_wire::emit::combat::intent_to_value(intent, state, dealer),
    )
}

/// `PyO3` wrapper: `sts2_rs.state_to_dict(state) -> dict`.
///
/// Inverse of [`crate::state_from_dict_py`]. Returns a dict that
/// survives [`parse_state_dict`] and round-trips through
/// [`state_from_dict`] modulo the documented ignored fields (RNG
/// cursor, `combat_log`, hook listener closures, intent history).
#[pyfunction]
#[pyo3(name = "state_to_dict")]
pub fn state_to_dict_py<'py>(
    py: Python<'py>,
    state: &crate::PyCombatState,
) -> PyResult<Bound<'py, PyDict>> {
    state_to_dict(py, state.inner_ref())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_to_dict_emits_osty_for_summoned_pet() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let mut state = CoreCombatState::default();
            let uid = state.summon_osty(12).expect("osty summoned");
            // Chip HP and add block to verify both fields are emitted (not
            // just the spawn defaults).
            let c = state
                .creatures
                .iter_mut()
                .find(|c| c.id == uid.0)
                .expect("pet creature present");
            c.current_hp = 9;
            c.block = 4;

            let d = state_to_dict(py, &state).expect("state_to_dict");
            let player_any = d.get_item("player").unwrap().unwrap();
            let player = player_any.downcast::<PyDict>().unwrap();
            let osty_any = player
                .get_item("osty")
                .unwrap()
                .expect("player.osty present when a pet is summoned");
            let osty = osty_any.downcast::<PyDict>().unwrap();
            assert!(osty
                .get_item("alive")
                .unwrap()
                .unwrap()
                .extract::<bool>()
                .unwrap());
            assert_eq!(
                osty.get_item("hp")
                    .unwrap()
                    .unwrap()
                    .extract::<i32>()
                    .unwrap(),
                9
            );
            assert_eq!(
                osty.get_item("max_hp")
                    .unwrap()
                    .unwrap()
                    .extract::<i32>()
                    .unwrap(),
                12
            );
            assert_eq!(
                osty.get_item("block")
                    .unwrap()
                    .unwrap()
                    .extract::<i32>()
                    .unwrap(),
                4
            );
        });
    }

    #[test]
    fn state_to_dict_omits_osty_without_pet() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let state = CoreCombatState::default();
            let d = state_to_dict(py, &state).expect("state_to_dict");
            let player_any = d.get_item("player").unwrap().unwrap();
            let player = player_any.downcast::<PyDict>().unwrap();
            assert!(
                player.get_item("osty").unwrap().is_none(),
                "no osty key when player.pet_uid is None"
            );
        });
    }

    /// A `ThievingHopper` mid-heist must say WHICH card it holds.
    ///
    /// The workbench pins the stolen card on the thief the way the game does
    /// (`ThievingHopper.cs` `%StolenCardPos`); without this key the display
    /// gets a bare "Swipe 1" and the card just vanishes from the deck view.
    /// A reclaimed entry (thief died, card given back) must NOT be emitted.
    #[test]
    fn state_to_dict_emits_stolen_cards_for_a_swipe_thief() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let mut state = CoreCombatState::default();
            let thief = NonZeroU32::new(1).expect("non-zero");
            let bystander = NonZeroU32::new(2).expect("non-zero");
            for (id, kind) in [
                (thief, MonsterKind::ThievingHopper),
                (bystander, MonsterKind::Nibbit),
            ] {
                state.creatures.push(Creature {
                    id,
                    current_hp: 40,
                    max_hp: 40,
                    block: 0,
                    powers: SmallVec::new(),
                    species: Some(kind),
                    intent: None,
                    intent_history: IntentHistory::default(),
                    monster_id: sts2_core::MonsterId("m"),
                    ai: None,
                });
                state.enemy_uids.push(CreatureUid(id));
            }
            state.record_swipe_steal(CreatureUid(thief), CardId::Bash, true);
            // A second steal that was already given back — omitted.
            state.record_swipe_steal(CreatureUid(thief), CardId::DefendIronclad, false);
            state.swipe_stolen[1].reclaimed = true;

            let d = state_to_dict(py, &state).expect("state_to_dict");
            let enc = d
                .get_item("encounter")
                .unwrap()
                .unwrap()
                .downcast_into::<PyList>()
                .unwrap();
            let stolen = enc
                .get_item(0)
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap()
                .get_item("stolen_cards")
                .unwrap()
                .expect("thief carries stolen_cards")
                .downcast_into::<PyList>()
                .unwrap();
            assert_eq!(stolen.len(), 1, "reclaimed steals are not emitted");
            let card = stolen
                .get_item(0)
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap();
            assert_eq!(
                card.get_item("id")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "BASH"
            );
            assert!(card
                .get_item("upgraded")
                .unwrap()
                .unwrap()
                .extract::<bool>()
                .unwrap());
            // The ledger is per-thief: the enemy standing next to it is clean.
            assert!(
                enc.get_item(1)
                    .unwrap()
                    .downcast_into::<PyDict>()
                    .unwrap()
                    .get_item("stolen_cards")
                    .unwrap()
                    .is_none(),
                "a non-thief must not carry the key at all"
            );
        });
    }

    #[test]
    fn state_to_dict_emits_orbs_and_slots_for_defect_belt() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            // A 3-slot belt with a Lightning orb and a Dark orb (with stored
            // damage) channelled; the third slot stays empty.
            let mut state = CoreCombatState::default();
            state.player.orbs = sts2_core::OrbSlots::with_default_capacity();
            state.player.orbs.slots[0] = Orb::Lightning;
            state.player.orbs.slots[1] = Orb::Dark { stored: 18 };

            let d = state_to_dict(py, &state).expect("state_to_dict");
            let player_any = d.get_item("player").unwrap().unwrap();
            let player = player_any.downcast::<PyDict>().unwrap();

            // Belt capacity is emitted as `orb_slots`.
            assert_eq!(
                player
                    .get_item("orb_slots")
                    .unwrap()
                    .expect("orb_slots present for a Defect belt")
                    .extract::<i32>()
                    .unwrap(),
                3
            );

            // Only the two *filled* slots are emitted (empty slot skipped).
            let orbs_any = player
                .get_item("orbs")
                .unwrap()
                .expect("player.orbs present");
            let orbs = orbs_any.downcast::<PyList>().unwrap();
            assert_eq!(orbs.len(), 2, "only channelled orbs are emitted");

            let lightning = orbs.get_item(0).unwrap();
            let lightning = lightning.downcast::<PyDict>().unwrap();
            assert_eq!(
                lightning
                    .get_item("type")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "Lightning"
            );
            assert_eq!(
                lightning
                    .get_item("name")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "LIGHTNING_ORB.title"
            );
            // Focus 0: passive 3, evoke 8.
            assert_eq!(
                lightning
                    .get_item("passive")
                    .unwrap()
                    .unwrap()
                    .extract::<i32>()
                    .unwrap(),
                3
            );
            assert_eq!(
                lightning
                    .get_item("evoke")
                    .unwrap()
                    .unwrap()
                    .extract::<i32>()
                    .unwrap(),
                8
            );

            let dark = orbs.get_item(1).unwrap();
            let dark = dark.downcast::<PyDict>().unwrap();
            assert_eq!(
                dark.get_item("type")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "Dark"
            );
            // Dark passive shows the per-turn increment (6); evoke is the C#
            // `EvokeVal`, i.e. the accumulated 18 PLUS the 6 a fresh Dark orb
            // already carries (`DarkOrb.cs:17`). Emitting the raw 18 put every
            // mirror Dark orb 6 low against the live game.
            assert_eq!(
                dark.get_item("passive")
                    .unwrap()
                    .unwrap()
                    .extract::<i32>()
                    .unwrap(),
                6
            );
            assert_eq!(
                dark.get_item("evoke")
                    .unwrap()
                    .unwrap()
                    .extract::<i32>()
                    .unwrap(),
                24
            );
        });
    }

    #[test]
    fn dark_orb_evoke_is_the_csharp_evoke_val_on_both_wire_legs() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            // A FRESH Dark orb already evokes for 6 in C# (`DarkOrb.cs:17`),
            // while `Orb::Dark`'s `stored` — the accumulated passive growth —
            // is 0. Emit must add the base and parse must take it off; when
            // only one leg converts, the belt drifts by 6 per orb per hop.
            let mut state = CoreCombatState::default();
            state.player.orbs = sts2_core::OrbSlots::with_default_capacity();
            state.player.orbs.slots[0] = Orb::dark();
            state.player.orbs.slots[1] = Orb::Dark { stored: 12 };

            let d = state_to_dict(py, &state).expect("state_to_dict");
            let player_any = d.get_item("player").unwrap().unwrap();
            let player = player_any.downcast::<PyDict>().unwrap();
            let orbs_any = player.get_item("orbs").unwrap().unwrap();
            let orbs = orbs_any.downcast::<PyList>().unwrap();
            let evoke_at = |i: usize| {
                let orb = orbs.get_item(i).unwrap();
                orb.downcast::<PyDict>()
                    .unwrap()
                    .get_item("evoke")
                    .unwrap()
                    .unwrap()
                    .extract::<i32>()
                    .unwrap()
            };
            assert_eq!(evoke_at(0), 6, "a fresh Dark orb evokes for 6, not 0");
            assert_eq!(evoke_at(1), 18);

            // Round trip: the belt that comes back must be the belt that left.
            let back = state_from_dict(&d).expect("state_from_dict");
            assert_eq!(back.player.orbs.slots[0], Orb::Dark { stored: 0 });
            assert_eq!(back.player.orbs.slots[1], Orb::Dark { stored: 12 });
        });
    }

    /// GH #368.2 — the Glass round trip UNDER FOCUS, which is the only place
    /// the asymmetry shows.
    ///
    /// Both wire legs carry `PassiveVal` (Focus-scaled, C#
    /// `OrbModel.PassiveVal => ModifyOrbValue(_passiveVal)`), but the engine's
    /// `Orb::Glass { current_passive }` is the RAW `_passiveVal` — the thing
    /// `GlassOrb.Passive` decays by 1 per firing tick. The parse leg used to
    /// store the scaled value verbatim, so a focused belt came back hot and,
    /// worse, needed `focus` extra ticks to decay to 0.
    ///
    /// A Focus-0 fixture cannot see this: the scale is the identity there, and
    /// emit-then-parse agrees with itself either way. Every case below
    /// therefore has non-zero Focus.
    #[test]
    fn glass_orb_passive_round_trips_under_focus() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            for (focus, raw, want_wire) in [(2, 4, 6), (1, 3, 4), (-1, 4, 3), (-3, 2, 0)] {
                let mut state = CoreCombatState::default();
                state.player.orbs = sts2_core::OrbSlots::with_default_capacity();
                state.player.orbs.slots[0] = Orb::Glass {
                    current_passive: raw,
                };
                let pid = NonZeroU32::new(1).expect("non-zero");
                state.powers.push(Power {
                    id: pid,
                    kind: PowerKind::Focus,
                    amount: focus,
                    counter: 0,
                    applier: None,
                    skip_next_tick: false,
                });
                state.player.powers.push(PowerUid(pid));

                let d = state_to_dict(py, &state).expect("state_to_dict");
                let player_any = d.get_item("player").unwrap().unwrap();
                let player = player_any.downcast::<PyDict>().unwrap();
                let orbs_any = player.get_item("orbs").unwrap().unwrap();
                let orbs = orbs_any.downcast::<PyList>().unwrap();
                let orb = orbs.get_item(0).unwrap();
                let wire_passive: i32 = orb
                    .downcast::<PyDict>()
                    .unwrap()
                    .get_item("passive")
                    .unwrap()
                    .unwrap()
                    .extract()
                    .unwrap();
                assert_eq!(
                    wire_passive, want_wire,
                    "wire carries the Focus-scaled PassiveVal (raw {raw}, focus {focus})"
                );

                let back = state_from_dict(&d).expect("state_from_dict");
                // Exact whenever the scaled passive stayed positive. Once it
                // clamped to 0 the raw is genuinely unrecoverable — the wire
                // says only `raw <= -focus` — and the natural inverse returns
                // that upper bound. Both endpoints are wrong by at most
                // `|focus|`, and the upper bound is the safer error: the orb
                // is inert either way while Focus stays negative, but taking 0
                // would delete its remaining life for good if Focus recovers.
                let want_raw = if want_wire == 0 { -focus } else { raw };
                assert_eq!(
                    back.player.orbs.slots[0],
                    Orb::Glass {
                        current_passive: want_raw
                    },
                    "parse takes Focus back off (raw {raw}, focus {focus})"
                );
            }
        });
    }

    #[test]
    fn state_to_dict_omits_orbs_without_belt() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            // Default state has a zero-capacity belt (non-Defect): no orb keys.
            let state = CoreCombatState::default();
            let d = state_to_dict(py, &state).expect("state_to_dict");
            let player_any = d.get_item("player").unwrap().unwrap();
            let player = player_any.downcast::<PyDict>().unwrap();
            assert!(
                player.get_item("orbs").unwrap().is_none(),
                "no orbs key when the belt has zero capacity"
            );
            assert!(
                player.get_item("orb_slots").unwrap().is_none(),
                "no orb_slots key when the belt has zero capacity"
            );
        });
    }

    /// The C# dialect carries Tender's cards-played count as `display`
    /// (`TenderPower.DisplayAmount => CardsPlayedThisTurn`) and sends no
    /// `counter`. Rebuilding it at 0 makes the turn-end restore a no-op, so
    /// the 1 Strength + 1 Dexterity Tender drains per card played never comes
    /// back and the mirror's player drifts permanently weaker — 13 of the 43
    /// single-field `$.player.powers` records in the v17 corpus.
    /// C# `SlowPower.DisplayAmount => SlowAmount * 10` (`SlowPower.cs:22`), and
    /// `SlowAmount` is what drives `1 + 0.1 * SlowAmount` extra damage taken
    /// (`SlowPower.cs:32-43`). Rebuilding it at 0 made the mirror apply a
    /// weaker multiplier than the game and therefore sit HP-HIGH on every Slow
    /// enemy — BygoneEffigy alone was 135 of the 250 single-field
    /// `$.encounter[].hp` records in v18, the largest class in that corpus.
    #[test]
    fn enemy_slow_amount_survives_a_counterless_wire() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let mut state = CoreCombatState::default();
            let cid = NonZeroU32::new(1).expect("non-zero");
            let pid = NonZeroU32::new(1).expect("non-zero");
            state.powers.push(Power {
                id: pid,
                kind: PowerKind::Slow,
                amount: 1,
                counter: 3,
                applier: None,
                skip_next_tick: false,
            });
            let mut cpowers: SmallVec<[PowerUid; 8]> = SmallVec::new();
            cpowers.push(PowerUid(pid));
            state.creatures.push(Creature {
                id: cid,
                current_hp: 40,
                max_hp: 40,
                block: 0,
                powers: cpowers,
                species: Some(MonsterKind::Nibbit),
                intent: None,
                intent_history: IntentHistory::default(),
                monster_id: sts2_core::MonsterId("Nibbit"),
                ai: None,
            });
            state.enemy_uids.push(CreatureUid(cid));

            let d = state_to_dict(py, &state).expect("state_to_dict");
            // Re-shape into the C# dialect: no `counter`, SlowAmount x10 in
            // `display`.
            let p0 = d
                .get_item("encounter")
                .unwrap()
                .unwrap()
                .downcast_into::<PyList>()
                .unwrap()
                .get_item(0)
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap()
                .get_item("powers")
                .unwrap()
                .unwrap()
                .downcast_into::<PyList>()
                .unwrap()
                .get_item(0)
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap();
            p0.del_item("counter").unwrap();
            p0.set_item("display", 30).unwrap();

            let rebuilt = state_from_dict(&d).expect("slow state must rebuild");
            let slow = rebuilt
                .powers
                .iter()
                .find(|p| p.kind == PowerKind::Slow)
                .expect("slow present");
            assert_eq!(
                slow.counter, 3,
                "display is SlowAmount x10; rebuilding at 0 under-damages every \
                 Slow enemy for the rest of the turn"
            );
        });
    }

    /// Rebuild a lone PLAYER power from a C#-dialect wire — `display` present,
    /// `counter` absent — and return the counter the parser inferred.
    fn player_counter_from_display(kind: PowerKind, amount: i32, display: i32) -> i32 {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let mut state = CoreCombatState::default();
            let pid = NonZeroU32::new(1).expect("non-zero");
            state.powers.push(Power {
                id: pid,
                kind,
                amount,
                counter: 0,
                applier: None,
                skip_next_tick: false,
            });
            state.player.powers.push(PowerUid(pid));
            let d = state_to_dict(py, &state).expect("state_to_dict");
            let p0 = d
                .get_item("player")
                .unwrap()
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap()
                .get_item("powers")
                .unwrap()
                .unwrap()
                .downcast_into::<PyList>()
                .unwrap()
                .get_item(0)
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap();
            p0.del_item("counter").unwrap();
            p0.set_item("display", display).unwrap();
            state_from_dict(&d)
                .expect("state must rebuild")
                .powers
                .iter()
                .find(|p| p.kind == kind)
                .expect("power present")
                .counter
        })
    }

    /// C# `PanachePower.DisplayAmount => CardsLeft` and the engine's `counter`
    /// IS CardsLeft. Rebuilding at 0 does not merely lose progress: the
    /// decrement takes 0 -> -1, which trips the `<= 0` fire test, so a
    /// reconstructed Panache detonates on the very next card played.
    #[test]
    fn panache_cards_left_survives_a_counterless_wire() {
        assert_eq!(
            player_counter_from_display(PowerKind::Panache, 10, 3),
            3,
            "CardsLeft rides `display` verbatim"
        );
    }

    /// C# `AutomationPower.DisplayAmount => cardsLeft` counts DOWN from 10
    /// while the engine counts the same cards UP from 0, so the wire value is
    /// the complement. Cards DRAWN, not played — both tick on the
    /// after-card-drawn hook. `amount` is the energy granted, not the cycle
    /// length.
    #[test]
    fn automation_progress_survives_a_counterless_wire() {
        assert_eq!(
            player_counter_from_display(PowerKind::Automation, 1, 4),
            6,
            "6 cards played of the 10-card cycle when 4 are left"
        );
        assert_eq!(
            player_counter_from_display(PowerKind::Automation, 1, 10),
            0,
            "a fresh Automation has played none"
        );
    }

    /// GH #363 A1. C# `OrbitPower` hides `energySpent` and publishes only
    /// `DisplayAmount => 4 - energySpent % 4` (`OrbitPower.cs:25`) — the energy
    /// still owed before the next grant, so a fresh Orbit shows 4 and not 0.
    /// The engine's `counter` is cumulative energy spent, and only its residue
    /// mod 4 matters: `prev_triggers = counter / 4` cancels exactly, so
    /// rebuilding at `energySpent % 4` reproduces every future trigger.
    ///
    /// Rebuilding at 0 instead claimed a full 4 energy was owed regardless of
    /// phase — 30 of the 38 fully-explained `$.player.energy` records on the
    /// 2026-08-03 live Regent A1 corpus (22 mirror-low, 8 mirror-high).
    #[test]
    fn orbit_energy_residue_survives_a_counterless_wire() {
        assert_eq!(
            player_counter_from_display(PowerKind::Orbit, 1, 4),
            0,
            "display 4 ⇒ nothing spent toward the next grant"
        );
        assert_eq!(
            player_counter_from_display(PowerKind::Orbit, 1, 3),
            1,
            "display 3 ⇒ 1 energy into the cycle"
        );
        assert_eq!(
            player_counter_from_display(PowerKind::Orbit, 1, 1),
            3,
            "display 1 ⇒ 3 spent, one more energy fires the grant"
        );
    }

    /// The residue is all the trigger arithmetic needs: a counter rebuilt at
    /// `energySpent % 4` fires on exactly the same spends as the true total.
    /// C# computes `energySpent/4 - triggerCount` where `triggerCount` is
    /// always the previous `energySpent/4`; with `energySpent = 4q + r` that is
    /// `(4q + r + spend)/4 - q == (r + spend)/4`, independent of `q`.
    #[test]
    fn orbit_residue_fires_on_the_same_spends_as_the_full_total() {
        for r in 0..4 {
            for spend in 1..=8 {
                let from_residue =
                    sts2_core::powers::regent::orbit_triggers_on_energy_spent(r + spend, r / 4);
                for q in 0..5 {
                    let total = 4 * q + r;
                    let from_full = sts2_core::powers::regent::orbit_triggers_on_energy_spent(
                        total + spend,
                        total / 4,
                    );
                    assert_eq!(
                        from_residue, from_full,
                        "residue {r} must fire like total {total} on a {spend}-energy spend"
                    );
                }
            }
        }
    }

    /// The emit leg is the mirror image: `state_to_dict` must publish Orbit's
    /// `display_amount` as C# does, or the mirror's own wire disagrees with the
    /// world's on a field the diff compares. It previously emitted `amount` —
    /// the energy the power GRANTS, an unrelated number.
    #[test]
    fn orbit_display_amount_is_emitted_as_the_energy_still_owed() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            for (counter, want) in [(0, 4), (1, 3), (3, 1), (4, 4), (7, 1)] {
                let mut state = CoreCombatState::default();
                let pid = NonZeroU32::new(1).expect("non-zero");
                state.powers.push(Power {
                    id: pid,
                    kind: PowerKind::Orbit,
                    amount: 1,
                    counter,
                    applier: None,
                    skip_next_tick: false,
                });
                state.player.powers.push(PowerUid(pid));
                let d = state_to_dict(py, &state).expect("state_to_dict");
                let got: i32 = d
                    .get_item("player")
                    .unwrap()
                    .unwrap()
                    .downcast_into::<PyDict>()
                    .unwrap()
                    .get_item("powers")
                    .unwrap()
                    .unwrap()
                    .downcast_into::<PyList>()
                    .unwrap()
                    .get_item(0)
                    .unwrap()
                    .downcast_into::<PyDict>()
                    .unwrap()
                    .get_item("display_amount")
                    .unwrap()
                    .unwrap()
                    .extract()
                    .unwrap();
                assert_eq!(
                    got, want,
                    "counter {counter} ⇒ C# DisplayAmount {want} (4 - {counter} % 4)"
                );
            }
        });
    }

    /// GH #370. Sloth gates card LEGALITY, so a count rebuilt at 0 does not
    /// mis-damage — it hands the search actions the world will refuse.
    #[test]
    fn sloth_restores_the_started_play_count_from_its_display() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let mut state = CoreCombatState::default();
            let pid = NonZeroU32::new(1).expect("non-zero");
            state.powers.push(Power {
                id: pid,
                kind: PowerKind::Sloth,
                // C# `Amount` is the THRESHOLD: 2 cards a turn.
                amount: 2,
                counter: 0,
                applier: None,
                skip_next_tick: false,
            });
            state.player.powers.push(PowerUid(pid));
            state.player.card_plays_started_this_turn = 2;

            let d = state_to_dict(py, &state).expect("state_to_dict");
            let p0 = d
                .get_item("player")
                .unwrap()
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap()
                .get_item("powers")
                .unwrap()
                .unwrap()
                .downcast_into::<PyList>()
                .unwrap()
                .get_item(0)
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap();
            // The C# dialect: `amount` is the threshold, `display` the count.
            p0.set_item("display", 2).unwrap();

            let rebuilt = state_from_dict(&d).expect("sloth state must rebuild");
            assert_eq!(
                rebuilt.player.card_plays_started_this_turn, 2,
                "`SlothPower.DisplayAmount` is the live `_cardsPlayedThisTurn`; \
                 rebuilding it at 0 re-opens a spent Sloth for the rest of the turn"
            );
            let sloth = rebuilt
                .powers
                .iter()
                .find(|p| p.kind == PowerKind::Sloth)
                .expect("sloth present");
            assert_eq!(
                sloth.amount, 2,
                "`amount` stays the threshold — the count must NOT overwrite it"
            );
        });
    }

    /// GH #370. The three Sly/Retain uid lists ride the card FLAGS, and the
    /// emit and the parse have to agree — a one-sided emit would show up as a
    /// per-decision flags desync on every affected card rather than as a lost
    /// gate. Round-trips a hand holding one of each transient grant.
    /// GH #372: the per-combat Ethereal grant must survive the round trip.
    ///
    /// `state_from_dict` maps the wire's `ethereal` flag onto
    /// `VarKey::EtherealThisCombat`, but the emit used to read only
    /// `card_ethereal_inst` (keyword_override / static registry) — so a
    /// GhostSeed run's basic Strike/Defend arrived Ethereal and came back out
    /// with no flag. `_card_key` diffs the flag tuple on all five piles, so
    /// that was a per-decision desync on every affected card for the whole
    /// fight.
    #[test]
    fn the_per_combat_ethereal_grant_survives_a_wire_round_trip() {
        let mut state = state_with_hand(&[CardId::DefendIronclad, CardId::StrikeIronclad]);
        // GhostSeed's stamp (`relics/shop_event.rs:1964`) on the Defend only.
        state.cards[0].set_combat_var(VarKey::EtherealThisCombat, 1);

        let flags_of = |d: &pyo3::Bound<'_, pyo3::types::PyDict>, i: usize| -> Vec<String> {
            let hand = d
                .get_item("deck")
                .unwrap()
                .unwrap()
                .downcast_into::<pyo3::types::PyDict>()
                .unwrap()
                .get_item("hand")
                .unwrap()
                .unwrap();
            let card = hand
                .downcast::<pyo3::types::PyList>()
                .unwrap()
                .get_item(i)
                .unwrap();
            card.downcast::<pyo3::types::PyDict>()
                .unwrap()
                .get_item("flags")
                .unwrap()
                .map_or_else(Vec::new, |f| f.extract::<Vec<String>>().unwrap())
        };

        Python::with_gil(|py| {
            let out = state_to_dict(py, &state).unwrap();
            assert!(
                flags_of(&out, 0).contains(&"ethereal".to_string()),
                "the granted card must emit the flag it was parsed from",
            );
            assert!(
                !flags_of(&out, 1).contains(&"ethereal".to_string()),
                "an ungranted basic must stay clean",
            );

            // Full round trip: wire -> state -> wire must be stable, which is
            // the property the mirror's `_card_key` actually depends on.
            let rebuilt = state_from_dict(&out).unwrap();
            let again = state_to_dict(py, &rebuilt).unwrap();
            assert_eq!(
                flags_of(&again, 0),
                flags_of(&out, 0),
                "a second trip must not change the flag tuple",
            );
        });
    }

    fn state_with_hand(ids: &[CardId]) -> CoreCombatState {
        let mut state = CoreCombatState::default();
        for (i, &card_id) in ids.iter().enumerate() {
            let uid = CardUid::new(u32::try_from(i).unwrap() + 1).expect("non-zero");
            state.cards.push(sts2_core::cards::Card {
                uid,
                card_id,
                ..sts2_core::cards::Card::pod_defaults()
            });
            state.player.hand.push(uid);
        }
        state
    }

    #[test]
    fn single_turn_sly_and_retain_round_trip_through_the_card_flags() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            // Three distinct instances so each grant is attributed separately.
            let mut state =
                state_with_hand(&[CardId::StrikeIronclad, CardId::DefendIronclad, CardId::Bash]);
            let (hand_trick, master_planner, well_laid_plans) = (
                state.player.hand[0],
                state.player.hand[1],
                state.player.hand[2],
            );
            state.player.sly_this_turn.push(hand_trick);
            state.player.sly_this_combat.push(master_planner);
            state.player.retain_this_turn.push(well_laid_plans);

            let d = state_to_dict(py, &state).expect("state_to_dict");
            let rebuilt = state_from_dict(&d).expect("state must rebuild");

            // Uids are re-allocated by the pile walk, so compare by POSITION.
            assert_eq!(
                rebuilt.player.sly_this_turn.as_slice(),
                &[rebuilt.player.hand[0]],
                "Hand Trick's single-turn Sly must survive as a per-TURN grant"
            );
            assert_eq!(
                rebuilt.player.sly_this_combat.as_slice(),
                &[rebuilt.player.hand[1]],
                "MasterPlanner's keyword Sly must survive as a per-COMBAT grant — \
                 the two lifetimes are not interchangeable"
            );
            assert_eq!(
                rebuilt.player.retain_this_turn.as_slice(),
                &[rebuilt.player.hand[2]],
                "Well-Laid Plans' single-turn Retain must survive, or the card \
                 flushes at end of turn while the world keeps it"
            );
        });
    }

    /// The keyword half must NOT leak into the per-turn list: `IsSlyThisTurn`
    /// short-circuits on the keyword in C#, so an intrinsically-Sly card
    /// carries no transient grant to restore.
    #[test]
    fn an_intrinsically_sly_card_seeds_no_transient_grant() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            // Registered Sly by the content registry, not by a uid list.
            sts2_core::card_effects::register_card_sly(CardId::Tactician);
            let state = state_with_hand(&[CardId::Tactician]);

            let d = state_to_dict(py, &state).expect("state_to_dict");
            let rebuilt = state_from_dict(&d).expect("state must rebuild");

            assert!(
                rebuilt.player.sly_this_turn.is_empty(),
                "an intrinsic Sly is registry knowledge, not a this-turn grant"
            );
            assert!(
                rebuilt.player.sly_this_combat.is_empty(),
                "nor a combat-long one — only `AddKeyword` produces those"
            );
            assert!(
                sts2_core::card_effects::card_is_sly(&rebuilt, rebuilt.player.hand[0]),
                "it is still Sly, via the registry"
            );
        });
    }

    #[test]
    fn tender_cards_played_survives_a_counterless_wire() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let mut state = CoreCombatState::default();
            let pid = NonZeroU32::new(1).expect("non-zero");
            state.powers.push(Power {
                id: pid,
                kind: PowerKind::Tender,
                amount: 1,
                counter: 3,
                applier: None,
                skip_next_tick: false,
            });
            state.player.powers.push(PowerUid(pid));

            let d = state_to_dict(py, &state).expect("state_to_dict");
            // Re-shape into the C# dialect: no `counter`, count in `display`.
            let powers = d
                .get_item("player")
                .unwrap()
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap()
                .get_item("powers")
                .unwrap()
                .unwrap()
                .downcast_into::<PyList>()
                .unwrap();
            let p0 = powers
                .get_item(0)
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap();
            p0.del_item("counter").unwrap();
            p0.set_item("display", 3).unwrap();

            let rebuilt = state_from_dict(&d).expect("tender state must rebuild");
            let tender = rebuilt
                .powers
                .iter()
                .find(|p| p.kind == PowerKind::Tender)
                .expect("tender present");
            assert_eq!(
                tender.counter, 3,
                "`display` is the only carrier for cards-played; \
                 rebuilding at 0 silently disables the turn-end restore"
            );
        });
    }

    /// The Necrobinder's Osty must survive a round trip. It was parsed and
    /// then dropped, so a rebuilt state had `pet_uid == None` and no pet at
    /// all — BoundPhylactery re-summoned a 1-HP one next turn and the player
    /// absorbed every hit the real Osty should have taken (127 oracle records,
    /// mirror HP always low).
    #[test]
    fn osty_survives_a_round_trip() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let mut state = CoreCombatState::default();
            state.player.max_hp = 60;
            state.player.current_hp = 60;
            let uid = state.summon_osty(11).expect("osty summoned");
            if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
                c.current_hp = 5;
                c.block = 2;
            }

            let d = state_to_dict(py, &state).expect("state_to_dict");
            let rebuilt = state_from_dict(&d).expect("osty state must round-trip");

            let pet = rebuilt
                .player
                .pet_uid
                .expect("a rebuilt state keeps its pet");
            let c = rebuilt
                .creatures
                .iter()
                .find(|c| c.id == pet.0)
                .expect("pet creature present");
            assert_eq!((c.current_hp, c.max_hp, c.block), (5, 11, 2));
            assert!(
                rebuilt
                    .powers
                    .iter()
                    .any(|p| p.kind == PowerKind::DieForYou),
                "the redirect link must be re-established, or the pet absorbs nothing"
            );
        });
    }

    #[test]
    fn orbs_round_trip_despite_mixed_case_emission() {
        // Regression (rung-3-PPO seed 1019, 2026-07-12): `state_to_dict`
        // emits the C#-parity mixed-case orb names ("Plasma") while the
        // parser's canonical names are UPPER ("PLASMA"), so any state with
        // a channelled orb failed its own round-trip with
        // `$.player.orbs[0]: Plasma`. The parser is now case-insensitive.
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let mut state = CoreCombatState::default();
            state.player.orbs = sts2_core::OrbSlots::with_default_capacity();
            state.player.orbs.slots[0] = Orb::Plasma;
            state.player.orbs.slots[1] = Orb::Lightning;

            let d = state_to_dict(py, &state).expect("state_to_dict");
            let rebuilt = state_from_dict(&d).expect("orb state must round-trip");
            let kinds: Vec<OrbKind> = rebuilt.player.orbs.slots.iter().map(OrbKind::of).collect();
            assert_eq!(&kinds[..2], &[OrbKind::Plasma, OrbKind::Lightning]);
        });
    }

    #[test]
    fn enchantment_survives_the_round_trip() {
        // 2026-08-01, DLL mirror seed 1000: the world blocked 7 off a Nimble
        // Defend, the mirror 5, every decision after that planned on the wrong
        // board. `enchantment` was parsed and then thrown away by `build_card`,
        // so no reconstructed card was ever enchanted. Both legs are pinned
        // here — the stamp on the way in, the field on the way out — because
        // dropping either one puts the bug straight back.
        use pyo3::types::PyList;
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let state = CoreCombatState::default();
            let d = state_to_dict(py, &state).expect("state_to_dict");
            let deck = d
                .get_item("deck")
                .unwrap()
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap();

            let card = PyDict::new(py);
            card.set_item("id", "DEFEND_IRONCLAD").unwrap();
            card.set_item("stable_idx", 1).unwrap();
            card.set_item("kind", "card").unwrap();
            card.set_item("upgraded", false).unwrap();
            card.set_item("base_block", 5).unwrap();
            card.set_item("enchantment", "Nimble").unwrap();
            card.set_item("enchantment_amount", 2).unwrap();
            deck.set_item("hand", PyList::new(py, [&card]).unwrap())
                .unwrap();

            let rebuilt = state_from_dict(&d).expect("enchanted card must round-trip");
            let c = rebuilt
                .cards
                .iter()
                .find(|c| c.card_id == CardId::DefendIronclad)
                .expect("defend present");
            assert_eq!(
                c.enchantment(EnchantmentId::Nimble),
                2,
                "wire enchantment + amount must land on the reconstructed card"
            );
            // The bonus is the hook's to add at play time; the wire's
            // `base_block` is the pre-enchantment base and must stay put, or
            // the +2 lands twice.
            assert_eq!(c.base_block, 5);

            // ... and back out again, in the C# producers' spelling.
            let out = state_to_dict(py, &rebuilt).expect("state_to_dict");
            let out_hand = out
                .get_item("deck")
                .unwrap()
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap()
                .get_item("hand")
                .unwrap()
                .unwrap();
            let emitted = out_hand
                .downcast_into::<PyList>()
                .unwrap()
                .get_item(0)
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap();
            assert_eq!(
                emitted
                    .get_item("enchantment")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "Nimble"
            );
            assert_eq!(
                emitted
                    .get_item("enchantment_amount")
                    .unwrap()
                    .unwrap()
                    .extract::<i32>()
                    .unwrap(),
                2
            );

            // An amount-less enchantment (C# omits `Amount` when it is 0)
            // reconstructs as ONE stack — `has_enchantment` is `> 0`, so a
            // literal 0 would read back as "not enchanted".
            card.del_item("enchantment_amount").unwrap();
            card.set_item("enchantment", "Steady").unwrap();
            deck.set_item("hand", PyList::new(py, [&card]).unwrap())
                .unwrap();
            let rebuilt2 = state_from_dict(&d).expect("round-trip without an amount");
            let c2 = rebuilt2
                .cards
                .iter()
                .find(|c| c.card_id == CardId::DefendIronclad)
                .expect("defend present (control)");
            assert!(c2.has_enchantment(EnchantmentId::Steady));

            // An unknown spelling fails loud rather than reconstructing a
            // plain card that silently plans weaker than the real one.
            card.set_item("enchantment", "NotAnEnchantment").unwrap();
            deck.set_item("hand", PyList::new(py, [&card]).unwrap())
                .unwrap();
            assert!(state_from_dict(&d).is_err());
        });
    }

    #[test]
    fn upgraded_card_base_stats_survive_round_trip() {
        // csview recon audit (2026-07-15): an upgraded/enchanted card carries
        // its baked base stats in `base_damage`/`base_block`. `state_to_dict`
        // emits them, but the parser used to ignore them and rebuild from the
        // UNUPGRADED registry spec (`resolve_card_name` returns the base
        // variant), so an upgraded Strike (9) reconstructed as 6 -> every
        // reconstruction path (live bridge, headless, native round-trip)
        // planned the whole deck weaker than it is. The wire value must win.
        use pyo3::types::PyList;
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let state = CoreCombatState::default();
            let d = state_to_dict(py, &state).expect("state_to_dict");
            let deck = d
                .get_item("deck")
                .unwrap()
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap();

            let card = PyDict::new(py);
            card.set_item("id", "STRIKE_IRONCLAD").unwrap();
            card.set_item("stable_idx", 1).unwrap();
            card.set_item("kind", "card").unwrap();
            card.set_item("upgraded", true).unwrap();
            card.set_item("base_damage", 9).unwrap();
            card.set_item("base_block", 0).unwrap();
            let hand = PyList::new(py, [&card]).unwrap();
            deck.set_item("hand", &hand).unwrap();

            let rebuilt = state_from_dict(&d).expect("upgraded card must round-trip");
            let c = rebuilt
                .cards
                .iter()
                .find(|c| c.card_id == CardId::StrikeIronclad)
                .expect("strike present");
            assert_eq!(
                c.base_damage, 9,
                "wire base_damage must override the unupgraded registry spec"
            );
            assert!(c.upgraded);

            // An `upgraded: true` card from a STATS-LESS wire (the live
            // bridge dialect has no `stats` block) falls back to the
            // factory's UPGRADED build, not the unupgraded registry spec —
            // otherwise every upgraded card on the live path planned at base
            // values (mirror_live2 `$.player.block`: world exactly +3 over
            // the mirror on 14/17 records — the Defend/Colossus upgrade
            // delta).
            card.del_item("base_damage").unwrap();
            card.del_item("base_block").unwrap();
            let hand2 = PyList::new(py, [&card]).unwrap();
            deck.set_item("hand", &hand2).unwrap();
            let rebuilt2 = state_from_dict(&d).expect("round-trip without base stats");
            let c2 = rebuilt2
                .cards
                .iter()
                .find(|c| c.card_id == CardId::StrikeIronclad)
                .expect("strike present (control)");
            assert_eq!(
                c2.base_damage, 9,
                "absent base stats on an upgraded card adopt the upgraded factory build"
            );

            // An UNUPGRADED stats-less card keeps the registry default.
            card.set_item("upgraded", false).unwrap();
            let hand3 = PyList::new(py, [&card]).unwrap();
            deck.set_item("hand", &hand3).unwrap();
            let rebuilt3 = state_from_dict(&d).expect("round-trip unupgraded without base stats");
            let c3 = rebuilt3
                .cards
                .iter()
                .find(|c| c.card_id == CardId::StrikeIronclad)
                .expect("strike present (unupgraded control)");
            assert_eq!(
                c3.base_damage, 6,
                "unupgraded stats-less card keeps the registry default"
            );
        });
    }

    /// MadScience's `CardType` is PER INSTANCE: C#
    /// `../sts2-wiki/decompiled/v0.107.1/MegaCrit.Sts2.Core.Models.Cards/MadScience.cs:80`
    /// `Type => TinkerTimeType`, `:82-92` `TargetType => TinkerTimeType !=
    /// Attack ? Self : AnyEnemy`, and `OnPlay` (`:189-214`) branches on it.
    /// The registry spec is the default-constructed Attack build
    /// (`MadScience.cs:167-170`), so a reconstruction that ignores the wire's
    /// `type` plays the WRONG BRANCH — the live corpus showed the mirror
    /// dealing 7/18/27/21/28 damage where the world applied CURIOUS 1 /
    /// IMPROVEMENT 1 and dealt 0.
    ///
    /// The rider is a separate `[SavedProperty]` (`MadScience.cs:125-137`) no
    /// live producer sends yet; the parser accepts it under
    /// `tinker_time_rider` so the mod only has to start emitting the name.
    #[test]
    fn mad_science_variant_seeds_from_the_wire_type_and_rider() {
        use pyo3::types::PyList;
        use sts2_content::cards::colorless::{mad_science_variant_of, TinkerTimeRider};
        use sts2_core::cards::{CardType, TargetType};
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let rebuild = |ty: Option<&str>, rider: Option<&str>| {
                let state = CoreCombatState::default();
                let d = state_to_dict(py, &state).expect("state_to_dict");
                let deck = d
                    .get_item("deck")
                    .unwrap()
                    .unwrap()
                    .downcast_into::<PyDict>()
                    .unwrap();
                let card = PyDict::new(py);
                card.set_item("id", "MAD_SCIENCE").unwrap();
                card.set_item("stable_idx", 0).unwrap();
                card.set_item("kind", "card").unwrap();
                card.set_item("upgraded", false).unwrap();
                if let Some(ty) = ty {
                    card.set_item("type", ty).unwrap();
                }
                if let Some(r) = rider {
                    card.set_item("tinker_time_rider", r).unwrap();
                }
                let hand = PyList::new(py, [&card]).unwrap();
                deck.set_item("hand", &hand).unwrap();
                let rebuilt = state_from_dict(&d).expect("MadScience must round-trip");
                *rebuilt
                    .cards
                    .iter()
                    .find(|c| c.card_id == CardId::MadScience)
                    .expect("mad science present")
            };

            // No `type` at all (the pre-fix live dialect): the C# default —
            // Attack, enemy-targeted, no rider.
            let plain = rebuild(None, None);
            assert_eq!(plain.card_type, CardType::Attack);
            assert_eq!(plain.target_type, TargetType::Enemy);
            assert_eq!(
                mad_science_variant_of(&plain),
                (CardType::Attack, TinkerTimeRider::None)
            );

            // `type: "Power"` — the branch that applies the rider. TargetType
            // flips to Self, and the card is now visible to every
            // card-type-gated surface (CuriousPower's Power-card discount).
            let power = rebuild(Some("Power"), None);
            assert_eq!(power.card_type, CardType::Power);
            assert_eq!(power.target_type, TargetType::Self_);
            assert_eq!(
                mad_science_variant_of(&power),
                (CardType::Power, TinkerTimeRider::None)
            );

            // `type: "Skill"` (case-insensitive) — Self-targeted, gains block.
            let skill = rebuild(Some("skill"), None);
            assert_eq!(skill.card_type, CardType::Skill);
            assert_eq!(skill.target_type, TargetType::Self_);

            // With the rider the mod does not emit yet, the Power arm resolves
            // fully — this is the only piece still blocked on a mod change.
            let curious = rebuild(Some("Power"), Some("Curious"));
            assert_eq!(
                mad_science_variant_of(&curious),
                (CardType::Power, TinkerTimeRider::Curious)
            );

            // A packed `combat_vars.generic` (the engine's own dialect) still
            // wins, and now also re-stamps type/target — before this the
            // native round-trip came back typed Attack too.
            let state = CoreCombatState::default();
            let d = state_to_dict(py, &state).expect("state_to_dict");
            let deck = d
                .get_item("deck")
                .unwrap()
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap();
            let card = PyDict::new(py);
            card.set_item("id", "MAD_SCIENCE").unwrap();
            card.set_item("stable_idx", 0).unwrap();
            card.set_item("kind", "card").unwrap();
            card.set_item("upgraded", false).unwrap();
            let cv = PyDict::new(py);
            // mode Power (2 << 4) | rider Improvement (9) = 41.
            cv.set_item("generic", 41).unwrap();
            card.set_item("combat_vars", &cv).unwrap();
            // A contradictory `type` must NOT override the packed variant.
            card.set_item("type", "Attack").unwrap();
            let hand = PyList::new(py, [&card]).unwrap();
            deck.set_item("hand", &hand).unwrap();
            let rebuilt = state_from_dict(&d).expect("packed MadScience must round-trip");
            let packed = rebuilt
                .cards
                .iter()
                .find(|c| c.card_id == CardId::MadScience)
                .expect("mad science present");
            assert_eq!(
                mad_science_variant_of(packed),
                (CardType::Power, TinkerTimeRider::Improvement),
                "the packed generic var is authoritative over the wire `type`"
            );
            assert_eq!(packed.card_type, CardType::Power);
            assert_eq!(packed.target_type, TargetType::Self_);
        });
    }

    /// A C# `MoveState` carries `params AbstractIntent[]`
    /// (`../sts2-wiki/decompiled/v0.107.1/MegaCrit.Sts2.Core.MonsterMoves.MonsterMoveStateMachine/MoveState.cs:48-53`)
    /// and `TerrorEel.cs:72` uses two — `MultiAttackIntent(3, 3)` and
    /// `BuffIntent()` — so the world telegraphs `[Attack x3, Buff]`. The
    /// Python dict carries both, the primed head first.
    #[test]
    fn wire_emits_every_intent_of_the_primed_move() {
        use pyo3::types::PyList;
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let mut state = CoreCombatState::default();
            state.player.current_hp = 70;
            state.player.max_hp = 70;
            state.init_player_creature();
            let uid = state.spawn_enemy(MonsterKind::TerrorEel);

            let creature = state
                .creatures
                .iter_mut()
                .find(|c| c.id == uid.0)
                .expect("eel spawned");
            creature.ai = Some(build_ai(
                MonsterKind::TerrorEel,
                MonsterSpawnArgs::default(),
                SpawnPosition::default(),
            ));
            let ai = creature.ai.as_mut().expect("TerrorEel is DSL-driven");
            assert!(
                ai.force_current_state_by_name("ThrashMove"),
                "the ported eel has a ThrashMove state"
            );
            let declared = ai.current_move().intents.clone();
            assert_eq!(
                declared.len(),
                2,
                "content declares BOTH C# intents (TerrorEel.cs:72: \
                 MultiAttackIntent + BuffIntent)"
            );
            creature.intent = declared.first().copied();

            let d = state_to_dict(py, &state).expect("state_to_dict");
            let enc = d
                .get_item("encounter")
                .unwrap()
                .unwrap()
                .downcast_into::<PyList>()
                .unwrap();
            let eel = enc.get_item(0).unwrap().downcast_into::<PyDict>().unwrap();
            let intents = eel
                .get_item("intents")
                .unwrap()
                .unwrap()
                .downcast_into::<PyList>()
                .unwrap();

            assert_eq!(intents.len(), 2, "the whole move: [Attack x3, Buff]");
            let tail = intents
                .get_item(1)
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap();
            assert_eq!(
                tail.get_item("kind")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "Buff"
            );
            // The head is the move's first intent — the C#
            // `MultiAttackIntent(ThrashDamage=3, ThrashRepeat=3)`, which the
            // wire spells `kind: "Attack"` + `times: 3`.
            let head = intents
                .get_item(0)
                .unwrap()
                .downcast_into::<PyDict>()
                .unwrap();
            assert_eq!(
                head.get_item("kind")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "Attack"
            );
            assert_eq!(
                head.get_item("times")
                    .unwrap()
                    .unwrap()
                    .extract::<i32>()
                    .unwrap(),
                3
            );
        });
    }

    #[test]
    fn intent_to_pydict_emits_attack_with_amount_and_times() {
        Python::with_gil(|py| {
            // Single-hit attack: kind=Attack, amount=damage, no times.
            let st = CoreCombatState::default();
            let uid = CreatureUid(NonZeroU32::new(1).expect("non-zero"));
            let d = intent_to_pydict(py, Intent::Attack { damage: 12 }, &st, uid).unwrap();
            assert_eq!(
                d.get_item("kind")
                    .unwrap()
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "Attack"
            );
            assert_eq!(
                d.get_item("amount")
                    .unwrap()
                    .unwrap()
                    .extract::<i32>()
                    .unwrap(),
                12
            );
            assert!(d.get_item("times").unwrap().is_none());
            // With no modifiers live, the baked telegraph equals the raw base.
            assert_eq!(
                d.get_item("effective_amount")
                    .unwrap()
                    .unwrap()
                    .extract::<i32>()
                    .unwrap(),
                12,
                "an unmodified board bakes to the base damage"
            );

            // Multi-attack carries the hit count in `times`.
            let m =
                intent_to_pydict(py, Intent::MultiAttack { damage: 5, hits: 3 }, &st, uid).unwrap();
            assert_eq!(
                m.get_item("amount")
                    .unwrap()
                    .unwrap()
                    .extract::<i32>()
                    .unwrap(),
                5
            );
            assert_eq!(
                m.get_item("times")
                    .unwrap()
                    .unwrap()
                    .extract::<i32>()
                    .unwrap(),
                3
            );
        });
    }

    #[test]
    fn intent_to_pydict_kind_round_trips_through_parser() {
        // The PascalCase `kind` we emit must be one `parse_intent` accepts,
        // so a serialized intent survives the round-trip parser.
        Python::with_gil(|py| {
            for intent in [
                Intent::Attack { damage: 9 },
                Intent::MultiAttack { damage: 4, hits: 2 },
                Intent::Defend { block: 8 },
                Intent::Buff {
                    kind: PowerKind::Strength,
                    amount: 2,
                },
                Intent::Debuff {
                    kind: PowerKind::Weak,
                    amount: 1,
                },
                Intent::DebuffStrong,
                Intent::Sleep,
                Intent::Stun,
                Intent::StatusCard,
                Intent::Unknown,
            ] {
                let d = intent_to_pydict(
                    py,
                    intent,
                    &CoreCombatState::default(),
                    CreatureUid(NonZeroU32::new(1).expect("non-zero")),
                )
                .unwrap();
                let mut damage_known = true;
                // Must parse without error.
                parse_intent(d.as_any(), "$.test.intents[0]", &mut damage_known)
                    .unwrap_or_else(|e| panic!("{intent:?} did not re-parse: {e}"));
            }
        });
    }

    #[test]
    fn list_registry_names_known_kinds_nonempty_and_deterministic() {
        for kind in [
            "relic", "relic_id", "card", "monster", "power", "potion", "orb", "event", "boss",
        ] {
            let names = list_registry_names_impl(kind).unwrap();
            assert!(!names.is_empty(), "{kind} list was empty");
            // Deterministic: second call returns identical output.
            let names2 = list_registry_names_impl(kind).unwrap();
            assert_eq!(names, names2);
        }
        // Sortedness is NOT a property of these lists any more. A name's
        // position is its PPO embedding row, so the content-derived kinds are
        // ordered by an append-only pin instead
        // (`sts2_runai::ppo::vocab::registry_names`) — sorting made every
        // content addition renumber rows a deployed champion had learned
        // (GH #544). The kinds with no pin are still sorted, and `boss` is
        // where the two orders visibly differ.
        for kind in ["relic", "monster", "power", "orb"] {
            let names = list_registry_names_impl(kind).unwrap();
            let mut sorted = names.clone();
            sorted.sort();
            assert_eq!(names, sorted, "unpinned kind {kind} should still sort");
        }
        let bosses = list_registry_names_impl("boss").unwrap();
        let mut sorted = bosses.clone();
        sorted.sort();
        assert_ne!(
            bosses, sorted,
            "the boss pin appends Underdocks' bosses after the original nine, \
             so it must NOT be in sorted order"
        );
    }

    #[test]
    fn list_registry_names_unknown_kind_errors() {
        let err = list_registry_names_impl("widget").unwrap_err();
        // Just verify it's a Python ValueError shape; message contains the bad kind.
        let msg = err.to_string();
        assert!(msg.contains("widget"), "msg: {msg}");
    }

    #[test]
    fn list_registry_names_known_entries() {
        // Spot-check one canonical name per registry.
        let relics = list_registry_names_impl("relic").unwrap();
        assert!(relics.iter().any(|s| s == "BURNING_BLOOD"));
        // The four Act-3 soft-drop relics (2026-07-17): the pycli hint
        // adapter prunes titles missing from this list, silently dropping
        // the relic out of the MCTS search state. They have real combat
        // listeners — they MUST stay listed.
        for id in [
            "RAZOR_TOOTH",
            "JEWELED_MASK",
            "CHANDELIER",
            "BEATING_REMNANT",
        ] {
            assert!(
                relics.iter().any(|s| s == id),
                "{id} missing from list_registry_names(\"relic\") — pycli will soft-drop it"
            );
        }
        // Wave 2 (2026-07-17): the seeds-1000-1019 real-game census
        // relics. Same failure mode — the pycli hint adapter prunes any
        // relic title whose id is absent from this list, silently
        // dropping it out of every MCTS search state. Keep ALL of these
        // pinned so a future mirror regeneration can't regress them.
        for id in [
            "NEOWS_TORMENT",
            "RADIANT_PEARL",
            "LOST_COFFER",
            "PRECISE_SCISSORS",
            "NEOWS_TALISMAN",
            "THE_COURIER",
            "UNCEASING_TOP",
            "ELECTRIC_SHRYMP",
            "TOY_BOX",
            "WHITE_STAR",
            "POMANDER",
            "JEWELRY_BOX",
            "ARCANE_SCROLL",
            "PUMPKIN_CANDLE",
            "WINGED_BOOTS",
            "BEAUTIFUL_BRACELET",
            "RUINED_HELMET",
            "SCROLL_BOXES",
            "PRISMATIC_GEM",
            "FIDDLE",
            "GAMBLING_CHIP",
            "BREAD",
            "LIZARD_TAIL",
            "LARGE_CAPSULE",
            "SMALL_CAPSULE",
            "GOLDEN_COMPASS",
            "OLD_COIN",
            "BLESSED_ANTLER",
            "SNECKO_EYE",
            "SCREAMING_FLAGON",
            "STONE_CALENDAR",
            "ART_OF_WAR",
            "BRIMSTONE",
            "WHISPERING_EARRING",
            "SEAL_OF_GOLD",
            "PAELS_HORN",
            "CHARONS_ASHES",
        ] {
            assert!(
                relics.iter().any(|s| s == id),
                "{id} missing from list_registry_names(\"relic\") — pycli will soft-drop it"
            );
        }
        // Wave 3 (2026-07-17): fresh-seed sweep census tail. The
        // structural fix aliases RELIC_KINDS to the macro-derived
        // `RelicKind::ALL`, so these are present by construction —
        // this pin exists so any future regression (e.g. reintroducing
        // a hand-maintained mirror) fails loudly. FISHING_ROD also
        // covers the leaked `FISHING_ROD.title` loc-key: the adapter
        // resolves leaked keys via the registry, so listing the id
        // fixes the title drop too.
        for id in [
            "NUTRITIOUS_OYSTER",
            "SILVER_CRUCIBLE",
            "FISHING_ROD",
            "TOASTY_MITTENS",
            "LASTING_CANDY",
            "SAND_CASTLE",
            "TUNGSTEN_ROD",
            "WHITE_BEAST_STATUE",
            "DELICATE_FROND",
            "DRAGON_FRUIT",
            "CAPTAINS_WHEEL",
            "STURDY_CLAMP",
            "MOLTEN_EGG",
            "PAELS_LEGION",
            "PAELS_BLOOD",
            "GIRYA",
            "PAELS_FLESH",
            "MUMMIFIED_HAND",
            "ICE_CREAM",
            "SEA_GLASS",
            "STONE_HUMIDIFIER",
            "PAELS_TEARS",
            "WAR_HAMMER",
            "LOOMING_FRUIT",
            "UNSETTLING_LAMP",
            "KUNAI",
            "INTIMIDATING_HELMET",
            "PRAYER_WHEEL",
            "SHOVEL",
            "PHIAL_HOLSTER",
            "LEES_WAFFLE",
            "FROZEN_EGG",
            "CLOAK_CLASP",
            // The last two W9 Neow deferrals gained marker kinds in the
            // same wave (see `to_relic_kind_is_total` in sts2-core).
            "NEOWS_BONES",
            "HEFTY_TABLET",
        ] {
            assert!(
                relics.iter().any(|s| s == id),
                "{id} missing from list_registry_names(\"relic\") — pycli will soft-drop it"
            );
        }
        let monsters = list_registry_names_impl("monster").unwrap();
        assert!(monsters.iter().any(|s| s == "Nibbit"));
        let cards = list_registry_names_impl("card").unwrap();
        assert!(cards.iter().any(|s| s == "STRIKE_IRONCLAD"));
        let powers = list_registry_names_impl("power").unwrap();
        assert!(powers.iter().any(|s| s == "STRENGTH"));
        let potions = list_registry_names_impl("potion").unwrap();
        assert!(potions.iter().any(|s| s == "ATTACK_POTION"));
        let orbs = list_registry_names_impl("orb").unwrap();
        assert!(orbs.contains(&"LIGHTNING".to_string()));
    }

    /// Bead `sts2-rs-2rp7`: wire intent dicts must round-trip into
    /// `Intent` enum values, including the multi-attack / variant
    /// fallbacks that earlier dropped silently on the floor.
    #[test]
    fn parse_intent_covers_canonical_kinds() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let cases: &[(&str, Option<i32>, Option<i32>, Intent)] = &[
                ("Attack", Some(6), None, Intent::Attack { damage: 6 }),
                (
                    "Attack",
                    Some(4),
                    Some(3),
                    Intent::MultiAttack { damage: 4, hits: 3 },
                ),
                // hits=1 collapses back to a single Attack.
                ("Attack", Some(9), Some(1), Intent::Attack { damage: 9 }),
                ("Defend", None, None, Intent::Defend { block: 0 }),
                (
                    "Buff",
                    None,
                    None,
                    Intent::Buff {
                        kind: PowerKind::Strength,
                        amount: 0,
                    },
                ),
                (
                    "Debuff",
                    None,
                    None,
                    Intent::Debuff {
                        kind: PowerKind::Weak,
                        amount: 0,
                    },
                ),
                ("DebuffStrong", None, None, Intent::DebuffStrong),
                ("Sleep", None, None, Intent::Sleep),
                ("Stun", None, None, Intent::Stun),
                ("StatusCard", None, None, Intent::StatusCard),
                // Telegraphs the engine has a variant for: these must
                // survive the wire, or a mirror rebuilt from a board the
                // game DID disclose reports its own correct prediction as
                // a divergence (61 Summon + 19 Heal records on the v14
                // sweep were exactly that).
                ("Heal", None, None, Intent::Heal),
                ("Summon", None, None, Intent::Summon),
                (
                    "DeathBlow",
                    Some(30),
                    None,
                    Intent::DeathBlow { damage: 30 },
                ),
                // The game's own "you may not see this" telegraphs — no
                // information to lose.
                ("Hidden", None, None, Intent::Unknown),
                // Bead `sts2-rs-8f336`: Escape round-trips.
                ("Escape", None, None, Intent::Escape),
            ];
            for (kind, amount, times, expected) in cases {
                let d = PyDict::new(py);
                d.set_item("kind", kind).unwrap();
                if let Some(a) = amount {
                    d.set_item("amount", a).unwrap();
                }
                if let Some(t) = times {
                    d.set_item("times", t).unwrap();
                }
                let mut dk = true;
                let got = parse_intent(d.as_any(), "test", &mut dk)
                    .expect("parse_intent succeeded")
                    .expect("intent kind translated");
                assert_eq!(
                    got, *expected,
                    "kind={kind:?} amount={amount:?} times={times:?}"
                );
            }

            // Unknown kinds → schema error (not silent acceptance).
            let bad = PyDict::new(py);
            bad.set_item("kind", "ChaosMagicAttack").unwrap();
            let mut dk_bad = true;
            assert!(
                parse_intent(bad.as_any(), "test", &mut dk_bad).is_err(),
                "unknown intent kind should error, not silently downgrade"
            );

            // Missing `kind` field → schema error.
            let empty = PyDict::new(py);
            let mut dk_empty = true;
            assert!(parse_intent(empty.as_any(), "test", &mut dk_empty).is_err());
        });
    }

    /// `parse_intent` flips `damage_known` to false when an Attack
    /// intent ends up with damage=0 after both the wire `amount` and
    /// the label-regex fallback fail. Non-Attack intents and Attacks
    /// with rescue-able labels keep the flag at the caller-initialised
    /// `true`. Bead `sts2-rs-bpt4`.
    #[test]
    fn parse_intent_label_salvage_and_damage_known_flag() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            // Case 1: wire amount=0 but label rescues damage=6 →
            // Intent::Attack { damage: 6 }, damage_known stays true.
            let d = PyDict::new(py);
            d.set_item("kind", "Attack").unwrap();
            d.set_item("amount", 0_i32).unwrap();
            d.set_item("label", "Attack 6").unwrap();
            let mut dk = true;
            let got = parse_intent(d.as_any(), "test", &mut dk).unwrap().unwrap();
            assert_eq!(got, Intent::Attack { damage: 6 });
            assert!(dk, "label-salvaged Attack should leave damage_known=true");

            // Case 2: amount missing, label gives multi-hit "6 × 3".
            let d = PyDict::new(py);
            d.set_item("kind", "Attack").unwrap();
            d.set_item("label", "6 × 3").unwrap();
            let mut dk = true;
            let got = parse_intent(d.as_any(), "test", &mut dk).unwrap().unwrap();
            assert_eq!(got, Intent::MultiAttack { damage: 6, hits: 3 });
            assert!(dk);

            // Case 3: amount=0, no label → damage_known flips to false.
            let d = PyDict::new(py);
            d.set_item("kind", "Attack").unwrap();
            d.set_item("amount", 0_i32).unwrap();
            let mut dk = true;
            let got = parse_intent(d.as_any(), "test", &mut dk).unwrap().unwrap();
            assert_eq!(got, Intent::Attack { damage: 0 });
            assert!(!dk, "no salvage path → damage_known must be false");

            // Case 4: amount=0, label has no integer → flag flips false.
            let d = PyDict::new(py);
            d.set_item("kind", "Attack").unwrap();
            d.set_item("amount", 0_i32).unwrap();
            d.set_item("label", "Attack").unwrap();
            let mut dk = true;
            let got = parse_intent(d.as_any(), "test", &mut dk).unwrap().unwrap();
            assert_eq!(got, Intent::Attack { damage: 0 });
            assert!(!dk);

            // Case 5: non-Attack intents never touch the flag, even
            // with no amount.
            let d = PyDict::new(py);
            d.set_item("kind", "Defend").unwrap();
            let mut dk = true;
            parse_intent(d.as_any(), "test", &mut dk).unwrap();
            assert!(dk, "Defend has no damage to be missing");

            // Case 6: wire `amount` already correct → label is ignored
            // (no need to consult salvage), flag stays true.
            let d = PyDict::new(py);
            d.set_item("kind", "Attack").unwrap();
            d.set_item("amount", 8_i32).unwrap();
            d.set_item("label", "Attack 999").unwrap();
            let mut dk = true;
            let got = parse_intent(d.as_any(), "test", &mut dk).unwrap().unwrap();
            assert_eq!(got, Intent::Attack { damage: 8 });
            assert!(dk);
        });
    }
}
