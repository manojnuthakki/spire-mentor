//! Blight Strike (Necrobinder pool). Hand-authored C#-first port.
use super::prelude::*;

/// Blight Strike (Common, 1 energy, Attack). Deal 8 damage, then apply Doom
/// to the target equal to the total damage dealt by the attack.
/// C# `BlightStrike.cs` OnPlay: DamageCmd.Attack(8) then
/// PowerCmd.Apply<DoomPower>(target, sum of DamageResult.TotalDamage).
// C# source: ../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Cards/BlightStrike.cs
pub(crate) fn play_blight_strike(
    state: &mut CombatState,
    target: Option<CreatureUid>,
) -> EffectResult {
    let t = target.expect("Blight Strike: enemy target required");
    // C# DamageVar(8); OnUpgrade Damage.UpgradeValueBy(2) → 8/10 lives in the
    // central card_upgrades_generated table (CardId::BlightStrike); read current_damage.
    let dmg = current_damage(state);
    // C# applies DoomPower equal to attackCommand.Results.Sum(r => r.TotalDamage),
    // i.e. the damage dealt by this single-target attack. Use the full outcome's
    // total_damage so the Doom amount tracks the realized hit, not the raw base.
    let outcome = card_effects::deal_damage_player_to_enemy_full(state, t, dmg);
    sts2_core::attack_command::end_card_command(state);
    apply_power_to_enemy(state, t, PowerKind::Doom, outcome.total_damage);
    EffectResult::Resolved
}

#[must_use]
pub(crate) const fn spec() -> CardSpec {
    CardSpec {
        id: CardId::BlightStrike,
        name: "BLIGHT_STRIKE",
        cost: 1,
        card_type: CardType::Attack,
        target_type: TargetType::Enemy,
        rarity: CardRarity::Common,
        base_damage: 8,
        base_block: 0,
        keywords: CardKeywords::NONE,
        ..CardSpec::base()
    }
}

#[allow(clippy::unnecessary_wraps, dead_code)]
pub(crate) fn upgrade() -> Option<CardUpgradeDelta> {
    // C# OnUpgrade: Damage 8 → 10 lives in the central card_upgrades_generated
    // table (CardId::BlightStrike).
    None
}
