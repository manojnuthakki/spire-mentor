//! Doll Room's pages.
//!
//! The game's `DollRoom` opens on RANDOM, TAKE_SOME_TIME and EXAMINE only;
//! TakeSomeTime / Examine show `_dolls.StableShuffle(base.Rng)` (two / three),
//! each option keyed by its relic's title (`OptionFromChoice`: the text key is
//! `relic.Title`'s raw text, `<RELIC>.title` on the host). `StableShuffle`
//! sorts by `RelicModel.CompareTo` (the ids' ordinal order) first. The engine
//! listed the dolls on the first page too (08H6B8FJ5U, A10 walk), keyed them
//! by nothing, and shuffled them in declaration order. The expected pages are
//! the Headless host's (agent mode, seeds "3" and "7", entered by the harness).

use sts2_content::events::dispatch::present;
use sts2_content::run_callbacks::{
    initialize_run_setup, new_ironclad_run_manager_from_string_seed,
};
use sts2_core::events::EventId;
use sts2_core::relics::RelicId;
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};

fn in_doll_room(seed: &str) -> RunManager {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
    let mut rm = new_ironclad_run_manager_from_string_seed(seed);
    initialize_run_setup(&mut rm);
    for _ in 0..50 {
        if rm.phase() == Phase::MapChoice {
            break;
        }
        let a = rm.get_available_actions()[0];
        rm.take_action(a);
    }
    rm.run_state.direct_event_relic_grant = true;
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Event(EventId::DollRoom)),
        ActionResult::Advanced
    );
    rm
}

fn keys(rm: &RunManager) -> Vec<&'static str> {
    present(&rm.run_state)
        .unwrap_or_default()
        .into_iter()
        .map(|o| o.loc_key)
        .collect()
}

fn choose(rm: &mut RunManager, idx: usize) {
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx }),
        ActionResult::Advanced
    );
}

#[test]
fn the_first_page_is_the_three_approaches() {
    let rm = in_doll_room("3");
    assert_eq!(
        keys(&rm),
        [
            "DOLL_ROOM.pages.INITIAL.options.RANDOM",
            "DOLL_ROOM.pages.INITIAL.options.TAKE_SOME_TIME",
            "DOLL_ROOM.pages.INITIAL.options.EXAMINE",
        ]
    );
}

#[test]
fn the_doll_pages_are_the_hosts() {
    let mut rm = in_doll_room("3");
    choose(&mut rm, 1);
    assert_eq!(keys(&rm), ["BING_BONG.title", "MR_STRUGGLES.title"]);
    let mut rm = in_doll_room("3");
    choose(&mut rm, 2);
    assert_eq!(
        keys(&rm),
        [
            "BING_BONG.title",
            "MR_STRUGGLES.title",
            "DAUGHTER_OF_THE_WIND.title"
        ]
    );
    let mut rm = in_doll_room("7");
    let hp = rm.run_state.current_hp;
    choose(&mut rm, 2);
    assert_eq!(rm.run_state.current_hp, hp - 15);
    assert_eq!(
        keys(&rm),
        [
            "DAUGHTER_OF_THE_WIND.title",
            "MR_STRUGGLES.title",
            "BING_BONG.title"
        ]
    );
    choose(&mut rm, 1);
    assert!(rm.run_state.relics.contains(&RelicId::MrStruggles));
    assert_eq!(rm.run_state.current_event, None, "finished");
}
