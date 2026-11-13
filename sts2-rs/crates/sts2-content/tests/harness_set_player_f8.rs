//! The harness's `set_player` setters of lane F's F8, sts2-agent fork: a relic
//! added bare (C# `SetPlayerImpl`: no `RelicCmd.Obtain`), a deck card added at
//! its upgraded values, and hp 0 ending the run.

use sts2_content::run_callbacks::ironclad_content_callbacks;
use sts2_core::cards::CardId;
use sts2_core::potions::CharacterId;
use sts2_core::relics::RelicKind;
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunManager};
use sts2_core::RelicId;

fn neow_run(seed: i32) -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    RunManager::new(seed, CharacterId::Ironclad)
        .with_content_callbacks(ironclad_content_callbacks())
        .with_neow_start()
}

#[test]
fn a_bare_relic_add_runs_no_obtain_hook() {
    let mut rm = neow_run(5);
    let last = rm.run_state.last_obtained_relic;
    assert!(rm.force_add_relic_bare(RelicId::PumpkinCandle));
    assert!(rm.run_state.relics.contains(&RelicId::PumpkinCandle));
    // Held already: nothing changes.
    assert!(!rm.force_add_relic_bare(RelicId::PumpkinCandle));
    assert_eq!(
        rm.run_state
            .relics
            .iter()
            .filter(|r| **r == RelicId::PumpkinCandle)
            .count(),
        1
    );
    assert_eq!(rm.run_state.last_obtained_relic, last, "no obtain dispatch");
    assert!(!rm
        .run_state
        .relic_counters
        .contains_key(&RelicId::PumpkinCandle));
}

#[test]
fn in_a_live_fight_a_bare_relic_listens_at_once() {
    let mut rm = neow_run(5);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Combat("NIBBITS_WEAK")),
        ActionResult::Advanced
    );
    assert!(!rm
        .combat
        .as_ref()
        .unwrap()
        .relics
        .owns(RelicKind::Permafrost));
    assert!(rm.force_add_relic_bare(RelicId::Permafrost));
    assert!(rm
        .combat
        .as_ref()
        .unwrap()
        .relics
        .owns(RelicKind::Permafrost));
    // And removal takes it out of the store again.
    assert!(rm.force_remove_relic(RelicId::Permafrost));
    assert!(!rm
        .combat
        .as_ref()
        .unwrap()
        .relics
        .owns(RelicKind::Permafrost));
}

#[test]
fn an_upgraded_card_is_added_at_its_upgraded_values() {
    let mut rm = neow_run(5);
    assert!(rm.force_add_card(CardId::Bludgeon, false));
    assert!(rm.force_add_card(CardId::Bludgeon, true));
    let deck = &rm.run_state.master_deck;
    let (base, up) = (&deck[deck.len() - 2], &deck[deck.len() - 1]);
    assert!(!base.upgraded && up.upgraded);
    assert_eq!(base.base_damage, 32);
    assert_eq!(up.base_damage, 42, "Bludgeon+ deals 42, not the base 32");
}

#[test]
fn hp_zero_ends_the_run() {
    let mut rm = neow_run(5);
    rm.debug_set_player(Some(1), None, None);
    assert_ne!(rm.phase(), Phase::RunOver, "hp 1 is alive");
    rm.debug_set_player(Some(0), None, None);
    assert_eq!(rm.phase(), Phase::RunOver);
    assert!(rm.is_over());
    assert!(!rm.run_state.player_won);
}
