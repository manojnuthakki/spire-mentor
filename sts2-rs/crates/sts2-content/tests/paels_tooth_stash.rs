//! Pael's Tooth (`PaelsTooth.AfterObtained`): `CardSelectCmd.FromDeckForRemoval(
//! filter: IsUpgradable)` for five cards, which it keeps (stashes) rather than
//! deletes. In an interactive run the pick is a removal batch: an offer of the
//! removable, upgradable cards in deck order, `RemoveDeckCard` per pick, the
//! exact cards stashed in deck order whatever order they were picked in.
//! sts2-agent fork, step 65.
use sts2_content::cards::spec_index::card_pod_from_id;
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
    rm.run_state.interactive_card_removal = interactive;
    rm.run_state.relics.clear();
    rm
}

fn obtain_tooth(rm: &mut RunManager) {
    rm.run_state.offer_relic_rewards(RelicId::PaelsTooth, 1);
    assert_eq!(
        rm.run_state.accept_relic_reward(0),
        Some(RelicId::PaelsTooth)
    );
}

/// Deck: 0 Strike, 1 Defend, 2 Bash+ (upgraded: not offered), 3 Anger,
/// 4 Inflame, 5 Strike (enchanted: a marker for the exact instance), 6 Cinder, 7 Defend.
fn deck() -> Vec<Card> {
    let mut marked = card(CardId::StrikeIronclad, false);
    marked.set_enchantment(EnchantmentId::Swift, 1);
    vec![
        card(CardId::StrikeIronclad, false),
        card(CardId::DefendIronclad, false),
        card(CardId::Bash, true),
        card(CardId::Anger, false),
        card(CardId::Inflame, false),
        marked,
        card(CardId::Cinder, false),
        card(CardId::DefendIronclad, false),
    ]
}

#[test]
fn interactive_offer_is_the_upgradable_cards_in_deck_order_and_picks_are_stashed_exactly() {
    let mut rm = manager(true);
    rm.run_state.master_deck = deck();
    obtain_tooth(&mut rm);

    // Five of the seven upgradable cards; nothing stashed yet, the deck intact.
    assert_eq!(rm.run_state.pending_deck_removals, 5);
    assert!(rm.run_state.paels_tooth_stored.is_empty());
    assert_eq!(rm.run_state.master_deck.len(), 8);
    assert!(
        rm.run_state.pending_choice.is_none(),
        "no stash prompt left open"
    );
    let offer = rm.run_state.removal_offer_order();
    assert_eq!(offer, vec![0, 1, 3, 4, 5, 6, 7], "Bash+ is not upgradable");
    let actions = rm.get_available_actions();
    assert_eq!(actions.len(), 7);
    assert!(actions
        .iter()
        .all(|a| matches!(a, RunAction::RemoveDeckCard { .. })));

    // Pick deck cards 7, 0, 5, 3, 1, out of order. Each index is read against the
    // offer as it stands: the earlier picks have left the deck.
    let mut removed: Vec<usize> = Vec::new();
    for target in [7usize, 0, 5, 3, 1] {
        let want = target - removed.iter().filter(|&&r| r < target).count();
        let pos = rm
            .run_state
            .removal_offer_order()
            .iter()
            .position(|&d| d == want)
            .expect("still offered");
        assert_eq!(
            rm.take_action(RunAction::RemoveDeckCard { idx: pos }),
            ActionResult::Advanced
        );
        removed.push(target);
    }
    assert_eq!(rm.run_state.pending_deck_removals, 0);
    assert!(!rm.run_state.pending_paels_tooth_stash);

    // Stashed ordered by id entry, ordinally, equal ids in deck order
    // (`OrderBy(c => c.Id.Entry, StringComparer.Ordinal)`): ANGER, DEFEND
    // (deck 1, 7), STRIKE (deck 0, then the marked one, deck 5).
    let ids: Vec<CardId> = rm
        .run_state
        .paels_tooth_stored
        .iter()
        .map(|c| c.card_id)
        .collect();
    assert_eq!(
        ids,
        vec![
            CardId::Anger,
            CardId::DefendIronclad,
            CardId::DefendIronclad,
            CardId::StrikeIronclad,
            CardId::StrikeIronclad,
        ]
    );
    assert_eq!(
        rm.run_state.paels_tooth_stored[4].enchantment(EnchantmentId::Swift),
        1,
        "the marked Strike is the stashed one, not a copy by id"
    );
    assert_eq!(
        rm.run_state.paels_tooth_stored[3].enchantment(EnchantmentId::Swift),
        0
    );

    // The deck keeps the others, in order.
    let left: Vec<CardId> = rm.run_state.master_deck.iter().map(|c| c.card_id).collect();
    assert_eq!(left, vec![CardId::Bash, CardId::Inflame, CardId::Cinder]);

    // Successive returns: each combat end draws by `rng.rewards` over the stored
    // list as it stands, upgraded, the same instance (the marker survives).
    let mut probe = rm.run_state.rng.rewards.clone();
    let mut stored = rm.run_state.paels_tooth_stored.clone();
    for _ in 0..5 {
        let i = probe.next_int_exclusive(0, stored.len() as i32) as usize;
        let want = stored.remove(i);
        let before = rm.run_state.master_deck.len();
        let _ = sts2_core::run_effects::dispatch_after_combat_end(&mut rm.run_state);
        assert_eq!(rm.run_state.master_deck.len(), before + 1);
        let got = rm.run_state.master_deck.last().unwrap();
        assert_eq!(got.card_id, want.card_id);
        assert_eq!(
            got.enchantment(EnchantmentId::Swift),
            want.enchantment(EnchantmentId::Swift)
        );
        assert!(got.upgraded, "returned upgraded");
    }
    assert!(rm.run_state.paels_tooth_stored.is_empty());
}

#[test]
fn a_pool_no_larger_than_five_is_stashed_without_a_choice() {
    let mut rm = manager(true);
    rm.run_state.master_deck = vec![
        card(CardId::StrikeIronclad, false),
        card(CardId::Anger, false),
        card(CardId::Bash, true),
    ];
    obtain_tooth(&mut rm);
    assert_eq!(rm.run_state.pending_deck_removals, 0);
    assert!(!rm.run_state.pending_paels_tooth_stash);
    assert_eq!(rm.run_state.paels_tooth_stored.len(), 2);
    assert_eq!(rm.run_state.master_deck.len(), 1);
}

#[test]
fn a_later_removal_is_a_removal_again() {
    let mut rm = manager(true);
    rm.run_state.master_deck = deck();
    obtain_tooth(&mut rm);
    for _ in 0..5 {
        assert_eq!(
            rm.take_action(RunAction::RemoveDeckCard { idx: 0 }),
            ActionResult::Advanced
        );
    }
    assert_eq!(rm.run_state.paels_tooth_stored.len(), 5);
    // The flag is off: a removal now deletes and offers every removable card.
    rm.run_state.pending_deck_removals = 1;
    assert_eq!(rm.run_state.removal_offer_order().len(), 3);
    assert_eq!(
        rm.take_action(RunAction::RemoveDeckCard { idx: 0 }),
        ActionResult::Advanced
    );
    assert_eq!(rm.run_state.paels_tooth_stored.len(), 5);
    assert_eq!(rm.run_state.master_deck.len(), 2);
}
