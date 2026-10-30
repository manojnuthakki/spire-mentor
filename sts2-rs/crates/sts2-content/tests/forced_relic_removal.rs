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
fn in_a_live_fight_it_leaves_the_fights_relics_too() {
    let mut rm = neow_run(5);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Combat("NIBBITS_WEAK")),
        ActionResult::Advanced
    );
    assert!(rm.force_remove_relic(RelicId::BurningBlood));
    assert!(!rm.run_state.relics.contains(&RelicId::BurningBlood));
    let combat = rm.combat.as_ref().expect("a fight");
    assert!(!combat
        .relics
        .owns(sts2_core::relics::RelicKind::BurningBlood));
}

#[test]
fn winged_boots_obtained_again_start_with_every_use() {
    let mut rm = neow_run(5);
    rm.run_state.relics.push(RelicId::WingedBoots);
    rm.run_state.winged_boots_record_off_path();
    assert!(rm.force_remove_relic(RelicId::WingedBoots));
    assert_eq!(rm.run_state.winged_boots_off_path_used, 0);
}

/// Plays the live fight out: the first card play each time, else the end
/// of the turn.
fn finish_fight(rm: &mut RunManager) {
    use sts2_core::run_manager::{Phase, RunAction};
    for _ in 0..500 {
        if rm.phase() != Phase::Combat {
            return;
        }
        let actions = rm.get_available_actions();
        let a = actions
            .iter()
            .find(|a| matches!(a, RunAction::PlayCard { .. }))
            .or_else(|| actions.iter().find(|a| matches!(a, RunAction::EndTurn)))
            .copied()
            .unwrap_or(actions[0]);
        rm.take_action(a);
    }
    panic!("the fight didn't end");
}

#[test]
fn removed_after_a_finished_fight_whose_snapshot_is_kept() {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    // A run with its starting deck, to play the fight out.
    let mut rm =
        sts2_content::run_callbacks::new_run_manager_for_character(5, CharacterId::Ironclad);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Combat("NIBBITS_WEAK")),
        ActionResult::Advanced
    );
    let combat = rm.combat.as_mut().expect("a fight");
    let enemies: Vec<u32> = combat.enemy_uids.iter().map(|u| u.0.get()).collect();
    for c in &mut combat.creatures {
        if enemies.contains(&c.id.get()) {
            c.current_hp = 1;
        }
    }
    finish_fight(&mut rm);
    assert!(!rm.run_state.is_over, "the fight is won");
    assert!(rm.combat.is_some(), "the finished fight's snapshot stays");
    assert!(rm.force_remove_relic(RelicId::BurningBlood));
    assert!(!rm.run_state.relics.contains(&RelicId::BurningBlood));
}
