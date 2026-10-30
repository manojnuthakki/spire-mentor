//! The harness's forced room entry (`RunManager::force_enter_room`), the
//! engine side of the C# headless `enter_room` (`RunSimulator.EnterRoomImpl`
//! → `RunManager.EnterRoom`), sts2-agent fork.

use sts2_content::run_callbacks::{default_relic_grab_bag, ironclad_content_callbacks};
use sts2_core::events::EventId;
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};
use sts2_core::runstate::RoomType;

fn neow_run(seed: i32) -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm = RunManager::new(seed, CharacterId::Ironclad)
        .with_content_callbacks(ironclad_content_callbacks())
        .with_neow_start();
    let (by_rarity, fallback, boss) = default_relic_grab_bag();
    rm.run_state
        .populate_relic_grab_bag_by_rarity(by_rarity, fallback, boss);
    rm
}

/// Neow's first option, then the first map node.
fn on_first_node(seed: i32) -> RunManager {
    let mut rm = neow_run(seed);
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 0 }),
        ActionResult::Advanced
    );
    while rm.phase() != Phase::MapChoice {
        // A reward screen after Neow's option: take the first way on.
        let first = rm.get_available_actions()[0];
        rm.take_action(first);
    }
    let mv = rm
        .get_available_actions()
        .into_iter()
        .find(|a| matches!(a, RunAction::Move { .. }))
        .expect("a map move");
    assert_eq!(rm.take_action(mv), ActionResult::Advanced);
    rm
}

fn enemy_count(rm: &RunManager) -> usize {
    rm.combat.as_ref().map_or(0, |c| c.enemy_uids.len())
}

#[test]
fn combat_from_neow_enters_the_encounter_without_a_floor() {
    let mut rm = neow_run(7);
    assert_eq!(rm.phase(), Phase::Event);
    let floor = rm.run_state.total_floor;
    let events = rm.run_state.acts[0].events_visited;
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Combat("NIBBITS_WEAK")),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::Combat);
    assert!(enemy_count(&rm) >= 1);
    // Neow's room is left; no map-point history, so no floor.
    assert_eq!(rm.run_state.current_event, None);
    assert_eq!(rm.run_state.total_floor, floor);
    assert_eq!(rm.run_state.acts[0].events_visited, events);
    // `MarkVisited(Monster)`: the normal cursor moved.
    assert_eq!(rm.run_state.monster_combats_this_act, 1);
    assert_eq!(
        rm.run_state.current_room.map(|r| r.room_type),
        Some(RoomType::Monster)
    );
}

#[test]
fn combat_room_type_is_the_encounters() {
    let mut rm = neow_run(7);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Combat("KNOWLEDGE_DEMON_BOSS")),
        ActionResult::Advanced
    );
    assert_eq!(
        rm.run_state.current_room.map(|r| r.room_type),
        Some(RoomType::Boss)
    );
    assert_eq!(enemy_count(&rm), 1);
    assert_eq!(rm.run_state.monster_combats_this_act, 0);
}

#[test]
fn unknown_encounter_changes_nothing() {
    let mut rm = neow_run(7);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Combat("NO_SUCH_ENCOUNTER")),
        ActionResult::InvalidForPhase
    );
    assert_eq!(rm.phase(), Phase::Event);
    assert_eq!(rm.run_state.current_event, Some(EventId::Neow));
}

#[test]
fn rooms_keep_the_map_position() {
    let mut rm = on_first_node(11);
    // Leave whatever the node opened (a fight, an event...) for a shop.
    let standing = rm.run_state.current_room.and_then(|r| r.map_coord);
    let floor = rm.run_state.total_floor;
    assert!(standing.is_some());
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Shop),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::Shop);
    assert!(rm.combat.is_none());
    assert_eq!(
        rm.run_state.current_room.and_then(|r| r.map_coord),
        standing
    );
    assert_eq!(rm.run_state.total_floor, floor);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::RestSite),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::RestSite);
    assert_eq!(rm.run_state.total_floor, floor);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Combat("MAWLER_NORMAL")),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::Combat);
    assert_eq!(
        rm.run_state.current_room.and_then(|r| r.map_coord),
        standing
    );
    assert_eq!(rm.run_state.total_floor, floor);
}

#[test]
fn event_room_moves_the_event_cursor() {
    let mut rm = on_first_node(11);
    let events = rm.run_state.acts[0].events_visited;
    let visited = rm.run_state.visited_event_ids.clone();
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Event(EventId::TheFutureOfPotions)),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::Event);
    assert_eq!(
        rm.run_state.current_event,
        Some(EventId::TheFutureOfPotions)
    );
    // `MarkVisited(Event)`, but no pool pull: the visited set is unchanged.
    assert_eq!(rm.run_state.acts[0].events_visited, events + 1);
    assert_eq!(rm.run_state.visited_event_ids, visited);
}

#[test]
fn treasure_room_opens() {
    let mut rm = neow_run(3);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Treasure),
        ActionResult::Advanced
    );
    assert_ne!(rm.phase(), Phase::Event);
    assert_eq!(
        rm.run_state.current_room.map(|r| r.room_type),
        Some(RoomType::Treasure)
    );
}

#[test]
fn leaving_a_live_fight_keeps_what_it_changed() {
    let mut rm = neow_run(7);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Combat("NIBBITS_WEAK")),
        ActionResult::Advanced
    );
    let combat = rm.combat.as_mut().expect("a fight");
    combat.player.current_hp -= 18;
    combat.player.max_hp += 2;
    combat.gold -= 5;
    let (hp, max_hp, gold) = (combat.player.current_hp, combat.player.max_hp, combat.gold);
    let belt = combat.player.potions.len();
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Shop),
        ActionResult::Advanced
    );
    // C#'s one `Player`: the fight's changes stay, and no combat-end
    // effect runs (Burning Blood heals only on a won fight).
    assert_eq!(rm.run_state.current_hp, hp);
    assert_eq!(rm.run_state.max_hp, max_hp);
    assert_eq!(rm.run_state.gold, gold);
    assert_eq!(rm.run_state.potions.len(), belt);
    assert!(rm.combat.is_none());
}

/// A run on its first node with Winged Boots, then one off-path move, or
/// `None` when the map offers none.
fn off_path_with_boots(seed: i32) -> Option<RunManager> {
    let mut rm = on_first_node(seed);
    rm.run_state.relics.push(sts2_core::RelicId::WingedBoots);
    // Whatever the node holds, a shop left at once gets back to the map.
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Shop),
        ActionResult::Advanced
    );
    rm.take_action(RunAction::LeaveShop);
    assert_eq!(rm.phase(), Phase::MapChoice);
    let coord = rm.run_state.current_room?.map_coord?;
    let children: Vec<i32> = rm
        .run_state
        .current_act_map
        .as_ref()?
        .children_of(coord)
        .iter()
        .map(|p| p.col())
        .collect();
    let mv = rm
        .get_available_actions()
        .into_iter()
        .find(|a| matches!(a, RunAction::Move { col } if !children.contains(col)))?;
    assert_eq!(rm.take_action(mv), ActionResult::Advanced);
    Some(rm)
}

#[test]
fn a_forced_entry_after_an_off_path_move_spends_winged_boots_again() {
    let mut rm = (1..40)
        .find_map(off_path_with_boots)
        .expect("a seed with an off-path move");
    assert_eq!(rm.run_state.winged_boots_off_path_used, 1);
    // `WingedBoots.AfterRoomEntered` reads the last two visited coords.
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Shop),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.winged_boots_off_path_used, 2);
}

#[test]
fn a_forced_entry_after_an_on_path_move_leaves_winged_boots() {
    let mut rm = on_first_node(7);
    rm.run_state.relics.push(sts2_core::RelicId::WingedBoots);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Shop),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.winged_boots_off_path_used, 0);
}

#[test]
fn gold_the_harness_sets_in_a_fight_stays_when_it_is_left() {
    let mut rm = neow_run(7);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Combat("NIBBITS_WEAK")),
        ActionResult::Advanced
    );
    rm.debug_set_player(Some(50), None, Some(999));
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Shop),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.gold, 999);
    assert_eq!(rm.run_state.current_hp, 50);
}
