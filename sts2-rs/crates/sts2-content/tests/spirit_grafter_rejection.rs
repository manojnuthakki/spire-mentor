//! Spirit Grafter's Rejection: the upgrade pick, then the damage.
//!
//! The game's `SpiritGrafter.Rejection` awaits `CardSelectCmd.FromDeckForUpgrade`
//! and upgrades the pick, and only then deals its 10 unblockable damage. The
//! engine took the 10 HP when the option was chosen, so the upgrade's
//! card_select showed 70 HP where the Headless host shows 80 (scenario
//! card_view_x_replay_sly, step 2, S20).

use sts2_content::run_callbacks::{
    initialize_run_setup, new_ironclad_run_manager_from_string_seed,
};
use sts2_core::events::EventId;
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};

fn in_spirit_grafter(seed: &str) -> RunManager {
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
    rm.run_state.interactive_card_upgrade = true;
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Event(EventId::SpiritGrafter)),
        ActionResult::Advanced
    );
    rm
}

#[test]
fn rejection_damages_after_the_upgrade_pick() {
    let mut rm = in_spirit_grafter("3");
    let hp = rm.run_state.current_hp;
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 1 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.pending_deck_upgrades, 1);
    assert_eq!(rm.run_state.current_hp, hp, "no damage during the pick");
    assert!(rm
        .get_available_actions()
        .iter()
        .all(|a| matches!(a, RunAction::UpgradeDeckCard { .. })));
    assert_eq!(
        rm.take_action(RunAction::UpgradeDeckCard { idx: 0 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.pending_deck_upgrades, 0);
    assert_eq!(rm.run_state.current_hp, hp - 10);
}

#[test]
fn rejection_with_nothing_to_upgrade_damages_at_once() {
    let mut rm = in_spirit_grafter("3");
    for c in &mut rm.run_state.master_deck {
        if c.is_upgradable() {
            c.upgrade();
        }
    }
    let hp = rm.run_state.current_hp;
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 1 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.pending_deck_upgrades, 0);
    assert_eq!(rm.run_state.current_hp, hp - 10);
}
