//! The Crystal Sphere event end to end (lane F's F8, `crystal-sphere`), against
//! the host's recordings (seed "3": `crystal_sphere_uncover_future` and
//! `crystal_sphere_payment_plan`): cost, grid, divinations, the curse at its
//! reveal, the rewards the revealed items offer and their order.

use sts2_core::cards::CardId;
use sts2_core::events::EventId;
use sts2_core::potions::{CharacterId, PotionKind};
use sts2_core::run_manager::{
    ActionResult, ForcedRoom, Phase, RunAction, RunManager, ScreenReward,
};

fn run() -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let seed = sts2_core::deterministic_hash_code(&sts2_core::canonicalize_seed("3"));
    let mut rm =
        sts2_content::run_callbacks::new_run_manager_for_character(seed, CharacterId::Ironclad)
            .with_neow_start();
    rm.run_state.direct_event_relic_grant = true;
    rm.run_state.interactive_rewards = true;
    let (by_rarity, fallback, boss) = sts2_content::run_callbacks::default_relic_grab_bag();
    rm.run_state
        .populate_relic_grab_bag_by_rarity(by_rarity, fallback, boss);
    rm.debug_set_player(None, None, Some(300));
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Event(EventId::CrystalSphere)),
        ActionResult::Advanced
    );
    rm
}

fn divine(rm: &mut RunManager, x: u8, y: u8, big: bool) -> ActionResult {
    rm.take_action(RunAction::CrystalSphereDivine { x, y, big })
}

#[test]
fn uncover_future_follows_the_recording() {
    let mut rm = run();
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 0 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.gold, 247, "300 - the rolled 53");
    assert_eq!(rm.phase(), Phase::Event);
    assert_eq!(
        rm.run_state
            .crystal_sphere
            .as_ref()
            .unwrap()
            .cleared_cells()
            .len(),
        24
    );
    // The grid has no way out but divining.
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 0 }),
        ActionResult::InvalidForPhase
    );
    assert_eq!(divine(&mut rm, 5, 5, true), ActionResult::Advanced);
    assert_eq!(
        rm.run_state
            .crystal_sphere
            .as_ref()
            .unwrap()
            .cleared_cells()
            .len(),
        33
    );
    assert_eq!(
        divine(&mut rm, 5, 5, true),
        ActionResult::InvalidForPhase,
        "a cleared cell"
    );
    assert_eq!(divine(&mut rm, 4, 3, false), ActionResult::Advanced);
    assert_eq!(divine(&mut rm, 8, 5, true), ActionResult::Advanced);
    // The last divination ended the event: its rewards, gold before potion.
    assert!(rm.run_state.crystal_sphere.is_none());
    let screen = rm.reward_screen().expect("the revealed items' rewards");
    assert!(matches!(screen[0], ScreenReward::Gold { .. }), "{screen:?}");
    assert!(
        matches!(&screen[1], ScreenReward::Potion { kind } if *kind == PotionKind::StrengthPotion),
        "{screen:?}"
    );
    assert_eq!(screen.len(), 2);
}

#[test]
fn payment_plan_adds_debt_then_the_revealed_curse_adds_doubt() {
    let mut rm = run();
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 1 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.gold, 300);
    let has = |rm: &RunManager, c| rm.run_state.master_deck.iter().any(|x| x.card_id == c);
    assert!(has(&rm, CardId::Debt));
    assert_eq!(
        rm.run_state
            .crystal_sphere
            .as_ref()
            .unwrap()
            .divinations_left(),
        6
    );
    for (x, y, big) in [(5, 5, true), (3, 3, false), (2, 5, true), (8, 5, true)] {
        assert_eq!(divine(&mut rm, x, y, big), ActionResult::Advanced);
        assert!(!has(&rm, CardId::Doubt), "not yet revealed");
    }
    assert_eq!(divine(&mut rm, 5, 2, true), ActionResult::Advanced);
    assert_eq!(divine(&mut rm, 5, 8, true), ActionResult::Advanced);
    assert!(has(&rm, CardId::Doubt), "the curse item was revealed");
    let kinds: Vec<&str> = rm
        .reward_screen()
        .expect("rewards")
        .iter()
        .map(|r| match r {
            ScreenReward::Gold { .. } => "gold",
            ScreenReward::Potion { .. } => "potion",
            ScreenReward::Card { .. } => "card",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, vec!["gold", "gold", "potion", "potion", "card"]);
}

#[test]
fn the_actions_on_the_grid_are_divinations_only() {
    let mut rm = run();
    rm.take_action(RunAction::EventChoice { idx: 0 });
    let actions = rm.get_available_actions();
    assert!(actions
        .iter()
        .all(|a| matches!(a, RunAction::CrystalSphereDivine { .. })));
    // 97 hidden cells, two tools each.
    assert_eq!(actions.len(), (121 - 24) * 2);
}
