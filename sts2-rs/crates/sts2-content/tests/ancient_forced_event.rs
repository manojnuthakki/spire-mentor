//! An Ancient entered as an event by the harness (`force_enter_room`): the
//! host presents the relics its `OptionPool` rolled, not the engine's former
//! fixed RELIC_1..3 stub. Lane F's F8 (`ancient-forced-as-event`), sts2-agent fork.

use sts2_content::events::dispatch::present;
use sts2_content::run_callbacks::{default_relic_grab_bag, ironclad_content_callbacks};
use sts2_core::events::EventId;
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};
use sts2_core::RelicId;

fn run(seed: i32) -> RunManager {
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

fn keys(rm: &RunManager) -> Vec<&'static str> {
    present(&rm.run_state)
        .expect("an event is open")
        .iter()
        .map(|o| o.loc_key)
        .collect()
}

#[test]
fn nonupeipe_offers_its_rolled_pool_with_the_games_text_keys() {
    let mut rm = run(5);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Event(EventId::Nonupeipe)),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::Event);
    let k = keys(&rm);
    assert_eq!(k.len(), 3);
    assert!(k
        .iter()
        .all(|k| k.starts_with("NONUPEIPE.pages.INITIAL.options.")));
    // The old stub's fixed offers are gone.
    assert!(!k
        .iter()
        .any(|k| k.ends_with("DELICATE_FROND_X") || k.ends_with("RELIC_1")));
    // The offers are the run's own roll for this Ancient, in its option order.
    let want = rm
        .run_state
        .roll_ancient_event_offers(sts2_core::run_room_gen::AncientId::Nonupeipe);
    let got: Vec<String> = k
        .iter()
        .map(|k| k.rsplit('.').next().unwrap().to_string())
        .collect();
    let want: Vec<String> = want
        .iter()
        .map(|r| r.to_python_name().to_string())
        .collect();
    assert_eq!(got, want);
}

#[test]
fn picking_an_option_obtains_the_relic_it_names() {
    let mut rm = run(5);
    rm.run_state.direct_event_relic_grant = true; // the run-parity flow, as the Sim sets it
    rm.force_enter_room(ForcedRoom::Event(EventId::Orobas));
    let k = keys(&rm);
    let want = k[1].rsplit('.').next().unwrap().to_string();
    assert_ne!(
        rm.take_action(RunAction::EventChoice { idx: 1 }),
        ActionResult::InvalidForPhase
    );
    let held: Vec<&str> = rm
        .run_state
        .relics
        .iter()
        .map(|r| r.to_python_name())
        .collect();
    assert!(held.contains(&want.as_str()), "{want} not in {held:?}");
}

#[test]
fn the_roll_follows_the_seed() {
    let offers = |seed| {
        let rm = run(seed);
        rm.run_state
            .roll_ancient_event_offers(sts2_core::run_room_gen::AncientId::Orobas)
    };
    // Not one fixed list (the stub's Electric Shrymp, Alchemical Coffer, Touch of Orobas).
    let fixed = vec![
        RelicId::ElectricShrymp,
        RelicId::AlchemicalCoffer,
        RelicId::TouchOfOrobas,
    ];
    assert!((1..12).any(|s| offers(s) != fixed));
    assert_eq!(offers(7), offers(7));
}
