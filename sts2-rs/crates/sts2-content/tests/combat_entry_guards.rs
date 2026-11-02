//! Entry boundaries from the installed v0.107.1 CardCmd/CardSelectCmd DLL.
//! These tests distinguish a new command from the tail of a started card.
use std::num::NonZeroU32;
use std::sync::{Mutex, OnceLock};
use sts2_content::cards::spec_index::card_pod_from_id;
use sts2_core::card_effects::{EffectResult, PlayCardFn};
use sts2_core::cards::{CardId, PileKind, VarKey};
use sts2_core::combat_loop::{
    auto_play_card_from_draw_top, auto_play_n_from_draw_top, auto_play_specific_card,
    auto_play_specific_card_from_discard, auto_play_specific_card_from_exhaust_at_target,
    play_card,
};
use sts2_core::{
    apply, legal_actions, Action, CardUid, ChoiceKind, ChoiceOption, CombatPhase, CombatState,
    CreatureUid, MonsterKind, PendingChoice, PostChoiceEffect, PowerKind, Relic, RelicKind,
};

fn board(hp: i32) -> (CombatState, CreatureUid) {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut s = CombatState::default();
    s.player.current_hp = 80;
    s.player.max_hp = 80;
    s.player.energy = 10;
    s.init_player_creature();
    s.phase = CombatPhase::PlayerPlayPhase;
    let e = s.spawn_enemy(MonsterKind::Nibbit);
    s.creatures
        .iter_mut()
        .find(|c| c.id == e.0)
        .unwrap()
        .current_hp = hp;
    (s, e)
}

fn add(s: &mut CombatState, id: CardId, pile: PileKind) -> CardUid {
    let mut c = card_pod_from_id(id, false).unwrap();
    c.uid = CardUid::new(s.cards.len() as u32 + 1).unwrap();
    s.cards.push(c);
    match pile {
        PileKind::Hand => s.player.hand.push(c.uid),
        PileKind::Draw => s.player.draw.push(c.uid),
        PileKind::Discard => s.player.discard.push(c.uid),
        PileKind::Exhaust => s.player.exhaust.push(c.uid),
        PileKind::Play => s.player.play.push(c.uid),
    }
    c.uid
}

fn power(s: &mut CombatState, kind: PowerKind, n: i32) {
    let p = s.player.creature_uid.unwrap();
    sts2_core::card_effects::apply_power_to_creature(s, p, kind, n);
}

fn ready_juggling(s: &mut CombatState) {
    power(s, PowerKind::Juggling, 1);
    // Juggling clones every third attack; this play is the third, as distinct
    // from merely holding the power. Keep the fixture's premise explicit.
    for p in s
        .powers
        .iter_mut()
        .filter(|p| p.kind == PowerKind::Juggling)
    {
        p.counter = 2;
    }
}

#[test]
fn source_autoplay_refuses_ending_terminal_and_dead_owner_before_any_mutation() {
    for boundary in 0..3 {
        for source in [PileKind::Draw, PileKind::Discard, PileKind::Exhaust] {
            let (mut s, e) = board(if boundary == 0 { 0 } else { 100 });
            if boundary == 1 {
                s.is_over = true;
            }
            if boundary == 2 {
                s.player.current_hp = 0;
            }
            let uid = add(&mut s, CardId::StrikeIronclad, source);
            let before = s.clone();
            let admitted = match source {
                PileKind::Draw => auto_play_specific_card(&mut s, uid, false),
                PileKind::Discard => auto_play_specific_card_from_discard(&mut s, uid, false),
                _ => auto_play_specific_card_from_exhaust_at_target(&mut s, uid, Some(e), false),
            };
            assert!(!admitted, "{source:?} boundary {boundary}");
            assert_eq!(
                s, before,
                "refusal must not move, count, target or consume RNG"
            );
        }
    }
}

#[test]
fn draw_command_refuses_before_shuffle_or_collection() {
    for many in [false, true] {
        let (mut s, _) = board(0);
        add(&mut s, CardId::StrikeIronclad, PileKind::Discard);
        add(&mut s, CardId::DefendIronclad, PileKind::Discard);
        let before = s.clone();
        if many {
            auto_play_n_from_draw_top(&mut s, 2, false);
        } else {
            assert!(!auto_play_card_from_draw_top(&mut s, false));
        }
        assert_eq!(s, before);
    }
}

static STRIKE: OnceLock<PlayCardFn> = OnceLock::new();
static BODIES: Mutex<Vec<(CardUid, Option<CreatureUid>, bool)>> = Mutex::new(Vec::new());
fn observed_strike(s: &mut CombatState, t: Option<CreatureUid>) -> EffectResult {
    let uid = sts2_core::card_effects::resolving_card_uid(s).unwrap();
    BODIES
        .lock()
        .unwrap()
        .push((uid, t, sts2_core::combat_loop::combat_is_ending(s)));
    STRIKE.get().unwrap()(s, t)
}
fn observe_strikes() {
    STRIKE
        .set(sts2_core::card_effects::lookup(CardId::StrikeIronclad).unwrap())
        .unwrap();
    sts2_core::card_effects::register(CardId::StrikeIronclad, observed_strike);
}

#[test]
fn admitted_collection_keeps_unstarted_sibling_and_lethal_card_callbacks() {
    let (mut s, e) = board(1);
    observe_strikes();
    let first = add(&mut s, CardId::StrikeIronclad, PileKind::Draw);
    let second = add(&mut s, CardId::StrikeIronclad, PileKind::Draw);
    ready_juggling(&mut s);
    auto_play_n_from_draw_top(&mut s, 2, false);
    assert_eq!(*BODIES.lock().unwrap(), vec![(first, Some(e), false)]);
    assert!(
        s.player.play.contains(&second),
        "gathered before refusal; no result-pile move"
    );
    assert_eq!(
        s.cards[second.get() as usize - 1].combat_var(VarKey::PendingBatchAutoPlay),
        1
    );
    assert_eq!(s.combat_log.player_card_plays_this_combat, 1);
    assert_eq!(
        s.cards.len(),
        3,
        "already-started lethal card still delivers Juggling"
    );
}

#[test]
fn stampede_keeps_caller_shuffle_picks_but_does_not_start_second_native_card() {
    let (mut s, e) = board(1);
    observe_strikes();
    let first = add(&mut s, CardId::StrikeIronclad, PileKind::Hand);
    power(&mut s, PowerKind::Stampede, 2);
    ready_juggling(&mut s);
    let shuffle = s.run_rng.shuffle.counter();
    let _ = apply(&mut s, Action::EndTurn);
    assert_eq!(*BODIES.lock().unwrap(), vec![(first, Some(e), false)]);
    assert_eq!(
        s.run_rng.shuffle.counter() - shuffle,
        2,
        "both caller attempts still pick"
    );
    assert_eq!(s.combat_log.player_card_plays_this_combat, 1);
    assert_eq!(
        s.cards.len(),
        2,
        "first callback cloned the original exactly once"
    );
    assert_ne!(s.cards[1].uid, first);
}

#[test]
fn already_started_repeat_is_not_a_new_autoplay_command() {
    let (mut s, e) = board(1);
    observe_strikes();
    let first = add(&mut s, CardId::StrikeIronclad, PileKind::Draw);
    s.cards[0].base_replay_count = 1;
    assert!(auto_play_specific_card(&mut s, first, false));
    assert_eq!(
        *BODIES.lock().unwrap(),
        vec![(first, Some(e), false), (first, Some(e), true)]
    );
    assert_eq!(s.combat_log.player_card_plays_this_combat, 2);
}

#[test]
fn brand_lethal_inferno_returns_empty_selection_and_still_grants_strength() {
    let (mut s, _) = board(1);
    power(&mut s, PowerKind::Inferno, 10);
    add(&mut s, CardId::Brand, PileKind::Hand);
    let keep = add(&mut s, CardId::StrikeIronclad, PileKind::Hand);
    add(&mut s, CardId::DefendIronclad, PileKind::Hand);
    let _ = play_card(&mut s, 0, None);
    assert!(s.pending_choice.is_none());
    assert!(s.player.hand.contains(&keep));
    assert_eq!(s.player.current_hp, 79);
    assert_eq!(s.player_power_amount(PowerKind::Strength), 1);
}

#[test]
fn armaments_base_rejects_singleton_and_prompt_but_upgrade_all_still_runs() {
    for upgraded in [false, true] {
        for n in [1, 2] {
            let (mut s, _) = board(1);
            power(&mut s, PowerKind::Juggernaut, 10);
            add(&mut s, CardId::Armaments, PileKind::Hand);
            if upgraded {
                s.cards[0].upgrade();
            }
            let candidates: Vec<_> = (0..n)
                .map(|_| add(&mut s, CardId::DefendIronclad, PileKind::Hand))
                .collect();
            let _ = play_card(&mut s, 0, None);
            assert!(s.pending_choice.is_none());
            assert!(candidates
                .iter()
                .all(|u| s.cards[u.get() as usize - 1].upgraded == upgraded));
        }
    }
}

fn choice(uid: CardUid, post_effect: Option<PostChoiceEffect>) -> PendingChoice {
    PendingChoice {
        kind: ChoiceKind::ExhaustFromHand,
        options: vec![ChoiceOption {
            card_id: CardId::DefendIronclad,
            upgraded: false,
            card_uid: Some(uid),
            source_pile: Some(PileKind::Hand),
        }],
        min_select: 1,
        max_select: 1,
        can_skip: false,
        post_effect,
        picked: smallvec::SmallVec::new(),
    }
}

#[test]
fn refused_new_hand_admission_preserves_parked_choice_and_unconditional_continuation() {
    let (mut s, _) = board(0);
    let uid = add(&mut s, CardId::DefendIronclad, PileKind::Hand);
    let parked = choice(uid, None);
    s.pending_choice = Some(parked.clone());
    s.open_hand_choice(choice(uid, Some(PostChoiceEffect::GainStrength(2))));
    assert_eq!(s.pending_choice, Some(parked));
    assert_eq!(s.player_power_amount(PowerKind::Strength), 2);
    assert!(s.player.hand.contains(&uid));
}

#[test]
fn live_selection_and_parked_autoplay_resume_are_unchanged_and_clone_independent() {
    let (mut s, _) = board(100);
    add(&mut s, CardId::Armaments, PileKind::Draw);
    add(&mut s, CardId::DefendIronclad, PileKind::Hand);
    add(&mut s, CardId::StrikeIronclad, PileKind::Hand);
    assert!(auto_play_card_from_draw_top(&mut s, false));
    assert!(s.pending_autoplay_resume.is_some());
    let action = legal_actions(&s)
        .into_iter()
        .find(|a| matches!(a,Action::ResolveChoice {selection} if !selection.is_empty()))
        .unwrap();
    let before = s.clone();
    let mut clone = s.clone();
    let _ = apply(&mut clone, action.clone());
    assert_eq!(s, before);
    let _ = apply(&mut s, action);
    assert_eq!(s, clone);
    assert!(s.pending_choice.is_none());
    assert!(s.pending_autoplay_resume.is_none());
}

#[test]
fn history_course_refusal_keeps_registered_dupe_without_play_or_target_rng() {
    let (mut s, _) = board(0);
    s.round = 2;
    let uid = add(&mut s, CardId::StrikeIronclad, PileKind::Discard);
    s.relics.push(Relic {
        id: NonZeroU32::new(1).unwrap(),
        kind: RelicKind::HistoryCourse,
        counter: 0,
        is_melted: false,
        is_wax: false,
    });
    s.combat_log.prev_round_last_attack_skill_played =
        Some((CardId::StrikeIronclad, false, Some(uid)));
    let rng = s.run_rng.clone();
    let _ = sts2_core::relic_effects::dispatch_before_play_phase_start(&mut s);
    assert_eq!(
        s.cards.len(),
        2,
        "CreateDupe argument is evaluated before AutoPlay entry"
    );
    assert_eq!(s.cards[1].combat_var(VarKey::IsDupe), 1);
    assert!(s.player.play.is_empty());
    assert_eq!(s.combat_log.player_card_plays_this_combat, 0);
    assert_eq!(s.run_rng, rng);
}

#[test]
fn singleton_hand_selection_fast_paths_obey_the_same_ending_admission() {
    // A repeat of an already-started series can reach these bodies while
    // ending. The body still runs; FromHand returns no selected cards.
    for id in [
        CardId::ThinkingAhead,
        CardId::HandTrick,
        CardId::Begone,
        CardId::Transfigure,
        CardId::DecisionsDecisions,
    ] {
        let (mut s, _) = board(0);
        let candidate = add(&mut s, CardId::DefendIronclad, PileKind::Hand);
        let playing = add(&mut s, id, PileKind::Play);
        let before = s.cards[candidate.get() as usize - 1];
        let _ = sts2_core::card_effects::dispatch_for_card(&mut s, playing, id, None);
        assert!(s.pending_choice.is_none(), "{id:?}");
        assert!(
            s.player.hand.contains(&candidate),
            "{id:?} must not take a singleton"
        );
        assert_eq!(s.cards[candidate.get() as usize - 1], before, "{id:?}");
        assert!(!s.player.sly_this_turn.contains(&candidate), "{id:?}");
    }
}

#[test]
fn hidden_daggers_refused_selection_preserves_hand_and_runs_generation_tail() {
    for count in [1, 2, 3] {
        let (mut s, _) = board(0);
        let original: Vec<_> = (0..count)
            .map(|_| add(&mut s, CardId::DefendIronclad, PileKind::Hand))
            .collect();
        let playing = add(&mut s, CardId::HiddenDaggers, PileKind::Play);
        let _ = sts2_core::card_effects::dispatch_for_card(
            &mut s,
            playing,
            CardId::HiddenDaggers,
            None,
        );
        assert!(s.pending_choice.is_none());
        assert!(original.iter().all(|uid| s.player.hand.contains(uid)));
        assert_eq!(
            s.cards.len(),
            count + 3,
            "body's unconditional generation remains"
        );
    }
}

#[test]
fn dual_wield_refused_from_hand_does_not_create_a_copy() {
    let (mut s, _) = board(0);
    add(&mut s, CardId::StrikeIronclad, PileKind::Hand);
    let uid = add(&mut s, CardId::DualWield, PileKind::Play);
    let before = s.clone();
    let _ = sts2_core::card_effects::dispatch_for_card(&mut s, uid, CardId::DualWield, None);
    assert_eq!(s, before);
}

#[test]
fn selection_consumers_continue_the_started_walk_without_admitting_after_lethal_boulder() {
    for kind in [
        PowerKind::Entropy,
        PowerKind::Tyranny,
        PowerKind::ToolsOfTheTrade,
        PowerKind::Mayhem,
    ] {
        let (mut s, _) = board(1);
        for _ in 0..8 {
            add(&mut s, CardId::DefendIronclad, PileKind::Draw);
        }
        power(&mut s, PowerKind::RollingBoulder, 10);
        power(&mut s, kind, 1);
        s.phase = CombatPhase::PlayerTurnStart;
        let _ = sts2_core::combat_loop::start_player_turn(&mut s);
        assert!(s.pending_choice.is_none(), "{kind:?}");
        assert!(s.pending_cluster_resume.is_none(), "{kind:?}");
        assert!(s.player.exhaust.is_empty(), "{kind:?}");
        assert!(s.player.discard.is_empty(), "{kind:?}");
        assert_eq!(s.combat_log.player_card_plays_this_combat, 0, "{kind:?}");
        assert_eq!(s.cards.len(), 8, "{kind:?} must not transform");
        assert!(s.cards.iter().all(|c| c.card_id == CardId::DefendIronclad));
    }
}
