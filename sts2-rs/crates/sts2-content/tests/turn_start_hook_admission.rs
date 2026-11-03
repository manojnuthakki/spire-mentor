//! New turn-start walks refuse ending combat; admitted walks keep their tail.
//! Authority: v0.107.1 Hook.IterateCombatHookListeners / CombatManager.SetupPlayerTurn.
use sts2_content::cards::spec_index::card_pod_from_id;
use sts2_core::cards::CardId;
use sts2_core::combat_loop::{combat_is_ending, start_player_turn};
use sts2_core::{CardUid, CombatPhase, CombatState, MonsterKind, PowerKind, Relic, RelicKind};

fn board(enemy_hp: i32) -> CombatState {
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
        .current_hp = enemy_hp;
    s
}

fn power(s: &mut CombatState, kind: PowerKind, amount: i32) {
    sts2_core::card_effects::apply_power_to_creature(
        s,
        s.player.creature_uid.unwrap(),
        kind,
        amount,
    );
    if kind == PowerKind::CrimsonMantle {
        for p in s.powers.iter_mut().filter(|p| p.kind == kind) {
            p.counter = 1;
        }
    }
}

fn draw_card(s: &mut CombatState, id: CardId) {
    let mut c = card_pod_from_id(id, false).unwrap();
    c.uid = CardUid::new(s.cards.len() as u32 + 1).unwrap();
    s.cards.push(c);
    s.player.draw.push(c.uid);
}

fn flower(s: &mut CombatState) {
    s.relics.push(Relic {
        id: std::num::NonZeroU32::new(1).unwrap(),
        kind: RelicKind::HappyFlower,
        counter: 0,
        is_melted: false,
        is_wax: false,
    });
}

#[test]
fn lethal_natural_hellraiser_draw_refuses_new_power_and_relic_walks() {
    for lethal in [true, false] {
        let mut s = board(if lethal { 1 } else { 100 });
        power(&mut s, PowerKind::Hellraiser, 1);
        power(&mut s, PowerKind::CrimsonMantle, 8);
        power(&mut s, PowerKind::Rupture, 2);
        flower(&mut s);
        draw_card(&mut s, CardId::StrikeIronclad);
        for _ in 0..4 {
            draw_card(&mut s, CardId::DefendIronclad);
        }
        let mut clone = s.clone();
        start_player_turn(&mut s);
        start_player_turn(&mut clone);
        assert_eq!(
            s, clone,
            "clones execute the same admitted boundaries and RNG"
        );
        assert_eq!(s.player.current_hp, if lethal { 80 } else { 79 });
        assert_eq!(s.player.block, if lethal { 0 } else { 8 });
        assert_eq!(
            s.player_power_amount(PowerKind::Strength),
            if lethal { 0 } else { 2 }
        );
        assert_eq!(s.relics[0].counter, i32::from(!lethal));
        assert_eq!(s.is_over, lethal);
        assert_eq!(
            s.player.play.len(),
            0,
            "the started Strike finished routing"
        );
    }
}

fn inferno(s: &mut CombatState) {
    power(s, PowerKind::Inferno, 6);
    for p in s.powers.iter_mut().filter(|p| p.kind == PowerKind::Inferno) {
        p.counter = 1;
    }
}

#[test]
fn lethal_started_power_walk_continues_but_new_side_walk_refuses() {
    for lethal in [true, false] {
        let mut s = board(if lethal { 6 } else { 100 });
        inferno(&mut s);
        power(&mut s, PowerKind::CrimsonMantle, 8);
        flower(&mut s);
        let rng = s.run_rng.clone();
        start_player_turn(&mut s);
        assert_eq!(
            s.player.current_hp, 78,
            "Crimson Mantle still fires in the admitted Normal walk"
        );
        assert_eq!(s.player.block, 8);
        assert_eq!(
            s.relics[0].counter,
            i32::from(!lethal),
            "Happy Flower belongs to a NEW side walk"
        );
        assert_eq!(combat_is_ending(&s), lethal);
        assert_eq!(s.run_rng, rng, "these callbacks have no random choice");
    }
}

#[test]
fn natural_draw_parked_by_stratagem_admits_hooks_only_after_resume() {
    for lethal in [true, false] {
        let mut s = board(if lethal { 1 } else { 100 });
        power(&mut s, PowerKind::Stratagem, 1);
        power(&mut s, PowerKind::Hellraiser, 1);
        power(&mut s, PowerKind::CrimsonMantle, 8);
        flower(&mut s);
        draw_card(&mut s, CardId::DefendIronclad);
        for _ in 0..5 {
            draw_card(&mut s, CardId::StrikeIronclad);
        }
        s.player.discard = std::mem::take(&mut s.player.draw);
        start_player_turn(&mut s);
        assert!(s.pending_choice.is_some());
        assert!(s.pending_draw_resume.as_ref().unwrap().resume_turn_setup);
        assert_eq!(s.player.current_hp, 80);
        assert_eq!(s.relics[0].counter, 0);
        let mut clone = s.clone();
        let action = sts2_core::Action::ResolveChoice {
            selection: smallvec::smallvec![0],
        };
        sts2_core::apply(&mut s, action.clone());
        sts2_core::apply(&mut clone, action);
        assert_eq!(s, clone);
        assert!(s.pending_choice.is_none());
        assert!(s.pending_draw_resume.is_none());
        assert_eq!(s.player.current_hp, if lethal { 80 } else { 79 });
        assert_eq!(s.relics[0].counter, i32::from(!lethal));
    }
}

fn normal_relic(s: &mut CombatState) -> sts2_core::EffectResult {
    assert!(
        combat_is_ending(s),
        "the Normal walk was admitted before Inferno killed"
    );
    s.relics
        .iter_mut()
        .find(|r| r.kind == RelicKind::Anchor)
        .unwrap()
        .counter += 1;
    sts2_core::EffectResult::Resolved
}

#[test]
fn parked_normal_walk_finishes_its_powers_and_relics_exactly_once() {
    let mut s = board(6);
    power(&mut s, PowerKind::Tyranny, 1);
    inferno(&mut s);
    power(&mut s, PowerKind::CrimsonMantle, 8);
    flower(&mut s);
    s.relics.push(Relic {
        id: std::num::NonZeroU32::new(2).unwrap(),
        kind: RelicKind::Anchor,
        counter: 0,
        is_melted: false,
        is_wax: false,
    });
    // Observe a real relic bus slot without replacing any power/card body.
    sts2_core::relic_effects::register_after_player_turn_start(
        sts2_core::relics::RelicId::Anchor,
        normal_relic,
    );
    for _ in 0..7 {
        draw_card(&mut s, CardId::DefendIronclad);
    }
    start_player_turn(&mut s);
    assert!(s.pending_cluster_resume.is_some());
    assert!(!combat_is_ending(&s));
    let mut clone = s.clone();
    let action = sts2_core::Action::ResolveChoice {
        selection: smallvec::smallvec![0],
    };
    sts2_core::apply(&mut s, action.clone());
    sts2_core::apply(&mut clone, action);
    assert_eq!(s, clone);
    assert!(s.pending_cluster_resume.is_none());
    assert_eq!(s.player.current_hp, 78);
    assert_eq!(s.player.block, 8);
    assert_eq!(s.relics[0].counter, 0);
    assert_eq!(
        s.relics[1].counter, 1,
        "same Normal walk's relic tail is not a new admission"
    );
}
