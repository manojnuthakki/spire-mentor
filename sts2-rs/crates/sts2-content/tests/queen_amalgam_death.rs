//! Step48 (`queen-amalgam-death`): C# runs `Hook.AfterDeath` at the kill (`Creature` death in
//! `CreatureCmd.Kill`), and `Queen.AfterDeath` calls `SetMoveImmediate(EnragedState)` when her
//! `NextMove` is Burn Bright For Me. The engine delivered monster death callbacks only inside the
//! enemies' move walk and queued the override for the next roll.
use sts2_content::cards::spec_index::card_pod_from_id;
use sts2_content::monster_dispatch::{build_ai, MonsterSpawnArgs, SpawnPosition};
use sts2_core::cards::CardId;
use sts2_core::combat_loop::play_card;
use sts2_core::{
    apply, Action, CardUid, CombatPhase, CombatState, CreatureUid, MonsterKind, PowerKind,
};

const BURN_BRIGHT: &str = "BURN_BRIGHT_FOR_ME_MOVE";

fn board(amalgam_hp: i32) -> (CombatState, CreatureUid, CreatureUid) {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut s = CombatState::default();
    s.player.current_hp = 400;
    s.player.max_hp = 400;
    s.player.energy = 3;
    s.init_player_creature();
    let amalgam = s.spawn_enemy(MonsterKind::TorchHeadAmalgam);
    let queen = s.spawn_enemy(MonsterKind::Queen);
    for (uid, kind, hp) in [
        (amalgam, MonsterKind::TorchHeadAmalgam, amalgam_hp),
        (queen, MonsterKind::Queen, 400),
    ] {
        let c = s.creatures.iter_mut().find(|c| c.id == uid.0).unwrap();
        c.current_hp = hp;
        c.max_hp = hp.max(400);
        c.ai = Some(build_ai(
            kind,
            MonsterSpawnArgs::None,
            SpawnPosition::default(),
        ));
    }
    s.creatures
        .iter_mut()
        .find(|c| c.id == queen.0)
        .unwrap()
        .ai
        .as_mut()
        .unwrap()
        .force_current_state_by_name(BURN_BRIGHT)
        .then_some(())
        .expect("Queen has Burn Bright");
    let q = s.creatures.iter_mut().find(|c| c.id == queen.0).unwrap();
    q.intent =
        q.ai.as_ref()
            .unwrap()
            .current_move()
            .intents
            .first()
            .copied();
    s.phase = CombatPhase::PlayerPlayPhase;
    (s, amalgam, queen)
}

fn queen_state(s: &CombatState, queen: CreatureUid) -> (&'static str, i32) {
    let c = s.creatures.iter().find(|c| c.id == queen.0).unwrap();
    let ai = c.ai.as_ref().unwrap();
    (ai.current_state_id(), ai.counter())
}

#[test]
fn a_player_kill_of_the_amalgam_sets_the_queens_move_at_once() {
    let (mut s, _amalgam, queen) = board(3);
    let mut c = card_pod_from_id(CardId::StrikeIronclad, false).unwrap();
    c.uid = CardUid::new(1).unwrap();
    s.cards.push(c);
    s.player.hand.push(c.uid);
    assert_eq!(queen_state(&s, queen), (BURN_BRIGHT, 0));
    // enemy index 0 is the Amalgam (spawned first)
    let _ = play_card(&mut s, 0, Some(0));
    assert_eq!(
        queen_state(&s, queen),
        ("ENRAGE_MOVE", 1),
        "SetMoveImmediate(EnragedState) runs at the kill, in the player's turn"
    );
    assert!(
        s.combat_log.newly_dead_this_turn.is_empty(),
        "delivered once at the kill; the enemy walk finds nothing left to deliver"
    );
}

#[test]
fn a_demise_kill_after_her_move_skips_enrage_in_the_roll_that_follows() {
    // Demise ticks at the owner's (enemy) turn end, after the Queen performed Burn Bright and
    // before the roll. Native: SetMoveImmediate(Enraged) then the roll continues from Enraged,
    // so the next turn shows Off With Your Head (checked against the game's DLL).
    let (mut s, amalgam, queen) = board(5);
    sts2_core::card_effects::apply_power_to_creature(&mut s, amalgam, PowerKind::Demise, 9);
    let _ = apply(&mut s, Action::EndTurn);
    assert!(
        s.creatures
            .iter()
            .find(|c| c.id == amalgam.0)
            .unwrap()
            .current_hp
            <= 0
    );
    let q = s.creatures.iter().find(|c| c.id == queen.0).unwrap();
    assert_eq!(q.block, 20, "she performed Burn Bright this turn");
    assert_eq!(queen_state(&s, queen), ("OFF_WITH_YOUR_HEAD_MOVE", 1));
    assert_eq!(
        q.intent,
        q.ai.as_ref()
            .unwrap()
            .current_move()
            .intents
            .first()
            .copied()
    );
}

#[test]
fn a_kill_inside_the_enemy_walk_after_her_move_sets_enrage_then_the_roll_follows_it() {
    let (mut s, _amalgam, queen) = board(5);
    let q = s.creatures.iter_mut().find(|c| c.id == queen.0).unwrap();
    q.ai.as_mut().unwrap().on_move_started(); // Burn Bright on show was already performed
    let amalgam_uid = s.enemy_uids[0];
    sts2_core::damage::apply_damage_full(
        &mut s,
        sts2_core::hooks::CreatureRef::Player,
        amalgam_uid,
        50,
        sts2_core::hooks::ValueProps::UNPOWERED,
        None,
    );
    let c = s.creatures.iter_mut().find(|c| c.id == queen.0).unwrap();
    let ai = c.ai.as_mut().unwrap();
    assert_eq!(ai.current_state_id(), "ENRAGE_MOVE");
    assert_eq!(ai.forced_next_state(), None);
    let _ = ai.roll_move_seeded(1);
    assert_eq!(ai.current_state_id(), "OFF_WITH_YOUR_HEAD_MOVE");
}

static DELIVERIES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn count_delivery(_: &mut CombatState, _dead: CreatureUid, _owner: CreatureUid) {
    DELIVERIES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

#[test]
fn a_lethal_hit_delivers_the_death_to_listeners_exactly_once() {
    use std::sync::atomic::Ordering::SeqCst;
    let (mut s, amalgam, queen) = board(5);
    s.creatures
        .iter_mut()
        .find(|c| c.id == queen.0)
        .unwrap()
        .ai
        .as_mut()
        .unwrap()
        .after_death = Some(count_delivery);
    DELIVERIES.store(0, SeqCst);
    let hit = |s: &mut CombatState, n: i32| {
        sts2_core::damage::apply_damage_full(
            s,
            sts2_core::hooks::CreatureRef::Player,
            amalgam,
            n,
            sts2_core::hooks::ValueProps::UNPOWERED,
            None,
        )
    };
    let _ = hit(&mut s, 2);
    assert_eq!(
        DELIVERIES.load(SeqCst),
        0,
        "a non-lethal hit delivers nothing"
    );
    let _ = hit(&mut s, 50);
    assert_eq!(
        DELIVERIES.load(SeqCst),
        1,
        "the lethal hit delivers at the kill"
    );
    let _ = hit(&mut s, 5);
    assert_eq!(
        DELIVERIES.load(SeqCst),
        1,
        "a posthumous hit does not deliver again"
    );
}
