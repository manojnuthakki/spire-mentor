//! This-turn cost riders hold only in hand and in play.
//!
//! The game's `FreeAttackPower` (and `FreeSkillPower`, `FreePowerPower`,
//! `VeilpiercerPower`, `VoidFormPower`) set a card's cost to 0 only while the
//! card's pile is Hand or Play: a Strike in the discard pile keeps its cost
//! while one in hand costs 0, and the Strike played from hand still gets the
//! discount when its cost is paid.

use sts2_content::cards::dispatch as content_dispatch;
use sts2_core::cards::CardId;
use sts2_core::hooks::effective_card_cost;
use sts2_core::{CardUid, CombatState, MonsterKind, Power, PowerKind, PowerUid};

fn add_card(state: &mut CombatState, card_id: CardId) -> CardUid {
    let mut card = sts2_core::lookup_card_factory()
        .and_then(|f| f(card_id, false))
        .expect("card factory has the card");
    let uid = CardUid::new(u32::try_from(state.cards.len() + 1).expect("fits")).expect("nz");
    card.uid = uid;
    state.cards.push(card);
    uid
}

fn state_with_power(kind: PowerKind, amount: i32) -> CombatState {
    content_dispatch::install_default_registry();
    sts2_core::register_card_factory(sts2_content::cards::spec_index::card_pod_from_id);
    let mut state = CombatState {
        run_rng: sts2_core::run_rng_set::RunRngSet::new(4242_i32),
        ..Default::default()
    };
    state.player.character = Some(sts2_core::potions::CharacterId::Ironclad);
    state.player.current_hp = 80;
    state.player.max_hp = 80;
    state.phase = sts2_core::CombatPhase::PlayerPlayPhase;
    state.init_player_creature();
    let enemy = state.spawn_enemy(MonsterKind::Nibbit);
    if let Some(c) = state.creatures.iter_mut().find(|c| c.id == enemy.0) {
        c.max_hp = 200;
        c.current_hp = 200;
    }
    let pid = PowerUid::new(1).expect("nz");
    state.powers.push(Power {
        id: pid.0,
        kind,
        amount,
        counter: 0,
        applier: None,
        skip_next_tick: false,
    });
    state.player.powers.push(pid);
    state
}

#[test]
fn free_attack_discounts_a_strike_in_hand_not_one_in_the_discard_pile() {
    let mut state = state_with_power(PowerKind::FreeAttack, 1);
    let in_hand = add_card(&mut state, CardId::StrikeIronclad);
    let in_discard = add_card(&mut state, CardId::StrikeIronclad);
    state.player.hand.push(in_hand);
    state.player.discard.push(in_discard);

    assert_eq!(effective_card_cost(&state, in_hand), 0, "in hand: free");
    assert_eq!(
        effective_card_cost(&state, in_discard),
        1,
        "in the discard pile: its own cost"
    );

    // The Strike played from hand is paid at 0.
    state.player.energy = 0;
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, Some(0));
    assert!(
        !state.player.hand.contains(&in_hand),
        "the free Strike was played with 0 energy"
    );
}

#[test]
fn void_form_discounts_only_cards_in_hand() {
    let mut state = state_with_power(PowerKind::VoidForm, 2);
    let in_hand = add_card(&mut state, CardId::DefendIronclad);
    let in_draw = add_card(&mut state, CardId::DefendIronclad);
    state.player.hand.push(in_hand);
    state.player.draw.push(in_draw);

    assert_eq!(effective_card_cost(&state, in_hand), 0, "in hand: free");
    assert_eq!(
        effective_card_cost(&state, in_draw),
        1,
        "in the draw pile: its own cost"
    );
}
