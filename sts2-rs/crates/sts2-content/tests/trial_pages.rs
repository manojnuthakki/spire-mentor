//! Trial's later pages.
//!
//! The game's `Trial.Accept` picks the case with the event stream's
//! `NextInt(3)` (MERCHANT, NOBLE, NONDESCRIPT) and shows that case's page
//! (GUILTY / INNOCENT); `Reject` shows the REJECT page (ACCEPT, DOUBLE_DOWN).
//! The engine finished the event on ACCEPT or REJECT and went back to the map
//! (EATY6KEJFL: NOBLE on Headless, the map on the Sim; seed "1": MERCHANT, the
//! host's scenario trial_case_page_end_to_end).

use sts2_content::events::dispatch::present;
use sts2_content::run_callbacks::{
    initialize_run_setup, new_ironclad_run_manager_from_string_seed,
};
use sts2_core::cards::CardId;
use sts2_core::events::EventId;
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};

fn install() {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
}

/// A run at its first map choice, the Sim's interactive flags on, in Trial.
fn in_trial(seed: &str) -> RunManager {
    install();
    let mut rm = new_ironclad_run_manager_from_string_seed(seed);
    initialize_run_setup(&mut rm);
    for _ in 0..50 {
        if rm.phase() == Phase::MapChoice {
            break;
        }
        let a = rm.get_available_actions()[0];
        rm.take_action(a);
    }
    assert_eq!(rm.phase(), Phase::MapChoice);
    let rs = &mut rm.run_state;
    rs.interactive_card_upgrade = true;
    rs.interactive_card_transform = true;
    rs.interactive_rewards = true;
    rs.direct_event_relic_grant = true;
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Event(EventId::Trial)),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::Event);
    rm
}

fn page(rm: &RunManager) -> Vec<(&'static str, bool)> {
    present(&rm.run_state)
        .unwrap_or_default()
        .into_iter()
        .map(|o| (o.loc_key, o.enabled))
        .collect()
}

fn choose(rm: &mut RunManager, idx: usize) {
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx }),
        ActionResult::Advanced
    );
}

fn count(rm: &RunManager, id: CardId) -> usize {
    rm.run_state
        .master_deck
        .iter()
        .filter(|c| c.card_id == id)
        .count()
}

#[test]
fn accept_shows_the_noble_page_and_innocent_pays_300_with_regret() {
    let mut rm = in_trial("EATY6KEJFL");
    assert_eq!(
        page(&rm),
        [
            ("TRIAL.pages.INITIAL.options.ACCEPT", true),
            ("TRIAL.pages.INITIAL.options.REJECT", true)
        ]
    );
    choose(&mut rm, 0);
    assert_eq!(rm.phase(), Phase::Event, "the case's page, not the map");
    assert_eq!(
        page(&rm),
        [
            ("TRIAL.pages.NOBLE.options.GUILTY", true),
            ("TRIAL.pages.NOBLE.options.INNOCENT", true)
        ]
    );
    let gold = rm.run_state.gold;
    choose(&mut rm, 1);
    assert_eq!(rm.run_state.gold - gold, 300);
    assert_eq!(count(&rm, CardId::Regret), 1);
    assert_eq!(rm.run_state.current_event, None, "finished");
}

#[test]
fn reject_then_accept_shows_the_merchant_page_and_innocent_upgrades_two() {
    let mut rm = in_trial("1");
    choose(&mut rm, 1);
    assert_eq!(
        page(&rm),
        [
            ("TRIAL.pages.REJECT.options.ACCEPT", true),
            ("TRIAL.pages.REJECT.options.DOUBLE_DOWN", false)
        ]
    );
    choose(&mut rm, 0);
    assert_eq!(
        page(&rm),
        [
            ("TRIAL.pages.MERCHANT.options.GUILTY", true),
            ("TRIAL.pages.MERCHANT.options.INNOCENT", true)
        ]
    );
    choose(&mut rm, 1);
    assert_eq!(count(&rm, CardId::Shame), 1);
    assert_eq!(rm.run_state.pending_deck_upgrades, 2, "upgrade exactly 2");
}

#[test]
fn the_case_is_the_event_streams_first_read() {
    // 2QCCD7D3E8: the MERCHANT page on Headless (the A5 walk).
    let mut rm = in_trial("2QCCD7D3E8");
    choose(&mut rm, 0);
    assert_eq!(page(&rm)[0].0, "TRIAL.pages.MERCHANT.options.GUILTY");
}

/// The first of seeds "2".. whose case is NONDESCRIPT.
fn nondescript_seed() -> String {
    (2..100)
        .map(|n| n.to_string())
        .find(|s| {
            let mut rm = in_trial(s);
            choose(&mut rm, 0);
            page(&rm)[0].0 == "TRIAL.pages.NONDESCRIPT.options.GUILTY"
        })
        .expect("a NONDESCRIPT seed")
}

#[test]
fn nondescript_guilty_offers_two_card_rewards_and_innocent_transforms_two() {
    let seed = nondescript_seed();
    let mut rm = in_trial(&seed);
    choose(&mut rm, 0);
    choose(&mut rm, 0);
    assert_eq!(count(&rm, CardId::Doubt), 1);
    let screen = rm.reward_screen().expect("the OfferCustom set's screen");
    assert_eq!(screen.len(), 2, "two card rewards");

    let mut rm = in_trial(&seed);
    choose(&mut rm, 0);
    choose(&mut rm, 1);
    assert_eq!(count(&rm, CardId::Doubt), 1);
    let rolls: Vec<_> = rm
        .run_state
        .pending_deck_transforms
        .iter()
        .map(|t| t.event_stream_roll)
        .collect();
    assert_eq!(
        rolls,
        [Some(("TRIAL", 1)), Some(("TRIAL", 2))],
        "transformed off the event stream after the case pick"
    );
}

#[test]
fn merchant_guilty_adds_regret_and_two_relics_and_noble_guilty_heals_ten() {
    let mut rm = in_trial("1");
    let relics = rm.run_state.relics.len();
    choose(&mut rm, 0);
    choose(&mut rm, 0);
    assert_eq!(count(&rm, CardId::Regret), 1);
    assert_eq!(rm.run_state.relics.len(), relics + 2);

    let mut rm = in_trial("EATY6KEJFL");
    rm.run_state.current_hp = 30;
    choose(&mut rm, 0);
    choose(&mut rm, 0);
    assert_eq!(rm.run_state.current_hp, 40);
    assert_eq!(rm.run_state.current_event, None, "finished");
}
