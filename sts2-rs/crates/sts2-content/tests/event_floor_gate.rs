//! Floor-gated event rules read the game's floor at the pull.
//!
//! The game pulls a room's event in CreateRoom, before the room enters the map
//! point history (RunManager.EnterMapPointInternal), so `TotalFloor` there counts
//! the rooms before this one, each act's Ancient included. Slippery Bridge needs
//! `TotalFloor > 6`, Punch Off `TotalFloor >= 6`. The engine picks after the map
//! move's floor bump and added one more, offering both a floor early
//! (APEGNEW8XT, U3SKSJ0AS6: Slippery Bridge at floor 7).

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_with_ascension,
};
use sts2_core::events::EventId;
use sts2_core::map::MapPointType;
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::{Phase, RunAction};
use sts2_core::runstate::RoomType;

fn install() {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
}

/// The event bound on entering an unknown room from the map, with the act's
/// event order `order` and the engine's `total_floor` at `before` before the move.
fn bound(order: &[EventId], before: i32) -> Option<EventId> {
    let mut rm = new_run_manager_for_character_with_ascension(3, CharacterId::Ironclad, 0);
    initialize_run_setup(&mut rm);
    for _ in 0..50 {
        if rm.phase() == Phase::MapChoice {
            break;
        }
        let a = rm.get_available_actions()[0];
        rm.take_action(a);
    }
    assert_eq!(rm.phase(), Phase::MapChoice);
    for rt in [
        RoomType::Monster,
        RoomType::Elite,
        RoomType::Treasure,
        RoomType::Shop,
    ] {
        rm.run_state.unknown_map_point_odds.current.insert(rt, 0.0);
    }
    let act = usize::from(rm.run_state.act_index) - 1;
    rm.run_state.generated_rooms[act].events_shuffled =
        order.iter().map(|e| e.python_name()).collect();
    let map = rm.run_state.current_act_map.as_mut().expect("a map");
    for p in &mut map.points {
        if p.point_type != MapPointType::Boss {
            p.point_type = MapPointType::Unknown;
        }
    }
    rm.run_state.total_floor = before;
    let mv = rm
        .get_available_actions()
        .into_iter()
        .find(|a| matches!(a, RunAction::Move { .. }))
        .expect("a map move");
    rm.take_action(mv);
    rm.run_state.current_event
}

#[test]
fn slippery_bridge_is_offered_from_floor_8() {
    install();
    let order = [EventId::SlipperyBridge, EventId::BrainLeech];
    // Floor 7 (Neow and six rooms; TotalFloor 6 at the pull): not yet.
    assert_eq!(bound(&order, 5), Some(EventId::BrainLeech));
    // Floor 8 (TotalFloor 7 at the pull).
    assert_eq!(bound(&order, 6), Some(EventId::SlipperyBridge));
}

#[test]
fn punch_off_is_offered_from_floor_7() {
    install();
    let order = [EventId::PunchOff, EventId::BrainLeech];
    // Floor 6 (TotalFloor 5 at the pull): not yet.
    assert_eq!(bound(&order, 4), Some(EventId::BrainLeech));
    // Floor 7 (TotalFloor 6 at the pull).
    assert_eq!(bound(&order, 5), Some(EventId::PunchOff));
}
