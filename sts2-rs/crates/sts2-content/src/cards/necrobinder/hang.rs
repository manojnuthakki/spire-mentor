//! Hand-authored Necrobinder card port (C#-first).
use super::prelude::*;

/// Hang (Rare, 1 energy, Attack). Deal 10 damage (13 upgraded) to an enemy,
/// then apply `max(2, current HangPower on target)` stacks of HangPower to it
/// (overflow-clamped at 999_999_999).
// C# source: ../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Cards/Hang.cs
pub(crate) fn play_hang(state: &mut CombatState, target: Option<CreatureUid>) -> EffectResult {
    let t = target.expect("Hang: Enemy-targeted");
    // C# Hang.OnPlay: DamageCmd.Attack(Damage.BaseValue).Targeting(Target).
    let _ = deal_damage_player_to_enemy(state, t, current_damage(state));
    sts2_core::attack_command::end_card_command(state);

    // powerAmount = Target.GetPowerAmount<HangPower>()
    let power_amount = creature_power_amount(state, t, PowerKind::Hang);
    // num = Math.Max(2, powerAmount); overflow clamp at 999_999_999.
    let mut num = power_amount.max(2);
    if power_amount.saturating_add(num) > 999_999_999 {
        num = (999_999_999 - power_amount).max(0);
    }
    // PowerCmd.Apply<HangPower>(Target, num, Owner.Creature, this)
    apply_power_to_enemy(state, t, PowerKind::Hang, num);
    EffectResult::Resolved
}

#[must_use]
pub(crate) const fn spec() -> CardSpec {
    CardSpec {
        id: CardId::Hang,
        name: "HANG",
        cost: 1,
        card_type: CardType::Attack,
        target_type: TargetType::Enemy,
        rarity: CardRarity::Rare,
        base_damage: 10,
        base_block: 0,
        keywords: CardKeywords::NONE,
        ..CardSpec::base()
    }
}

#[allow(clippy::unnecessary_wraps, dead_code)]
pub(crate) fn upgrade() -> Option<CardUpgradeDelta> {
    // C# OnUpgrade: Damage.UpgradeValueBy(3m) → 10 → 13.
    // Delta lives in the central card_upgrades_generated.rs table
    // (CardId::Hang row); per-card upgrade() returns None.
    None
}
