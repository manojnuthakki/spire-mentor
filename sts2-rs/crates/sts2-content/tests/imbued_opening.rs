//! F6 step34, the Imbued opening lifecycle (fresh DLL authority, sha 08faee0c...):
//!
//! * `CombatManager.SetupPlayerTurn` at turn 1: every draw card whose enchantment
//!   `ShouldStartAtBottomOfDrawPile` (Imbued) goes to the bottom, in pile order,
//!   the rest keeping theirs; the Innate list is taken excluding them.
//! * `Imbued.AfterAutoPrePlayPhaseEntered` is a NORMAL AutoPrePlay listener. The
//!   pass's listener snapshot is taken when it begins: after `SetupPlayerTurn`
//!   (Gambling Chip's awaited mulligan included), `AfterSideTurnStart` and the
//!   Early pass, and powers (Mayhem) come before cards. Each captured card is
//!   played by reference and awaited, nested selects included.
//!
//! (`imbued-bottom`, `A6c26`.)

use smallvec::SmallVec;

use sts2_content::monster_dispatch::{build_ai, MonsterSpawnArgs, SpawnPosition};
use sts2_core::cards::{Card, CardId, CardType, EnchantmentId, TargetType};
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::{
    apply, legal_actions, Action, CardUid, ChoiceKind, CombatState, Creature, CreatureUid,
    IntentHistory, MonsterId, MonsterKind, PlayerCombatState, PowerKind, Relic, RelicKind,
    StepResult,
};

/// A fat lone enemy: no auto-played attack may end the combat (an ending combat
/// suppresses the select entirely — C# `CardSelectCmd` returns empty when
/// `IsEnding` — which would defeat the point of these tests).
fn lone_fat_enemy() -> (Vec<Creature>, CreatureUid) {
    let enemy_uid = CreatureUid::new(1).expect("non-zero");
    let creatures = vec![Creature {
        id: enemy_uid.0,
        current_hp: 400,
        max_hp: 400,
        block: 0,
        powers: SmallVec::new(),
        species: Some(MonsterKind::ShrinkerBeetle),
        monster_id: MonsterId(MonsterKind::ShrinkerBeetle.python_name()),
        intent: None,
        intent_history: IntentHistory::default(),
        ai: Some(build_ai(
            MonsterKind::ShrinkerBeetle,
            MonsterSpawnArgs::None,
            SpawnPosition {
                alone: true,
                front: true,
                back: true,
                index: 0,
            },
        )),
    }];
    (creatures, enemy_uid)
}

fn push_card(cards: &mut sts2_core::CardStore, card_id: CardId, card_type: CardType) -> CardUid {
    let uid = CardUid::new(u32::try_from(cards.len() + 1).expect("fits")).expect("non-zero");
    cards.push(Card {
        uid,
        card_id,
        cost: 1,
        card_type,
        target_type: TargetType::Self_,
        upgraded: false,
        base_damage: 0,
        base_block: 5,
        ..Card::pod_defaults()
    });
    uid
}

fn push(cards: &mut sts2_core::CardStore, id: CardId, imbued: bool) -> CardUid {
    let uid = push_card(cards, id, CardType::Skill);
    if imbued {
        cards[uid.get() as usize - 1].set_enchantment(EnchantmentId::Imbued, 1);
    }
    uid
}

/// A turn-1 board whose draw pile is exactly `deck` (`imbued` marks the Imbued
/// cards), set after `start_combat`'s shuffle and before the turn setup draws.
fn board(deck: &[(CardId, bool)], relics: &[RelicKind]) -> (CombatState, Vec<CardUid>) {
    sts2_content::cards::dispatch::ensure_installed();
    sts2_content::relics::dispatch::ensure_installed();
    let mut cards = sts2_core::CardStore::new();
    let uids: Vec<CardUid> = deck
        .iter()
        .map(|&(id, im)| push(&mut cards, id, im))
        .collect();
    let (creatures, enemy_uid) = lone_fat_enemy();
    let player = PlayerCombatState {
        max_energy: 3,
        current_hp: 400,
        max_hp: 400,
        character: Some(sts2_core::potions::CharacterId::Ironclad),
        ..PlayerCombatState::default()
    };
    let relics: Vec<Relic> = relics
        .iter()
        .enumerate()
        .map(|(i, &kind)| Relic {
            id: std::num::NonZeroU32::new(u32::try_from(i + 1).expect("fits")).expect("nz"),
            kind,
            counter: kind.initial_counter(),
            is_wax: false,
            is_melted: false,
        })
        .collect();
    let mut state = CombatState {
        rng: sts2_core::dotnet_rng::Rng::new(7),
        run_rng: sts2_core::run_rng_set::RunRngSet::new(7),
        creatures,
        cards,
        relics: relics.into(),
        player,
        enemy_uids: SmallVec::from_slice(&[enemy_uid]),
        ..CombatState::default()
    };
    let _ = start_combat(&mut state);
    state.player.hand.clear();
    state.player.discard.clear();
    state.player.exhaust.clear();
    state.player.draw = uids.iter().copied().collect();
    (state, uids)
}

fn answer_choice(state: &mut CombatState) -> StepResult {
    let pick = legal_actions(state)
        .into_iter()
        .find_map(|a| match a {
            Action::ResolveChoice { selection } => Some(selection),
            _ => None,
        })
        .expect("a ResolveChoice is legal while a select is surfaced");
    apply(state, Action::ResolveChoice { selection: pick })
}

const D: CardId = CardId::DefendIronclad;

#[test]
fn imbued_moves_to_the_bottom_and_the_rest_keep_their_order() {
    // Imbued Defend fourth in a ten-card pile. The bottom move closes the pile
    // up (d0 d1 d2 d4 d5 ... d9 I), so the hand is d0 d1 d2 d4 d5, the Imbued
    // card then plays from the bottom and the draw pile is d6 d7 d8 d9.
    let mut deck = vec![(D, false); 10];
    deck[3].1 = true;
    let (mut state, u) = board(&deck, &[]);
    let res = start_player_turn(&mut state);
    assert_eq!(res, StepResult::Advanced);
    assert_eq!(
        state.player.hand.as_slice(),
        &[u[0], u[1], u[2], u[4], u[5]],
        "the Imbued card is not drawn and the others keep their order"
    );
    assert_eq!(state.player.draw.as_slice(), &[u[6], u[7], u[8], u[9]]);
    assert_eq!(state.player.discard.as_slice(), &[u[3]], "Imbued played");
    assert!(state.player.play.is_empty());
}

#[test]
fn imbued_cards_keep_their_relative_order_at_the_bottom() {
    // Two Imbued cards: both go to the bottom in pile order, the first plays
    // first (it reaches the discard first).
    let mut deck = vec![(D, false); 10];
    deck[2].1 = true;
    deck[6].1 = true;
    let (mut state, u) = board(&deck, &[]);
    assert_eq!(start_player_turn(&mut state), StepResult::Advanced);
    assert_eq!(
        state.player.hand.as_slice(),
        &[u[0], u[1], u[3], u[4], u[5]]
    );
    assert_eq!(state.player.draw.as_slice(), &[u[7], u[8], u[9]]);
    assert_eq!(state.player.discard.as_slice(), &[u[2], u[6]]);
}

#[test]
fn imbued_waits_for_gambling_chip() {
    // `Hook.AfterPlayerTurnStart` awaits Gambling Chip's mulligan; the Normal
    // AutoPrePlay pass starts after the setup task, so the Imbued card is still
    // at the bottom while the screen is open.
    let mut deck = vec![(D, false); 12];
    deck[11].1 = true;
    let (mut state, u) = board(&deck, &[RelicKind::GamblingChip]);
    let res = start_player_turn(&mut state);
    assert_eq!(res, StepResult::PendingChoice);
    assert_eq!(
        state.pending_choice.as_ref().map(|c| c.kind),
        Some(ChoiceKind::DiscardFromHandAndDraw)
    );
    assert_eq!(
        state.player.draw.last(),
        Some(&u[11]),
        "Imbued still at the bottom"
    );
    assert!(
        state.player.discard.is_empty(),
        "nothing played before the pick"
    );
    let _ = answer_choice(&mut state);
    assert!(state.pending_choice.is_none());
    assert!(
        state.player.discard.contains(&u[11]),
        "Imbued played after the pick"
    );
    assert!(state.pending_cluster_resume.is_none());
}

#[test]
fn imbued_awaits_a_nested_select_before_its_sibling() {
    // Two Imbued Armaments. The first opens its upgrade select; the second,
    // captured by the same pass, waits in the draw pile until it is answered.
    let mut deck = vec![(D, false); 7];
    deck.push((CardId::Armaments, true));
    deck.push((CardId::Armaments, true));
    let (mut state, u) = board(&deck, &[]);
    let res = start_player_turn(&mut state);
    assert_eq!(res, StepResult::PendingChoice);
    assert!(state.pending_choice.is_some());
    assert!(
        state.player.draw.contains(&u[8]),
        "the sibling is not played past the open select: {:?}",
        state.player.draw
    );
    assert_eq!(
        state
            .pending_cluster_resume
            .as_ref()
            .map(|r| r.imbued.as_slice()),
        Some([u[8]].as_slice()),
        "the unplayed listener rides the turn-start record"
    );
    // Answering the first select must NOT answer the sibling's: its own upgrade
    // select surfaces as the next decision (Armaments awaits its selection).
    let res = answer_choice(&mut state);
    assert_eq!(
        res,
        StepResult::PendingChoice,
        "the sibling's select surfaces"
    );
    assert!(state.pending_choice.is_some());
    let upgraded = |s: &CombatState| s.cards.iter().filter(|c| c.upgraded).count();
    assert_eq!(upgraded(&state), 1, "only the first pick has landed");
    let _ = answer_choice(&mut state);
    assert!(state.pending_choice.is_none());
    assert_eq!(upgraded(&state), 2);
    assert!(state.player.play.is_empty(), "{:?}", state.player.play);
    assert!(state.player.discard.contains(&u[7]) && state.player.discard.contains(&u[8]));
    assert!(state.pending_autoplay_resume.is_none() && state.pending_cluster_resume.is_none());
}

#[test]
fn imbued_plays_after_mayhems_batch() {
    // Mayhem is a power, so its listener precedes every card's. It plays the
    // draw top (an Armaments, opening a select); the captured Imbued Defend
    // waits for it and plays once the select is answered.
    let mut deck = vec![(D, false); 5];
    deck.push((CardId::Armaments, false));
    deck.extend(vec![(D, false); 3]);
    deck.push((D, true));
    let (mut state, u) = board(&deck, &[]);
    let puid = state.player.creature_uid.expect("player creature mirror");
    sts2_core::card_effects::apply_power_to_creature(&mut state, puid, PowerKind::Mayhem, 1);
    let res = start_player_turn(&mut state);
    assert_eq!(res, StepResult::PendingChoice);
    assert!(
        state.player.draw.contains(&u[9]),
        "Imbued has not played while Mayhem's card waits on its select"
    );
    assert_eq!(
        state
            .pending_cluster_resume
            .as_ref()
            .map(|r| r.imbued.as_slice()),
        Some([u[9]].as_slice()),
        "the captured card listener is carried across the suspended Mayhem"
    );
    let _ = answer_choice(&mut state);
    while state.pending_choice.is_some() {
        let _ = answer_choice(&mut state);
    }
    assert!(
        state.player.discard.contains(&u[9]),
        "Imbued played after Mayhem"
    );
    assert!(state.pending_cluster_resume.is_none());
}

#[test]
fn imbued_sibling_is_not_moved_after_combat_ends() {
    // Imbued Armaments, Defend, Defend with Juggernaut: the second card's block
    // kills the enemy, so `CardCmd.AutoPlay` refuses the third before anything
    // moves; it stays in the draw pile instead of being stranded in Play.
    let mut deck = vec![(D, false); 5];
    deck.push((CardId::Armaments, true));
    deck.push((D, true));
    deck.push((D, true));
    let (mut state, u) = board(&deck, &[]);
    for c in state.creatures.iter_mut().filter(|c| c.species.is_some()) {
        c.current_hp = 10;
        c.max_hp = 10;
    }
    let puid = state.player.creature_uid.expect("player creature mirror");
    sts2_core::card_effects::apply_power_to_creature(&mut state, puid, PowerKind::Juggernaut, 5);
    assert_eq!(start_player_turn(&mut state), StepResult::PendingChoice);
    let _ = answer_choice(&mut state);
    while state.pending_choice.is_some() {
        let _ = answer_choice(&mut state);
    }
    assert!(state.is_over, "the second Imbued card killed the enemy");
    assert!(
        state.player.play.is_empty(),
        "nothing stranded: {:?}",
        state.player.play
    );
    assert!(
        state.player.draw.contains(&u[7]),
        "the refused card never left the draw pile"
    );
}

#[test]
fn imbued_cascade_keeps_every_nested_select() {
    // An Imbued Cascade auto-plays the top three cards: Armaments, Armaments,
    // Defend. Each Armaments opens its own upgrade select; answering the first
    // must surface the second (not answer it), and only then does the Defend
    // and the later Imbued Defend play.
    let mut deck = vec![(D, false); 5];
    deck.push((CardId::Armaments, false));
    deck.push((CardId::Armaments, false));
    deck.push((D, false));
    deck.push((CardId::Cascade, true));
    deck.push((D, true));
    let (mut state, u) = board(&deck, &[]);
    state.cards[u[8].get() as usize - 1].cost = -1; // X cost: plays three
    assert_eq!(start_player_turn(&mut state), StepResult::PendingChoice);
    let upgraded = |s: &CombatState| s.cards.iter().filter(|c| c.upgraded).count();
    assert_eq!(upgraded(&state), 0);
    assert_eq!(answer_choice(&mut state), StepResult::PendingChoice);
    assert!(
        state.pending_choice.is_some(),
        "the second Armaments' select surfaces"
    );
    assert_eq!(upgraded(&state), 1, "only the first pick has landed");
    assert!(
        !state.player.discard.contains(&u[9]),
        "the last Imbued listener waits for both selects"
    );
    let _ = answer_choice(&mut state);
    assert!(state.pending_choice.is_none());
    assert_eq!(upgraded(&state), 2);
    assert!(
        state.player.discard.contains(&u[9]),
        "the last listener played"
    );
    assert!(state.player.play.is_empty(), "{:?}", state.player.play);
    assert!(state.pending_autoplay_resume.is_none() && state.pending_cluster_resume.is_none());
}

#[test]
fn imbued_cascade_replay_surfaces_the_replays_select() {
    // The first Armaments carries Spiral, so Cascade's auto-play resolves it
    // twice. Each resolution opens its own upgrade select: answering the first
    // must surface the replay's, not answer it (F6 step34, review r3 F1).
    let mut deck = vec![(D, false); 5];
    deck.push((CardId::Armaments, false));
    deck.push((D, false));
    deck.push((D, false));
    deck.push((CardId::Cascade, true));
    deck.push((D, true));
    let (mut state, u) = board(&deck, &[]);
    state.cards[u[8].get() as usize - 1].cost = -1;
    state.cards[u[5].get() as usize - 1].set_enchantment(EnchantmentId::Spiral, 1);
    assert_eq!(start_player_turn(&mut state), StepResult::PendingChoice);
    let upgraded = |s: &CombatState| s.cards.iter().filter(|c| c.upgraded).count();
    assert_eq!(answer_choice(&mut state), StepResult::PendingChoice);
    assert!(
        state.pending_choice.is_some(),
        "the replay's select surfaces"
    );
    assert_eq!(upgraded(&state), 1, "only the first pick has landed");
    assert!(
        !state.player.discard.contains(&u[9]),
        "the later Imbued listener waits for the replay"
    );
    let _ = answer_choice(&mut state);
    assert!(state.pending_choice.is_none());
    assert_eq!(upgraded(&state), 2);
    assert!(state.player.discard.contains(&u[9]));
    assert!(state.player.play.is_empty(), "{:?}", state.player.play);
    assert!(state.pending_autoplay_resume.is_none() && state.pending_cluster_resume.is_none());
}

#[test]
fn imbued_listeners_suspended_draw_completes_before_play_opens() {
    // An Imbued Offering draws three with only the discard to draw from; the
    // shuffle opens Stratagem's select. Answering it must finish the draw (eight
    // cards in hand) before the opening yields (review r3 F5).
    let mut deck = vec![(D, false); 5];
    deck.push((CardId::Offering, true));
    let (mut state, _u) = board(&deck, &[]);
    let puid = state.player.creature_uid.unwrap();
    sts2_core::card_effects::apply_power_to_creature(&mut state, puid, PowerKind::Stratagem, 1);
    for _ in 0..3 {
        let uid = push_card(&mut state.cards, D, CardType::Skill);
        state.player.discard.push(uid);
    }
    assert_eq!(start_player_turn(&mut state), StepResult::PendingChoice);
    assert!(state.pending_draw_resume.is_some(), "the draw is suspended");
    let _ = answer_choice(&mut state);
    assert!(state.pending_choice.is_none());
    assert!(state.pending_draw_resume.is_none(), "the draw completed");
    assert_eq!(state.player.hand.len(), 8);
    assert!(state.pending_autoplay_resume.is_none() && state.pending_cluster_resume.is_none());
}
