//! Wide-schema feature extractor for the AlphaZero-style Expert
//! Iteration path (bead `sts2-rs-qctu`, Wave-6). Sister to the
//! 12-feature [`crate::eval_features`] surface used by the Texel
//! HCE tuner — this module exposes ~80 floats covering player
//! pile composition, per-power amounts, and per-enemy-slot state
//! for use as MLP value-net / policy-net input.
//!
//! ## When to use which
//!
//! - [`crate::eval_features`] (12 floats): the linear-logistic
//!   Texel-fitted evaluator that lives at the MCTS rollout leaf.
//!   Hot path — one call per depth-cap rollout. ~24 flops.
//! - [`eval_features_wide`] (this module, ~80 floats): the
//!   feature surface for any nonlinear leaf-eval experiment
//!   (Wave-6 spike `sts2-rs-eima`, full Expert Iteration
//!   `sts2-rs-qctu`). Not yet wired into the MCTS hot path; the
//!   plan §4.7.3 calls for a tract-ONNX-runtime integration once
//!   a trained net beats the Texel HCE on the yuyc A/B gate
//!   (sub-bead `qctu.5`).
//!
//! ## Column layout
//!
//! The schema is pinned by [`EVAL_FEATURE_NAMES_WIDE`]. Re-ordering
//! columns MUST flow through the corpus generator
//! (`sts2-rs-qctu.1`) and any downstream training scripts in
//! lockstep. The layout deliberately groups related signals into
//! contiguous slices so dropping/extending a group is a single
//! edit on both sides.
//!
//! - Indices 0..10:  player scalars (HP, energy, stars, pile sizes)
//! - Indices 10..15: hand card-type counts (by [`CardType`])
//! - Indices 15..20: draw + discard combined card-type counts
//! - Indices 20..28: 8 key player Powers (Strength, Dex, Focus,
//!                   Vulnerable, Weak, Intangible, Thorns, Buffer)
//! - Indices 28..76: 4 enemy slots × 12 features each
//!                   (alive, hp_ratio, hp_norm, intent kind one-hots,
//!                   intent damage, Vuln, Weak, Strength)
//! - Indices 76..80: round + flags (round_norm, is_elite,
//!                   energy_wasted_norm, cards_drawn_norm)
//!
//! Per-enemy slot uses canonical `enemy_uids` order so slot 0 ==
//! the front enemy. Dead enemies still occupy their slot (the
//! `alive_flag` is 0); the slot is reused only when the
//! engine itself reuses the uid (it doesn't at this pin).

use smallvec::SmallVec;

use crate::cards::CardType;
use crate::monsters::Intent;
use crate::{CombatState, PowerKind};

#[cfg(test)]
use crate::MonsterAi;

#[cfg(test)]
use crate::PlayerCombatState;

/// Number of enemy slots the wide schema reserves. Chosen so the
/// Act-3 boss `Decimillipede` (3 segments + boss = 4) fits without
/// truncation, while keeping the per-state vector tractably small
/// for hobby-scale GPU training. Extra slots beyond this are
/// silently dropped — flag if a future encounter exceeds 4 active
/// alive enemies at the same time.
pub const WIDE_ENEMY_SLOTS: usize = 4;

/// Features per enemy slot (see column layout note in the module
/// docstring).
pub const FEATURES_PER_ENEMY: usize = 12;

/// Total wide-feature dimension. Pinned at 172 — bumping this
/// requires touching every downstream consumer (corpus generator,
/// training scripts, any future tract-loader Rust path).
///
/// Bead `sts2-rs-uuln` (feature-set extension): grew from 80 → 88
/// by adding 4 missing player Powers (Frail / Poison / Constrict /
/// Ritual), 3 pile-cost scalars (hand_total_cost / deck_total_cost
/// / hand_avg_cost), and 1 pet-HP slot (Necrobinder Osty). The
/// gate was eima FAIL — the +8 features target the gap that the
/// 80-feature schema's missing-debuff coverage left for the
/// nonlinear value-net to chase.
///
/// Bead `sts2-rs-h5nf6` (6pzjm.1): grew from 88 → 172 by adding
/// per-enemy-slot K=3 lookahead columns sourced from
/// [`crate::MonsterAi::peek_intents`]. Each of the 4 enemy slots
/// emits 3 turns × (5 intent-kind one-hots + dmg_norm + certainty)
/// = 21 columns; 4 × 21 = 84 total. Layout details at
/// [`LOOKAHEAD_BASE`].
///
/// 2026-07-19 regent-combat-leaf plan (Task 1): grew from 172 → 192
/// by appending 20 Regent-only engine columns (stars-gained-this-turn,
/// 14 player-side Regent power stacks, 3 Sovereign-Blade forge
/// scalars, 2 stars×power interaction terms). Every new column reads
/// **Regent-only state** (stars pipeline, Regent-exclusive
/// `PowerKind`s, `CardId::SovereignBlade`) and is `0.0` on every other
/// character's state, so this is a pure additive extension —
/// `EVAL_FEATURE_WEIGHTS_WIDE`'s 20 new entries start at `0.0`, making
/// [`evaluate_in_progress_wide`] bit-identical for every non-Regent
/// state until the retune (Task 4) fills them in. See
/// `regent_extension_columns_present_and_default_zero` /
/// `regent_extension_columns_nonzero_on_regent_state` below for the
/// full column-by-column contract.
pub const WIDE_FEATURE_DIM: usize = 216;

/// Index of the first lookahead column. Indices `LOOKAHEAD_BASE..
/// WIDE_FEATURE_DIM` are the per-enemy K=3 peek block. Layout:
///
/// ```text
/// LOOKAHEAD_BASE + slot * 21 + (turn - 1) * 7 + sub
///   sub 0: intent_attack one-hot
///   sub 1: intent_buff one-hot
///   sub 2: intent_debuff one-hot
///   sub 3: intent_defend one-hot
///   sub 4: intent_other one-hot
///   sub 5: dmg_norm (sum across step's intents, /20)
///   sub 6: certainty (running branch-probability product at step)
/// ```
pub const LOOKAHEAD_BASE: usize = 88;
/// Number of lookahead turns peeked per enemy slot.
pub const LOOKAHEAD_TURNS: usize = 3;
/// Number of columns per (enemy_slot, turn) tuple.
pub const LOOKAHEAD_FEATURES_PER_TURN: usize = 7;
/// Number of lookahead columns per enemy slot.
pub const LOOKAHEAD_FEATURES_PER_ENEMY: usize = LOOKAHEAD_TURNS * LOOKAHEAD_FEATURES_PER_TURN;

/// Canonical column names for [`eval_features_wide`]. The downstream
/// Wave-6 corpus generator + training scripts use these as the NPY
/// header so column drift surfaces as a test failure rather than a
/// silent feature mismatch.
pub const EVAL_FEATURE_NAMES_WIDE: [&str; WIDE_FEATURE_DIM] = [
    // 0..10 — player scalars
    "p_hp_ratio",
    "p_max_hp_norm",
    "p_block_norm",
    "p_energy",
    "p_max_energy",
    "p_stars",
    "p_hand_size",
    "p_draw_size",
    "p_discard_size",
    "p_exhaust_size",
    // 10..15 — hand card-type counts (Attack/Skill/Power/Curse/Status)
    "hand_attacks",
    "hand_skills",
    "hand_powers",
    "hand_curses",
    "hand_statuses",
    // 15..20 — combined draw+discard card-type counts (deck-remaining signal)
    "deck_attacks",
    "deck_skills",
    "deck_powers",
    "deck_curses",
    "deck_statuses",
    // 20..28 — 8 key player Powers (raw amounts)
    "pp_strength",
    "pp_dexterity",
    "pp_focus",
    "pp_vulnerable",
    "pp_weak",
    "pp_intangible",
    "pp_thorns",
    "pp_buffer",
    // 28..40 — enemy slot 0 (12 features)
    "e0_alive",
    "e0_hp_ratio",
    "e0_hp_norm",
    "e0_intent_attack",
    "e0_intent_buff",
    "e0_intent_debuff",
    "e0_intent_defend",
    "e0_intent_other",
    "e0_intent_dmg_norm",
    "e0_vuln",
    "e0_weak",
    "e0_strength",
    // 40..52 — enemy slot 1
    "e1_alive",
    "e1_hp_ratio",
    "e1_hp_norm",
    "e1_intent_attack",
    "e1_intent_buff",
    "e1_intent_debuff",
    "e1_intent_defend",
    "e1_intent_other",
    "e1_intent_dmg_norm",
    "e1_vuln",
    "e1_weak",
    "e1_strength",
    // 52..64 — enemy slot 2
    "e2_alive",
    "e2_hp_ratio",
    "e2_hp_norm",
    "e2_intent_attack",
    "e2_intent_buff",
    "e2_intent_debuff",
    "e2_intent_defend",
    "e2_intent_other",
    "e2_intent_dmg_norm",
    "e2_vuln",
    "e2_weak",
    "e2_strength",
    // 64..76 — enemy slot 3
    "e3_alive",
    "e3_hp_ratio",
    "e3_hp_norm",
    "e3_intent_attack",
    "e3_intent_buff",
    "e3_intent_debuff",
    "e3_intent_defend",
    "e3_intent_other",
    "e3_intent_dmg_norm",
    "e3_vuln",
    "e3_weak",
    "e3_strength",
    // 76..80 — round + flags
    "round_norm",
    "is_elite",
    "energy_wasted_norm",
    "cards_drawn_norm",
    // 80..84 — bead sts2-rs-uuln: missing player Powers
    "pp_frail",  // common enemy-applied debuff (Mawler/SnappingJaxfruit/Parafright/Vantom)
    "pp_poison", // Seapunk POISON_SPIT etc.
    "pp_constrict", // SlitheringStrangler debuff
    "pp_ritual", // Ritual-style buff (rare on player; signal slot for symmetry)
    // 84..87 — bead sts2-rs-uuln: pile cost scalars
    "hand_total_cost", // sum of card.cost across hand
    "deck_total_cost", // sum across draw + discard
    "hand_avg_cost",   // hand_total_cost / max(1, hand_size)
    // 87 — bead sts2-rs-uuln: Necrobinder pet signal
    "pet_hp_ratio", // Osty current_hp / max_hp if alive else 0
    // 88..172 — bead sts2-rs-h5nf6: per-enemy-slot K=3 peek_intents lookahead
    // (4 slots × 3 turns × 7 features). Names are generated by
    // `lookahead_column_name`; the static table mirrors that helper
    // so re-ordering is a single-edit error in the matching test.
    "e0_t+1_intent_attack",
    "e0_t+1_intent_buff",
    "e0_t+1_intent_debuff",
    "e0_t+1_intent_defend",
    "e0_t+1_intent_other",
    "e0_t+1_dmg_norm",
    "e0_t+1_certainty",
    "e0_t+2_intent_attack",
    "e0_t+2_intent_buff",
    "e0_t+2_intent_debuff",
    "e0_t+2_intent_defend",
    "e0_t+2_intent_other",
    "e0_t+2_dmg_norm",
    "e0_t+2_certainty",
    "e0_t+3_intent_attack",
    "e0_t+3_intent_buff",
    "e0_t+3_intent_debuff",
    "e0_t+3_intent_defend",
    "e0_t+3_intent_other",
    "e0_t+3_dmg_norm",
    "e0_t+3_certainty",
    "e1_t+1_intent_attack",
    "e1_t+1_intent_buff",
    "e1_t+1_intent_debuff",
    "e1_t+1_intent_defend",
    "e1_t+1_intent_other",
    "e1_t+1_dmg_norm",
    "e1_t+1_certainty",
    "e1_t+2_intent_attack",
    "e1_t+2_intent_buff",
    "e1_t+2_intent_debuff",
    "e1_t+2_intent_defend",
    "e1_t+2_intent_other",
    "e1_t+2_dmg_norm",
    "e1_t+2_certainty",
    "e1_t+3_intent_attack",
    "e1_t+3_intent_buff",
    "e1_t+3_intent_debuff",
    "e1_t+3_intent_defend",
    "e1_t+3_intent_other",
    "e1_t+3_dmg_norm",
    "e1_t+3_certainty",
    "e2_t+1_intent_attack",
    "e2_t+1_intent_buff",
    "e2_t+1_intent_debuff",
    "e2_t+1_intent_defend",
    "e2_t+1_intent_other",
    "e2_t+1_dmg_norm",
    "e2_t+1_certainty",
    "e2_t+2_intent_attack",
    "e2_t+2_intent_buff",
    "e2_t+2_intent_debuff",
    "e2_t+2_intent_defend",
    "e2_t+2_intent_other",
    "e2_t+2_dmg_norm",
    "e2_t+2_certainty",
    "e2_t+3_intent_attack",
    "e2_t+3_intent_buff",
    "e2_t+3_intent_debuff",
    "e2_t+3_intent_defend",
    "e2_t+3_intent_other",
    "e2_t+3_dmg_norm",
    "e2_t+3_certainty",
    "e3_t+1_intent_attack",
    "e3_t+1_intent_buff",
    "e3_t+1_intent_debuff",
    "e3_t+1_intent_defend",
    "e3_t+1_intent_other",
    "e3_t+1_dmg_norm",
    "e3_t+1_certainty",
    "e3_t+2_intent_attack",
    "e3_t+2_intent_buff",
    "e3_t+2_intent_debuff",
    "e3_t+2_intent_defend",
    "e3_t+2_intent_other",
    "e3_t+2_dmg_norm",
    "e3_t+2_certainty",
    "e3_t+3_intent_attack",
    "e3_t+3_intent_buff",
    "e3_t+3_intent_debuff",
    "e3_t+3_intent_defend",
    "e3_t+3_intent_other",
    "e3_t+3_dmg_norm",
    "e3_t+3_certainty",
    // 172..192 — bead sts2-rs-regent-eval (2026-07-19 regent-combat-leaf
    // plan, Task 1): Regent-only engine columns. 0.0 on every non-Regent
    // state (stars/Regent-PowerKinds/SovereignBlade never populate for
    // other characters).
    "p_stars_gained_this_turn",
    // 14 player-side Regent power stacks (raw amounts, unnormalized —
    // mirrors the pp_strength/pp_frail convention). See the module-level
    // inclusion-rule note above `player_power_amount` call sites in
    // `eval_features_wide` for the per-power apply-target citations that
    // justify each inclusion/exclusion.
    "pp_black_hole",
    "pp_child_of_the_stars",
    "pp_star_next_turn",
    "pp_arsenal",
    "pp_vigor",
    "pp_furnace",
    "pp_the_sealed_throne",
    "pp_genesis",
    "pp_void_form",
    "pp_seeking_edge",
    "pp_sword_sage",
    "pp_tyranny",
    "pp_monarchs_gaze",
    "pp_reflect",
    // Sovereign Blade forge state (hand+draw+discard).
    "n_blades_norm",
    "blade_total_damage_norm",
    "blade_max_damage_norm",
    // Interaction terms (linear model can't learn products on its own).
    "x_stars_child",
    "x_stars_black_hole",
    // ── 192..216 — Necrobinder engine columns ──
    // Per-enemy-slot Doom: amount (hp_norm /100 scale) + threshold-kill
    // proximity (doom/current_hp clamped to 1.0 — Doom is a standing
    // threshold, not damage: the enemy dies at its own turn end once
    // current_hp <= Doom).
    "e0_doom_norm",
    "e0_doom_kill_prox",
    "e1_doom_norm",
    "e1_doom_kill_prox",
    "e2_doom_norm",
    "e2_doom_kill_prox",
    "e3_doom_norm",
    "e3_doom_kill_prox",
    // Osty investment absolutes — `pet_hp_ratio` (87) is scale-blind:
    // a fresh 3/3 Osty and a built-up 40/40 Osty both read 1.0, yet
    // Unleash keys off current HP, Protector/Sacrifice off MaxHp, and
    // DieForYou soak capacity off current HP.
    "osty_cur_hp_norm",
    "osty_max_hp_norm",
    "osty_alive",
    "osty_block_norm",
    // Per-turn combat-log flags (Rattle hit count / Flatten free-cost;
    // DeathsDoor tripled block).
    "p_osty_attacks_this_turn",
    "p_doom_applied_this_turn",
    // Player-side Necrobinder power stacks (raw amounts, pp_strength
    // convention). See the inclusion-rule note at the engine block in
    // `eval_features_wide`.
    "pp_reaper_form",
    "pp_shroud",
    "pp_necro_mastery",
    "pp_devour_life",
    "pp_haunt",
    "pp_countdown",
    "pp_neurosurge",
    // Soul cards across hand+draw+discard (n_blades_norm spirit).
    "n_souls_norm",
    // Interaction terms (linear model can't learn products on its own).
    "x_devour_life_souls",
    "x_haunt_souls",
];

const _: () = assert!(EVAL_FEATURE_NAMES_WIDE.len() == WIDE_FEATURE_DIM);

/// Per-pile per-CardType counters used by [`eval_features_wide`].
/// Returned by [`count_pile_card_types`] for hand / deck-remaining
/// slices.
#[derive(Default)]
struct PileTypeCounts {
    attack: u32,
    skill: u32,
    power: u32,
    curse: u32,
    status: u32,
}

fn count_pile_card_types(
    state: &CombatState,
    pile: &SmallVec<[crate::CardUid; 32]>,
) -> PileTypeCounts {
    let mut counts = PileTypeCounts::default();
    for uid in pile {
        if let Some(c) = state.cards.iter().find(|c| c.uid == *uid) {
            match c.card_type {
                CardType::Attack => counts.attack += 1,
                CardType::Skill => counts.skill += 1,
                CardType::Power => counts.power += 1,
                CardType::Curse => counts.curse += 1,
                CardType::Status => counts.status += 1,
            }
        }
    }
    counts
}

fn count_hand_card_types(state: &CombatState) -> PileTypeCounts {
    // `hand` is SmallVec<[CardUid; 16]> — the helper takes the
    // wider `SmallVec<[_; 32]>` shape used by draw/discard so we
    // re-collect once. Hot path is not this function (the corpus
    // generator hits eval_features_wide once per player decision,
    // not per rollout step).
    let mut counts = PileTypeCounts::default();
    for uid in &state.player.hand {
        if let Some(c) = state.cards.iter().find(|c| c.uid == *uid) {
            match c.card_type {
                CardType::Attack => counts.attack += 1,
                CardType::Skill => counts.skill += 1,
                CardType::Power => counts.power += 1,
                CardType::Curse => counts.curse += 1,
                CardType::Status => counts.status += 1,
            }
        }
    }
    counts
}

/// Sum the per-instance `amount` for every player Power matching
/// `kind`. Two stacks of Strength 1 + 2 report as `3`, matching the
/// engine's effective-strength arithmetic.
fn player_power_amount(state: &CombatState, kind: PowerKind) -> i32 {
    let Some(uid) = state.player.creature_uid else {
        return 0;
    };
    let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) else {
        return 0;
    };
    c.powers
        .iter()
        .filter_map(|puid| state.powers.get_by_id(puid.0))
        .filter(|p| p.kind == kind)
        .map(|p| p.amount)
        .sum()
}

/// Compute the wide-schema feature vector at `state`.
///
/// Bead `sts2-rs-qctu.1` (schema extension). See module docstring
/// for the column layout. NaN / Inf are never produced — every
/// division falls back to `0.0` on zero denominators (dead enemies,
/// 0-HP edge cases).
/// Scale that maps the wide raw dot-product into tanh's useful range. 1.0 because the
/// fitted weights absorb scale directly (the value-regression tuner fits `tanh(f·w)` to
/// the reference root Q, so no separate divisor is needed). Mirrors `IN_PROGRESS_DIVISOR`.
pub const WIDE_IN_PROGRESS_DIVISOR: f32 = 1.0;

/// Texel-tuned weights for the wide leaf. Positional: index i pairs with
/// `eval_features_wide(state)[i]` / `EVAL_FEATURE_NAMES_WIDE[i]`. Initialized to zero
/// (a neutral leaf) until `scripts/texel_writeback_wide.py` fills them.
pub const EVAL_FEATURE_WEIGHTS_WIDE: [f32; 216] = [
    1.5892386,
    1.5257365,
    2.7541847,
    0.13895534,
    0.17410605,
    0.012901323,
    -0.094340555,
    -0.11693625,
    -0.12131835,
    0.0039171753,
    -0.11432402,
    -0.019985553,
    0.16129693,
    0.0,
    -0.3407278,
    -0.13906541,
    0.04367112,
    -0.01224671,
    0.0,
    -0.29700884,
    0.43144736,
    -0.18357228,
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
    -0.07065305,
    -1.9104327,
    -1.3054028,
    0.56321555,
    0.0,
    0.0,
    0.0,
    0.20332082,
    -0.6597858,
    0.18225703,
    -0.018390398,
    -0.046278123,
    -0.54072267,
    -2.3759222,
    -4.1597295,
    2.0506535,
    0.0,
    0.0,
    0.0,
    1.9447048,
    -0.03995127,
    0.32883313,
    -0.1084217,
    -0.040671684,
    -0.44265813,
    -1.5438492,
    -3.3270795,
    0.9675606,
    0.0,
    0.0,
    0.0,
    0.57866883,
    -0.27847967,
    0.15885991,
    0.19289266,
    -0.25172326,
    -0.53389364,
    -0.7596052,
    -6.0104375,
    0.65317,
    0.0,
    0.0,
    0.0,
    0.430824,
    0.09951596,
    0.16533428,
    -0.0589015,
    0.6235535,
    1.4465522,
    -0.55918074,
    0.08309843,
    -0.640076,
    0.0,
    0.0,
    0.0,
    0.0,
    0.23870356,
    0.18555805,
    -0.12763564,
    0.0,
    0.121905655,
    -0.30556235,
    0.01203876,
    0.14130682,
    -0.40228492,
    -0.62653595,
    -0.21258178,
    0.10856573,
    -0.33473298,
    -0.053466927,
    -0.20849158,
    -0.04499992,
    -0.909296,
    1.0242808,
    0.07571939,
    -0.45939773,
    -0.3358054,
    -0.5251515,
    0.3435136,
    -0.81929123,
    -0.17056166,
    0.32239985,
    -0.1236032,
    0.5621008,
    -0.09713195,
    -0.078186154,
    -0.6824802,
    -0.075744644,
    0.111407,
    -0.4852428,
    0.22440706,
    -0.25039023,
    0.484125,
    -0.89007896,
    0.5100087,
    0.37091607,
    0.053077176,
    0.10687321,
    -0.069341116,
    1.0874159,
    -0.36099067,
    0.15010391,
    -0.14772186,
    -0.5103545,
    -2.3660018,
    -0.8940765,
    1.2211416,
    -1.3436637,
    0.611481,
    0.26571146,
    0.75812715,
    -1.4496999,
    0.3979729,
    0.96395946,
    0.5213354,
    0.021997636,
    0.18421659,
    -0.12613583,
    -2.528022,
    -0.37556458,
    -0.08632152,
    -0.24907865,
    0.13347414,
    0.8259359,
    -0.07890196,
    -4.94669,
    0.0,
    -4.581637,
    -2.015068,
    1.099796,
    0.4345842,
    -0.009905762,
    -5.9709067,
    0.0,
    1.1832101,
    -2.0756493,
    -0.09114983,
    -1.5756602,
    -1.2668262,
    -0.507887,
    0.0,
    4.5971293,
    -0.58881855,
    -0.09114983,
    0.026073912,
    0.123755,
    -0.011902918,
    0.056752466,
    0.12845841,
    0.013218281,
    0.031126307,
    0.0,
    -0.110769734,
    -0.017301546,
    0.5578729,
    -3.0230467,
    1.3104193,
    0.2717296,
    -0.37017554,
    -0.7083702,
    1.2839866,
    0.8304874,
    -0.00013684103,
    0.026637957,
    1.1653422,
    0.6615259,
    1.0390747,
    0.9173135,
    0.36508864,
    0.57197416,
    0.34597084,
    0.6372376,
    0.6806537,
    0.6981763,
    -0.1454864,
    0.0,
    -0.027714707,
    0.065480925,
    0.7374718,
    0.0,
    0.0,
    0.0,
    0.025408063,
    -0.024877558,
    0.438008,
    -0.6315649,
    0.0,
    0.1875989,
];

/// Pure leaf score: `tanh((features · weights) / WIDE_IN_PROGRESS_DIVISOR)`. Bounded in (-1, 1).
#[must_use]
pub fn wide_leaf_score(
    features: &[f32; WIDE_FEATURE_DIM],
    weights: &[f32; WIDE_FEATURE_DIM],
) -> f32 {
    let dot: f32 = features
        .iter()
        .zip(weights.iter())
        .map(|(f, w)| f * w)
        .sum();
    (dot / WIDE_IN_PROGRESS_DIVISOR).tanh()
}

/// Wide-schema in-progress leaf evaluation (the `LeafEval::TexelWide` value —
/// the deployed `eval-cheap` depth-cap leaf).
///
/// The wide feature table carries no potion feature, so a held potion is added
/// here as an explicit `in_progress_potion_weight · potions` term inside the
/// `tanh` — mirroring the narrow [`crate::evaluate_in_progress_raw`] potion term
/// so the deployed leaf prices held potions *during* the fight, not only at win
/// terminals (issue #253). The weight scales with the swept victory knob
/// ([`crate::potion_weight`]); at the deployed default it is `POTIONS_WEIGHT =
/// 2.0 · potions`. Zero potions held ⇒ identical to the pure [`wide_leaf_score`],
/// so this stays byte-identical to the pre-#253 leaf on every potion-free state
/// (all existing wide-eval pin tests use a 0-potion `CombatState::default`).
#[must_use]
pub fn evaluate_in_progress_wide(state: &CombatState) -> f32 {
    let features = eval_features_wide(state);
    let dot: f32 = features
        .iter()
        .zip(EVAL_FEATURE_WEIGHTS_WIDE.iter())
        .map(|(f, w)| f * w)
        .sum();
    let potion_term = crate::in_progress_belt_term(state);
    // HP-repricing knob (2026-07-18 campaign): scale the trained hp_ratio
    // weight (`EVAL_FEATURE_WEIGHTS_WIDE[0]`) by the swept victory knob
    // without touching the Texel table — the extra term is exactly the
    // surplus `(w − 1) · W_hp · hp_ratio`, so the deployed default
    // (`w = 1`) stays byte-identical to the pure dot product.
    let hp_term = features[0] * EVAL_FEATURE_WEIGHTS_WIDE[0] * (crate::hp_weight() - 1.0);
    ((dot + potion_term + hp_term) / WIDE_IN_PROGRESS_DIVISOR).tanh()
}

#[must_use]
pub fn eval_features_wide(state: &CombatState) -> [f32; WIDE_FEATURE_DIM] {
    let mut out = [0.0_f32; WIDE_FEATURE_DIM];

    // ── 0..10 — player scalars ───────────────────────────────────
    let max_hp = state.player.max_hp.max(1) as f32;
    out[0] = (state.player.current_hp.max(0) as f32) / max_hp;
    out[1] = state.player.max_hp as f32 / 100.0;
    out[2] = state.player.block as f32 / max_hp;
    out[3] = f32::from(state.player.energy);
    out[4] = f32::from(state.player.max_energy);
    out[5] = f32::from(state.player.stars);
    out[6] = state.player.hand.len() as f32;
    out[7] = state.player.draw.len() as f32;
    out[8] = state.player.discard.len() as f32;
    out[9] = state.player.exhaust.len() as f32;

    // ── 10..15 — hand card-type counts ───────────────────────────
    let hand_counts = count_hand_card_types(state);
    out[10] = hand_counts.attack as f32;
    out[11] = hand_counts.skill as f32;
    out[12] = hand_counts.power as f32;
    out[13] = hand_counts.curse as f32;
    out[14] = hand_counts.status as f32;

    // ── 15..20 — deck-remaining (draw + discard) ────────────────
    let draw_counts = count_pile_card_types(state, &state.player.draw);
    let discard_counts = count_pile_card_types(state, &state.player.discard);
    out[15] = (draw_counts.attack + discard_counts.attack) as f32;
    out[16] = (draw_counts.skill + discard_counts.skill) as f32;
    out[17] = (draw_counts.power + discard_counts.power) as f32;
    out[18] = (draw_counts.curse + discard_counts.curse) as f32;
    out[19] = (draw_counts.status + discard_counts.status) as f32;

    // ── 20..28 — player Powers ──────────────────────────────────
    out[20] = player_power_amount(state, PowerKind::Strength) as f32;
    out[21] = player_power_amount(state, PowerKind::Dexterity) as f32;
    out[22] = player_power_amount(state, PowerKind::Focus) as f32;
    out[23] = player_power_amount(state, PowerKind::Vulnerable) as f32;
    out[24] = player_power_amount(state, PowerKind::Weak) as f32;
    out[25] = player_power_amount(state, PowerKind::Intangible) as f32;
    out[26] = player_power_amount(state, PowerKind::Thorns) as f32;
    out[27] = player_power_amount(state, PowerKind::Buffer) as f32;

    // ── 28..76 — enemy slots (4 × 12 features) ──────────────────
    for slot in 0..WIDE_ENEMY_SLOTS {
        let base = 28 + slot * FEATURES_PER_ENEMY;
        let Some(uid) = state.enemy_uids.get(slot) else {
            // Slot not occupied — leave zeros (alive_flag stays 0).
            continue;
        };
        let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) else {
            continue;
        };
        let alive = c.current_hp > 0;
        out[base] = if alive { 1.0 } else { 0.0 };
        let emax = c.max_hp.max(1) as f32;
        // out[base + 1] (poison-adjusted hp_ratio) is set after the
        // power scan below, once this enemy's Poison stack is known.
        out[base + 2] = c.max_hp as f32 / 100.0;

        // 3..7: intent kind one-hots (Attack / Buff / Debuff /
        // Defend / Other). MultiAttack rolls into the Attack
        // one-hot; DebuffStrong rolls into Debuff. Sleep / Stun /
        // StatusCard / Unknown / Summon / Heal collapse into
        // "Other" — the spike trains nonlinearly over these so the
        // exact bucket doesn't have to be a leaf decision.
        let (a, b, d, def, other, dmg_norm) = match c.intent {
            Some(Intent::Attack { damage }) => (1.0, 0.0, 0.0, 0.0, 0.0, damage as f32 / 20.0),
            Some(Intent::MultiAttack { damage, hits }) => (
                1.0,
                0.0,
                0.0,
                0.0,
                0.0,
                (damage * i32::from(hits)) as f32 / 20.0,
            ),
            // GH #375. `amount: 0` / `block: 0` are NOT wildcards — per the
            // `Intent` doc-comments (`monsters.rs:2220-2260`) they are the
            // "unspecified / look at the move's effect_fn" SENTINELS left by
            // the pre-`1usm` mechanical migration. Matching them means an
            // intent whose payload has been *audited* falls through to
            // `Some(_)` and classifies as `other`, so refining the monster
            // roster silently degrades this encoding. Measured: `defend`
            // fires on 1 of 1652 live enemy slots, against 41 audited
            // `Intent::Defend { block: <non-zero> }` call sites in content.
            //
            // `intent_one_hot_fix()` binds the payloads instead. Default OFF:
            // the four current-turn `intent_{buff,debuff,defend}` weights are
            // all exactly 0.0 (Texel fitted them against a column that never
            // fires) while `intent_other` is non-zero, so flipping this moves
            // defending enemies from a weighted column to a zero one and
            // changes the deployed leaf by an unmeasured amount. Correct
            // encoding first, re-tune, then flip.
            Some(Intent::Buff {
                kind: PowerKind::Strength,
                amount: 0,
            }) => (0.0, 1.0, 0.0, 0.0, 0.0, 0.0),
            Some(
                Intent::Debuff {
                    kind: PowerKind::Weak,
                    amount: 0,
                }
                | Intent::DebuffStrong,
            ) => (0.0, 0.0, 1.0, 0.0, 0.0, 0.0),
            Some(Intent::Defend { block: 0 }) => (0.0, 0.0, 0.0, 1.0, 0.0, 0.0),
            Some(Intent::Buff { .. }) if crate::intent_one_hot_fix() => {
                (0.0, 1.0, 0.0, 0.0, 0.0, 0.0)
            }
            Some(Intent::Debuff { .. }) if crate::intent_one_hot_fix() => {
                (0.0, 0.0, 1.0, 0.0, 0.0, 0.0)
            }
            Some(Intent::Defend { .. }) if crate::intent_one_hot_fix() => {
                (0.0, 0.0, 0.0, 1.0, 0.0, 0.0)
            }
            Some(_) | None => (0.0, 0.0, 0.0, 0.0, 1.0, 0.0),
        };
        out[base + 3] = a;
        out[base + 4] = b;
        out[base + 5] = d;
        out[base + 6] = def;
        out[base + 7] = other;
        out[base + 8] = dmg_norm;

        // 9..12: enemy Vuln / Weak / Strength scalars.
        let mut vuln = 0i32;
        let mut weak = 0i32;
        let mut strength = 0i32;
        let mut poison = 0i32;
        for puid in &c.powers {
            if let Some(p) = state.powers.get_by_id(puid.0) {
                match p.kind {
                    PowerKind::Vulnerable => vuln += p.amount,
                    PowerKind::Weak => weak += p.amount,
                    PowerKind::Strength => strength += p.amount,
                    PowerKind::Poison => poison += p.amount,
                    _ => {}
                }
            }
        }
        out[base + 9] = vuln as f32;
        out[base + 10] = weak as f32;
        out[base + 11] = strength as f32;

        // hp_ratio (out[base + 1]) treats Poison as pending damage.
        // Poison ticks P at the enemy's turn-end, then decrements by
        // 1, so a stack of P deals P + (P-1) + ... + 1 = P·(P+1)/2
        // eventual HP loss if left to resolve. Folding that triangular
        // total into an EFFECTIVE current HP makes the leaf — and thus
        // the MCTS that maximizes it — treat a big poison stack as a
        // near-dead enemy, which is exactly Silent's superlinear win
        // condition: stacking poison is worth far more than its face
        // value. Capped at current HP (poison can't overkill) and
        // clamped ≥ 0. No-op for enemies with no Poison (poison == 0),
        // so Ironclad and every non-poison position is byte-identical.
        // (sts2-rs Silent combat-eval, GH #194.)
        let poison_pending = i64::from(poison) * i64::from(poison + 1) / 2;
        let cur = i64::from(c.current_hp.max(0));
        let eff_hp = (cur - poison_pending).max(0) as f32;
        out[base + 1] = eff_hp / emax;
    }

    // ── 76..80 — round + flags ──────────────────────────────────
    out[76] = f32::from(state.round) / 30.0;
    out[77] = if state.is_elite { 1.0 } else { 0.0 };
    // Wide-schema includes the same two cumulative counters the
    // Texel chain learned to use (energy_wasted / cards_drawn),
    // normalized so the MLP doesn't have to learn the scale.
    out[78] = state.energy_wasted as f32 / 10.0;
    out[79] = f32::from(state.cards_drawn) / 50.0;

    // ── 80..84 — bead sts2-rs-uuln: missing player Powers ──────
    // The 80-feature schema captured Strength/Dex/Focus/Vulnerable/
    // Weak/Intangible/Thorns/Buffer but left out 4 commonly-applied
    // states that materially shift the value-net's signal.
    out[80] = player_power_amount(state, PowerKind::Frail) as f32;
    out[81] = player_power_amount(state, PowerKind::Poison) as f32;
    out[82] = player_power_amount(state, PowerKind::Constrict) as f32;
    out[83] = player_power_amount(state, PowerKind::Ritual) as f32;

    // ── 84..87 — bead sts2-rs-uuln: pile cost scalars ──────────
    // Cumulative card cost across each pile. Total deck cost is a
    // proxy for "how energy-hungry is this deck" — high-total-cost
    // decks struggle in low-energy states. The hand_avg_cost lets
    // the net see "expensive cards stuck in hand" without needing
    // a per-card-id embedding.
    let hand_total_cost: i32 = state
        .player
        .hand
        .iter()
        .filter_map(|uid| state.cards.iter().find(|c| c.uid == *uid))
        .map(|c| i32::from(c.cost.max(0)))
        .sum();
    let deck_total_cost: i32 = state
        .player
        .draw
        .iter()
        .chain(state.player.discard.iter())
        .filter_map(|uid| state.cards.iter().find(|c| c.uid == *uid))
        .map(|c| i32::from(c.cost.max(0)))
        .sum();
    out[84] = hand_total_cost as f32;
    out[85] = deck_total_cost as f32;
    let hand_size = state.player.hand.len() as f32;
    out[86] = if hand_size > 0.0 {
        hand_total_cost as f32 / hand_size
    } else {
        0.0
    };

    // ── 88..172 — bead sts2-rs-h5nf6: per-enemy peek_intents lookahead ──
    // For each enemy slot, walk K=LOOKAHEAD_TURNS virtual moves via
    // the AI's `peek_intents`, then aggregate per-step:
    //   - intent-kind one-hots: set if ANY intent at that step matches
    //     the bucket (MultiAttack → attack; DebuffStrong → debuff;
    //     anything outside attack/buff/debuff/defend → other).
    //   - dmg_norm: sum of damage*hits across the step's intents, /20.
    //   - certainty: single value (all intents at a step share certainty).
    //
    // Enemies with no AI (player-only fixtures, summoned no-AI
    // placeholders) leave their lookahead block as zeros — that's the
    // signal "we have no information about this slot's future intent".
    for slot in 0..WIDE_ENEMY_SLOTS {
        let Some(uid) = state.enemy_uids.get(slot) else {
            continue;
        };
        let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) else {
            continue;
        };
        let Some(ai) = c.ai.as_ref() else {
            continue;
        };
        let peeks = ai.peek_intents(LOOKAHEAD_TURNS, state);
        let slot_base = LOOKAHEAD_BASE + slot * LOOKAHEAD_FEATURES_PER_ENEMY;
        for p in &peeks {
            let step = p.step as usize;
            if step == 0 || step > LOOKAHEAD_TURNS {
                continue;
            }
            let turn_base = slot_base + (step - 1) * LOOKAHEAD_FEATURES_PER_TURN;
            // Sub-indices 0..5: intent kind one-hots, 5: dmg_norm, 6: certainty.
            let (kind_idx, dmg) = match p.intent {
                Intent::Attack { damage } => (0usize, damage),
                Intent::MultiAttack { damage, hits } => (0, damage * i32::from(hits)),
                Intent::Buff { .. } => (1, 0),
                Intent::Debuff { .. } | Intent::DebuffStrong => (2, 0),
                Intent::Defend { .. } => (3, 0),
                _ => (4, 0),
            };
            out[turn_base + kind_idx] = 1.0;
            out[turn_base + 5] += dmg as f32 / 20.0;
            // certainty: all intents at the same step share the same
            // running_certainty, so writing it repeatedly is idempotent.
            out[turn_base + 6] = p.certainty;
        }
    }

    // ── 87 — bead sts2-rs-uuln: pet Osty HP ratio ──────────────
    // Necrobinder's Osty is a player-aligned ally creature whose
    // survival materially shifts the matchup. 0 if the player has
    // no pet OR the pet is dead.
    out[87] = if let Some(pet_uid) = state.player.pet_uid {
        if let Some(pet) = state.creatures.iter().find(|c| c.id == pet_uid.0) {
            if pet.current_hp > 0 && pet.max_hp > 0 {
                pet.current_hp as f32 / pet.max_hp as f32
            } else {
                0.0
            }
        } else {
            0.0
        }
    } else {
        0.0
    };

    // ── 172..192 — bead sts2-rs-regent-eval: Regent engine columns ──
    // (2026-07-19 regent-combat-leaf plan, Task 1). Every column below
    // reads state that is exclusively Regent's (the stars pipeline,
    // Regent-only `PowerKind`s, `CardId::SovereignBlade`), so this
    // whole block is 0.0 on any non-Regent state.

    // 172 — stars gained this turn (raw, unnormalized like p_stars).
    out[172] = state.player.stars_gained_this_turn as f32;

    // 173..187 — 14 player-side Regent power stacks (raw amounts, same
    // convention as pp_strength / pp_frail). Inclusion rule: player-side
    // (applied via `apply_power_to_player`, verified against
    // `crates/sts2-content/src/cards/regent/*.rs`) AND carries value
    // across at least one turn boundary (persistent buff, a recurring
    // per-turn trigger, or a one-turn-delayed payload). Of the 26
    // Regent-exclusive `PowerKind`s in `powers/regent.rs`, the following
    // are EXCLUDED and why:
    //   - Conqueror, CrushUnder, DyingStar: applied via
    //     `apply_power_to_enemy` (conqueror.rs / crush_under.rs /
    //     dying_star.rs) — these are ENEMY-side debuffs, not player
    //     state, despite the plan's illustrative list naming
    //     DyingStar/Conqueror as examples. Verified by grep against the
    //     actual card implementations, not the (enemy-ambiguous)
    //     doc-comment prose in `powers/regent.rs`.
    //   - Royalties: player-side but combat-inert — its only listener
    //     is `after_combat_victory` (post-fight gold), so it carries no
    //     signal for a leaf that scores in-progress combat.
    //   - HammerTime: player-side and genuinely forge-relevant, but its
    //     effect (re-forge every living teammate) is a no-op in every
    //     corpus we generate (no player-teammate mechanic wired for
    //     solo Regent runs) — dropped to stay near the plan's ~16-20
    //     column budget rather than spend a column on permanent
    //     dead weight.
    //   - Monologue: player-side, but nets to exactly zero net Strength
    //     every turn (grants Strength per card played, then reverts the
    //     *entire* accumulated total at that same turn's end) — any
    //     mid-turn bonus it grants already shows up in `pp_strength`
    //     (out[20]), so a dedicated column adds only the "this will
    //     unwind at EOT" signal, judged not worth the column budget.
    //   - PaleBlueDot, PillarOfCreation, Orbit, ForegoneConclusion,
    //     Parry, SpectrumShift: real, player-side, and pass the
    //     inclusion rule, but trimmed to hit the ~16-20 total-column
    //     target — narrower / lower-magnitude effects than the 14 kept
    //     below.
    out[173] = player_power_amount(state, PowerKind::BlackHole) as f32;
    out[174] = player_power_amount(state, PowerKind::ChildOfTheStars) as f32;
    out[175] = player_power_amount(state, PowerKind::StarNextTurn) as f32;
    out[176] = player_power_amount(state, PowerKind::Arsenal) as f32;
    out[177] = player_power_amount(state, PowerKind::Vigor) as f32;
    out[178] = player_power_amount(state, PowerKind::Furnace) as f32;
    out[179] = player_power_amount(state, PowerKind::TheSealedThrone) as f32;
    out[180] = player_power_amount(state, PowerKind::Genesis) as f32;
    out[181] = player_power_amount(state, PowerKind::VoidForm) as f32;
    out[182] = player_power_amount(state, PowerKind::SeekingEdge) as f32;
    out[183] = player_power_amount(state, PowerKind::SwordSage) as f32;
    out[184] = player_power_amount(state, PowerKind::Tyranny) as f32;
    out[185] = player_power_amount(state, PowerKind::MonarchsGaze) as f32;
    out[186] = player_power_amount(state, PowerKind::Reflect) as f32;

    // 187..190 — Sovereign Blade forge state. Scans hand+draw+discard
    // (a blade played this turn already left the play/exhaust
    // bookkeeping the wide schema doesn't track elsewhere; the three
    // piles the plan names cover every blade the player could still
    // draw or has drawn). Divisors are scale-matching, not hard caps
    // (same spirit as `energy_wasted_norm` /10.0, `cards_drawn_norm`
    // /50.0): a blade's `base_damage` starts at 10 and is raised by
    // `Forge N` per call (`combat_forge` in
    // `crates/sts2-content/src/cards/regent/helpers.rs`), so a
    // heavily-forged late-fight blade can reach ~40+; a multi-blade
    // hand rarely exceeds ~5 copies.
    let mut n_blades: i32 = 0;
    let mut blade_total_damage: i32 = 0;
    let mut blade_max_damage: i32 = 0;
    for uid in state
        .player
        .hand
        .iter()
        .chain(state.player.draw.iter())
        .chain(state.player.discard.iter())
    {
        if let Some(c) = state.cards.iter().find(|c| c.uid == *uid) {
            if c.card_id == crate::cards::CardId::SovereignBlade {
                n_blades += 1;
                blade_total_damage += c.base_damage;
                blade_max_damage = blade_max_damage.max(c.base_damage);
            }
        }
    }
    out[187] = n_blades as f32 / 5.0;
    out[188] = blade_total_damage as f32 / 100.0;
    out[189] = blade_max_damage as f32 / 40.0;

    // 190..192 — stars × power interaction terms. The leaf is a linear
    // model (`tanh(features · weights)`), so it cannot learn the
    // product of two raw columns on its own; these two explicit cross
    // terms let it price "stars currently banked, times how much each
    // stack of Child-of-the-Stars / Black Hole is worth *if* those
    // stars get spent/gained" — the two YAGNI-scoped interactions the
    // plan calls out (stars_norm here is the same raw `p_stars` scalar
    // as out[5]; "_norm" in the plan text is a misnomer since p_stars
    // itself is unnormalized).
    out[190] = out[5] * out[174];
    out[191] = out[5] * out[173];

    // ── 192..216 — Necrobinder engine columns (2026-07-20) ──
    // Mirror of the Regent block above: every column reads state that is
    // exclusively Necrobinder's (Doom stacks, the Osty pet, Necro-only
    // `PowerKind`s, `CardId::Soul`), so this whole block is 0.0 on any
    // non-Necrobinder state.

    // 192..200 — per-enemy-slot Doom. Doom is a standing threshold, not
    // damage: the enemy dies at ITS OWN turn end once current_hp <= Doom
    // (C# `DoomPower.BeforeSideTurnEnd`; no decay, no tick). Without
    // these columns a Doom-40-on-45-HP-boss state scores identically to
    // Doom-0 — the entire Necrobinder win condition was invisible to the
    // leaf. `doom_norm` shares the /100 hp_norm scale; `doom_kill_prox`
    // is doom/current_hp clamped to 1.0 (1.0 = dies at its next turn
    // end).
    for slot in 0..WIDE_ENEMY_SLOTS {
        let base = 192 + slot * 2;
        let Some(uid) = state.enemy_uids.get(slot) else {
            continue;
        };
        let Some(c) = state.creatures.iter().find(|c| c.id == uid.0) else {
            continue;
        };
        if c.current_hp <= 0 {
            continue;
        }
        let doom: i32 = c
            .powers
            .iter()
            .filter_map(|puid| state.powers.get_by_id(puid.0))
            .filter(|p| p.kind == PowerKind::Doom)
            .map(|p| p.amount)
            .sum();
        out[base] = doom as f32 / 100.0;
        out[base + 1] = (doom as f32 / c.current_hp as f32).min(1.0);
    }

    // 200..204 — Osty investment absolutes. `pet_hp_ratio` (87) is
    // scale-blind (fresh 3/3 and built-up 40/40 both read 1.0), yet
    // Unleash damage keys off current HP, Protector / Sacrifice off
    // MaxHp, DieForYou soak capacity off current HP, and summoning
    // compounds MaxHp while death resets it to the next summon amount.
    // All zero while dead: the accumulated investment is lost on death
    // (revive re-bases at the summon amount), and `summon_osty`'s revive
    // branch overwrites max_hp, so a dead pet's recorded max is not
    // recoverable state.
    if let Some(pet) = state
        .player
        .pet_uid
        .and_then(|uid| state.creatures.iter().find(|c| c.id == uid.0))
    {
        if pet.current_hp > 0 {
            out[200] = pet.current_hp as f32 / 50.0;
            out[201] = pet.max_hp as f32 / 50.0;
            out[202] = 1.0;
            out[203] = pet.block as f32 / 20.0;
        }
    }

    // 204..206 — per-turn combat-log flags: Rattle's hit count and
    // Flatten's free-cost read `osty_attacks_this_turn`; DeathsDoor's
    // tripled block reads `doom_applied_by_player_this_turn`.
    out[204] = state.combat_log.osty_attacks_this_turn as f32;
    out[205] = if state.combat_log.doom_applied_by_player_this_turn {
        1.0
    } else {
        0.0
    };

    // 206..213 — player-side Necrobinder power stacks (raw amounts,
    // pp_strength convention). Inclusion rule as in the Regent block:
    // player-side AND carries value across a turn boundary. Excluded:
    //   - SicEmPower, Oblivion: applied to the ENEMY target (SicEm's
    //     re-summon-on-Osty-hit marker, Oblivion's next-card re-Doom) —
    //     enemy-side state, not player state.
    //   - DieForYou: lives on the Osty creature and is constant-1
    //     whenever Osty is alive — fully redundant with `osty_alive`.
    out[206] = player_power_amount(state, PowerKind::ReaperForm) as f32;
    out[207] = player_power_amount(state, PowerKind::Shroud) as f32;
    out[208] = player_power_amount(state, PowerKind::NecroMastery) as f32;
    out[209] = player_power_amount(state, PowerKind::DevourLife) as f32;
    out[210] = player_power_amount(state, PowerKind::Haunt) as f32;
    out[211] = player_power_amount(state, PowerKind::Countdown) as f32;
    out[212] = player_power_amount(state, PowerKind::Neurosurge) as f32;

    // 213 — Soul cards across hand+draw+discard (same pile scan as the
    // Sovereign-Blade block above; /5 scale-match, not a hard cap).
    // Souls are the trigger currency for DevourLife (summon per Soul)
    // and Haunt (unblockable damage per Soul).
    let mut n_souls: i32 = 0;
    for uid in state
        .player
        .hand
        .iter()
        .chain(state.player.draw.iter())
        .chain(state.player.discard.iter())
    {
        if let Some(c) = state.cards.iter().find(|c| c.uid == *uid) {
            if c.card_id == crate::cards::CardId::Soul {
                n_souls += 1;
            }
        }
    }
    out[213] = n_souls as f32 / 5.0;

    // 214..216 — power × souls interaction terms (the linear leaf
    // cannot form products): "how much is each banked Soul worth given
    // the DevourLife / Haunt engines currently in play".
    out[214] = out[209] * out[213];
    out[215] = out[210] * out[213];

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CombatState;

    #[test]
    fn wide_dim_matches_name_count() {
        assert_eq!(EVAL_FEATURE_NAMES_WIDE.len(), WIDE_FEATURE_DIM);
        // The 4-slot × 12-feature enemy block sits at indices 28..76.
        assert_eq!(28 + WIDE_ENEMY_SLOTS * FEATURES_PER_ENEMY, 76);
    }

    #[test]
    fn empty_state_produces_finite_zeros() {
        let s = CombatState::default();
        let v = eval_features_wide(&s);
        assert_eq!(v.len(), WIDE_FEATURE_DIM);
        for (i, x) in v.iter().enumerate() {
            assert!(
                x.is_finite(),
                "feature {i} ({}) = {x} is not finite",
                EVAL_FEATURE_NAMES_WIDE[i]
            );
        }
        // Default state: max_hp=1, current_hp=0 → hp_ratio = 0/1 = 0.
        assert!(v[0].abs() < 1e-9, "p_hp_ratio on default state");
        // No enemies → every alive_flag must be 0.
        for slot in 0..WIDE_ENEMY_SLOTS {
            let base = 28 + slot * FEATURES_PER_ENEMY;
            assert!(v[base].abs() < 1e-9, "e{slot}_alive on default state");
        }
        // round=0 → round_norm=0.
        assert!(v[76].abs() < 1e-9);
    }

    #[test]
    fn hp_ratio_tracks_player_hp() {
        let s = CombatState {
            player: PlayerCombatState {
                max_hp: 80,
                current_hp: 40,
                ..PlayerCombatState::default()
            },
            ..CombatState::default()
        };
        let v = eval_features_wide(&s);
        assert!(
            (v[0] - 0.5).abs() < 1e-6,
            "p_hp_ratio = {} expected ~0.5",
            v[0]
        );
    }

    #[test]
    fn round_norm_tracks_state_round() {
        let s = CombatState {
            round: 6,
            ..CombatState::default()
        };
        let v = eval_features_wide(&s);
        assert!(
            (v[76] - 0.2).abs() < 1e-6,
            "round_norm = {} expected ~0.2",
            v[76]
        );
    }

    #[test]
    fn is_elite_flag_round_trips() {
        let s0 = CombatState::default();
        let v0 = eval_features_wide(&s0);
        assert!(v0[77].abs() < 1e-9);
        let s1 = CombatState {
            is_elite: true,
            is_boss: true,
            act_index: 1,
            ..CombatState::default()
        };
        let v1 = eval_features_wide(&s1);
        assert!((v1[77] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn energy_wasted_and_cards_drawn_normalize() {
        let s = CombatState {
            energy_wasted: 25,
            cards_drawn: 100,
            pending_run_gold_loss: 0,
            last_drawn_card: None,
            ..CombatState::default()
        };
        let v = eval_features_wide(&s);
        assert!((v[78] - 2.5).abs() < 1e-6);
        assert!((v[79] - 2.0).abs() < 1e-6);
    }

    #[test]
    fn column_names_are_unique_and_nonempty() {
        use std::collections::HashSet;
        let mut seen: HashSet<&str> = HashSet::new();
        for name in EVAL_FEATURE_NAMES_WIDE {
            assert!(!name.is_empty(), "empty feature name");
            assert!(seen.insert(name), "duplicate feature name: {name}");
        }
    }

    /// Bead `sts2-rs-uuln`: the +8 feature extension (Frail / Poison /
    /// Constrict / Ritual + 3 pile-cost scalars + pet HP) must be in
    /// the canonical column order and must zero-default on an empty
    /// `CombatState`.
    #[test]
    fn uuln_extension_columns_present_and_default_zero() {
        let s = CombatState::default();
        let v = eval_features_wide(&s);
        // pp_frail / pp_poison / pp_constrict / pp_ritual at 80..84
        assert!(v[80].abs() < 1e-9, "pp_frail default 0");
        assert!(v[81].abs() < 1e-9, "pp_poison default 0");
        assert!(v[82].abs() < 1e-9, "pp_constrict default 0");
        assert!(v[83].abs() < 1e-9, "pp_ritual default 0");
        // pile-cost scalars at 84..87
        assert!(v[84].abs() < 1e-9, "hand_total_cost default 0");
        assert!(v[85].abs() < 1e-9, "deck_total_cost default 0");
        assert!(v[86].abs() < 1e-9, "hand_avg_cost default 0");
        // pet_hp_ratio at 87
        assert!(v[87].abs() < 1e-9, "pet_hp_ratio default 0 (no pet)");
        // Column names match.
        assert_eq!(EVAL_FEATURE_NAMES_WIDE[80], "pp_frail");
        assert_eq!(EVAL_FEATURE_NAMES_WIDE[81], "pp_poison");
        assert_eq!(EVAL_FEATURE_NAMES_WIDE[82], "pp_constrict");
        assert_eq!(EVAL_FEATURE_NAMES_WIDE[83], "pp_ritual");
        assert_eq!(EVAL_FEATURE_NAMES_WIDE[84], "hand_total_cost");
        assert_eq!(EVAL_FEATURE_NAMES_WIDE[85], "deck_total_cost");
        assert_eq!(EVAL_FEATURE_NAMES_WIDE[86], "hand_avg_cost");
        assert_eq!(EVAL_FEATURE_NAMES_WIDE[87], "pet_hp_ratio");
    }

    /// Bead `sts2-rs-h5nf6` (6pzjm.1): the K=3 per-enemy lookahead
    /// block lives at indices 88..172, and its layout
    /// `slot * 21 + (turn - 1) * 7 + sub` must match the static
    /// `EVAL_FEATURE_NAMES_WIDE` table.
    #[test]
    fn h5nf6_lookahead_columns_present_and_named() {
        // The lookahead block itself is a fixed-width sub-range
        // (88..172) regardless of how many columns the Regent
        // extension (sts2-rs-regent-eval) appends after it — assert
        // the block boundary directly instead of the whole-vector
        // `WIDE_FEATURE_DIM` (which the Regent extension legitimately
        // grew past 172).
        assert_eq!(
            LOOKAHEAD_BASE + WIDE_ENEMY_SLOTS * LOOKAHEAD_FEATURES_PER_ENEMY,
            172
        );
        assert_eq!(LOOKAHEAD_BASE, 88);
        assert_eq!(LOOKAHEAD_TURNS, 3);
        assert_eq!(LOOKAHEAD_FEATURES_PER_TURN, 7);
        assert_eq!(LOOKAHEAD_FEATURES_PER_ENEMY, 21);
        assert_eq!(EVAL_FEATURE_NAMES_WIDE[88], "e0_t+1_intent_attack");
        assert_eq!(EVAL_FEATURE_NAMES_WIDE[88 + 6], "e0_t+1_certainty");
        assert_eq!(EVAL_FEATURE_NAMES_WIDE[88 + 7], "e0_t+2_intent_attack");
        assert_eq!(EVAL_FEATURE_NAMES_WIDE[88 + 21], "e1_t+1_intent_attack");
        assert_eq!(EVAL_FEATURE_NAMES_WIDE[171], "e3_t+3_certainty");
        let v = eval_features_wide(&CombatState::default());
        for (i, x) in v
            .iter()
            .enumerate()
            .skip(LOOKAHEAD_BASE)
            .take(WIDE_ENEMY_SLOTS * LOOKAHEAD_FEATURES_PER_ENEMY)
        {
            assert!(x.abs() < 1e-9, "lookahead col {i} default 0");
        }
    }

    /// Smoke test (per bead acceptance): a 50/50 RandomBranch monster
    /// emits `certainty ≈ 0.5` in its t+1 column after the first move
    /// has performed.
    #[test]
    fn h5nf6_random_branch_emits_half_certainty_at_t_plus_1() {
        use crate::monster_ai::{
            MonsterState, MoveRepeatType, MoveState, RandomBranchState, WeightedBranch,
        };
        use crate::monsters::MonsterKind;
        use smallvec::smallvec;
        use std::collections::BTreeMap;

        fn noop(_combat: &mut CombatState, _self_uid: crate::CreatureUid) {}

        let mut states = BTreeMap::new();
        states.insert(
            "root",
            MonsterState::RandomBranch(RandomBranchState {
                id: "root",
                branches: smallvec![
                    WeightedBranch {
                        state_id: "left",
                        repeat_type: MoveRepeatType::CanRepeatForever,
                        max_times: 1,
                        weight: 50.0,
                        cooldown: 0,
                    },
                    WeightedBranch {
                        state_id: "right",
                        repeat_type: MoveRepeatType::CanRepeatForever,
                        max_times: 1,
                        weight: 50.0,
                        cooldown: 0,
                    },
                ],
            }),
        );
        states.insert(
            "left",
            MonsterState::Move(MoveState {
                id: "left",
                effect: noop,
                intents: smallvec![Intent::Attack { damage: 10 }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        states.insert(
            "right",
            MonsterState::Move(MoveState {
                id: "right",
                effect: noop,
                intents: smallvec![Intent::Attack { damage: 4 }],
                follow_up: Some("root"),
                must_perform_once: false,
            }),
        );
        let mut ai = MonsterAi::new(states, "root");
        ai.on_move_performed();

        let mut state = CombatState::default();
        let _uid = state.summon_monster(MonsterKind::Nibbit, 30, ai);

        let v = eval_features_wide(&state);
        assert!(
            (v[88] - 1.0).abs() < 1e-6,
            "e0_t+1_intent_attack expected 1.0, got {}",
            v[88]
        );
        assert!(
            (v[88 + 5] - 0.5).abs() < 1e-6,
            "e0_t+1_dmg_norm expected 0.5 (10/20), got {}",
            v[88 + 5]
        );
        assert!(
            (v[88 + 6] - 0.5).abs() < 1e-6,
            "e0_t+1_certainty expected ~0.5, got {}",
            v[88 + 6]
        );
        assert!(
            (v[88 + 13] - 0.25).abs() < 1e-6,
            "e0_t+2_certainty expected ~0.25, got {}",
            v[88 + 13]
        );
        for slot in 1..WIDE_ENEMY_SLOTS {
            let base = LOOKAHEAD_BASE + slot * LOOKAHEAD_FEATURES_PER_ENEMY;
            for (i, x) in v
                .iter()
                .enumerate()
                .skip(base)
                .take(LOOKAHEAD_FEATURES_PER_ENEMY)
            {
                assert!(x.abs() < 1e-9, "slot {slot} col {i} expected 0");
            }
        }
    }

    #[test]
    fn wide_leaf_score_is_tanh_of_dot() {
        let mut f = [0.0f32; WIDE_FEATURE_DIM];
        let mut w = [0.0f32; WIDE_FEATURE_DIM];
        // dot = 2.0*0.5 + 1.0*1.0 = 2.0  ->  tanh(2.0 / WIDE_IN_PROGRESS_DIVISOR)
        f[0] = 2.0;
        w[0] = 0.5;
        f[1] = 1.0;
        w[1] = 1.0;
        let expected = (2.0f32 / WIDE_IN_PROGRESS_DIVISOR).tanh();
        assert!((wide_leaf_score(&f, &w) - expected).abs() < 1e-6);
    }

    #[test]
    #[allow(clippy::float_cmp)] // exact-by-construction: same fn on the same features.
    fn wide_leaf_is_bounded_and_matches_pure_fn() {
        let s = CombatState::default();
        // Weights are Texel-fit (no longer all-zero); the leaf must stay
        // bounded and the state-level fn must equal the pure fn on the
        // same features + const.
        let v = evaluate_in_progress_wide(&s);
        assert!(v.abs() <= 1.0);
        assert_eq!(
            v,
            wide_leaf_score(&eval_features_wide(&s), &EVAL_FEATURE_WEIGHTS_WIDE)
        );
    }

    /// Bead `sts2-rs-regent-eval` (2026-07-19 regent-combat-leaf plan,
    /// Task 1): the +20 Regent engine columns (stars-gained-this-turn,
    /// 14 player-side Regent power stacks, 3 Sovereign-Blade forge
    /// scalars, 2 stars×power interaction terms) must be in the
    /// canonical column order and must zero-default on a non-Regent
    /// (default) `CombatState` — this is the "byte-identical for other
    /// characters" contract the plan requires before the retune lands.
    #[test]
    fn regent_extension_columns_present_and_default_zero() {
        // 192 (Regent bump) + 24 Necrobinder engine columns (2026-07-20).
        assert_eq!(WIDE_FEATURE_DIM, 216);
        let s = CombatState::default();
        let v = eval_features_wide(&s);
        assert_eq!(v.len(), WIDE_FEATURE_DIM);

        let expected_names = [
            (172, "p_stars_gained_this_turn"),
            (173, "pp_black_hole"),
            (174, "pp_child_of_the_stars"),
            (175, "pp_star_next_turn"),
            (176, "pp_arsenal"),
            (177, "pp_vigor"),
            (178, "pp_furnace"),
            (179, "pp_the_sealed_throne"),
            (180, "pp_genesis"),
            (181, "pp_void_form"),
            (182, "pp_seeking_edge"),
            (183, "pp_sword_sage"),
            (184, "pp_tyranny"),
            (185, "pp_monarchs_gaze"),
            (186, "pp_reflect"),
            (187, "n_blades_norm"),
            (188, "blade_total_damage_norm"),
            (189, "blade_max_damage_norm"),
            (190, "x_stars_child"),
            (191, "x_stars_black_hole"),
        ];
        for (idx, name) in expected_names {
            assert_eq!(
                EVAL_FEATURE_NAMES_WIDE[idx], name,
                "column {idx} name mismatch"
            );
            assert!(
                v[idx].abs() < 1e-9,
                "{name} (col {idx}) default 0, got {}",
                v[idx]
            );
        }
    }

    /// Non-zero values on a constructed Regent-shaped state: stars set
    /// + `stars_gained_this_turn` set, a `BlackHole` power stack on the
    /// player, and two `SovereignBlade` cards (hand + discard) with
    /// known forged `base_damage`. Confirms the extraction formulas
    /// (raw power amounts, blade aggregation + divisors, the two
    /// stars×power interaction terms) match by hand computation.
    #[test]
    fn regent_extension_columns_nonzero_on_regent_state() {
        use crate::card_effects::apply_power_to_creature;
        use crate::cards::{Card, CardId, CardType, TargetType};
        use crate::CardUid;

        let mut s = CombatState::default();
        // `init_player_creature` mirrors current_hp/max_hp at creation
        // time, and `apply_power_to_creature` requires a *living*
        // target (`current_hp > 0`) — set HP before init, or the power
        // applies would silently no-op against a 0-HP creature.
        s.player.max_hp = 50;
        s.player.current_hp = 50;
        let player_uid = s.init_player_creature();
        s.player.stars = 3;
        s.player.stars_gained_this_turn = 5;

        // BlackHole 2 stacks + ChildOfTheStars 4 stacks on the player.
        apply_power_to_creature(&mut s, player_uid, PowerKind::BlackHole, 2);
        apply_power_to_creature(&mut s, player_uid, PowerKind::ChildOfTheStars, 4);

        // Two Sovereign Blade cards: one in hand (base_damage 15), one in
        // discard (base_damage 25) — n_blades=2, total=40, max=25.
        let blade_hand = Card {
            uid: CardUid::new(1).expect("non-zero"),
            card_id: CardId::SovereignBlade,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            base_damage: 15,
            ..Card::pod_defaults()
        };
        let blade_discard = Card {
            uid: CardUid::new(2).expect("non-zero"),
            card_id: CardId::SovereignBlade,
            card_type: CardType::Attack,
            target_type: TargetType::Enemy,
            base_damage: 25,
            ..Card::pod_defaults()
        };
        s.player.hand.push(blade_hand.uid);
        s.player.discard.push(blade_discard.uid);
        s.cards.push(blade_hand);
        s.cards.push(blade_discard);

        let v = eval_features_wide(&s);

        assert!((v[172] - 5.0).abs() < 1e-6, "stars_gained_this_turn");
        assert!((v[173] - 2.0).abs() < 1e-6, "pp_black_hole");
        assert!((v[174] - 4.0).abs() < 1e-6, "pp_child_of_the_stars");
        // No StarNextTurn/Arsenal/etc applied -> stay 0.
        for (idx, x) in v.iter().enumerate().skip(175).take(187 - 175) {
            assert!(x.abs() < 1e-9, "col {idx} expected 0");
        }
        assert!((v[187] - 2.0 / 5.0).abs() < 1e-6, "n_blades_norm");
        assert!(
            (v[188] - 40.0 / 100.0).abs() < 1e-6,
            "blade_total_damage_norm"
        );
        assert!((v[189] - 25.0 / 40.0).abs() < 1e-6, "blade_max_damage_norm");
        // x_stars_child = p_stars(3) * pp_child_of_the_stars(4) = 12.
        assert!((v[190] - 12.0).abs() < 1e-6, "x_stars_child");
        // x_stars_black_hole = p_stars(3) * pp_black_hole(2) = 6.
        assert!((v[191] - 6.0).abs() < 1e-6, "x_stars_black_hole");
    }

    /// Necrobinder engine columns: Doom threshold-kill proximity per
    /// enemy slot, absolute Osty investment scalars (the pre-existing
    /// `pet_hp_ratio` at 87 is scale-blind: a fresh 3/3 Osty and a
    /// built-up 40/40 Osty both read 1.0), the two per-turn combat-log
    /// flags, player-side Necro power stacks, Soul-card count, and the
    /// two power×souls interaction terms.
    #[test]
    fn necrobinder_extension_columns_track_state() {
        use crate::card_effects::apply_power_to_creature;
        use crate::cards::{Card, CardId, CardType, TargetType};
        use crate::{CardUid, MonsterKind};

        let idx = |name: &str| {
            EVAL_FEATURE_NAMES_WIDE
                .iter()
                .position(|n| *n == name)
                .unwrap_or_else(|| panic!("missing wide feature column {name}"))
        };

        // Default state: the whole Necrobinder block must be zeros.
        let v0 = eval_features_wide(&CombatState::default());
        for name in [
            "e0_doom_norm",
            "e0_doom_kill_prox",
            "e3_doom_norm",
            "e3_doom_kill_prox",
            "osty_cur_hp_norm",
            "osty_max_hp_norm",
            "osty_alive",
            "osty_block_norm",
            "p_osty_attacks_this_turn",
            "p_doom_applied_this_turn",
            "pp_reaper_form",
            "pp_shroud",
            "pp_necro_mastery",
            "pp_devour_life",
            "pp_haunt",
            "pp_countdown",
            "pp_neurosurge",
            "n_souls_norm",
            "x_devour_life_souls",
            "x_haunt_souls",
        ] {
            assert!(
                v0[idx(name)].abs() < 1e-9,
                "{name} must be 0 on default state"
            );
        }

        let mut s = CombatState::default();
        s.player.max_hp = 50;
        s.player.current_hp = 50;
        let player_uid = s.init_player_creature();

        // Enemy with Doom 40 at 100/100 HP: doom_norm = 40/100 (hp_norm
        // scale), kill proximity = min(40/100, 1) = 0.4.
        let enemy = s.spawn_enemy(MonsterKind::Nibbit);
        {
            let c = s
                .creatures
                .iter_mut()
                .find(|c| c.id == enemy.0)
                .expect("spawned enemy present");
            c.max_hp = 100;
            c.current_hp = 100;
        }
        apply_power_to_creature(&mut s, enemy, PowerKind::Doom, 40);

        // Osty: summon 10, then damage to 6 and give 3 block.
        s.summon_osty(10);
        {
            let pet_uid = s.player.pet_uid.expect("summon sets pet_uid");
            let c = s
                .creatures
                .iter_mut()
                .find(|c| c.id == pet_uid.0)
                .expect("pet in arena");
            c.current_hp = 6;
            c.block = 3;
        }

        // Per-turn flags.
        s.combat_log.osty_attacks_this_turn = 2;
        s.combat_log.doom_applied_by_player_this_turn = true;

        // Player powers: ReaperForm 1, DevourLife 3, Haunt 2.
        apply_power_to_creature(&mut s, player_uid, PowerKind::ReaperForm, 1);
        apply_power_to_creature(&mut s, player_uid, PowerKind::DevourLife, 3);
        apply_power_to_creature(&mut s, player_uid, PowerKind::Haunt, 2);

        // Two Soul cards (hand + draw): n_souls_norm = 2/5.
        let soul = |uid: u32| Card {
            uid: CardUid::new(uid).expect("non-zero"),
            card_id: CardId::Soul,
            card_type: CardType::Skill,
            target_type: TargetType::None,
            ..Card::pod_defaults()
        };
        s.cards.push(soul(1));
        s.cards.push(soul(2));
        s.player.hand.push(CardUid::new(1).expect("non-zero"));
        s.player.draw.push(CardUid::new(2).expect("non-zero"));

        let v = eval_features_wide(&s);
        assert!((v[idx("e0_doom_norm")] - 0.4).abs() < 1e-6, "e0_doom_norm");
        assert!(
            (v[idx("e0_doom_kill_prox")] - 0.4).abs() < 1e-6,
            "e0_doom_kill_prox at 100 hp"
        );
        assert!(
            (v[idx("osty_cur_hp_norm")] - 6.0 / 50.0).abs() < 1e-6,
            "osty_cur_hp_norm"
        );
        assert!(
            (v[idx("osty_max_hp_norm")] - 10.0 / 50.0).abs() < 1e-6,
            "osty_max_hp_norm"
        );
        assert!((v[idx("osty_alive")] - 1.0).abs() < 1e-6, "osty_alive");
        assert!(
            (v[idx("osty_block_norm")] - 3.0 / 20.0).abs() < 1e-6,
            "osty_block_norm"
        );
        assert!(
            (v[idx("p_osty_attacks_this_turn")] - 2.0).abs() < 1e-6,
            "p_osty_attacks_this_turn"
        );
        assert!(
            (v[idx("p_doom_applied_this_turn")] - 1.0).abs() < 1e-6,
            "p_doom_applied_this_turn"
        );
        assert!(
            (v[idx("pp_reaper_form")] - 1.0).abs() < 1e-6,
            "pp_reaper_form"
        );
        assert!(
            (v[idx("pp_devour_life")] - 3.0).abs() < 1e-6,
            "pp_devour_life"
        );
        assert!((v[idx("pp_haunt")] - 2.0).abs() < 1e-6, "pp_haunt");
        assert!(
            (v[idx("n_souls_norm")] - 2.0 / 5.0).abs() < 1e-6,
            "n_souls_norm"
        );
        // Interactions: power stacks × souls_norm.
        assert!(
            (v[idx("x_devour_life_souls")] - 3.0 * 0.4).abs() < 1e-6,
            "x_devour_life_souls"
        );
        assert!(
            (v[idx("x_haunt_souls")] - 2.0 * 0.4).abs() < 1e-6,
            "x_haunt_souls"
        );

        // Chip the enemy to 30 HP: Doom 40 ≥ 30 ⇒ kill proximity caps
        // at 1.0 (enemy dies at its own next turn end) while doom_norm
        // is unchanged.
        {
            let c = s
                .creatures
                .iter_mut()
                .find(|c| c.id == enemy.0)
                .expect("enemy present");
            c.current_hp = 30;
        }
        let v2 = eval_features_wide(&s);
        assert!(
            (v2[idx("e0_doom_kill_prox")] - 1.0).abs() < 1e-6,
            "doomed-past-threshold proximity must cap at 1.0"
        );
        assert!(
            (v2[idx("e0_doom_norm")] - 0.4).abs() < 1e-6,
            "doom_norm unchanged by enemy hp"
        );

        // Osty dead-but-revivable (current_hp 0, pet_uid still Some):
        // alive flag and cur_hp drop to 0, max_hp investment stays
        // visible (revive resets it, so the leaf must see the loss
        // coming — but while dead the recorded max is what a revive
        // does NOT restore; keep it 0 to mark "investment gone").
        {
            let pet_uid = s.player.pet_uid.expect("pet_uid");
            let c = s
                .creatures
                .iter_mut()
                .find(|c| c.id == pet_uid.0)
                .expect("pet in arena");
            c.current_hp = 0;
        }
        let v3 = eval_features_wide(&s);
        assert!(v3[idx("osty_alive")].abs() < 1e-9, "dead osty alive flag");
        assert!(v3[idx("osty_cur_hp_norm")].abs() < 1e-9, "dead osty cur hp");
        assert!(
            v3[idx("osty_max_hp_norm")].abs() < 1e-9,
            "dead osty max_hp reads 0 (accumulated investment is lost on death)"
        );
    }

    #[test]
    fn enemy_poison_folds_into_effective_hp_ratio() {
        use crate::card_effects::apply_power_to_creature;
        use crate::MonsterKind;

        // Enemy slot 0's hp_ratio lives at index 28 + 1.
        let e0_hp_ratio = 28 + 1;

        // Baseline: full-HP enemy, no poison → hp_ratio == 1.0.
        let mut s = CombatState::default();
        let enemy = s.spawn_enemy(MonsterKind::Nibbit);
        {
            let c = s
                .creatures
                .iter_mut()
                .find(|c| c.id == enemy.0)
                .expect("spawned enemy present");
            c.max_hp = 100;
            c.current_hp = 100;
        }
        let base = eval_features_wide(&s);
        assert!(
            (base[e0_hp_ratio] - 1.0).abs() < 1e-6,
            "no-poison hp_ratio = {} expected 1.0",
            base[e0_hp_ratio]
        );

        // 10 Poison ⇒ triangular pending damage 10·11/2 = 55, so
        // effective HP = 100 − 55 = 45 ⇒ hp_ratio 0.45. The stack is
        // worth far more than its face value (10) — the superlinear
        // signal the leaf needs for Silent poison decks.
        apply_power_to_creature(&mut s, enemy, PowerKind::Poison, 10);
        let poisoned = eval_features_wide(&s);
        assert!(
            (poisoned[e0_hp_ratio] - 0.45).abs() < 1e-6,
            "poisoned hp_ratio = {} expected 0.45 (triangular fold)",
            poisoned[e0_hp_ratio]
        );

        // A lethal-by-poison stack drives effective HP to 0 (capped),
        // never negative: 20 Poison ⇒ 20·21/2 = 210 > 100.
        let mut s2 = CombatState::default();
        let e2 = s2.spawn_enemy(MonsterKind::Nibbit);
        {
            let c = s2
                .creatures
                .iter_mut()
                .find(|c| c.id == e2.0)
                .expect("spawned enemy present");
            c.max_hp = 100;
            c.current_hp = 100;
        }
        apply_power_to_creature(&mut s2, e2, PowerKind::Poison, 20);
        let lethal = eval_features_wide(&s2);
        assert!(
            lethal[e0_hp_ratio].abs() < 1e-6,
            "lethal-poison hp_ratio = {} expected 0.0 (capped)",
            lethal[e0_hp_ratio]
        );
    }

    /// GH #375 fixture: one enemy in slot 0 carrying `intent`.
    fn state_with_enemy_intent(intent: Intent) -> CombatState {
        use crate::{Creature, CreatureUid, IntentHistory, MonsterId};
        use std::num::NonZeroU32;
        let mut s = CombatState::default();
        let id = NonZeroU32::new(100).expect("non-zero");
        s.creatures.push(Creature {
            id,
            current_hp: 40,
            max_hp: 40,
            block: 0,
            powers: smallvec::SmallVec::new(),
            species: None,
            monster_id: MonsterId("Cultist"),
            intent: Some(intent),
            intent_history: IntentHistory::default(),
            ai: None,
        });
        s.enemy_uids = smallvec::smallvec![CreatureUid(id)];
        s
    }

    fn col(name: &str) -> usize {
        EVAL_FEATURE_NAMES_WIDE
            .iter()
            .position(|n| *n == name)
            .unwrap_or_else(|| panic!("missing wide feature column {name}"))
    }

    /// GH #375: pins the DEFECT as the current default, deliberately. An
    /// audited `Intent::Defend { block: 5 }` — the shape that appears 41
    /// times in real monster content — classifies as `other`, not `defend`,
    /// because the match arm tests the literal `block: 0` sentinel.
    ///
    /// This test exists so the default is a recorded decision rather than an
    /// accident: flipping it changes the deployed leaf's feature
    /// distribution and needs a Texel re-tune first (see
    /// [`crate::intent_one_hot_fix`]).
    #[test]
    fn an_audited_defend_intent_still_reads_as_other_by_default() {
        let v = eval_features_wide(&state_with_enemy_intent(Intent::Defend { block: 5 }));
        assert!(
            (v[col("e0_intent_other")] - 1.0).abs() < 1e-9,
            "audited Defend lands in other"
        );
        assert!(v[col("e0_intent_defend")].abs() < 1e-9, "and NOT in defend");

        // The unrefined sentinel still classifies correctly — that is the
        // only shape the current encoding gets right.
        let sentinel = eval_features_wide(&state_with_enemy_intent(Intent::Defend { block: 0 }));
        assert!((sentinel[col("e0_intent_defend")] - 1.0).abs() < 1e-9);
        assert!(sentinel[col("e0_intent_other")].abs() < 1e-9);
    }

    /// GH #375: with the fix enabled, audited payloads bind and classify by
    /// their variant, and the sentinels keep working.
    #[test]
    fn the_intent_one_hot_fix_classifies_audited_payloads() {
        std::env::set_var("STS2_RUNAI_INTENT_ONEHOT_FIX", "1");
        assert!(crate::intent_one_hot_fix());

        for (intent, want) in [
            (Intent::Defend { block: 5 }, "e0_intent_defend"),
            (
                Intent::Buff {
                    kind: PowerKind::Ritual,
                    amount: 3,
                },
                "e0_intent_buff",
            ),
            (
                Intent::Debuff {
                    kind: PowerKind::Vulnerable,
                    amount: 2,
                },
                "e0_intent_debuff",
            ),
        ] {
            let v = eval_features_wide(&state_with_enemy_intent(intent));
            assert!(
                (v[col(want)] - 1.0).abs() < 1e-9,
                "{intent:?} must set {want}"
            );
            assert!(
                v[col("e0_intent_other")].abs() < 1e-9,
                "{intent:?} must leave other"
            );
        }

        // Attacks are unaffected — they always bound their payload correctly.
        let atk = eval_features_wide(&state_with_enemy_intent(Intent::Attack { damage: 20 }));
        assert!((atk[col("e0_intent_attack")] - 1.0).abs() < 1e-9);
        assert!(
            (atk[col("e0_intent_dmg_norm")] - 1.0).abs() < 1e-9,
            "20 damage / 20"
        );
        std::env::remove_var("STS2_RUNAI_INTENT_ONEHOT_FIX");
    }
}
