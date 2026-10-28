#![allow(dead_code)]
//! Shared test fixtures for sts2-mcts integration tests.
//!
//! No P4-p84 standalone fixture file was located in the workspace; the
//! `random_playout_fuzz` test uses inline construction (see
//! `crates/sts2-core/tests/random_playout_fuzz.rs`). We mirror that
//! pattern here.

use smallvec::SmallVec;

use sts2_core::card_effects;
use sts2_core::cards::{Card, CardId, CardTags, CardType, TargetType};
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::{
    CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory, MonsterId,
    MonsterKind, PlayerCombatState, Side, TurnEvents,
};

/// Same as [`build_handful`] but with full control over the cultist's
/// intent and the player's potion inventory. Used by the value-function
/// regression tests (bead `sts2-rs-v2oh`) where the eval depends on
/// remaining HP / kept potions at terminal — knobs the original
/// fixture didn't expose.
pub fn build_handful_ex(
    seed: u64,
    hand_cards: &[(CardId, i32, i32, i8, TargetType)],
    cultist_hp: i32,
    player_hp: i32,
    cultist_intent: sts2_core::Intent,
    potions: &[sts2_core::PotionKind],
    energy: i8,
) -> CombatState {
    let mut state = build_handful(seed, hand_cards, cultist_hp, player_hp);
    let enemy_id = state.enemy_uids[0].0;
    if let Some(c) = state.creatures.iter_mut().find(|c| c.id == enemy_id) {
        c.intent = Some(cultist_intent);
    }
    state.player.energy = energy;
    state.player.max_energy = energy.max(state.player.max_energy);
    for &p in potions {
        state.player.potions.push(p);
    }
    state
}

/// Build a `PlayerPlayPhase` Ironclad-vs-Cultist scenario with `hand_cards`
/// already in the player's hand. Used for both the toy convergence test
/// (a hand-rigged dominant Strike) and the vs-random benchmark.
pub fn build_handful(
    seed: u64,
    hand_cards: &[(CardId, i32, i32, i8, TargetType)],
    cultist_hp: i32,
    player_hp: i32,
) -> CombatState {
    card_effects::ensure_installed();
    sts2_content::cards::dispatch::install_default_registry();

    let mut cards_arena: Vec<Card> = Vec::with_capacity(hand_cards.len());
    let mut hand: SmallVec<[CardUid; 16]> = SmallVec::new();
    for (idx, &(card_id, base_damage, base_block, cost, target_type)) in
        hand_cards.iter().enumerate()
    {
        let raw = u32::try_from(idx + 1).expect("hand fits u32");
        let uid = CardUid::new(raw).expect("non-zero");
        let card_type = match target_type {
            TargetType::Enemy => CardType::Attack,
            _ => CardType::Skill,
        };
        cards_arena.push(Card {
            uid,
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
        });
        hand.push(uid);
    }

    let player = PlayerCombatState {
        current_hp: player_hp,
        max_hp: player_hp,
        block: 0,
        energy: 3,
        max_energy: 3,
        hand,
        draw: SmallVec::new(),
        discard: SmallVec::new(),
        exhaust: SmallVec::new(),
        play: SmallVec::new(),
        stars: 0,
        powers: SmallVec::new(),
        character: None,
        potions: smallvec::SmallVec::new(),
        potion_slot_indices: smallvec::SmallVec::new(),
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
        retain_this_turn: smallvec::SmallVec::new(),
        sly_this_turn: smallvec::SmallVec::new(),
        sly_this_combat: smallvec::SmallVec::new(),
        pending_retain_count: 0,
        stars_gained_this_turn: 0,
        pinned_top: smallvec::SmallVec::new(),
        pinned_bottom: SmallVec::new(),
        deferred_draw_order: false,
    };

    let cultist_uid = CreatureUid::new(1).expect("non-zero");
    let cultist = Creature {
        id: cultist_uid.0,
        current_hp: cultist_hp,
        max_hp: cultist_hp,
        block: 0,
        powers: SmallVec::new(),
        species: Some(MonsterKind::Nibbit),
        intent: Some(sts2_core::Intent::Attack { damage: 6 }),
        intent_history: IntentHistory::default(),
        monster_id: MonsterId("Nibbit"),
        ai: None,
    };

    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    enemy_uids.push(cultist_uid);

    CombatState {
        belt_value: None,
        phase: CombatPhase::PlayerPlayPhase,
        round: 1,
        side: Side::Player,
        rng: sts2_core::dotnet_rng::Rng::new((seed) as i32),

        run_rng: sts2_core::run_rng_set::RunRngSet::new((seed) as i32),
        creatures: vec![cultist],
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
    }
}

/// Vertical-slice fixture: Ironclad with starter draw pile vs a Cultist
/// at 48 HP. Mirrors the encounter `build_encounter_state` in sts2-py
/// but inline so the mcts crate doesn't need a sts2-py dev-dep.
///
/// `player_hp` allows callers to dial the difficulty — at the default
/// 80/80 random play wins ~100% (the Cultist Phase-3 stub is harmless
/// because relics/powers aren't fully wired), so the vs-random test
/// passes a lower HP to surface the random vs MCTS gap.
pub fn build_vertical_slice(seed: u64) -> CombatState {
    build_vertical_slice_hp(seed, 80, 48)
}

pub fn build_vertical_slice_hp(seed: u64, player_hp: i32, cultist_hp: i32) -> CombatState {
    card_effects::ensure_installed();
    // Install card registry so PlayCard dispatch works during MCTS rollouts.
    sts2_content::cards::dispatch::install_default_registry();

    let deck = sts2_content::cards::ironclad_basic::ironclad_starter_deck();
    let mut draw: SmallVec<[CardUid; 32]> = SmallVec::new();
    let mut cards_arena: Vec<Card> = Vec::with_capacity(deck.len());
    for (idx, spec) in deck.iter().enumerate() {
        let raw = u32::try_from(idx + 1).expect("deck fits u32");
        let uid = CardUid::new(raw).expect("non-zero");
        draw.push(uid);
        cards_arena.push(Card {
            uid,
            card_id: spec.id,
            cost: spec.cost,
            card_type: spec.card_type,
            target_type: spec.target_type,
            upgraded: false,
            base_damage: spec.base_damage,
            base_block: spec.base_block,
            ..Card::pod_defaults()
        });
    }

    let player = PlayerCombatState {
        hand: SmallVec::new(),
        draw,
        discard: SmallVec::new(),
        exhaust: SmallVec::new(),
        play: SmallVec::new(),
        energy: 0,
        max_energy: 3,
        stars: 0,
        current_hp: player_hp,
        max_hp: player_hp,
        block: 0,
        powers: SmallVec::new(),
        character: None,
        potions: smallvec::SmallVec::new(),
        potion_slot_indices: smallvec::SmallVec::new(),
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
        retain_this_turn: smallvec::SmallVec::new(),
        sly_this_turn: smallvec::SmallVec::new(),
        sly_this_combat: smallvec::SmallVec::new(),
        pending_retain_count: 0,
        stars_gained_this_turn: 0,
        pinned_top: smallvec::SmallVec::new(),
        pinned_bottom: SmallVec::new(),
        deferred_draw_order: false,
    };

    let cultist_uid = CreatureUid::new(1).expect("non-zero");
    let cultist = Creature {
        id: cultist_uid.0,
        current_hp: cultist_hp,
        max_hp: cultist_hp,
        block: 0,
        powers: SmallVec::new(),
        species: Some(MonsterKind::Nibbit),
        monster_id: MonsterId(MonsterKind::Nibbit.python_name()),
        intent: None,
        intent_history: IntentHistory::default(),
        ai: None,
    };

    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    enemy_uids.push(cultist_uid);

    let mut state = CombatState {
        belt_value: None,
        phase: CombatPhase::Setup,
        round: 0,
        side: Side::Player,
        rng: sts2_core::dotnet_rng::Rng::new((seed) as i32),

        run_rng: sts2_core::run_rng_set::RunRngSet::new((seed) as i32),
        creatures: vec![cultist],
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
    let _ = start_combat(&mut state);
    let _ = start_player_turn(&mut state);
    state
}
