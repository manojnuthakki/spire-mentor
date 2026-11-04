//! F6 step35, the relic-hook batch (fresh DLL authority, sha 08faee0c...):
//!
//! * `CardModel.OnPlayWrapper` ends with `CombatManager.CheckForEmptyHand`
//!   (no card or potion effect executing, hand empty): `Hook.AfterHandEmptied`,
//!   Unceasing Top's draw (`unceasing-top`).
//! * `Kusarigama.AfterSideTurnEnd` resets its count at the turn's END
//!   (`kusarigama`).
//! * `PaelsTears.AfterCombatEnd` clears its leftover-energy latch
//!   (`paels-tears-combat-end`).
//! * `PenNib.BeforeCardPlayed` bumps the count before the attack's effects and
//!   marks the 10th in `AttackToDouble` until its `AfterCardPlayed` (`pen-nib`).
//! * `BingBong.AfterCardChangedPiles` clones every card entering the Deck,
//!   the run-side adds included (`bing-bong`).
use std::num::NonZeroU32;
use sts2_content::cards::spec_index::card_pod_from_id;
use sts2_core::cards::CardId;
use sts2_core::combat_loop::{check_combat_end, end_turn_phase, play_card};
use sts2_core::relics::RelicId;
use sts2_core::{CardUid, CombatPhase, CombatState, MonsterKind, Relic, RelicKind};

fn add(s: &mut CombatState, id: CardId) -> CardUid {
    let mut c = card_pod_from_id(id, false).unwrap();
    c.uid = CardUid::new(s.cards.len() as u32 + 1).unwrap();
    s.cards.push(c);
    c.uid
}

fn relic(s: &mut CombatState, kind: RelicKind, counter: i32) {
    let id = NonZeroU32::new(900 + s.relics.len() as u32).unwrap();
    let mut r = Relic::new(id, kind);
    r.counter = counter;
    s.relics.push(r);
}

fn counter(s: &CombatState, kind: RelicKind) -> i32 {
    s.relics.iter().find(|r| r.kind == kind).unwrap().counter
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

fn enemy_hp(s: &CombatState) -> i32 {
    let uid = s.enemy_uids[0];
    sts2_core::creature_by_id(&s.creatures, uid.0)
        .unwrap()
        .current_hp
}

fn hand_index(s: &CombatState, uid: CardUid) -> u8 {
    s.player.hand.iter().position(|&u| u == uid).unwrap() as u8
}

fn play(s: &mut CombatState, uid: CardUid) {
    let target = match s.cards[uid.get() as usize - 1].target_type {
        sts2_core::cards::TargetType::Enemy => Some(0),
        _ => None,
    };
    let _ = play_card(s, hand_index(s, uid), target);
}

// ------------------------------------------------------------ unceasing top --

#[test]
fn unceasing_top_draws_when_the_last_hand_card_is_played() {
    let mut s = board(3);
    relic(&mut s, RelicKind::UnceasingTop, 0);
    let last = add(&mut s, CardId::DefendIronclad);
    s.player.hand.push(last);
    let d1 = add(&mut s, CardId::StrikeIronclad);
    let d2 = add(&mut s, CardId::StrikeIronclad);
    s.player.draw.push(d1);
    s.player.draw.push(d2);
    play(&mut s, last);
    assert_eq!(
        s.player.hand.as_slice(),
        &[d1],
        "the hand emptied: one card drawn"
    );
}

#[test]
fn unceasing_top_waits_for_the_outer_play_when_a_child_card_empties_the_hand() {
    // Havoc plays the draw pile's top card, which empties nothing by itself;
    // the child's end sees Havoc still executing (depth > 0) and the check
    // belongs to Havoc's own end: exactly ONE card is drawn.
    let mut s = board(3);
    relic(&mut s, RelicKind::UnceasingTop, 0);
    let havoc = add(&mut s, CardId::Havoc);
    s.player.hand.push(havoc);
    let child = add(&mut s, CardId::DefendIronclad);
    let d1 = add(&mut s, CardId::StrikeIronclad);
    let d2 = add(&mut s, CardId::StrikeIronclad);
    s.player.draw.extend([d2, d1, child]);
    play(&mut s, havoc);
    assert_eq!(
        s.player.hand.as_slice(),
        &[d1],
        "Havoc's child ran inside it; one draw at Havoc's end"
    );
}

#[test]
fn unceasing_top_does_not_draw_with_cards_left_in_hand() {
    let mut s = board(3);
    relic(&mut s, RelicKind::UnceasingTop, 0);
    let a = add(&mut s, CardId::DefendIronclad);
    let b = add(&mut s, CardId::DefendIronclad);
    s.player.hand.extend([a, b]);
    let d1 = add(&mut s, CardId::StrikeIronclad);
    s.player.draw.push(d1);
    play(&mut s, a);
    assert_eq!(s.player.hand.as_slice(), &[b]);
}

// --------------------------------------------------------------- kusarigama --

#[test]
fn kusarigama_resets_at_the_players_turn_end() {
    let mut s = board(3);
    relic(&mut s, RelicKind::Kusarigama, 0);
    for _ in 0..2 {
        let u = add(&mut s, CardId::StrikeIronclad);
        s.player.hand.push(u);
        play(&mut s, u);
    }
    assert_eq!(counter(&s, RelicKind::Kusarigama), 2);
    for phase in 0..2 {
        let _ = end_turn_phase(&mut s, phase);
    }
    assert_eq!(
        counter(&s, RelicKind::Kusarigama),
        0,
        "AfterSideTurnEnd reset: the enemy turn (and a turn-end selection) sees 0"
    );
}

// ------------------------------------------------------------ pael's tears --

#[test]
fn paels_tears_latch_is_cleared_when_the_combat_ends() {
    let mut s = board(3);
    relic(&mut s, RelicKind::PaelsTears, 1);
    let uid = s.enemy_uids[0];
    s.creatures
        .iter_mut()
        .find(|c| c.id == uid.0)
        .unwrap()
        .current_hp = 0;
    let _ = check_combat_end(&mut s);
    assert!(s.is_over);
    assert_eq!(
        counter(&s, RelicKind::PaelsTears),
        0,
        "AfterCombatEnd: HadLeftoverEnergy = false"
    );
}

// ------------------------------------------------------------------ pen nib --

#[test]
fn pen_nib_counts_before_the_attack_resolves() {
    // Headbutt opens a discard selection; PenNib.BeforeCardPlayed has already
    // bumped the count, so the open selection shows it.
    let mut s = board(3);
    relic(&mut s, RelicKind::PenNib, 5);
    let hb = add(&mut s, CardId::Headbutt);
    s.player.hand.push(hb);
    let dis = add(&mut s, CardId::DefendIronclad);
    let dis2 = add(&mut s, CardId::StrikeIronclad);
    s.player.discard.extend([dis, dis2]);
    let _ = play_card(&mut s, 0, Some(0));
    assert!(s.pending_choice.is_some(), "Headbutt's selection is open");
    assert_eq!(counter(&s, RelicKind::PenNib), 6);
}

#[test]
fn pen_nib_doubles_the_tenth_attack_and_only_it() {
    let mut s = board(9);
    relic(&mut s, RelicKind::PenNib, 8);
    let mut hits = Vec::new();
    for _ in 0..3 {
        let u = add(&mut s, CardId::StrikeIronclad);
        s.player.hand.push(u);
        let before = enemy_hp(&s);
        play(&mut s, u);
        hits.push(before - enemy_hp(&s));
    }
    assert_eq!(
        hits,
        vec![6, 12, 6],
        "the 9th hits 6, the 10th doubled, the 11th 6"
    );
    assert_eq!(s.combat_log.pen_nib_attack_to_double, None);
    assert_eq!(counter(&s, RelicKind::PenNib), 11);
}

#[test]
fn unceasing_top_draws_inside_the_auto_pre_play_phase() {
    // Mayhem auto-plays Fiend Fire (exhausting the hand) in AutoPrePlay: the
    // DLL's `IsValidPhase` allows Top there, so one more card is drawn.
    let mut s = board(3);
    relic(&mut s, RelicKind::UnceasingTop, 0);
    let puid = s.player.creature_uid.expect("player creature");
    sts2_core::card_effects::apply_power_to_creature(&mut s, puid, sts2_core::PowerKind::Mayhem, 1);
    let mut draw = Vec::new();
    for _ in 0..5 {
        draw.push(add(&mut s, CardId::DefendIronclad));
    }
    draw.push(add(&mut s, CardId::FiendFire));
    let extra = add(&mut s, CardId::DefendIronclad);
    draw.push(extra);
    s.player.draw = draw.into();
    s.phase = CombatPhase::PlayerTurnStart;
    let _ = sts2_core::combat_loop::start_player_turn(&mut s);
    assert_eq!(s.phase, CombatPhase::PlayerPlayPhase);
    assert_eq!(
        s.player.hand.as_slice(),
        &[extra],
        "Top drew in AutoPrePlay"
    );
    assert!(
        !s.combat_log.auto_pre_play_open,
        "the window closes with the phase"
    );
}

#[test]
fn unceasing_top_waits_for_late_card_hooks_to_refill_the_hand() {
    // Make It So returns to hand (AfterCardPlayedLate) when the 3rd skill is
    // played; the empty-hand check runs after it, so the hand is not empty.
    let mut s = board(10);
    relic(&mut s, RelicKind::UnceasingTop, 0);
    let mis = add(&mut s, CardId::MakeItSo);
    let anger = add(&mut s, CardId::Anger);
    let defs: Vec<_> = (0..3)
        .map(|_| add(&mut s, CardId::DefendIronclad))
        .collect();
    s.player.hand.extend([mis, anger]);
    s.player.hand.extend(defs.iter().copied());
    let extra = add(&mut s, CardId::StrikeIronclad);
    s.player.draw.push(extra);
    play(&mut s, mis);
    play(&mut s, anger);
    for d in defs {
        play(&mut s, d);
    }
    assert_eq!(s.player.hand.as_slice(), &[mis], "Make It So came back");
    assert_eq!(s.player.draw.as_slice(), &[extra], "Top drew nothing");
}

// ---------------------------------------------------------------- bing bong --

#[test]
fn bing_bong_clones_a_run_side_deck_add() {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut rs = sts2_core::RunState::new();
    rs.relics.push(RelicId::BingBong);
    let before = rs.master_deck.len();
    let mut card = card_pod_from_id(CardId::PommelStrike, true).unwrap();
    card.uid = CardUid::new(7).unwrap();
    rs.add_card_to_deck(card, sts2_core::relic_effects::AddSource::Reward);
    assert_eq!(rs.master_deck.len(), before + 2, "the card and its clone");
    let n = rs.master_deck.len();
    assert_eq!(rs.master_deck[n - 1].card_id, CardId::PommelStrike);
    assert!(
        rs.master_deck[n - 1].upgraded,
        "the clone keeps the upgrade"
    );
    // Without the relic: one card.
    let mut plain = sts2_core::RunState::new();
    let b = plain.master_deck.len();
    plain.add_card_to_deck(card, sts2_core::relic_effects::AddSource::Reward);
    assert_eq!(plain.master_deck.len(), b + 1);
}
