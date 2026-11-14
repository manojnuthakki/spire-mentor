//! The policy-facing observation of a card reward carries the enchantment the
//! late reward hooks left (sts2-agent fork, `card-reward-enchantment-dropped`):
//! Glitter's Glam on every offered card.

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_with_ascension,
};
use sts2_core::cards::CardId;
use sts2_core::potions::CharacterId;
use sts2_core::relics::RelicId;
use sts2_core::run_manager::{Phase, RunAction};
use sts2_core::runstate::{OfferedReward, RelicReward};
use sts2_wire::emit::run_manager::{card_reward_option_enchantments, card_reward_option_upgraded};

#[test]
fn an_offered_card_shows_glam_on_the_wire() {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_with_ascension(3, CharacterId::Ironclad, 0);
    initialize_run_setup(&mut rm);
    for _ in 0..50 {
        if rm.phase() == Phase::MapChoice {
            break;
        }
        let a = rm.get_available_actions()[0];
        rm.take_action(a);
    }
    rm.run_state.interactive_rewards = true;
    rm.run_state.offer_custom_rewards(
        vec![
            OfferedReward::Relic(RelicReward::new(RelicId::Glitter)),
            OfferedReward::Card {
                enchantments: Vec::new(),
                options: Some(vec![CardId::Inflame, CardId::Anger]),
                upgraded: vec![false, false],
                no_upgrade_roll: false,
            },
        ],
        true,
    );
    rm.take_action(RunAction::ProceedRewards);
    rm.take_action(RunAction::ClaimReward { idx: 0 });
    // Before the card reward is opened it shows on the rewards screen's option.
    rm.take_action(RunAction::ClaimReward { idx: 0 });
    let wire = card_reward_option_enchantments(&rm);
    assert_eq!(
        wire,
        vec![Some(("Glam".to_owned(), 1)), Some(("Glam".to_owned(), 1))]
    );
    assert_eq!(card_reward_option_upgraded(&rm), vec![false, false]);
}
