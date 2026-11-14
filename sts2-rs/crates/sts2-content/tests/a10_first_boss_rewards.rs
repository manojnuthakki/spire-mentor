//! A10's final act has two bosses. C# `RewardsSet.WithRewardsFromRoom` returns an
//! empty set for any Boss room once `CurrentActIndex >= Acts.Count - 1`, so the
//! FIRST final-act boss offers no gold, potion or card reward either (nor extra
//! rewards or hook rewards); the rewards screen is empty and proceeding returns
//! to the map, where the second boss is the one choice
//! (`NRewardsScreen.cs:449-456`). The Headless scenario `ascension_double_boss`.
//! sts2-agent fork, `double-boss-first-boss-rewards`.

use sts2_content::run_callbacks::{
    default_relic_grab_bag, new_run_manager_for_character_with_ascension,
};
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};

fn on_first_boss(interactive: bool) -> RunManager {
    on_first_boss_with(interactive, |_| {})
}

fn on_first_boss_with(interactive: bool, setup: impl FnOnce(&mut RunManager)) -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_with_ascension(5, CharacterId::Ironclad, 10)
        .with_neow_start();
    let (by_rarity, fallback, boss) = default_relic_grab_bag();
    rm.run_state
        .populate_relic_grab_bag_by_rarity(by_rarity, fallback, boss);
    rm.run_state.interactive_rewards = interactive;
    rm.run_state.interactive_potions = interactive;
    rm.run_state.act_index = 3;
    rm.run_state.generate_map();
    let (boss_coord, point_type) = {
        let map = rm.run_state.current_act_map.as_ref().expect("map");
        let p = map.point(map.boss.expect("boss"));
        (p.coord, p.point_type)
    };
    rm.run_state.current_room = Some(sts2_core::runstate::Room::at(
        boss_coord,
        point_type,
        sts2_core::runstate::RoomType::Boss,
    ));
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Combat("KNOWLEDGE_DEMON_BOSS")),
        ActionResult::Advanced
    );
    assert!(rm.run_state.awaiting_second_boss());
    setup(&mut rm);
    rm.debug_set_player(Some(1_000_000), Some(1_000_000), None);
    for _ in 0..5000 {
        if rm.phase() != Phase::Combat {
            return rm;
        }
        let actions = rm.get_available_actions();
        let a = actions
            .iter()
            .copied()
            .find(|a| matches!(a, RunAction::PlayCard { .. }))
            .unwrap_or(actions[0]);
        rm.take_action(a);
    }
    panic!("the fight never ended");
}

#[test]
fn first_final_act_boss_screen_is_empty() {
    let mut rm = on_first_boss(true);
    let gold = rm.run_state.gold;
    assert_eq!(rm.phase(), Phase::CardReward);
    assert_eq!(rm.reward_screen().map(<[_]>::len), Some(0), "no rewards");
    assert!(rm.reward_screen_can_proceed());
    assert_eq!(
        rm.take_action(RunAction::ProceedRewards),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::MapChoice);
    assert_eq!(rm.run_state.gold, gold);
    assert_eq!(rm.run_state.act_index, 3);
    assert_eq!(rm.get_available_actions().len(), 1, "the second boss only");
}

#[test]
fn first_final_act_boss_gives_nothing_without_the_screen() {
    let rm = on_first_boss(false);
    assert_eq!(rm.phase(), Phase::MapChoice);
    assert!(rm.reward_screen().is_none());
}

/// Wongo's ticket pays out on its fifth combat whatever the room: the hooks
/// still run over the empty first-boss set (`GenerateWithoutOffering` ->
/// `Hook.ModifyRewards`), so the three relics are offered and the base
/// rewards stay out. Headless: Self-Forming Clay, Potion Belt, Ripple Basin.
#[test]
fn first_final_act_boss_still_runs_the_reward_hooks() {
    use sts2_core::relics::RelicId;
    let rm = on_first_boss_with(true, |rm| {
        rm.run_state.relics.push(RelicId::WongosMysteryTicket);
        rm.run_state
            .relic_counters
            .insert(RelicId::WongosMysteryTicket, 4);
    });
    assert_eq!(rm.phase(), Phase::CardReward);
    let screen = rm.reward_screen().expect("screen");
    assert_eq!(
        screen.len(),
        3,
        "the ticket's three relics only: {screen:?}"
    );
    assert!(screen
        .iter()
        .all(|r| matches!(r, sts2_core::run_manager::ScreenReward::Relic { .. })));
    assert!(
        rm.run_state.relic_counters[&RelicId::WongosMysteryTicket] < 0,
        "the ticket's state change stands"
    );
}
