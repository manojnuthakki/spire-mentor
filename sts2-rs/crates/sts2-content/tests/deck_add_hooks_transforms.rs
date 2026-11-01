//! deck-add-hooks-transforms (sts2-agent fork): every deck add the game makes
//! with `CardPileCmd.Add(card, Deck)` or `CardCmd.Transform` runs
//! `Hook.ModifyCardBeingAddedToDeck` before the append and
//! `Hook.AfterCardChangedPiles` after it. Lucky Fysh pays 15 gold for each
//! card added, the eggs upgrade the added card of their type, Darkstone
//! Periapt gives 6 max HP for each curse. Each case below added its card raw
//! before the fix.

use sts2_content::events::dispatch::present;
use sts2_content::run_callbacks::{
    initialize_run_setup, new_ironclad_run_manager_from_string_seed,
};
use sts2_core::cards::{Card, CardId, CardType};
use sts2_core::events::EventId;
use sts2_core::relics::RelicId;
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};
use sts2_core::RunState;

const EGGS_AND_FYSH: [RelicId; 5] = [
    RelicId::LuckyFysh,
    RelicId::DarkstonePeriapt,
    RelicId::MoltenEgg,
    RelicId::ToxicEgg,
    RelicId::FrozenEgg,
];

fn install() {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
}

fn pod(id: CardId) -> Card {
    sts2_content::cards::spec_index::card_pod_from_id(id, false).expect("a card spec")
}

fn hooked_state() -> RunState {
    install();
    let mut rs = RunState::new();
    rs.relics.extend(EGGS_AND_FYSH);
    rs.gold = 100;
    rs
}

/// The deck's last card went in with its hooks: upgraded where the eggs
/// upgrade it (every card type but curses and statuses, when upgradable).
fn assert_upgraded_by_eggs(card: &Card) {
    if matches!(
        card.card_type,
        CardType::Attack | CardType::Skill | CardType::Power
    ) && sts2_content::cards::spec_index::card_pod_from_id(card.card_id, true)
        .is_some_and(|u| u.upgraded)
    {
        assert!(card.upgraded, "{:?} upgraded by its egg", card.card_id);
    }
}

fn in_event(seed: &str, event: EventId) -> RunManager {
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
    rs.relics.extend(EGGS_AND_FYSH);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Event(event)),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::Event);
    rm
}

fn choose_key(rm: &mut RunManager, key: &str) {
    let idx = present(&rm.run_state)
        .unwrap_or_default()
        .iter()
        .position(|o| o.loc_key == key)
        .unwrap_or_else(|| panic!("{key} offered"));
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx }),
        ActionResult::Advanced
    );
}

/// The run loop's transform (`do_transform_deck_card_at`): Aroma of Chaos's
/// Let Go transforms the picked card; its replacement goes in with the hooks.
#[test]
fn a_picked_transform_runs_the_deck_add_hooks() {
    let mut rm = in_event("1", EventId::AromaOfChaos);
    choose_key(&mut rm, "AROMA_OF_CHAOS.pages.INITIAL.options.LET_GO");
    let (gold, deck) = (rm.run_state.gold, rm.run_state.master_deck.len());
    assert_eq!(rm.run_state.pending_deck_transforms.len(), 1);
    assert_eq!(
        rm.take_action(RunAction::TransformDeckCard { idx: 0 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.master_deck.len(), deck);
    assert_eq!(
        rm.run_state.gold - gold,
        15,
        "Lucky Fysh paid for the replacement"
    );
    assert_upgraded_by_eggs(rm.run_state.master_deck.last().unwrap());
}

/// Room Full of Cheese's Gorge (`do_grant_deck_card_at`): each of the two
/// picked commons goes in with the hooks.
#[test]
fn room_full_of_cheese_grants_run_the_deck_add_hooks() {
    let mut rm = in_event("1", EventId::RoomFullOfCheese);
    choose_key(&mut rm, "ROOM_FULL_OF_CHEESE.pages.INITIAL.options.GORGE");
    let gold = rm.run_state.gold;
    for _ in 0..2 {
        assert_eq!(
            rm.take_action(RunAction::GrantDeckCard { idx: 0 }),
            ActionResult::Advanced
        );
        assert_upgraded_by_eggs(rm.run_state.master_deck.last().unwrap());
    }
    assert_eq!(rm.run_state.gold - gold, 30, "15 for each card");
}

/// `RunState::transform_specific_cards` (Astrolabe, Leafy Poultice): the
/// originals removed, each replacement through the hooks, in ascending
/// order of the originals' deck indices.
#[test]
fn transform_specific_cards_runs_the_deck_add_hooks() {
    let mut rs = hooked_state();
    rs.master_deck = vec![
        pod(CardId::StrikeIronclad),
        pod(CardId::DefendIronclad),
        pod(CardId::Bash),
    ];
    let n = rs.transform_specific_cards(&[2, 0], vec![pod(CardId::Inflame), pod(CardId::Anger)]);
    assert_eq!(n, 2);
    let ids: Vec<CardId> = rs.master_deck.iter().map(|c| c.card_id).collect();
    assert_eq!(
        ids,
        [CardId::DefendIronclad, CardId::Anger, CardId::Inflame]
    );
    assert_eq!(rs.gold, 130);
    assert!(rs.master_deck[1].upgraded && rs.master_deck[2].upgraded);
}

/// `RunState::transform_cards` and `duplicate_card_from_deck` (Dolly's
/// Mirror's fallback) add with the hooks too.
#[test]
fn transform_cards_and_duplicate_run_the_deck_add_hooks() {
    let mut rs = hooked_state();
    rs.master_deck = vec![pod(CardId::StrikeIronclad), pod(CardId::DefendIronclad)];
    assert_eq!(rs.transform_cards(1, vec![pod(CardId::Anger)]), 1);
    assert_eq!(rs.gold, 115);
    assert!(rs.master_deck.last().unwrap().upgraded);
    assert!(rs.duplicate_card_from_deck());
    assert_eq!(rs.gold, 130);
    assert_eq!(rs.master_deck.len(), 3);
}

/// Pael's Tooth returns a stored card after combat, upgraded, with
/// `CardPileCmd.Add(card, Deck)`.
#[test]
fn paels_tooth_return_runs_the_deck_add_hooks() {
    let mut rs = hooked_state();
    rs.relics.push(RelicId::PaelsTooth);
    rs.paels_tooth_stored.push(pod(CardId::Bash));
    let deck = rs.master_deck.len();
    let _ = sts2_core::run_effects::dispatch_after_combat_end(&mut rs);
    assert_eq!(rs.master_deck.len(), deck + 1);
    assert_eq!(rs.gold, 115);
}

/// events.rs's `EventEffect::AddCardToDeck` and a curse add: Darkstone
/// Periapt's max HP for the curse, Lucky Fysh's gold for both.
#[test]
fn event_effect_add_card_to_deck_runs_the_deck_add_hooks() {
    let mut rs = hooked_state();
    let max_hp = rs.max_hp;
    sts2_core::events::apply_event_effects(
        &mut rs,
        &[
            sts2_core::events::EventEffect::AddCardToDeck(pod(CardId::Anger)),
            sts2_core::events::EventEffect::AddCardToDeck(pod(CardId::Injury)),
        ],
    );
    assert_eq!(rs.gold, 130);
    assert_eq!(rs.max_hp - max_hp, 6);
    assert!(rs.master_deck[rs.master_deck.len() - 2].upgraded);
}

/// The events' own transforms without interactive selection (Neow's
/// transform, Aroma of Chaos, Symbiote, Whispering Hollow, Morphic Grove, the
/// Trial, Wood Carvings): each replacement through the hooks.
#[test]
fn auto_mode_event_transforms_run_the_deck_add_hooks() {
    for (event, option) in [
        (EventId::AromaOfChaos, "let_go"),
        (EventId::Symbiote, "kill_fire"),
        (EventId::WhisperingHollow, "hug"),
    ] {
        let mut rs = hooked_state();
        rs.interactive_card_transform = false;
        rs.master_deck = vec![pod(CardId::StrikeIronclad), pod(CardId::DefendIronclad)];
        rs.current_event = Some(event);
        let spec = sts2_content::events::dispatch::spec_for(event);
        let opt = spec
            .options
            .iter()
            .find(|o| o.option_id == option)
            .unwrap_or_else(|| panic!("{event:?} {option}"));
        let gold = rs.gold;
        let _ = (opt.handler)(&mut rs);
        assert_eq!(rs.gold - gold, 15, "{event:?}");
        assert_upgraded_by_eggs(rs.master_deck.last().unwrap());
    }
}

/// Kaleidoscope's two rewards keep each card's upgrade roll and run the late
/// reward hooks (the eggs upgrade every Attack, Skill and Power offered).
#[test]
fn kaleidoscope_rewards_run_the_late_reward_hooks() {
    let mut rs = hooked_state();
    rs.interactive_rewards = true;
    rs.relics.push(RelicId::Kaleidoscope);
    rs.last_obtained_relic = Some(RelicId::Kaleidoscope);
    let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
    let set = rs.offered_reward_sets.last().expect("Kaleidoscope's set");
    assert_eq!(set.rewards.len(), 2);
    for reward in &set.rewards {
        let sts2_core::runstate::OfferedReward::Card {
            options: Some(options),
            upgraded,
            ..
        } = reward
        else {
            panic!("a rolled card reward, got {reward:?}");
        };
        assert_eq!(options.len(), 3);
        assert_eq!(upgraded.len(), 3);
        for (id, up) in options.iter().zip(upgraded) {
            let card = pod(*id);
            if sts2_content::cards::spec_index::card_pod_from_id(*id, true)
                .is_some_and(|u| u.upgraded)
                && matches!(
                    card.card_type,
                    CardType::Attack | CardType::Skill | CardType::Power
                )
            {
                assert!(*up, "{id:?} upgraded by its egg");
            }
        }
    }
}

/// F5 step 6 review (F1): Kaleidoscope's per-card `CreateForReward` options
/// lack `IsCardReward`, so Silver Crucible passes there and upgrades each of
/// the two populated `CardReward`s whole: every upgradable card of both
/// rewards upgraded, two of its three uses spent.
#[test]
fn kaleidoscope_spends_silver_crucible_once_per_reward() {
    let mut rs = hooked_state();
    rs.relics.clear();
    rs.relics.push(RelicId::SilverCrucible);
    rs.relics.push(RelicId::Kaleidoscope);
    let rewards = sts2_content::cards::spec_index::roll_kaleidoscope_card_rewards(
        &mut rs,
        sts2_core::CharacterId::Ironclad,
    );
    assert_eq!(rewards.len(), 2);
    for (ids, upgraded) in &rewards {
        for (id, up) in ids.iter().zip(upgraded) {
            let upgradable = sts2_content::cards::spec_index::card_pod_from_id(*id, true)
                .is_some_and(|u| u.upgraded);
            assert_eq!(*up, upgradable, "{id:?} upgraded by Silver Crucible");
        }
    }
    let packed = rs
        .relic_counters
        .get(&RelicId::SilverCrucible)
        .copied()
        .unwrap_or(0);
    assert_eq!(packed & 0xFF, 2, "one use for each populated reward");
}

/// F5 step 6 review (F2): no enchantment is `IsStackable`, so Wing Charm's
/// whole-reward pass has no candidate once each card's own creation pass gave
/// it Swift: Niche draws are the two pool shuffles' plus one for each card
/// Swift can take, and no more.
#[test]
fn kaleidoscope_wing_charm_draws_once_per_eligible_card() {
    let shuffles = {
        let mut rs = hooked_state();
        rs.relics.clear();
        let before = rs.rng.niche.counter();
        let _ = sts2_content::cards::spec_index::roll_kaleidoscope_card_rewards(
            &mut rs,
            sts2_core::CharacterId::Ironclad,
        );
        rs.rng.niche.counter() - before
    };
    let mut rs = hooked_state();
    rs.relics.clear();
    rs.relics.push(RelicId::WingCharm);
    let before = rs.rng.niche.counter();
    let rewards = sts2_content::cards::spec_index::roll_kaleidoscope_card_rewards(
        &mut rs,
        sts2_core::CharacterId::Ironclad,
    );
    let eligible = rewards
        .iter()
        .flat_map(|(ids, _)| ids.iter())
        .filter(|id| {
            let card = pod(**id);
            sts2_content::cards::spec_index::spec_for_card_id(**id).is_some_and(|spec| {
                sts2_content::enchantments::can_enchant_card(
                    &spec,
                    &card,
                    sts2_core::cards::EnchantmentId::Swift,
                )
            })
        })
        .count() as u32;
    assert!(eligible > 0);
    assert_eq!(rs.rng.niche.counter() - before, shuffles + eligible);
}
