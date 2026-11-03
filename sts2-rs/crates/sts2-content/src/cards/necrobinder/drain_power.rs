//! Necrobinder DrainPower. Ported C#-first via the character-port skill.
//! C# source:
//! ../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Cards/DrainPower.cs
//!
//! `OnPlay`: deal `DamageVar.BaseValue` (10) to the target, then take
//! `CardsVar` (2) random upgradable cards from the discard pile and upgrade
//! each. `TakeRandom(count, Rng.CombatCardSelection)` =
//! `list.UnstableShuffle(rng).Take(count)` (IEnumerableExtensions.cs) over the
//! `IsUpgradable` subset of the discard pile.
//! `OnUpgrade`: Damage +2 (→12), Cards +1 (→3).
use super::prelude::*;
use sts2_core::CardUid;

/// DrainPower (Common, 1 energy, Attack). Deal 10 damage, then upgrade 2 random
/// upgradable cards in your discard pile. Upgrade: deal 12 / upgrade 3.
pub(crate) fn play_drain_power(
    state: &mut CombatState,
    target: Option<CreatureUid>,
) -> EffectResult {
    let t = target.expect("DrainPower: AnyEnemy-targeted");
    // C# `DamageVar.BaseValue` (the base 10/12, NOT the calculated value).
    let _ = deal_damage_player_to_enemy(state, t, current_damage(state));
    sts2_core::attack_command::end_card_command(state);

    let count = if current_card_upgraded(state) { 3 } else { 2 };

    // C# `discard.Cards.Where(IsUpgradable).TakeRandom(count, CombatCardSelection)`
    // = `UnstableShuffle(rng).Take(count)` over the upgradable subset.
    let mut upgradable: Vec<CardUid> = state
        .player
        .discard
        .iter()
        .copied()
        .filter(|uid| {
            let idx = uid.0.get() as usize - 1;
            state
                .cards
                .get(idx)
                .is_some_and(sts2_core::cards::Card::is_upgradable)
        })
        .collect();
    state
        .run_rng
        .combat_card_selection
        .shuffle(upgradable.as_mut_slice());

    for uid in upgradable.into_iter().take(count) {
        card_effects::upgrade_card_uid(state, uid);
    }

    EffectResult::Resolved
}

#[must_use]
pub(crate) const fn spec() -> CardSpec {
    CardSpec {
        id: CardId::DrainPower,
        name: "DRAIN_POWER",
        cost: 1,
        card_type: CardType::Attack,
        target_type: TargetType::Enemy,
        rarity: CardRarity::Common,
        base_damage: 10,
        base_block: 0,
        keywords: CardKeywords::NONE,
        ..CardSpec::base()
    }
}

#[allow(clippy::unnecessary_wraps, dead_code)]
pub(crate) fn upgrade() -> Option<CardUpgradeDelta> {
    // C# OnUpgrade: Damage.UpgradeValueBy(2) → 10 + 2 = 12.
    // Cards.UpgradeValueBy(1) is a CardsVar bump (read in the handler via
    // current_card_upgraded), not a damage/block/cost stat field.
    Some(CardUpgradeDelta {
        cost: None,
        base_damage: Some(12),
        base_block: None,
        keywords: None,
    })
}
