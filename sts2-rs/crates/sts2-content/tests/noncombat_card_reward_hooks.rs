//! A card reward a relic offers out of combat (Orrery's, Lost Coffer's) is a
//! C# `CardReward` (`IsCardReward`) rolled by `CardFactory.CreateForReward`:
//! each card's upgrade roll applies, then `Hook.TryModifyCardRewardOptions`
//! runs every relic's `TryModifyCardRewardOptionsLate` (the eggs upgrade,
//! Wing Charm enchants one card it can enchant, on `Rng.Niche`); Lava Lamp's
//! applies only in a combat room (sts2-agent fork,
//! noncombat-card-reward-hooks). Values: the Headless scenario
//! `shop_reward_offer_upgraded` (Ironclad, seed "shop39", Orrery is relic 2).

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_from_string_seed,
};
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};
use sts2_core::RelicId;

fn orrerys_first_card_reward(relics: &[RelicId]) -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_from_string_seed("shop39", CharacterId::Ironclad)
        .with_neow_start();
    initialize_run_setup(&mut rm);
    rm.run_state.interactive_rewards = true;
    rm.run_state.interactive_potions = true;
    rm.run_state.gold = 2000;
    rm.run_state.relics = relics.to_vec();
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Shop),
        ActionResult::Advanced
    );
    let orrery = rm.shop_inventory().expect("inventory").relics[2].relic_id;
    assert_eq!(orrery, Some(RelicId::Orrery));
    assert_eq!(
        rm.take_action(RunAction::ShopBuyRelic { idx: 2 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::CardReward);
    assert_eq!(
        rm.take_action(RunAction::ClaimReward { idx: 0 }),
        ActionResult::Advanced
    );
    rm
}

fn offer(rm: &RunManager) -> Vec<(String, bool)> {
    let upgraded = rm.card_reward_upgraded_flags();
    rm.card_reward_options()
        .iter()
        .enumerate()
        .map(|(i, id)| (format!("{id:?}"), upgraded.get(i).copied().unwrap_or(false)))
        .collect()
}

#[test]
fn molten_egg_upgrades_orrerys_attacks() {
    let rm = orrerys_first_card_reward(&[RelicId::BurningBlood, RelicId::MoltenEgg]);
    assert_eq!(
        offer(&rm),
        vec![
            ("Bludgeon".to_string(), true),
            ("TwinStrike".to_string(), true),
            ("AshenStrike".to_string(), true),
        ]
    );
}

#[test]
fn without_a_hook_the_offer_is_unupgraded() {
    let rm = orrerys_first_card_reward(&[RelicId::BurningBlood]);
    assert_eq!(
        offer(&rm),
        vec![
            ("Bludgeon".to_string(), false),
            ("TwinStrike".to_string(), false),
            ("AshenStrike".to_string(), false),
        ]
    );
}

/// Lava Lamp upgrades a combat room's card rewards only
/// (`CurrentRoom is CombatRoom`): not Orrery's in a shop.
#[test]
fn lava_lamp_leaves_a_shops_card_reward_alone() {
    let rm = orrerys_first_card_reward(&[RelicId::BurningBlood, RelicId::LavaLamp]);
    assert!(rm.card_reward_upgraded_flags().iter().all(|u| !u));
}

/// Wing Charm draws its card on `Rng.Niche`, not the rewards stream: the
/// rest of Orrery's rewards roll as without it, and each of its 5 card
/// rewards (all rolled when the set is offered) draws once.
#[test]
fn wing_charm_leaves_the_rewards_stream_alone() {
    let plain = orrerys_first_card_reward(&[RelicId::BurningBlood]);
    let charmed = orrerys_first_card_reward(&[RelicId::BurningBlood, RelicId::WingCharm]);
    assert_eq!(
        plain.run_state.rng.rewards.counter(),
        charmed.run_state.rng.rewards.counter()
    );
    assert_eq!(
        charmed.run_state.rng.niche.counter(),
        plain.run_state.rng.niche.counter() + 5
    );
}
