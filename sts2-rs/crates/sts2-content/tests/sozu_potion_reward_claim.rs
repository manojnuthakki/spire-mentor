//! A potion reward Sozu vetoes, on the rewards screen with a full belt
//! (sts2-agent fork, d1b-headless-sozu-potion-reward-belt-full). C#
//! `PotionReward.OnSelect` calls `PotionCmd.TryToProcure`, which checks
//! `Hook.ShouldProcurePotion` before the belt: under Sozu the failure is
//! `NotAllowed`, not `TooFull`, and the reward is taken with nothing obtained,
//! full belt or not. So the claim is offered; claiming it leaves the belt as it
//! was.

use sts2_core::potions::{CharacterId, PotionKind};
use sts2_core::run_manager::{Phase, RunAction, RunManager, ScreenReward};
use sts2_core::{OfferedReward, RelicId};

/// The first fight won, its rewards screen open with a Snecko Oil reward too.
fn room_screen_with_a_potion() -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm = sts2_content::run_callbacks::new_run_manager_for_character_with_ascension(
        11,
        CharacterId::Ironclad,
        0,
    );
    rm.run_state.interactive_rewards = true;
    sts2_content::run_callbacks::initialize_run_setup(&mut rm);
    rm.run_state.room_extra_rewards = vec![OfferedReward::Potion(Some(PotionKind::SneckoOil))];
    for _ in 0..2000 {
        if rm.reward_screen().is_some() {
            return rm;
        }
        if rm.phase() == Phase::Combat {
            rm.debug_set_player(Some(9999), Some(9999), None);
            let actions = rm.get_available_actions();
            let a = actions
                .iter()
                .copied()
                .find(|a| matches!(a, RunAction::PlayCard { .. }))
                .unwrap_or(actions[0]);
            rm.take_action(a);
        } else {
            let a = rm.get_available_actions()[0];
            rm.take_action(a);
        }
    }
    panic!("no rewards screen");
}

fn snecko_oil(rm: &RunManager) -> usize {
    rm.reward_set()
        .unwrap()
        .rewards
        .iter()
        .position(|r| matches!(r, ScreenReward::Potion { kind } if *kind == PotionKind::SneckoOil))
        .expect("the Snecko Oil reward is listed")
}

fn fill_belt(rm: &mut RunManager) {
    let fill = [
        PotionKind::FirePotion,
        PotionKind::BlockPotion,
        PotionKind::SwiftPotion,
    ];
    while rm.run_state.potions.len() < rm.run_state.max_potion_slots as usize {
        let k = fill[rm.run_state.potions.len() % fill.len()];
        assert!(rm.run_state.add_potion_to_belt(k));
    }
}

#[test]
fn a_potion_sozu_vetoes_is_claimable_with_a_full_belt_and_gives_nothing() {
    let mut rm = room_screen_with_a_potion();
    rm.run_state.relics.push(RelicId::Sozu);
    fill_belt(&mut rm);
    let belt = rm.run_state.potions.clone();
    let idx = snecko_oil(&rm);
    assert!(
        rm.get_available_actions()
            .contains(&RunAction::ClaimReward { idx }),
        "TryToProcure fails NotAllowed (not TooFull): the reward is taken"
    );
    let listed = rm.reward_set().unwrap().rewards.len();
    rm.take_action(RunAction::ClaimReward { idx });
    assert_eq!(rm.reward_set().map_or(0, |s| s.rewards.len()), listed - 1);
    assert_eq!(rm.run_state.potions, belt, "nothing obtained");
}

#[test]
fn without_sozu_a_full_belt_still_refuses_the_claim() {
    let mut rm = room_screen_with_a_potion();
    fill_belt(&mut rm);
    let idx = snecko_oil(&rm);
    assert!(!rm
        .get_available_actions()
        .contains(&RunAction::ClaimReward { idx }));
}
