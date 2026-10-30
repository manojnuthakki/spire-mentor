//! Stone of All Time's Push: the damage, then the Vigorous pick.
//!
//! The game's `StoneOfAllTime.Push` deals 6 unblockable damage, then awaits
//! `CardSelectCmd.FromDeckForEnchantment(Vigorous, 8)` (Attacks only,
//! `Vigorous.CanEnchantCardType`; exactly one, auto-taken when at most one
//! card qualifies) and enchants the pick with Vigorous 8. The engine took the
//! damage and finished: no selection and no enchantment (scenario
//! stone_of_all_time_locked_selection, step 4, A6d17).

use sts2_content::cards::spec_index::card_pod_from_id;
use sts2_content::run_callbacks::{
    initialize_run_setup, new_ironclad_run_manager_from_string_seed,
};
use sts2_core::cards::{CardId, EnchantmentId};
use sts2_core::events::EventId;
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};

fn at_the_stone(seed: &str, deck: &[CardId]) -> RunManager {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
    let mut rm = new_ironclad_run_manager_from_string_seed(seed);
    initialize_run_setup(&mut rm);
    for _ in 0..50 {
        if rm.phase() == Phase::MapChoice {
            break;
        }
        let a = rm.get_available_actions()[0];
        rm.take_action(a);
    }
    rm.run_state.interactive_card_transform = true;
    rm.run_state.master_deck = deck
        .iter()
        .map(|id| card_pod_from_id(*id, false).expect("card pod"))
        .collect();
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Event(EventId::StoneOfAllTime)),
        ActionResult::Advanced
    );
    rm
}

fn vigorous(rm: &RunManager) -> Vec<(CardId, i32)> {
    rm.run_state
        .master_deck
        .iter()
        .map(|c| (c.card_id, c.enchantment(EnchantmentId::Vigorous)))
        .collect()
}

#[test]
fn push_damages_then_asks_for_an_attack_to_enchant() {
    let mut rm = at_the_stone(
        "3",
        &[
            CardId::StrikeIronclad,
            CardId::DefendIronclad,
            CardId::Bash,
            CardId::Inflame,
        ],
    );
    let hp = rm.run_state.current_hp;
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 1 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.current_hp, hp - 6);
    assert_eq!(
        rm.get_available_actions(),
        [
            RunAction::EnchantDeckCard { idx: 0 },
            RunAction::EnchantDeckCard { idx: 1 }
        ]
    );
    assert_eq!(rm.enchant_eligible_deck_indices(), [0, 2], "Strike, Bash");
    assert_eq!(
        rm.take_action(RunAction::EnchantDeckCard { idx: 1 }),
        ActionResult::Advanced
    );
    assert_eq!(
        vigorous(&rm),
        [
            (CardId::StrikeIronclad, 0),
            (CardId::DefendIronclad, 0),
            (CardId::Bash, 8),
            (CardId::Inflame, 0)
        ]
    );
    assert_eq!(rm.phase(), Phase::MapChoice);
}

#[test]
fn push_with_one_attack_enchants_it_without_a_pick() {
    let mut rm = at_the_stone("3", &[CardId::DefendIronclad, CardId::Bash]);
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 1 }),
        ActionResult::Advanced
    );
    assert!(rm.run_state.pending_deck_enchants.is_empty());
    assert_eq!(
        vigorous(&rm),
        [(CardId::DefendIronclad, 0), (CardId::Bash, 8)]
    );
}
