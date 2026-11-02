//! RelicModel.IsBeforeAct3TreasureChest and RelicGrabBag rejection semantics.
//! Authority: v0.107.1 Models/RelicModel.cs, Models/Relics/*.cs,
//! Runs/RelicGrabBag.cs and Factories/RelicFactory.cs (local DLL extraction).
use std::collections::BTreeMap;
use sts2_core::runstate::{RelicRarity, RunState};
use sts2_core::RelicId;

const LATE_FORBIDDEN: [RelicId; 17] = [
    RelicId::AmethystAubergine,
    RelicId::BookOfFiveRings,
    RelicId::BowlerHat,
    RelicId::DragonFruit,
    RelicId::FrozenEgg,
    RelicId::Girya,
    RelicId::JuzuBracelet,
    RelicId::LastingCandy,
    RelicId::LuckyFysh,
    RelicId::MealTicket,
    RelicId::MoltenEgg,
    RelicId::OldCoin,
    RelicId::Planisphere,
    RelicId::Shovel,
    RelicId::ToxicEgg,
    RelicId::WhiteBeastStatue,
    RelicId::WhiteStar,
];

fn bag(floor: i32, relics: Vec<RelicId>) -> RunState {
    let mut rs = RunState {
        act_index: 3,
        total_floor: floor - 3,
        ..RunState::default()
    };
    rs.populate_relic_grab_bag_by_rarity(
        BTreeMap::from([(RelicRarity::Common, relics.clone())]),
        vec![],
        vec![],
    );
    rs.shared_relic_grab_bag_by_rarity = BTreeMap::from([(RelicRarity::Common, relics)]);
    rs
}

#[test]
fn relic_act3_boundary_front_back_shared_and_preview() {
    for floor in [40, 41, 42] {
        for relic in LATE_FORBIDDEN {
            let front = vec![relic, RelicId::Anchor];
            let expected = if floor < 41 { relic } else { RelicId::Anchor };
            let rs = bag(floor, front);
            for _ in 0..2 {
                assert_eq!(
                    rs.peek_relic_from_grab_bag_where(RelicRarity::Common, |_| true, &[]),
                    Some(expected),
                    "preview {floor}: {relic:?}"
                );
            }
            assert_eq!(
                rs.clone()
                    .pop_relic_from_shared_grab_bag(RelicRarity::Common),
                Some(expected),
                "shared {floor}: {relic:?}"
            );
            assert_eq!(
                rs.clone().pop_relic_from_grab_bag(RelicRarity::Common),
                Some(expected),
                "front {floor}: {relic:?}"
            );
            let mut back = bag(floor, vec![RelicId::Anchor, relic]);
            assert_eq!(
                back.pop_relic_from_grab_bag_back(RelicRarity::Common, |_| true),
                Some((expected, RelicRarity::Common)),
                "back {floor}: {relic:?}"
            );
        }
    }
}

#[test]
fn relic_act3_rejection_prunes_all_player_buckets_before_caller_filter() {
    let mut rs = bag(
        41,
        vec![
            RelicId::BookOfFiveRings,
            RelicId::Anchor,
            RelicId::JuzuBracelet,
        ],
    );
    rs.relic_grab_bag_by_rarity
        .insert(RelicRarity::Rare, vec![RelicId::OldCoin, RelicId::Shovel]);
    rs.relic_grab_bag
        .extend([RelicId::OldCoin, RelicId::Shovel]);
    rs.relic_grab_bag_fallback = vec![RelicId::Girya, RelicId::Pendulum];
    let shared_before = rs.shared_relic_grab_bag_by_rarity.clone();
    for back in [false, true] {
        let mut trial = rs.clone();
        if back {
            assert_eq!(
                trial.pop_relic_from_grab_bag_back(RelicRarity::Common, |_| false),
                None
            );
        } else {
            assert_eq!(
                trial.pop_relic_from_grab_bag_where(RelicRarity::Common, |_| false),
                None
            );
        }
        assert_eq!(trial.relic_grab_bag, vec![RelicId::Anchor]);
        assert!(trial.relic_grab_bag_by_rarity[&RelicRarity::Rare].is_empty());
        assert_eq!(trial.relic_grab_bag_fallback, vec![RelicId::Pendulum]);
        // Rejections affect the bag being queried, not an unqueried shared bag.
        assert_eq!(trial.shared_relic_grab_bag_by_rarity, shared_before);
    }
}

#[test]
fn relic_act3_shared_rejection_prunes_every_bucket_without_draining_player() {
    let mut rs = bag(
        41,
        vec![
            RelicId::BookOfFiveRings,
            RelicId::Anchor,
            RelicId::JuzuBracelet,
        ],
    );
    rs.shared_relic_grab_bag_by_rarity
        .insert(RelicRarity::Rare, vec![RelicId::OldCoin]);
    let player_before = rs.relic_grab_bag.clone();
    assert_eq!(
        rs.pop_relic_from_shared_grab_bag(RelicRarity::Common),
        Some(RelicId::Anchor)
    );
    assert!(rs
        .shared_relic_grab_bag_by_rarity
        .values()
        .all(Vec::is_empty));
    assert_eq!(rs.relic_grab_bag, player_before);
    assert_eq!(rs.pop_relic_from_shared_grab_bag(RelicRarity::Common), None);
}

#[test]
fn relic_act3_owned_excluded_filtered_fallback_and_exhaustion() {
    let mut rs = bag(42, vec![RelicId::BookOfFiveRings, RelicId::Anchor]);
    rs.relics.push(RelicId::Anchor);
    rs.relic_grab_bag_by_rarity.insert(
        RelicRarity::Uncommon,
        vec![RelicId::MoltenEgg, RelicId::Lantern],
    );
    rs.relic_grab_bag_fallback = vec![RelicId::OldCoin, RelicId::Pendulum, RelicId::BloodVial];
    assert_eq!(
        rs.peek_relic_from_grab_bag_where(RelicRarity::Common, |_| true, &[RelicId::Lantern]),
        Some(RelicId::Pendulum)
    );
    assert_eq!(
        rs.pop_relic_from_grab_bag_where(RelicRarity::Common, |id| id != RelicId::Lantern),
        Some(RelicId::Pendulum)
    );
    assert_eq!(
        rs.pop_relic_from_grab_bag(RelicRarity::Common),
        Some(RelicId::Lantern)
    );
    assert_eq!(
        rs.pop_relic_from_grab_bag_back(RelicRarity::Rare, |_| true),
        Some((RelicId::BloodVial, RelicRarity::Rare))
    );
    assert!(!rs.has_available_relics());
    assert_eq!(rs.pop_relic_from_grab_bag(RelicRarity::Common), None);
    assert_eq!(
        rs.pop_relic_from_grab_bag_back(RelicRarity::Common, |_| true),
        None
    );
}

#[test]
fn relic_act3_availability_and_clones_do_not_resurrect_rejected_relics() {
    let mut rs = bag(41, LATE_FORBIDDEN.to_vec());
    assert!(!rs.has_available_relics());
    assert_eq!(rs.pop_relic_from_grab_bag(RelicRarity::Common), None);
    let mut clone = rs.clone();
    // Permanent removal, as in RemoveDisallowedRelicsFromDeques.
    clone.total_floor = 0;
    assert_eq!(clone.pop_relic_from_grab_bag(RelicRarity::Common), None);
    assert!(rs.relic_grab_bag.is_empty());
}

#[test]
fn relic_act3_rejection_consumes_no_rng_or_extra_rarity_rolls() {
    let mut rs = bag(
        41,
        vec![
            RelicId::BookOfFiveRings,
            RelicId::JuzuBracelet,
            RelicId::Anchor,
            RelicId::Pendulum,
        ],
    );
    let rewards = rs.rng.rewards.counter();
    let treasure = rs.rng.treasure_room.counter();
    let shops = rs.rng.shops.counter();
    rs.offer_random_relic_rewards(2, &[Some(RelicRarity::Common); 2]);
    assert_eq!(
        rs.relic_rewards
            .iter()
            .map(|r| r.relic_id)
            .collect::<Vec<_>>(),
        vec![RelicId::Anchor, RelicId::Pendulum]
    );
    assert_eq!(rs.rng.rewards.counter(), rewards);
    assert_eq!(rs.rng.treasure_room.counter(), treasure);
    assert_eq!(rs.rng.shops.counter(), shops);
    let mut rolled = bag(41, LATE_FORBIDDEN.to_vec());
    let mut control = bag(41, vec![]);
    rolled.offer_random_relic_rewards(3, &[]);
    control.offer_random_relic_rewards(3, &[]);
    assert_eq!(rolled.rng.rewards.counter(), control.rng.rewards.counter());
}

#[test]
fn relic_act3_negative_controls_remain_available_after_boundary() {
    for relic in [
        RelicId::Anchor,
        RelicId::BloodVial,
        RelicId::MummifiedHand,
        RelicId::Pendulum,
        RelicId::WingedBoots,
        RelicId::SilverCrucible,
    ] {
        let mut rs = bag(46, vec![relic]);
        assert_eq!(rs.pop_relic_from_grab_bag(RelicRarity::Common), Some(relic));
    }
}

#[test]
fn relic_act3_legacy_treasure_prunes_all_callback_pools() {
    use sts2_core::potions::CharacterId;
    use sts2_core::run_manager::RunManager;
    let mut rm = RunManager::new(0, CharacterId::Ironclad);
    rm.run_state.act_index = 3;
    rm.run_state.total_floor = 38;
    let mut common = vec![RelicId::BookOfFiveRings, RelicId::Anchor];
    let mut uncommon = vec![RelicId::MoltenEgg, RelicId::Lantern];
    let mut rare = vec![RelicId::OldCoin, RelicId::MummifiedHand];
    let picked = rm
        .enter_treasure(&mut common, &mut uncommon, &mut rare)
        .unwrap();
    assert!([RelicId::Anchor, RelicId::Lantern, RelicId::MummifiedHand].contains(&picked));
    assert!(common
        .iter()
        .chain(&uncommon)
        .chain(&rare)
        .all(|r| !LATE_FORBIDDEN.contains(r)));
    assert_eq!(rm.run_state.rng.treasure_room.counter(), 1);
}
