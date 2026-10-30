//! An event room entered from the map runs its room-entry hooks once.
//!
//! The game's EventRoom.EnterInternal binds the event and then runs
//! Hook.AfterRoomEntered once; Maw Bank (MawBank.AfterRoomEntered) pays 12 gold
//! per room. The engine's map move entered the room, and binding its event
//! entered it again, so Maw Bank paid 24 (39RE6GZJPL, X1WLDAFFQ7).

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_with_ascension,
};
use sts2_core::events::EventId;
use sts2_core::map::MapPointType;
use sts2_core::potions::CharacterId;
use sts2_core::relics::RelicId;
use sts2_core::run_manager::{Phase, RunAction};
use sts2_core::runstate::RoomType;

fn install() {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
}

#[test]
fn maw_bank_pays_once_on_entering_an_event_room_from_the_map() {
    install();
    for event in [
        EventId::LostWisp,
        EventId::SunkenTreasury,
        EventId::ThisOrThat,
    ] {
        let mut rm = new_run_manager_for_character_with_ascension(3, CharacterId::Ironclad, 0);
        initialize_run_setup(&mut rm);
        for _ in 0..50 {
            if rm.phase() == Phase::MapChoice {
                break;
            }
            let a = rm.get_available_actions()[0];
            rm.take_action(a);
        }
        assert_eq!(rm.phase(), Phase::MapChoice, "{event:?}");
        rm.run_state.obtain_relic(RelicId::MawBank);
        for rt in [
            RoomType::Monster,
            RoomType::Elite,
            RoomType::Treasure,
            RoomType::Shop,
        ] {
            rm.run_state.unknown_map_point_odds.current.insert(rt, 0.0);
        }
        let act = usize::from(rm.run_state.act_index) - 1;
        rm.run_state.generated_rooms[act].events_shuffled = vec![event.python_name()];
        let map = rm.run_state.current_act_map.as_mut().expect("a map");
        for p in &mut map.points {
            if p.point_type != MapPointType::Boss {
                p.point_type = MapPointType::Unknown;
            }
        }
        let gold = rm.run_state.gold;
        let mv = rm
            .get_available_actions()
            .into_iter()
            .find(|a| matches!(a, RunAction::Move { .. }))
            .expect("a map move");
        rm.take_action(mv);
        assert_eq!(rm.run_state.current_event, Some(event), "{event:?} bound");
        assert_eq!(
            rm.run_state.gold - gold,
            12,
            "{event:?}: Maw Bank pays once"
        );
    }
}
