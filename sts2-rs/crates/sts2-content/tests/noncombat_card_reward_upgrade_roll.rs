//! Whether a non-combat card reward rolls its cards' upgrades is the source's
//! choice. C# `CardFactory.CreateForReward` calls `RollForUpgrade` (one
//! `NextFloat` on `PlayerRng.Rewards` per card) unless the options carry
//! `CardCreationFlags.NoUpgradeRoll`; the late hooks
//! (`Hook.TryModifyCardRewardOptions`) run either way. Orrery and Lost Coffer
//! build plain `CardCreationOptions(..., Other, RegularEncounter)`: they roll.
//! Glass Eye's `ForNonCombatWithUniformOdds` (and Trial's
//! `ForNonCombatWithDefaultOdds`) add `NoUpgradeRoll`: no upgrade, no float
//! (sts2-agent fork, noncombat-card-reward-hooks).

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_from_string_seed,
};
use sts2_core::cards::CardId;
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::{Phase, RunAction, ScreenReward};
use sts2_core::RelicId;

/// The set `obtained` offers in act `act` (1-based), every reward's cards
/// with their upgrade flags, and the rewards stream's draws to roll it.
fn offered_set(act: u8, relics: &[RelicId], obtained: RelicId) -> (Vec<Vec<(CardId, bool)>>, u32) {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_from_string_seed("shop39", CharacterId::Ironclad);
    initialize_run_setup(&mut rm);
    for _ in 0..50 {
        if rm.phase() == Phase::MapChoice {
            break;
        }
        let a = rm.get_available_actions()[0];
        rm.take_action(a);
    }
    assert_eq!(rm.phase(), Phase::MapChoice);
    rm.run_state.act_index = act;
    rm.run_state.interactive_rewards = true;
    rm.run_state.relics = relics.to_vec();
    let before = rm.run_state.rng.rewards.counter();
    rm.run_state.obtain_relic(obtained);
    // Any action opens the offered set (this one is refused on the map).
    rm.take_action(RunAction::ProceedRewards);
    let screen = rm.reward_screen().expect("the set is open");
    let rewards = screen
        .iter()
        .filter_map(|r| match r {
            ScreenReward::Card {
                options, upgraded, ..
            } => Some(
                options
                    .iter()
                    .enumerate()
                    .map(|(i, id)| (*id, upgraded.get(i).copied().unwrap_or(false)))
                    .collect(),
            ),
            _ => None,
        })
        .collect();
    (rewards, rm.run_state.rng.rewards.counter() - before)
}

fn upgraded(rewards: &[Vec<(CardId, bool)>]) -> usize {
    rewards.iter().flatten().filter(|(_, u)| *u).count()
}

/// Glass Eye in act 3: no card is upgraded and no upgrade float is drawn,
/// nor a rarity (uniform odds, a forced rarity: pool-placeholders): 15 cards,
/// two draws fewer each than Orrery's same-sized set.
#[test]
fn glass_eye_rolls_no_upgrade() {
    let (glass, glass_draws) = offered_set(3, &[RelicId::BurningBlood], RelicId::GlassEye);
    let (orrery, orrery_draws) = offered_set(3, &[RelicId::BurningBlood], RelicId::Orrery);
    assert_eq!(glass.len(), 5);
    assert_eq!(orrery.len(), 5);
    assert_eq!(upgraded(&glass), 0, "{glass:?}");
    assert_eq!(orrery_draws, glass_draws + 30);
}

/// Orrery in act 3 rolls each card's upgrade on the rewards stream (one
/// float a card, 3 draws a card with the rarity and the pick).
#[test]
fn orrery_rolls_its_upgrades() {
    let (orrery, draws) = offered_set(3, &[RelicId::BurningBlood], RelicId::Orrery);
    assert_eq!(draws, 45);
    assert!(upgraded(&orrery) > 0, "{orrery:?}");
}

/// The late hooks still run on a reward without the upgrade roll: Molten Egg
/// upgrades Glass Eye's Attacks, and only them.
#[test]
fn molten_egg_still_upgrades_glass_eyes_attacks() {
    let (glass, _) = offered_set(
        3,
        &[RelicId::BurningBlood, RelicId::MoltenEgg],
        RelicId::GlassEye,
    );
    let specs = glass.iter().flatten();
    let mut attacks = 0;
    for (id, up) in specs {
        let is_attack = sts2_content::cards::spec_index::spec_for_card_id(*id)
            .is_some_and(|s| s.card_type == sts2_core::cards::CardType::Attack);
        attacks += usize::from(is_attack);
        assert_eq!(*up, is_attack, "{id:?}");
    }
    assert!(attacks > 0, "{glass:?}");
}
