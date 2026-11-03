//! Necrobinder SculptingStrike. Ported C#-first via the character-port skill.
//! C# source:
//! ../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Cards/SculptingStrike.cs
//!
//! `OnPlay`:
//!   1. `DamageCmd.Attack(DynamicVars.Damage.BaseValue)` to the target (9 base).
//!   2. `CardSelectCmd.FromHand(prefs(prompt, 1), filter: !c.Keywords.Contains(Ethereal))`
//!      — choose up to 1 non-Ethereal hand card.
//!   3. If a card was chosen, `CardCmd.ApplyKeyword(card, CardKeyword.Ethereal)`.
//!
//! `OnUpgrade`: `DynamicVars.Damage.UpgradeValueBy(3m)` → 9 + 3 = 12.
//!
//! The `Strike` CanonicalTag is applied centrally by
//! `cards::status::helpers::tags_for_card_id` (SculptingStrike is listed there);
//! the Ethereal ExtraHoverTip is cosmetic.
use super::prelude::*;

/// SculptingStrike (Common, 1 energy, Attack). Deal 9 damage, then apply
/// Ethereal to a chosen non-Ethereal card in hand. Upgrade: +3 damage (→12).
pub(crate) fn play_sculpting_strike(
    state: &mut CombatState,
    target: Option<CreatureUid>,
) -> EffectResult {
    let t = target.expect("SculptingStrike: Enemy-targeted");
    let _ = deal_damage_player_to_enemy(state, t, current_damage(state));
    sts2_core::attack_command::end_card_command(state);

    // C# `CardSelectCmd.FromHand` early-returns when `CombatManager.IsOverOrEnding`,
    // so a lethal Sculpting Strike never opens its Ethereal prompt — combat goes
    // straight to the reward instead of stranding Rust in a phantom `card_select`.
    if sts2_core::combat_loop::combat_is_ending(state) {
        return EffectResult::Resolved;
    }

    // C# CardSelectCmd.FromHand(prefs(prompt, 1), filter: not already Ethereal)
    // → ApplyKeyword(Ethereal) — a PLAYER CHOICE with ZERO RNG. Surfaces as a
    // `ChoiceKind::ApplyEtherealInHand` pending choice (the prior port auto-picked
    // the first eligible card, which desynced whole-run parity vs the C# headless
    // agent's pick). When `<= 1` candidate exists C# auto-applies (FromHand's
    // `cards.Count <= MinSelect` fast path); 2+ → open in hand order (no OrderBy).
    let candidates: Vec<sts2_core::CardUid> = state
        .player
        .hand
        .iter()
        .copied()
        .filter(|uid| {
            let idx = uid.0.get() as usize - 1;
            state
                .cards
                .get(idx)
                .is_some_and(|c| !card_effects::card_ethereal_inst(c))
        })
        .collect();
    match candidates.as_slice() {
        [] => {}
        &[uid] => {
            let idx = uid.0.get() as usize - 1;
            if let Some(card) = state.cards.get_mut(idx) {
                card_effects::apply_ethereal_keyword(card);
            }
        }
        _ => {
            let options: Vec<sts2_core::ChoiceOption> = candidates
                .iter()
                .filter_map(|&uid| {
                    let idx = uid.0.get() as usize - 1;
                    let card = state.cards.get(idx)?;
                    Some(sts2_core::ChoiceOption {
                        card_id: card.card_id,
                        upgraded: card.upgraded,
                        card_uid: Some(uid),
                        source_pile: Some(sts2_core::cards::PileKind::Hand),
                    })
                })
                .collect();
            state.request_card_choice(
                sts2_core::ChoiceKind::ApplyEtherealInHand,
                options,
                1,
                1,
                false,
            );
        }
    }
    EffectResult::Resolved
}

#[must_use]
pub(crate) const fn spec() -> CardSpec {
    CardSpec {
        id: CardId::SculptingStrike,
        name: "SCULPTING_STRIKE",
        cost: 1,
        card_type: CardType::Attack,
        target_type: TargetType::Enemy,
        rarity: CardRarity::Common,
        base_damage: 9,
        base_block: 0,
        keywords: CardKeywords::NONE,
        ..CardSpec::base()
    }
}

#[allow(clippy::unnecessary_wraps, dead_code)]
pub(crate) fn upgrade() -> Option<CardUpgradeDelta> {
    // C# OnUpgrade: DynamicVars.Damage.UpgradeValueBy(3m) → 9 + 3 = 12.
    Some(CardUpgradeDelta {
        cost: None,
        base_damage: Some(12),
        base_block: None,
        keywords: None,
    })
}
