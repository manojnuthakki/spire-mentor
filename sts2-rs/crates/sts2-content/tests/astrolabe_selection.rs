//! Astrolabe's chosen instances, Niche replacements and upgrade-before-add.
//! Authority: installed v0.107.1 Astrolabe.AfterObtained,
//! CardSelectCmd.FromDeckForTransformation, CardFactory and CardCmd.Transform.
use sts2_content::cards::spec_index::{card_pod_from_id, roll_transform_replacement_basic_niche};
use sts2_content::run_callbacks::{
    initialize_run_setup, new_ironclad_run_manager_from_string_seed,
};
use sts2_core::cards::{Card, CardId, EnchantmentId};
use sts2_core::relics::RelicId;
use sts2_core::run_manager::{ActionResult, Phase, RunAction, RunManager};

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

fn obtain(rm: &mut RunManager) {
    rm.run_state.offer_relic_rewards(RelicId::Astrolabe, 1);
    assert_eq!(
        rm.run_state.accept_relic_reward(0),
        Some(RelicId::Astrolabe)
    );
}

#[test]
fn astrolabe_waits_without_rng_and_offers_exact_eligible_instances() {
    let mut rm = manager(true);
    let mut enchanted = card(CardId::StrikeIronclad, false);
    enchanted.set_enchantment(EnchantmentId::Spiral, 3);
    rm.run_state.master_deck = vec![
        card(CardId::AscendersBane, false),
        card(CardId::SpoilsMap, false),
        card(CardId::StrikeIronclad, false),
        card(CardId::StrikeIronclad, true),
        enchanted,
        card(CardId::Injury, false),
        card(CardId::Greed, false),
    ];
    let before = rm.run_state.master_deck.clone();
    let rng = format!("{:?}", rm.run_state.rng);
    obtain(&mut rm);
    assert_eq!(
        rm.run_state.master_deck, before,
        "all selection precedes mutation"
    );
    assert_eq!(
        format!("{:?}", rm.run_state.rng),
        rng,
        "no RNG before selection"
    );
    assert_eq!(rm.run_state.pending_deck_transforms.len(), 3);
    assert_eq!(rm.transform_eligible_deck_indices(), vec![2, 3, 4, 5]);
    assert_eq!(
        rm.take_action(RunAction::TransformDeckCard { idx: 4 }),
        ActionResult::InvalidForPhase
    );
    assert_eq!(rm.run_state.master_deck, before);
}

#[test]
fn astrolabe_chosen_order_rolls_niche_upgrades_values_and_runs_hooks_once() {
    let mut rm = manager(true);
    let mut marked = card(CardId::StrikeIronclad, false);
    marked.set_enchantment(EnchantmentId::Spiral, 3);
    rm.run_state.master_deck = vec![
        card(CardId::StrikeIronclad, false),
        card(CardId::StrikeIronclad, true),
        marked,
        card(CardId::Bash, false),
    ];
    rm.run_state.relics.push(RelicId::LuckyFysh);
    let kept = rm.run_state.master_deck[1];
    let mut expected = rm.run_state.clone();
    let mut replacements = Vec::new();
    for id in [CardId::StrikeIronclad, CardId::Bash, CardId::StrikeIronclad] {
        let id = roll_transform_replacement_basic_niche(
            &mut expected,
            sts2_core::CharacterId::Ironclad,
            id,
        )
        .unwrap();
        let mut replacement = card(id, false);
        if replacement.is_upgradable() {
            replacement.upgrade();
        }
        replacements.push(replacement);
    }
    let gold = rm.run_state.gold;
    obtain(&mut rm);
    let cloned = rm.clone();
    // Original indices [2,3,0], remapped by the same stable-instance rule as Sim.
    for idx in [2, 2, 0] {
        assert_eq!(
            rm.take_action(RunAction::TransformDeckCard { idx }),
            ActionResult::Advanced
        );
    }
    assert_eq!(
        rm.run_state.master_deck[0], kept,
        "unselected upgraded same-id copy survives"
    );
    assert_eq!(rm.run_state.master_deck[1..], replacements);
    assert_eq!(
        format!("{:?}", rm.run_state.rng),
        format!("{:?}", expected.rng)
    );
    assert_eq!(rm.run_state.gold, gold + 45);
    assert!(rm.run_state.pending_deck_transforms.is_empty());
    assert_eq!(
        rm.take_action(RunAction::TransformDeckCard { idx: 0 }),
        ActionResult::InvalidForPhase
    );
    assert_eq!(rm.run_state.gold, gold + 45, "completion cannot repeat");
    assert_eq!(
        cloned.run_state.pending_deck_transforms.len(),
        3,
        "clone independently owns pending picks"
    );
    assert_eq!(cloned.run_state.master_deck.len(), 4);
}

#[test]
fn astrolabe_small_empty_and_noninteractive_paths_auto_transform_in_deck_order() {
    for interactive in [false, true] {
        for n in 0..=3 {
            let mut rm = manager(interactive);
            let mut deck = vec![
                card(CardId::SpoilsMap, false),
                card(CardId::AscendersBane, false),
            ];
            deck.extend((0..n).map(|_| card(CardId::DefendIronclad, false)));
            rm.run_state.master_deck = deck;
            let mut expected = rm.run_state.clone();
            let mut replacements = Vec::new();
            for _ in 0..n {
                let id = roll_transform_replacement_basic_niche(
                    &mut expected,
                    sts2_core::CharacterId::Ironclad,
                    CardId::DefendIronclad,
                )
                .unwrap();
                replacements.push(card(id, true));
            }
            obtain(&mut rm);
            assert!(
                rm.run_state.pending_deck_transforms.is_empty(),
                "<=3 auto-select, including no eligible cards"
            );
            assert_eq!(
                rm.run_state.master_deck[..2],
                [
                    card(CardId::SpoilsMap, false),
                    card(CardId::AscendersBane, false)
                ]
            );
            assert_eq!(rm.run_state.master_deck[2..], replacements);
            assert_eq!(
                format!("{:?}", rm.run_state.rng),
                format!("{:?}", expected.rng)
            );
        }
    }
}

#[test]
fn astrolabe_curse_and_status_keep_their_pools_and_cannot_upgrade() {
    for original in [CardId::Injury, CardId::Wound] {
        let mut rm = manager(true);
        rm.run_state.master_deck = vec![card(original, false)];
        let old_type = rm.run_state.master_deck[0].card_type;
        rm.run_state
            .relics
            .extend([RelicId::LuckyFysh, RelicId::DarkstonePeriapt]);
        let (gold, hp) = (rm.run_state.gold, rm.run_state.max_hp);
        obtain(&mut rm);
        let replacement = &rm.run_state.master_deck[0];
        assert_ne!(replacement.card_id, original);
        assert_eq!(replacement.card_type, old_type);
        assert!(!replacement.upgraded);
        assert_eq!(rm.run_state.gold, gold + 15);
        assert_eq!(
            rm.run_state.max_hp,
            hp + if original == CardId::Injury { 6 } else { 0 }
        );
    }
}

#[test]
fn astrolabe_defers_followup_grants_and_cards_until_the_final_pick() {
    let mut rm = manager(true);
    rm.run_state.master_deck = vec![card(CardId::StrikeIronclad, false); 4];
    rm.run_state.relics.push(RelicId::LuckyFysh);
    let hp = rm.run_state.max_hp;
    let gold = rm.run_state.gold;
    obtain(&mut rm);
    rm.run_state
        .pending_post_pick_relic_grants
        .push(RelicId::Strawberry);
    rm.run_state
        .pending_post_pick_cards
        .push(card(CardId::Anger, false));
    for n in 0..3 {
        assert_eq!(
            rm.take_action(RunAction::TransformDeckCard { idx: 0 }),
            ActionResult::Advanced
        );
        assert_eq!(rm.run_state.max_hp, hp + if n == 2 { 7 } else { 0 });
        assert_eq!(rm.run_state.master_deck.len(), if n == 2 { 5 } else { 4 });
    }
    assert!(rm.run_state.pending_post_pick_relic_grants.is_empty());
    assert!(rm.run_state.pending_post_pick_cards.is_empty());
    assert_eq!(
        rm.run_state.gold,
        gold + 60,
        "three transforms and one deferred add"
    );
    assert_eq!(
        rm.take_action(RunAction::TransformDeckCard { idx: 0 }),
        ActionResult::InvalidForPhase
    );
    assert_eq!(rm.run_state.max_hp, hp + 7);
    assert_eq!(rm.run_state.gold, gold + 60);
}

#[test]
fn astrolabe_excludes_eternal_same_id_but_accepts_occupied_enchantments() {
    let mut rm = manager(true);
    let mut eternal = card(CardId::StrikeIronclad, false);
    sts2_content::enchantments::apply_static_enchantment(
        &mut eternal,
        EnchantmentId::TezcatarasEmber,
        1,
    );
    let mut occupied = card(CardId::StrikeIronclad, false);
    occupied.set_enchantment(EnchantmentId::Sharp, 7);
    rm.run_state.master_deck = vec![
        eternal,
        occupied,
        card(CardId::StrikeIronclad, true),
        card(CardId::DefendIronclad, false),
        card(CardId::Bash, false),
    ];
    obtain(&mut rm);
    assert_eq!(rm.transform_eligible_deck_indices(), vec![1, 2, 3, 4]);
    assert_eq!(
        rm.take_action(RunAction::TransformDeckCard { idx: 0 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.master_deck[0], eternal);
    assert!(
        rm.run_state
            .master_deck
            .last()
            .unwrap()
            .enchantments
            .is_empty(),
        "random replacement resets original enchantment"
    );
}

#[test]
fn astrolabe_empty_deck_completes_without_selection_or_rng() {
    for interactive in [false, true] {
        let mut rm = manager(interactive);
        rm.run_state.master_deck.clear();
        let rng = format!("{:?}", rm.run_state.rng);
        obtain(&mut rm);
        assert!(rm.run_state.master_deck.is_empty());
        assert!(!rm.run_state.has_pending_grant_choice());
        assert_eq!(format!("{:?}", rm.run_state.rng), rng);
    }
}
