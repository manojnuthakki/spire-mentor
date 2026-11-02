//! Grave.Confront/SoulsPower and TriBoomerang/Instinct: installed v0.107.1
//! CardSelectCmd.FromDeckForEnchantment, CardSelectorPrefs and CardCmd.Enchant.
//! Existing mandatory queue, with exact instances and no RNG/add on settlement.
use sts2_content::cards::spec_index::card_pod_from_id;
use sts2_content::run_callbacks::{
    initialize_run_setup, new_ironclad_run_manager_from_string_seed,
};
use sts2_core::card_effects::card_exhausts_inst;
use sts2_core::cards::{Card, CardId, EnchantmentId, KeywordOverride};
use sts2_core::events::EventId;
use sts2_core::relics::RelicId;
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};
use sts2_core::PendingDeckEnchant;

fn card(id: CardId, up: bool) -> Card {
    card_pod_from_id(id, up).unwrap()
}

fn manager(interactive: bool) -> RunManager {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
    let mut rm = new_ironclad_run_manager_from_string_seed("3");
    initialize_run_setup(&mut rm);
    for _ in 0..50 {
        if rm.phase() == Phase::MapChoice {
            break;
        }
        rm.take_action(rm.get_available_actions()[0]);
    }
    assert_eq!(rm.phase(), Phase::MapChoice);
    rm.run_state.interactive_card_transform = interactive;
    rm.run_state.relics.clear();
    rm
}

fn confront(rm: &mut RunManager) {
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Event(EventId::GraveOfTheForgotten)),
        ActionResult::Advanced
    );
    assert_eq!(
        rm.take_action(RunAction::EventChoice { idx: 0 }),
        ActionResult::Advanced
    );
}

fn obtain_tri(rm: &mut RunManager) {
    rm.run_state.offer_relic_rewards(RelicId::TriBoomerang, 1);
    assert_eq!(
        rm.run_state.accept_relic_reward(0),
        Some(RelicId::TriBoomerang)
    );
}

fn souls_entry() -> PendingDeckEnchant {
    PendingDeckEnchant {
        enchantment: EnchantmentId::SoulsPower,
        amount: 1,
        type_restriction: None,
        secondary_type: None,
    }
}

#[test]
fn souls_queue_filters_own_exhaust_quest_occupied_and_upgraded_instances() {
    let mut rm = manager(true);
    let mut added = card(CardId::StrikeIronclad, false);
    added.keyword_override = Some(KeywordOverride::EXHAUST);
    let mut removed = card(CardId::Offering, false);
    removed.keyword_override = Some(KeywordOverride::NONE);
    let mut occupied = card(CardId::Offering, true);
    occupied.set_enchantment(EnchantmentId::Swift, 1);
    let mut quest = card(CardId::SpoilsMap, false);
    quest.cost = 0;
    quest.keyword_override = Some(KeywordOverride::EXHAUST);
    rm.run_state.master_deck = vec![
        card(CardId::StrikeIronclad, false),
        card(CardId::Offering, false),
        card(CardId::Offering, true),
        added,
        removed,
        occupied,
        quest,
        card(CardId::Decay, false),
        card(CardId::Dazed, false),
    ];
    rm.run_state.pending_deck_enchants.push(souls_entry());
    assert_eq!(rm.enchant_eligible_deck_indices(), vec![1, 2, 3]);
}

#[test]
fn grave_waits_after_decay_hooks_then_enchants_the_chosen_upgraded_copy_once() {
    let mut rm = manager(true);
    rm.run_state.master_deck = vec![
        card(CardId::Offering, false),
        card(CardId::StrikeIronclad, false),
        card(CardId::Offering, true),
    ];
    rm.run_state.relics = vec![RelicId::LuckyFysh, RelicId::DarkstonePeriapt];
    let before = rm.run_state.master_deck.clone();
    let rng = format!("{:?}", rm.run_state.rng);
    let (gold, max_hp) = (rm.run_state.gold, rm.run_state.max_hp);
    confront(&mut rm);
    assert_eq!(rm.run_state.pending_deck_enchants, vec![souls_entry()]);
    assert_eq!(rm.enchant_eligible_deck_indices(), vec![0, 2]);
    assert_eq!(rm.run_state.master_deck[..3], before);
    assert_eq!(rm.run_state.master_deck[3].card_id, CardId::Decay);
    assert_eq!(rm.run_state.gold, gold + 15);
    assert_eq!(rm.run_state.max_hp, max_hp + 6);
    let mut branch = rm.clone();
    let offered = rm.run_state.master_deck.clone();
    assert_eq!(
        rm.take_action(RunAction::EnchantDeckCard { idx: 2 }),
        ActionResult::InvalidForPhase
    );
    assert_eq!(rm.run_state.master_deck, offered);
    assert_eq!(rm.run_state.pending_deck_enchants.len(), 1);
    assert_eq!(
        rm.take_action(RunAction::EnchantDeckCard { idx: 1 }),
        ActionResult::Advanced
    );
    assert!(card_exhausts_inst(&rm.run_state.master_deck[0]));
    assert!(!card_exhausts_inst(&rm.run_state.master_deck[2]));
    let mut expected = before[2];
    expected.set_enchantment(EnchantmentId::SoulsPower, 1);
    assert_eq!(rm.run_state.master_deck[2], expected);
    assert_eq!(rm.phase(), Phase::MapChoice);
    assert!(rm.run_state.pending_deck_enchants.is_empty());
    assert_eq!(
        rm.take_action(RunAction::EnchantDeckCard { idx: 0 }),
        ActionResult::InvalidForPhase
    );
    assert_eq!(rm.run_state.gold, gold + 15);
    assert_eq!(rm.run_state.max_hp, max_hp + 6);
    assert_eq!(format!("{:?}", rm.run_state.rng), rng);
    assert_eq!(branch.run_state.master_deck, offered);
    assert_eq!(
        branch.take_action(RunAction::EnchantDeckCard { idx: 0 }),
        ActionResult::Advanced
    );
    assert!(!card_exhausts_inst(&branch.run_state.master_deck[0]));
    assert!(card_exhausts_inst(&branch.run_state.master_deck[2]));
}

#[test]
fn grave_one_candidate_auto_enchants_and_noninteractive_chooses_first() {
    for interactive in [true, false] {
        let mut rm = manager(interactive);
        rm.run_state.master_deck = vec![card(CardId::Offering, true)];
        if !interactive {
            rm.run_state.master_deck.push(card(CardId::Offering, false));
        }
        confront(&mut rm);
        assert!(rm.run_state.pending_deck_enchants.is_empty());
        assert_eq!(rm.phase(), Phase::MapChoice);
        assert_eq!(
            rm.run_state.master_deck[0].enchantment(EnchantmentId::SoulsPower),
            1
        );
        assert!(!card_exhausts_inst(&rm.run_state.master_deck[0]));
        assert_eq!(rm.run_state.master_deck.last().unwrap().card_id, CardId::Decay);
        if !interactive {
            assert!(card_exhausts_inst(&rm.run_state.master_deck[1]));
        }
    }
}

#[test]
fn grave_empty_or_only_ineligible_deck_keeps_confront_locked() {
    for deck in [vec![], vec![card(CardId::SpoilsMap, false)], vec![card(CardId::Decay, false)]] {
        let mut rm = manager(true);
        rm.run_state.master_deck = deck.clone();
        rm.force_enter_room(ForcedRoom::Event(EventId::GraveOfTheForgotten));
        assert_eq!(
            rm.take_action(RunAction::EventChoice { idx: 0 }),
            ActionResult::InvalidForPhase
        );
        assert_eq!(rm.run_state.master_deck, deck);
        assert!(rm.run_state.pending_deck_enchants.is_empty());
    }
}

#[test]
fn tri_waits_for_three_exact_attack_instances_without_rng() {
    let mut rm = manager(true);
    let mut occupied = card(CardId::StrikeIronclad, false);
    occupied.set_enchantment(EnchantmentId::Sharp, 3);
    rm.run_state.master_deck = vec![
        card(CardId::SpoilsMap, false),
        card(CardId::StrikeIronclad, false),
        card(CardId::StrikeIronclad, true),
        card(CardId::DefendIronclad, false),
        occupied,
        card(CardId::Whirlwind, true),
        card(CardId::Bash, false),
        card(CardId::Decay, false),
    ];
    let before = rm.run_state.master_deck.clone();
    let rng = format!("{:?}", rm.run_state.rng);
    obtain_tri(&mut rm);
    assert_eq!(rm.run_state.master_deck, before, "choose before mutation");
    assert_eq!(rm.run_state.pending_deck_enchants.len(), 3);
    assert_eq!(rm.enchant_eligible_deck_indices(), vec![1, 2, 5, 6]);
    assert_eq!(format!("{:?}", rm.run_state.rng), rng);
    let branch = rm.clone();
    assert_eq!(
        rm.take_action(RunAction::EnchantDeckCard { idx: 4 }),
        ActionResult::InvalidForPhase
    );
    assert_eq!(rm.run_state.master_deck, before);
    // Original candidates [3,0,2], in chosen order, map to [3,0,1].
    for idx in [3, 0, 1] {
        assert_eq!(
            rm.take_action(RunAction::EnchantDeckCard { idx }),
            ActionResult::Advanced
        );
    }
    let mut expected = before.clone();
    for d in [6, 1, 5] {
        expected[d].set_enchantment(EnchantmentId::Instinct, 1);
    }
    assert_eq!(rm.run_state.master_deck, expected, "all other values unchanged");
    assert!(rm.run_state.pending_deck_enchants.is_empty());
    assert_eq!(rm.phase(), Phase::MapChoice);
    assert_eq!(format!("{:?}", rm.run_state.rng), rng);
    assert_eq!(branch.run_state.master_deck, before);
    assert_eq!(branch.run_state.pending_deck_enchants.len(), 3);
    assert_eq!(
        rm.take_action(RunAction::EnchantDeckCard { idx: 0 }),
        ActionResult::InvalidForPhase
    );
    assert_eq!(rm.run_state.master_deck, expected);
}

#[test]
fn tri_small_empty_threshold_and_noninteractive_complete_without_queue() {
    for interactive in [true, false] {
        for count in 0..=4 {
            if interactive && count == 4 {
                continue;
            }
            let mut rm = manager(interactive);
            rm.run_state.master_deck = (0..count)
                .map(|i| card(CardId::StrikeIronclad, i % 2 == 1))
                .chain([card(CardId::DefendIronclad, false)])
                .collect();
            let mut expected = rm.run_state.master_deck.clone();
            for c in expected.iter_mut().take(count.min(3)) {
                c.set_enchantment(EnchantmentId::Instinct, 1);
            }
            let rng = format!("{:?}", rm.run_state.rng);
            obtain_tri(&mut rm);
            assert_eq!(rm.run_state.master_deck, expected);
            assert!(rm.run_state.pending_deck_enchants.is_empty());
            assert_eq!(format!("{:?}", rm.run_state.rng), rng);
        }
    }
}
