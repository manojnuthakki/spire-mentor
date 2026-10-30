//! The belt is locked from an event's first page.
//!
//! The game's TheFutureOfPotions, StoneOfAllTime and RanwidTheElder set
//! `Owner.CanRemovePotions = false` in `BeforeEventStarted`, before the first
//! page is shown, and restore it in `OnEventFinished`. Entering such an event
//! from the map, the player can neither drink nor discard a potion before
//! choosing anything.

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_with_ascension,
};
use sts2_core::events::EventId;
use sts2_core::map::MapPointType;
use sts2_core::potions::{CharacterId, PotionKind};
use sts2_core::run_manager::{Phase, RunAction};
use sts2_core::runstate::RoomType;

fn install() {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
}

#[test]
fn a_belt_locking_event_entered_from_the_map_locks_the_belt_before_any_choice() {
    install();
    for event in [
        EventId::TheFutureOfPotions,
        EventId::StoneOfAllTime,
        EventId::RanwidTheElder,
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
        rm.run_state.interactive_potions = true;
        rm.run_state.max_potion_slots = 3;
        rm.run_state.potions = vec![PotionKind::BloodPotion, PotionKind::FirePotion];
        // Every room is an unknown room that can only be an event, and the
        // act's next event is `event`.
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

        let mv = rm
            .get_available_actions()
            .into_iter()
            .find(|a| matches!(a, RunAction::Move { .. }))
            .expect("a map move");
        rm.take_action(mv);

        assert_eq!(rm.phase(), Phase::Event, "{event:?}");
        assert_eq!(
            rm.run_state.current_event,
            Some(event),
            "{event:?} bound on entry"
        );
        assert!(rm.potions_locked(), "{event:?}: the belt is locked");
        let belt_actions: Vec<RunAction> = rm
            .get_available_actions()
            .into_iter()
            .filter(|a| {
                matches!(
                    a,
                    RunAction::DrinkPotion { .. } | RunAction::DiscardPotion { .. }
                )
            })
            .collect();
        assert!(belt_actions.is_empty(), "{event:?}: {belt_actions:?}");
    }
}
