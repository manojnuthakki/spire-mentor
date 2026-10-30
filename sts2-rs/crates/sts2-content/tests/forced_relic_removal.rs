//! The harness's relic removal (`RunManager::force_remove_relic`), C#
//! `RelicCmd.Remove`: the engine side of the headless `set_player`'s relic
//! list, sts2-agent fork.

use sts2_content::run_callbacks::ironclad_content_callbacks;
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::{ActionResult, ForcedRoom, RunManager};
use sts2_core::RelicId;

fn neow_run(seed: i32) -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    RunManager::new(seed, CharacterId::Ironclad)
        .with_content_callbacks(ironclad_content_callbacks())
        .with_neow_start()
}

#[test]
fn removes_a_held_relic_once() {
    let mut rm = neow_run(5);
    assert!(rm.run_state.relics.contains(&RelicId::BurningBlood));
    assert!(rm.force_remove_relic(RelicId::BurningBlood));
    assert!(!rm.run_state.relics.contains(&RelicId::BurningBlood));
    // Not held any more: nothing to remove.
    assert!(!rm.force_remove_relic(RelicId::BurningBlood));
}

#[test]
fn the_relics_marks_go_with_it() {
    let mut rm = neow_run(5);
    rm.run_state.relics.push(RelicId::SwordOfStone);
    rm.run_state.relic_counters.insert(RelicId::SwordOfStone, 2);
    rm.run_state.wax_relics.insert(RelicId::SwordOfStone);
    assert!(rm.force_remove_relic(RelicId::SwordOfStone));
    assert!(!rm
        .run_state
        .relic_counters
        .contains_key(&RelicId::SwordOfStone));
    assert!(!rm.run_state.wax_relics.contains(&RelicId::SwordOfStone));
    assert!(rm.run_state.relics.contains(&RelicId::BurningBlood));
}

#[test]
fn a_removed_relic_is_not_in_the_next_fight() {
    let mut rm = neow_run(5);
    assert!(rm.force_remove_relic(RelicId::BurningBlood));
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Combat("NIBBITS_WEAK")),
        ActionResult::Advanced
    );
    let combat = rm.combat.as_ref().expect("a fight");
    assert!(!combat
        .relics
        .owns(sts2_core::relics::RelicKind::BurningBlood));
}

#[test]
fn refused_in_a_live_fight() {
    let mut rm = neow_run(5);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Combat("NIBBITS_WEAK")),
        ActionResult::Advanced
    );
    assert!(!rm.force_remove_relic(RelicId::BurningBlood));
    assert!(rm.run_state.relics.contains(&RelicId::BurningBlood));
}
