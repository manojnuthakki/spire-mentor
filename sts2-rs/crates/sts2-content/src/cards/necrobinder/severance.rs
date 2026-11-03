//! Hand-authored from the C# decompilation (character-port skill).
use super::prelude::*;

/// Severance (Uncommon, 2 energy, Attack). Deal `DamageVar`(13) damage to the
/// target, then generate 3 Soul tokens: one into the Draw pile at a random
/// position, one into the Discard pile, and one into the Hand.
//
// C# source:
// ../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Cards/Severance.cs
//   ctor: base(2, CardType.Attack, CardRarity.Uncommon, TargetType.AnyEnemy)
//   CanonicalVars: DamageVar(13, Move)
//   OnPlay: DamageCmd.Attack(Damage.BaseValue).Targeting(Target);
//           souls = Soul.Create(Owner, 3, CombatState);
//           CardPileCmd.AddGeneratedCardToCombat(souls[0], PileType.Draw,
//               addedByPlayer: true, CardPilePosition.Random);
//           CardPileCmd.AddGeneratedCardToCombat(souls[1], PileType.Discard,
//               addedByPlayer: true);
//           CardPileCmd.AddGeneratedCardToCombat(souls[2], PileType.Hand,
//               addedByPlayer: true);
//           CardCmd.PreviewCardPileAdd(...);   // cosmetic preview, ignored
//   OnUpgrade: Damage.UpgradeValueBy(5m)  (13 -> 18; spec-level, via upgrade()).
//
// Note: Severance's OnPlay never calls CardCmd.Upgrade on the generated Souls;
// the Souls are always spawned un-upgraded regardless of Severance's upgrade.
pub(crate) fn play_severance(state: &mut CombatState, target: Option<CreatureUid>) -> EffectResult {
    let t = target.expect("Severance: AnyEnemy-targeted");
    let _ = deal_damage_player_to_enemy(state, t, current_damage(state));
    sts2_core::attack_command::end_card_command(state);

    // souls[0] -> Draw at CardPilePosition.Random.
    let soul_draw = card_from_spec(&status_specs::soul());
    let _ = spawn_card_into_draw_pile_random_position(state, soul_draw);

    // souls[1] -> Discard (append, back of pile).
    let soul_discard = card_from_spec(&status_specs::soul());
    let _ = spawn_card_into_pile(state, soul_discard, PileKind::Discard);

    // souls[2] -> Hand.
    let soul_hand = card_from_spec(&status_specs::soul());
    let _ = spawn_card_into_pile(state, soul_hand, PileKind::Hand);

    EffectResult::Resolved
}

#[must_use]
pub(crate) const fn spec() -> CardSpec {
    CardSpec {
        id: CardId::Severance,
        name: "SEVERANCE",
        cost: 2,
        card_type: CardType::Attack,
        target_type: TargetType::Enemy,
        rarity: CardRarity::Uncommon,
        base_damage: 13,
        base_block: 0,
        keywords: CardKeywords::NONE,
        ..CardSpec::base()
    }
}

#[allow(clippy::unnecessary_wraps, dead_code)]
pub(crate) fn upgrade() -> Option<CardUpgradeDelta> {
    // C# OnUpgrade: Damage.UpgradeValueBy(5) -> 13 + 5 = 18.
    Some(CardUpgradeDelta {
        cost: None,
        base_damage: Some(18),
        base_block: None,
        keywords: None,
    })
}
