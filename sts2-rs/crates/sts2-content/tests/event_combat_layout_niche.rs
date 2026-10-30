//! A combat-layout event creates its encounter's monsters at the room's entry.
//!
//! The game's EventRoom.EnterInternal calls GenerateInternalCombatState for an
//! event with a combat layout (Punch Off, The Lantern Key, The Architect):
//! CreateCreature for each monster draws its HP on RunState.Rng.Niche
//! (SetUniqueMonsterHpValue), whether or not the player fights, and the later
//! fight reuses those creatures. The engine drew nothing at entry and The
//! Lantern Key's knight's HP at the fight (VD5A1JD2AP's Whetstone, 7TQML37GFT's
//! Mytes).

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_with_ascension,
};
use sts2_core::events::EventId;
use sts2_core::map::MapPointType;
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::{ForcedRoom, Phase, RunAction};
use sts2_core::runstate::RoomType;

fn install() {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
}

fn at_map() -> sts2_core::run_manager::RunManager {
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
    rm
}

/// Niche's draws on entering an unknown room from the map that binds `event`.
fn niche_on_entry(event: EventId) -> u32 {
    let mut rm = at_map();
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
    rm.run_state.total_floor = 10;
    let before = rm.run_state.rng.niche.counter();
    let mv = rm
        .get_available_actions()
        .into_iter()
        .find(|a| matches!(a, RunAction::Move { .. }))
        .expect("a map move");
    rm.take_action(mv);
    assert_eq!(rm.run_state.current_event, Some(event));
    rm.run_state.rng.niche.counter() - before
}

#[test]
fn punch_off_draws_its_two_constructs_hp_on_entry() {
    install();
    let plain = niche_on_entry(EventId::BrainLeech);
    assert_eq!(niche_on_entry(EventId::PunchOff) - plain, 2);
}

#[test]
fn the_lantern_keys_knight_is_drawn_on_entry_and_kept_for_the_fight() {
    install();
    let mut rm = at_map();
    let before = rm.run_state.rng.niche.counter();
    rm.force_enter_room(ForcedRoom::Event(EventId::TheLanternKey));
    assert_eq!(rm.run_state.rng.niche.counter() - before, 1, "on entry");
    // Keep the key, then fight: no second draw.
    rm.take_action(RunAction::EventChoice { idx: 1 });
    rm.take_action(RunAction::EventChoice { idx: 0 });
    assert_eq!(rm.phase(), Phase::Combat);
    assert_eq!(rm.run_state.rng.niche.counter() - before, 1, "at the fight");
    let combat = rm.combat.as_ref().expect("the knight's fight");
    let knight = combat
        .creatures
        .iter()
        .find(|c| c.monster_id.0 == "MysteriousKnight" || c.max_hp > 90)
        .expect("the knight");
    assert_eq!(knight.max_hp, 101);
}
