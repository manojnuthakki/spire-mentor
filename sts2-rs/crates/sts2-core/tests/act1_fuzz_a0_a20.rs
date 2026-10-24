//! Act-1 fuzz × {A0, A20} ascension matrix — bead `sts2-rs-29v` (k3y-a).
//!
//! Extends the existing P7-8 Act-1 fuzz harness (commit `e921231`,
//! file `act1_fuzz_p7.rs`) to **fully enumerate** the
//! `character × Act-1 encounter × ascension` matrix at the two
//! ascension levels currently plumbed end-to-end on `CombatState`
//! (`ascension: u8`, P7-1b): **A0** and **A20**.
//!
//! Differences vs `act1_fuzz_p7.rs`:
//!
//! - That harness picks an encounter *at random per seed*; this one
//!   walks the matrix deterministically so every (char, encounter,
//!   asc) cell records its own pass/fail counts.
//! - Seed encoding is `(seed_idx << 24) | (char as u64) << 16 |
//!   (enc_idx as u64) << 8 | asc as u64` so a failing playout's
//!   `(char, enc, asc, seed_idx)` is recoverable by inspecting the
//!   asserted seed in the panic message.
//! - Per-ascension-level pass/fail rollups are printed at the end so
//!   "did A20 regress vs A0?" can be eyeballed at a glance.
//!
//! Like its sibling harness this is `#[ignore]`-gated and only runs
//! under the nightly parity workflow (consistent with commit
//! `a275b4a` which moved the existing heavy fuzz out of `ci-fast`).
//!
//! ## Coverage matrix
//!
//! | Character (variant)     | A0 | A20 |
//! |-------------------------|----|-----|
//! | Ironclad (basic)        | ✓  | ✓   |
//! | Silent                  | ✓  | ✓   |
//! | Ironclad+AscBane        | ✓  | ✓   |
//! | Necrobinder             | ✓  | ✓   |
//! | Regent                  | ✓  | ✓   |
//!
//! 5 characters × 5 Act-1 encounters × 2 ascensions × 1 000 seeds
//! = **50 000** playouts (default).  Necrobinder + Regent coverage
//! added by bead `sts2-rs-s39` (bay-fixtures sub-task 2) once the
//! `necrobinder_starter_deck` / `regent_starter_deck` helpers landed
//! in `sts2-content`.
//!
//! The seed count is intentionally smaller than `act1_fuzz_p7`'s
//! 10 000 per cell because the matrix is denser here (30 cells vs 9)
//! and the test must still fit inside the nightly 240-minute budget
//! alongside the existing P7-8 harness. Bumping `SEEDS_PER_CELL`
//! before the audit-driven divergences in `k3y-c` land would just
//! amplify the same misses.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use smallvec::SmallVec;

use sts2_content::cards::ironclad_basic::{
    ironclad_starter_deck, ironclad_starter_deck_with_ascenders_bane,
};
use sts2_content::cards::regent::regent_starter_deck;
use sts2_content::cards::silent::silent_starter_deck;
use sts2_content::cards::CardSpec;
use sts2_content::potions::dispatch as potion_dispatch;
use sts2_content::relics::dispatch as relic_dispatch;
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::{
    apply, Action, Card, CardUid, CombatPhase, CombatState, Creature, CreatureUid, IntentHistory,
    MonsterId, MonsterKind, PlayerCombatState, Side, StepResult, TurnEvents,
};

/// Seeds per (character × encounter × ascension) cell. The default
/// fans out to 30 000 total playouts; bump locally when chasing a
/// specific divergence. Overridable via the
/// `STS2_FUZZ_29V_SEEDS_PER_CELL` env var so the smoke-run path
/// (`100` iters per cell) doesn't require an edit-recompile cycle.
const DEFAULT_SEEDS_PER_CELL: u64 = 1_000;

fn seeds_per_cell() -> u64 {
    std::env::var("STS2_FUZZ_29V_SEEDS_PER_CELL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_SEEDS_PER_CELL)
}
/// Defensive guard: random `EndTurn`-only playouts never need this
/// many turns to terminate (Cultist self-killable in <100).
const MAX_TURNS: u32 = 500;

/// Ascension levels covered by this harness. Mirrors the bead-spec
/// `{A0, A20}` tuple — A10 already lives in `act1_fuzz_p7.rs`.
const ASCENSIONS: &[u8] = &[0, 20];

#[derive(Clone, Copy)]
enum CharVariant {
    Ironclad,
    Silent,
    IroncladBane,
    // Necrobinder removed — card pool/starter deleted for a from-scratch
    // C#-first re-port. Re-add when the necrobinder starter factory returns.
    Regent,
}

impl CharVariant {
    const fn label(self) -> &'static str {
        match self {
            CharVariant::Ironclad => "Ironclad",
            CharVariant::Silent => "Silent",
            CharVariant::IroncladBane => "IroncladBane",
            CharVariant::Regent => "Regent",
        }
    }

    const fn all() -> &'static [CharVariant] {
        &[
            CharVariant::Ironclad,
            CharVariant::Silent,
            CharVariant::IroncladBane,
            CharVariant::Regent,
        ]
    }

    fn deck(self) -> Vec<CardSpec> {
        match self {
            CharVariant::Ironclad => ironclad_starter_deck(),
            CharVariant::Silent => silent_starter_deck(),
            CharVariant::IroncladBane => ironclad_starter_deck_with_ascenders_bane(),
            CharVariant::Regent => regent_starter_deck(),
        }
    }
}

/// Act-1 encounters mirroring `act1_fuzz_p7.rs::ACT1_ENCOUNTERS` plus
/// the 2× ShrinkerBeetle pair (handled separately because it's
/// 2 creatures). STS2-native analogues for the legacy STS1 stubs
/// (sts2-rs-vvz0).
const SINGLE_ENCOUNTERS: &[(&str, MonsterKind, i32)] = &[
    ("Nibbit", MonsterKind::Nibbit, 45),
    ("Mawler", MonsterKind::Mawler, 72),
    ("LeafSlimeS", MonsterKind::LeafSlimeS, 13),
    ("TwigSlimeS", MonsterKind::TwigSlimeS, 9),
];

const SENTRIES_LABEL: &str = "ShrinkerBeetles";

const fn num_encounters() -> usize {
    // 4 singletons + 1 Sentries cell.
    SINGLE_ENCOUNTERS.len() + 1
}

fn encounter_label(enc_idx: usize) -> &'static str {
    if enc_idx < SINGLE_ENCOUNTERS.len() {
        SINGLE_ENCOUNTERS[enc_idx].0
    } else {
        SENTRIES_LABEL
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

/// Compose a deterministic 64-bit seed from the matrix coordinates so
/// any failing playout's coordinates are recoverable from the seed
/// alone. Layout (LSB first):
///
/// | bits  | field      |
/// |-------|------------|
/// | 0..8  | ascension  |
/// | 8..16 | enc_idx    |
/// | 16..24| char_idx   |
/// | 24..64| seed_idx   |
fn encode_seed(seed_idx: u64, char_idx: u8, enc_idx: u8, ascension: u8) -> u64 {
    (seed_idx << 24)
        | (u64::from(char_idx) << 16)
        | (u64::from(enc_idx) << 8)
        | u64::from(ascension)
}

fn make_combat(seed: u64, variant: CharVariant, ascension: u8, enc_idx: usize) -> CombatState {
    let deck = variant.deck();
    let (player, cards_arena) = build_player(&deck);

    let mut creatures: Vec<Creature> = Vec::new();
    let mut enemy_uids: SmallVec<[CreatureUid; 8]> = SmallVec::new();
    if enc_idx < SINGLE_ENCOUNTERS.len() {
        let (_label, kind, hp) = SINGLE_ENCOUNTERS[enc_idx];
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
    } else {
        // 2× ShrinkerBeetle: STS2-native analogue for the legacy
        // 2-creature Sentry encounter (sts2-rs-vvz0). ShrinkerBeetle
        // has no Sentry-style offset-phase concept, so both creatures
        // share `IntentHistory::default()`.
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
    state
}

fn run_one(state: &mut CombatState, seed: u64) -> u32 {
    let mut turns = 0u32;
    while !state.is_over {
        let r = apply(state, Action::EndTurn);
        turns += 1;
        assert!(
            !matches!(r, StepResult::InvalidTarget),
            "InvalidTarget on EndTurn — seed=0x{seed:016x} turn={turns} round={}",
            state.round,
        );
        assert!(
            turns < MAX_TURNS,
            "exceeded {MAX_TURNS} turns — seed=0x{seed:016x} round={} hp={}",
            state.round,
            state.player.current_hp,
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

impl CellStats {
    fn runs(self) -> u64 {
        self.won + self.lost
    }
}

fn run_cell(variant: CharVariant, char_idx: u8, enc_idx: u8, ascension: u8) -> CellStats {
    let mut stats = CellStats::default();
    let per_cell = seeds_per_cell();
    for seed_idx in 0..per_cell {
        let seed = encode_seed(seed_idx, char_idx, enc_idx, ascension);
        let mut combat = make_combat(seed, variant, ascension, enc_idx as usize);
        let turns = run_one(&mut combat, seed);
        stats.total_turns += u64::from(turns);
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
        .join("act1_winrate_baseline_a0_a20.json")
}

/// Heavy fuzz harness: deterministic enumeration of
/// `{Ironclad, Silent, IroncladBane} × Act-1 5-encounter set × {A0, A20}`
/// at `SEEDS_PER_CELL` seeds per cell. `#[ignore]`-gated, runs nightly
/// via `parity-nightly.yml` (consistent with the `act1_fuzz_p7`
/// gating from commit `a275b4a`).
#[test]
#[ignore]
fn fuzz_a0_a20() {
    relic_dispatch::install_default_registry();
    potion_dispatch::install_default_registry();

    // (level → (cell-key → stats)) so we can roll up per-ascension
    // pass/fail counts cleanly at the end.
    let mut per_level: BTreeMap<u8, BTreeMap<String, CellStats>> = BTreeMap::new();
    for &asc in ASCENSIONS {
        per_level.insert(asc, BTreeMap::new());
    }

    for (char_idx, &variant) in CharVariant::all().iter().enumerate() {
        let char_idx_u8 = u8::try_from(char_idx).expect("≤5 chars");
        for enc_idx in 0..num_encounters() {
            let enc_idx_u8 = u8::try_from(enc_idx).expect("≤5 encounters");
            for &asc in ASCENSIONS {
                let stats = run_cell(variant, char_idx_u8, enc_idx_u8, asc);
                let expected = seeds_per_cell();
                assert_eq!(
                    stats.runs(),
                    expected,
                    "cell {}/{}/A{asc}: runs={} expected {expected}",
                    variant.label(),
                    encounter_label(enc_idx),
                    stats.runs(),
                );
                let key = format!("{}/{}", variant.label(), encounter_label(enc_idx));
                per_level
                    .get_mut(&asc)
                    .expect("level pre-inserted")
                    .insert(key, stats);
            }
        }
    }

    // Per-level rollup + per-cell printout.
    let mut summary = String::new();
    summary.push_str("# bead sts2-rs-29v · Act-1 fuzz × {A0, A20} per-cell summary\n");
    summary.push_str("# Format: A{asc} {char}/{enc}: runs=N won=N lost=N winrate=p avg_turns=t\n");
    let mut json_report: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for &asc in ASCENSIONS {
        let cells = per_level.get(&asc).expect("level present");
        let mut level_won = 0u64;
        let mut level_lost = 0u64;
        let mut level_cells: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        for (key, s) in cells {
            level_won += s.won;
            level_lost += s.lost;
            #[allow(clippy::cast_precision_loss)]
            let winrate = s.won as f64 / s.runs().max(1) as f64;
            #[allow(clippy::cast_precision_loss)]
            let avg_turns = s.total_turns as f64 / s.runs().max(1) as f64;
            summary.push_str(&format!(
                "A{asc} {key}: runs={} won={} lost={} winrate={winrate:.4} \
                 avg_turns={avg_turns:.2}\n",
                s.runs(),
                s.won,
                s.lost,
            ));
            level_cells.insert(
                key.clone(),
                serde_json::json!({
                    "runs": s.runs(),
                    "won": s.won,
                    "lost": s.lost,
                    "winrate": winrate,
                    "avg_turns": avg_turns,
                }),
            );
        }
        #[allow(clippy::cast_precision_loss)]
        let level_winrate = level_won as f64 / (level_won + level_lost).max(1) as f64;
        summary.push_str(&format!(
            "A{asc} TOTAL: runs={} won={} lost={} winrate={level_winrate:.4}\n",
            level_won + level_lost,
            level_won,
            level_lost,
        ));
        json_report.insert(
            format!("A{asc}"),
            serde_json::json!({
                "runs": level_won + level_lost,
                "won": level_won,
                "lost": level_lost,
                "winrate": level_winrate,
                "cells": level_cells,
            }),
        );
    }
    eprintln!("{summary}");

    // Drop the per-level rollup next to the existing P7-8 baseline
    // so reviewers can eyeball A0 vs A20 drift without rerunning.
    let path = baseline_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create tests/parity");
    }
    let json = serde_json::to_string_pretty(&json_report).expect("serialize report");
    fs::write(&path, json).expect("write baseline");
    eprintln!("[29v-fuzz] wrote per-level summary to {}", path.display());
}

#[cfg(test)]
mod seed_encoding {
    use super::encode_seed;

    /// Round-trip: each coordinate occupies its own byte / 40-bit
    /// slot and the encoding is reversible by masking. Guards
    /// against accidental field-overlap regressions.
    #[test]
    fn seed_encoding_is_invertible() {
        let cases = [
            (0u64, 0u8, 0u8, 0u8),
            (1, 0, 0, 0),
            (0, 2, 4, 20),
            (999, 2, 4, 20),
            (12_345, 1, 3, 0),
        ];
        for &(seed_idx, c, e, a) in &cases {
            let s = encode_seed(seed_idx, c, e, a);
            assert_eq!(s & 0xFF, u64::from(a), "asc byte");
            assert_eq!((s >> 8) & 0xFF, u64::from(e), "enc byte");
            assert_eq!((s >> 16) & 0xFF, u64::from(c), "char byte");
            assert_eq!(s >> 24, seed_idx, "seed_idx prefix");
        }
    }
}
