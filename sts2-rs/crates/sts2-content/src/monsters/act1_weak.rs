// Doc comments use upper-snake-case state names (e.g. `CHOMP`, `STICKY_SHOT`)
// that clippy flags as missing-backticks identifiers. They are DSL state-id
// strings, not Rust paths, so suppress the lint at the module level.
#![allow(clippy::doc_markdown)]

//! Act-1 weak monster ports (bead sts2-rs-05f).
//!
//! Ported from sts2-rl-agent@01c094d:sts2_env/monsters/act1_weak.py
//!
//! Ports the 7 monsters from
//! `sts2-rl-agent@01c094d:sts2_env/monsters/act1_weak.py` (212 LOC) to
//! data-driven `MonsterAi` instances using the DSL skeleton from P5-1
//! (`sts2-rs-gqy`).
//!
//! ## Scope notes
//!
//! - **Effect bodies are no-ops** (same convention as
//!   [`super::legacy_stubs`]). The Rust DSL's `effect: fn(&mut CombatState)`
//!   signature receives `&mut CombatState`, but the helper surface
//!   (`apply_power_to`, `add_card_to_discard`, `gain_block`) lands in
//!   later beads. Intent metadata is preserved so the parity exporter and
//!   AI tests have what they need; the side-effects come online when the
//!   Phase-5 dispatcher wires monster turns end-to-end.
//! - **Return type is `MonsterAi`, not `(Creature, MonsterAi)`.** The
//!   `Creature.kind: MonsterKind` field uses a `#[non_exhaustive]` enum
//!   pinned to the 5 P4-3 stub variants; extending it for every Act-1+
//!   monster cascades through the parity exporter and arena code. The
//!   `monster_id` plumbing in [`crate::monsters::dsl`] lets the dispatcher
//!   identify a monster by its DSL-side string id without needing a
//!   `MonsterKind` variant. The bead spec's `(Creature, MonsterAi)` shape
//!   is therefore deferred until the dispatcher integration bead.
//! - **Conditional-branch start states** (e.g. Nibbit's
//!   IsAlone/IsFront/IsBack predicate) are resolved at call time via the
//!   `is_alone`/`is_front` parameters rather than via a
//!   `ConditionalBranchState`. The Rust DSL's `ConditionFn = fn(&CombatState) -> bool`
//!   is a function pointer with no closure capture, so the
//!   construction-time predicates that Python lambdas capture cannot be
//!   expressed at this pin. The behavior is identical — the resolved
//!   initial state is wired directly.

use std::collections::BTreeMap;

use smallvec::smallvec;
use sts2_core::monster_ai::{
    MonsterAi, MonsterState, MoveRepeatType, MoveState, RandomBranchState, WeightedBranch,
};
use sts2_core::monsters::Intent;
use sts2_core::powers::PowerKind;

fn noop_effect(_: &mut sts2_core::CombatState, _: sts2_core::CreatureUid) {}

/// `Nibbit` HISS_MOVE: self-buff `Strength(2)`. Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/Nibbit.cs:108-112`
/// `HissMove`: `await PowerCmd.Apply<StrengthPower>(base.Creature, HissStrengthGain, ...)`
/// where `HissStrengthGain` is 2 at the pin (3 only at the DeadlyEnemies
/// ascension, which is identity here). The `Intent::Buff { Strength, 2 }` on
/// HISS is telegraph-only (`perform_intent` runs the effect_fn for non-attack
/// intents); the power apply lives here. Previously `noop_effect`, so Nibbit
/// never ramped Strength.
fn nibbit_hiss_effect(state: &mut sts2_core::CombatState, owner: sts2_core::CreatureUid) {
    let amount = sts2_core::ascension::get_value_if_ascension(
        state.ascension,
        sts2_core::ascension::AscensionLevel::DeadlyEnemies,
        3,
        2,
    );
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, amount);
}

/// `Nibbit` SLICE move: gain block on self (alongside the 6-damage hit on
/// the primary `Intent::Attack`). C# `Nibbit.cs` SLICE_MOVE carries two intents
/// (`SingleAttackIntent(6)` + `DefendIntent()`) and runs
/// `CreatureCmd.GainBlock(self, SliceBlock)`. `perform_intent` acts on
/// `intents.first()` only (combat_loop.rs:1903-1908), so any secondary
/// `Intent::Defend` would be dropped — the block has to live in `effect_fn`.
/// Previously `noop_effect`, so the block silently vanished. Faithfulness-
/// audit fix 2026-05-28 (bead `sts2-rs-5i8yj`).
///
/// `SliceBlock` scales with ascension: C# `Nibbit.cs:29`
/// `SliceBlock => AscensionHelper.GetValueIfAscension(AscensionLevel.ToughEnemies, 6, 5)`
/// — 6 block at A8+ (ToughEnemies), 5 below. The earlier port hardcoded 5,
/// so at A8/A9 the Nibbit absorbed one less point of player damage and ended
/// 1 HP lower than the oracle (surfaced by the first C#-oracle ascension
/// sweep). Mirrors the sibling `nibbit_hiss_effect`, which already scales.
fn nibbit_slice_effect(state: &mut sts2_core::CombatState, owner: sts2_core::CreatureUid) {
    let amount = sts2_core::ascension::get_value_if_ascension(
        state.ascension,
        sts2_core::ascension::AscensionLevel::ToughEnemies,
        6,
        5,
    );
    sts2_core::hooks::apply_effects(
        state,
        &[sts2_core::hooks::Effect::GainBlockCreature {
            target: sts2_core::hooks::CreatureRef::Enemy(owner),
            amount,
        }],
    );
}

/// `FuzzyWurmCrawler` INHALE move: apply `Strength(7)` to self. Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/FuzzyWurmCrawler.cs:78-84`
/// `Inhale`: `await PowerCmd.Apply<StrengthPower>(base.Creature, 7m, base.Creature, null)`.
/// The `Intent::Buff { Strength, 7 }` on the INHALE state is telegraph-only
/// (`perform_intent` runs the effect_fn for non-attack intents); the actual
/// power apply lives here. Previously `noop_effect`, so the wurm never
/// ramped Strength — flagged by parity-fuzz `monsters[*].powers.len` drift.
fn fuzzy_wurm_inhale_effect(state: &mut sts2_core::CombatState, owner: sts2_core::CreatureUid) {
    sts2_core::card_effects::apply_power_to_creature(state, owner, PowerKind::Strength, 7);
}

/// Bead `sts2-rs-fqcx0`: the slimes' sticky/goop moves add `Slimed` status
/// cards to the player's discard pile. Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/`
/// `{LeafSlimeS,LeafSlimeM,TwigSlimeM}.cs`:
/// `await CardPileCmd.AddToCombatAndPreview<Slimed>(targets, PileType.Discard, N, addedByPlayer: false)`
/// (LeafSlimeS GOOP `StatusIntent(1)`; LeafSlimeM STICKY_SHOT `StatusIntent(2)`;
/// TwigSlimeM STICKY_SHOT `StatusIntent(1)`). The `Intent::StatusCard` on those
/// states is telegraph-only (`perform_intent` only runs the effect_fn for
/// non-attack intents); the card add must live in the effect body. Previously
/// `noop_effect`, so the slimes never gunked the deck — surfaced by parity-fuzz
/// `discard_count` drift on FLYCONID_NORMAL (Flyconid + a medium slime), where
/// C# turn-0 discard carried an extra `SLIMED` that Rust lacked.
fn slime_goop_one_effect(state: &mut sts2_core::CombatState, _owner: sts2_core::CreatureUid) {
    use sts2_core::card_effects::PileKind;
    let _ = crate::cards::status::spawn_slimed(state, PileKind::Discard);
}

fn slime_goop_two_effect(state: &mut sts2_core::CombatState, _owner: sts2_core::CreatureUid) {
    use sts2_core::card_effects::PileKind;
    for _ in 0..2 {
        let _ = crate::cards::status::spawn_slimed(state, PileKind::Discard);
    }
}

/// Bead `sts2-rs-rnzuw` (o1mwv C.4): ShrinkerBeetle SHRINK_MOVE applies
/// `ShrinkPower(-1)` to the player. Mirrors C#
/// `../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Monsters/ShrinkerBeetle.cs:43-49`
/// `ShrinkMove`: `await PowerCmd.Apply<ShrinkPower>(targets, -1m, base.Creature, null)`.
///
/// `ShrinkPower` is `AllowNegative` and infinite at `amount < 0`
/// (StackType `Single`): it persists at -1 and does NOT tick down, and a
/// second application does not accumulate. The trailing
/// `Intent::DebuffStrong` in the move is telegraph-only (`perform_intent`
/// runs the effect_fn for non-attack intents); the application lives
/// here. The -30% powered-attack damage effect lives in
/// `hooks::modify_damage_multiplicative` (`PowerKind::Shrink` arm).
///
/// Applies via the legacy `state.player.powers` surface — `state_to_dict`
/// (and the parity harness) read player powers there, not from the d03
/// creature mirror (same rationale as the Flyconid/Fogmog player-debuff
/// helpers in `act1_overgrowth`).
fn shrinker_beetle_shrink_effect(
    state: &mut sts2_core::CombatState,
    owner: sts2_core::CreatureUid,
) {
    // Infinite (Single) stack: leave an existing instance untouched.
    let already = state.player.powers.iter().any(|puid| {
        state
            .powers
            .iter()
            .any(|p| p.id == puid.0 && p.kind == PowerKind::Shrink)
    });
    if already {
        return;
    }
    let next_id_raw = state
        .powers
        .iter()
        .map(|p| p.id.get())
        .max()
        .map_or(1, |m| m.saturating_add(1));
    let new_id = std::num::NonZeroU32::new(next_id_raw).expect("power id non-zero");
    state.powers.push(sts2_core::Power {
        id: new_id,
        kind: PowerKind::Shrink,
        amount: -1,
        counter: 0,
        // C# `ShrinkPower.AfterDeath` (ShrinkPower.cs:71-73) removes the
        // Shrink when its applier dies (`creature == base.Applier`), exactly
        // like Constrict. Stamp the beetle as applier so
        // `remove_shrink_from_dead_applier` can drop it on the beetle's death
        // (bead `sts2-rs-creca`). Previously `None`, so the player kept a
        // corpse's Shrink forever (OMNISLICE seed 1293 player.powers.len rs=1
        // vs cs=0).
        applier: Some(owner),
        skip_next_tick: false,
    });
    state.player.powers.push(sts2_core::PowerUid(new_id));
}

/// `ShrinkerBeetle` — cycle: SHRINK → CHOMP → STOMP → CHOMP → STOMP …
pub fn create_shrinker_beetle_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "SHRINK",
        MonsterState::Move(MoveState {
            id: "SHRINK",
            effect: shrinker_beetle_shrink_effect,
            intents: smallvec![Intent::DebuffStrong],
            follow_up: Some("CHOMP"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "CHOMP",
        MonsterState::Move(MoveState {
            id: "CHOMP",
            effect: noop_effect,
            intents: smallvec![Intent::Attack { damage: 7 }],
            follow_up: Some("STOMP"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "STOMP",
        MonsterState::Move(MoveState {
            id: "STOMP",
            effect: noop_effect,
            intents: smallvec![Intent::Attack { damage: 13 }],
            follow_up: Some("CHOMP"),
            must_perform_once: false,
        }),
    );
    MonsterAi::new(s, "SHRINK")
}

/// `FuzzyWurmCrawler` — FIRST_ACID_GOOP → INHALE → ACID_GOOP → FIRST_ACID_GOOP …
///
/// Ascension-blind: AcidGoop's base damage 4 is encoded here; the
/// `AcidGoopDamage => GetValueIfAscension(DeadlyEnemies, 6, 4)` bump to 6
/// (ascension >= 9) is applied post-build via the
/// `ascension_scaling::ASCENSION_ATTACK_SCALING` table. Inhale's
/// `Strength(7)` is not ascension-scaled in C#.
pub fn create_fuzzy_wurm_crawler_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "FIRST_ACID_GOOP",
        MonsterState::Move(MoveState {
            id: "FIRST_ACID_GOOP",
            effect: noop_effect,
            intents: smallvec![Intent::Attack { damage: 4 }],
            follow_up: Some("INHALE"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "INHALE",
        MonsterState::Move(MoveState {
            id: "INHALE",
            effect: fuzzy_wurm_inhale_effect,
            intents: smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 7
            }],
            follow_up: Some("ACID_GOOP"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "ACID_GOOP",
        MonsterState::Move(MoveState {
            id: "ACID_GOOP",
            effect: noop_effect,
            intents: smallvec![Intent::Attack { damage: 4 }],
            follow_up: Some("FIRST_ACID_GOOP"),
            must_perform_once: false,
        }),
    );
    MonsterAi::new(s, "FIRST_ACID_GOOP")
}

/// `Nibbit` — BUTT(12) → SLICE(6+5blk) → HISS(Str+2) → BUTT … The
/// `is_alone` / `is_front` flags resolve the initial state at
/// construction time (Python uses a `ConditionalBranchState` of lambdas;
/// our `ConditionFn` is a function pointer that cannot capture them).
pub fn create_nibbit_ai(is_alone: bool, is_front: bool) -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "BUTT",
        MonsterState::Move(MoveState {
            id: "BUTT",
            effect: noop_effect,
            intents: smallvec![Intent::Attack { damage: 12 }],
            follow_up: Some("SLICE"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "SLICE",
        MonsterState::Move(MoveState {
            id: "SLICE",
            // 5 block applied in `nibbit_slice_effect` because `perform_intent`
            // dispatches only the primary attack intent (bead `sts2-rs-5i8yj`).
            // The Defend is the telegraph's second intent (`Nibbit.cs:72`).
            effect: nibbit_slice_effect,
            intents: smallvec![Intent::Attack { damage: 6 }, Intent::Defend { block: 5 }],
            follow_up: Some("HISS"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "HISS",
        MonsterState::Move(MoveState {
            id: "HISS",
            effect: nibbit_hiss_effect,
            intents: smallvec![Intent::Buff {
                kind: PowerKind::Strength,
                amount: 2
            }],
            follow_up: Some("BUTT"),
            must_perform_once: false,
        }),
    );
    let initial = if is_alone {
        "BUTT"
    } else if is_front {
        "SLICE"
    } else {
        "HISS"
    };
    MonsterAi::new(s, initial)
}

/// `LeafSlimeS` — random BUTT(3) or GOOP(slime), CannotRepeat.
pub fn create_leaf_slime_s_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "RANDOM",
        MonsterState::RandomBranch(RandomBranchState {
            id: "RANDOM",
            branches: smallvec![
                WeightedBranch {
                    state_id: "BUTT",
                    repeat_type: MoveRepeatType::CannotRepeat,
                    max_times: 1,
                    weight: 1.0,
                    cooldown: 0,
                },
                WeightedBranch {
                    state_id: "GOOP",
                    repeat_type: MoveRepeatType::CannotRepeat,
                    max_times: 1,
                    weight: 1.0,
                    cooldown: 0,
                },
            ],
        }),
    );
    s.insert(
        "BUTT",
        MonsterState::Move(MoveState {
            id: "BUTT",
            effect: noop_effect,
            intents: smallvec![Intent::Attack { damage: 3 }],
            follow_up: Some("RANDOM"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "GOOP",
        MonsterState::Move(MoveState {
            id: "GOOP",
            effect: slime_goop_one_effect,
            intents: smallvec![Intent::StatusCard],
            follow_up: Some("RANDOM"),
            must_perform_once: false,
        }),
    );
    MonsterAi::new(s, "RANDOM")
}

/// `TwigSlimeS` — BUTT(4) self-loop.
pub fn create_twig_slime_s_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "BUTT",
        MonsterState::Move(MoveState {
            id: "BUTT",
            effect: noop_effect,
            intents: smallvec![Intent::Attack { damage: 4 }],
            follow_up: Some("BUTT"),
            must_perform_once: false,
        }),
    );
    MonsterAi::new(s, "BUTT")
}

/// `LeafSlimeM` — STICKY_SHOT(2 Slimed) ↔ CLUMP_SHOT(8).
pub fn create_leaf_slime_m_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "STICKY_SHOT",
        MonsterState::Move(MoveState {
            id: "STICKY_SHOT",
            effect: slime_goop_two_effect,
            intents: smallvec![Intent::StatusCard],
            follow_up: Some("CLUMP_SHOT"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "CLUMP_SHOT",
        MonsterState::Move(MoveState {
            id: "CLUMP_SHOT",
            effect: noop_effect,
            intents: smallvec![Intent::Attack { damage: 8 }],
            follow_up: Some("STICKY_SHOT"),
            must_perform_once: false,
        }),
    );
    MonsterAi::new(s, "STICKY_SHOT")
}

/// `TwigSlimeM` — STICKY_SHOT → Random(CLUMP_SHOT[max 2] | STICKY_SHOT[CannotRepeat]).
pub fn create_twig_slime_m_ai() -> MonsterAi {
    let mut s = BTreeMap::new();
    s.insert(
        "RANDOM",
        MonsterState::RandomBranch(RandomBranchState {
            id: "RANDOM",
            branches: smallvec![
                WeightedBranch {
                    state_id: "CLUMP_SHOT",
                    repeat_type: MoveRepeatType::CanRepeatXTimes,
                    max_times: 2,
                    weight: 1.0,
                    cooldown: 0,
                },
                WeightedBranch {
                    state_id: "STICKY_SHOT",
                    repeat_type: MoveRepeatType::CannotRepeat,
                    max_times: 1,
                    weight: 1.0,
                    cooldown: 0,
                },
            ],
        }),
    );
    s.insert(
        "STICKY_SHOT",
        MonsterState::Move(MoveState {
            id: "STICKY_SHOT",
            effect: slime_goop_one_effect,
            intents: smallvec![Intent::StatusCard],
            follow_up: Some("RANDOM"),
            must_perform_once: false,
        }),
    );
    s.insert(
        "CLUMP_SHOT",
        MonsterState::Move(MoveState {
            id: "CLUMP_SHOT",
            effect: noop_effect,
            intents: smallvec![Intent::Attack { damage: 11 }],
            follow_up: Some("RANDOM"),
            must_perform_once: false,
        }),
    );
    MonsterAi::new(s, "STICKY_SHOT")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrinker_beetle_cycle() {
        let mut ai = create_shrinker_beetle_ai();
        assert_eq!(ai.current_move().id, "SHRINK");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "CHOMP");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(1).id, "STOMP");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(2).id, "CHOMP");
    }

    /// Bead `sts2-rs-rnzuw` (o1mwv C.4): SHRINK applies ShrinkPower(-1) to
    /// the player (C# `ShrinkMove` → `PowerCmd.Apply<ShrinkPower>(targets,
    /// -1m, ...)`), via the legacy `state.player.powers` surface. Infinite
    /// (Single) stack: a second application does not accumulate.
    #[test]
    fn shrinker_beetle_shrink_effect_applies_shrink_minus_one_to_player() {
        use sts2_core::{CombatState, CreatureUid, PowerKind};

        let mut state = CombatState::default();
        // Owner uid is irrelevant — the effect targets the player.
        shrinker_beetle_shrink_effect(&mut state, CreatureUid::new(1).expect("nz"));

        let shrink_amount = |s: &CombatState| -> Option<i32> {
            s.player.powers.iter().find_map(|puid| {
                s.powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == PowerKind::Shrink)
                    .map(|p| p.amount)
            })
        };
        assert_eq!(
            shrink_amount(&state),
            Some(-1),
            "player must gain ShrinkPower at amount -1 (infinite)"
        );

        // Re-apply: infinite stack does not accumulate, stays a single -1.
        shrinker_beetle_shrink_effect(&mut state, CreatureUid::new(1).expect("nz"));
        let count = state
            .player
            .powers
            .iter()
            .filter(|puid| {
                state
                    .powers
                    .iter()
                    .any(|p| p.id == puid.0 && p.kind == PowerKind::Shrink)
            })
            .count();
        assert_eq!(count, 1, "ShrinkPower(-1) must not stack a second instance");
        assert_eq!(shrink_amount(&state), Some(-1));
    }

    #[test]
    fn fuzzy_wurm_inhale_effect_applies_seven_strength_to_self() {
        use sts2_core::{CombatState, MonsterKind, PowerKind};

        let mut state = CombatState::default();
        let uid = state.spawn_enemy(MonsterKind::FuzzyWurmCrawler);
        fuzzy_wurm_inhale_effect(&mut state, uid);

        let creature = state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .expect("spawned wurm");
        let strength: i32 = creature
            .powers
            .iter()
            .filter_map(|puid| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == PowerKind::Strength)
                    .map(|p| p.amount)
            })
            .sum();
        assert_eq!(
            strength, 7,
            "INHALE must grant the wurm +7 Strength (C# Inhale)"
        );
    }

    /// AcidGoop damage scales via the `ascension_scaling` table applied in
    /// `build_ai_with_ascension`. C# `FuzzyWurmCrawler.cs`
    /// `AcidGoopDamage => GetValueIfAscension(DeadlyEnemies, 6, 4)` —
    /// DeadlyEnemies activates at ascension >= 9.
    #[test]
    fn fuzzy_wurm_crawler_acid_goop_scales_at_ascension_9() {
        use crate::monster_dispatch::{build_ai_with_ascension, MonsterSpawnArgs, SpawnPosition};
        use sts2_core::{Intent, MonsterKind};
        let acid_damage = |asc: u8| {
            let ai = build_ai_with_ascension(
                MonsterKind::FuzzyWurmCrawler,
                MonsterSpawnArgs::None,
                SpawnPosition::default(),
                asc,
            );
            match ai.current_move().intents[0] {
                Intent::Attack { damage } => damage,
                other => panic!("FIRST_ACID_GOOP must be an Attack intent, got {other:?}"),
            }
        };
        assert_eq!(acid_damage(0), 4, "A0 base AcidGoop damage");
        assert_eq!(acid_damage(8), 4, "A8 below DeadlyEnemies threshold");
        assert_eq!(acid_damage(9), 6, "A9 activates DeadlyEnemies");
        assert_eq!(acid_damage(10), 6, "A10 keeps scaled damage");
    }

    #[test]
    fn fuzzy_wurm_crawler_initial_then_inhale_then_loop() {
        let mut ai = create_fuzzy_wurm_crawler_ai();
        assert_eq!(ai.current_move().id, "FIRST_ACID_GOOP");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(0).id, "INHALE");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(1).id, "ACID_GOOP");
        ai.on_move_performed();
        assert_eq!(ai.roll_move_seeded(2).id, "FIRST_ACID_GOOP");
    }

    /// Bead `sts2-rs-pdsdl`: Nibbit HISS applies +2 Strength to self (C#
    /// `HissMove` → `PowerCmd.Apply<StrengthPower>(self, HissStrengthGain=2)`).
    /// At DeadlyEnemies (ascension >= 9), scales to +3.
    #[test]
    fn nibbit_hiss_effect_applies_two_strength_to_self() {
        use sts2_core::{CombatState, MonsterKind, PowerKind};

        let mut state = CombatState::default();
        let uid = state.spawn_enemy(MonsterKind::Nibbit);
        nibbit_hiss_effect(&mut state, uid);

        let creature = state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .expect("spawned nibbit");
        let strength: i32 = creature
            .powers
            .iter()
            .filter_map(|puid| {
                state
                    .powers
                    .iter()
                    .find(|p| p.id == puid.0 && p.kind == PowerKind::Strength)
                    .map(|p| p.amount)
            })
            .sum();
        assert_eq!(
            strength, 2,
            "HISS must grant Nibbit +2 Strength (C# HissMove, A0)"
        );
    }

    /// Nibbit HISS Strength scales to +3 at DeadlyEnemies (ascension >= 9).
    #[test]
    fn nibbit_hiss_effect_scales_strength_at_deadly_enemies() {
        use sts2_core::{CombatState, MonsterKind, PowerKind};

        let strength_at_ascension = |asc: u8| {
            let mut state = CombatState {
                ascension: asc,
                ..CombatState::default()
            };
            let uid = state.spawn_enemy(MonsterKind::Nibbit);
            nibbit_hiss_effect(&mut state, uid);

            let creature = state
                .creatures
                .iter()
                .find(|c| c.id == uid.0)
                .expect("spawned nibbit");
            creature
                .powers
                .iter()
                .filter_map(|puid| {
                    state
                        .powers
                        .iter()
                        .find(|p| p.id == puid.0 && p.kind == PowerKind::Strength)
                        .map(|p| p.amount)
                })
                .sum::<i32>()
        };

        assert_eq!(strength_at_ascension(0), 2, "A0 base strength");
        assert_eq!(
            strength_at_ascension(8),
            2,
            "A8 below DeadlyEnemies threshold"
        );
        assert_eq!(strength_at_ascension(9), 3, "A9 activates DeadlyEnemies");
        assert_eq!(strength_at_ascension(10), 3, "A10 keeps scaled strength");
    }

    #[test]
    fn nibbit_initial_state_resolves() {
        assert_eq!(create_nibbit_ai(true, false).current_move().id, "BUTT");
        assert_eq!(create_nibbit_ai(false, true).current_move().id, "SLICE");
        assert_eq!(create_nibbit_ai(false, false).current_move().id, "HISS");
    }

    #[test]
    fn leaf_slime_s_alternates() {
        let mut ai = create_leaf_slime_s_ai();
        let mut prev = ai.current_move().id;
        for seed in 0..50 {
            ai.on_move_performed();
            let m = ai.roll_move_seeded(seed);
            if m.id == "RANDOM" {
                continue;
            }
            assert_ne!(m.id, prev, "seed {seed} repeated {prev}");
            prev = m.id;
        }
    }

    #[test]
    fn twig_slime_s_self_loop() {
        let mut ai = create_twig_slime_s_ai();
        for seed in 0..10 {
            assert_eq!(ai.current_move().id, "BUTT");
            ai.on_move_performed();
            ai.roll_move_seeded(seed);
        }
    }

    #[test]
    fn leaf_slime_m_strict_alternation() {
        let mut ai = create_leaf_slime_m_ai();
        for seed in 0..6 {
            let expected = if seed % 2 == 0 {
                "STICKY_SHOT"
            } else {
                "CLUMP_SHOT"
            };
            assert_eq!(ai.current_move().id, expected, "seed {seed}");
            ai.on_move_performed();
            ai.roll_move_seeded(seed);
        }
    }

    #[test]
    fn twig_slime_m_no_three_clumps_no_two_sticky() {
        let mut ai = create_twig_slime_m_ai();
        let mut clump_run = 0u32;
        let mut last = ai.current_move().id;
        for seed in 0..300 {
            ai.on_move_performed();
            let m = ai.roll_move_seeded(seed);
            if m.id == "CLUMP_SHOT" {
                clump_run += 1;
                assert!(clump_run <= 2, "3 CLUMP_SHOTs in a row at seed {seed}");
            } else {
                clump_run = 0;
            }
            if m.id == "STICKY_SHOT" {
                assert_ne!(last, "STICKY_SHOT", "STICKY_SHOT repeated at seed {seed}");
            }
            last = m.id;
        }
    }

    /// Bead `sts2-rs-fqcx0`: LeafSlimeM STICKY_SHOT adds exactly 2
    /// `Slimed` cards to the player's discard (C# `StatusIntent(2)` /
    /// `AddToCombatAndPreview<Slimed>(…, Discard, 2, …)`).
    #[test]
    fn leaf_slime_m_sticky_shot_adds_two_slimed() {
        use sts2_core::cards::CardId;
        use sts2_core::{CombatState, CreatureUid};

        let mut state = CombatState::default();
        let pre = state.player.discard.len();
        slime_goop_two_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(state.player.discard.len(), pre + 2);
        for uid in &state.player.discard {
            let idx = uid.0.get() as usize - 1;
            assert_eq!(state.cards[idx].card_id, CardId::Slimed);
        }
    }

    /// Bead `sts2-rs-fqcx0`: TwigSlimeM STICKY_SHOT and LeafSlimeS GOOP
    /// each add exactly 1 `Slimed` card (C# `StatusIntent(1)`).
    #[test]
    fn slime_goop_one_adds_single_slimed() {
        use sts2_core::cards::CardId;
        use sts2_core::{CombatState, CreatureUid};

        let mut state = CombatState::default();
        let pre = state.player.discard.len();
        slime_goop_one_effect(&mut state, CreatureUid::new(1).expect("nz"));
        assert_eq!(state.player.discard.len(), pre + 1);
        let uid = *state.player.discard.last().expect("one card");
        let idx = uid.0.get() as usize - 1;
        assert_eq!(state.cards[idx].card_id, CardId::Slimed);
    }

    /// Mutation coverage (issue #65): `nibbit_slice_effect` (act1_weak:86)
    /// grants the Nibbit 5 block (SliceBlock, non-ascension). Kills the
    /// `replace nibbit_slice_effect with ()` mutant.
    #[test]
    fn nibbit_slice_effect_grants_five_block_to_self() {
        use sts2_core::{CombatState, MonsterKind};

        let mut state = CombatState::default();
        let owner = state.spawn_enemy(MonsterKind::Nibbit);
        let block_of = |state: &CombatState| {
            state
                .creatures
                .iter()
                .find(|c| c.id == owner.0)
                .map_or(-1, |c| c.block)
        };
        assert_eq!(block_of(&state), 0, "no block before SLICE");
        nibbit_slice_effect(&mut state, owner);
        assert_eq!(
            block_of(&state),
            5,
            "SLICE grants 5 block to self (C# SliceBlock, non-ascension)"
        );
    }

    /// Mutation coverage (issue #65): `shrinker_beetle_shrink_effect`'s
    /// already-present guard (act1_weak:162) `p.id == puid.0 && p.kind ==
    /// Shrink` must ignore a player decoy power of another kind. With only a
    /// Vulnerable decoy present (no Shrink), SHRINK still applies a fresh
    /// Shrink(-1). Kills the `&&`→`||` mutant — under `||` the decoy's
    /// id-match would falsely report Shrink already present and skip it.
    #[test]
    fn shrinker_beetle_shrink_applies_past_decoy_player_power() {
        use sts2_core::{CombatState, CreatureUid, PowerKind, PowerUid};

        let mut state = CombatState::default();
        // Seed a player decoy power of a DIFFERENT kind (Vulnerable).
        let decoy_id = std::num::NonZeroU32::new(1).expect("nz");
        state.powers.push(sts2_core::Power {
            id: decoy_id,
            kind: PowerKind::Vulnerable,
            amount: 2,
            counter: 0,
            applier: None,
            skip_next_tick: false,
        });
        state.player.powers.push(PowerUid(decoy_id));

        shrinker_beetle_shrink_effect(&mut state, CreatureUid::new(1).expect("nz"));

        let shrink_amount = state.player.powers.iter().find_map(|pu| {
            state
                .powers
                .iter()
                .find(|p| p.id == pu.0 && p.kind == PowerKind::Shrink)
                .map(|p| p.amount)
        });
        assert_eq!(
            shrink_amount,
            Some(-1),
            "Shrink applied despite a different-kind decoy player power"
        );
    }
}
