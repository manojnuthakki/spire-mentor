//! Lord's Parasol pauses before a potion purchase with a full belt
//! (sts2-agent fork, lords-parasol-pause). C# `LordsParasol.
//! PurchaseEverything` awaits `Cmd.Wait(0.25f)` after each purchase with the
//! belt usable, so a potion drunk or discarded there makes room for the next
//! potion entry. The Headless host's agent mode offers that window, before a
//! potion purchase when the belt is full, as its `parasol_continue` decision
//! (use_potion, discard_potion, continue); the engine pauses there under
//! `interactive_potions`. Values: the Headless scenarios
//! `lords_parasol_full_belt` (seed "3"), `lords_parasol_full_belt_continue`
//! and `lords_parasol_pause_left_for_another_room` (seed "rev3a95").

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_from_string_seed,
};
use sts2_core::potions::{CharacterId, PotionKind};
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};
use sts2_core::RelicId;

fn with_the_parasol(seed: &str, potions: &[PotionKind]) -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_from_string_seed(seed, CharacterId::Ironclad)
        .with_neow_start();
    initialize_run_setup(&mut rm);
    rm.run_state.interactive_rewards = true;
    rm.run_state.interactive_card_removal = true;
    rm.run_state.interactive_potions = true;
    rm.run_state.relics = vec![RelicId::BurningBlood, RelicId::LordsParasol];
    rm.run_state.potions = potions.to_vec();
    rm
}

/// Only the pause's answer and the belt's actions are offered.
fn paused_actions_only(rm: &RunManager) -> bool {
    let actions = rm.get_available_actions();
    actions.contains(&RunAction::LordsParasolContinue)
        && actions.iter().all(|a| {
            matches!(
                a,
                RunAction::LordsParasolContinue
                    | RunAction::DiscardPotion { .. }
                    | RunAction::DrinkPotion { .. }
            )
        })
}

/// `lords_parasol_full_belt`: a pause before each potion; a discard at the
/// first lets Flex Potion into the freed slot, the other two fail.
#[test]
fn a_full_belt_pauses_before_each_potion() {
    let mut rm = with_the_parasol("3", &[PotionKind::FirePotion; 3]);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Shop),
        ActionResult::Advanced
    );
    assert_eq!(
        (
            rm.phase(),
            rm.lords_parasol_pause(),
            rm.run_state.relics.len()
        ),
        (Phase::Shop, Some(PotionKind::FlexPotion), 5)
    );
    assert!(paused_actions_only(&rm));
    assert_eq!(rm.run_state.pending_deck_removals, 0);
    // Neither a buy nor leaving is taken while paused.
    assert_eq!(
        rm.take_action(RunAction::LeaveShop),
        ActionResult::InvalidForPhase
    );
    assert_eq!(
        rm.take_action(RunAction::DiscardPotion { slot: 0 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.lords_parasol_pause(), Some(PotionKind::FlexPotion));
    assert_eq!(
        rm.take_action(RunAction::LordsParasolContinue),
        ActionResult::Advanced
    );
    assert_eq!(
        (rm.lords_parasol_pause(), rm.run_state.potions.clone()),
        (
            Some(PotionKind::SpeedPotion),
            vec![
                PotionKind::FlexPotion,
                PotionKind::FirePotion,
                PotionKind::FirePotion
            ]
        )
    );
    assert_eq!(
        rm.take_action(RunAction::LordsParasolContinue),
        ActionResult::Advanced
    );
    assert_eq!(rm.lords_parasol_pause(), Some(PotionKind::PowerPotion));
    assert_eq!(
        rm.take_action(RunAction::LordsParasolContinue),
        ActionResult::Advanced
    );
    // Then the removal, not cancelable; the two potions stay for sale.
    assert_eq!(
        (rm.lords_parasol_pause(), rm.run_state.pending_deck_removals),
        (None, 1)
    );
    assert_eq!(rm.run_state.gold, 99);
    assert_eq!(
        rm.take_action(RunAction::LordsParasolContinue),
        ActionResult::InvalidForPhase
    );
}

/// `lords_parasol_full_belt_continue`: the Blood Potion is bought into the
/// last slot with no pause; before Fortifier the belt is full. Drunk there,
/// it makes room for Fortifier; the Colorless Potion then fails.
#[test]
fn a_potion_drunk_at_the_pause_makes_room() {
    let mut rm = with_the_parasol(
        "rev3a95",
        &[PotionKind::FirePotion, PotionKind::DexterityPotion],
    );
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Shop),
        ActionResult::Advanced
    );
    assert_eq!(
        (rm.lords_parasol_pause(), rm.run_state.potions.clone()),
        (
            Some(PotionKind::Fortifier),
            vec![
                PotionKind::FirePotion,
                PotionKind::DexterityPotion,
                PotionKind::BloodPotion
            ]
        )
    );
    assert!(paused_actions_only(&rm));
    let hp = rm.run_state.current_hp;
    assert_eq!(
        rm.take_action(RunAction::DrinkPotion {
            potion_slot: 2,
            target: None
        }),
        ActionResult::Advanced
    );
    assert!(rm.run_state.current_hp >= hp);
    assert_eq!(rm.lords_parasol_pause(), Some(PotionKind::Fortifier));
    assert_eq!(
        rm.take_action(RunAction::LordsParasolContinue),
        ActionResult::Advanced
    );
    assert_eq!(
        (rm.lords_parasol_pause(), rm.run_state.potions.clone()),
        (
            Some(PotionKind::ColorlessPotion),
            vec![
                PotionKind::FirePotion,
                PotionKind::DexterityPotion,
                PotionKind::Fortifier
            ]
        )
    );
    assert_eq!(
        rm.take_action(RunAction::LordsParasolContinue),
        ActionResult::Advanced
    );
    assert_eq!(
        (
            rm.lords_parasol_pause(),
            rm.run_state.pending_deck_removals,
            rm.run_state.gold
        ),
        (None, 1, 99)
    );
}

/// `lords_parasol_pause_left_for_another_room`: a room entered while paused
/// ends the purchases.
#[test]
fn a_room_entered_while_paused_ends_the_purchases() {
    let mut rm = with_the_parasol(
        "rev3a95",
        &[PotionKind::FirePotion, PotionKind::DexterityPotion],
    );
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Shop),
        ActionResult::Advanced
    );
    assert_eq!(rm.lords_parasol_pause(), Some(PotionKind::Fortifier));
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Combat("NIBBITS_WEAK")),
        ActionResult::Advanced
    );
    assert_eq!(
        (
            rm.phase(),
            rm.lords_parasol_pause(),
            rm.run_state.pending_deck_removals
        ),
        (Phase::Combat, None, 0)
    );
    assert_eq!(
        rm.take_action(RunAction::LordsParasolContinue),
        ActionResult::InvalidForPhase
    );
}

/// Without `interactive_potions` nothing can change the belt, so there is
/// no pause: the purchases run through as before.
#[test]
fn no_pause_without_interactive_potions() {
    let mut rm = with_the_parasol("3", &[PotionKind::FirePotion; 3]);
    rm.run_state.interactive_potions = false;
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Shop),
        ActionResult::Advanced
    );
    assert_eq!(
        (rm.lords_parasol_pause(), rm.run_state.pending_deck_removals),
        (None, 1)
    );
}
