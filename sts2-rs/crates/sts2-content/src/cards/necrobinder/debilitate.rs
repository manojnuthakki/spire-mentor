//! Debilitate (Necrobinder pool). Hand-authored C#-first port.
use super::prelude::*;

/// Debilitate (Uncommon, 1 energy, Attack). Deal 10 damage, then apply 2
/// Debilitate to the target.
///
/// C# source: ../sts2-wiki/decompiled/v0.107.1/MegaCrit.Sts2.Core.Models.Cards/Debilitate.cs
///
/// C# `CanonicalVars`: `DamageVar(10)` + `PowerVar<DebilitatePower>(2)`
/// (v0.107.1; the power amount/duration was 3 in v0.103.2). C#
/// `OnPlay`: `DamageCmd.Attack(Damage.BaseValue).Targeting(target)` then
/// `PowerCmd.Apply<DebilitatePower>(target, DebilitatePower.BaseValue)`
/// (the `WithHitFx` slash is cosmetic and ignored). C# ctor
/// `base(1, Attack, Uncommon, AnyEnemy)` → cost 1, Attack, Uncommon, Enemy.
///
/// C# `OnUpgrade`: `Damage.UpgradeValueBy(2)` (10 → 12) and
/// `DebilitatePower.UpgradeValueBy(1)` (2 → 3). The damage delta is a
/// stat-field bump → lives in the central `card_upgrades_generated` table
/// (CardId::DebilitateCard); the DebilitatePower amount is an effect-var
/// bump → gated here on `current_card_upgraded`.
pub(crate) fn play_debilitate(
    state: &mut CombatState,
    target: Option<CreatureUid>,
) -> EffectResult {
    let t = target.expect("Debilitate: enemy target required");
    let dmg = current_damage(state);
    let _ = deal_damage_player_to_enemy(state, t, dmg);
    sts2_core::attack_command::end_card_command(state);
    let debilitate_amt = if current_card_upgraded(state) { 3 } else { 2 };
    apply_power_to_enemy(state, t, PowerKind::Debilitate, debilitate_amt);
    EffectResult::Resolved
}

#[must_use]
pub(crate) const fn spec() -> CardSpec {
    CardSpec {
        id: CardId::DebilitateCard,
        name: "DEBILITATE_CARD",
        cost: 1,
        card_type: CardType::Attack,
        target_type: TargetType::Enemy,
        rarity: CardRarity::Uncommon,
        base_damage: 10,
        base_block: 0,
        keywords: CardKeywords::NONE,
        ..CardSpec::base()
    }
}

#[allow(clippy::unnecessary_wraps, dead_code)]
pub(crate) fn upgrade() -> Option<CardUpgradeDelta> {
    // C# OnUpgrade: Damage 10 → 12 lives in the central
    // card_upgrades_generated table (CardId::DebilitateCard). The
    // DebilitatePower 2 → 3 effect-var bump is handled inline in
    // play_debilitate via the current_card_upgraded branch.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mint_into_play(state: &mut CombatState, upgraded: bool) -> sts2_core::CardUid {
        let mut card = crate::cards::spec_index::card_pod_from_id(CardId::DebilitateCard, upgraded)
            .expect("spec");
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

    /// v0.107.1: 10 damage + 2 Debilitate (the power amount/duration was 3 in
    /// v0.103.2); upgraded is 12 damage + 3 Debilitate.
    #[test]
    fn debilitate_applies_two_base_three_upgraded() {
        let mut state = CombatState::default();
        let enemy = state.spawn_enemy(sts2_core::MonsterKind::Nibbit);

        mint_into_play(&mut state, false);
        play_debilitate(&mut state, Some(enemy));
        assert_eq!(
            enemy_power(&state, enemy, PowerKind::Debilitate),
            Some(2),
            "base Debilitate applies 2"
        );

        let mut up_state = CombatState::default();
        let up_enemy = up_state.spawn_enemy(sts2_core::MonsterKind::Nibbit);
        mint_into_play(&mut up_state, true);
        play_debilitate(&mut up_state, Some(up_enemy));
        assert_eq!(
            enemy_power(&up_state, up_enemy, PowerKind::Debilitate),
            Some(3),
            "upgraded Debilitate applies 3"
        );
    }
}
