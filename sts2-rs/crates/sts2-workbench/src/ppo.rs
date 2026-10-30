//! The PPO run policy in the workbench: port of `workbench/ppo_policy.py`
//! and `BranchingLiveSession.ppo_policy_panel` / `_ppo_recommend`.
//!
//! The Python session lazily loads one champion per character from
//! `data/ppo/champions.toml` through `sts2_rs.PpoRunAgent` (tract-onnx).
//! The browser has no filesystem, so here the host fetches the champion
//! file ([`champion_file`] names it — the generated table the manifest is
//! synced into) and hands the bytes to [`load`]; the session holds the
//! resulting [`PpoPolicy`] behind an `Rc` and scores every non-combat
//! decision with it: the value recorded on each node (the WPA review's data),
//! the probability panel, and the `ppo` off-combat brain.
//!
//! A missing net is not an error, exactly as a missing checkpoint is not in
//! Python: [`evaluate`] answers `ppo_available: false` with the reason, the
//! brain falls back to the heuristic, node values stay `null`.
//!
//! With the `ppo` feature off (a leaner wasm) every entry point answers as
//! if no net were loadable.

use serde_json::{json, Value};
use sts2_core::run_manager::{Phase, RunAction, RunManager};

use crate::rmx;

#[cfg(feature = "ppo")]
pub use sts2_runai::ppo::policy::PpoPolicy;

/// A loaded champion, shared between the session and its forks.
#[cfg(feature = "ppo")]
pub type Net = std::rc::Rc<PpoPolicy>;

/// Placeholder so the session's field has one type either way.
#[cfg(not(feature = "ppo"))]
#[derive(Debug)]
pub struct PpoPolicy;
#[cfg(not(feature = "ppo"))]
pub type Net = std::rc::Rc<PpoPolicy>;

/// The committed champion's ONNX filename for `character`
/// (`champions.toml` → the generated `champion_onnx_filename` table).
///
/// # Errors
/// An unknown character name.
pub fn champion_file(character: &str) -> Result<&'static str, String> {
    let cid = rmx::character_from_str(character)?;
    #[cfg(feature = "ppo")]
    {
        Ok(sts2_runai::ppo::net::champion_onnx_filename(cid))
    }
    #[cfg(not(feature = "ppo"))]
    {
        let _ = cid;
        Err("PPO is not compiled into this build".to_string())
    }
}

/// Build the policy for `character` from the ONNX bytes, at the encoder
/// schema its manifest entry records (`features_for`).
///
/// # Errors
/// An unknown character, or tract's error text for an unloadable export.
pub fn load(character: &str, bytes: &[u8]) -> Result<Net, String> {
    let cid = rmx::character_from_str(character)?;
    #[cfg(feature = "ppo")]
    {
        let features = sts2_runai::ppo::net::features_for(cid);
        PpoPolicy::from_bytes_with(bytes, features).map(std::rc::Rc::new)
    }
    #[cfg(not(feature = "ppo"))]
    {
        let _ = (cid, bytes);
        Err("PPO is not compiled into this build".to_string())
    }
}

/// `"no PPO net loaded for {character}"` — the reason a session without a
/// net reports, optionally with what the host saw when it tried
/// (`: fetch failed …`). The golden dump stubs the Python loader with the
/// bare form so both hosts answer the same `error`.
#[must_use]
pub fn missing_net_error(character: &str, reason: Option<&str>) -> String {
    match reason {
        Some(r) if !r.is_empty() => format!("no PPO net loaded for {character}: {r}"),
        _ => format!("no PPO net loaded for {character}"),
    }
}

/// `PpoEval` — PPO's view of one non-combat decision.
#[derive(Debug, Clone)]
pub struct PpoEval {
    pub phase: &'static str,
    pub surface: &'static str,
    /// The masked candidates — the net's view.
    pub actions: Vec<RunAction>,
    /// The unmasked `available_actions()`.
    pub full_actions: Vec<RunAction>,
    /// `None` when the net can't load / infer.
    pub probs: Option<Vec<f32>>,
    pub value: Option<f32>,
    /// Argmax over `probs` (group-mass decode).
    pub chosen: Option<usize>,
    pub error: Option<String>,
}

impl PpoEval {
    fn unavailable(
        phase: &'static str,
        surface: &'static str,
        actions: Vec<RunAction>,
        full_actions: Vec<RunAction>,
        error: String,
    ) -> Self {
        Self {
            phase,
            surface,
            actions,
            full_actions,
            probs: None,
            value: None,
            chosen: None,
            error: Some(error),
        }
    }
}

/// `PpoPolicy::prepare`: materialize a pending event so the observation
/// can be built (idempotent, RNG-free — see `RunPolicy::prepare`).
fn prepare(rm: &mut RunManager) {
    if rm.phase() == Phase::Event && rm.run_state.current_event.is_none() {
        if let Some(id) = sts2_content::events::dispatch::pick_event(&mut rm.run_state, None) {
            rm.run_state.bind_event(id);
        }
    }
}

/// `ppo_policy.evaluate`: PPO's distribution over the current decision, or
/// `None` when the decision isn't PPO's (combat / finished / no actions /
/// unknown surface). `net` is the session's loaded champion, `net_error`
/// why there is none.
#[must_use]
pub fn evaluate(net: Option<&Net>, net_error: &str, rm: &mut RunManager) -> Option<PpoEval> {
    if rm.is_over() {
        return None;
    }
    let phase = rm.phase();
    if phase == Phase::Combat || phase == Phase::RunOver {
        return None;
    }
    let full = rmx::available_actions(rm);
    if full.is_empty() {
        return None;
    }
    #[cfg(feature = "ppo")]
    {
        use sts2_runai::ppo::policy::{classify_surface, mask_rejectable, phase_str};
        let masked = mask_rejectable(rm, phase, &full);
        let surface = classify_surface(phase, &masked)?;
        let phase_s = phase_str(phase);
        if masked.len() == 1 {
            // Single-option decisions are auto-taken in training; no net needed.
            return Some(PpoEval {
                phase: phase_s,
                surface,
                actions: masked,
                full_actions: full,
                probs: Some(vec![1.0]),
                value: None,
                chosen: Some(0),
                error: None,
            });
        }
        let Some(net) = net else {
            return Some(PpoEval::unavailable(
                phase_s,
                surface,
                masked,
                full,
                net_error.to_string(),
            ));
        };
        // `PpoRunAgent.evaluate_with_actions`: prepare, then score.
        prepare(rm);
        let actions = rmx::available_actions(rm);
        if actions.is_empty() {
            return None;
        }
        match net.try_evaluate_with_actions(rm, &actions) {
            Err(e) => Some(PpoEval::unavailable(
                phase_s,
                surface,
                masked,
                full,
                format!("inference failed: {e}"),
            )),
            // Parity: rust also declines <2 masked; single-option handled
            // above, so this only happens if the two masks disagree.
            Ok(None) => Some(PpoEval::unavailable(
                phase_s,
                surface,
                masked,
                full,
                "rust backend returned no distribution for a multi-option decision".to_string(),
            )),
            Ok(Some((rust_actions, probs, value, chosen))) => {
                let rust_surface = classify_surface(phase, &rust_actions).unwrap_or(surface);
                Some(PpoEval {
                    phase: phase_s,
                    surface: rust_surface,
                    actions: rust_actions,
                    full_actions: full,
                    probs: Some(probs),
                    value: Some(value),
                    chosen: Some(chosen),
                    error: None,
                })
            }
        }
    }
    #[cfg(not(feature = "ppo"))]
    {
        let _ = (net, phase);
        Some(PpoEval::unavailable(
            "",
            "",
            full.clone(),
            full,
            net_error.to_string(),
        ))
    }
}

/// `ppo_policy.shop_remove_label`: name the master-deck card a shop
/// `shop_remove_card` removes, or `None` when `deck_idx` is out of range.
#[must_use]
pub fn shop_remove_label(deck: &[(String, bool)], deck_idx: i64) -> Option<String> {
    let idx = usize::try_from(deck_idx).ok()?;
    let (card_id, upgraded) = deck.get(idx)?;
    let name = camel_split(card_id);
    let plus = if *upgraded { "+" } else { "" };
    Some(format!("Remove {name}{plus} (deck #{deck_idx})"))
}

/// `_CAMEL_SPLIT.sub(" ", s)`: a space before each `[A-Z]` that follows a
/// `[a-z0-9]`, and before an `[A-Z][a-z]` that follows an `[A-Z]`.
fn camel_split(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 {
            let prev = chars[i - 1];
            let next = chars.get(i + 1).copied();
            let after_lower_digit =
                (prev.is_ascii_lowercase() || prev.is_ascii_digit()) && c.is_ascii_uppercase();
            let acronym_end = prev.is_ascii_uppercase()
                && c.is_ascii_uppercase()
                && next.is_some_and(|n| n.is_ascii_lowercase());
            if after_lower_digit || acronym_end {
                out.push(' ');
            }
        }
        out.push(c);
    }
    out
}

/// `ppo_policy_panel`'s payload from an evaluation: the labelled option
/// list. `resolve` maps a native action to the workbench `(verb, args)`
/// (`resolve_workbench_verb`), `label` renders it against the parent state.
#[must_use]
pub fn panel_value(
    ev: &PpoEval,
    ppo_impl: &str,
    deck: &[(String, bool)],
    mut resolve: impl FnMut(&RunAction) -> Option<(String, Value)>,
    mut label: impl FnMut(&str, &Value) -> String,
) -> Value {
    let mut options = Vec::with_capacity(ev.actions.len());
    for (i, native) in ev.actions.iter().enumerate() {
        let Some((verb, args)) = resolve(native) else {
            continue;
        };
        let wire = rmx::action_value(native);
        let text = match (wire["action"].as_str(), wire["deck_idx"].as_i64()) {
            (Some("shop_remove_card"), Some(deck_idx)) => {
                shop_remove_label(deck, deck_idx).unwrap_or_else(|| label(&verb, &args))
            }
            (Some("shop_remove_card"), None) => {
                shop_remove_label(deck, -1).unwrap_or_else(|| label(&verb, &args))
            }
            _ => label(&verb, &args),
        };
        options.push(json!({
            "action": verb,
            "args": args,
            "label": text,
            "prob": ev.probs.as_ref().map(|p| p[i]),
            "is_best": ev.chosen == Some(i),
        }));
    }
    json!({
        "ppo_available": ev.probs.is_some(),
        "impl": ppo_impl,
        "surface": ev.surface,
        "value": ev.value,
        "options": options,
        "error": ev.error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel_split_matches_the_python_regex() {
        assert_eq!(camel_split("PerfectedStrike"), "Perfected Strike");
        assert_eq!(camel_split("Strike"), "Strike");
        assert_eq!(camel_split("SWORDBoomerang"), "SWORD Boomerang");
        assert_eq!(camel_split("Bash2Go"), "Bash2 Go");
    }

    #[test]
    fn shop_remove_label_names_the_deck_card() {
        let deck = vec![
            ("Strike".to_string(), false),
            ("BodySlam".to_string(), true),
        ];
        assert_eq!(
            shop_remove_label(&deck, 1).as_deref(),
            Some("Remove Body Slam+ (deck #1)")
        );
        assert_eq!(shop_remove_label(&deck, 2), None);
        assert_eq!(shop_remove_label(&deck, -1), None);
    }

    #[test]
    fn missing_net_error_forms() {
        assert_eq!(
            missing_net_error("Ironclad", None),
            "no PPO net loaded for Ironclad"
        );
        assert_eq!(
            missing_net_error("Ironclad", Some("fetch failed")),
            "no PPO net loaded for Ironclad: fetch failed"
        );
    }
}
