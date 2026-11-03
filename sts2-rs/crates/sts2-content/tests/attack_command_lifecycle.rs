//! F6 step30: the player's attack command lifecycle (C# `AttackCommand`).
//!
//! C# `AttackCommand.Execute` (`Commands.Builders/AttackCommand.cs`) admits the
//! command, awaits `Hook.BeforeAttack`, runs every hit (each hit's
//! `CreatureCmd.Damage` results appended to `command.Results`), then awaits
//! `Hook.AfterAttack` before returning to the card's `OnPlay`. So:
//! * `VigorPower` / `GigantificationPower` are spent at the command's
//!   `AfterAttack`, before a selection the card opens after its attack
//!   (Headbutt, Neow's Fury) — `attack-power-before-selection`;
//! * `SkittishPower` grants its block at the command's `AfterAttack`, before the
//!   card's `AfterCardPlayed` callbacks (Kusarigama's hit), on manual and
//!   auto-played attacks alike — `gardener-hp-block`, `havoc-skittish-hit`;
//! * Skittish reads the command's FIRST result for its owner: a blocked first
//!   hit grants nothing even when a later hit lands.
//!
//! These play real cards through `combat_loop::play_card` and the auto-play
//! resolver, and use only state that exists with or without the lifecycle (so
//! they also ran fail-first against the card-tail implementation).

use smallvec::SmallVec;
use std::num::NonZeroU32;

use sts2_content::cards::dispatch as content_dispatch;
use sts2_core::cards::{Card, CardId, CardType, TargetType};
use sts2_core::{
    CombatPhase, CombatState, Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind, Power,
    PowerKind, PowerUid, Relic, RelicKind, Side, StepResult,
};

fn card(
    state: &mut CombatState,
    id: CardId,
    ty: CardType,
    tt: TargetType,
    dmg: i32,
) -> sts2_core::CardUid {
    let raw = u32::try_from(state.cards.len() + 1).expect("u32");
    let uid = sts2_core::CardUid::new(raw).expect("nz");
    state.cards.push(Card {
        uid,
        card_id: id,
        cost: 0,
        card_type: ty,
        target_type: tt,
        upgraded: false,
        base_damage: dmg,
        base_block: 0,
        ..Card::pod_defaults()
    });
    uid
}

fn attack(state: &mut CombatState, id: CardId, dmg: i32) -> sts2_core::CardUid {
    card(state, id, CardType::Attack, TargetType::Enemy, dmg)
}

fn enemy(state: &mut CombatState, raw: u32, hp: i32, block: i32, skittish: Option<u32>) {
    let eid = NonZeroU32::new(raw).expect("nz");
    let mut epowers: SmallVec<[PowerUid; 8]> = SmallVec::new();
    if let Some(pid) = skittish {
        let pid = NonZeroU32::new(pid).expect("nz");
        state.powers.push(Power {
            id: pid,
            kind: PowerKind::Skittish,
            amount: 6,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        epowers.push(PowerUid(pid));
    }
    state.creatures.push(Creature {
        id: eid,
        current_hp: hp,
        max_hp: hp,
        block,
        powers: epowers,
        species: Some(if skittish.is_some() {
            MonsterKind::PhantasmalGardener
        } else {
            MonsterKind::Nibbit
        }),
        intent: None,
        intent_history: IntentHistory::default(),
        monster_id: MonsterId(if skittish.is_some() {
            "PhantasmalGardener"
        } else {
            "Nibbit"
        }),
        ai: None,
    });
    state.enemy_uids.push(CreatureUid(eid));
}

/// The player's creature is `creatures[0]` (created first); enemies follow.
fn base_state() -> CombatState {
    content_dispatch::ensure_installed();
    sts2_content::relics::dispatch::ensure_installed();
    let mut state = CombatState::default();
    state.player.current_hp = 80;
    state.player.max_hp = 80;
    state.player.energy = 99;
    state.player.max_energy = 99;
    let _ = state.init_player_creature();
    state.phase = CombatPhase::PlayerPlayPhase;
    state.side = Side::Player;
    state
}

fn player_power(state: &mut CombatState, raw: u32, kind: PowerKind, amount: i32) {
    let pid = NonZeroU32::new(raw).expect("nz");
    state.powers.push(Power {
        id: pid,
        kind,
        amount,
        counter: 0,
        applier: None,
        skip_next_tick: false,
    });
    state.player.powers.push(PowerUid(pid));
}

fn player_power_amount(state: &CombatState, kind: PowerKind) -> Option<i32> {
    state
        .player_power_uids()
        .into_iter()
        .find_map(|u| state.powers.get_by_id(u.0).filter(|p| p.kind == kind))
        .map(|p| p.amount)
}

fn kusarigama(state: &mut CombatState, counter: i32) {
    let mut r = Relic::new(NonZeroU32::new(900).expect("nz"), RelicKind::Kusarigama);
    r.counter = counter;
    state.relics.push(r);
}

fn skittish_counter(state: &CombatState, pid: u32) -> i32 {
    state
        .powers
        .iter()
        .find(|p| p.id.get() == pid)
        .map_or(-1, |p| p.counter)
}

fn two_discards(state: &mut CombatState) {
    let a = attack(state, CardId::StrikeIronclad, 6);
    let b = card(
        state,
        CardId::DefendIronclad,
        CardType::Skill,
        TargetType::Self_,
        0,
    );
    state.player.discard.push(a);
    state.player.discard.push(b);
}

fn resolve_first(state: &mut CombatState) {
    let _ = sts2_core::apply(
        state,
        sts2_core::Action::ResolveChoice {
            selection: smallvec::smallvec![0],
        },
    );
}

// ── attack-power-before-selection ─────────────────────────────────────────

/// DFJJVF4EEL@1303 / S02XBX5L88@1126 shape: Headbutt with Vigor 8 deals 9 + 8,
/// and its command's `AfterAttack` has spent Vigor by the time the discard pick
/// opens (Headless shows no Vigor there; the card-tail engine kept 8).
#[test]
fn headbutt_spends_vigor_before_its_selection() {
    let mut state = base_state();
    enemy(&mut state, 10, 200, 0, None);
    player_power(&mut state, 60, PowerKind::Vigor, 8);
    two_discards(&mut state);
    let hb = attack(&mut state, CardId::Headbutt, 9);
    state.player.hand.push(hb);
    let step = sts2_core::combat_loop::play_card(&mut state, 0, Some(0));
    assert_eq!(
        step,
        StepResult::PendingChoice,
        "Headbutt asks for a discard card"
    );
    assert_eq!(
        state.creatures[1].current_hp,
        200 - 17,
        "Vigor boosted the hit"
    );
    assert_eq!(
        player_power_amount(&state, PowerKind::Vigor),
        None,
        "Vigor is spent at the command's AfterAttack, before the pick"
    );
    resolve_first(&mut state);
    assert_eq!(player_power_amount(&state, PowerKind::Vigor), None);
    assert_eq!(
        state.creatures[1].current_hp,
        200 - 17,
        "no second spend or hit"
    );
}

/// VU45NWWFP8 replay@65 shape: Neow's Fury under Gigantification 1 triples its
/// hit and has decremented the power away before its discard pick.
#[test]
fn neows_fury_spends_gigantification_before_its_selection() {
    let mut state = base_state();
    enemy(&mut state, 10, 200, 0, None);
    player_power(&mut state, 61, PowerKind::Gigantification, 1);
    two_discards(&mut state);
    let nf = attack(&mut state, CardId::NeowsFury, 10);
    state.player.hand.push(nf);
    let step = sts2_core::combat_loop::play_card(&mut state, 0, Some(0));
    assert_eq!(
        step,
        StepResult::PendingChoice,
        "Neow's Fury asks for discard cards"
    );
    assert_eq!(
        state.creatures[1].current_hp,
        200 - 30,
        "×3 on the command's hit"
    );
    assert_eq!(
        player_power_amount(&state, PowerKind::Gigantification),
        None,
        "Gigantification is decremented at the command's AfterAttack, before the pick"
    );
}

/// The auto-play pending tail: Havoc auto-plays Headbutt from the draw pile;
/// the auto-played Headbutt's own command spends Vigor before its pick
/// surfaces through the auto-play path.
#[test]
fn autoplayed_headbutt_spends_vigor_before_its_selection() {
    let mut state = base_state();
    enemy(&mut state, 10, 200, 0, None);
    player_power(&mut state, 60, PowerKind::Vigor, 8);
    two_discards(&mut state);
    let hb = attack(&mut state, CardId::Headbutt, 9);
    state.player.draw.push(hb);
    let havoc = card(
        &mut state,
        CardId::Havoc,
        CardType::Skill,
        TargetType::Self_,
        0,
    );
    state.player.hand.push(havoc);
    let step = sts2_core::combat_loop::play_card(&mut state, 0, None);
    assert_eq!(
        step,
        StepResult::PendingChoice,
        "the auto-played Headbutt asks"
    );
    assert_eq!(state.creatures[1].current_hp, 200 - 17);
    assert_eq!(
        player_power_amount(&state, PowerKind::Vigor),
        None,
        "the auto-played command spent Vigor before its pick"
    );
}

/// Control: Vigor reaches every hit of ONE command (Twin Strike's two hits).
#[test]
fn vigor_reaches_every_hit_of_one_command() {
    let mut state = base_state();
    enemy(&mut state, 10, 200, 0, None);
    player_power(&mut state, 60, PowerKind::Vigor, 3);
    let ts = attack(&mut state, CardId::TwinStrike, 5);
    state.player.hand.push(ts);
    assert_ne!(
        sts2_core::combat_loop::play_card(&mut state, 0, Some(0)),
        StepResult::PendingChoice
    );
    assert_eq!(state.creatures[1].current_hp, 200 - 16);
    assert_eq!(player_power_amount(&state, PowerKind::Vigor), None);
}

/// Control: a later command gets no Vigor once it is spent; a Vigor applied
/// afterwards is a fresh instance that latches the next command.
#[test]
fn vigor_spent_then_reapplied_latches_afresh() {
    let mut state = base_state();
    enemy(&mut state, 10, 200, 0, None);
    player_power(&mut state, 60, PowerKind::Vigor, 8);
    let s1 = attack(&mut state, CardId::StrikeIronclad, 6);
    let s2 = attack(&mut state, CardId::StrikeIronclad, 6);
    state.player.hand.push(s1);
    state.player.hand.push(s2);
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, Some(0));
    assert_eq!(state.creatures[1].current_hp, 200 - 14);
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, Some(0));
    assert_eq!(state.creatures[1].current_hp, 200 - 20, "no Vigor left");
    player_power(&mut state, 62, PowerKind::Vigor, 4);
    let s3 = attack(&mut state, CardId::StrikeIronclad, 6);
    state.player.hand.push(s3);
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, Some(0));
    assert_eq!(
        state.creatures[1].current_hp,
        200 - 30,
        "the fresh Vigor boosts"
    );
    assert_eq!(player_power_amount(&state, PowerKind::Vigor), None);
}

/// Control: an admitted command with zero hits (Whirlwind, X = 0) still runs
/// BeforeAttack/AfterAttack and spends Vigor.
#[test]
fn zero_hit_command_still_spends_vigor() {
    let mut state = base_state();
    enemy(&mut state, 10, 200, 0, None);
    player_power(&mut state, 60, PowerKind::Vigor, 8);
    state.player.energy = 0;
    let ww = card(
        &mut state,
        CardId::Whirlwind,
        CardType::Attack,
        TargetType::AllEnemies,
        5,
    );
    state.player.hand.push(ww);
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, None);
    assert_eq!(state.creatures[1].current_hp, 200, "zero hits");
    assert_eq!(player_power_amount(&state, PowerKind::Vigor), None);
}

/// Control: lethal — the command's hit ends combat; nothing is left open and
/// the play finishes.
#[test]
fn lethal_command_finishes_cleanly() {
    let mut state = base_state();
    enemy(&mut state, 10, 10, 0, None);
    player_power(&mut state, 60, PowerKind::Vigor, 8);
    let s1 = attack(&mut state, CardId::StrikeIronclad, 6);
    state.player.hand.push(s1);
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, Some(0));
    assert!(state.creatures[1].current_hp <= 0);
    assert!(sts2_core::combat_loop::combat_is_ending(&state));
}

/// Control: a clone taken at Headbutt's open pick resolves identically.
#[test]
fn clone_at_the_selection_resolves_identically() {
    let mut state = base_state();
    enemy(&mut state, 10, 200, 0, Some(50));
    player_power(&mut state, 60, PowerKind::Vigor, 8);
    two_discards(&mut state);
    let hb = attack(&mut state, CardId::Headbutt, 9);
    state.player.hand.push(hb);
    assert_eq!(
        sts2_core::combat_loop::play_card(&mut state, 0, Some(0)),
        StepResult::PendingChoice
    );
    let mut twin = state.clone();
    resolve_first(&mut state);
    resolve_first(&mut twin);
    assert_eq!(state, twin);
}

// ── gardener-hp-block / havoc-skittish-hit ────────────────────────────────

/// BZC1RDELNH@265 shape: Breakthrough (all enemies) lands unblocked on the
/// Gardener as the third attack of the turn. Skittish grants 6 at the command's
/// AfterAttack; Kusarigama's AfterCardPlayed 6 then hits that block (Headless
/// HP 20 block 0; the card-tail engine took 6 HP and granted after).
#[test]
fn manual_attack_skittish_block_precedes_kusarigama() {
    let mut state = base_state();
    enemy(&mut state, 10, 200, 0, Some(50));
    kusarigama(&mut state, 2);
    let bt = card(
        &mut state,
        CardId::Breakthrough,
        CardType::Attack,
        TargetType::AllEnemies,
        9,
    );
    state.player.hand.push(bt);
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, None);
    assert_eq!(
        state.creatures[1].current_hp,
        200 - 9,
        "Kusarigama's 6 was blocked"
    );
    assert_eq!(
        state.creatures[1].block, 0,
        "the fresh 6 block took Kusarigama's hit"
    );
    assert_eq!(skittish_counter(&state, 50), 1);
}

/// A84V6MRJPR@173 shape: Havoc auto-plays Strike; the auto-played Strike's
/// command grants Skittish before its AfterCardPlayed Kusarigama hit.
#[test]
fn autoplayed_attack_skittish_block_precedes_kusarigama() {
    let mut state = base_state();
    enemy(&mut state, 10, 200, 0, Some(50));
    kusarigama(&mut state, 2);
    let strike = attack(&mut state, CardId::StrikeIronclad, 6);
    state.player.draw.push(strike);
    let havoc = card(
        &mut state,
        CardId::Havoc,
        CardType::Skill,
        TargetType::Self_,
        0,
    );
    state.player.hand.push(havoc);
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, None);
    assert_eq!(
        state.creatures[1].current_hp,
        200 - 6,
        "Kusarigama's 6 was blocked"
    );
    assert_eq!(state.creatures[1].block, 0);
    assert_eq!(skittish_counter(&state, 50), 1);
}

/// `command.Results...FirstOrDefault(r => r.Receiver == Owner)`: the FIRST
/// result for the owner decides. Twin Strike 5+5 into 5 block: the first hit is
/// fully blocked (unblocked 0), so no block is granted although the second hit
/// lands.
#[test]
fn skittish_reads_the_first_result_for_its_owner() {
    let mut state = base_state();
    enemy(&mut state, 10, 200, 5, Some(50));
    let ts = attack(&mut state, CardId::TwinStrike, 5);
    state.player.hand.push(ts);
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, Some(0));
    assert_eq!(state.creatures[1].current_hp, 200 - 5);
    assert_eq!(
        state.creatures[1].block, 0,
        "a blocked first result grants nothing"
    );
    assert_eq!(skittish_counter(&state, 50), 0);
}

/// Control (multi-target): each Gardener reads its own first result in the one
/// all-enemies command: the blocked one grants nothing, the hit one grants 6.
#[test]
fn multi_target_command_each_owner_reads_its_own_result() {
    let mut state = base_state();
    enemy(&mut state, 10, 200, 20, Some(50));
    enemy(&mut state, 11, 200, 0, Some(51));
    let bt = card(
        &mut state,
        CardId::Breakthrough,
        CardType::Attack,
        TargetType::AllEnemies,
        9,
    );
    state.player.hand.push(bt);
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, None);
    assert_eq!(state.creatures[1].block, 11, "blocked: no grant");
    assert_eq!(skittish_counter(&state, 50), 0);
    assert_eq!(state.creatures[2].current_hp, 200 - 9);
    assert_eq!(
        state.creatures[2].block, 6,
        "hit: granted after the command"
    );
    assert_eq!(skittish_counter(&state, 51), 1);
}

/// Nested: Uproar's two hits are one command, which completes (Skittish grants
/// 6) before its `OnPlay` auto-plays an Attack from the draw pile; the nested
/// Strike is its own command and its 6 lands on that fresh block.
#[test]
fn nested_autoplay_after_the_command_sees_its_after_attack() {
    let mut state = base_state();
    enemy(&mut state, 10, 200, 0, Some(50));
    let strike = attack(&mut state, CardId::StrikeIronclad, 6);
    state.player.draw.push(strike);
    let up = attack(&mut state, CardId::Uproar, 5);
    state.player.hand.push(up);
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, Some(0));
    assert_eq!(
        state.creatures[1].current_hp,
        200 - 10,
        "the nested Strike was blocked"
    );
    assert_eq!(state.creatures[1].block, 0);
    assert_eq!(skittish_counter(&state, 50), 1);
}

/// Nested control with Vigor: the outer command spends Vigor at its
/// AfterAttack; the auto-played Strike gets none.
#[test]
fn nested_autoplay_gets_no_vigor_from_the_outer_command() {
    let mut state = base_state();
    enemy(&mut state, 10, 200, 0, None);
    player_power(&mut state, 60, PowerKind::Vigor, 8);
    let strike = attack(&mut state, CardId::StrikeIronclad, 6);
    state.player.draw.push(strike);
    let up = attack(&mut state, CardId::Uproar, 5);
    state.player.hand.push(up);
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, Some(0));
    assert_eq!(state.creatures[1].current_hp, 200 - 26 - 6);
    assert_eq!(player_power_amount(&state, PowerKind::Vigor), None);
}
