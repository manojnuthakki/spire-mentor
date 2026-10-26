//! The opt-in rewards screen (`RunState::interactive_rewards`, sts2-agent
//! fork) on a content-wired run: the screen rolls the same rewards as the
//! auto-collect path, read for read on the `Rewards` stream, and claiming
//! every reward then leaving lands the run where auto-collect does.

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_with_ascension,
};
use sts2_core::run_manager::{Phase, RunAction, RunManager, ScreenReward};
use sts2_core::CharacterId;

fn run(seed: i32, screen: bool) -> RunManager {
    let mut rm = new_run_manager_for_character_with_ascension(seed, CharacterId::Ironclad, 1);
    rm.run_state.interactive_rewards = screen;
    initialize_run_setup(&mut rm);
    rm
}

/// First map node, then every fight played by the first playable card (HP
/// pinned high so it is won), until the first fight is over.
fn through_first_fight(rm: &mut RunManager) {
    let mut fought = false;
    for _ in 0..2000 {
        match rm.phase() {
            Phase::Combat => {
                if !fought {
                    rm.debug_set_player(Some(9999), Some(9999), None);
                    fought = true;
                }
                let actions = rm.get_available_actions();
                let a = actions
                    .iter()
                    .copied()
                    .find(|a| matches!(a, RunAction::PlayCard { .. }))
                    .unwrap_or(actions[0]);
                rm.take_action(a);
            }
            _ if fought => return,
            _ => {
                let a = rm.get_available_actions()[0];
                rm.take_action(a);
            }
        }
    }
    panic!("the first fight never ended");
}

#[test]
fn screen_rolls_what_auto_collect_applies() {
    for seed in [1, 7, 42, 1234, 99991] {
        let mut auto = run(seed, false);
        let mut screen = run(seed, true);
        through_first_fight(&mut auto);
        through_first_fight(&mut screen);
        assert_eq!(
            auto.run_state.rng.rewards.counter(),
            screen.run_state.rng.rewards.counter(),
            "seed {seed}: the screen reads the Rewards stream as auto-collect does"
        );
        let listed = screen
            .reward_screen()
            .expect("a won fight opens the screen")
            .to_vec();
        let gold: i32 = listed
            .iter()
            .filter_map(|r| match r {
                ScreenReward::Gold { amount } => Some(*amount),
                _ => None,
            })
            .sum();
        assert_eq!(
            screen.run_state.gold + gold,
            auto.run_state.gold,
            "seed {seed}: gold"
        );
        let potions: Vec<_> = listed
            .iter()
            .filter_map(|r| match r {
                ScreenReward::Potion { kind } => Some(*kind),
                _ => None,
            })
            .collect();
        let mut expected = screen.run_state.potions.clone();
        expected.extend(potions);
        assert_eq!(expected, auto.run_state.potions, "seed {seed}: potions");
        let cards: Vec<_> = listed
            .iter()
            .filter_map(|r| match r {
                ScreenReward::Card { options, .. } => Some(options.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            cards,
            vec![auto.card_reward_options().to_vec()],
            "seed {seed}: card reward"
        );

        // Claim everything (the card: its first option) and leave; the
        // auto-collect run picks the same card.
        while let Some(idx) = screen.get_available_actions().iter().find_map(|a| match a {
            RunAction::ClaimReward { idx } => Some(*idx),
            _ => None,
        }) {
            screen.take_action(RunAction::ClaimReward { idx });
            if screen.screen_card_claim().is_some() {
                screen.take_action(RunAction::PickCardReward { idx: Some(0) });
            }
        }
        screen.take_action(RunAction::ProceedRewards);
        auto.take_action(RunAction::PickCardReward { idx: Some(0) });
        assert_eq!(
            screen.phase(),
            auto.phase(),
            "seed {seed}: phase after the rewards"
        );
        assert_eq!(screen.run_state.gold, auto.run_state.gold, "seed {seed}");
        assert_eq!(
            screen.run_state.potions, auto.run_state.potions,
            "seed {seed}"
        );
        assert_eq!(
            screen.run_state.relics, auto.run_state.relics,
            "seed {seed}"
        );
        let deck = |rm: &RunManager| {
            rm.run_state
                .master_deck
                .iter()
                .map(|c| (c.card_id, c.upgraded))
                .collect::<Vec<_>>()
        };
        assert_eq!(deck(&screen), deck(&auto), "seed {seed}");
    }
}

#[test]
fn auto_collect_is_unchanged_without_the_flag() {
    let mut rm = run(42, false);
    through_first_fight(&mut rm);
    assert!(rm.reward_screen().is_none());
    assert!(rm
        .get_available_actions()
        .iter()
        .all(|a| !matches!(a, RunAction::ClaimReward { .. } | RunAction::ProceedRewards)));
}
