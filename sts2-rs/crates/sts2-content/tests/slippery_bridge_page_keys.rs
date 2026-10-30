//! Slippery Bridge's Hold On pages.
//!
//! The game's `SlipperyBridge.HoldOn` sets a new page each time with the Hold
//! On option keyed `SLIPPERY_BRIDGE.pages.HOLD_ON_<n-1>.options.HOLD_ON_<n>`
//! (n = holds so far, each suffix `LOOP` from 7 on); Overcome keeps its
//! INITIAL key. The engine kept `pages.INITIAL.options.HOLD_ON_0` on every
//! page (scenario event_loops_not_cut, step 8). The expected keys are the
//! Headless host's (agent mode, seed "3", entered by the harness).

use sts2_content::events::dispatch::present;
use sts2_content::run_callbacks::{
    initialize_run_setup, new_ironclad_run_manager_from_string_seed,
};
use sts2_core::events::EventId;
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};

fn on_bridge(seed: &str) -> RunManager {
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
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Event(EventId::SlipperyBridge)),
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

#[test]
fn each_hold_on_page_has_the_games_key() {
    let mut rm = on_bridge("3");
    assert_eq!(
        keys(&rm),
        [
            "SLIPPERY_BRIDGE.pages.INITIAL.options.OVERCOME",
            "SLIPPERY_BRIDGE.pages.INITIAL.options.HOLD_ON_0",
        ]
    );
    let expected = [
        "SLIPPERY_BRIDGE.pages.HOLD_ON_0.options.HOLD_ON_1",
        "SLIPPERY_BRIDGE.pages.HOLD_ON_1.options.HOLD_ON_2",
        "SLIPPERY_BRIDGE.pages.HOLD_ON_2.options.HOLD_ON_3",
        "SLIPPERY_BRIDGE.pages.HOLD_ON_3.options.HOLD_ON_4",
        "SLIPPERY_BRIDGE.pages.HOLD_ON_4.options.HOLD_ON_5",
        "SLIPPERY_BRIDGE.pages.HOLD_ON_5.options.HOLD_ON_6",
        "SLIPPERY_BRIDGE.pages.HOLD_ON_6.options.HOLD_ON_LOOP",
        "SLIPPERY_BRIDGE.pages.HOLD_ON_LOOP.options.HOLD_ON_LOOP",
        "SLIPPERY_BRIDGE.pages.HOLD_ON_LOOP.options.HOLD_ON_LOOP",
    ];
    for want in expected {
        assert_eq!(
            rm.take_action(RunAction::EventChoice { idx: 1 }),
            ActionResult::Advanced
        );
        assert_eq!(
            keys(&rm),
            ["SLIPPERY_BRIDGE.pages.INITIAL.options.OVERCOME", want]
        );
    }
}
