//! `PyO3` binding for [`sts2_core::run_manager::RunManager`].
//!
//! Bead `sts2-rs-8xlqe` (P9-PY). Exposes a thin Python surface so the
//! sister `../sts2-pycli` project can drive Rust-side full runs the same
//! way it drives combat via the existing `sts2_hint` adapter. Only
//! Ironclad is supported today — that's the only character with
//! wired-up `sts2_content::run_callbacks::ContentCallbacks` at this
//! pin. Adding Silent/Defect/Regent/Necrobinder is a follow-up once
//! their `*_content_callbacks()` constructors land.
//!
//! Action dict shape mirrors [`crate::action_to_pydict`]:
//! `{"action": "<name>", ...payload}`. Phase/result strings are
//! lowercase `snake_case`. See the integration test
//! `crates/sts2-cli/tests/full_run_act1to3_ironclad.rs` for the
//! canonical Rust-side driver this mirrors.

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};

use sts2_content::run_callbacks::{
    new_ironclad_run_manager_with_deck_and_hp, new_run_manager_for_character,
    new_run_manager_for_character_from_string_seed, new_run_manager_for_character_with_ascension,
};
use sts2_core::act_selection::ActVariant;
use sts2_core::cards::CardId;
use sts2_core::map::MapPointType;
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::{ActionResult, RunAction, RunManager};
use sts2_wire::emit::names::{character_str, result_str};
use sts2_wire::emit::run_manager as emit_rm;

/// The `phase()` vocabulary, re-exported so the agent modules keep reading it
/// from here.
pub(crate) use sts2_wire::emit::names::phase_str;
fn character_from_str(s: &str) -> PyResult<CharacterId> {
    sts2_wire::emit::names::character_from_str(s).map_err(PyValueError::new_err)
}

fn card_id_from_str(s: &str) -> PyResult<CardId> {
    // Accept both `CARD.STRIKE` (sts2log / scraped `.run` shape) and
    // `STRIKE` (canonical python_name). `names::card_id_from_str`
    // additionally tolerates the `_CARD` suffix used by some sts2log
    // entries (bead `sts2-rs-u5zc`).
    let stripped = s.strip_prefix("CARD.").unwrap_or(s);
    sts2_content::names::card_id_from_str(stripped)
        .ok_or_else(|| PyValueError::new_err(format!("unknown card id {s:?}")))
}

/// Parse a `take_action` dict: the Python dict is read into a
/// `serde_json::Value` and handed to
/// [`sts2_wire::emit::run_action_from_value`], the ONE parser of this shape.
/// Faults raise `ValueError` with that parser's message.
fn action_from_pydict(d: &Bound<'_, PyDict>) -> PyResult<RunAction> {
    let value: serde_json::Value = pythonize::depythonize(d.as_any())?;
    sts2_wire::emit::run_action_from_value(&value).map_err(crate::emit_err)
}

/// The `{"action": "<name>", …}` dict for a [`RunAction`], built by
/// [`sts2_wire::emit::run_action_to_value`].
pub(crate) fn run_action_to_pydict<'py>(
    py: Python<'py>,
    a: &RunAction,
) -> PyResult<Bound<'py, PyDict>> {
    crate::value_to_pydict(py, &sts2_wire::emit::run_action_to_value(a))
}

/// JSON has no tuples. The wire emitters spell a `(col, row)` pair as a
/// two-element array; this restores the tuple the Python surface has always
/// presented (`act_map()[i]["children"]`, `current_room()["map_coord"]`).
/// A dialect concern — the numbers come from the emitter untouched.
fn pair_to_tuple<'py>(py: Python<'py>, v: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyTuple>> {
    let pair: Vec<i32> = v.extract()?;
    PyTuple::new(py, pair)
}

/// [`pair_to_tuple`] over every `children` entry of an `act_map()` dump.
fn tuplize_act_map_children<'py>(py: Python<'py>, list: &Bound<'py, PyList>) -> PyResult<()> {
    for point in list.iter() {
        let point = point.downcast::<PyDict>()?;
        if let Some(children) = point.get_item("children")? {
            let children = children.downcast::<PyList>()?;
            let tuples = children
                .iter()
                .map(|c| pair_to_tuple(py, &c))
                .collect::<PyResult<Vec<_>>>()?;
            point.set_item("children", PyList::new(py, tuples)?)?;
        }
    }
    Ok(())
}

/// An `act_map()` / `act_map_for()` dump as Python: `None` for `Null`,
/// otherwise the point list with tuple children.
fn act_map_to_py(py: Python<'_>, value: &serde_json::Value) -> PyResult<PyObject> {
    if value.is_null() {
        return Ok(py.None());
    }
    let list = crate::value_to_pylist(py, value)?;
    tuplize_act_map_children(py, &list)?;
    Ok(list.into_any().unbind())
}

/// Python wrapper around [`sts2_core::run_manager::RunManager`].
///
/// Bead `sts2-rs-8xlqe`. Only Ironclad is wired today — passing any
/// other character to the constructor raises `NotImplementedError`.
#[pyclass(name = "RunManager", module = "sts2_rs")]
#[derive(Clone, Debug)]
pub struct PyRunManager {
    pub(crate) inner: RunManager,
    seed: i32,
    /// When `true`, the live combat records its card play-log (workbench event
    /// log). Default false; only the workbench enables it. Propagated onto the
    /// live `CombatState` so it survives combat-start resets of `TurnEvents`.
    record_card_plays: bool,
    /// When `true`, the live combat records the FULL game-style event log
    /// (`sts2_core::event_log`) — every hit, block, power, draw, orb and
    /// potion, with the card or move that caused it. Same sticky-flag
    /// discipline as `record_card_plays`: `TurnEvents` resets at combat start,
    /// so the opt-in is re-asserted before every action.
    record_combat_log: bool,
}

impl PyRunManager {
    /// Re-assert the sticky play-log recording flag onto the live combat
    /// before stepping the engine. A freshly-started combat resets
    /// `TurnEvents` to its default (`record_card_play_log = false`), so the
    /// workbench's opt-in must be re-applied on every action. Display-only
    /// side channel — never affects `state_to_dict` / parity.
    pub(crate) fn ensure_play_log_flag(&mut self) {
        if self.record_card_plays {
            if let Some(c) = self.inner.combat.as_mut() {
                c.combat_log.record_card_play_log = true;
            }
        }
        if self.record_combat_log {
            if let Some(c) = self.inner.combat.as_mut() {
                c.combat_log.event_log.set_recording(true);
            }
        }
    }
}

#[pymethods]
impl PyRunManager {
    #[new]
    #[pyo3(signature = (seed = 0, character = "Ironclad", deck = None, max_hp = None, string_seed = None, ascension_level = 0, neow_start = false, interactive_card_removal = false, direct_event_relic_grant = false, interactive_card_upgrade = false, interactive_card_transform = false, interactive_potion_swap = false, act1 = "overgrowth", interactive_rewards = false, interactive_potions = false))]
    // A PyO3 constructor mirroring the optional-kwarg surface; the arg list
    // tracks the run's configurable knobs (seed forms, deck/hp overrides,
    // neow-start, C#-faithful UI toggles) rather than a bundled options struct.
    // The several `bool` UI toggles map 1:1 to Python kwargs — bundling them
    // into enums would break that surface, so the excessive-bools lint is
    // allowed here alongside too-many-arguments.
    #[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
    fn new(
        seed: i32,
        character: &str,
        deck: Option<Vec<String>>,
        max_hp: Option<i32>,
        string_seed: Option<&str>,
        ascension_level: u8,
        neow_start: bool,
        interactive_card_removal: bool,
        direct_event_relic_grant: bool,
        interactive_card_upgrade: bool,
        interactive_card_transform: bool,
        interactive_potion_swap: bool,
        act1: &str,
        interactive_rewards: bool,
        interactive_potions: bool,
    ) -> PyResult<Self> {
        let cid = character_from_str(character)?;
        // Bead `sts2-rs-livck`: Ironclad / Silent / Necrobinder / Regent
        // / Defect now route through the character-parameterised
        // factories in `sts2-content::run_callbacks`. Defect's
        // starter deck currently falls back to the Ironclad starter
        // deck because the Defect starter-card factory isn't ported
        // yet — runs will succeed but the deck won't match the C# pin
        // until `starters::starter_deck_for(Defect)` lands real content.
        // When `string_seed` is provided, it takes precedence — this is
        // the C#-game-style 10-char alphanumeric seed (e.g. "3ACCFREXKR")
        // as emitted by scraped .run files. We canonicalise + hash to the
        // same i32 the C# game uses, ignoring any explicit `seed` arg.
        let effective_seed = match string_seed {
            Some(s) => {
                let canonical = sts2_core::canonicalize_seed(s);
                sts2_core::deterministic_hash_code(&canonical)
            }
            None => seed,
        };
        let inner = match deck {
            None => {
                // When `ascension_level > 0`, route through the ascension
                // factory so A3 gold, A4 potion-slot, A5 AscendersBane,
                // A7 Scarcity (etc.) are applied. String-seed canonicalisation
                // is preserved by passing the already-hashed `effective_seed`.
                let mut rm = if ascension_level > 0 {
                    new_run_manager_for_character_with_ascension(
                        effective_seed,
                        cid,
                        ascension_level,
                    )
                } else {
                    match string_seed {
                        Some(s) => new_run_manager_for_character_from_string_seed(s, cid),
                        None => new_run_manager_for_character(effective_seed, cid),
                    }
                };
                if let Some(hp) = max_hp {
                    rm.run_state.max_hp = hp;
                    rm.run_state.current_hp = hp;
                }
                rm
            }
            Some(d) => {
                if ascension_level > 0 {
                    return Err(PyValueError::new_err(
                        "ascension_level + custom deck override is not yet supported",
                    ));
                }
                if !matches!(cid, CharacterId::Ironclad) {
                    return Err(PyValueError::new_err(format!(
                        "custom `deck` override is currently Ironclad-only \
                         (factory `new_ironclad_run_manager_with_deck_and_hp` \
                         hardcodes the character); character {character:?} \
                         must use the starter-deck path. Bead sts2-rs-livck \
                         follow-up."
                    )));
                }
                let cards = d
                    .iter()
                    .map(|s| card_id_from_str(s))
                    .collect::<PyResult<Vec<_>>>()?;
                new_ironclad_run_manager_with_deck_and_hp(effective_seed, cards, max_hp)
            }
        };
        // Bead `sts2-rs-g24zv`: opt-in Neow-start. When `neow_start=True`,
        // the run lands in `Phase::Event` with `current_event=Neow` and
        // the 3-relic offers cached on `run_state.neow_offers`. Required
        // for corpus replay (every scraped v0.103.2 run begins at
        // `first_room_type=ancient`).
        let mut inner = if neow_start {
            inner.with_neow_start()
        } else {
            inner
        };
        // Opt-in interactive deck removal: on-obtain card-removal relics
        // (Precise Scissors, …) present a player card_select instead of the
        // default front-of-deck auto-removal. The workbench sets this for
        // C#-faithful Neow; default off keeps the parity-fuzz behavior.
        inner.run_state.interactive_card_removal = interactive_card_removal;
        // Opt-in interactive deck upgrade: on-obtain card-upgrade relics
        // (Pomander) present a player card_select instead of the default
        // random rewards-stream auto-upgrade, matching C#
        // `Pomander.AfterObtained` → `CardSelectCmd.FromDeckForUpgrade`. The
        // workbench / `run_parity` lockstep set this; default off keeps the
        // parity-fuzz behavior. Bead `sts2-rs-1hiht`.
        inner.run_state.interactive_card_upgrade = interactive_card_upgrade;
        // Opt-in interactive deck transform: on-obtain card-transform relics
        // (NewLeaf) present a player card_select whose replacement is rolled
        // off `Rng.Niche` only after the pick, matching C#
        // `NewLeaf.AfterObtained` → `CardSelectCmd.FromDeckForTransformation`
        // → `CardCmd.TransformToRandom`. Bead 6ynt.4.deck-add / GH #34.
        inner.run_state.interactive_card_transform = interactive_card_transform;
        // Opt-in interactive full-belt potion swap: a potion reward landing
        // on a full belt surfaces a `potion_swap` discard-to-claim decision
        // instead of being silently dropped, matching C#'s TooFull reward +
        // `DiscardPotionGameAction` flow. Default off keeps the historical
        // (and headless-oracle) silent drop. GH #252.
        inner.run_state.interactive_potion_swap = interactive_potion_swap;
        // Grant Neow blessing relics directly (no extra Take relic/Skip
        // confirmation), matching C#. See `RunState::direct_event_relic_grant`.
        inner.run_state.direct_event_relic_grant = direct_event_relic_grant;
        // sts2-agent fork: the opt-in rewards screen after a won combat
        // (claim each reward or leave the rest) and the opt-in potion actions
        // (discard anywhere the belt shows, AnyTime drinks out of combat).
        // Default off keeps every existing caller's run unchanged.
        inner.run_state.interactive_rewards = interactive_rewards;
        inner.run_state.interactive_potions = interactive_potions;
        // GH #544: which of the two Act-1 models this run walks. Must be set
        // BEFORE `initialize_run_setup`, which pre-generates every act's
        // rooms off `up_front`. Defaults to `"overgrowth"` — the single Act 1
        // this engine modelled before the Underdocks port — so no existing
        // caller changes behaviour. `"roll"` reproduces the real game's
        // seed-driven pick.
        let mut inner = match act1 {
            "overgrowth" => inner.with_act1(ActVariant::Overgrowth),
            "underdocks" => inner.with_act1(ActVariant::Underdocks),
            "roll" => inner.with_rolled_act1(),
            other => {
                return Err(PyValueError::new_err(format!(
                    "unknown act1 {other:?} (expected \"overgrowth\", \"underdocks\" or \"roll\")"
                )))
            }
        };
        // C#-faithful run setup: populate the relic grab bags (player +
        // shared) and pre-roll all acts' room contents off the `up_front`
        // stream, then re-pin act 1's boss to the generated identity. The bare
        // run factories omit this; every full-run driver (this binding, the
        // run-AI arena / labeler / exporter / calibrator) calls the one shared
        // function so they cannot diverge. See `initialize_run_setup` for the
        // verified `up_front` read order (beads mcesz / 7e04v / jenwp).
        sts2_content::run_callbacks::initialize_run_setup(&mut inner);
        Ok(Self {
            inner,
            seed: effective_seed,
            record_card_plays: false,
            record_combat_log: false,
        })
    }

    #[getter]
    fn seed(&self) -> i32 {
        self.seed
    }

    /// The run's own hashed master seed, as `u32` — the value every RNG
    /// stream salts from, and the one the wire spells `$.rng.run_seed`
    /// (C# `RunRngSet.Seed` is a uint).
    ///
    /// Distinct from [`Self::seed`], which is the pre-hash i32 this binding
    /// was constructed with; for a `string_seed` run the two differ and only
    /// this one lines up with a producer's `run_seed`. Read it from the run
    /// rather than re-deriving it from the text seed — canonicalisation alone
    /// is enough to make a second derivation disagree.
    #[getter]
    fn run_seed(&self) -> u32 {
        emit_rm::run_seed(&self.inner)
    }

    /// Which Act-1 model this run ACTUALLY walks (GH #544): `"overgrowth"` or
    /// `"underdocks"`.
    ///
    /// The RESOLVED act, not the `act1=` argument — construct with
    /// `act1="roll"` and this reports which of the two the seed landed on.
    /// Read it from here rather than re-deriving it from the seed: the run is
    /// the authority, and a second derivation is a second thing that can be
    /// wrong (`canonicalize_seed` alone is enough to make them disagree).
    ///
    /// The workbench surfaces this so a bug report against "act 1" says WHICH
    /// act 1; STS2 ships two of them.
    #[getter]
    fn act1(&self) -> &'static str {
        emit_rm::act1(&self.inner)
    }

    /// Number of master-deck cards the player still owes an interactive
    /// removal pick for (non-zero only when the run was constructed with
    /// `interactive_card_removal=True`). While `> 0` the legal action set
    /// is `remove_deck_card` picks regardless of `phase()`, so a UI host
    /// should render a deck card-select.
    #[getter]
    fn pending_deck_removals(&self) -> u32 {
        self.inner.run_state.pending_deck_removals
    }

    /// Number of non-upgraded master-deck cards the player still owes an
    /// interactive upgrade pick for (non-zero only when the run was
    /// constructed with `interactive_card_upgrade=True`). While `> 0` the
    /// legal action set is `upgrade_deck_card` picks (non-upgraded deck
    /// indices) regardless of `phase()`, so a UI host should render a deck
    /// card-select. Bead `sts2-rs-1hiht`.
    #[getter]
    fn pending_deck_upgrades(&self) -> u32 {
        self.inner.run_state.pending_deck_upgrades
    }

    /// Potion kind (debug name, e.g. `"EnergyPotion"`) awaiting a
    /// full-belt swap decision, or `None` when no swap is pending
    /// (non-`None` only when the run was constructed with
    /// `interactive_potion_swap=True`). While pending, the legal action
    /// set is `potion_swap` picks (`discard_slot=None` declines) regardless
    /// of `phase()`. GH #252.
    /// Whether full-belt potion rewards surface interactive `potion_swap`
    /// decisions on this run (construction flag readback — the training
    /// env's flag-parity test pins it against `resolve_potion_swap_flag`).
    #[getter]
    fn interactive_potion_swap(&self) -> bool {
        self.inner.run_state.interactive_potion_swap
    }

    /// Debug/test helper (plan 2026-08-20-ppo-potion-bootstrap P2),
    /// `set_player`-style: replace the run belt with `potions` and enqueue
    /// `pending` as full-belt swap offers, both as UPPER_SNAKE python
    /// names (e.g. `"FIRE_POTION"`). Never call on a recorded or
    /// parity-pinned run — it perturbs run state out-of-band.
    #[pyo3(signature = (potions, pending = Vec::new()))]
    fn debug_set_potion_state(
        &mut self,
        potions: Vec<String>,
        pending: Vec<String>,
    ) -> PyResult<()> {
        let parse = |name: &str| {
            crate::state_dict::potion_kind_from_python_name(name).ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err(format!("unknown potion {name:?}"))
            })
        };
        let rs = &mut self.inner.run_state;
        rs.potions.clear();
        for name in &potions {
            rs.potions.push(parse(name)?);
        }
        rs.pending_potion_swaps.clear();
        for name in &pending {
            rs.pending_potion_swaps.push(parse(name)?);
        }
        Ok(())
    }

    /// Construction flag readback (sts2-agent fork).
    #[getter]
    fn interactive_rewards(&self) -> bool {
        self.inner.run_state.interactive_rewards
    }

    /// Construction flag readback (sts2-agent fork).
    #[getter]
    fn interactive_potions(&self) -> bool {
        self.inner.run_state.interactive_potions
    }

    /// Construction flag readback (sts2-agent fork).
    #[getter]
    fn interactive_card_removal(&self) -> bool {
        self.inner.run_state.interactive_card_removal
    }

    /// Construction flag readback (sts2-agent fork).
    #[getter]
    fn interactive_card_upgrade(&self) -> bool {
        self.inner.run_state.interactive_card_upgrade
    }

    /// Construction flag readback (sts2-agent fork).
    #[getter]
    fn interactive_card_transform(&self) -> bool {
        self.inner.run_state.interactive_card_transform
    }

    /// Construction flag readback (sts2-agent fork).
    #[getter]
    fn direct_event_relic_grant(&self) -> bool {
        self.inner.run_state.direct_event_relic_grant
    }

    /// The open rewards screen (sts2-agent fork; `interactive_rewards=True`):
    /// `[{"type": "gold", "amount"} | {"type": "potion", "id"} | {"type":
    /// "relic", "id"} | {"type": "card", "options": [{"id", "upgraded"}]}]`,
    /// index-aligned with `claim_reward`'s `idx`, or `None` when no screen is
    /// open. While a card reward claimed from it is open,
    /// `screen_card_claim()` names its index.
    fn reward_screen(&self, py: Python<'_>) -> PyResult<PyObject> {
        crate::value_to_py(py, &emit_rm::reward_screen(&self.inner)).map(Bound::unbind)
    }

    /// The deck as card dicts (sts2-agent fork): `id`, `upgraded`,
    /// `base_cost`, `base_damage`, `base_block`, `type`, keyword `flags`,
    /// enchantment, affliction, `replay_count` and the raw `deck_index`, in
    /// the deck view of `master_deck()` (in-progress steals excluded).
    fn master_deck_cards(&self, py: Python<'_>) -> PyResult<PyObject> {
        crate::value_to_py(py, &emit_rm::master_deck_cards(&self.inner)).map(Bound::unbind)
    }

    /// A fresh card of `card_id` as the deck would hold it (sts2-agent fork),
    /// in `master_deck_cards()`'s shape: what an offered card (a reward, a
    /// shop slot, a grant) is before it is taken. Raises `ValueError` for an
    /// unknown id.
    #[staticmethod]
    #[pyo3(signature = (card_id, upgraded = false))]
    fn card_spec(py: Python<'_>, card_id: &str, upgraded: bool) -> PyResult<PyObject> {
        let cid = card_id_from_str(card_id)?;
        let card = sts2_core::lookup_card_factory()
            .and_then(|f| f(cid, upgraded))
            .ok_or_else(|| PyValueError::new_err(format!("no card factory for {card_id:?}")))?;
        crate::value_to_py(py, &emit_rm::card_value(&card)).map(Bound::unbind)
    }

    /// A potion's target shape (sts2-agent fork): `"Self"`, `"AnyEnemy"`,
    /// `"AllEnemies"` or `"AnyPlayer"` (the engine's `PotionTargetType`, from the
    /// content registry), or `None` for an unregistered kind. A host offers an
    /// `AnyEnemy` potion once per living enemy (`drink_potion`'s `target_index`);
    /// the combat's own legal actions list it once, untargeted. Raises
    /// `ValueError` for an unknown potion name (`"FIRE_POTION"`).
    #[staticmethod]
    fn potion_target_type(potion: &str) -> PyResult<Option<&'static str>> {
        use sts2_core::potions::PotionTargetType as T;
        let kind = crate::state_dict::potion_kind_from_python_name(potion)
            .ok_or_else(|| PyValueError::new_err(format!("unknown potion {potion:?}")))?;
        Ok(
            sts2_core::potion_effects::lookup_target_type(kind).map(|t| match t {
                T::Self_ => "Self",
                T::AnyEnemy => "AnyEnemy",
                T::AllEnemies => "AllEnemies",
                T::AnyPlayer => "AnyPlayer",
            }),
        )
    }

    /// The rewards-screen entry whose card reward is open now (sts2-agent fork).
    fn screen_card_claim(&self) -> Option<usize> {
        self.inner.screen_card_claim()
    }

    #[getter]
    fn pending_potion_swap(&self) -> Option<String> {
        self.inner
            .run_state
            .pending_potion_swaps
            .first()
            .map(|k| format!("{k:?}"))
    }

    /// Potions irrecoverably lost to a full belt this run — silent drops
    /// plus declined `potion_swap` decisions (GH #252).
    #[getter]
    fn potions_overflow_dropped(&self) -> u32 {
        self.inner.run_state.potions_overflow_dropped
    }

    /// Number of master-deck cards the player still owes an interactive
    /// transform pick for (non-zero only when the run was constructed with
    /// `interactive_card_transform=True`). While `> 0` the legal action set
    /// is `transform_deck_card` picks (raw master-deck indices) regardless
    /// of `phase()`, so a UI host should render a deck card-select. Bead
    /// 6ynt.4.deck-add / GH #34.
    #[getter]
    fn pending_deck_transforms(&self) -> u32 {
        u32::try_from(self.inner.run_state.pending_deck_transforms.len()).unwrap_or(u32::MAX)
    }

    /// Number of master-deck cards the player still owes an interactive
    /// ENCHANT pick for (Wood Carvings' Snake → Slither). While `> 0` the
    /// legal action set is `enchant_deck_card` picks indexed into
    /// `enchant_select_deck_indices()`.
    #[getter]
    fn pending_deck_enchants(&self) -> u32 {
        u32::try_from(self.inner.run_state.pending_deck_enchants.len()).unwrap_or(u32::MAX)
    }

    /// WHICH enchantment the front queued deck-enchant pick will stamp
    /// (`"GOOPY"`, `"PERFECT_FIT"`, …), or `None` when nothing is queued.
    ///
    /// GH #545: `pending_deck_enchants` is a bare COUNT, so every enchant
    /// option featurized identically to a removal option — and the
    /// enchantments are not interchangeable (Goopy adds Exhaust; Perfect Fit
    /// is a shuffle hint). The FRONT entry is the one being decided:
    /// `do_enchant_deck_card_at` pops index 0 (`run_manager.rs:3374`).
    #[getter]
    fn pending_enchantment(&self) -> Option<&'static str> {
        self.inner
            .run_state
            .pending_deck_enchants
            .first()
            .map(|p| p.enchantment.python_name())
    }

    /// Master-deck positions eligible for the front pending interactive
    /// transform — the option pool a UI host must render, in order; the
    /// `transform_deck_card` action's `idx` is the position in THIS list
    /// (matching the C# headless card_select index space).
    fn transform_select_deck_indices(&self) -> Vec<usize> {
        self.inner.transform_eligible_deck_indices()
    }

    /// Master-deck positions eligible for the front pending interactive
    /// enchant (`enchant_deck_card`'s `idx` indexes this list).
    fn enchant_select_deck_indices(&self) -> Vec<usize> {
        self.inner.enchant_eligible_deck_indices()
    }

    /// Master-deck positions eligible for a pending interactive upgrade —
    /// C# `FromDeckForUpgrade`'s `Where(IsUpgradable)` subset (curses /
    /// statuses / quest cards excluded, NOT merely non-upgraded). The
    /// `upgrade_deck_card` action's `idx` is the position in THIS list;
    /// a picker that offers merely non-upgraded cards over-offers (e.g.
    /// GUILTY in AromaOfChaos' upgrade select, census seed 1010) and
    /// desyncs the pick index space vs the engine + the DLL.
    fn upgrade_select_deck_indices(&self) -> Vec<usize> {
        self.inner
            .run_state
            .master_deck
            .iter()
            .enumerate()
            .filter(|(_, c)| c.is_upgradable())
            .map(|(i, _)| i)
            .collect()
    }

    /// Number of picks the FRONT pending multi-pick card grant still requires
    /// (RoomFullOfCheese Gorge → 2). `0` when no grant is open. While `> 0` the
    /// legal action set is `grant_deck_card` picks indexed into
    /// `card_grant_options()`, and a UI host should render a mandatory pick-N
    /// `card_select` (`min == max ==` this value). Bead `sts2-rs-gorge-select`.
    #[getter]
    fn pending_card_grants(&self) -> u32 {
        self.inner
            .run_state
            .pending_card_grants
            .first()
            .map_or(0, |g| u32::try_from(g.pick).unwrap_or(u32::MAX))
    }

    /// `(PascalCase id, upgraded)` of the FRONT pending card grant's generated
    /// options, in C# generation order — the option pool a UI host renders and
    /// the index space `grant_deck_card`'s `idx` selects. Empty when no grant
    /// is open. Bead `sts2-rs-gorge-select`.
    fn card_grant_options(&self) -> Vec<(String, bool)> {
        emit_rm::card_grant_options(&self.inner)
    }

    /// `true` while the current card offering is a *choose screen* (C#
    /// `FromChooseACardScreen` — HeftyTablet's 3 Rares, LeadPaperweight's
    /// 2 Colorless): a UI host should render a skippable 1-of-N
    /// `card_select` (min 0 / max 1) instead of a `card_reward`, while
    /// still resolving picks through `pick_card_reward`. Bead
    /// 6ynt.4.deck-add / GH #34.
    #[getter]
    fn card_offer_choose_screen(&self) -> bool {
        self.inner.card_offer_choose_screen()
    }

    /// Card ids (PascalCase `Debug` form, e.g. `"Anger"`) of the
    /// upgradeable cards offered by an open Smith rest-site choice, in the
    /// order a `SmithCard{idx}` action indexes them. Empty when no
    /// `CardSelect` pending choice is open (the candidates are the deck's
    /// not-yet-upgraded cards). Lets a UI host render the Smith pick as a
    /// proper card list (names / art / text) instead of bare buttons.
    /// Relic ids (PascalCase `Debug` form, e.g. `"LostWisp"`) currently
    /// offered as relic rewards (head of the `relic_rewards` queue first), so a
    /// UI host can name the relic on the reward screen instead of a bare "Take
    /// relic". Empty when no relic reward is pending.
    fn relic_reward_ids(&self) -> Vec<String> {
        emit_rm::relic_reward_ids(&self.inner)
    }

    /// Relic ids (PascalCase `Debug` form, e.g. `"SneckoEye"`) offered
    /// on the post-boss boss-relic-choice screen, in the order a
    /// `PickBossRelic{idx}` action indexes them. Empty when no
    /// `BossRelic` phase is active. Lets a UI host render the offers as
    /// proper relic cards (art + name + description) instead of bare
    /// "Relic 0 / Relic 1 / Relic 2" buttons.
    fn boss_relic_options(&self) -> Vec<String> {
        emit_rm::boss_relic_options(&self.inner)
    }

    /// `(option_id, enabled)` for every rest-site option the site
    /// offers, in the order a `RestOption{idx}` action indexes them.
    /// `option_id` is the game's own `RestSiteOption.OptionId`
    /// (`"HEAL"` / `"SMITH"` / `"KINDLE"` / ...), so a UI host can look
    /// up the real plaque art + `rest_site_ui` loc row instead of
    /// numbering the buttons. Empty outside `Phase::RestSite`.
    ///
    /// The offered set is dynamic (Shovel appends Dig, Girya Lift,
    /// PumpkinCandle Kindle, MeatCleaver Cook, a ByrdonisEgg Hatch), so
    /// an index-keyed label table in the host is wrong the moment a
    /// relic adds a third option — which is what shipped a bare
    /// "Option 2" plaque for Kindle (`wb-500a91f0d29256f6`).
    fn rest_site_options(&self) -> Vec<(String, bool)> {
        emit_rm::rest_site_options(&self.inner)
    }

    fn smith_options(&self) -> Vec<String> {
        emit_rm::smith_options(&self.inner)
    }

    /// 3-tuple of `SCREAMING_SNAKE_CASE` relic ids `(positive_1,
    /// positive_2, cursed)` rolled for the run-start Neow event, or
    /// `None` if `neow_start=False` was passed to the constructor. Bead
    /// `sts2-rs-k8z6m` (R3.1.4 under `sts2-rs-nr8l4`).
    fn neow_offers(&self) -> Option<(String, String, String)> {
        emit_rm::neow_offers(&self.inner)
    }

    #[getter]
    fn character(&self) -> &'static str {
        character_str(self.inner.character_id)
    }

    fn phase(&self) -> &'static str {
        phase_str(self.inner.phase())
    }

    fn is_over(&self) -> bool {
        self.inner.is_over()
    }

    /// Run-level RNG stream cursors (read counts so far), for whole-run
    /// parity diagnostics (`run_parity.py`). Keyed to line up with the C#
    /// `RunSimulator.RngCounters()` dump — except `run_manager`, which is a
    /// Python-pin stream with no C# `RunRngType` analog (its non-zero count
    /// vs C#'s advanced `up_front` is the run-RNG divergence signal).
    fn rng_counters(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        crate::value_to_pydict(py, &emit_rm::rng_counters(&self.inner)).map(Bound::unbind)
    }

    /// Restore this run's RNG cursors from a live-game snapshot.
    ///
    /// `counters` is the `{stream: read_count}` block SpireMentor stamps onto
    /// every outbound message (`SimStateExtractor.RngState`), keyed by the
    /// same snake_case stream names this crate uses. Each named stream is
    /// fast-forwarded to its recorded count, which is what makes an imported
    /// run roll the SAME next card reward, shop stock, event and monster
    /// intent as the game it was copied from. Without it an import shares
    /// only the seed, and every downstream roll is a different run's.
    ///
    /// `run_seed` is the producer's once-hashed master seed. It must equal
    /// this run's own: counters from a different seed describe a different
    /// run's dice, and restoring those is strictly worse than restoring
    /// nothing.
    ///
    /// Returns the names it did NOT apply — streams that belong to a
    /// `CombatState` (`state_from_dict` restores those) or that this engine
    /// does not carry. Reported rather than dropped silently, because a
    /// half-restored lineage looks exactly like a whole one.
    ///
    /// Debug/reconstruction surface, not a parity surface: never call it on
    /// a recorded or parity-pinned run.
    ///
    /// Raises `ValueError` when `run_seed` disagrees, when `counters` is
    /// empty, or when a counter would move a stream BACKWARDS — the dotnet
    /// Rng can only fast-forward, and a lower count means this snapshot does
    /// not belong to this run.
    fn debug_set_rng_counters(
        &mut self,
        run_seed: i64,
        counters: &Bound<'_, PyDict>,
    ) -> PyResult<Vec<String>> {
        if counters.is_empty() {
            return Err(PyValueError::new_err(
                "debug_set_rng_counters: empty counters — a seed without \
                 cursors rebuilds every stream at count 0, which destroys \
                 more lineage than it restores. Pass the whole block or none.",
            ));
        }
        let ours = i64::from(u32::from_ne_bytes(
            self.inner.run_state.rng.seed.to_ne_bytes(),
        ));
        if run_seed != ours {
            return Err(PyValueError::new_err(format!(
                "debug_set_rng_counters: run_seed {run_seed} is not this run's \
                 seed ({ours}) — these cursors belong to a different run"
            )));
        }
        let mut ignored: Vec<String> = Vec::new();
        for (key, value) in counters.iter() {
            let name: String = key.extract()?;
            let target: u32 = value.extract()?;
            let Some(stream) = self.inner.run_state.rng.stream_mut(&name) else {
                ignored.push(name);
                continue;
            };
            let current = stream.counter();
            if target < current {
                return Err(PyValueError::new_err(format!(
                    "debug_set_rng_counters: stream {name:?} is already at \
                     {current}, cannot rewind to {target}"
                )));
            }
            stream.fast_forward_counter(target);
        }
        ignored.sort();
        Ok(ignored)
    }

    /// Place this run on `act`'s map at `(col, row)` without walking to it.
    ///
    /// The positioning half of a live-state import — see
    /// [`sts2_core::run_manager::RunManager::debug_place_on_map`] for what
    /// placement does and does not do. `visited` is the world's own
    /// visited-coord list as `(col, row)` pairs, recorded verbatim rather
    /// than inferred, because the live wire carries it exactly.
    ///
    /// Debug/reconstruction surface, not a parity surface.
    ///
    /// Raises `ValueError` for an act outside this run's range, an act the
    /// run has already passed, or a coord that is not a live node on that
    /// act's map.
    #[pyo3(signature = (act, col, row, visited, act_floor, total_floor))]
    fn debug_place_on_map(
        &mut self,
        act: u8,
        col: i32,
        row: i32,
        visited: Vec<(i32, i32)>,
        act_floor: i32,
        total_floor: i32,
    ) -> PyResult<()> {
        self.inner
            .debug_place_on_map(
                act,
                sts2_core::map::MapCoord::new(col, row),
                visited
                    .into_iter()
                    .map(|(c, r)| sts2_core::map::MapCoord::new(c, r))
                    .collect(),
                act_floor,
                total_floor,
            )
            .map_err(|e| PyValueError::new_err(format!("debug_place_on_map: {e}")))
    }

    /// Per-act pre-generated room contents (`run_room_gen::generate_all_rooms`):
    /// the C#-faithful `up_front` GenerateRooms output. Each entry is
    /// `{"normal": [id, ...], "elite": [id, ...], "boss": id}` with `UPPER_SNAKE`
    /// encounter ids matching the headless `dump_rooms` `Id.Entry`s. Ground truth
    /// for `run_parity.py --encounter-parity`. Bead `sts2-rs-jenwp` (M3).
    fn generated_rooms<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        crate::value_to_pylist(py, &emit_rm::generated_rooms(&self.inner))
    }

    /// Roll one card reward off the fresh `rewards` stream (counter 0)
    /// and return each option's `{id, upgraded}`. Mirrors the C#
    /// `dump_card_reward` headless command (`CardFactory.CreateForReward`
    /// via `CardCreationOptions.ForRoom`) so `run_parity.py
    /// --reward-parity` can compare the reward RNG + character-pool
    /// ordering with no combat plumbing. MUTATES the rewards stream +
    /// pity counter — call once per fresh `RunManager`. `room_type` is
    /// `"Monster"` (regular) / `"Elite"` / `"Boss"`. M4.
    #[pyo3(signature = (count = 3, room_type = "Monster"))]
    fn dump_card_reward<'py>(
        &mut self,
        py: Python<'py>,
        count: u8,
        room_type: &str,
    ) -> PyResult<Bound<'py, PyList>> {
        let context = match room_type {
            "Elite" | "elite" => "elite",
            "Boss" | "boss" => "boss",
            _ => "regular",
        };
        let opts = sts2_core::runstate::CardRewardGenerationOptions {
            context,
            num_cards: count,
            ..Default::default()
        };
        let character = self.inner.character_id;
        let cards = sts2_content::cards::spec_index::roll_card_reward_cards(
            &mut self.inner.run_state,
            opts,
            character,
        );
        let list = PyList::empty(py);
        for c in &cards {
            let d = PyDict::new(py);
            d.set_item("id", c.card_id.to_python_name())?;
            d.set_item("upgraded", c.upgraded)?;
            list.append(d)?;
        }
        Ok(list)
    }

    /// Roll `count` relic rewards off the fresh `rewards` stream (counter 0),
    /// returning each relic's `{id}` (PascalCase `Debug` form). Mirrors the C#
    /// `dump_relic_reward` headless command (`RelicFactory.PullNextRelicFromFront`
    /// per pull: `RollRarity(player.PlayerRng.Rewards)` then a front-of-bucket
    /// grab-bag pop). Lets `run_parity.py --relic-reward-parity` compare the
    /// reward-stream rarity roll + grab-bag ordering with no combat plumbing.
    /// MUTATES the rewards stream + grab bag — call once per fresh `RunManager`.
    /// M4.
    #[pyo3(signature = (count = 1))]
    fn dump_relic_reward<'py>(
        &mut self,
        py: Python<'py>,
        count: u32,
    ) -> PyResult<Bound<'py, PyList>> {
        let rs = &mut self.inner.run_state;
        let list = PyList::empty(py);
        for _ in 0..count {
            let rarity = rs.roll_reward_relic_rarity();
            if let Some(id) = rs.pop_relic_from_grab_bag(rarity) {
                let d = PyDict::new(py);
                d.set_item("id", format!("{id:?}"))?;
                list.append(d)?;
            }
        }
        Ok(list)
    }

    /// Generate a full normal-merchant inventory off the fresh `shops` +
    /// `rewards` streams and return its card / colorless / relic / potion
    /// rows + the Shops/Rewards read counts. Mirrors the C# `dump_shop`
    /// headless command (`MerchantInventory.CreateForNormalMerchant`) so
    /// `run_parity.py --shop-parity` can compare item identities, prices,
    /// on-sale slot, and per-stream read counts with no combat plumbing.
    /// MUTATES both streams — call once per fresh `RunManager`. M4.
    fn dump_shop<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let character = self.inner.run_state.character_id;
        let shops0 = self.inner.run_state.rng.shops.counter();
        let rewards0 = self.inner.run_state.rng.rewards.counter();
        let inv = sts2_content::cards::spec_index::generate_merchant_inventory(
            &mut self.inner.run_state,
            character,
        );
        let shops_reads = self.inner.run_state.rng.shops.counter() - shops0;
        let rewards_reads = self.inner.run_state.rng.rewards.counter() - rewards0;

        let card_dict =
            |py: Python<'py>, e: &sts2_core::shop::ShopCardEntry| -> PyResult<Py<PyDict>> {
                let d = PyDict::new(py);
                d.set_item("id", e.card.as_ref().map(|c| c.card_id.to_python_name()))?;
                d.set_item("upgraded", e.card.as_ref().is_some_and(|c| c.upgraded))?;
                d.set_item("on_sale", e.on_sale)?;
                d.set_item("price", e.price)?;
                Ok(d.into())
            };

        let cards = PyList::empty(py);
        for e in &inv.cards {
            cards.append(card_dict(py, e)?)?;
        }
        let colorless = PyList::empty(py);
        for e in &inv.colorless_cards {
            colorless.append(card_dict(py, e)?)?;
        }
        let relics = PyList::empty(py);
        for e in &inv.relics {
            let d = PyDict::new(py);
            d.set_item("id", e.relic_id.map(|r| format!("{r:?}")))?;
            d.set_item("price", e.price)?;
            relics.append(d)?;
        }
        let potions = PyList::empty(py);
        for e in &inv.potions {
            let d = PyDict::new(py);
            d.set_item("id", e.potion_kind.map(|k| format!("{k:?}")))?;
            d.set_item("price", e.price)?;
            potions.append(d)?;
        }

        let out = PyDict::new(py);
        out.set_item("cards", cards)?;
        out.set_item("colorless", colorless)?;
        out.set_item("relics", relics)?;
        out.set_item("potions", potions)?;
        out.set_item("removal_cost", inv.removal_cost)?;
        out.set_item("shops_reads", shops_reads)?;
        out.set_item("rewards_reads", rewards_reads)?;
        Ok(out)
    }

    /// Spawn `encounter_id`'s roster off the fresh `niche` stream (counter 0
    /// on a just-constructed `RunManager`) and return each enemy's
    /// `{id, max_hp, current_hp}` in spawn order. Mirrors the C# ground truth
    /// (`enter_room` + `get_state` on a fresh run, `niche@0`) used by
    /// `run_parity.py --roster-parity`: C# `CombatState.CreateCreature` rolls
    /// every enemy's HP off `RunState.Rng.Niche`. `id` is the canonical
    /// monster id (`MonsterKind::python_name`, matching C# `Monster.Id.Entry`
    /// after normalization). MUTATES the `niche` stream — call once per fresh
    /// `RunManager`. Returns an empty list for an unknown encounter id. M3b.
    fn dump_roster<'py>(
        &mut self,
        py: Python<'py>,
        encounter_id: &str,
    ) -> PyResult<Bound<'py, PyList>> {
        let list = PyList::empty(py);
        if let Some(roster) = sts2_content::run_callbacks::dump_roster_for_encounter(
            &mut self.inner.run_state,
            encounter_id,
        ) {
            for (id, max_hp, current_hp) in roster {
                let d = PyDict::new(py);
                d.set_item("id", id)?;
                d.set_item("max_hp", max_hp)?;
                d.set_item("current_hp", current_hp)?;
                list.append(d)?;
            }
        }
        Ok(list)
    }

    #[getter]
    fn current_hp(&self) -> i32 {
        self.inner.run_state.current_hp
    }

    #[getter]
    fn max_hp(&self) -> i32 {
        self.inner.run_state.max_hp
    }

    #[getter]
    fn gold(&self) -> i32 {
        emit_rm::gold(&self.inner)
    }

    #[getter]
    fn act_index(&self) -> u8 {
        self.inner.run_state.act_index
    }

    #[getter]
    fn ascension_level(&self) -> u8 {
        self.inner.run_state.ascension_level
    }

    /// Debug/coverage override mirroring the C# headless `set_player`
    /// scalar subset (HP / max-HP / gold). Used by the invincible-lockstep
    /// act-2/3 sampling driver to pin both engines to identical HP so a
    /// random-policy run survives into the later acts. Propagates HP into
    /// the live combat creature when a fight is active. Never call on a
    /// trace-recorded run — it perturbs combat state.
    #[pyo3(signature = (hp = None, max_hp = None, gold = None))]
    fn set_player(&mut self, hp: Option<i32>, max_hp: Option<i32>, gold: Option<i32>) {
        self.inner.debug_set_player(hp, max_hp, gold);
    }

    #[getter]
    fn player_won(&self) -> bool {
        self.inner.run_state.player_won
    }

    /// List of owned relics as the C# game-style `RELIC.<UPPER_SNAKE>`
    /// identifiers — matches the format emitted by scraped `.run`
    /// files at `../sts2_run_scraper/data/files/`. Bead `sts2-rs-plrg2`.
    fn relics(&self) -> Vec<String> {
        emit_rm::relics(&self.inner)
    }

    /// Potion belt — the potions the player currently holds, as
    /// `PascalCase` `PotionKind` debug names (e.g. `"FirePotion"`). The
    /// workbench's `display_maps.loc_id` normalizes these to the
    /// SCREAMING_SNAKE loc-key / art stem. Empty when the belt is empty.
    ///
    /// During combat the live belt is the combat-side
    /// `CombatState::player.potions` (seeded from the run belt at
    /// `enter_combat`, written back at `exit_combat`), so a potion drunk
    /// mid-fight disappears from this list immediately — without it, the
    /// UI would keep showing a spent potion until combat ended. Outside
    /// combat the run-level belt is authoritative. Bead `sts2-rs-01o`.
    ///
    /// The combat belt is only authoritative while combat is actually
    /// LIVE ([`Phase::Combat`]). After a non-boss fight, `self.inner.combat`
    /// is NOT torn down — it lingers (with `is_over == true`) through the
    /// card-reward / map / event phases until the next `enter_combat`
    /// replaces it. Reading that stale combat belt during those phases
    /// would shadow `run_state.potions`, hiding any potion the combat-end
    /// reward stream just dropped (it lands on the run belt, then seeds
    /// the next combat). So gate on the phase, not merely `combat.is_some()`.
    fn potions(&self) -> Vec<String> {
        emit_rm::potions(&self.inner)
    }

    /// Maximum potion-belt slots (run-side: A4 TightBelt and any
    /// `gain_potion_slots` effects included). The PPO env masks
    /// `shop_buy_potion` when the belt is full — the engine deliberately
    /// accepts that action as a C#-lockstep graceful no-op
    /// (`do_shop_buy_potion` `PotionBeltFull => Advanced`), which would
    /// deadlock a greedy argmax policy.
    fn max_potion_slots(&self) -> i32 {
        self.inner.run_state.max_potion_slots
    }

    fn master_deck(&self) -> Vec<String> {
        emit_rm::master_deck(&self.inner)
    }

    /// The live merchant inventory while in `Phase::Shop`, or `None`.
    ///
    /// Surfaces concrete ids + prices so a UI host can render names/costs
    /// (the `shop_buy_*` actions carry only an index). Card entries combine
    /// the character row then the colorless row, so `idx` matches the
    /// `shop_buy_card` index space. `available` is `true` for in-stock,
    /// payload-bearing slots (sold-out slots have a sentinel price).
    fn shop_inventory(&self, py: Python<'_>) -> PyResult<PyObject> {
        crate::value_to_py(py, &emit_rm::shop_inventory(&self.inner)).map(Bound::unbind)
    }

    /// Master deck with per-card upgrade flag as `[(id_str, upgraded), …]`.
    /// Used by the R-Act1-Replay probe to verify that on-obtain relic
    /// listeners like Whetstone actually applied their upgrades, which
    /// the bare `master_deck()` string formatter elides.
    fn master_deck_detailed(&self) -> Vec<(String, bool)> {
        emit_rm::master_deck_detailed(&self.inner)
    }

    /// `(card_id, upgraded)` for the pending deck-removal offer, in C#
    /// `FromDeckForRemoval` order (removable cards only, Curse-type sorted to
    /// the front). The workbench renders these as the removal `card_select`
    /// options; the chosen index is a position in THIS list, which
    /// `remove_deck_card` maps back to the raw master-deck position. Mirrors
    /// `RunState::removal_offer_order`. Bead: deck-removal curse-front parity.
    fn card_removal_options(&self) -> Vec<(String, bool)> {
        emit_rm::card_removal_options(&self.inner)
    }

    /// Append a card to the master deck by id (e.g. `"CARD.STRIKE"` or
    /// `"STRIKE"`). Bypasses the card-reward sampler entirely so a
    /// replay driver can inject the human's actual picks even when the
    /// engine's reward RNG drifts from the C# pin (bead
    /// `sts2-rs-i6h9i.1.1`). Mirrors the deck-side effect of
    /// `RunManager::do_card_reward_pick` (see `run_manager.rs:1078`).
    ///
    /// Does NOT advance phase or consume a pending reward — call after
    /// taking a `pick_card_reward {idx: None}` (skip) so the engine's
    /// reward queue drains while the deck still matches the human's.
    /// Bead `sts2-rs-i6h9i.1`.
    fn force_add_card(&mut self, card_id: &str) -> PyResult<()> {
        let cid = card_id_from_str(card_id)?;
        // Build via the registered factory so spec stats (cost / damage /
        // block / keywords) are populated, matching the real card-reward
        // path; fall back to a bare POD only if no spec is known.
        let card = sts2_core::lookup_card_factory()
            .and_then(|f| f(cid, false))
            .unwrap_or_else(|| {
                let mut c = sts2_core::cards::Card::pod_defaults();
                c.card_id = cid;
                c
            });
        self.inner.run_state.master_deck.push(card);
        Ok(())
    }

    /// Upgrade the first un-upgraded master-deck entry matching
    /// `card_id`. Used by the `act1_replay` driver to mirror scraped
    /// smith picks when the engine's smith RNG / surface can't be
    /// driven to the exact human-chosen card. Returns `True` on a
    /// successful upgrade, `False` if no matching un-upgraded card
    /// exists. Bead `sts2-rs-i6h9i.1`.
    fn force_upgrade_card(&mut self, card_id: &str) -> PyResult<bool> {
        let cid = card_id_from_str(card_id)?;
        for c in &mut self.inner.run_state.master_deck {
            if c.card_id == cid && !c.upgraded {
                c.upgraded = true;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Remove the first master-deck entry matching `card_id`. Accepts the
    /// scraped `"CARD.DOUBT"` / bare `"DOUBT"` forms **and** the Debug
    /// enum name emitted by [`Self::master_deck_detailed`] (e.g.
    /// `"AscendersBane"`), because `card_id_from_str` falls through to a
    /// case/underscore-insensitive `normalize` match. This lets the
    /// `act1_replay` deck-reconcile both replay scraped `cards_removed`
    /// (shop / event card-removal services) and strip engine-side cards
    /// the human's run never carried — the missing inverse of
    /// [`Self::force_add_card`]. Prefers removing an un-upgraded copy so a
    /// later [`Self::force_upgrade_card`] reconcile still has an
    /// upgradable target. Returns `True` if a card was removed, `False`
    /// if no matching entry exists. Bead `sts2-rs-i6h9i.1`.
    fn force_remove_card(&mut self, card_id: &str) -> PyResult<bool> {
        let cid = card_id_from_str(card_id)?;
        let deck = &mut self.inner.run_state.master_deck;
        let pos = deck
            .iter()
            .position(|c| c.card_id == cid && !c.upgraded)
            .or_else(|| deck.iter().position(|c| c.card_id == cid));
        if let Some(pos) = pos {
            deck.remove(pos);
            return Ok(true);
        }
        Ok(false)
    }

    /// Grant a relic to the player by id (e.g. `"RELIC.WHETSTONE"` or
    /// `"WHETSTONE"`), bypassing the relic-reward sampler. Mirrors
    /// the side effect of `RunState::accept_relic_reward` (see
    /// `runstate.rs:1606`):
    ///
    /// 1. Pushes the [`RelicId`] onto `run_state.relics`.
    /// 2. Stashes `last_obtained_relic` so per-relic listeners can
    ///    self-gate (Astrolabe et al.).
    /// 3. Fires the `after_obtained` dispatch directly (no
    ///    `with_deferred_followup_rewards` wrap), so listeners that
    ///    grant follow-up effects apply them eagerly: Whetstone
    ///    auto-upgrades 2 random attacks, `WarPaint` auto-upgrades
    ///    Skills, etc. The deferred path (which queues a
    ///    player-pickable reward instead) is wrong for the replay
    ///    driver — there is no human pick to mirror, and the queued
    ///    reward never gets resolved by the dispatcher, silently
    ///    no-op'ing the relic.
    ///
    /// Returns `True` on success. Returns `False` if the player already
    /// owns the relic (idempotent — STS-style relics don't stack).
    ///
    /// Used by the R-Act1-Replay driver (`act1_replay.py` W8.5) to
    /// mirror scraped `relic_choices[was_picked]` across every floor
    /// type without going through the diverged relic-reward RNG.
    /// Bead `sts2-rs-i6h9i.1`.
    fn force_add_relic(&mut self, relic_id: &str) -> PyResult<bool> {
        let stripped = relic_id.strip_prefix("RELIC.").unwrap_or(relic_id);
        let rid = sts2_content::names::relic_id_from_str(stripped)
            .ok_or_else(|| PyValueError::new_err(format!("unknown relic id {relic_id:?}")))?;
        let rs = &mut self.inner.run_state;
        if rs.relics.contains(&rid) {
            return Ok(false);
        }
        rs.relics.push(rid);
        rs.last_obtained_relic = Some(rid);
        let _ = sts2_core::run_effects::dispatch_after_obtained(rs);
        rs.last_obtained_relic = None;
        Ok(true)
    }

    /// Active-event option list as `[(option_id, label), …]`, or `None`
    /// when no event is currently materialized (i.e.
    /// `RunState::current_event` is `None`). Mirrors
    /// `sts2_content::events::dispatch::present`. Bead `sts2-rs-h9qn3`
    /// (W4): lets Python drivers map scraped `TextKey` strings
    /// (`<EVENT_ID>.pages.INITIAL.options.<OPT>.title`) to the engine
    /// option index by lower-casing the trailing path segment and
    /// looking it up against the returned `option_id` column.
    fn event_options(&self) -> Option<Vec<(String, String)>> {
        emit_rm::event_options(&self.inner)
    }

    /// Rich active-event option list as
    /// `[{"option_id", "label", "description", "enabled"}, …]`, or
    /// `None` when no event is materialized. Unlike
    /// [`Self::event_options`] (which returns only `(option_id, label)`
    /// tuples), this surfaces the per-choice long-form `description`
    /// (e.g. "Take 5 damage, gain a colorless card.") and the `enabled`
    /// gate so a UI can render the full choice screen the C# game
    /// shows on arrival.
    ///
    /// Also carries `loc_key` + `vars`: the game's own localization path
    /// for the choice and the values for its `{VarName}` placeholders. A
    /// display layer should PREFER those — `label` / `description` are
    /// developer shorthand written for logs, and read as such next to the
    /// real game ("Gain a random potion." vs "Procure 1 random Potion.").
    fn event_option_infos(&self, py: Python<'_>) -> PyResult<PyObject> {
        crate::value_to_py(py, &emit_rm::event_option_infos(&self.inner)).map(Bound::unbind)
    }

    /// Materialize the random event for the room the player is standing
    /// in: if `RunState::current_event` is `None`, roll the next allowed
    /// event from the act pool ([`dispatch::pick_event`]) and bind it via
    /// `enter_event_room`, so [`Self::event_options`] /
    /// [`Self::event_option_infos`] return the real choices immediately.
    ///
    /// The C# game presents the event (name, body, options) on arrival;
    /// the Rust port otherwise defers the pick until the first
    /// `EventChoice` resolves, leaving a UI with nothing to show. This
    /// lets a driver present the event up front. Idempotent — a no-op
    /// when an event is already bound (so it never double-advances the
    /// act's `events_visited` cursor). The deterministic, RNG-free pick
    /// matches what the lazy resolve path would have selected.
    ///
    /// Returns the bound event's Python class name (e.g. "LostWisp"),
    /// or `None` when no event is active and none could be picked.
    fn materialize_event(&mut self) -> Option<String> {
        if self.inner.run_state.current_event.is_none() {
            let id = sts2_content::events::dispatch::pick_event(&mut self.inner.run_state, None)?;
            self.inner.run_state.enter_event_room(id);
        }
        self.inner
            .run_state
            .current_event
            .map(|id| id.python_name().to_string())
    }

    /// Python class name of the currently-bound event (e.g.
    /// "LostWisp"), or `None` outside an event room.
    fn current_event_name(&self) -> Option<String> {
        self.inner
            .run_state
            .current_event
            .map(|id| id.python_name().to_string())
    }

    /// Force-bind a SPECIFIC event by its Python class name (e.g.
    /// "LostWisp"), bypassing the act-pool draw, so
    /// [`Self::event_option_infos`] returns that event's options
    /// immediately. This is the Rust analogue of the C# headless
    /// `enter_room {type: event, event: <Id.Entry>}` command
    /// (`RunSimulator.EnterRoomImpl`) — it lets `event_parity.py`
    /// present any event's option list on either engine WITHOUT a map
    /// walk or combat, so option-level parity can be checked across all
    /// acts. Overwrites any currently-bound event (including the
    /// run-start Neow), and does NOT advance the act `events_visited`
    /// cursor (it is not a pool pick). Returns the bound event's Python
    /// class name, or `None` if `name` resolves to no known `EventId`.
    fn force_event(&mut self, name: &str) -> Option<String> {
        let id = sts2_core::EventId::from_python_name(name)?;
        self.inner.run_state.enter_event_room(id);
        Some(id.python_name().to_string())
    }

    fn card_reward_options<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        crate::value_to_pylist(py, &emit_rm::card_reward_options(&self.inner))
    }

    /// Currently-offered card-reward `CardId` list as
    /// `["CARD.<UPPER_SNAKE>", …]` strings — matches the format
    /// emitted by scraped `.run` files at
    /// `../sts2_run_scraper/data/files/`. Empty when no card-reward
    /// fan-out is pending (e.g. relic-only reward, or outside the
    /// `CardReward` phase entirely).
    ///
    /// Bead `sts2-rs-ze744` (W5a — R-Act1-Replay). Index aligns with
    /// `PickCardReward{Some(idx)}` — Python drivers can match scraped
    /// `card_choices[*].card_id` against this list to find `idx`, then
    /// dispatch the action. A scraped `card_choices` entry with no
    /// pick maps to `PickCardReward{idx: None}` (skip).
    fn card_reward_option_ids(&self) -> Vec<String> {
        emit_rm::card_reward_option_ids(&self.inner)
    }

    /// Per-option upgrade flags, index-aligned with
    /// [`Self::card_reward_option_ids`] — `roll_for_upgrade` pre-upgrades
    /// non-rare reward options in acts 2+ (`card_rewards.rs`), and
    /// SilverCrucible stamps its own. Shorter than the ids list (or empty)
    /// means "not upgraded": bundle and choose-screen offerings carry no
    /// upgrade state, per `RunManager::card_reward_upgraded_flags`.
    ///
    /// The heuristic run policy has always read this (`runai::view`); the PPO
    /// obs could not, so a free upgraded reward featurized as the base card
    /// (GH #545).
    fn card_reward_option_upgraded(&self) -> Vec<bool> {
        emit_rm::card_reward_option_upgraded(&self.inner)
    }

    /// When the current `CardReward` offering is a Scroll Boxes bundle pick,
    /// returns the packs as `[["CARD.<UPPER_SNAKE>", …], …]` — one inner list
    /// per selectable pack, index-aligned with `PickCardReward{Some(i)}`.
    /// Returns `None` for an ordinary single-card reward, so a Python backend
    /// can branch to a `bundle_select` decision when this is present.
    fn card_reward_bundles(&self) -> Option<Vec<Vec<String>>> {
        emit_rm::card_reward_bundles(&self.inner)
    }

    fn map_coord(&self) -> Option<(i32, i32)> {
        emit_rm::map_coord(&self.inner)
    }

    fn act_floor(&self) -> i32 {
        self.inner.run_state.act_floor
    }

    fn total_floor(&self) -> i32 {
        self.inner.run_state.total_floor
    }

    /// Canonical `UPPER_SNAKE` encounter id of the pre-rolled act boss
    /// (`RunState::pinned_act_boss`), resolvable by
    /// `sts2_rs.build_scenario_state(encounter=…)`, or `None` when no
    /// boss is pinned / the pin has no content `EncounterDef` row.
    ///
    /// Act 1 pins index `ACT1_ENCOUNTERS` directly
    /// (`pre_roll_act_boss` enumerates before filtering, so
    /// `encounter_index` is a full-table index). Act 2/3 pins carry the
    /// boss-pool `EncounterSpec.name` (snake_case), which matches its
    /// `EncounterDef.id` uppercased. Lets a run-policy host simulate
    /// "this loadout vs the upcoming boss" candidate fights.
    fn pinned_boss_encounter_id(&self) -> Option<String> {
        let pin = self.inner.run_state.pinned_act_boss.as_ref()?;
        if pin.act_index == 1 {
            // GH #544: index the act-1 table THIS RUN walks. Hardcoding the
            // Overgrowth table here reported an Overgrowth boss for Underdocks
            // runs — a wrong answer, not an error, so a host asking "what am I
            // about to fight?" prepared for the wrong boss.
            return sts2_content::encounters::act1_encounters(self.inner.run_state.act1)
                .get(pin.encounter_index)
                .map(|d| d.id.to_string());
        }
        let upper = pin.display_name.to_uppercase();
        sts2_content::encounters::find(&upper).map(|d| d.id.to_string())
    }

    /// Fast-forward the combat this run is currently in through the combat
    /// surrogate, instead of playing it out.
    ///
    /// The PPO screening env's combat backend (`ppo/env.py`, `combat=
    /// "surrogate"`): one forward pass per fight in place of a 1000-sim MCTS
    /// search per decision. Rewards are unaffected — the fight is stamped
    /// terminal and the ordinary `exit_combat_and_transition` generates gold /
    /// potions / card rewards exactly as a played fight would.
    ///
    /// `outcome_base` seeds the Bernoulli survival draw; pass a per-run
    /// constant so a paired screen's two arms see the same outcome wherever
    /// they reach the same fight. Returns
    /// `{win_prob, hp_frac_lost, survived, hp_before, hp_after}`.
    ///
    /// Raises if the run is not in `Phase::Combat`.
    fn surrogate_resolve_combat<'py>(
        &mut self,
        py: Python<'py>,
        surrogate: &crate::combat_surrogate::PyCombatSurrogate,
        outcome_base: u64,
    ) -> PyResult<Bound<'py, PyDict>> {
        crate::combat_surrogate::resolve_current_combat(
            py,
            &mut self.inner,
            surrogate,
            outcome_base,
        )
    }

    fn current_room(&self, py: Python<'_>) -> PyResult<PyObject> {
        let value = emit_rm::current_room(&self.inner);
        if value.is_null() {
            return Ok(py.None());
        }
        let d = crate::value_to_pydict(py, &value)?;
        // `map_coord` is a `(col, row)` tuple on this surface; see `pair_to_tuple`.
        if let Some(coord) = d.get_item("map_coord")? {
            if !coord.is_none() {
                d.set_item("map_coord", pair_to_tuple(py, &coord)?)?;
            }
        }
        Ok(d.into_any().unbind())
    }

    /// Snapshot of the in-flight combat as a [`crate::PyCombatState`],
    /// or `None` outside [`Phase::Combat`]. Returns a CLONE — mutating
    /// the returned `CombatState` does not affect this `RunManager`.
    /// Callers wanting to advance combat should keep using
    /// `take_action({"action": "play_card", ...})` / `{"action":
    /// "end_turn"}` on the `RunManager`. The clone is intended for
    /// read-only inspection and for feeding `sts2_rs.Mcts(cs, …)` to
    /// get hint recommendations that the caller then translates back
    /// into `RunAction` dicts.
    fn combat_state(&self) -> Option<crate::PyCombatState> {
        self.inner.combat.as_ref().map(|c| {
            let seed_u64 = u64::from(u32::from_ne_bytes(self.seed.to_ne_bytes()));
            crate::PyCombatState::from_inner(c.clone(), seed_u64)
        })
    }

    /// Run MCTS on a clone of the active combat and return the
    /// highest-visit action as a `RunAction` dict ready to feed back
    /// into [`Self::take_action`], or `None` when no combat is active
    /// or the action MCTS picks is not expressible at the run-action
    /// surface. `play_card` / `end_turn` / `drink_potion` /
    /// `combat_resolve_choice` all translate; only orb actions
    /// (`Channel` / `Evoke`, never emitted by `legal_actions`) fall
    /// through to `None`.
    ///
    /// Defaults track auto-memory `project_mcts_budget_knee`:
    /// `sims=1000` and `depth=10` is the published knee for Act-1
    /// normals/elites; bump `sims` to ~3000 for Act-1 bosses. Bead
    /// `sts2-rs-yb3cm` (W6).
    #[pyo3(signature = (sims = 1000, depth = 10, c_puct = std::f32::consts::SQRT_2, seed = 0, threads = 8))]
    fn mcts_pick_action(
        &self,
        py: Python<'_>,
        sims: u32,
        depth: u32,
        c_puct: f32,
        seed: u64,
        threads: u32,
    ) -> PyResult<PyObject> {
        let Some(combat) = self.inner.combat.as_ref() else {
            return Ok(py.None());
        };
        let cfg = sts2_mcts::MctsConfig {
            c_puct,
            max_simulations: sims,
            max_rollout_depth: depth,
            seed,
            ..sts2_mcts::MctsConfig::default()
        };
        let root = combat.clone();
        // Root-parallel by default (threads=8): independent trees on root
        // clones, merged root-child counters, argmax-Q best action — same
        // selection rule as the single-threaded path, deterministic in
        // (seed, threads, sims). `threads<=1` keeps the no-thread-pool path.
        let chosen = py.allow_threads(move || {
            if threads <= 1 {
                let mut mcts = sts2_mcts::Mcts::new(root, cfg);
                mcts.search()
            } else {
                sts2_mcts::par_search(root, cfg, threads).best_action
            }
        });
        crate::value_to_py(
            py,
            &sts2_wire::emit::run_action::combat_action_to_run_action_value(&chosen),
        )
        .map(Bound::unbind)
    }

    /// Dump the current act's `ActMap` as a list of point dicts. Used
    /// by R3.2 corpus parity tooling — a fresh `RunManager` already
    /// has the Act-1 map generated (see [`run_manager.rs:373`]'s
    /// `run_state.generate_map()` call). Each entry shape:
    ///
    /// ```python
    /// {
    ///   "col": int, "row": int,
    ///   "point_type": "monster" | "elite" | "boss" | "shop" | ...,
    ///   "children": [(col, row), ...],
    /// }
    /// ```
    ///
    /// Returns `None` if the run has no current act map (which
    /// shouldn't happen for a freshly-constructed `RunManager` but
    /// is possible mid-run between acts). Bead `sts2-rs-5hsqp`.
    fn act_map(&self, py: Python<'_>) -> PyResult<PyObject> {
        act_map_to_py(py, &emit_rm::act_map(&self.inner))
    }

    /// Generate the ActMap for an ARBITRARY 1-based act (1/2/3) on this
    /// run's seed and serialize it in the same flat schema as
    /// [`Self::act_map`], WITHOUT advancing the run. The map graph is a
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
    /// to call for any act in any order. Returns `None` for an out-of-
    /// range act. Intended for a throwaway `RunManager` used only to dump.
    fn act_map_for(&mut self, py: Python<'_>, act: u8) -> PyResult<PyObject> {
        act_map_to_py(py, &emit_rm::act_map_for(&mut self.inner, act))
    }

    /// Dump the current act's map in the C# `full_map` schema the sts2-pycli
    /// web UI consumes (`web/src/api.ts` `FullMap`, rendered by
    /// `MapView.tsx`). Shape:
    ///
    /// ```python
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
    /// `PascalCase` to match the UI's `NODE` glyph table. Returns `None` when
    /// no act map is loaded (mid-act transition). Distinct from `act_map()`,
    /// which keeps its flat lowercase shape for corpus parity tooling.
    fn full_map(&self, py: Python<'_>) -> PyResult<PyObject> {
        crate::value_to_py(py, &emit_rm::full_map(&self.inner)).map(Bound::unbind)
    }

    /// Optionality-aware path plan for the current map, for the workbench
    /// overlay (`web/src/components/MapView.tsx`). The run-AI pathing surface
    /// plans the WHOLE route to the boss and replans every floor; this dumps
    /// that plan so the UI can draw it. Shape:
    ///
    /// ```python
    /// {
    ///   "best_col": int,            # optionality first-move column
    ///   "baseline_col": int,        # legacy hard-max first-move column
    ///   "diverged": bool,           # optionality re-routed vs the baseline
    ///   "route": [{"col": int, "row": int}, ...],  # planned route to boss
    ///   "first_col": int | None,    # the seeded first hop, if one was given
    ///   "columns": [{              # one per legal first move, ascending col
    ///       "col": int,
    ///       "score": float,         # optionality-aware column score
    ///       "baseline_score": float,# legacy hard-max column score
    ///       "n_paths": int,         # distinct routes to the boss through it
    ///       "min_elites": int, "max_elites": int,
    ///       "can_skip_elite": bool, # keeps both an elite-free and an
    ///                               # elite-bearing route open
    ///       "is_pick": bool,        # == best_col
    ///   }, ...],
    /// }
    /// ```
    ///
    /// `first_col` seeds the route's FIRST hop instead of letting the planner
    /// choose it (the rest of the route is planned normally from there). The
    /// workbench passes the column the *driving* run policy actually picked, so
    /// the line drawn on the map is the continuation of the move that will
    /// happen — with the PPO brain selected, this surface's own first pick can
    /// differ from the net's, and the map then argued with the policy panel
    /// beside it. An illegal or unknown column is ignored.
    ///
    /// Returns `None` when no act map is loaded or the map has no legal first
    /// move (e.g. mid-act transition). Uses the embedded default
    /// [`sts2_runai::eval::HeuristicV`] weights — the same the deployed
    /// tier-1 policy uses. Read-only; consumes no RNG.
    #[pyo3(signature = (first_col = None))]
    fn path_plan(&self, py: Python<'_>, first_col: Option<i32>) -> PyResult<PyObject> {
        use sts2_runai::eval::{HeuristicV, OptionalityWeights};
        use sts2_runai::policy::surfaces::pathing;
        use sts2_runai::view::VisibleRunView;

        let view = VisibleRunView::from_run_manager(&self.inner);
        if view.map.is_none() {
            return Ok(py.None());
        }
        let v = HeuristicV::default();
        let Some((best_col, opt_cols)) = pathing::scored_first_cols(&view, &v) else {
            return Ok(py.None());
        };
        // Baseline arm: the pre-feature hard-max aggregator (no flexibility
        // valuation) — surfaced so the UI can show what optionality changed.
        let baseline = pathing::scored_first_cols_opt(
            &view,
            &v,
            &std::collections::BTreeMap::new(),
            &OptionalityWeights::hard_max(),
        );
        let baseline_col = baseline.as_ref().map_or(best_col, |(c, _)| *c);
        let baseline_scores: std::collections::BTreeMap<i32, f64> = baseline
            .map(|(_, cols)| cols.into_iter().collect())
            .unwrap_or_default();
        let stats: std::collections::BTreeMap<i32, pathing::ColumnOptionality> =
            pathing::column_optionality(&view)
                .unwrap_or_default()
                .into_iter()
                .collect();

        let out = PyDict::new(py);
        // `best_col` stays THIS surface's own pick even when the route is
        // seeded elsewhere: it is what the columns table is scored against, and
        // the seeded first hop is reported by `route[0]` (plus `first_col`
        // below) so a caller can still see the two disagree.
        out.set_item("best_col", best_col)?;
        out.set_item("first_col", first_col)?;
        out.set_item("baseline_col", baseline_col)?;
        out.set_item("diverged", best_col != baseline_col)?;

        let route = PyList::empty(py);
        for c in pathing::planned_route_from(&view, &v, first_col) {
            let d = PyDict::new(py);
            d.set_item("col", c.col)?;
            d.set_item("row", c.row)?;
            route.append(d)?;
        }
        out.set_item("route", route)?;

        let columns = PyList::empty(py);
        for (col, score) in opt_cols {
            let st = stats.get(&col).copied().unwrap_or_default();
            let cd = PyDict::new(py);
            cd.set_item("col", col)?;
            cd.set_item("score", score)?;
            cd.set_item("baseline_score", baseline_scores.get(&col).copied())?;
            cd.set_item("n_paths", st.n_paths)?;
            cd.set_item("min_elites", st.min_elites)?;
            cd.set_item("max_elites", st.max_elites)?;
            cd.set_item("can_skip_elite", st.can_skip_elite())?;
            cd.set_item("is_pick", col == best_col)?;
            columns.append(cd)?;
        }
        out.set_item("columns", columns)?;
        Ok(out.into_any().unbind())
    }

    /// Infer the per-row column sequence for a scraped `(map_point_type)`
    /// walk through the **current** act map. `scraped_room_types` is a
    /// list of lowercase room-type strings (`"monster"`, `"elite"`,
    /// `"boss"`, `"shop"`, `"rest_site"`, `"treasure"`, `"ancient"`,
    /// `"unknown"`) — one entry per row, length must equal the act map's
    /// `map_length` (i.e. covers rows `1..=map_length` inclusive of the
    /// boss row).
    ///
    /// Returns the `Vec<i32>` column sequence the player visited. Raises
    /// `PyRuntimeError` on any [`sts2_core::InferenceError`] variant
    /// (length mismatch, no-match, unbound map). Ambiguous matches
    /// resolve to the "straightest" path (min sum |Δcol|, lex-smallest
    /// on further tie) per the W2 tie-breaker; callers wanting to
    /// detect ambiguity should compare engine output against the
    /// scraped `visited_cols` if available.
    ///
    /// Bead `sts2-rs-cr5xs` (W8 — R-Act1-Replay corpus harness needs
    /// this to convert scraped `(map_point_type)` sequences into the
    /// `Move{col}` actions the engine dispatches). Backed by
    /// [`sts2_core::infer_visited_cols`] (W2, sts2-rs-ed5nw).
    // `Vec<String>` is required by `pyo3`'s argument-extraction trait
    // bounds — `&[String]` doesn't implement `FromPyObject` at this
    // pyo3 version.
    #[allow(clippy::needless_pass_by_value)]
    fn infer_visited_cols(&self, scraped_room_types: Vec<String>) -> PyResult<Vec<i32>> {
        let Some(act_map) = self.inner.run_state.current_act_map.as_ref() else {
            return Err(PyRuntimeError::new_err(
                "infer_visited_cols: no active act_map (call after \
                 initialize_run or act-entry)",
            ));
        };
        let mut typed: Vec<MapPointType> = Vec::with_capacity(scraped_room_types.len());
        for (i, s) in scraped_room_types.iter().enumerate() {
            let t = match s.as_str() {
                "unassigned" => MapPointType::Unassigned,
                "unknown" => MapPointType::Unknown,
                "monster" => MapPointType::Monster,
                "elite" => MapPointType::Elite,
                "boss" => MapPointType::Boss,
                "shop" => MapPointType::Shop,
                "rest_site" => MapPointType::RestSite,
                "treasure" => MapPointType::Treasure,
                "ancient" => MapPointType::Ancient,
                other => {
                    return Err(PyValueError::new_err(format!(
                        "infer_visited_cols: scraped_room_types[{i}] = {other:?} \
                         is not a known map_point_type"
                    )));
                }
            };
            typed.push(t);
        }
        match sts2_core::infer_visited_cols(act_map, &typed) {
            Ok(r) => Ok(r.visited_cols),
            Err(e) => Err(PyRuntimeError::new_err(format!(
                "infer_visited_cols failed: {e:?}"
            ))),
        }
    }

    fn available_actions<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        crate::value_to_pylist(py, &emit_rm::available_actions(&self.inner))
    }

    fn take_action(&mut self, action: &Bound<'_, PyAny>) -> PyResult<&'static str> {
        let d: Bound<'_, PyDict> = action
            .downcast::<PyDict>()
            .map_err(|_| PyValueError::new_err("take_action expects a dict"))?
            .clone();
        // Explicit-index multi-pick combat-choice resolution. `RunAction` is
        // `Copy`, so it can't carry an arbitrary index list — this action is
        // routed directly to the core `resolve_combat_choice_selection` instead
        // of through `action_from_pydict`. Needed for parity/replay drivers that
        // must reproduce a SPECIFIC C# multi-select (e.g. Dredge picking discard
        // indices `[1, 2, 4]`, not the first 3). `selection` is a list of
        // non-negative ints; an empty list / absent key skips the choice.
        let kind: Option<String> = match d.get_item("action")? {
            Some(v) => Some(v.extract()?),
            None => None,
        };
        if kind.as_deref() == Some("combat_resolve_choice_selection") {
            let selection: smallvec::SmallVec<[u8; 4]> = match d.get_item("selection")? {
                Some(v) if !v.is_none() => v.extract::<Vec<u8>>()?.into_iter().collect(),
                _ => smallvec::SmallVec::new(),
            };
            self.ensure_play_log_flag();
            let r = self.inner.resolve_combat_choice_selection(selection);
            return Ok(result_str(&r));
        }
        let act = action_from_pydict(&d)?;
        self.ensure_play_log_flag();
        let r = self.inner.take_action(act);
        Ok(result_str(&r))
    }

    /// Enable/disable recording of the per-combat card play-log (workbench
    /// event log). Off by default; the workbench turns it on after starting a
    /// run. Idempotent. Display side-channel only — not a parity surface.
    fn set_record_card_plays(&mut self, on: bool) {
        self.record_card_plays = on;
        if let Some(c) = self.inner.combat.as_mut() {
            c.combat_log.record_card_play_log = on;
        }
    }

    /// The current combat's ordered card play-log as a list of dicts:
    /// `{"id": str, "upgraded": bool, "energy_spent": int, "auto_played": bool,
    ///  "target": int | None}`. Empty list outside combat. NOT part of
    /// `state_to_dict` and NOT a parity surface.
    fn card_play_log<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        crate::value_to_pylist(py, &emit_rm::card_play_log(&self.inner))
    }

    /// Enable/disable recording of the FULL game-style combat log — the port
    /// of the game's own log panel (`sts2_core::event_log`). Off by default;
    /// the workbench turns it on after starting a run. Idempotent. Display
    /// side-channel only — not a parity surface, and never seen by search
    /// (`MctsTree::new` disarms its root clone).
    fn set_record_combat_log(&mut self, on: bool) {
        self.record_combat_log = on;
        if let Some(c) = self.inner.combat.as_mut() {
            c.combat_log.event_log.set_recording(on);
        }
    }

    /// The current combat's ordered event log, oldest first.
    ///
    /// Each row is `{"kind": str, "round": int, "side": "player"|"enemy",
    /// "text": str, "source": {...} | None, ...kind-specific fields}`. `text`
    /// is the C# `CombatHistoryEntry.Description` verbatim (ids, not display
    /// names) — useful for debugging and for diffing against the world-side
    /// `[HISTTRACE]` stream; the workbench renders its own localized line from
    /// the structured fields instead.
    ///
    /// Empty outside combat, and empty unless [`Self::set_record_combat_log`]
    /// was called. NOT part of `state_to_dict` and NOT a parity surface.
    fn combat_log<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        crate::value_to_pylist(py, &emit_rm::combat_log(&self.inner))
    }

    /// Rows the current combat's event log refused after hitting its cap.
    /// Non-zero means [`Self::combat_log`] is a PREFIX of the fight, not the
    /// whole of it — surface it rather than showing a truncated log as
    /// complete.
    fn combat_log_dropped(&self) -> u32 {
        self.inner
            .combat
            .as_ref()
            .map_or(0, |c| c.combat_log.event_log.dropped())
    }

    /// Drive the run with a uniform-random policy over
    /// `available_actions()` until [`Phase::RunOver`] or `max_steps`
    /// is reached. Returns `{"steps": N, "player_won": bool,
    /// "final_phase": str, "final_hp": i32, "final_gold": i32,
    /// "act_index": i32}`. Convenience for benchmarks and smoke
    /// tests — production callers should drive the loop themselves
    /// to record per-step state.
    #[pyo3(signature = (max_steps = 100_000, policy_seed = None))]
    fn random_playout<'py>(
        &mut self,
        py: Python<'py>,
        max_steps: usize,
        policy_seed: Option<u64>,
    ) -> PyResult<Bound<'py, PyDict>> {
        use rand::seq::SliceRandom;
        use rand::SeedableRng;
        use rand_xoshiro::Xoshiro256PlusPlus;
        let seed_u64 = u64::from(u32::from_ne_bytes(self.seed.to_ne_bytes()));
        let mut rng: Xoshiro256PlusPlus = match policy_seed {
            Some(s) => Xoshiro256PlusPlus::seed_from_u64(s),
            None => Xoshiro256PlusPlus::seed_from_u64(seed_u64 ^ 0xA5A5_A5A5_DEAD_BEEF),
        };
        let mut steps = 0_usize;
        while steps < max_steps && !self.inner.is_over() {
            let actions = self.inner.get_available_actions();
            if actions.is_empty() {
                return Err(PyRuntimeError::new_err(format!(
                    "random_playout stalled at phase={} step={steps} — \
                     get_available_actions() returned empty",
                    phase_str(self.inner.phase())
                )));
            }
            let act = *actions.choose(&mut rng).unwrap();
            let r = self.inner.take_action(act);
            if matches!(r, ActionResult::NotImplemented) {
                return Err(PyRuntimeError::new_err(format!(
                    "random_playout hit NotImplemented at phase={} action={:?}",
                    phase_str(self.inner.phase()),
                    act
                )));
            }
            steps += 1;
        }
        let d = PyDict::new(py);
        d.set_item("steps", steps)?;
        d.set_item("player_won", self.inner.run_state.player_won)?;
        d.set_item("final_phase", phase_str(self.inner.phase()))?;
        d.set_item("final_hp", self.inner.run_state.current_hp)?;
        d.set_item("final_gold", self.inner.run_state.gold)?;
        d.set_item("act_index", self.inner.run_state.act_index)?;
        d.set_item("terminated", self.inner.is_over())?;
        Ok(d)
    }

    /// Deep-clone the whole run (engine state + RNG cursors + live combat).
    /// The inner `RunManager` derives `Clone` and holds no `Arc`/`Rc`
    /// aliases, so a shallow `clone()` is observationally a deep clone —
    /// the copy advances independently of the original. Used by the
    /// workbench card-pick advisor to fork the run at a decision point and
    /// play each candidate out in isolation without disturbing the live run.
    #[pyo3(name = "clone")]
    fn py_clone(&self) -> Self {
        self.clone()
    }

    /// Support `copy.copy(rm)`.
    fn __copy__(&self) -> Self {
        self.clone()
    }

    /// Support `copy.deepcopy(rm, memo)` (see [`Self::py_clone`]).
    #[pyo3(signature = (_memo))]
    fn __deepcopy__(&self, _memo: &Bound<'_, PyAny>) -> Self {
        self.clone()
    }

    /// Re-seed the run-level RNG set in place, replacing every sub-stream
    /// cursor with a fresh draw from `seed`. The act map and current state
    /// (deck / HP / relics / phase) are left untouched — only *future* rolls
    /// (combat shuffles, monster AI, reward/event/shop generation) change.
    ///
    /// This is the Monte-Carlo lever the card-pick advisor pulls between
    /// rollouts: clone the run, reseed, then play the rest of the act out, so
    /// each rollout samples a different plausible future from the same
    /// decision point. Mirrors the in-test `run_state.rng = RunRngSet::new(s)`
    /// idiom (a documented, safe whole-set swap).
    fn reseed_run(&mut self, seed: i32) {
        self.inner.run_state.rng = sts2_core::RunRngSet::new(seed);
    }

    fn __repr__(&self) -> String {
        format!(
            "RunManager(seed={}, character={}, phase={}, hp={}/{}, gold={}, act={})",
            self.seed,
            character_str(self.inner.character_id),
            phase_str(self.inner.phase()),
            self.inner.run_state.current_hp,
            self.inner.run_state.max_hp,
            self.inner.run_state.gold,
            self.inner.run_state.act_index,
        )
    }
}
