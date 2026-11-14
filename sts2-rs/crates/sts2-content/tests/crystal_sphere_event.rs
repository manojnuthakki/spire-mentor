//! The Crystal Sphere event end to end (lane F's F8, `crystal-sphere`), against
//! the host's recordings (seed "3": `crystal_sphere_uncover_future` and
//! `crystal_sphere_payment_plan`): cost, grid, divinations, the curse at its
//! reveal, the rewards the revealed items offer and their order.

use sts2_core::cards::CardId;
use sts2_core::events::EventId;
use sts2_core::potions::{CharacterId, PotionKind};
use sts2_core::run_manager::{
    ActionResult, ForcedRoom, Phase, RunAction, RunManager, ScreenReward,
};

fn run() -> RunManager {
    run_seed("3")
}

fn run_seed(name: &str) -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let seed = sts2_core::deterministic_hash_code(&sts2_core::canonicalize_seed(name));
    let mut rm =
        sts2_content::run_callbacks::new_run_manager_for_character(seed, CharacterId::Ironclad)
            .with_neow_start();
    rm.run_state.direct_event_relic_grant = true;
    rm.run_state.interactive_rewards = true;
    let (by_rarity, fallback, boss) = sts2_content::run_callbacks::default_relic_grab_bag();
    rm.run_state
        .populate_relic_grab_bag_by_rarity(by_rarity, fallback, boss);
    rm.debug_set_player(None, None, Some(300));
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Event(EventId::CrystalSphere)),
        ActionResult::Advanced
    );
    rm
}

fn divine(rm: &mut RunManager, x: u8, y: u8, big: bool) -> ActionResult {
    rm.take_action(RunAction::CrystalSphereDivine { x, y, big })
}

#[test]
fn uncover_future_follows_the_recording() {
    let mut rm = run();
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 0 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.gold, 247, "300 - the rolled 53");
    assert_eq!(rm.phase(), Phase::Event);
    let rewards_before = rm.run_state.rng.rewards.counter();
    assert_eq!(
        rm.run_state
            .crystal_sphere
            .as_ref()
            .unwrap()
            .cleared_cells()
            .len(),
        24
    );
    // The grid has no way out but divining.
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 0 }),
        ActionResult::InvalidForPhase
    );
    assert_eq!(divine(&mut rm, 5, 5, true), ActionResult::Advanced);
    assert_eq!(
        rm.run_state
            .crystal_sphere
            .as_ref()
            .unwrap()
            .cleared_cells()
            .len(),
        33
    );
    assert_eq!(
        divine(&mut rm, 5, 5, true),
        ActionResult::InvalidForPhase,
        "a cleared cell"
    );
    assert_eq!(divine(&mut rm, 4, 3, false), ActionResult::Advanced);
    assert_eq!(divine(&mut rm, 8, 5, true), ActionResult::Advanced);
    // The last divination ended the event: its rewards, gold before potion.
    assert!(rm.run_state.crystal_sphere.is_none());
    let screen = rm.reward_screen().expect("the revealed items' rewards");
    assert_eq!(
        rm.run_state.rng.rewards.counter(),
        rewards_before,
        "the sphere's gold is drawn from the event's stream, not the run's rewards stream"
    );
    assert!(matches!(screen[0], ScreenReward::Gold { .. }), "{screen:?}");
    assert!(
        matches!(&screen[1], ScreenReward::Potion { kind } if *kind == PotionKind::StrengthPotion),
        "{screen:?}"
    );
    assert_eq!(screen.len(), 2);
}

#[test]
fn payment_plan_adds_debt_then_the_revealed_curse_adds_doubt() {
    let mut rm = run();
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 1 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.gold, 300);
    let has = |rm: &RunManager, c| rm.run_state.master_deck.iter().any(|x| x.card_id == c);
    assert!(has(&rm, CardId::Debt));
    assert_eq!(
        rm.run_state
            .crystal_sphere
            .as_ref()
            .unwrap()
            .divinations_left(),
        6
    );
    for (x, y, big) in [(5, 5, true), (3, 3, false), (2, 5, true), (8, 5, true)] {
        assert_eq!(divine(&mut rm, x, y, big), ActionResult::Advanced);
        assert!(!has(&rm, CardId::Doubt), "not yet revealed");
    }
    assert_eq!(divine(&mut rm, 5, 2, true), ActionResult::Advanced);
    assert_eq!(divine(&mut rm, 5, 8, true), ActionResult::Advanced);
    assert!(has(&rm, CardId::Doubt), "the curse item was revealed");
    let kinds: Vec<&str> = rm
        .reward_screen()
        .expect("rewards")
        .iter()
        .map(|r| match r {
            ScreenReward::Gold { .. } => "gold",
            ScreenReward::Potion { .. } => "potion",
            ScreenReward::Card { .. } => "card",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, vec!["gold", "gold", "potion", "potion", "card"]);
}

#[test]
fn the_actions_on_the_grid_are_divinations_only() {
    let mut rm = run();
    rm.take_action(RunAction::EventChoice { idx: 0 });
    let actions = rm.get_available_actions();
    assert!(actions
        .iter()
        .all(|a| matches!(a, RunAction::CrystalSphereDivine { .. })));
    // 97 hidden cells, two tools each.
    assert_eq!(actions.len(), (121 - 24) * 2);
}

/// `RewardsSet`'s order: gold, potion, relic, card (`RewardsSetIndex`).
fn set_index(r: &ScreenReward) -> u8 {
    match r {
        ScreenReward::Gold { .. } => 1,
        ScreenReward::Potion { .. } => 2,
        ScreenReward::Relic { .. } => 3,
        _ => 5,
    }
}

/// Through the real path (the event's rolled cost, then a fixed
/// pseudo-random walk over the grid until the game ends), the offered set is in
/// `RewardsSetIndex` order for every seed. NOTE: this does not guard the
/// sphere's own sort call (the run manager sorts the offered set again,
/// stably); `two_potions_then_gold_come_out_in_the_net_networks_order` below
/// pins that call with a witness.
#[test]
fn the_offered_rewards_come_out_sorted_on_the_real_path() {
    let mut finished = 0;
    let mut mixed = 0;
    for n in 1..=60 {
        let mut rm = run_seed(&n.to_string());
        assert_eq!(
            rm.take_action(RunAction::EventChoice { idx: 0 }),
            ActionResult::Advanced
        );
        let mut state = 12345u64 + n as u64;
        for _ in 0..40 {
            if rm.run_state.crystal_sphere.is_none() {
                break;
            }
            let actions = rm.get_available_actions();
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let a = actions[(state >> 33) as usize % actions.len()];
            assert_eq!(rm.take_action(a), ActionResult::Advanced);
        }
        let Some(screen) = rm.reward_screen() else {
            continue;
        };
        finished += 1;
        let idx: Vec<u8> = screen.iter().map(set_index).collect();
        assert!(idx.windows(2).all(|w| w[0] <= w[1]), "seed {n}: {idx:?}");
        if idx.len() >= 3 {
            mixed += 1;
        }
    }
    assert!(finished >= 20, "{finished} runs offered rewards");
    assert!(mixed >= 5, "{mixed} runs offered three or more");
}

/// The sphere's own sort call, through the real path with a witness found on
/// a cloned grid: two potions revealed before a gold (keys 2, 2, 1). .NET's
/// three-item compare-swap network turns that into gold, second potion, first
/// potion (a stable sort would keep the potions in reveal order). The
/// expected identities are rolled off the grid's own RNG in reveal order.
#[test]
fn two_potions_then_gold_come_out_in_the_net_networks_order() {
    use sts2_core::crystal_sphere::{ItemKind, Tool};
    use sts2_core::potions::PotionRarity;
    let rar = |r| {
        if r {
            PotionRarity::Rare
        } else {
            PotionRarity::Common
        }
    };
    let mut witnesses = 0;
    'seeds: for n in 1..=200 {
        let mut rm = run_seed(&n.to_string());
        assert_eq!(
            rm.take_action(RunAction::EventChoice { idx: 0 }),
            ActionResult::Advanced
        );
        let start = rm.run_state.crystal_sphere.clone().expect("sphere");
        let all: Vec<RunAction> = rm.get_available_actions();
        // Search random walks on clones of the grid for the witness.
        let mut state = 777u64 + n as u64;
        for _ in 0..3000 {
            let mut probe = start.clone();
            let mut walk = Vec::new();
            while !probe.is_finished() {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let a = all[(state >> 33) as usize % all.len()];
                let RunAction::CrystalSphereDivine { x, y, big } = a else {
                    continue;
                };
                let tool = if big { Tool::Big } else { Tool::Small };
                if probe.divine(i32::from(x), i32::from(y), tool).is_ok() {
                    walk.push(a);
                }
            }
            let kinds: Vec<ItemKind> = probe
                .revealed()
                .iter()
                .map(|&i| probe.items()[i].kind)
                .filter(|k| *k != ItemKind::Curse)
                .collect();
            let [ItemKind::Potion { rare: r1 }, ItemKind::Potion { rare: r2 }, ItemKind::Gold { .. }] =
                kinds[..]
            else {
                continue;
            };
            let mut rng = probe.rng.clone();
            let roll = |rng: &mut _, r| {
                sts2_content::potions::all::roll_potion_of_rarity(
                    rng,
                    CharacterId::Ironclad,
                    rar(r),
                )
            };
            let (p1, p2) = (roll(&mut rng, r1), roll(&mut rng, r2));
            let (Some(p1), Some(p2)) = (p1, p2) else {
                continue;
            };
            if p1 == p2 {
                continue;
            }
            for a in walk {
                assert_eq!(rm.take_action(a), ActionResult::Advanced);
            }
            let screen = rm.reward_screen().expect("rewards offered");
            assert!(matches!(screen[0], ScreenReward::Gold { .. }), "seed {n}");
            match (&screen[1], &screen[2]) {
                (ScreenReward::Potion { kind: a }, ScreenReward::Potion { kind: b }) => {
                    assert_eq!((*a, *b), (p2, p1), "seed {n}: network order");
                }
                other => panic!("seed {n}: {other:?}"),
            }
            witnesses += 1;
            if witnesses >= 3 {
                break 'seeds;
            }
            continue 'seeds;
        }
    }
    assert!(witnesses >= 1, "no witness found");
}
