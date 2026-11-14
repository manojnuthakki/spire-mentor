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

/// Walks seeds 1..=n to the end of the minigame and returns what each run
/// offered, with `setup` applied after the room is entered.
fn walk_all(n: u32, interactive: bool, setup: impl Fn(&mut RunManager)) -> Vec<(RunManager, i32)> {
    let mut out = Vec::new();
    for k in 1..=n {
        let mut rm = run_seed(&k.to_string());
        rm.run_state.interactive_rewards = interactive;
        setup(&mut rm);
        assert_eq!(
            rm.take_action(RunAction::EventChoice { idx: 0 }),
            ActionResult::Advanced
        );
        let gold_before = rm.run_state.gold;
        let mut state = 4242u64 + u64::from(k);
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
        out.push((rm, gold_before));
    }
    out
}

fn offered_cards(rm: &RunManager) -> Vec<CardId> {
    rm.reward_screen()
        .into_iter()
        .flatten()
        .filter_map(|r| match r {
            ScreenReward::Card { options, .. } => Some(options.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn ironclad_ids() -> Vec<CardId> {
    sts2_content::cards::spec_index::specs_for_character(CharacterId::Ironclad)
        .iter()
        .map(|f| f().id)
        .collect()
}

/// F1 (review round 1): the sphere's card rewards go through the creation
/// option hooks. Prismatic Gem widens the pool to every character, Dingy Rug
/// adds colorless; without either the offers are Ironclad's alone.
#[test]
fn sphere_card_rewards_apply_the_creation_option_hooks() {
    use sts2_core::RelicId;
    let iron = ironclad_ids();
    let plain: Vec<CardId> = walk_all(80, true, |_| {})
        .iter()
        .flat_map(|(rm, _)| offered_cards(rm))
        .collect();
    assert!(!plain.is_empty());
    assert!(plain.iter().all(|c| iron.contains(c)), "control leaked");
    let gem: Vec<CardId> = walk_all(80, true, |rm| {
        assert!(rm.force_add_relic_bare(RelicId::PrismaticGem));
    })
    .iter()
    .flat_map(|(rm, _)| offered_cards(rm))
    .collect();
    assert!(
        gem.iter().any(|c| !iron.contains(c)),
        "Prismatic Gem changed nothing"
    );
    let rug: Vec<CardId> = walk_all(80, true, |rm| {
        assert!(rm.force_add_relic_bare(RelicId::DingyRug));
    })
    .iter()
    .flat_map(|(rm, _)| offered_cards(rm))
    .collect();
    assert!(
        rug.iter().any(|c| !iron.contains(c)),
        "Dingy Rug changed nothing"
    );
}

/// F2 (review rounds 1 and 2): with the default (non-interactive) rewards the
/// revealed items are still granted, through paths that are consumed: gold
/// lands in the purse, a relic is obtained, potions are claimed. Compared with
/// what the interactive path offers for the same walk.
#[test]
fn noninteractive_sphere_still_grants_its_rewards() {
    let inter = walk_all(150, true, |_| {});
    let non = walk_all(150, false, |_| {});
    let (mut gold_runs, mut potion_runs) = (0, 0);
    for ((ri, _), (rn, gold_before)) in inter.iter().zip(&non) {
        let Some(screen) = ri.reward_screen() else {
            continue;
        };
        let gold: i32 = screen
            .iter()
            .filter_map(|r| match r {
                ScreenReward::Gold { amount } => Some(*amount),
                _ => None,
            })
            .sum();
        if gold > 0 {
            gold_runs += 1;
            assert_eq!(rn.run_state.gold - gold_before, gold, "gold not granted");
        }
        for r in screen {
            if let ScreenReward::Potion { kind } = r {
                potion_runs += 1;
                assert!(
                    rn.run_state.potions.iter().any(|p| p == kind)
                        || rn
                            .run_state
                            .pending_rewards
                            .iter()
                            .any(|q| matches!(q, sts2_core::runstate::Reward::Potion(_))),
                    "potion not delivered"
                );
            }
        }
    }
    assert!(
        gold_runs >= 5 && potion_runs >= 3,
        "{gold_runs} {potion_runs}"
    );
}

/// F2 (review round 2): a revealed relic is obtained without the interactive
/// screen. The walk is searched on a cloned grid until a relic is revealed,
/// then played on a non-interactive run and an interactive one: the relic the
/// screen offers is the relic the other run holds.
#[test]
fn noninteractive_sphere_obtains_a_revealed_relic() {
    use sts2_core::crystal_sphere::{ItemKind, Tool};
    let mut found = 0;
    'seeds: for n in 1u32..=120 {
        let probe_rm = {
            let mut rm = run_seed(&n.to_string());
            assert_eq!(
                rm.take_action(RunAction::EventChoice { idx: 1 }),
                ActionResult::Advanced
            );
            rm
        };
        let start = probe_rm.run_state.crystal_sphere.clone().expect("sphere");
        let all = probe_rm.get_available_actions();
        let mut state = 99u64 + u64::from(n);
        for _ in 0..2000 {
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
            if !probe
                .revealed()
                .iter()
                .any(|&i| probe.items()[i].kind == ItemKind::Relic)
            {
                continue;
            }
            let play = |interactive: bool| {
                let mut rm = run_seed(&n.to_string());
                rm.run_state.interactive_rewards = interactive;
                assert_eq!(
                    rm.take_action(RunAction::EventChoice { idx: 1 }),
                    ActionResult::Advanced
                );
                for a in &walk {
                    assert_eq!(rm.take_action(*a), ActionResult::Advanced);
                }
                rm
            };
            let (inter, non) = (play(true), play(false));
            let relic = inter
                .reward_screen()
                .expect("rewards")
                .iter()
                .find_map(|r| match r {
                    ScreenReward::Relic { relic } => Some(relic.relic_id),
                    _ => None,
                })
                .expect("a relic offered");
            assert!(
                non.run_state.relics.contains(&relic),
                "seed {n}: {relic:?} not obtained"
            );
            found += 1;
            continue 'seeds;
        }
    }
    assert!(found >= 2, "{found} relic witnesses");
}

/// Step53 (`sphere-noninteractive-relic-card-flags`): a relic the sphere obtains
/// updates the upgrade flags of the card rewards it rolled, as in the game
/// (`CardReward.OnRelicObtained`: the eggs upgrade the cards already on offer).
/// The reproduction: Ironclad, seed "78", the Payment Plan option, Big at
/// (4,7), (0,4), (5,9), (6,6), (2,8), (0,6). The non-interactive path (cards
/// queued as the run's pending rewards) must leave the flags the interactive
/// path shows once its relic is claimed.
#[test]
fn noninteractive_sphere_relic_updates_the_card_flags() {
    use sts2_core::runstate::Reward;
    let walk = [(4u8, 7u8), (0, 4), (5, 9), (6, 6), (2, 8), (0, 6)];
    let play = |interactive: bool| {
        let mut rm = run_seed("78");
        rm.run_state.interactive_rewards = interactive;
        // Molten Egg is the relic on offer in every rarity: it upgrades the attack
        // cards the walk's card reward rolled (SwordBoomerang, BodySlam, Cinder).
        let by_rarity = sts2_core::RelicRarity::ALL
            .into_iter()
            .map(|r| {
                (
                    r,
                    vec![sts2_core::RelicId::MoltenEgg, sts2_core::RelicId::ToxicEgg],
                )
            })
            .collect();
        rm.run_state
            .populate_relic_grab_bag_by_rarity(by_rarity, Vec::new(), Vec::new());
        assert_eq!(
            rm.take_action(RunAction::EventChoice { idx: 1 }),
            ActionResult::Advanced
        );
        for (x, y) in walk {
            if rm.run_state.crystal_sphere.is_none() {
                break;
            }
            assert_eq!(divine(&mut rm, x, y, true), ActionResult::Advanced);
        }
        rm
    };
    let mut inter = play(true);
    let non = play(false);
    // Claim every relic on the interactive screen first (they sort before the cards).
    while let Some(idx) = inter.reward_screen().and_then(|s| {
        s.iter()
            .position(|r| matches!(r, ScreenReward::Relic { .. }))
    }) {
        assert_eq!(
            inter.take_action(RunAction::ClaimReward { idx }),
            ActionResult::Advanced
        );
    }
    let want: Vec<(Vec<CardId>, Vec<bool>)> = inter
        .reward_screen()
        .into_iter()
        .flatten()
        .filter_map(|r| match r {
            ScreenReward::Card {
                options, upgraded, ..
            } => Some((options.clone(), upgraded.clone())),
            _ => None,
        })
        .collect();
    // The auto-collect path surfaces the first card reward at once; the rest wait.
    let mut got: Vec<(Vec<CardId>, Vec<bool>)> = Vec::new();
    if !non.card_reward_options().is_empty() {
        got.push((
            non.card_reward_options().to_vec(),
            non.card_reward_upgraded_flags().to_vec(),
        ));
    }
    got.extend(
        non.run_state
            .pending_rewards
            .iter()
            .filter_map(|r| match r {
                Reward::Card(c) => Some((c.options.clone(), c.upgraded.clone())),
                _ => None,
            }),
    );
    assert!(!want.is_empty(), "the walk offers card rewards");
    assert!(
        want.iter().any(|(_, u)| u.iter().any(|b| *b)),
        "the walk's relic upgrades a card on offer: {want:?}"
    );
    assert_eq!(got, want);
}
