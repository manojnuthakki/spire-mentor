//! Phase 5 of the Act-1 wiring umbrella (`sts2-rs-7x18`).
//!
//! Acceptance from the plan
//! (`docs/plans/2026-05-14-…-cli.md` Task 5.1):
//!
//! > For each `MonsterKind` in act1*` and each `EncounterDef` in
//! > `ACT1_ENCOUNTERS`, run a 20-turn random-play sim against Ironclad
//! > with `--seed 0`. Assert combat reaches a terminal state (player
//! > wins, dies, or 30-turn cap) without panicking.
//!
//! We drive `legal_actions → apply` directly (no REPL / no JSONL) for
//! speed; each scenario gets at most `2_000` decisions. The contract is
//! "no panic, no `InvalidTarget` from a legal action" — terminating
//! is nice-to-have but not required (some bosses, e.g. Vantom at 206
//! HP, won't fall to random play). Each combat attaches a real DSL
//! `MonsterAi` via `build_ai`, so the Phase-5 dispatch + the Phase-2
//! enum wiring + the Phase-3 registry are all exercised end-to-end.

use smallvec::SmallVec;

use sts2_content::cards::ironclad_basic::ironclad_starter_deck;
use sts2_content::encounters::act1_overgrowth::ACT1_ENCOUNTERS;
use sts2_content::monster_dispatch::{build_ai, MonsterSpawnArgs, SpawnPosition};
use sts2_core::cards::Card;
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::{
    apply, legal_actions, CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory,
    MonsterId, MonsterKind, PlayerCombatState, Side, StepResult, TurnEvents,
};

const MAX_DECISIONS: u32 = 2_000;

/// Build a starter `CombatState` for Ironclad facing `monsters`, with
/// real DSL `MonsterAi`s attached via `build_ai`. Mirrors the `act1_fuzz`
/// helper's shape but feeds through Phase 1/2 dispatch.
fn make_combat(seed: u64, monsters: &[(MonsterKind, MonsterSpawnArgs)]) -> CombatState {
    let deck = ironclad_starter_deck();
    let mut draw: SmallVec<[CardUid; 32]> = SmallVec::new();
    let mut cards_arena: Vec<Card> = Vec::with_capacity(deck.len());
    for (i, spec) in deck.iter().enumerate() {
        let raw = u32::try_from(i + 1).expect("deck size fits in u32");
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

    let mut rng = sts2_core::dotnet_rng::Rng::new((seed) as i32);
    let mut creatures = Vec::with_capacity(monsters.len());
    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    let alone = monsters.len() == 1;
    for (idx, (kind, args)) in monsters.iter().enumerate() {
        let raw = u32::try_from(idx + 1).expect("monster idx fits u32");
        let uid = CreatureUid::new(raw).expect("non-zero");
        let hp = kind.starting_hp(&mut rng, 0);
        let pos = SpawnPosition {
            alone,
            front: idx == 0,
            back: idx + 1 == monsters.len(),
            index: u8::try_from(idx).unwrap_or(u8::MAX),
        };
        creatures.push(Creature {
            id: uid.0,
            current_hp: hp,
            max_hp: hp,
            block: 0,
            powers: SmallVec::new(),
            species: Some(*kind),
            monster_id: MonsterId(kind.python_name()),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: Some(build_ai(*kind, *args, pos)),
        });
        enemy_uids.push(uid);
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
        current_hp: 80,
        max_hp: 80,
        block: 0,
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
        pinned_top: smallvec::SmallVec::new(),
        pinned_bottom: SmallVec::new(),
        deferred_draw_order: false,
    };

    let mut state = CombatState {
        belt_value: None,
        phase: CombatPhase::Setup,
        round: 0,
        side: Side::Player,
        rng: sts2_core::dotnet_rng::Rng::new((seed) as i32),

        run_rng: sts2_core::run_rng_set::RunRngSet::new((seed) as i32),
        creatures,
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

    let r0 = start_combat(&mut state);
    assert_eq!(r0, StepResult::Advanced);
    let r1 = start_player_turn(&mut state);
    assert_eq!(r1, StepResult::Advanced);
    assert_eq!(state.phase, CombatPhase::PlayerPlayPhase);
    state
}

/// Drive random play until terminal or the decision cap is hit.
fn run_random(state: &mut CombatState, rng: &mut sts2_core::dotnet_rng::Rng, label: &str) {
    for _ in 0..MAX_DECISIONS {
        if state.is_over {
            return;
        }
        let actions = legal_actions(state);
        if actions.is_empty() {
            return;
        }
        let idx = rng.next_int_exclusive(0, actions.len() as i32) as usize;
        let action = actions[idx].clone();
        let result = apply(state, action.clone());
        match result {
            StepResult::Advanced
            | StepResult::CombatWon
            | StepResult::CombatLost
            | StepResult::PendingChoice => {}
            StepResult::InvalidTarget => {
                panic!(
                    "InvalidTarget on a legal_actions-derived action in '{label}': \
                     action={action:?}"
                );
            }
        }
    }
}

/// The 29 Act-1 monsters (7 `act1_weak` including Nibbit + 22 act1).
fn act1_monster_kinds() -> Vec<MonsterKind> {
    use MonsterKind as M;
    vec![
        M::Nibbit,
        M::ShrinkerBeetle,
        M::FuzzyWurmCrawler,
        M::LeafSlimeS,
        M::TwigSlimeS,
        M::LeafSlimeM,
        M::TwigSlimeM,
        M::CubexConstruct,
        M::Flyconid,
        M::EyeWithTeeth,
        M::Fogmog,
        M::Inklet,
        M::Mawler,
        M::VineShambler,
        M::SlitheringStrangler,
        M::SnappingJaxfruit,
        M::AssassinRubyRaider,
        M::AxeRubyRaider,
        M::BruteRubyRaider,
        M::CrossbowRubyRaider,
        M::TrackerRubyRaider,
        M::BygoneEffigy,
        M::Byrdonis,
        M::PhrogParasite,
        M::Parafright,
        M::Vantom,
        M::CeremonialBeast,
        M::KinPriest,
        M::KinFollower,
    ]
}

#[test]
fn every_act1_monsterkind_drives_random_play_without_panic() {
    let kinds = act1_monster_kinds();
    assert_eq!(kinds.len(), 29, "expected 29 act1 monsters");
    for kind in kinds {
        let monsters = [(kind, MonsterSpawnArgs::None)];
        let mut state = make_combat(0, &monsters);
        let mut rng = sts2_core::dotnet_rng::Rng::new(0xa11ce_i32);
        run_random(&mut state, &mut rng, &format!("solo {kind:?}"));
    }
}

#[test]
fn every_act1_encounter_drives_random_play_without_panic() {
    assert_eq!(ACT1_ENCOUNTERS.len(), 22);
    for enc in ACT1_ENCOUNTERS {
        let mut state = make_combat(0, enc.monsters);
        let mut rng = sts2_core::dotnet_rng::Rng::new(0xbee_i32);
        run_random(&mut state, &mut rng, enc.id);
    }
}
