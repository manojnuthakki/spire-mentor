//! `RunAction` ⇄ the `{"action": "<name>", …payload}` dict.
//!
//! Phase/result strings are lowercase `snake_case`. See the integration test
//! `crates/sts2-cli/tests/full_run_act1to3_ironclad.rs` for the canonical
//! Rust-side driver this mirrors.

use serde_json::{Map, Value};
use sts2_core::run_manager::RunAction;
use sts2_core::Action;

use super::map_ext::{obj, set};
use super::EmitError;

/// `RunManager.available_actions()[i]` / the dict `take_action` accepts.
#[must_use]
pub fn run_action_to_value(a: &RunAction) -> Value {
    let mut d = obj();
    match a {
        RunAction::Move { col } => {
            set(&mut d, "action", "move");
            set(&mut d, "col", *col);
        }
        RunAction::PlayCard { hand_idx, target } => {
            set(&mut d, "action", "play_card");
            set(&mut d, "hand_idx", *hand_idx);
            set(&mut d, "target_index", *target);
        }
        RunAction::EndTurn => {
            set(&mut d, "action", "end_turn");
        }
        RunAction::PickCardReward { idx } => {
            set(&mut d, "action", "pick_card_reward");
            set(&mut d, "idx", *idx);
        }
        RunAction::PickRelicReward => {
            set(&mut d, "action", "pick_relic_reward");
        }
        RunAction::SkipRelic => {
            set(&mut d, "action", "skip_relic");
        }
        RunAction::PickBossRelic { idx } => {
            set(&mut d, "action", "pick_boss_relic");
            set(&mut d, "idx", *idx);
        }
        RunAction::LeaveShop => {
            set(&mut d, "action", "leave_shop");
        }
        RunAction::ShopBuyCard { idx } => {
            set(&mut d, "action", "shop_buy_card");
            set(&mut d, "idx", *idx);
        }
        RunAction::ShopBuyRelic { idx } => {
            set(&mut d, "action", "shop_buy_relic");
            set(&mut d, "idx", *idx);
        }
        RunAction::ShopBuyPotion { idx } => {
            set(&mut d, "action", "shop_buy_potion");
            set(&mut d, "idx", *idx);
        }
        RunAction::ShopRemoveCard { deck_idx } => {
            set(&mut d, "action", "shop_remove_card");
            set(&mut d, "deck_idx", *deck_idx);
        }
        RunAction::RemoveDeckCard { idx } => {
            set(&mut d, "action", "remove_deck_card");
            set(&mut d, "idx", *idx);
        }
        RunAction::UpgradeDeckCard { idx } => {
            set(&mut d, "action", "upgrade_deck_card");
            set(&mut d, "idx", *idx);
        }
        RunAction::TransformDeckCard { idx } => {
            set(&mut d, "action", "transform_deck_card");
            set(&mut d, "idx", *idx);
        }
        RunAction::EnchantDeckCard { idx } => {
            set(&mut d, "action", "enchant_deck_card");
            set(&mut d, "idx", *idx);
        }
        RunAction::GrantDeckCard { idx } => {
            set(&mut d, "action", "grant_deck_card");
            set(&mut d, "idx", *idx);
        }
        RunAction::SmithCard { idx } => {
            set(&mut d, "action", "smith_card");
            set(&mut d, "idx", *idx);
        }
        RunAction::RestOption { idx } => {
            set(&mut d, "action", "rest_option");
            set(&mut d, "idx", *idx);
        }
        RunAction::EventChoice { idx } => {
            set(&mut d, "action", "event_choice");
            set(&mut d, "idx", *idx);
        }
        RunAction::CollectTreasure => {
            set(&mut d, "action", "collect_treasure");
        }
        RunAction::SacrificeCardReward => {
            set(&mut d, "action", "sacrifice_card_reward");
        }
        RunAction::CombatResolveChoice { option } => {
            set(&mut d, "action", "combat_resolve_choice");
            set(&mut d, "option", *option);
        }
        RunAction::DrinkPotion {
            potion_slot,
            target,
        } => {
            set(&mut d, "action", "drink_potion");
            set(&mut d, "potion_slot", *potion_slot);
            set(&mut d, "target_index", *target);
        }
        RunAction::CombatResolveChoiceTakeFirst { count } => {
            set(&mut d, "action", "combat_resolve_choice_take_first");
            set(&mut d, "count", *count);
        }
        RunAction::PotionSwap { discard_slot } => {
            set(&mut d, "action", "potion_swap");
            set(&mut d, "discard_slot", *discard_slot);
        }
        RunAction::Unhandled => {
            set(&mut d, "action", "unhandled");
        }
        // sts2-agent fork: the opt-in rewards screen and potion discard.
        RunAction::ClaimReward { idx } => {
            set(&mut d, "action", "claim_reward");
            set(&mut d, "idx", *idx);
        }
        RunAction::ProceedRewards => {
            set(&mut d, "action", "proceed_rewards");
        }
        RunAction::CancelRewardClaim => {
            set(&mut d, "action", "cancel_reward_claim");
        }
        RunAction::DiscardPotion { slot } => {
            set(&mut d, "action", "discard_potion");
            set(&mut d, "slot", *slot);
        }
        _ => {
            set(&mut d, "action", "unknown");
        }
    }
    Value::Object(d)
}

/// A key that is absent or `null` reads as `None`; a non-negative integer
/// reads as `Some`; anything else is an error naming the key.
fn opt_usize(d: &Map<String, Value>, k: &str) -> Result<Option<usize>, EmitError> {
    match d.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| EmitError::new(format!("key {k:?} must be a non-negative integer"))),
    }
}

fn req_usize(d: &Map<String, Value>, k: &str) -> Result<usize, EmitError> {
    opt_usize(d, k)?.ok_or_else(|| EmitError::new(format!("missing required key {k:?}")))
}

fn opt_u32(d: &Map<String, Value>, k: &str) -> Result<Option<u32>, EmitError> {
    match d.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| EmitError::new(format!("key {k:?} must be a non-negative integer"))),
    }
}

fn req_i32(d: &Map<String, Value>, k: &str) -> Result<i32, EmitError> {
    let v = d
        .get(k)
        .ok_or_else(|| EmitError::new(format!("missing required key {k:?}")))?;
    v.as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| EmitError::new(format!("key {k:?} must be an integer")))
}

/// Parse the `{"action": "<name>", …}` dict `take_action` accepts — the
/// inverse of [`run_action_to_value`].
///
/// # Errors
///
/// A dict with no `action` key, an unknown action kind, a missing required
/// payload key, or a payload value of the wrong type. Messages are the ones
/// the Python `take_action` has always raised as `ValueError`.
pub fn run_action_from_value(v: &Value) -> Result<RunAction, EmitError> {
    let d = v
        .as_object()
        .ok_or_else(|| EmitError::new("take_action expects a dict"))?;
    let kind = d
        .get("action")
        .ok_or_else(|| EmitError::new("action dict missing 'action' key"))?
        .as_str()
        .ok_or_else(|| EmitError::new("action dict 'action' key must be a string"))?;
    Ok(match kind {
        "move" => RunAction::Move {
            col: req_i32(d, "col")?,
        },
        "play_card" => RunAction::PlayCard {
            hand_idx: req_usize(d, "hand_idx")?,
            target: opt_u32(d, "target_index")?,
        },
        "end_turn" => RunAction::EndTurn,
        "pick_card_reward" => RunAction::PickCardReward {
            idx: opt_usize(d, "idx")?,
        },
        "pick_relic_reward" => RunAction::PickRelicReward,
        "skip_relic" => RunAction::SkipRelic,
        "pick_boss_relic" => RunAction::PickBossRelic {
            idx: req_usize(d, "idx")?,
        },
        "leave_shop" => RunAction::LeaveShop,
        // Bead `sts2-rs-i6h9i.1`: expose shop purchase + remove +
        // smith-card actions to the Python driver so scraped Act-1
        // runs can be replayed faithfully. Variants exist in the
        // engine (`sts2-rs-if6ry` / `sts2-rs-5abnx`) but were
        // previously stranded behind the `_ => "unknown"` arm.
        "shop_buy_card" => RunAction::ShopBuyCard {
            idx: req_usize(d, "idx")?,
        },
        "shop_buy_relic" => RunAction::ShopBuyRelic {
            idx: req_usize(d, "idx")?,
        },
        "shop_buy_potion" => RunAction::ShopBuyPotion {
            idx: req_usize(d, "idx")?,
        },
        "shop_remove_card" => RunAction::ShopRemoveCard {
            deck_idx: req_usize(d, "deck_idx")?,
        },
        "smith_card" => RunAction::SmithCard {
            idx: req_usize(d, "idx")?,
        },
        "remove_deck_card" => RunAction::RemoveDeckCard {
            idx: req_usize(d, "idx")?,
        },
        "upgrade_deck_card" => RunAction::UpgradeDeckCard {
            idx: req_usize(d, "idx")?,
        },
        "transform_deck_card" => RunAction::TransformDeckCard {
            idx: req_usize(d, "idx")?,
        },
        "enchant_deck_card" => RunAction::EnchantDeckCard {
            idx: req_usize(d, "idx")?,
        },
        "grant_deck_card" => RunAction::GrantDeckCard {
            idx: req_usize(d, "idx")?,
        },
        "rest_option" => RunAction::RestOption {
            idx: req_usize(d, "idx")?,
        },
        "event_choice" => RunAction::EventChoice {
            idx: req_usize(d, "idx")?,
        },
        "collect_treasure" => RunAction::CollectTreasure,
        "sacrifice_card_reward" => RunAction::SacrificeCardReward,
        // Bead `sts2-rs-i6h9i.1`: surface in-combat pending-choice
        // resolution (added to the core RunManager API in `sts2-rs-6dtxp`)
        // so the replay driver can resolve choices MCTS opens mid-combat.
        // `option: None` skips; `option: Some(i)` selects legal option i.
        "combat_resolve_choice" => RunAction::CombatResolveChoice {
            option: opt_usize(d, "option")?,
        },
        // Multi-pick resolution: take the first `count` options (the form
        // `legal_actions` enumerates for max_select > 1 choices, e.g. a
        // surfaced discard-2). Bead `sts2-rs-mcts-surface-onplay-select`.
        "combat_resolve_choice_take_first" => RunAction::CombatResolveChoiceTakeFirst {
            count: u8::try_from(req_usize(d, "count")?).unwrap_or(u8::MAX),
        },
        // Drink a held potion in combat (workbench `use_potion`). `target`
        // is an optional enemy index for `AnyEnemy` potions; self/no-target
        // potions ignore it.
        "drink_potion" => RunAction::DrinkPotion {
            potion_slot: req_usize(d, "potion_slot")?,
            target: opt_u32(d, "target_index")?,
        },
        // Full-belt potion-reward swap (GH #252): `discard_slot: None`
        // declines the reward; `Some(slot)` discards the held potion at
        // `slot` and claims the pending one. Only surfaced when
        // `run_state.interactive_potion_swap` is enabled.
        "potion_swap" => RunAction::PotionSwap {
            discard_slot: opt_usize(d, "discard_slot")?,
        },
        "unhandled" => RunAction::Unhandled,
        // sts2-agent fork: the opt-in rewards screen and potion discard.
        "claim_reward" => RunAction::ClaimReward {
            idx: req_usize(d, "idx")?,
        },
        "proceed_rewards" => RunAction::ProceedRewards,
        "cancel_reward_claim" => RunAction::CancelRewardClaim,
        "discard_potion" => RunAction::DiscardPotion {
            slot: req_usize(d, "slot")?,
        },
        other => {
            return Err(EmitError::new(format!("unknown action kind {other:?}")));
        }
    })
}

/// The `RunAction` dict for a combat [`Action`] MCTS picked — what
/// `RunManager.mcts_pick_action()` returns, ready to feed back into
/// `take_action`. `Null` when the action is not expressible at the
/// run-action surface: `play_card` / `end_turn` / `drink_potion` /
/// `combat_resolve_choice` all translate; only orb actions (`Channel` /
/// `Evoke`, never emitted by `legal_actions`) fall through.
#[must_use]
pub fn combat_action_to_run_action_value(chosen: &Action) -> Value {
    let mut d = obj();
    match chosen {
        Action::PlayCard { hand_index, target } => {
            set(&mut d, "action", "play_card");
            set(&mut d, "hand_idx", *hand_index);
            set(&mut d, "target_index", target.map(u32::from));
        }
        Action::EndTurn => {
            set(&mut d, "action", "end_turn");
        }
        Action::DrinkPotion {
            potion_slot,
            target,
        }
        | Action::UsePotion {
            slot: potion_slot,
            target,
        } => {
            set(&mut d, "action", "drink_potion");
            set(&mut d, "potion_slot", *potion_slot);
            set(&mut d, "target_index", target.map(u32::from));
        }
        Action::ResolveChoice { selection } => {
            // Single-pick is `[i]`, skip is `[]` — same surface as
            // `RunAction::CombatResolveChoice` (bead sts2-rs-6dtxp).
            set(&mut d, "action", "combat_resolve_choice");
            set(&mut d, "option", selection.first().map(|i| usize::from(*i)));
        }
        // Orb actions (Channel / Evoke) are never emitted by
        // `legal_actions`, so MCTS can't pick them at the root;
        // None stays as the defensive fallback.
        _ => return Value::Null,
    }
    Value::Object(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One of every `RunAction` variant, with both arms of each `Option`
    /// payload. Grows with the enum: a new variant that is not listed here
    /// still round-trips as `"unknown"` → error, which the loop below would
    /// surface the moment someone adds it to this list.
    fn every_variant() -> Vec<RunAction> {
        vec![
            RunAction::Move { col: -1 },
            RunAction::PlayCard {
                hand_idx: 3,
                target: Some(1),
            },
            RunAction::PlayCard {
                hand_idx: 0,
                target: None,
            },
            RunAction::EndTurn,
            RunAction::PickCardReward { idx: Some(2) },
            RunAction::PickCardReward { idx: None },
            RunAction::PickRelicReward,
            RunAction::SkipRelic,
            RunAction::PickBossRelic { idx: 1 },
            RunAction::LeaveShop,
            RunAction::ShopBuyCard { idx: 4 },
            RunAction::ShopBuyRelic { idx: 0 },
            RunAction::ShopBuyPotion { idx: 2 },
            RunAction::ShopRemoveCard { deck_idx: 7 },
            RunAction::SmithCard { idx: 5 },
            RunAction::RemoveDeckCard { idx: 1 },
            RunAction::UpgradeDeckCard { idx: 2 },
            RunAction::TransformDeckCard { idx: 3 },
            RunAction::EnchantDeckCard { idx: 4 },
            RunAction::GrantDeckCard { idx: 0 },
            RunAction::RestOption { idx: 1 },
            RunAction::EventChoice { idx: 2 },
            RunAction::CollectTreasure,
            RunAction::SacrificeCardReward,
            RunAction::CombatResolveChoice { option: Some(0) },
            RunAction::CombatResolveChoice { option: None },
            RunAction::CombatResolveChoiceTakeFirst { count: 2 },
            RunAction::DrinkPotion {
                potion_slot: 1,
                target: Some(0),
            },
            RunAction::DrinkPotion {
                potion_slot: 0,
                target: None,
            },
            RunAction::PotionSwap {
                discard_slot: Some(1),
            },
            RunAction::PotionSwap { discard_slot: None },
            RunAction::Unhandled,
        ]
    }

    #[test]
    fn every_run_action_round_trips_through_its_dict() {
        for a in every_variant() {
            let v = run_action_to_value(&a);
            assert_ne!(v["action"], "unknown", "{a:?} has no dict spelling");
            let back = run_action_from_value(&v).unwrap_or_else(|e| panic!("{a:?}: {e}"));
            assert_eq!(back, a, "dict {v}");
        }
    }

    #[test]
    fn option_payloads_are_null_not_absent() {
        // `None` is spelled as an explicit `null`, as the PyDict code set
        // `py.None()`; a reader distinguishing absent from null must see null.
        let v = run_action_to_value(&RunAction::PlayCard {
            hand_idx: 0,
            target: None,
        });
        assert_eq!(v.get("target_index"), Some(&Value::Null));
        let v = run_action_to_value(&RunAction::PotionSwap { discard_slot: None });
        assert_eq!(v.get("discard_slot"), Some(&Value::Null));
    }

    #[test]
    fn parse_failures_name_the_problem() {
        let e = run_action_from_value(&serde_json::json!({})).unwrap_err();
        assert_eq!(e.message(), "action dict missing 'action' key");
        let e = run_action_from_value(&serde_json::json!({"action": "teleport"})).unwrap_err();
        assert_eq!(e.message(), "unknown action kind \"teleport\"");
        let e = run_action_from_value(&serde_json::json!({"action": "move"})).unwrap_err();
        assert_eq!(e.message(), "missing required key \"col\"");
        let e = run_action_from_value(&serde_json::json!({"action": "smith_card", "idx": -1}))
            .unwrap_err();
        assert_eq!(e.message(), "key \"idx\" must be a non-negative integer");
        // A `null` idx on a required key is "missing", as `Some(v) if v.is_none()`
        // read it before.
        let e = run_action_from_value(&serde_json::json!({"action": "smith_card", "idx": null}))
            .unwrap_err();
        assert_eq!(e.message(), "missing required key \"idx\"");
    }

    #[test]
    fn combat_actions_translate_to_run_actions_or_null() {
        let v = combat_action_to_run_action_value(&Action::PlayCard {
            hand_index: 2,
            target: Some(1),
        });
        assert_eq!(
            v,
            serde_json::json!({"action": "play_card", "hand_idx": 2, "target_index": 1})
        );
        let v = combat_action_to_run_action_value(&Action::ResolveChoice {
            selection: smallvec::SmallVec::new(),
        });
        assert_eq!(
            v,
            serde_json::json!({"action": "combat_resolve_choice", "option": null})
        );
        assert_eq!(
            combat_action_to_run_action_value(&Action::Evoke { times: 1 }),
            Value::Null
        );
    }
}
