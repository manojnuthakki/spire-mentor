//! Pendulum awaits its third draw inside the admitted Normal player-start walk.
//! Authority: v0.107.1 Pendulum.AfterPlayerTurnStart / Hook / CardPileCmd.Draw.
use sts2_content::cards::spec_index::card_pod_from_id;
use sts2_core::cards::CardId;
use sts2_core::combat_loop::{combat_is_ending, start_player_turn};
use sts2_core::{
    Action, CardUid, CombatPhase, CombatState, MonsterKind, PowerKind, Relic, RelicKind,
};

fn board(hp: i32) -> CombatState {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut s = CombatState::default();
    s.player.current_hp = 80;
    s.player.max_hp = 80;
    s.init_player_creature();
    s.phase = CombatPhase::PlayerTurnStart;
    s.round = 2;
    let e = s.spawn_enemy(MonsterKind::Nibbit);
    s.creatures
        .iter_mut()
        .find(|c| c.id == e.0)
        .unwrap()
        .current_hp = hp;
    s
}
fn relic(s: &mut CombatState, kind: RelicKind, counter: i32) {
    s.relics.push(Relic {
        id: std::num::NonZeroU32::new(s.relics.len() as u32 + 1).unwrap(),
        kind,
        counter,
        is_melted: false,
        is_wax: false,
    });
}
fn power(s: &mut CombatState, kind: PowerKind, amount: i32) {
    sts2_core::card_effects::apply_power_to_creature(
        s,
        s.player.creature_uid.unwrap(),
        kind,
        amount,
    );
    if kind == PowerKind::Inferno {
        for p in s.powers.iter_mut().filter(|p| p.kind == kind) {
            p.counter = 1;
        }
    }
}
fn card(s: &mut CombatState, id: CardId) -> CardUid {
    let mut c = card_pod_from_id(id, false).unwrap();
    c.uid = CardUid::new(s.cards.len() as u32 + 1).unwrap();
    s.cards.push(c);
    s.player.draw.push(c.uid);
    c.uid
}
fn natural(s: &mut CombatState) {
    for _ in 0..5 {
        card(s, CardId::DefendIronclad);
    }
}
fn counter(s: &CombatState) -> i32 {
    s.relics
        .iter()
        .find(|r| r.kind == RelicKind::Pendulum)
        .unwrap()
        .counter
}
fn resolve(s: &mut CombatState) {
    let mut clone = s.clone();
    let action = Action::ResolveChoice {
        selection: smallvec::smallvec![0],
    };
    sts2_core::apply(s, action.clone());
    sts2_core::apply(&mut clone, action);
    assert_eq!(*s, clone, "clones retain all parked callbacks and RNG");
}

#[test]
fn normal_counter_continues_after_lethal_power_but_draw_refuses() {
    for initial in [0, 1, 2] {
        let mut s = board(6);
        power(&mut s, PowerKind::Inferno, 6);
        relic(&mut s, RelicKind::Pendulum, initial);
        natural(&mut s);
        let undrawn = card(&mut s, CardId::StrikeIronclad);
        let rng = s.run_rng.clone();
        start_player_turn(&mut s);
        assert!(combat_is_ending(&s));
        assert_eq!(counter(&s), (initial + 1) % 3);
        assert!(s.player.draw.contains(&undrawn), "new Draw refuses ending");
        assert_eq!(s.run_rng, rng);
    }
}

#[test]
fn live_non_third_and_third_draw_once_before_side_start() {
    for initial in [0, 1, 2] {
        let mut s = board(100);
        relic(&mut s, RelicKind::Pendulum, initial);
        natural(&mut s);
        let extra = card(&mut s, CardId::Bash);
        let rng = s.run_rng.clone();
        let mut clone = s.clone();
        start_player_turn(&mut s);
        start_player_turn(&mut clone);
        assert_eq!(s, clone);
        assert_eq!(counter(&s), (initial + 1) % 3);
        assert_eq!(s.player.hand.contains(&extra), initial == 2);
        assert_eq!(s.cards_drawn, if initial == 2 { 6 } else { 5 });
        assert_eq!(s.run_rng, rng);
    }
}

#[test]
fn mercury_equip_order_decides_whether_third_draw_was_admitted() {
    for pendulum_first in [true, false] {
        let mut s = board(3);
        if pendulum_first {
            relic(&mut s, RelicKind::Pendulum, 2);
        }
        relic(&mut s, RelicKind::MercuryHourglass, 0);
        if !pendulum_first {
            relic(&mut s, RelicKind::Pendulum, 2);
        }
        natural(&mut s);
        let extra = card(&mut s, CardId::Bash);
        let rng = s.run_rng.clone();
        start_player_turn(&mut s);
        assert_eq!(counter(&s), 0, "same admitted walk still visits Pendulum");
        assert_eq!(s.player.hand.contains(&extra), pendulum_first);
        assert!(combat_is_ending(&s));
        assert_eq!(s.run_rng, rng);
    }
}

#[test]
fn natural_hellraiser_lethal_refuses_new_normal_walk() {
    let mut s = board(1);
    power(&mut s, PowerKind::Hellraiser, 1);
    relic(&mut s, RelicKind::Pendulum, 2);
    for _ in 0..6 {
        card(&mut s, CardId::StrikeIronclad);
    }
    start_player_turn(&mut s);
    assert_eq!(counter(&s), 2, "step23 new-walk admission stays intact");
    assert!(combat_is_ending(&s));
}

#[test]
fn third_hellraiser_draw_finishes_before_later_relic_and_consumes_one_target() {
    for hp in [1, 100] {
        let mut s = board(hp);
        power(&mut s, PowerKind::Hellraiser, 1);
        relic(&mut s, RelicKind::Pendulum, 2);
        relic(&mut s, RelicKind::MercuryHourglass, 0);
        natural(&mut s);
        let strike = card(&mut s, CardId::StrikeIronclad);
        start_player_turn(&mut s);
        assert_eq!(counter(&s), 0);
        assert!(s.player.discard.contains(&strike));
        assert!(s.player.play.is_empty());
        assert_eq!(s.run_rng.combat_targets.counter(), 1);
        let hp_after = s
            .creatures
            .iter()
            .find(|c| s.enemy_uids.iter().any(|u| u.0 == c.id))
            .unwrap()
            .current_hp;
        assert_eq!(hp_after, if hp == 1 { 0 } else { 91 });
    }
}

#[test]
fn stratagem_third_draw_parks_later_mercury_and_resumes_once() {
    let mut s = board(100);
    power(&mut s, PowerKind::Stratagem, 1);
    relic(&mut s, RelicKind::Pendulum, 2);
    relic(&mut s, RelicKind::MercuryHourglass, 0);
    natural(&mut s);
    let extra = card(&mut s, CardId::Bash);
    s.player.draw.pop();
    s.player.discard.push(extra);
    let other = card(&mut s, CardId::DefendIronclad);
    s.player.draw.pop();
    s.player.discard.push(other);
    start_player_turn(&mut s);
    assert!(s.pending_choice.is_some());
    assert_eq!(counter(&s), 0);
    assert_eq!(
        s.pending_cluster_resume.as_ref().unwrap().relic_start,
        Some(1)
    );
    assert_eq!(
        s.creatures
            .iter()
            .find(|c| c.id == s.enemy_uids[0].0)
            .unwrap()
            .current_hp,
        100
    );
    resolve(&mut s);
    assert_eq!(
        s.player.hand.len(),
        7,
        "Stratagem also moves its chosen card to hand"
    );
    assert_eq!(s.cards_drawn, 6);
    assert_eq!(counter(&s), 0);
    assert!(s.pending_cluster_resume.is_none());
    assert!(s.pending_draw_resume.is_none());
    assert!(s.pending_choice.is_none());
    assert_eq!(
        s.creatures
            .iter()
            .find(|c| c.id == s.enemy_uids[0].0)
            .unwrap()
            .current_hp,
        97
    );
}

#[test]
fn tyranny_parked_normal_walk_enters_pendulum_once_after_pick() {
    let mut s = board(6);
    power(&mut s, PowerKind::Tyranny, 1);
    power(&mut s, PowerKind::Inferno, 6);
    relic(&mut s, RelicKind::Pendulum, 2);
    natural(&mut s);
    card(&mut s, CardId::Bash);
    start_player_turn(&mut s);
    assert_eq!(counter(&s), 2);
    assert!(s.pending_cluster_resume.is_some());
    resolve(&mut s);
    assert_eq!(counter(&s), 0);
    assert!(combat_is_ending(&s));
    assert_eq!(s.cards_drawn, 6, "Tyranny adds one to the natural draw");
    assert!(s.pending_cluster_resume.is_none());
}

#[test]
fn hellraiser_parked_third_draw_awaits_card_result_before_later_relic() {
    let mut s = board(100);
    power(&mut s, PowerKind::Hellraiser, 1);
    relic(&mut s, RelicKind::Pendulum, 2);
    relic(&mut s, RelicKind::MercuryHourglass, 0);
    natural(&mut s);
    let strike = card(&mut s, CardId::SeekerStrike);
    card(&mut s, CardId::Bash);
    card(&mut s, CardId::DefendIronclad);
    start_player_turn(&mut s);
    assert!(s.pending_choice.is_some());
    assert!(s.player.play.contains(&strike));
    assert_eq!(counter(&s), 0);
    assert_eq!(
        s.pending_cluster_resume.as_ref().unwrap().relic_start,
        Some(1)
    );
    assert!(s.pending_autoplay_resume.is_some());
    assert!(s.pending_draw_resume.is_some());
    let before = s
        .creatures
        .iter()
        .find(|c| c.id == s.enemy_uids[0].0)
        .unwrap()
        .current_hp;
    assert_eq!(before, 91, "Mercury awaits the parked Seeker Strike");
    resolve(&mut s);
    assert!(s.player.discard.contains(&strike));
    assert!(s.player.play.is_empty());
    assert!(s.pending_autoplay_resume.is_none());
    assert!(s.pending_draw_resume.is_none());
    assert!(s.pending_cluster_resume.is_none());
    assert!(s.pending_choice.is_none());
    assert_eq!(counter(&s), 0);
    assert_eq!(s.run_rng.combat_targets.counter(), 1);
    assert_eq!(
        s.creatures
            .iter()
            .find(|c| c.id == s.enemy_uids[0].0)
            .unwrap()
            .current_hp,
        before - 3
    );
}

#[test]
fn natural_stratagem_draw_resumes_before_pendulum_is_admitted() {
    let mut s = board(100);
    power(&mut s, PowerKind::Stratagem, 1);
    relic(&mut s, RelicKind::Pendulum, 2);
    natural(&mut s);
    let extra = card(&mut s, CardId::Bash);
    card(&mut s, CardId::Bash);
    s.player.discard = std::mem::take(&mut s.player.draw);
    start_player_turn(&mut s);
    assert_eq!(counter(&s), 2);
    assert!(s.pending_draw_resume.as_ref().unwrap().resume_turn_setup);
    resolve(&mut s);
    assert_eq!(counter(&s), 0);
    assert_eq!(s.cards_drawn, 6);
    assert!(s.player.hand.contains(&extra));
    assert!(s.pending_draw_resume.is_none());
}
