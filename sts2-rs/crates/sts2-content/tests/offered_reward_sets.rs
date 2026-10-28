//! Reward sets an event or relic offers (C# `RewardsCmd.OfferCustom`) under
//! `RunState::interactive_rewards` (sts2-agent fork): each becomes a claimable
//! set on the room's rewards screen, rolled when offered, sorted as the game
//! sorts a set, nested over a set it was offered from, with its own skipping
//! policy (Neow's Bones' `WithSkippingDisallowed()`); a won combat room's extra
//! rewards (C# `CombatRoom.AddExtraReward`) join the room's set.

use sts2_content::events::dispatch::resolve;
use sts2_content::run_callbacks::{default_relic_grab_bag, ironclad_content_callbacks};
use sts2_core::cards::{CardId, CardType};
use sts2_core::events::EventId;
use sts2_core::potions::{CharacterId, PotionKind};
use sts2_core::run_manager::{ActionResult, Phase, RunAction, RunManager, ScreenReward};
use sts2_core::{NeowRelic, OfferedReward, RelicId, RunState};

/// A Neow start with every relic listener and the grab bag in place, the
/// chosen Neow relic granted at once (`direct_event_relic_grant`, as Headless
/// does), and the rewards screen on or off.
fn neow_run(seed: i32, screen: bool) -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm = RunManager::new(seed, CharacterId::Ironclad)
        .with_content_callbacks(ironclad_content_callbacks())
        .with_neow_start();
    rm.run_state.direct_event_relic_grant = true;
    rm.run_state.interactive_rewards = screen;
    rm.run_state.interactive_card_transform = true;
    rm.run_state.interactive_card_upgrade = true;
    rm.run_state.interactive_card_removal = true;
    let (by_rarity, fallback, boss) = default_relic_grab_bag();
    rm.run_state
        .populate_relic_grab_bag_by_rarity(by_rarity, fallback, boss);
    rm
}

/// Take Neow's first offer, forced to `relic`.
fn take_neow(rm: &mut RunManager, relic: NeowRelic) {
    let mut offers = rm.run_state.neow_offers.expect("neow offers rolled");
    offers.positive_1 = relic;
    rm.run_state.neow_offers = Some(offers);
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 0 }),
        ActionResult::Advanced
    );
}

fn claims(rm: &RunManager) -> Vec<usize> {
    rm.get_available_actions()
        .iter()
        .filter_map(|a| match a {
            RunAction::ClaimReward { idx } => Some(*idx),
            _ => None,
        })
        .collect()
}

fn can_proceed(rm: &RunManager) -> bool {
    rm.get_available_actions()
        .contains(&RunAction::ProceedRewards)
}

/// Make whatever deck pick is up (the first card), until none is.
fn make_deck_picks(rm: &mut RunManager) {
    for _ in 0..10 {
        let first = rm.get_available_actions().first().copied();
        match first {
            Some(
                a @ (RunAction::RemoveDeckCard { .. }
                | RunAction::UpgradeDeckCard { .. }
                | RunAction::TransformDeckCard { .. }
                | RunAction::EnchantDeckCard { .. }),
            ) => {
                rm.take_action(a);
            }
            _ => return,
        }
    }
}

#[test]
fn small_capsule_offers_its_relic_as_a_set_to_claim() {
    let mut rm = neow_run(0, true);
    let before = rm.run_state.relics.len();
    take_neow(&mut rm, NeowRelic::SmallCapsule);
    assert!(rm.run_state.relics.contains(&RelicId::SmallCapsule));
    assert_eq!(
        rm.run_state.relics.len(),
        before + 1,
        "Neow's relic is granted at once; its follow-up waits on the screen"
    );
    assert_eq!(rm.phase(), Phase::CardReward);
    let set = rm.reward_set().expect("Small Capsule's set is shown");
    assert!(set.custom && set.can_skip);
    let [ScreenReward::Relic { relic }] = set.rewards.as_slice() else {
        panic!("one relic reward, got {:?}", set.rewards);
    };
    let rolled = relic.relic_id;
    assert!(can_proceed(&rm));
    assert_eq!(claims(&rm), vec![0]);
    rm.take_action(RunAction::ClaimReward { idx: 0 });
    assert!(rm.run_state.relics.contains(&rolled));
    assert!(
        rm.reward_screen().is_none(),
        "a completed custom set closes"
    );
    assert_eq!(rm.phase(), Phase::MapChoice);
}

#[test]
fn small_capsules_set_left_forfeits_the_relic() {
    let mut rm = neow_run(0, true);
    take_neow(&mut rm, NeowRelic::SmallCapsule);
    let Some([ScreenReward::Relic { relic }]) = rm.reward_screen() else {
        panic!("one relic reward");
    };
    let rolled = relic.relic_id;
    rm.take_action(RunAction::ProceedRewards);
    assert!(!rm.run_state.relics.contains(&rolled));
    assert!(rm.reward_screen().is_none());
    assert_eq!(rm.phase(), Phase::MapChoice);
}

#[test]
fn lost_coffers_set_rolls_its_card_then_its_potion_and_lists_the_potion_first() {
    // The engine's auto path rolls the same two rewards in the same order (the
    // card's options, then the potion), on the Rewards stream.
    let mut auto = neow_run(3, false);
    take_neow(&mut auto, NeowRelic::LostCoffer);
    let auto_cards = auto.card_reward_options().to_vec();
    let auto_potions = auto.run_state.potions.clone();
    let mut rm = neow_run(3, true);
    take_neow(&mut rm, NeowRelic::LostCoffer);
    assert!(rm.run_state.potions.is_empty(), "nothing taken yet");
    let set = rm.reward_set().expect("Lost Coffer's set");
    let [ScreenReward::Potion { kind }, ScreenReward::Card { options, .. }] =
        set.rewards.as_slice()
    else {
        panic!("a potion, then a card reward: {:?}", set.rewards);
    };
    assert_eq!(options, &auto_cards);
    assert_eq!(vec![*kind], auto_potions);
    assert_eq!(
        rm.run_state.rng.rewards.counter(),
        auto.run_state.rng.rewards.counter()
    );
    // The card reward: Skip keeps it, a pick uses it up; the set then closes
    // once the potion is claimed too.
    rm.take_action(RunAction::ClaimReward { idx: 1 });
    assert_eq!(rm.screen_card_claim(), Some(1));
    rm.take_action(RunAction::PickCardReward { idx: None });
    assert_eq!(rm.reward_screen().map(<[_]>::len), Some(2));
    rm.take_action(RunAction::ClaimReward { idx: 1 });
    rm.take_action(RunAction::PickCardReward { idx: Some(0) });
    assert_eq!(rm.reward_screen().map(<[_]>::len), Some(1));
    rm.take_action(RunAction::ClaimReward { idx: 0 });
    assert_eq!(rm.run_state.potions, auto_potions);
    assert!(rm.reward_screen().is_none());
    assert_eq!(rm.phase(), Phase::MapChoice);
}

/// The first seed whose Neow's Bones offers Small Capsule among its two.
fn bones_with_small_capsule() -> RunManager {
    for seed in 0..500 {
        let mut rm = neow_run(seed, true);
        take_neow(&mut rm, NeowRelic::NeowsBones);
        let has_capsule = rm.reward_screen().is_some_and(|s| {
            s.iter().any(|r| {
                matches!(r, ScreenReward::Relic { relic } if relic.relic_id == RelicId::SmallCapsule)
            })
        });
        if has_capsule {
            return rm;
        }
    }
    panic!("no seed below 500 offers Small Capsule from Neow's Bones");
}

fn curses(rm: &RunManager) -> usize {
    rm.run_state
        .master_deck
        .iter()
        .filter(|c| c.card_type == CardType::Curse)
        .count()
}

#[test]
fn neows_bones_set_disallows_skipping_and_its_small_capsule_set_allows_it() {
    let mut rm = bones_with_small_capsule();
    let curses_before = curses(&rm);
    let set = rm.reward_set().expect("Neow's Bones' set");
    assert!(set.custom && !set.can_skip);
    assert_eq!(set.rewards.len(), 2);
    assert!(!rm.reward_screen_can_proceed());
    assert!(!can_proceed(&rm), "no proceed while a reward is left");
    assert_eq!(
        rm.take_action(RunAction::ProceedRewards),
        ActionResult::InvalidForPhase
    );
    assert_eq!(
        curses(&rm),
        curses_before,
        "the curse waits for the set to finish"
    );

    let capsule = rm
        .reward_screen()
        .unwrap()
        .iter()
        .position(|r| matches!(r, ScreenReward::Relic { relic } if relic.relic_id == RelicId::SmallCapsule))
        .unwrap();
    rm.take_action(RunAction::ClaimReward { idx: capsule });
    assert!(rm.run_state.relics.contains(&RelicId::SmallCapsule));
    assert_eq!(
        rm.reward_set_depth(),
        2,
        "Small Capsule's set nests over it"
    );
    let inner = rm.reward_set().unwrap();
    assert!(inner.custom && inner.can_skip && inner.rewards.len() == 1);
    assert!(can_proceed(&rm));
    rm.take_action(RunAction::ProceedRewards);

    assert_eq!(rm.reward_set_depth(), 1, "back to Neow's Bones' set");
    assert_eq!(rm.reward_screen().map(<[_]>::len), Some(1));
    assert!(
        !can_proceed(&rm),
        "its policy is kept apart from the nested set's"
    );
    assert_eq!(curses(&rm), curses_before);

    rm.take_action(RunAction::ClaimReward { idx: 0 });
    make_deck_picks(&mut rm);
    // A choose screen or a card reward the second relic opened: skip it.
    for _ in 0..10 {
        if rm.reward_set_depth() > 1 {
            rm.take_action(RunAction::ProceedRewards);
        } else if !rm.card_reward_options().is_empty() {
            rm.take_action(RunAction::PickCardReward { idx: None });
        } else {
            break;
        }
        make_deck_picks(&mut rm);
    }
    assert!(rm.reward_screen().is_none());
    assert_eq!(rm.phase(), Phase::MapChoice);
    assert_eq!(curses(&rm), curses_before + 1, "then the curse is added");
}

#[test]
fn the_engines_auto_path_is_unchanged_without_the_screen() {
    let mut rm = neow_run(0, false);
    let before = rm.run_state.relics.len();
    take_neow(&mut rm, NeowRelic::SmallCapsule);
    assert_eq!(rm.run_state.relics.len(), before + 2);
    assert!(rm.reward_screen().is_none());
    assert!(rm.run_state.offered_reward_sets.is_empty());
}

fn event_state(event: EventId) -> RunState {
    sts2_content::cards::dispatch::ensure_installed();
    let mut rs = RunState::new();
    rs.interactive_rewards = true;
    rs.character_id = CharacterId::Ironclad;
    rs.max_potion_slots = 3;
    rs.gold = 500;
    rs.current_hp = 80;
    rs.max_hp = 80;
    rs.enter_event_room(event);
    rs
}

/// The one set the event option offered.
fn offered(event: EventId, option: &str) -> Vec<OfferedReward> {
    let mut rs = event_state(event);
    if event == EventId::TheFutureOfPotions {
        rs.potions = vec![PotionKind::BloodPotion];
        rs.potion_slot_indices = vec![0];
    }
    resolve(&mut rs, option).expect("option resolves");
    assert!(
        rs.pending_rewards.is_empty(),
        "{event:?}: not the auto path"
    );
    assert!(rs.pending_card_selects.is_empty(), "{event:?}");
    let [set] = rs.offered_reward_sets.as_slice() else {
        panic!(
            "{event:?} {option}: one set, got {:?}",
            rs.offered_reward_sets
        );
    };
    assert!(set.can_skip);
    set.rewards.clone()
}

#[test]
fn the_events_offer_their_rewards_as_one_set() {
    use OfferedReward as O;
    let relic = O::RandomRelic {
        rarity: None,
        is_wax: false,
    };
    assert_eq!(
        offered(EventId::DrowningBeacon, "bottle"),
        vec![O::Potion(Some(PotionKind::GlowwaterPotion))]
    );
    assert_eq!(
        offered(EventId::PotionCourier, "grab"),
        vec![O::Potion(Some(PotionKind::FoulPotion)); 3]
    );
    assert!(matches!(
        offered(EventId::PotionCourier, "ransack").as_slice(),
        [O::Potion(Some(_))]
    ));
    assert_eq!(
        offered(EventId::WhisperingHollow, "gold"),
        vec![O::Potion(None); 2]
    );
    assert!(matches!(
        offered(EventId::Wellspring, "bottle").as_slice(),
        [O::Potion(Some(_))]
    ));
    assert!(matches!(
        offered(EventId::TheLegendsWereTrue, "find_exit").as_slice(),
        [O::Potion(Some(_))]
    ));
    assert_eq!(
        offered(EventId::WarHistorianRepy, "unlock_chest"),
        vec![
            O::Potion(None),
            O::Potion(None),
            relic.clone(),
            relic.clone()
        ]
    );
    assert_eq!(offered(EventId::PunchOff, "nab"), vec![relic.clone()]);
    assert_eq!(
        offered(EventId::PunchOff, "take_them"),
        vec![relic, O::Potion(None)]
    );
    assert_eq!(
        offered(EventId::ColorfulPhilosophers, "pool_1").len(),
        3,
        "three card rewards"
    );
    assert!(matches!(
        offered(EventId::BrainLeech, "rip").as_slice(),
        [O::Card { options: Some(o), .. }] if o.len() == 3
    ));
    assert!(matches!(
        offered(EventId::TheFutureOfPotions, "trade_0").as_slice(),
        [O::Card { options: Some(o), upgraded }] if !o.is_empty() && upgraded.iter().all(|u| *u)
    ));
}

/// A run manager on a won room's screen with `extras` staged.
fn room_screen_with(extras: Vec<OfferedReward>) -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm = sts2_content::run_callbacks::new_run_manager_for_character_with_ascension(
        11,
        CharacterId::Ironclad,
        0,
    );
    rm.run_state.interactive_rewards = true;
    rm.run_state.interactive_card_removal = true;
    sts2_content::run_callbacks::initialize_run_setup(&mut rm);
    rm.run_state.room_extra_rewards = extras;
    // First fight, won.
    for _ in 0..2000 {
        if rm.reward_screen().is_some() {
            return rm;
        }
        if rm.phase() == Phase::Combat {
            rm.debug_set_player(Some(9999), Some(9999), None);
            let actions = rm.get_available_actions();
            let a = actions
                .iter()
                .copied()
                .find(|a| matches!(a, RunAction::PlayCard { .. }))
                .unwrap_or(actions[0]);
            rm.take_action(a);
        } else {
            let a = rm.get_available_actions()[0];
            rm.take_action(a);
        }
    }
    panic!("no rewards screen");
}

#[test]
fn the_rooms_set_lists_its_extras_in_the_games_order_and_claims_them() {
    let bash = sts2_content::cards::spec_index::card_pod_from_id(CardId::Bash, true).unwrap();
    let mut rm = room_screen_with(vec![
        OfferedReward::RemoveCard,
        OfferedReward::SpecialCard(bash),
        OfferedReward::Gold(30),
    ]);
    let set = rm.reward_set().unwrap();
    assert!(!set.custom && set.can_skip);
    let order: Vec<u8> = set
        .rewards
        .iter()
        .map(ScreenReward::rewards_set_index)
        .collect();
    let mut sorted = order.clone();
    sorted.sort_unstable();
    assert_eq!(
        order, sorted,
        "gold, potion, relic, special card, card, removal"
    );
    assert_eq!(set.rewards.last(), Some(&ScreenReward::RemoveCard));
    let special = set
        .rewards
        .iter()
        .position(|r| matches!(r, ScreenReward::SpecialCard { .. }))
        .unwrap();
    let deck = rm.run_state.master_deck.len();
    rm.take_action(RunAction::ClaimReward { idx: special });
    assert_eq!(rm.run_state.master_deck.len(), deck + 1);
    assert_eq!(
        rm.run_state
            .master_deck
            .last()
            .map(|c| (c.card_id, c.upgraded)),
        Some((CardId::Bash, true))
    );
    let removal = rm.reward_screen().unwrap().len() - 1;
    rm.take_action(RunAction::ClaimReward { idx: removal });
    assert!(matches!(
        rm.get_available_actions().first(),
        Some(RunAction::RemoveDeckCard { .. })
    ));
    rm.take_action(RunAction::RemoveDeckCard { idx: 0 });
    assert_eq!(rm.run_state.master_deck.len(), deck);
    assert_eq!(rm.phase(), Phase::CardReward, "back on the room's screen");
    assert!(can_proceed(&rm));
}

#[test]
fn a_removal_with_no_removable_card_leaves_the_reward_and_the_screen_usable() {
    let mut rm = room_screen_with(vec![OfferedReward::RemoveCard]);
    let bane =
        sts2_content::cards::spec_index::card_pod_from_id(CardId::AscendersBane, false).unwrap();
    rm.run_state.master_deck = vec![bane];
    assert!(
        rm.run_state.removal_offer_order().is_empty(),
        "the Bane is Eternal"
    );
    let removal = rm.reward_screen().unwrap().len() - 1;
    assert_eq!(
        rm.reward_screen().unwrap()[removal],
        ScreenReward::RemoveCard
    );
    assert_eq!(
        rm.take_action(RunAction::ClaimReward { idx: removal }),
        ActionResult::Advanced
    );
    assert_eq!(rm.screen_removal_claim(), None, "no selection opens");
    assert_eq!(
        rm.reward_screen().unwrap()[removal],
        ScreenReward::RemoveCard
    );
    assert!(can_proceed(&rm), "the run isn't stranded");
    rm.take_action(RunAction::ProceedRewards);
    assert_eq!(rm.run_state.master_deck, vec![bane]);
}

#[test]
fn the_rooms_gold_lists_its_own_then_the_extras_then_the_hooks() {
    // Amethyst Aubergine's gold is a reward hook's (C# `Hook.ModifyRewards`),
    // added after the room's extras.
    sts2_content::relics::dispatch::ensure_installed();
    let mut rm = room_screen_with_relic(RelicId::AmethystAubergine, vec![OfferedReward::Gold(20)]);
    let gold: Vec<i32> = rm
        .reward_screen()
        .unwrap()
        .iter()
        .filter_map(|r| match r {
            ScreenReward::Gold { amount } => Some(*amount),
            _ => None,
        })
        .collect();
    assert_eq!(gold.len(), 3, "{gold:?}");
    assert_eq!(&gold[1..], &[20, 15], "the extra, then the hook's");
    rm.take_action(RunAction::ProceedRewards);
}

fn room_screen_with_relic(relic: RelicId, extras: Vec<OfferedReward>) -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm = sts2_content::run_callbacks::new_run_manager_for_character_with_ascension(
        11,
        CharacterId::Ironclad,
        0,
    );
    rm.run_state.interactive_rewards = true;
    sts2_content::run_callbacks::initialize_run_setup(&mut rm);
    rm.run_state.relics.push(relic);
    rm.run_state.room_extra_rewards = extras;
    for _ in 0..2000 {
        if rm.reward_screen().is_some() {
            return rm;
        }
        if rm.phase() == Phase::Combat {
            rm.debug_set_player(Some(9999), Some(9999), None);
            let actions = rm.get_available_actions();
            let a = actions
                .iter()
                .copied()
                .find(|a| matches!(a, RunAction::PlayCard { .. }))
                .unwrap_or(actions[0]);
            rm.take_action(a);
        } else {
            let a = rm.get_available_actions()[0];
            rm.take_action(a);
        }
    }
    panic!("no rewards screen");
}
