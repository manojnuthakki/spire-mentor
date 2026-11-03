//! Misery — Necrobinder rare attack.
//! C# source:
//! ../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Cards/Misery.cs
use super::prelude::*;

/// Misery (Rare, 0 energy, Attack). Snapshot the target's debuffs, deal 7
/// damage to it, then copy each of those debuffs (by amount) onto every other
/// hittable enemy.
///
/// C# `Misery.cs::OnPlay`:
/// 1. `originalDebuffs = target.Powers.Where(p.TypeForCurrentAmount ==
///    PowerType.Debuff).Select(ClonePreservingMutability)` — snapshot taken
///    **before** the damage hit.
/// 2. `DamageCmd.Attack(7).Targeting(target)`.
/// 3. For each `HittableEnemies` except the target, for each snapshotted
///    debuff: if the enemy already has that power (and it's not instanced),
///    `PowerCmd.ModifyAmount(+item.Amount)`; otherwise `PowerCmd.Apply(copy,
///    enemy, item.Amount)`.
///
/// The Rust power model sums by `PowerKind`, so `apply_power_to_enemy(kind,
/// amount)` covers both the modify-existing and apply-fresh C# branches in
/// one call — but only for positive amounts; a snapshotted negative-attribute
/// debuff (see below) is copied via `apply_power_signed_to_creature` since the
/// positive-only helper early-outs on `amount <= 0`.
/// `DoHackyThingsForSpecificPowers`/`ITemporaryPower.IgnoreNextInstance` has no
/// Rust analogue — per `colorless/rend.rs`, `ITemporaryPower` is implemented
/// only by Temporary{Strength,Focus,Dexterity}, none of which are debuffs. The
/// HitFx is cosmetic and dropped. OnUpgrade (Damage 7->9, +Retain) lives in the
/// central `card_upgrades_generated.rs` table; the damage is read via
/// `current_damage`.
///
/// Bead `sts2-rs-rend-misery-signed-debuff-llzzt`: the debuff snapshot uses C#
/// `TypeForCurrentAmount` (`Misery.cs:28`), which is sign-sensitive — a
/// drained Strength/Dexterity/Focus (negative `AllowNegative` Counter attr)
/// reads as a Debuff. Filter via `is_debuff_for_amount`, not the sign-agnostic
/// `classify`, matching `colorless/rend.rs`.
pub(crate) fn play_misery(state: &mut CombatState, target: Option<CreatureUid>) -> EffectResult {
    let t = target.expect("Misery: Enemy-targeted");

    // 1. Snapshot the target's debuffs (kind + amount) before the hit.
    let original_debuffs: Vec<(PowerKind, i32)> = state
        .creatures
        .iter()
        .find(|c| c.id == t.0)
        .map(|c| {
            c.powers
                .iter()
                .filter_map(|p_uid| state.powers.get_by_id(p_uid.0))
                .filter(|p| sts2_core::afflictions::is_debuff_for_amount(p.kind, p.amount))
                .map(|p| (p.kind, p.amount))
                .collect()
        })
        .unwrap_or_default();

    // 2. Deal the attack damage to the target.
    let _ = deal_damage_player_to_enemy(state, t, current_damage(state));
    sts2_core::attack_command::end_card_command(state);

    // 3. Copy each snapshotted debuff onto every other hittable enemy.
    if !original_debuffs.is_empty() {
        let others: Vec<CreatureUid> = state
            .active_enemy_uids()
            .into_iter()
            .filter(|uid| *uid != t)
            .collect();
        for enemy in others {
            let alive = state
                .creatures
                .iter()
                .any(|c| c.id == enemy.0 && c.current_hp > 0);
            if !alive {
                continue;
            }
            for &(kind, amount) in &original_debuffs {
                // A snapshotted debuff can be a negative-amount attribute power
                // (e.g. drained Strength reads as a Debuff via
                // `is_debuff_for_amount`); `apply_power_to_enemy` early-outs on
                // `amount <= 0`, so route negatives through the signed helper to
                // mirror C# `PowerCmd.Apply(copy, enemy, item.Amount)`.
                if amount < 0 {
                    apply_power_signed_to_creature(state, enemy, kind, amount);
                } else {
                    apply_power_to_enemy(state, enemy, kind, amount);
                }
            }
        }
    }

    EffectResult::Resolved
}

#[must_use]
pub(crate) const fn spec() -> CardSpec {
    CardSpec {
        id: CardId::Misery,
        name: "MISERY",
        cost: 0,
        card_type: CardType::Attack,
        target_type: TargetType::Enemy,
        rarity: CardRarity::Rare,
        base_damage: 7,
        base_block: 0,
        keywords: CardKeywords::NONE,
        ..CardSpec::base()
    }
}

#[allow(clippy::unnecessary_wraps, dead_code)]
pub(crate) fn upgrade() -> Option<CardUpgradeDelta> {
    // C# OnUpgrade (Damage 7->9, AddKeyword(Retain)) lives in the central
    // `card_upgrades_generated.rs` table (CardId::Misery row); per-card
    // delta is None.
    None
}
