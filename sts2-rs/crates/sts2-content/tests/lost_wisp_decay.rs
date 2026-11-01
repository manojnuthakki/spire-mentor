//! A6d2: LostWisp.cs Claim adds Decay through CardPileCmd.AddCursesToDeck
//! before obtaining the relic and finishing. No other Lost Wisp mechanic.

use std::sync::atomic::{AtomicUsize, Ordering};

use sts2_content::events::dispatch::resolve;
use sts2_content::run_callbacks::{
    initialize_run_setup, new_ironclad_run_manager_from_string_seed,
};
use sts2_core::cards::{Card, CardId, CardType};
use sts2_core::events::EventId;
use sts2_core::relic_effects::AddSource;
use sts2_core::run_effects::{self, CardAddedRunCtx};
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction};
use sts2_core::EffectResult;
use sts2_core::{RelicId, RunState};

fn state(relics: &[RelicId]) -> RunState {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut rs = RunState::new();
    rs.master_deck
        .push(sts2_content::cards::spec_index::card_pod_from_id(CardId::Decay, false).unwrap());
    rs.relics.extend_from_slice(relics);
    rs.gold = 100;
    rs.current_hp = 40;
    rs.max_hp = 80;
    rs.enter_event_room(EventId::LostWisp);
    rs
}

#[test]
fn lost_wisp_adds_one_real_decay_and_hooks_once_before_the_relic_reward() {
    for relics in [
        vec![],
        vec![RelicId::LuckyFysh],
        vec![RelicId::DarkstonePeriapt],
        vec![
            RelicId::LuckyFysh,
            RelicId::DarkstonePeriapt,
            RelicId::FresnelLens,
            RelicId::MoltenEgg,
            RelicId::ToxicEgg,
            RelicId::FrozenEgg,
        ],
    ] {
        let mut rs = state(&relics);
        let rng = format!("{:?}", rs.rng);
        let original = rs.master_deck.clone();
        assert!(resolve(&mut rs, "claim").unwrap().finished);
        assert_eq!(rs.master_deck.len(), original.len() + 1);
        assert_eq!(&rs.master_deck[..original.len()], original.as_slice());
        let decay = rs.master_deck.last().unwrap();
        let expected =
            sts2_content::cards::spec_index::card_pod_from_id(CardId::Decay, false).unwrap();
        assert_eq!(*decay, expected, "fresh base Decay, no upgrade/enchantment");
        assert_eq!(decay.card_type, CardType::Curse);
        assert_eq!(
            rs.gold,
            100 + if relics.contains(&RelicId::LuckyFysh) {
                15
            } else {
                0
            }
        );
        assert_eq!(
            rs.max_hp,
            80 + if relics.contains(&RelicId::DarkstonePeriapt) {
                6
            } else {
                0
            }
        );
        let after = (rs.gold, rs.current_hp, rs.max_hp, rs.master_deck.clone());
        assert_eq!(format!("{:?}", rs.rng), rng, "CLAIM draws no run RNG");
        assert!(rs.current_event.is_none());
        assert!(!rs.relics.contains(&RelicId::LostWisp));
        assert_eq!(rs.relic_rewards.len(), 1);
        assert_eq!(rs.accept_relic_reward(0), Some(RelicId::LostWisp));
        assert_eq!(
            (rs.gold, rs.current_hp, rs.max_hp, rs.master_deck.clone()),
            after
        );
        assert!(
            resolve(&mut rs, "claim").is_none(),
            "completed event cannot repeat"
        );
        assert_eq!(rs.accept_relic_reward(0), None);
        assert_eq!(format!("{:?}", rs.rng), rng);
    }
}

static MODIFIED: AtomicUsize = AtomicUsize::new(0);
static ADDED: AtomicUsize = AtomicUsize::new(0);

fn observe_modify(rs: &mut RunState, card: &mut Card) -> EffectResult {
    assert_eq!(card.card_id, CardId::Decay);
    assert_eq!(rs.master_deck.len(), 1, "modify before append");
    assert_eq!(rs.current_event, Some(EventId::LostWisp));
    assert!(rs.relic_rewards.is_empty(), "curse before reward staging");
    assert!(!rs.relics.contains(&RelicId::LostWisp));
    MODIFIED.fetch_add(1, Ordering::SeqCst);
    EffectResult::Resolved
}

fn observe_added(rs: &mut RunState, ctx: &CardAddedRunCtx) -> EffectResult {
    assert_eq!(MODIFIED.load(Ordering::SeqCst), 1);
    assert_eq!(ctx.card_id, CardId::Decay);
    assert_eq!(ctx.card_type, CardType::Curse);
    assert_eq!(ctx.source, AddSource::EventGain);
    assert_eq!(rs.master_deck.len(), 2, "hook after append");
    assert_eq!(rs.current_event, Some(EventId::LostWisp));
    assert!(rs.relic_rewards.is_empty());
    assert!(!rs.relics.contains(&RelicId::LostWisp));
    ADDED.fetch_add(1, Ordering::SeqCst);
    EffectResult::Resolved
}

#[test]
fn lost_wisp_add_lifecycle_precedes_reward_and_completion() {
    let mut rs = state(&[RelicId::BurningBlood]);
    run_effects::register_modify_card_being_added_to_deck(RelicId::BurningBlood, observe_modify);
    run_effects::register_on_card_added_to_deck(RelicId::BurningBlood, observe_added);
    assert!(resolve(&mut rs, "claim").unwrap().finished);
    assert_eq!(MODIFIED.load(Ordering::SeqCst), 1);
    assert_eq!(ADDED.load(Ordering::SeqCst), 1);
    assert!(rs.current_event.is_none());
    assert_eq!(rs.relic_rewards.len(), 1);
}

#[test]
fn lost_wisp_run_loop_adds_once_in_interactive_and_noninteractive_modes() {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    for interactive in [false, true] {
        for direct in [false, true] {
            let mut rm = new_ironclad_run_manager_from_string_seed("3");
            initialize_run_setup(&mut rm);
            for _ in 0..50 {
                if rm.phase() == Phase::MapChoice {
                    break;
                }
                let action = rm.get_available_actions()[0];
                rm.take_action(action);
            }
            assert_eq!(rm.phase(), Phase::MapChoice);
            rm.run_state
                .relics
                .extend([RelicId::LuckyFysh, RelicId::DarkstonePeriapt]);
            rm.run_state.interactive_rewards = interactive;
            rm.run_state.direct_event_relic_grant = direct;
            assert_eq!(
                rm.force_enter_room(ForcedRoom::Event(EventId::LostWisp)),
                ActionResult::Advanced
            );
            let (deck, gold, hp) = (
                rm.run_state.master_deck.len(),
                rm.run_state.gold,
                rm.run_state.max_hp,
            );
            let rng = format!("{:?}", rm.run_state.rng);
            assert_eq!(
                rm.take_action(RunAction::EventChoice { idx: 0 }),
                ActionResult::Advanced
            );
            assert_eq!(rm.run_state.master_deck.len(), deck + 1);
            assert_eq!(
                rm.run_state.master_deck.last().unwrap().card_id,
                CardId::Decay
            );
            assert_eq!(rm.run_state.gold, gold + 15);
            assert_eq!(rm.run_state.max_hp, hp + 6);
            if direct {
                assert_eq!(rm.phase(), Phase::MapChoice);
                assert!(rm.run_state.relics.contains(&RelicId::LostWisp));
                assert!(rm.run_state.relic_rewards.is_empty());
            } else {
                assert_eq!(rm.phase(), Phase::CardReward);
                assert_eq!(rm.run_state.accept_relic_reward(0), Some(RelicId::LostWisp));
            }
            assert_eq!(rm.run_state.master_deck.len(), deck + 1);
            assert_eq!(rm.run_state.gold, gold + 15);
            assert_eq!(rm.run_state.max_hp, hp + 6);
            assert_eq!(format!("{:?}", rm.run_state.rng), rng);
        }
    }
}

#[test]
fn lost_wisp_search_still_adds_no_card_or_relic() {
    let mut rs = state(&[RelicId::LuckyFysh, RelicId::DarkstonePeriapt]);
    let deck = rs.master_deck.clone();
    let rng = format!("{:?}", rs.rng);
    assert!(resolve(&mut rs, "search").unwrap().finished);
    assert_eq!(rs.master_deck, deck);
    assert_eq!(rs.max_hp, 80);
    assert_eq!(rs.gold, 160);
    assert!(rs.relic_rewards.is_empty());
    assert_eq!(format!("{:?}", rs.rng), rng);
}
