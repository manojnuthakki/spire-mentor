//! Stomp.BeforeCardPlayed runs before each actual Attack body, including
//! CardCmd.AutoPlay and repeated plays (v0.107.1 CardModel.OnPlayWrapper).
//! No resource/source fields or selection lifecycle changes are needed.
use std::sync::{Mutex, OnceLock};
use sts2_content::cards::spec_index::card_pod_from_id;
use sts2_core::card_effects::{EffectResult, PlayCardFn};
use sts2_core::cards::{CardId, VarKey};
use sts2_core::combat_loop::{auto_play_specific_card, play_card};
use sts2_core::{
    apply, legal_actions, Action, CardUid, CombatPhase, CombatState, CreatureUid, MonsterKind,
    PowerKind,
};

fn add(s: &mut CombatState, id: CardId) -> CardUid {
    let mut c = card_pod_from_id(id, false).unwrap();
    c.uid = CardUid::new(s.cards.len() as u32 + 1).unwrap();
    s.cards.push(c);
    c.uid
}

fn board() -> (CombatState, CardUid) {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut s = CombatState::default();
    s.player.current_hp = 80;
    s.player.max_hp = 80;
    s.player.energy = 3;
    s.init_player_creature();
    let e = s.spawn_enemy(MonsterKind::Nibbit);
    let enemy = s.creatures.iter_mut().find(|c| c.id == e.0).unwrap();
    enemy.current_hp = 500;
    enemy.max_hp = 500;
    s.phase = CombatPhase::PlayerPlayPhase;
    let stomp = add(&mut s, CardId::Stomp);
    s.player.discard.push(stomp);
    let defend = add(&mut s, CardId::DefendIronclad);
    s.player.discard.push(defend);
    (s, stomp)
}

fn shaved(s: &CombatState, uid: CardUid, n: i32) {
    let c = &s.cards[uid.get() as usize - 1];
    assert_eq!(i32::from(c.cost), (3 - n).max(0));
    assert_eq!(c.combat_var(VarKey::StompCostReductionThisTurn), n);
}

fn answer(s: &mut CombatState) {
    let a = legal_actions(s)
        .into_iter()
        .find(|a| matches!(a, Action::ResolveChoice { selection } if !selection.is_empty()))
        .expect("a real surfaced choice");
    let _ = apply(s, a);
}

#[test]
fn havoc_headbutt_shaves_before_selection_and_resume_does_not_shave_again() {
    let (mut s, stomp) = board();
    let havoc = add(&mut s, CardId::Havoc);
    let headbutt = add(&mut s, CardId::Headbutt);
    s.player.hand.push(havoc);
    s.player.draw.push(headbutt);
    let _ = play_card(&mut s, 0, None);
    assert!(s.pending_choice.is_some());
    assert!(s.pending_autoplay_resume.is_some());
    shaved(&s, stomp, 1);
    let mut clone = s.clone();
    assert_eq!(clone, s);
    answer(&mut clone);
    shaved(&clone, stomp, 1);
    assert!(clone.pending_autoplay_resume.is_none());
    assert!(
        s.pending_choice.is_some(),
        "clone must not resume its parent"
    );
    answer(&mut s);
    assert_eq!(s, clone);
}

#[test]
fn stampede_headbutt_shaves_while_its_end_turn_body_is_parked() {
    let (mut s, stomp) = board();
    let headbutt = add(&mut s, CardId::Headbutt);
    s.player.hand.push(headbutt);
    let p = s.player.creature_uid.unwrap();
    sts2_core::card_effects::apply_power_to_creature(&mut s, p, PowerKind::Stampede, 1);
    let _ = apply(&mut s, Action::EndTurn);
    assert!(s.pending_choice.is_some());
    assert!(s.pending_end_turn_resume.is_some());
    shaved(&s, stomp, 1);
}

static ORIGINAL: OnceLock<PlayCardFn> = OnceLock::new();
static BODY_COSTS: Mutex<Vec<i32>> = Mutex::new(Vec::new());
fn observed_headbutt(s: &mut CombatState, target: Option<CreatureUid>) -> EffectResult {
    // Observe the entry to the genuine content body, then execute it normally.
    let c = s.cards.iter().find(|c| c.card_id == CardId::Stomp).unwrap();
    BODY_COSTS.lock().unwrap().push(i32::from(c.cost));
    ORIGINAL.get().unwrap()(s, target)
}

#[test]
fn every_autoplay_repeat_shaves_before_its_real_body_once() {
    let (mut s, stomp) = board();
    ORIGINAL
        .set(sts2_core::card_effects::lookup(CardId::Headbutt).unwrap())
        .unwrap();
    sts2_core::card_effects::register(CardId::Headbutt, observed_headbutt);
    let headbutt = add(&mut s, CardId::Headbutt);
    s.cards[headbutt.get() as usize - 1].base_replay_count = 2;
    s.player.draw.push(headbutt);
    assert!(auto_play_specific_card(&mut s, headbutt, false));
    assert!(s.pending_choice.is_some());
    answer(&mut s);
    assert_eq!(*BODY_COSTS.lock().unwrap(), vec![2, 1, 0]);
    shaved(&s, stomp, 3);
}

#[test]
fn manual_headbutt_keeps_its_existing_pre_selection_shave() {
    let (mut s, stomp) = board();
    let headbutt = add(&mut s, CardId::Headbutt);
    s.player.hand.push(headbutt);
    let _ = play_card(&mut s, 0, Some(0));
    assert!(s.pending_choice.is_some());
    shaved(&s, stomp, 1);
    answer(&mut s);
    shaved(&s, stomp, 1);
}

#[test]
fn skill_autoplay_and_refused_attack_do_not_shave() {
    let (mut s, stomp) = board();
    let defend = add(&mut s, CardId::DefendIronclad);
    s.player.draw.push(defend);
    assert!(auto_play_specific_card(&mut s, defend, false));
    shaved(&s, stomp, 0);
    let headbutt = add(&mut s, CardId::Headbutt);
    s.player.draw.push(headbutt);
    let p = sts2_core::PowerUid::new(s.powers.len() as u32 + 1).unwrap();
    s.powers.push(sts2_core::Power {
        id: p.0,
        kind: PowerKind::Sloth,
        amount: 1,
        counter: 0,
        applier: None,
        skip_next_tick: false,
    });
    s.player.powers.push(p);
    s.player.card_plays_started_this_turn = 3;
    assert!(auto_play_specific_card(&mut s, headbutt, false));
    assert!(s.pending_choice.is_none());
    shaved(&s, stomp, 0);
    assert!(!auto_play_specific_card(&mut s, headbutt, false));
    shaved(&s, stomp, 0);
}

#[test]
fn lethal_started_autoplay_keeps_its_shave_and_finished_play_tally() {
    let (mut s, stomp) = board();
    s.creatures
        .iter_mut()
        .find(|c| c.species.is_some())
        .unwrap()
        .current_hp = 1;
    let strike = add(&mut s, CardId::StrikeIronclad);
    s.player.draw.push(strike);
    assert!(auto_play_specific_card(&mut s, strike, true));
    shaved(&s, stomp, 1);
    assert_eq!(s.combat_log.player_card_plays_this_combat, 1);
}

#[test]
fn autoplay_without_stomp_is_unchanged() {
    let (mut s, stomp) = board();
    // Remove the watched card from the arena as well as its pile.
    s.player.discard.retain(|u| *u != stomp);
    s.cards[stomp.get() as usize - 1].card_id = CardId::DefendIronclad;
    s.cards = s.cards.iter().copied().collect();
    let strike = add(&mut s, CardId::StrikeIronclad);
    s.player.draw.push(strike);
    assert!(auto_play_specific_card(&mut s, strike, false));
    assert!(s.player.discard.contains(&strike));
    assert_eq!(s.combat_log.player_card_plays_this_combat, 1);
}
