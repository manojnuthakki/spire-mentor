//! F6 step36 (fresh DLL authority, sha 08faee0c...), card gates and per-card
//! hooks the engine ran at another point than the game:
//!
//! * `Normality.ShouldPlay` refuses autoplay once three plays STARTED while it
//!   sits in the Hand (`normality-autoplay-gate`).
//! * `BansheesCry.AfterCardPlayed` lowers its cost by 2 for every Ethereal card
//!   the owner plays (`banshees-cry-ethereal-discount`).
//! * `DarkEmbrace.AfterSideTurnEnd` draws through `ShouldDraw`: NoDraw, listed
//!   after it, is still on (`dark-embrace-nodraw`).
//! * `CombatManager.DoTurnEnd` runs a turn-end-in-hand card's effect and then
//!   exhausts it if Ethereal (`ethereal-turn-end-effect-order`).
use sts2_content::cards::spec_index::card_pod_from_id;
use sts2_core::cards::{CardId, VarKey};
use sts2_core::combat_loop::{end_turn_phase, play_card, player_should_play_blocked};
use sts2_core::{CardUid, CombatPhase, CombatState, MonsterKind, PowerKind};

fn add(s: &mut CombatState, id: CardId) -> CardUid {
    let mut c = card_pod_from_id(id, false).unwrap();
    c.uid = CardUid::new(s.cards.len() as u32 + 1).unwrap();
    s.cards.push(c);
    c.uid
}

fn board(energy: i8) -> CombatState {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut s = CombatState::default();
    s.player.current_hp = 80;
    s.player.max_hp = 80;
    s.player.energy = energy;
    s.init_player_creature();
    let e = s.spawn_enemy(MonsterKind::Nibbit);
    let enemy = s.creatures.iter_mut().find(|c| c.id == e.0).unwrap();
    enemy.current_hp = 500;
    enemy.max_hp = 500;
    s.phase = CombatPhase::PlayerPlayPhase;
    s
}

fn power(s: &mut CombatState, kind: PowerKind, amount: i32) {
    let me = s.player.creature_uid.unwrap();
    sts2_core::card_effects::apply_power_to_creature(s, me, kind, amount);
}

fn hand_index(s: &CombatState, uid: CardUid) -> u8 {
    s.player.hand.iter().position(|&u| u == uid).unwrap() as u8
}

fn end_turn(s: &mut CombatState) {
    for phase in 0..2 {
        let _ = end_turn_phase(s, phase);
    }
}

// ----------------------------------------------------------------- normality --

#[test]
fn normality_in_hand_blocks_autoplay_after_three_started_plays() {
    let mut s = board(3);
    let strike = add(&mut s, CardId::StrikeIronclad);
    s.player.hand.push(strike);
    let card = s.cards[strike.get() as usize - 1];
    s.player.card_plays_started_this_turn = 2;
    assert!(!player_should_play_blocked(&s, &card), "no Normality: free");
    let normality = add(&mut s, CardId::Normality);
    s.player.hand.push(normality);
    assert!(
        !player_should_play_blocked(&s, &card),
        "two started plays: under the limit"
    );
    s.player.card_plays_started_this_turn = 3;
    assert!(
        player_should_play_blocked(&s, &card),
        "three started plays with Normality in hand: ShouldPlay is false"
    );
    s.player.hand.retain(|u| *u != normality);
    s.player.discard.push(normality);
    assert!(
        !player_should_play_blocked(&s, &card),
        "Normality outside the Hand pile does not gate"
    );
}

// ------------------------------------------------------------ banshee's cry --

#[test]
fn banshees_cry_costs_two_less_per_ethereal_card_played() {
    let mut s = board(3);
    let cry_hand = add(&mut s, CardId::BansheesCry);
    let cry_discard = add(&mut s, CardId::BansheesCry);
    s.player.hand.push(cry_hand);
    s.player.discard.push(cry_discard);
    let defy = add(&mut s, CardId::Defy);
    s.player.hand.push(defy);
    let strike = add(&mut s, CardId::StrikeIronclad);
    s.player.hand.push(strike);
    let cost = |s: &CombatState, u: CardUid| s.cards[u.get() as usize - 1].cost;
    assert_eq!(cost(&s, cry_hand), 9);
    let i = hand_index(&s, strike);
    let _ = play_card(&mut s, i, Some(0));
    assert_eq!(cost(&s, cry_hand), 9, "a non-Ethereal card: no change");
    let i = hand_index(&s, defy);
    let _ = play_card(&mut s, i, Some(0));
    assert_eq!(cost(&s, cry_hand), 7, "the hand copy: 9 - 2");
    assert_eq!(cost(&s, cry_discard), 7, "any combat pile, the discard too");
}

// ----------------------------------------------------------- dark embrace --

fn dark_embrace_end_turn(no_draw_first: bool) -> bool {
    let mut s = board(3);
    let me = s.player.creature_uid.unwrap();
    let _ = me;
    if no_draw_first {
        power(&mut s, PowerKind::NoDraw, 1);
        power(&mut s, PowerKind::DarkEmbrace, 1);
    } else {
        power(&mut s, PowerKind::DarkEmbrace, 1);
        power(&mut s, PowerKind::NoDraw, 1);
    }
    let dazed = add(&mut s, CardId::Dazed);
    s.player.hand.push(dazed);
    let drawn = add(&mut s, CardId::StrikeIronclad);
    s.player.draw.push(drawn);
    end_turn(&mut s);
    s.player.hand.contains(&drawn)
}

#[test]
fn dark_embrace_before_no_draw_stays_vetoed_at_turn_end() {
    assert!(
        !dark_embrace_end_turn(false),
        "Dark Embrace's AfterSideTurnEnd runs first: NoDraw still vetoes its draw"
    );
}

#[test]
fn dark_embrace_after_no_draw_draws_at_turn_end() {
    assert!(
        dark_embrace_end_turn(true),
        "NoDraw removed itself first: the draw goes through"
    );
}

// ----------------------------------------------------- ethereal turn-end card --

fn shame_end_turn(ethereal: bool) -> (CombatState, CardUid) {
    let mut s = board(3);
    let shame = add(&mut s, CardId::Shame);
    if ethereal {
        s.cards[shame.get() as usize - 1].set_combat_var(VarKey::EtherealThisCombat, 1);
    }
    s.player.hand.push(shame);
    end_turn(&mut s);
    (s, shame)
}

#[test]
fn an_ethereal_shame_applies_its_frail_and_then_exhausts() {
    let (s, shame) = shame_end_turn(true);
    assert!(
        s.player_power_amount(PowerKind::Frail) > 0,
        "the turn-end effect runs before the Ethereal exhaust"
    );
    assert!(s.player.exhaust.contains(&shame), "then it is exhausted");
    assert!(!s.player.discard.contains(&shame));
}

#[test]
fn a_plain_shame_applies_its_frail_and_goes_to_the_discard_pile() {
    let (s, shame) = shame_end_turn(false);
    assert!(s.player_power_amount(PowerKind::Frail) > 0);
    assert!(s.player.discard.contains(&shame));
}
