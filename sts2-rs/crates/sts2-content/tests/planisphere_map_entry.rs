//! Planisphere reads the standing map point, not the resolved room.
//! Authority: sts2.dll v0.107.1, Models.Relics.Planisphere.AfterRoomEntered,
//! Runs.RunState.CurrentMapPoint, Commands.CreatureCmd.Heal and Creature.HealInternal.
//! Scripted maps below exercise actual Move actions; forced entry never invents
//! a map visit. No new entry hooks or state are introduced by this correction.

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_with_ascension,
};
use sts2_core::map::{ActMap, MapCoord, MapPointType};
use sts2_core::potions::CharacterId;
use sts2_core::relics::RelicId;
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};
use sts2_core::runstate::{Room, RoomType, RunState};

fn install() {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
}

fn map(point: MapPointType) -> ActMap {
    let mut map = ActMap::new(1);
    let start = map.get_or_create(0, 0);
    let next = map.get_or_create(0, 1);
    let boss = map.get_or_create(0, 2);
    map.point_mut(start).point_type = MapPointType::Ancient;
    map.point_mut(next).point_type = point;
    map.point_mut(boss).point_type = MapPointType::Boss;
    map.add_child(start, next);
    map.add_child(next, boss);
    map.start = Some(start);
    map.boss = Some(boss);
    map
}

fn standing(point: MapPointType) -> RunState {
    install();
    let mut rs = RunState::new();
    rs.current_act_map = Some(map(point));
    rs.relics = vec![RelicId::Planisphere];
    rs.current_hp = 30;
    rs.max_hp = 80;
    rs
}

fn enter(rs: &mut RunState, point: MapPointType, room: RoomType) {
    rs.enter_room(Room::at(MapCoord::new(0, 1), point, room));
}

#[test]
fn unknown_point_heals_for_each_resolved_room_kind() {
    for room in [RoomType::Shop, RoomType::Event, RoomType::Monster, RoomType::Treasure] {
        let mut rs = standing(MapPointType::Unknown);
        enter(&mut rs, MapPointType::Unknown, room);
        assert_eq!(rs.current_hp, 35, "resolved {room:?}");
    }
}

#[test]
fn resolved_unknown_without_a_current_map_point_does_not_heal() {
    let mut rs = standing(MapPointType::Unknown);
    rs.enter_room(Room::new(RoomType::Unknown));
    assert_eq!(rs.current_hp, 30);
    // A stale/absent coordinate also cannot identify a point on this map.
    rs.visited_map_coords.push(MapCoord::new(9, 9));
    rs.enter_room(Room::new(RoomType::Unknown));
    assert_eq!(rs.current_hp, 30);
}

#[test]
fn ordinary_points_do_not_heal_even_with_a_resolved_unknown_room() {
    for point in [MapPointType::Shop, MapPointType::Monster, MapPointType::Elite,
        MapPointType::Boss, MapPointType::Treasure, MapPointType::RestSite,
        MapPointType::Ancient] {
        let mut rs = standing(point);
        enter(&mut rs, point, RoomType::Unknown);
        assert_eq!(rs.current_hp, 30, "point {point:?}");
    }
}

#[test]
fn healing_is_exactly_five_with_max_hp_cap_and_no_rest_site_modifier() {
    for (hp, expected) in [(1, 6), (30, 35), (77, 80), (80, 80)] {
        let mut rs = standing(MapPointType::Unknown);
        rs.current_hp = hp;
        rs.relics.push(RelicId::RegalPillow);
        enter(&mut rs, MapPointType::Unknown, RoomType::Event);
        assert_eq!(rs.current_hp, expected);
    }
}

#[test]
fn absent_melted_and_dead_player_controls() {
    for control in 0..4 {
        let mut rs = standing(MapPointType::Unknown);
        match control {
            0 => rs.relics.clear(),
            1 => { rs.melted_relics.insert(RelicId::Planisphere); }
            2 => rs.current_hp = 0,
            _ => rs.current_hp = -1,
        }
        let before = rs.current_hp;
        enter(&mut rs, MapPointType::Unknown, RoomType::Event);
        assert_eq!(rs.current_hp, before);
    }
}

#[test]
fn act_transition_does_not_reuse_the_previous_unknown_point() {
    let mut rs = standing(MapPointType::Unknown);
    enter(&mut rs, MapPointType::Unknown, RoomType::Event);
    assert_eq!(rs.current_hp, 35);
    rs.advance_act();
    // The prior room may remain parked; CurrentMapPoint is now absent.
    let _ = sts2_core::run_effects::dispatch_after_room_entered(&mut rs, RoomType::Event);
    assert_eq!(rs.current_hp, 35);
}

fn manager(point: MapPointType, resolved: RoomType) -> RunManager {
    install();
    let mut rm = new_run_manager_for_character_with_ascension(7, CharacterId::Ironclad, 0);
    initialize_run_setup(&mut rm);
    for _ in 0..50 {
        if rm.phase() == Phase::MapChoice { break; }
        let action = rm.get_available_actions()[0];
        rm.take_action(action);
    }
    assert_eq!(rm.phase(), Phase::MapChoice);
    rm.run_state.current_act_map = Some(map(point));
    rm.run_state.current_room = None;
    rm.run_state.visited_map_coords.clear();
    rm.run_state.act_floor = 0;
    rm.run_state.total_floor = 0;
    rm.run_state.relics = vec![RelicId::Planisphere, RelicId::MawBank];
    rm.run_state.current_hp = 30;
    rm.run_state.max_hp = 80;
    // Deterministic resolver input; the move still rolls/resolves/enters via
    // the real manager. Event is the residual when all other odds are zero.
    for (kind, odds) in &mut rm.run_state.unknown_map_point_odds.current {
        *odds = if *kind == resolved { 1.0 } else { 0.0 };
    }
    rm
}

#[test]
fn actual_map_moves_heal_once_before_the_first_room_decision_and_combat_snapshot() {
    for resolved in [RoomType::Shop, RoomType::Event, RoomType::Monster] {
        let mut rm = manager(MapPointType::Unknown, resolved);
        let gold = rm.run_state.gold;
        assert_eq!(rm.take_action(RunAction::Move { col: 0 }), ActionResult::Advanced);
        assert_eq!(rm.run_state.current_room.unwrap().room_type, resolved);
        assert_eq!(rm.run_state.current_hp, 35, "{resolved:?}");
        assert_eq!(rm.run_state.gold, gold + 12, "Maw Bank fires once");
        assert_eq!(rm.run_state.total_floor, 1);
        if let Some(combat) = &rm.combat {
            assert_eq!(combat.player.current_hp, 35);
        }
        let _ = rm.get_available_actions();
        let _ = rm.get_available_actions();
        assert_eq!(rm.run_state.current_hp, 35, "observation is not re-entry");
    }
}

#[test]
fn clone_and_refused_move_do_not_repeat_or_share_healing() {
    let mut rm = manager(MapPointType::Unknown, RoomType::Shop);
    let mut clone = rm.clone();
    assert_eq!(rm.take_action(RunAction::Move { col: 9 }), ActionResult::InvalidForPhase);
    assert_eq!(rm.run_state.current_hp, 30);
    assert_eq!(rm.take_action(RunAction::Move { col: 0 }), ActionResult::Advanced);
    assert_eq!(rm.run_state.current_hp, 35);
    assert_eq!(clone.run_state.current_hp, 30);
    assert_eq!(clone.take_action(RunAction::Move { col: 0 }), ActionResult::Advanced);
    assert_eq!(clone.run_state.current_hp, 35);
    assert_eq!(rm.run_state, clone.run_state);
}

#[test]
fn forced_reentry_uses_the_actual_standing_point_without_another_map_visit() {
    for point in [MapPointType::Unknown, MapPointType::Shop] {
        let mut rm = manager(point, RoomType::Shop);
        // No standing point: a forced shop does not manufacture Unknown.
        assert_eq!(rm.force_enter_room(ForcedRoom::Shop), ActionResult::Advanced);
        assert_eq!(rm.run_state.current_hp, 30);
        // Start the independent natural-map case.
        let mut rm = manager(point, RoomType::Shop);
        assert_eq!(rm.take_action(RunAction::Move { col: 0 }), ActionResult::Advanced);
        let heal = if point == MapPointType::Unknown { 5 } else { 0 };
        assert_eq!(rm.run_state.current_hp, 30 + heal);
        let floor = rm.run_state.total_floor;
        let gold = rm.run_state.gold;
        assert_eq!(rm.force_enter_room(ForcedRoom::Shop), ActionResult::Advanced);
        assert_eq!(rm.run_state.current_hp, 30 + heal * 2);
        assert_eq!(rm.run_state.total_floor, floor);
        assert_eq!(rm.run_state.gold, gold + 12);
        assert_eq!(rm.force_enter_room(ForcedRoom::Combat("NO_SUCH_ENCOUNTER")), ActionResult::InvalidForPhase);
        assert_eq!(rm.run_state.current_hp, 30 + heal * 2);
    }
}
