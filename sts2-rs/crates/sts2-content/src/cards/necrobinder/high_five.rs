//! High Five (Necrobinder). C#-first port via the character-port skill.
use super::prelude::*;

/// High Five (Uncommon, 2 energy, Attack, AllEnemies). The Osty attacks every
/// enemy for 11 (13 upgraded), then applies 2 Vulnerable (3 upgraded) to all
/// hittable enemies. Does nothing if the Osty is missing.
//
// C# source: ../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Cards/HighFive.cs
//
// C# OnPlay:
//   if (!Osty.CheckMissingWithAnim(Owner)) {
//       DamageCmd.Attack(OstyDamage.BaseValue)   // 11, +2 on upgrade → 13
//           .FromOsty(Owner.Osty, this)          // attacker is the OSTY, not the player
//           .TargetingAllOpponents(CombatState)
//           .WithHitFx(...)                       // cosmetic VFX, ignored
//           .Execute(...);
//       PowerCmd.Apply<VulnerablePower>(CombatState.HittableEnemies,
//           Vulnerable.BaseValue, Owner.Creature, this);   // 2, +1 on upgrade → 3
//   }
//
// CanonicalVars:
//   OstyDamageVar(11m, Move)      → base_damage 11 (Move value-prop). OnUpgrade +2 → 13.
//   PowerVar<VulnerablePower>(2m) → 2 Vulnerable.                     OnUpgrade +1 → 3.
// CanonicalTags: OstyAttack → CardTag::OstyAttack (cosmetic glow tag; not carried
//                in CardSpec metadata here).
// ShouldGlowRedInternal / IsPlayable => Owner.IsOstyMissing — pure UI/playability
//                gating; the Osty-missing guard below makes the effect a no-op when
//                the Osty is absent, matching `!CheckMissingWithAnim`.
pub(crate) fn play_high_five(
    state: &mut CombatState,
    _target: Option<CreatureUid>,
) -> EffectResult {
    // C# `!Osty.CheckMissingWithAnim(Owner)` — only resolve when the Osty is
    // present and alive. Matches the sibling OstyAttack cards (bone_shards/poke).
    let osty_alive = state
        .player
        .pet_uid
        .and_then(|uid| state.creatures.iter().find(|c| c.id == uid.0))
        .is_some_and(|c| c.current_hp > 0);
    if !osty_alive {
        return EffectResult::Resolved;
    }

    // OstyDamage.BaseValue: 11 base, +2 on upgrade. Vulnerable: 2 base, +1 on
    // upgrade. Both magnitudes are computed in the handler, so the upgrade bumps
    // are gated on current_card_upgraded.
    let (dmg, vuln) = if current_card_upgraded(state) {
        (13, 3)
    } else {
        (11, 2)
    };

    // C# `DamageCmd.Attack(dmg).FromOsty(Owner.Osty, this).TargetingAllOpponents(...)`
    // — the Osty is the dealer for the AoE hit (Osty-side modifiers apply;
    // player-dealer-gated effects do not fire). `deal_damage_from_osty_to_all_
    // alive_enemies` resolves the dealer as the pet creature (see `osty_dealer_ref`).
    deal_damage_from_osty_to_all_alive_enemies(state, dmg);
    sts2_core::attack_command::end_card_command(state);

    // C# `PowerCmd.Apply<VulnerablePower>(CombatState.HittableEnemies, Vulnerable.
    // BaseValue, Owner.Creature, this)` — apply Vulnerable to every alive enemy.
    // The applier is the player's creature (not the Osty), matching `Owner.Creature`.
    let enemy_uids: Vec<CreatureUid> = state.enemy_uids.iter().copied().collect();
    for uid in enemy_uids {
        if state
            .creatures
            .iter()
            .any(|c| c.id == uid.0 && c.current_hp > 0)
        {
            apply_power_to_enemy(state, uid, PowerKind::Vulnerable, vuln);
        }
    }

    EffectResult::Resolved
}

#[must_use]
pub(crate) const fn spec() -> CardSpec {
    CardSpec {
        id: CardId::HighFive,
        name: "HIGH_FIVE",
        cost: 2,
        card_type: CardType::Attack,
        target_type: TargetType::AllEnemies,
        rarity: CardRarity::Uncommon,
        base_damage: 11, // OstyDamageVar(11m)
        base_block: 0,
        keywords: CardKeywords::NONE,
        ..CardSpec::base()
    }
}

#[allow(clippy::unnecessary_wraps, dead_code)]
pub(crate) fn upgrade() -> Option<CardUpgradeDelta> {
    // C# OnUpgrade: OstyDamage.UpgradeValueBy(2m) (11 → 13) and
    // Vulnerable.UpgradeValueBy(1m) (2 → 3). Both magnitudes are computed in the
    // handler from current_card_upgraded (OstyDamage is not a plain attack stat,
    // and Vulnerable is a PowerVar amount), so the bumps are applied there rather
    // than via a static spec delta; this returns None to keep one source of truth.
    None
}
