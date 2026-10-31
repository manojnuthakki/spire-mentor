//! Punch Off's and Dense Vegetation's fights are real fights.
//!
//! The game: `PunchOff.TakeThem` opens the I_CAN_TAKE_THEM page with one
//! option, FIGHT, which calls `EnterCombatWithoutExitingEvent<PunchOffEvent
//! Encounter>([RelicReward, PotionReward], shouldResumeAfterCombat: false)`
//! on the event's combat layout (the two Punch Constructs created when the
//! room was entered; `PunchOffEventEncounter.GenerateMonsters` rolls each a
//! `StartingHpReduction` of `NextInt(2, 10)` on the encounter's Rng, the
//! first `StartsWithFastPunch`). `DenseVegetation.Rest` heals like a rest site
//! (`MimicRestSiteHeal`) and opens the REST page with one option, FIGHT:
//! `EnterCombatWithoutExitingEvent<DenseVegetationEventEncounter>([], false)`,
//! four Wrigglers in slots wriggler1..4, not stunned. Each won fight offers
//! its room's rewards (a Monster room's) with the event's extras, and the
//! event ends. The engine granted Punch Off's relic and potion at once and
//! finished Dense Vegetation after the heal (sts2-agent fork,
//! `event-fight-unbridged`). Punch Off's values: the Headless scenario
//! `punch_off_fight_room_rewards` (Ironclad, seed "3").

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_from_string_seed,
};
use sts2_core::events::EventId;
use sts2_core::monsters::MonsterKind;
use sts2_core::potions::{CharacterId, PotionKind};
use sts2_core::run_manager::{
    ActionResult, ForcedRoom, Phase, RunAction, RunManager, ScreenReward,
};
use sts2_core::RelicId;

fn seed_3_at(event: EventId) -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_from_string_seed("3", CharacterId::Ironclad)
        .with_neow_start();
    // The run's shuffled relic bags, as the Sim's run starts.
    initialize_run_setup(&mut rm);
    rm.run_state.interactive_rewards = true;
    rm.run_state.interactive_potions = true;
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Event(event)),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::Event);
    rm
}

fn event_options(rm: &RunManager) -> usize {
    rm.get_available_actions()
        .iter()
        .filter(|a| matches!(a, RunAction::EventChoice { .. }))
        .count()
}

/// The fight's enemies: (kind, current hp, current move).
fn enemies(rm: &RunManager) -> Vec<(MonsterKind, i32, &'static str)> {
    let combat = rm.combat.as_ref().expect("a fight");
    combat
        .creatures
        .iter()
        .filter_map(|c| {
            let kind = c.species?;
            let ai = c.ai.as_ref()?;
            Some((kind, c.current_hp, ai.current_move().id))
        })
        .collect()
}

/// Play the fight out (HP pinned high), each turn's first playable card.
fn win_the_fight(rm: &mut RunManager) {
    rm.debug_set_player(Some(9999), Some(9999), None);
    for _ in 0..5000 {
        if rm.phase() != Phase::Combat {
            return;
        }
        let actions = rm.get_available_actions();
        let a = actions
            .iter()
            .copied()
            .find(|a| matches!(a, RunAction::PlayCard { .. }))
            .unwrap_or(actions[0]);
        rm.take_action(a);
    }
    panic!("the fight never ended");
}

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Gold(i32),
    Potion(PotionKind),
    Relic(RelicId),
    Card(usize),
    Other,
}

fn screen(rm: &RunManager) -> Vec<Kind> {
    rm.reward_screen()
        .expect("the rewards screen")
        .iter()
        .map(|r| match r {
            ScreenReward::Gold { amount } => Kind::Gold(*amount),
            ScreenReward::Potion { kind } => Kind::Potion(*kind),
            ScreenReward::Relic { relic } => Kind::Relic(relic.relic_id),
            ScreenReward::Card { options, .. } => Kind::Card(options.len()),
            _ => Kind::Other,
        })
        .collect()
}

#[test]
fn punch_off_fight_is_a_fight_with_its_rooms_rewards_and_extras() {
    let mut rm = seed_3_at(EventId::PunchOff);
    assert_eq!(event_options(&rm), 2);
    // I_CAN_TAKE_THEM: the one-option FIGHT page, nothing granted yet.
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 1 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::Event);
    assert_eq!(event_options(&rm), 1);
    assert!(rm.reward_screen().is_none());
    // FIGHT: the two constructs of the layout, 55 less their reductions.
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 0 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::Combat);
    let foes = enemies(&rm);
    assert_eq!(
        foes.iter().map(|e| (e.0, e.1)).collect::<Vec<_>>(),
        vec![
            (MonsterKind::PunchConstruct, 50),
            (MonsterKind::PunchConstruct, 48)
        ]
    );
    assert_eq!(foes[0].2, "FAST_PUNCH", "the first starts with Fast Punch");
    win_the_fight(&mut rm);
    assert_eq!(rm.phase(), Phase::CardReward);
    let set = rm.reward_set().expect("a rewards set");
    assert!(!set.custom, "the room's set (source room)");
    // Headless: gold 19, Energy Potion, Rainbow Ring, a card reward.
    assert_eq!(
        screen(&rm),
        vec![
            Kind::Gold(19),
            Kind::Potion(PotionKind::EnergyPotion),
            Kind::Relic(RelicId::RainbowRing),
            Kind::Card(3),
        ]
    );
    assert!(rm.run_state.current_event.is_none(), "the event is over");
    assert_eq!(
        rm.take_action(RunAction::ClaimReward { idx: 2 }),
        ActionResult::Advanced
    );
    assert!(rm.run_state.relics.contains(&RelicId::RainbowRing));
    assert_eq!(
        rm.take_action(RunAction::ProceedRewards),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::MapChoice);
}

#[test]
fn dense_vegetation_rest_heals_then_fights_four_wrigglers() {
    let mut rm = seed_3_at(EventId::DenseVegetation);
    rm.debug_set_player(Some(40), Some(80), None);
    assert_eq!(event_options(&rm), 2);
    // REST: the rest-site heal (30% of 80), then the one-option FIGHT page.
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 1 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.current_hp, 64);
    assert_eq!(rm.phase(), Phase::Event);
    assert_eq!(event_options(&rm), 1);
    // FIGHT: four Wrigglers, not stunned; wriggler1/3 bite, 2/4 wriggle.
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 0 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::Combat);
    let foes = enemies(&rm);
    assert_eq!(foes.len(), 4);
    for (kind, hp, _) in &foes {
        assert_eq!(*kind, MonsterKind::Wriggler);
        assert!((17..=21).contains(hp), "Wriggler hp {hp}");
    }
    assert_eq!(
        foes.iter().map(|e| e.2).collect::<Vec<_>>(),
        vec!["NASTY_BITE", "WRIGGLE", "NASTY_BITE", "WRIGGLE"]
    );
    win_the_fight(&mut rm);
    assert_eq!(rm.phase(), Phase::CardReward);
    let set = rm.reward_set().expect("a rewards set");
    assert!(!set.custom, "the room's set (source room)");
    // A Monster room's gold (10-20 at A0), maybe a potion, a card reward;
    // the event adds no extras.
    let listed = screen(&rm);
    let Some(Kind::Gold(gold)) = listed.first() else {
        panic!("the room's gold first: {listed:?}");
    };
    assert!((10..=20).contains(gold), "Monster gold: {gold}");
    let rest: Vec<&Kind> = listed[1..]
        .iter()
        .filter(|k| !matches!(k, Kind::Potion(_)))
        .collect();
    assert_eq!(rest, vec![&Kind::Card(3)]);
    assert!(rm.run_state.current_event.is_none(), "the event is over");
    assert_eq!(
        rm.take_action(RunAction::ProceedRewards),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::MapChoice);
}
