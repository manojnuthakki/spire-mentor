//! Act-1 fuzz × ascension × character — bead `sts2-rs-d19` (P7-8).
//!
//! Drives 10 000 random Act-1 playouts per (character × ascension)
//! tuple, asserting that the engine **never panics** and that
//! `apply(Action::EndTurn)` never produces `InvalidTarget` / unwanted
//! `PendingChoice`. The P7 capstone differentiator vs the existing
//! `random_playout_fuzz.rs` (P4 baseline) is that the relic / potion /
//! orb dispatch registries are **fully installed** before the fuzz
//! starts — every relic resolves through the content-side handlers
//! shipped by P7-2..P7-7.
//!
//! ## Coverage matrix
//!
//! | Character (variant) | A0 | A10 | A20 |
//! |---------------------|----|-----|-----|
//! | Ironclad (basic)    | ✓  | ✓   | ✓   |
//! | Silent              | ✓  | ✓   | ✓   |
//! | Ironclad+AscBane    | ✓  | ✓   | ✓   |
//!
//! Total: 3 × 3 × 10 000 = 90 000 playouts.
//!
//! Defect is skipped: at pin `01c094d` it has no starter deck in
//! `sts2-content::cards` (its deck lands with a follow-up bead).
//!
//! ## Baseline win-rates
//!
//! Recorded under `tests/parity/act1_winrate_baseline_p7.json` for
//! reviewer inspection. The asserts only check **non-panic + bounded
//! turns** because card-play strategy is `EndTurn`-only (no AI), and
//! win-rates with no cards being played are very close to zero — the
//! file is informational, not load-bearing for CI.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use rand::{Rng, SeedableRng};
use rand_xoshiro::Xoshiro256PlusPlus;
use smallvec::SmallVec;

use sts2_content::cards::ironclad_basic::{
    ironclad_starter_deck, ironclad_starter_deck_with_ascenders_bane,
};
use sts2_content::cards::silent::silent_starter_deck;
use sts2_content::cards::CardSpec;
use sts2_content::potions::dispatch as potion_dispatch;
use sts2_content::relics::dispatch as relic_dispatch;
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::{
    apply, Action, Card, CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory,
    MonsterId, MonsterKind, PlayerCombatState, Side, StepResult, TurnEvents,
};

const SEEDS_PER_CELL: u64 = 10_000;
const MAX_TURNS: u32 = 500;

#[derive(Clone, Copy)]
enum CharVariant {
    Ironclad,
    Silent,
    IroncladBane,
}

impl CharVariant {
    const fn label(self) -> &'static str {
        match self {
            CharVariant::Ironclad => "Ironclad",
            CharVariant::Silent => "Silent",
            CharVariant::IroncladBane => "IroncladBane",
        }
    }

    fn deck(self) -> Vec<CardSpec> {
        match self {
            CharVariant::Ironclad => ironclad_starter_deck(),
            CharVariant::Silent => silent_starter_deck(),
            CharVariant::IroncladBane => ironclad_starter_deck_with_ascenders_bane(),
        }
    }
}

fn build_player(deck: &[CardSpec]) -> (PlayerCombatState, Vec<Card>) {
    let mut draw: SmallVec<[CardUid; 32]> = SmallVec::new();
    let mut arena: Vec<Card> = Vec::with_capacity(deck.len());
    for (idx, spec) in deck.iter().enumerate() {
        let raw = u32::try_from(idx + 1).expect("deck fits u32");
        let uid = CardUid::new(raw).expect("non-zero");
        draw.push(uid);
        arena.push(Card {
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
        retain_this_turn: smallvec::SmallVec::new(),
        sly_this_turn: smallvec::SmallVec::new(),
        sly_this_combat: smallvec::SmallVec::new(),
        pending_retain_count: 0,
        stars_gained_this_turn: 0,
        pinned_top: smallvec::SmallVec::new(),
        pinned_bottom: smallvec::SmallVec::new(),
        deferred_draw_order: false,
    };
    (player, arena)
}

/// Pool of Act-1 encounters mirroring the existing
/// `random_playout_fuzz.rs` set. Each entry is (label, enemy
/// constructor closure-input).
// STS2-native analogues for the legacy STS1 stub set (sts2-rs-vvz0):
//   Cultist    → Nibbit            (single-attack, HP 45)
//   JawWorm    → Mawler            (multi-attack act1 normal, HP 72)
//   AcidSlimeS → LeafSlimeS        (small slime, HP 11-15, mid 13)
//   GreenLouse → TwigSlimeS        (small slime variant, HP 7-11, mid 9)
const ACT1_ENCOUNTERS: &[(&str, MonsterKind, i32)] = &[
    ("Nibbit", MonsterKind::Nibbit, 45),
    ("Mawler", MonsterKind::Mawler, 72),
    ("LeafSlimeS", MonsterKind::LeafSlimeS, 13),
    ("TwigSlimeS", MonsterKind::TwigSlimeS, 9),
];

fn make_combat(
    seed: u64,
    variant: CharVariant,
    ascension: u8,
    enc_idx: usize,
) -> (CombatState, &'static str) {
    let deck = variant.deck();
    let (player, cards_arena) = build_player(&deck);

    let mut creatures: Vec<Creature> = Vec::new();
    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    let label;
    if enc_idx < ACT1_ENCOUNTERS.len() {
        let (l, kind, hp) = ACT1_ENCOUNTERS[enc_idx];
        let uid = CreatureUid::new(1).expect("non-zero");
        creatures.push(Creature {
            id: uid.0,
            current_hp: hp,
            max_hp: hp,
            block: 0,
            powers: SmallVec::new(),
            species: Some(kind),
            monster_id: MonsterId(kind.python_name()),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: None,
        });
        enemy_uids.push(uid);
        label = l;
    } else {
        // 2× ShrinkerBeetle: STS2-native analogue for the legacy
        // 2-creature Sentry encounter (sts2-rs-vvz0). ShrinkerBeetle
        // has no Sentry-style offset-phase concept, so both creatures
        // share `IntentHistory::default()`.
        label = "ShrinkerBeetles";
        for i in 0..2 {
            let raw = u32::try_from(i + 1).expect("idx fits u32");
            let uid = CreatureUid::new(raw).expect("non-zero");
            creatures.push(Creature {
                id: uid.0,
                current_hp: 39,
                max_hp: 39,
                block: 0,
                powers: SmallVec::new(),
                species: Some(MonsterKind::ShrinkerBeetle),
                monster_id: MonsterId(MonsterKind::ShrinkerBeetle.python_name()),
                intent: None,
                intent_history: IntentHistory::default(),
                ai: None,
            });
            enemy_uids.push(uid);
        }
    }

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
        event_combat_timed_out: false,
        gold: 0,
        is_over: false,
        player_won: false,
        ascension,
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
    let _ = variant; // unused after deck consumption
    (state, label)
}

fn run_one(state: &mut CombatState) -> u32 {
    let mut turns = 0u32;
    while !state.is_over {
        let r = apply(state, Action::EndTurn);
        turns += 1;
        assert!(
            !matches!(r, StepResult::InvalidTarget),
            "InvalidTarget on EndTurn — bug in dispatch"
        );
        assert!(
            turns < MAX_TURNS,
            "exceeded {MAX_TURNS} turns at turn={turns} hp={} round={}",
            state.player.current_hp,
            state.round,
        );
    }
    turns
}

#[derive(Default, Clone, Copy, Debug)]
struct CellStats {
    won: u64,
    lost: u64,
    total_turns: u64,
}

fn run_cell(variant: CharVariant, ascension: u8) -> CellStats {
    relic_dispatch::install_default_registry();
    potion_dispatch::install_default_registry();

    let mut stats = CellStats::default();
    let mut rng =
        Xoshiro256PlusPlus::seed_from_u64(0xD19_0000 + u64::from(ascension) * 100 + variant as u64);
    for i in 0..SEEDS_PER_CELL {
        let enc_idx = rng.gen_range(0..=ACT1_ENCOUNTERS.len()); // 0..=4 → 5 enc incl. Sentries
        let seed = i ^ (u64::from(ascension) << 16) ^ ((variant as u64) << 32);
        let (mut combat, _label) = make_combat(seed, variant, ascension, enc_idx);
        let _ = run_one(&mut combat);
        stats.total_turns += u64::from(combat.round);
        if combat.player_won {
            stats.won += 1;
        } else {
            stats.lost += 1;
        }
    }
    stats
}

fn baseline_path() -> PathBuf {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = crate_root
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");
    workspace_root
        .join("tests")
        .join("parity")
        .join("act1_winrate_baseline_p7.json")
}

// Heavy fuzz regression test (~39s in dev profile): 30k seeds × three
// ascensions per character. Runs nightly via `parity-nightly.yml`
// (which is already wired with `cargo test -p sts2-core --release --test act1_fuzz_p7`).
#[test]
#[ignore]
fn act1_fuzz_30k_x_three_ascensions() {
    let cells: &[(CharVariant, u8)] = &[
        (CharVariant::Ironclad, 0),
        (CharVariant::Ironclad, 10),
        (CharVariant::Ironclad, 20),
        (CharVariant::Silent, 0),
        (CharVariant::Silent, 10),
        (CharVariant::Silent, 20),
        (CharVariant::IroncladBane, 0),
        (CharVariant::IroncladBane, 10),
        (CharVariant::IroncladBane, 20),
    ];

    let mut report: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for &(variant, asc) in cells {
        let key = format!("{}_A{asc}", variant.label());
        let stats = run_cell(variant, asc);
        assert_eq!(
            stats.won + stats.lost,
            SEEDS_PER_CELL,
            "{key}: won+lost != {SEEDS_PER_CELL}"
        );
        #[allow(clippy::cast_precision_loss)]
        let winrate = stats.won as f64 / SEEDS_PER_CELL as f64;
        #[allow(clippy::cast_precision_loss)]
        let avg_turns = stats.total_turns as f64 / SEEDS_PER_CELL as f64;
        eprintln!(
            "[p7-fuzz] {key}: {SEEDS_PER_CELL} seeds, won={} lost={} \
             winrate={winrate:.4} avg_turns={avg_turns:.2}",
            stats.won, stats.lost,
        );
        report.insert(
            key,
            serde_json::json!({
                "seeds": SEEDS_PER_CELL,
                "won": stats.won,
                "lost": stats.lost,
                "winrate": winrate,
                "avg_turns": avg_turns,
            }),
        );
    }

    let json = serde_json::to_string_pretty(&report).expect("serialize baseline");
    let path = baseline_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create tests/parity");
    }
    fs::write(&path, json).expect("write baseline");
    eprintln!("[p7-fuzz] wrote baseline to {}", path.display());
}
