//! Veilpiercer (Necrobinder pool). Hand-authored C#-first port.
use super::prelude::*;

/// Veilpiercer (Uncommon, 1 energy, Attack). Deal 10 damage, then gain 1
/// Veilpiercer (counter) — while held, Ethereal cards in hand/play cost 0 and
/// playing one decrements the counter.
/// C# `Veilpiercer.cs` OnPlay: DamageCmd.Attack(10) then
/// PowerCmd.Apply<VeilpiercerPower>(self, 1).
// C# source: ../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Cards/Veilpiercer.cs
pub(crate) fn play_veilpiercer(
    state: &mut CombatState,
    target: Option<CreatureUid>,
) -> EffectResult {
    let t = target.expect("Veilpiercer: enemy target required");
    // C# DamageVar(10); OnUpgrade Damage.UpgradeValueBy(3) → 10/13 lives in the
    // central card_upgrades_generated table (CardId::Veilpiercer); read current_damage.
    let dmg = current_damage(state);
    let _ = deal_damage_player_to_enemy(state, t, dmg);
    sts2_core::attack_command::end_card_command(state);
    // C# PowerCmd.Apply<VeilpiercerPower>(base.Owner.Creature, 1m, ...) — applies
    // to the card owner (the player). VeilpiercerPower is StackType.Counter.
    apply_power_to_player(state, PowerKind::Veilpiercer, 1);
    EffectResult::Resolved
}

#[must_use]
pub(crate) const fn spec() -> CardSpec {
    CardSpec {
        id: CardId::Veilpiercer,
        name: "VEILPIERCER",
        cost: 1,
        card_type: CardType::Attack,
        target_type: TargetType::Enemy,
        rarity: CardRarity::Uncommon,
        base_damage: 10,
        base_block: 0,
        // The card itself is NOT Ethereal — Ethereal appears only as an
        // ExtraHoverTip referencing the power's effect, not in CanonicalKeywords.
        keywords: CardKeywords::NONE,
        ..CardSpec::base()
    }
}

#[allow(clippy::unnecessary_wraps, dead_code)]
pub(crate) fn upgrade() -> Option<CardUpgradeDelta> {
    // C# OnUpgrade: Damage.UpgradeValueBy(3) → 10 → 13 lives in the central
    // card_upgrades_generated table (CardId::Veilpiercer).
    None
}
