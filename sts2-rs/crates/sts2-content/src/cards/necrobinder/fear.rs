//! Fear (Necrobinder pool). Hand-authored C#-first port.
use super::prelude::*;

/// Fear (Common, 1 energy, Attack). Deal 7 damage and apply 1 Vulnerable;
/// Ethereal. C# `Fear.cs` OnPlay: DamageCmd.Attack(7) + PowerCmd.Apply<Vulnerable>(1).
// C# source: ../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Cards/Fear.cs
pub(crate) fn play_fear(state: &mut CombatState, target: Option<CreatureUid>) -> EffectResult {
    let t = target.expect("Fear: enemy target required");
    // C# DamageVar(7), OnUpgrade Damage.UpgradeValueBy(1) → handled via the
    // central card_upgrades_generated table (base_damage 7 → 8); read current_damage.
    let dmg = current_damage(state);
    let _ = deal_damage_player_to_enemy(state, t, dmg);
    sts2_core::attack_command::end_card_command(state);
    // C# PowerVar<VulnerablePower>(1); OnUpgrade Vulnerable.UpgradeValueBy(1) is a
    // PowerVar bump → gate on the upgraded check.
    let vuln = if current_card_upgraded(state) { 2 } else { 1 };
    apply_power_to_enemy(state, t, PowerKind::Vulnerable, vuln);
    EffectResult::Resolved
}

#[must_use]
pub(crate) const fn spec() -> CardSpec {
    CardSpec {
        id: CardId::Fear,
        name: "FEAR",
        cost: 1,
        card_type: CardType::Attack,
        target_type: TargetType::Enemy,
        rarity: CardRarity::Common,
        base_damage: 7,
        base_block: 0,
        keywords: CardKeywords::ETHEREAL,
        ..CardSpec::base()
    }
}

#[allow(clippy::unnecessary_wraps, dead_code)]
pub(crate) fn upgrade() -> Option<CardUpgradeDelta> {
    // C# OnUpgrade: Damage 7 → 8 lives in the central card_upgrades_generated
    // table (CardId::Fear). Vulnerable 1 → 2 is a PowerVar bump handled in
    // play_fear via current_card_upgraded.
    None
}
