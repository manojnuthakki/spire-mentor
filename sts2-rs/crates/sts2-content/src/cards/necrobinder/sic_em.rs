//! Sic Em (Necrobinder). Hand-authored C#-first port via the character-port skill.
//! C# source: ../sts2-wiki/decompiled/v0.107.1/MegaCrit.Sts2.Core.Models.Cards/SicEm.cs
use super::prelude::*;

/// Sic Em (Uncommon, 1 energy, Attack, AnyEnemy). The Osty attacks the target
/// for 5 (6 upgraded), then `SicEmPower(3)` (4 upgraded) is applied to the
/// target. While the mark is up, whenever the Osty deals damage to that enemy
/// the Osty is summoned/grown (the `SicEmPower` reactive lives in
/// `dispatch_after_damage_given`); the mark removes itself at the enemy's turn
/// end.
//
// C# OnPlay:
//   if (!Osty.CheckMissingWithAnim(Owner))
//       DamageCmd.Attack(OstyDamage.BaseValue)   // 5, +1 on upgrade → 6
//           .FromOsty(Owner.Osty, this)          // attacker is the OSTY
//           .Targeting(cardPlay.Target).Execute(...);
//   PowerCmd.Apply<SicEmPower>(cardPlay.Target, SicEmPower.BaseValue, Owner.Creature, this);
//
// CanonicalVars:
//   OstyDamageVar(5m, Move)         → base_damage 5 (cosmetic metadata; the hit
//                                     is dealt by the Osty, not the player).
//                                     OnUpgrade +1 → 6.
//   PowerVar<SicEmPower>(3m)        → 3 stacks of SicEmPower. OnUpgrade +1 → 4.
// CanonicalTags: OstyAttack (cosmetic glow tag; not carried in CardSpec here).
// ShouldGlowRedInternal => Owner.IsOstyMissing — pure UI, ignored.
//
// IMPORTANT: the `SicEmPower` apply is OUTSIDE the Osty-missing guard in C# —
// the mark lands on the target whether or not the Osty is alive to swing. Only
// the damage half is skipped when the Osty is missing. `deal_damage_from_osty_
// to_enemy` is itself a no-op when the Osty is absent (`osty_dealer_ref` → None),
// so calling it unconditionally reproduces `CheckMissingWithAnim` (minus the
// cosmetic summon-prompt animation we don't model).
pub(crate) fn play_sic_em(state: &mut CombatState, target: Option<CreatureUid>) -> EffectResult {
    let t = target.expect("Sic Em: enemy target required");

    // OstyDamage: 5 base, +1 on upgrade. Computed here (the OstyDamage is not a
    // plain player-attack stat), gated on current_card_upgraded like the sibling
    // OstyAttack cards (poke/unleash/bone_shards). No-op if the Osty is missing.
    let dmg = if current_card_upgraded(state) { 6 } else { 5 };
    deal_damage_from_osty_to_enemy(state, t, dmg);
    sts2_core::attack_command::end_card_command(state);

    // SicEmPower: 3 base, +1 on upgrade (v0.107.1; was 2/3). Applied regardless
    // of the Osty's state (C# applies it after the `if (!CheckMissingWithAnim)`
    // block).
    let mark = if current_card_upgraded(state) { 4 } else { 3 };
    apply_power_to_enemy(state, t, PowerKind::SicEmPower, mark);
    EffectResult::Resolved
}

#[must_use]
pub(crate) const fn spec() -> CardSpec {
    CardSpec {
        id: CardId::SicEm,
        name: "SIC_EM",
        cost: 1,
        card_type: CardType::Attack,
        target_type: TargetType::Enemy, // C# TargetType.AnyEnemy → single-player Enemy
        rarity: CardRarity::Uncommon,
        base_damage: 5, // OstyDamageVar(5m) — cosmetic; dealt via the Osty
        base_block: 0,
        keywords: CardKeywords::NONE,
        ..CardSpec::base()
    }
}

#[allow(clippy::unnecessary_wraps, dead_code)]
pub(crate) fn upgrade() -> Option<CardUpgradeDelta> {
    // C# OnUpgrade: OstyDamage.UpgradeValueBy(1) (5 → 6) and
    // SicEmPower.UpgradeValueBy(1) (3 → 4). Both magnitudes are computed in the
    // handler (the OstyDamage is not a plain attack stat, and the power amount
    // is not a spec field), so the bumps are applied via current_card_upgraded
    // rather than a static spec delta. Returns None to keep one source of truth.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mint_into_play(state: &mut CombatState, upgraded: bool) -> sts2_core::CardUid {
        let mut card =
            crate::cards::spec_index::card_pod_from_id(CardId::SicEm, upgraded).expect("spec");
        let raw = u32::try_from(state.cards.len() + 1).expect("fits");
        let uid = sts2_core::CardUid::new(raw).expect("nz");
        card.uid = uid;
        state.cards.push(card);
        state.player.play.push(uid);
        uid
    }

    fn enemy_power(state: &CombatState, uid: CreatureUid, kind: PowerKind) -> Option<i32> {
        let creature = state.creatures.iter().find(|c| c.id == uid.0)?;
        creature.powers.iter().find_map(|pu| {
            state
                .powers
                .iter()
                .find(|p| p.id == pu.0 && p.kind == kind)
                .map(|p| p.amount)
        })
    }

    /// v0.107.1: the `SicEmPower` mark applied to the target is 3 base / 4
    /// upgraded (was 2/3 in v0.103.2). The Osty-damage half no-ops without an
    /// Osty, but the mark lands regardless (C# applies it outside the guard).
    #[test]
    fn sic_em_marks_three_base_four_upgraded() {
        let mut state = CombatState::default();
        let enemy = state.spawn_enemy(sts2_core::MonsterKind::Nibbit);
        mint_into_play(&mut state, false);
        play_sic_em(&mut state, Some(enemy));
        assert_eq!(
            enemy_power(&state, enemy, PowerKind::SicEmPower),
            Some(3),
            "base SicEm applies 3"
        );

        let mut up_state = CombatState::default();
        let up_enemy = up_state.spawn_enemy(sts2_core::MonsterKind::Nibbit);
        mint_into_play(&mut up_state, true);
        play_sic_em(&mut up_state, Some(up_enemy));
        assert_eq!(
            enemy_power(&up_state, up_enemy, PowerKind::SicEmPower),
            Some(4),
            "upgraded SicEm applies 4"
        );
    }
}
