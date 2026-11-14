//! A card reward keeps the enchantment a late reward hook put on its cards
//! (sts2-agent fork, `card-reward-enchantment-dropped`): Glitter's Glam on
//! every option, Wing Charm's on one. C# `CardReward` holds the `CardModel`s
//! those hooks mutate, so the offer, the screen and the card taken all carry
//! the enchantment. The engine's reward path held ids and upgrade flags only.

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_from_string_seed,
    new_run_manager_for_character_with_ascension,
};
use sts2_core::cards::{CardId, EnchantmentId};
use sts2_core::potions::CharacterId;
use sts2_core::relics::RelicId;
use sts2_core::run_manager::{
    ActionResult, ForcedRoom, Phase, RunAction, RunManager, ScreenReward,
};
use sts2_core::runstate::{OfferedReward, RelicReward};

fn glam_count(rm: &RunManager) -> usize {
    rm.card_reward_enchantments()
        .iter()
        .filter(|e| e.get(EnchantmentId::Glam) == 1)
        .count()
}

/// The first fight of a run holding `relics`, won, with the rewards screen
/// (`screen`) or the auto-collect phase; stops at the card offering.
fn first_combat_card_reward(relics: &[RelicId], screen: bool) -> RunManager {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_with_ascension(1, CharacterId::Ironclad, 1);
    rm.run_state.interactive_rewards = screen;
    initialize_run_setup(&mut rm);
    rm.run_state.relics.extend_from_slice(relics);
    let mut fought = false;
    for _ in 0..2000 {
        match rm.phase() {
            Phase::Combat => {
                if !fought {
                    rm.debug_set_player(Some(9999), Some(9999), None);
                    fought = true;
                }
                let actions = rm.get_available_actions();
                let a = actions
                    .iter()
                    .copied()
                    .find(|a| matches!(a, RunAction::PlayCard { .. }))
                    .unwrap_or(actions[0]);
                rm.take_action(a);
            }
            _ if fought && rm.reward_screen().is_some() => {
                let idx = rm
                    .reward_screen()
                    .expect("screen")
                    .iter()
                    .position(|r| matches!(r, ScreenReward::Card { .. }))
                    .expect("a card reward on the screen");
                rm.take_action(RunAction::ClaimReward { idx });
                return rm;
            }
            Phase::CardReward if fought => return rm,
            _ => {
                let a = rm.get_available_actions()[0];
                rm.take_action(a);
            }
        }
    }
    panic!("no card reward");
}

fn pick(rm: &mut RunManager, idx: usize) -> sts2_core::cards::Card {
    assert_eq!(
        rm.take_action(RunAction::PickCardReward { idx: Some(idx) }),
        ActionResult::Advanced
    );
    rm.run_state.master_deck.last().copied().expect("a card")
}

#[test]
fn glitter_enchants_every_combat_option_and_the_pick_keeps_it() {
    for screen in [false, true] {
        let mut rm = first_combat_card_reward(&[RelicId::Glitter], screen);
        let ids = rm.card_reward_options().to_vec();
        assert_eq!(ids.len(), 3, "screen={screen}");
        assert_eq!(glam_count(&rm), 3, "screen={screen}: Glam on every option");
        let before = rm.run_state.master_deck.len();
        let taken = ids[1];
        assert_eq!(
            rm.take_action(RunAction::PickCardReward { idx: Some(1) }),
            ActionResult::Advanced
        );
        assert_eq!(rm.run_state.master_deck.len(), before + 1);
        let card = rm.run_state.master_deck.last().expect("picked");
        assert_eq!(card.card_id, taken);
        assert_eq!(
            card.enchantment(EnchantmentId::Glam),
            1,
            "screen={screen}: the picked card keeps Glam"
        );
    }
}

#[test]
fn without_a_hook_nothing_is_enchanted() {
    let mut rm = first_combat_card_reward(&[], true);
    assert_eq!(glam_count(&rm), 0);
    assert!(rm
        .card_reward_enchantments()
        .iter()
        .all(sts2_core::inline_kv::InlineKv::is_empty));
    let _ = pick(&mut rm, 0);
    assert!(!rm
        .run_state
        .master_deck
        .last()
        .expect("card")
        .has_enchantment(EnchantmentId::Glam));
}

#[test]
fn a_skipped_screen_reward_keeps_its_enchantment_when_reopened_and_a_pick_clears_it() {
    let mut rm = first_combat_card_reward(&[RelicId::Glitter], true);
    assert_eq!(glam_count(&rm), 3);
    // Skip: back to the screen, the reward stays claimable.
    rm.take_action(RunAction::PickCardReward { idx: None });
    assert!(
        rm.card_reward_enchantments().is_empty(),
        "closed screen holds none"
    );
    let idx = rm
        .reward_screen()
        .expect("screen")
        .iter()
        .position(|r| matches!(r, ScreenReward::Card { .. }))
        .expect("the skipped reward is still there");
    rm.take_action(RunAction::ClaimReward { idx });
    assert_eq!(glam_count(&rm), 3, "reopened: Glam still on every option");
    rm.take_action(RunAction::PickCardReward { idx: Some(0) });
    assert!(rm.card_reward_enchantments().is_empty(), "taken: cleared");
}

/// Wing Charm enchants ONE card it can enchant (on Niche): the offer is mixed.
#[test]
fn wing_charm_leaves_a_mixed_offer_and_the_pick_follows_its_own_option() {
    let mut rm = first_combat_card_reward(&[RelicId::WingCharm], false);
    let enchanted: Vec<bool> = rm
        .card_reward_enchantments()
        .iter()
        .map(|e| !e.is_empty())
        .collect();
    assert_eq!(enchanted.iter().filter(|e| **e).count(), 1, "{enchanted:?}");
    let at = enchanted.iter().position(|e| *e).expect("one");
    let other = (at + 1) % 3;
    let _ = pick(&mut rm, other);
    assert!(rm
        .run_state
        .master_deck
        .last()
        .expect("c")
        .enchantments
        .is_empty());
}

/// A noncombat reward (Orrery's, rolled by the base-odds roller) carries it too.
#[test]
fn glitter_enchants_an_orrery_reward() {
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::cards::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_from_string_seed("shop39", CharacterId::Ironclad)
        .with_neow_start();
    initialize_run_setup(&mut rm);
    rm.run_state.interactive_rewards = true;
    rm.run_state.interactive_potions = true;
    rm.run_state.gold = 2000;
    rm.run_state.relics = vec![RelicId::BurningBlood, RelicId::Glitter];
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Shop),
        ActionResult::Advanced
    );
    assert_eq!(
        rm.take_action(RunAction::ShopBuyRelic { idx: 2 }),
        ActionResult::Advanced
    );
    rm.take_action(RunAction::ClaimReward { idx: 0 });
    assert_eq!(glam_count(&rm), 3);
    rm.take_action(RunAction::PickCardReward { idx: Some(2) });
    assert_eq!(
        rm.run_state
            .master_deck
            .last()
            .expect("card")
            .enchantment(EnchantmentId::Glam),
        1
    );
}

/// Glitter claimed from the same screen AFTER the card reward was rolled:
/// C# `CardReward.OnRelicObtained` runs its late hook on the rolled cards.
#[test]
fn a_glitter_obtained_after_the_reward_was_rolled_enchants_it() {
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
    rm.take_action(RunAction::ClaimReward { idx: 0 });
    assert_eq!(rm.card_reward_options(), [CardId::Inflame, CardId::Anger]);
    assert_eq!(glam_count(&rm), 2);
    rm.take_action(RunAction::PickCardReward { idx: Some(1) });
    assert_eq!(
        rm.run_state
            .master_deck
            .last()
            .expect("card")
            .enchantment(EnchantmentId::Glam),
        1
    );
}

/// `Glam.CanEnchant` refuses an enchanted card: Glitter and Wing Charm in
/// either order leave no option with two enchantments, and the card taken
/// has exactly what was shown.
#[test]
fn glitter_and_wing_charm_never_stack_two_enchantments_on_an_option() {
    for relics in [
        [RelicId::Glitter, RelicId::WingCharm],
        [RelicId::WingCharm, RelicId::Glitter],
    ] {
        for screen in [false, true] {
            let mut rm = first_combat_card_reward(&relics, screen);
            let held = rm.card_reward_enchantments().to_vec();
            assert_eq!(held.len(), 3, "{relics:?}");
            assert!(
                held.iter().all(|e| e.iter().count() <= 1),
                "{relics:?}: {held:?}"
            );
            for (idx, e) in held.iter().enumerate() {
                // Every option carries exactly one: Swift on one, Glam on the others.
                assert_eq!(e.iter().count(), 1, "{relics:?} option {idx}");
            }
            let shown = held[2];
            rm.take_action(RunAction::PickCardReward { idx: Some(2) });
            let card = rm.run_state.master_deck.last().expect("picked");
            assert_eq!(
                card.enchantments, shown,
                "{relics:?}: the card is the one shown"
            );
        }
    }
}

/// Glitter obtained from the same screen after Wing Charm enchanted the
/// rolled card: the re-run hook respects the card's existing enchantment.
#[test]
fn a_glitter_obtained_later_leaves_a_wing_charm_card_alone() {
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
    rm.run_state.relics.push(RelicId::WingCharm);
    rm.run_state.interactive_rewards = true;
    let mut swift = sts2_core::cards::CardEnchantments::new();
    swift.set(EnchantmentId::Swift, 1);
    rm.run_state.offer_custom_rewards(
        vec![
            OfferedReward::Relic(RelicReward::new(RelicId::Glitter)),
            OfferedReward::Card {
                enchantments: vec![swift, sts2_core::cards::CardEnchantments::new()],
                options: Some(vec![CardId::Anger, CardId::Inflame]),
                upgraded: vec![false, false],
                no_upgrade_roll: false,
            },
        ],
        true,
    );
    rm.take_action(RunAction::ProceedRewards);
    rm.take_action(RunAction::ClaimReward { idx: 0 });
    rm.take_action(RunAction::ClaimReward { idx: 0 });
    let held = rm.card_reward_enchantments();
    assert_eq!(held[0].get(EnchantmentId::Swift), 1);
    assert_eq!(held[0].get(EnchantmentId::Glam), 0, "no second enchantment");
    assert_eq!(held[1].get(EnchantmentId::Glam), 1);
}
