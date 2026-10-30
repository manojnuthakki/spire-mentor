//! Orobas's third option is drawn from the relics the player can set up.
//!
//! The game's Orobas adds Touch of Orobas to OptionPool3 only if the player has
//! a Starter relic (TouchOfOrobas.SetupForPlayer) and Archaic Tooth only if the
//! deck holds a card it transcends (Bash, Neutralize, Unleash, Falling Star,
//! Dualcast: ArchaicTooth.SetupForPlayer). The engine always offered both, so a
//! deck without Bash could be offered Archaic Tooth (MJW8C0NGM3; the scenario
//! prismatic_gem_card_rewards).

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_with_ascension,
};
use sts2_core::cards::CardId;
use sts2_core::potions::CharacterId;
use sts2_core::relics::RelicId;
use sts2_core::run_room_gen::AncientId;

fn third_offer(seed: i32, keep_bash: bool) -> RelicId {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_with_ascension(seed, CharacterId::Ironclad, 0);
    initialize_run_setup(&mut rm);
    rm.run_state.act_index = 1;
    rm.run_state.generated_rooms[1].ancient = Some(AncientId::Orobas);
    if !keep_bash {
        rm.run_state
            .master_deck
            .retain(|c| c.card_id != CardId::Bash);
    }
    let offers = rm.run_state.roll_boss_relic_offers();
    assert_eq!(offers.len(), 3, "{offers:?}");
    offers[2]
}

#[test]
fn without_bash_orobas_never_offers_archaic_tooth() {
    for seed in 0..40 {
        assert_eq!(
            third_offer(seed, false),
            RelicId::TouchOfOrobas,
            "seed {seed}"
        );
    }
}

#[test]
fn with_bash_and_burning_blood_orobas_offers_either() {
    let offers: Vec<RelicId> = (0..40).map(|seed| third_offer(seed, true)).collect();
    assert!(offers.contains(&RelicId::TouchOfOrobas));
    assert!(offers.contains(&RelicId::ArchaicTooth));
}
