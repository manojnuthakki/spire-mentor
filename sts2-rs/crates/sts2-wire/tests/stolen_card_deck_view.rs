//! The visible deck while a Thieving Hopper holds a card that was upgraded only
//! in this fight (sts2-agent fork): the deck view drops the stolen card by its
//! deck link (C# `SwipePower.Steal` removes its `DeckVersion` at once), not by
//! its combat state, so it shows one card fewer, as Headless does.

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_with_ascension,
};
use sts2_core::cards::CardId;
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::{Phase, RunAction};

#[test]
fn a_held_card_upgraded_in_the_fight_is_out_of_the_deck_view() {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_with_ascension(11, CharacterId::Ironclad, 0);
    initialize_run_setup(&mut rm);
    rm.run_state
        .master_deck
        .push(sts2_content::cards::spec_index::card_pod_from_id(CardId::Uppercut, false).unwrap());
    for _ in 0..200 {
        if rm.phase() == Phase::Combat {
            break;
        }
        let a: RunAction = rm.get_available_actions()[0];
        rm.take_action(a);
    }
    assert_eq!(rm.phase(), Phase::Combat);
    let deck_size = rm.run_state.master_deck.len();
    let combat = rm.combat.as_mut().unwrap();
    let uid = combat
        .player
        .draw
        .iter()
        .chain(combat.player.hand.iter())
        .copied()
        .find(|u| combat.cards[u.get() as usize - 1].card_id == CardId::Uppercut)
        .expect("Uppercut in the fight");
    // Armaments+'s in-combat upgrade, then keep it stealable.
    assert!(sts2_core::card_effects::upgrade_card_uid(combat, uid));
    if let Some(pos) = combat.player.hand.iter().position(|u| *u == uid) {
        combat.player.hand.remove(pos);
        combat.player.discard.push(uid);
    }
    let thief = combat.enemy_uids[0];
    sts2_content::monsters::act2_hive::thieving_hopper_thievery_effect(combat, thief);
    assert_eq!(combat.swipe_stolen[0].card_id, CardId::Uppercut);

    let view = sts2_wire::emit::run_manager::master_deck_cards(&rm);
    let cards = view.as_array().expect("a list");
    assert_eq!(
        cards.len(),
        deck_size - 1,
        "the held card is out of the deck view"
    );
    assert!(cards.iter().all(|c| c["id"] != "UPPERCUT"));
    assert_eq!(
        sts2_wire::emit::run_manager::master_deck(&rm).len(),
        deck_size - 1
    );
}
