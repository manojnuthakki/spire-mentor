//! The rewards a rest-site heal offers (C# `HealRestSiteOption.ExecuteRestSiteHeal`:
//! `Hook.ModifyRestSiteHealRewards`, then `RewardsCmd.OfferCustom`), sts2-agent
//! fork, `queued-producers`: Dream Catcher adds
//! `CardReward(CardCreationOptions.ForRoom(player, Monster), 3)` on every heal
//! (no limit in the game), Tiny Mailbox two `PotionReward(player)`. Under the
//! rewards screen they are one custom set, shown after the heal, the rest site
//! back once it is done. Values: the Headless scenario
//! `rest_site_dream_catcher_rewards` (Ironclad, seed "3", hp 30).

use sts2_content::run_callbacks::{
    default_relic_grab_bag, new_run_manager_for_character_from_string_seed,
};
use sts2_core::cards::CardId;
use sts2_core::potions::CharacterId;
use sts2_core::run_manager::{
    ActionResult, ForcedRoom, Phase, RunAction, RunManager, ScreenReward,
};
use sts2_core::RelicId;

fn seed_3_resting(relics: &[RelicId]) -> RunManager {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_from_string_seed("3", CharacterId::Ironclad)
        .with_neow_start();
    let (by_rarity, fallback, boss) = default_relic_grab_bag();
    rm.run_state
        .populate_relic_grab_bag_by_rarity(by_rarity, fallback, boss);
    rm.run_state.interactive_rewards = true;
    rm.run_state.interactive_potions = true;
    rm.run_state.relics.extend_from_slice(relics);
    rm.debug_set_player(Some(30), None, None);
    assert_eq!(
        rm.force_enter_room(ForcedRoom::RestSite),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::RestSite);
    // Rest (the heal is the first option).
    assert_eq!(
        rm.take_action(RunAction::RestOption { idx: 0 }),
        ActionResult::Advanced
    );
    rm
}

#[test]
fn dream_catchers_card_reward_is_offered_after_the_heal_on_the_combat_roller() {
    let mut rm = seed_3_resting(&[RelicId::DreamCatcher]);
    assert_eq!(rm.run_state.current_hp, 54);
    let set = rm.reward_set().expect("Dream Catcher's set");
    assert!(set.custom, "an OfferCustom set");
    let [ScreenReward::Card { options, .. }] = set.rewards.as_slice() else {
        panic!("one card reward: {:?}", set.rewards);
    };
    // CardCreationOptions.ForRoom(Monster): Headless's three cards.
    assert_eq!(
        options,
        &vec![CardId::Tremble, CardId::BloodWall, CardId::PommelStrike]
    );
    assert_eq!(
        rm.take_action(RunAction::ClaimReward { idx: 0 }),
        ActionResult::Advanced
    );
    assert_eq!(
        rm.take_action(RunAction::PickCardReward { idx: Some(1) }),
        ActionResult::Advanced
    );
    assert!(rm
        .run_state
        .master_deck
        .iter()
        .any(|c| c.card_id == CardId::BloodWall));
    assert!(rm.reward_set().is_none(), "the set is done");
    // Back at the rest site, which can only be left.
    assert_eq!(rm.phase(), Phase::MapChoice);
}

#[test]
fn dream_catcher_offers_a_card_reward_on_every_heal() {
    let mut rm = seed_3_resting(&[RelicId::DreamCatcher]);
    for _ in 0..4 {
        assert!(
            matches!(rm.reward_screen(), Some([ScreenReward::Card { .. }])),
            "{:?}",
            rm.reward_screen()
        );
        assert_eq!(
            rm.take_action(RunAction::ProceedRewards),
            ActionResult::Advanced
        );
        assert_eq!(
            rm.force_enter_room(ForcedRoom::RestSite),
            ActionResult::Advanced
        );
        rm.take_action(RunAction::RestOption { idx: 0 });
    }
}

#[test]
fn tiny_mailboxs_potions_are_offered_after_the_heal() {
    let rm = seed_3_resting(&[RelicId::TinyMailbox]);
    let set = rm.reward_set().expect("Tiny Mailbox's set");
    assert!(set.custom);
    assert!(
        matches!(
            set.rewards.as_slice(),
            [ScreenReward::Potion { .. }, ScreenReward::Potion { .. }]
        ),
        "{:?}",
        set.rewards
    );
    assert!(rm.run_state.potions.is_empty(), "nothing claimed yet");
}

/// The heal's reward hooks run in the order the relics are held (C#
/// `Hook.ModifyRestSiteHealRewards` over `IterateHookListeners`), and the set
/// rolls its rewards in that list order.
#[test]
fn the_heals_rewards_roll_in_the_order_the_relics_are_held() {
    fn rolled(rm: &RunManager) -> (Vec<CardId>, Vec<sts2_core::potions::PotionKind>) {
        let mut cards = Vec::new();
        let mut potions = Vec::new();
        for r in rm.reward_screen().expect("the heal's set") {
            match r {
                ScreenReward::Card { options, .. } => cards.clone_from(options),
                ScreenReward::Potion { kind } => potions.push(*kind),
                other => panic!("{other:?}"),
            }
        }
        (cards, potions)
    }
    let (catcher_cards, _) = rolled(&seed_3_resting(&[RelicId::DreamCatcher]));
    let (_, mailbox_potions) = rolled(&seed_3_resting(&[RelicId::TinyMailbox]));
    let (cards, _) = rolled(&seed_3_resting(&[
        RelicId::DreamCatcher,
        RelicId::TinyMailbox,
    ]));
    assert_eq!(cards, catcher_cards, "Dream Catcher held first rolls first");
    let (_, potions) = rolled(&seed_3_resting(&[
        RelicId::TinyMailbox,
        RelicId::DreamCatcher,
    ]));
    assert_eq!(
        potions, mailbox_potions,
        "Tiny Mailbox held first rolls first"
    );
}
