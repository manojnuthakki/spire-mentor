//! Integration test for the engine card play-log (`CombatState.combat_log`
//! `TurnEvents { card_play_log, record_card_play_log }`).
//!
//! Drives a real combat through `combat_loop::play_card` (NOT the lower-level
//! `dispatch()`, which bypasses the X-cost energy drain and the play-log
//! recording chokepoints). Cascade is an X-cost card that auto-plays the top X
//! cards of the draw pile (X = energy spent). Playing Cascade at energy 3 must
//! produce, in order: 1 manual Cascade entry (energy_spent=3, auto_played=false)
//! followed by 3 auto-played entries (the 3 cards on top of the draw pile, in
//! draw-top order).
//!
//! Construction mirrors `tests/play_card.rs::build_handful` (the canonical
//! in-crate fresh-combat builder) — that fixture is private to its own test
//! file, so the relevant fields are replicated here. The cards arena assigns
//! `CardUid = index + 1`; the draw pile is seeded directly with the auto-play
//! targets in draw-top order (`draw[0]` plays first).

use smallvec::SmallVec;

use sts2_core::cards::{Card, CardId, CardTags, CardType, TargetType};
use sts2_core::{
    apply, Action, CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory,
    MonsterId, MonsterKind, PlayerCombatState, Side, StepResult, TurnEvents,
};

/// Build one `Card` POD for the arena. `uid_raw` becomes `CardUid(uid_raw)`,
/// matching the `index + 1` convention `build_handful` (tests/play_card.rs)
/// and the content-crate test helpers rely on.
fn make_card(
    uid_raw: u32,
    card_id: CardId,
    card_type: CardType,
    target_type: TargetType,
    cost: i8,
    base_damage: i32,
    base_block: i32,
) -> Card {
    Card {
        uid: CardUid::new(uid_raw).expect("non-zero uid"),
        card_id,
        cost,
        card_type,
        target_type,
        upgraded: false,
        base_damage,
        base_block,
        combat_vars: sts2_core::cards::CombatVars::new(),
        enchantments: sts2_core::cards::CardEnchantments::new(),
        tags: CardTags::NONE,
        energy_spent: 0,
        base_replay_count: 0,
        keyword_override: None,
        affliction: None,
    }
}

#[test]
fn cascade_logs_trigger_then_auto_plays_in_order() {
    // The PlayCard dispatch registry must be populated before any handler can
    // fire. Cascade lives in the sts2-content registry (it routes through
    // `auto_play_n_from_draw_top`), so install the full content registry — the
    // same setup the Whirlwind X-cost integration test in tests/play_card.rs
    // uses.
    sts2_content::cards::dispatch::ensure_installed();

    // Cards arena: index 0 = Cascade (hand), indices 1..=3 = the draw pile in
    // draw-top order (draw[0] = StrikeIronclad plays first).
    let cards_arena: Vec<Card> = vec![
        make_card(
            1,
            CardId::Cascade,
            CardType::Skill,
            TargetType::Self_,
            -1,
            0,
            0,
        ),
        make_card(
            2,
            CardId::StrikeIronclad,
            CardType::Attack,
            TargetType::Enemy,
            1,
            6,
            0,
        ),
        make_card(
            3,
            CardId::StrikeIronclad,
            CardType::Attack,
            TargetType::Enemy,
            1,
            6,
            0,
        ),
        make_card(
            4,
            CardId::DefendIronclad,
            CardType::Skill,
            TargetType::Self_,
            1,
            0,
            5,
        ),
    ];

    let mut hand: SmallVec<[CardUid; 16]> = SmallVec::new();
    hand.push(CardUid::new(1).expect("non-zero")); // Cascade

    // Draw pile in draw-top order: [Strike, Strike, Defend]. `draw[0]` is the
    // first card auto-played by `auto_play_n_from_draw_top`.
    let mut draw: SmallVec<[CardUid; 32]> = SmallVec::new();
    draw.push(CardUid::new(2).expect("non-zero")); // StrikeIronclad
    draw.push(CardUid::new(3).expect("non-zero")); // StrikeIronclad
    draw.push(CardUid::new(4).expect("non-zero")); // DefendIronclad

    let player = PlayerCombatState {
        current_hp: 50,
        max_hp: 50,
        block: 0,
        energy: 3, // X = energy spent = 3 → 3 auto-plays.
        max_energy: 3,
        hand,
        draw,
        discard: SmallVec::new(),
        exhaust: SmallVec::new(),
        play: SmallVec::new(),
        stars: 0,
        powers: SmallVec::new(),
        character: None,
        potions: SmallVec::new(),
        potion_slot_indices: SmallVec::new(),
        potion_slots: 0,
        orbs: sts2_core::OrbSlots::default(),
        creature_uid: None,
        pet_uid: None,
        shivs_played_this_turn: 0,
        cards_played_this_turn: [0; 5],
        powered_hits_this_turn: 0,
        powered_hits_by_target_this_turn: smallvec::SmallVec::new(),
        powered_hits_at_card_play_start: 0,
        attack_command_suppressed_this_play: false,
        vigor_latch_card: None,
        generated_cards_this_combat: 0,
        skills_played_this_turn: 0,
        card_plays_started_this_turn: 0,
        cards_first_in_series_this_turn: 0,
        request_end_turn_after_play: false,
        retain_this_turn: SmallVec::new(),
        sly_this_turn: SmallVec::new(),
        sly_this_combat: SmallVec::new(),
        pending_retain_count: 0,
        stars_gained_this_turn: 0,
        pinned_top: SmallVec::new(),
        pinned_bottom: SmallVec::new(),
        deferred_draw_order: false,
    };

    // A single beefy enemy so the auto-played Strikes have a live target and
    // don't end combat mid-cascade (it must survive 12 damage).
    let enemy_uid = CreatureUid::new(1).expect("non-zero");
    let enemy = Creature {
        id: enemy_uid.0,
        current_hp: 200,
        max_hp: 200,
        block: 0,
        powers: SmallVec::new(),
        species: Some(MonsterKind::Nibbit),
        intent: None,
        intent_history: IntentHistory::default(),
        monster_id: MonsterId("Nibbit"),
        ai: None,
    };

    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    enemy_uids.push(enemy_uid);

    let mut s = CombatState {
        belt_value: None,
        phase: CombatPhase::PlayerPlayPhase,
        round: 1,
        side: Side::Player,
        rng: sts2_core::dotnet_rng::Rng::new(0_i32),
        run_rng: sts2_core::run_rng_set::RunRngSet::new(0_i32),
        creatures: vec![enemy],
        cards: cards_arena.into(),
        powers: Vec::new().into(),
        relics: Vec::new().into(),
        potions: Vec::new(),
        player,
        allies: SmallVec::new(),
        enemy_uids,
        pending_choice: None,
        resolving_card: None,
        pending_play: None,
        pending_turn_setup_resume: false,
        pending_draw_resume: None,
        pending_snecko_cost_randomize: false,
        pending_cluster_resume: None,
        pending_hand_draw_resume: None,
        pending_relic_hand_draw_resume: None,
        pending_wlp_retain_resume: false,
        pending_end_turn_resume: None,
        pending_autoplay_resume: None,
        autoplay_inline_resolve: false,
        pending_curse_choice: None,
        pending_potion_hook_resume: None,
        pending_enemy_exec_resume: None,
        combat_log: TurnEvents::default(),
        swipe_stolen: Vec::new(),
        escaped_creatures: 0,
        heist_recovered_gold: 0,
        room_extra_rewards: Vec::new(),
        deck_card_uids: Vec::new(),
        event_combat_timed_out: false,
        gold: 0,
        is_over: false,
        player_won: false,
        ascension: 0,
        master_deck: Vec::new(),
        last_added_card: None,
        is_elite: false,
        is_boss: false,
        act_index: 1,
        active_attack: None,
        energy_wasted: 0,
        cards_drawn: 0,
        pending_run_gold_loss: 0,
        last_drawn_card: None,
    };

    // Enable recording, then play Cascade (hand index 0). Cascade is an
    // untargeted X-cost Self card.
    s.combat_log.record_card_play_log = true;
    let result = apply(
        &mut s,
        Action::PlayCard {
            hand_index: 0,
            target: None,
        },
    );
    assert_eq!(result, StepResult::Advanced, "Cascade play must advance");

    let log = &s.combat_log.card_play_log;
    assert!(
        log.len() >= 4,
        "expected trigger + 3 auto-plays, got {}: {:?}",
        log.len(),
        log
    );

    // ENGINE ORDERING: `play_card` records the manual trigger BEFORE it
    // dispatches the card body, so a trigger that auto-plays other cards
    // (Cascade / Havoc / Catastrophe) appears in the play-log AHEAD of the
    // children it spawns — parent-first order, chosen for the workbench event
    // log. (This is a display side-channel, NOT the C# `History.CardPlayFinished`
    // parity surface, so ordering it for readability is intentional.) The log is:
    //   [Cascade(manual, X=3), Strike(auto), Strike(auto), Defend(auto)].

    // (b) exactly 3 auto-plays.
    let auto_plays: Vec<&sts2_core::PlayLogEntry> = log.iter().filter(|e| e.auto_played).collect();
    assert_eq!(
        auto_plays.len(),
        3,
        "Cascade at X=3 auto-plays exactly 3 cards, got {}: {:?}",
        auto_plays.len(),
        log
    );

    // (a) the manual Cascade trigger: exactly one non-auto-play, X == 3,
    // logged FIRST (ahead of its children).
    let manual: Vec<&sts2_core::PlayLogEntry> = log.iter().filter(|e| !e.auto_played).collect();
    assert_eq!(
        manual.len(),
        1,
        "exactly one manual play (Cascade): {log:?}"
    );
    assert_eq!(
        log[0].card_id,
        CardId::Cascade,
        "manual Cascade logged first: {log:?}"
    );
    assert!(!log[0].auto_played, "log[0] is the manual trigger");
    assert_eq!(log[0].energy_spent, 3, "X should equal energy spent (3)");

    // (c) the three auto-plays follow the trigger, in forced draw-top order
    // [Strike, Strike, Defend].
    assert_eq!(
        log[1].card_id,
        CardId::StrikeIronclad,
        "auto-play 1 = draw[0]"
    );
    assert!(log[1].auto_played, "log[1] is an auto-play");
    assert_eq!(
        log[2].card_id,
        CardId::StrikeIronclad,
        "auto-play 2 = draw[1]"
    );
    assert!(log[2].auto_played, "log[2] is an auto-play");
    assert_eq!(
        log[3].card_id,
        CardId::DefendIronclad,
        "auto-play 3 = draw[2]"
    );
    assert!(log[3].auto_played, "log[3] is an auto-play");
}
