//! Spoils Map in the deck replaces act 2's map with the Spoils map.
//!
//! The game's SpoilsMap (a Quest card) answers ModifyGeneratedMap for act 2
//! (`SpoilsActIndex` 1) with a new SpoilsActMap on its own `spoils_map` stream,
//! marks the map's treasure point with its quest, and pays 600 gold (leaving
//! the deck) when that treasure is opened. The engine kept the standard map
//! (MVFP85R5RK at A5: Headless's act 2 map has 48 nodes, the Sim's 59).

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_from_string_seed,
};
use sts2_core::cards::CardId;
use sts2_core::map::MapPointType;
use sts2_core::potions::CharacterId;

fn act2(spoils: bool) -> sts2_core::run_manager::RunManager {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut rm =
        new_run_manager_for_character_from_string_seed("MVFP85R5RK", CharacterId::Ironclad);
    initialize_run_setup(&mut rm);
    if spoils {
        let card = sts2_content::cards::spec_index::card_pod_from_id(CardId::SpoilsMap, false)
            .expect("pod");
        rm.run_state.master_deck.push(card);
    }
    rm.run_state.act_index = 2;
    rm.run_state.generate_map();
    rm
}

fn act2_map_nodes(spoils: bool) -> (usize, Vec<(i32, i32)>) {
    let rm = act2(spoils);
    let map = rm.run_state.current_act_map.as_ref().expect("act 2's map");
    let treasures = map
        .points
        .iter()
        .filter(|p| p.point_type == MapPointType::Treasure)
        .map(|p| (p.coord.col, p.coord.row))
        .collect();
    (map.grid.len(), treasures)
}

#[test]
fn spoils_map_replaces_act_2s_map() {
    let (standard, _) = act2_map_nodes(false);
    // The grid holds the boss too: the walk's map nodes are one fewer.
    assert_eq!(standard, 60, "the standard map, as the Sim showed it (59)");
    let (spoils, treasures) = act2_map_nodes(true);
    assert_eq!(spoils, 49, "Headless's act 2 map (48)");
    assert_eq!(treasures, [(3, 8)]);
}

/// The chest at the quest's treasure pays each Spoils Map 600 gold, after the
/// chest's own gold, and the card leaves the deck (C#
/// `OneOffSynchronizer.TryHandleSpoilsMap`, `SpoilsMap.OnQuestComplete`).
#[test]
fn the_quests_chest_pays_600_and_the_map_leaves_the_deck() {
    let opened = |keep: bool| {
        let mut rm = act2(true);
        let coord = rm.run_state.spoils_coord.expect("the quest's treasure");
        if !keep {
            rm.run_state
                .master_deck
                .retain(|c| c.card_id != CardId::SpoilsMap);
        }
        rm.run_state.current_room = Some(sts2_core::runstate::Room::at(
            coord,
            MapPointType::Treasure,
            sts2_core::runstate::RoomType::Treasure,
        ));
        let gold = rm.run_state.gold;
        rm.force_enter_room(sts2_core::run_manager::ForcedRoom::Treasure);
        let maps = rm
            .run_state
            .master_deck
            .iter()
            .filter(|c| c.card_id == CardId::SpoilsMap)
            .count();
        (rm.run_state.gold - gold, maps)
    };
    let (plain, _) = opened(false);
    let (quest, maps) = opened(true);
    assert_eq!(quest - plain, 600);
    assert_eq!(maps, 0);
}
