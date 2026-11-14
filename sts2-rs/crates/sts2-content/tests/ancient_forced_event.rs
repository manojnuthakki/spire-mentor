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

/// A run as the host starts one: from the canonical string seed.
fn run_str(seed: &str) -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm = sts2_content::run_callbacks::new_run_manager_for_character_from_string_seed(
        seed,
        CharacterId::Ironclad,
    )
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

/// Different seeds roll different offers (a roll that ignores the run seed
/// gives one list for all), and a few seeds' offers are pinned (the algorithm
/// itself, `event_rng` + `roll_offers`, is pinned to the recordings by the
/// boss/ancient offer tests; these pin that the event feeds it the run seed).
#[test]
fn distinct_seeds_roll_distinct_offers() {
    use sts2_core::run_room_gen::AncientId;
    let mut distinct = std::collections::BTreeSet::new();
    for seed in 1..=24 {
        let rm = run(seed);
        let got = rm.run_state.roll_ancient_event_offers(AncientId::Orobas);
        distinct.insert(format!("{got:?}"));
    }
    assert!(
        distinct.len() >= 4,
        "only {} distinct offer sets",
        distinct.len()
    );
    let pinned = |seed| {
        format!(
            "{:?}",
            run(seed)
                .run_state
                .roll_ancient_event_offers(AncientId::Orobas)
        )
    };
    assert_eq!(pinned(1), "[SandCastle, Driftwood, TouchOfOrobas]");
    assert_eq!(pinned(2), "[ElectricShrymp, Driftwood, TouchOfOrobas]");
    assert_eq!(pinned(3), "[PrismaticGem, RadiantPearl, TouchOfOrobas]");
}

/// Step53's pre-merge gate: Darv, Tanx, Tezcatara and Vakuu entered as events
/// present the relics the game rolled under its text keys
/// (`<ANCIENT>.pages.INITIAL.options.<RELIC>`), not fixed RELIC_n/WEAPON_n/
/// COMFORT_n stubs or blank keys. The witnesses are the offers the real game
/// (the Headless host, Ironclad, seeds 5, 7 and 11, hp 500, act 1, entered with
/// `enter_room type event`) showed.
#[test]
fn darv_tanx_tezcatara_and_vakuu_offer_what_the_game_rolled() {
    #[rustfmt::skip]
    let witnesses: [(EventId, &str, [(i32, [&str; 3]); 3]); 4] = [
        (EventId::Darv, "DARV", [
            (5, ["BLACK_STAR", "RUNIC_PYRAMID", "CALLING_BELL"]),
            (7, ["SNECKO_EYE", "EMPTY_CAGE", "DUSTY_TOME"]),
            (11, ["SNECKO_EYE", "EMPTY_CAGE", "DUSTY_TOME"])]),
        (EventId::Tanx, "TANX", [
            (5, ["SPIKED_GAUNTLETS", "WAR_HAMMER", "IRON_CLUB"]),
            (7, ["THROWING_AXE", "SAI", "CLAWS"]),
            (11, ["SPIKED_GAUNTLETS", "TRI_BOOMERANG", "CROSSBOW"])]),
        (EventId::Tezcatara, "TEZCATARA", [
            (5, ["YUMMY_COOKIE", "BIIIG_HUG", "TOY_BOX"]),
            (7, ["VERY_HOT_COCOA", "BIIIG_HUG", "PUMPKIN_CANDLE"]),
            (11, ["NUTRITIOUS_SOUP", "STORYBOOK", "SEAL_OF_GOLD"])]),
        (EventId::Vakuu, "VAKUU", [
            (5, ["BLOOD_SOAKED_ROSE", "PRESERVED_FOG", "MUSIC_BOX"]),
            (7, ["WHISPERING_EARRING", "DISTINGUISHED_CAPE", "CHOICES_PARADOX"]),
            (11, ["FIDDLE", "DISTINGUISHED_CAPE", "JEWELED_MASK"])]),
    ];
    for (event, name, seeds) in witnesses {
        for (seed, relics) in seeds {
            let mut rm = run_str(&seed.to_string());
            assert_eq!(
                rm.force_enter_room(ForcedRoom::Event(event)),
                ActionResult::Advanced
            );
            let want: Vec<String> = relics
                .iter()
                .map(|r| format!("{name}.pages.INITIAL.options.{r}"))
                .collect();
            assert_eq!(keys(&rm), want, "{name} seed {seed}");
        }
    }
}

/// A pick on each of the four obtains the relic its row names.
#[test]
fn darv_tanx_tezcatara_and_vakuu_picks_obtain_the_named_relic() {
    for event in [
        EventId::Darv,
        EventId::Tanx,
        EventId::Tezcatara,
        EventId::Vakuu,
    ] {
        let mut rm = run_str("5");
        rm.run_state.direct_event_relic_grant = true;
        rm.force_enter_room(ForcedRoom::Event(event));
        let want = keys(&rm)[2].rsplit('.').next().unwrap().to_string();
        assert_ne!(
            rm.take_action(RunAction::EventChoice { idx: 2 }),
            ActionResult::InvalidForPhase
        );
        let held: Vec<&str> = rm
            .run_state
            .relics
            .iter()
            .map(|r| r.to_python_name())
            .collect();
        assert!(
            held.contains(&want.as_str()),
            "{event:?}: {want} not in {held:?}"
        );
    }
}
