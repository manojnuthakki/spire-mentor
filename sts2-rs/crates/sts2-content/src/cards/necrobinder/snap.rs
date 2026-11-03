//! Hand-authored via the character-port skill (C#-first).
use super::prelude::*;
use sts2_core::cards::KeywordOverride;

/// Snap (Common, 1 energy, Attack). The Osty attacks the target for 7 (10
/// upgraded), then you choose a non-Retain card in your hand and give it
/// Retain. Does nothing on the attack half if the Osty is missing.
///
/// C# `OnPlay`:
///   ArgumentNullException.ThrowIfNull(cardPlay.Target);
///   if (!Osty.CheckMissingWithAnim(Owner)) {
///       DamageCmd.Attack(OstyDamage.BaseValue)   // 7, +3 on upgrade → 10
///           .FromOsty(Owner.Osty, this)          // attacker is the OSTY, not the player
///           .Targeting(cardPlay.Target)
///           .Execute(...);
///   }
///   var card = CardSelectCmd.FromHand(prefs(prompt, 1), ctx, Owner,
///       filter: c => !c.Keywords.Contains(Retain), source: this).FirstOrDefault();
///   if (card != null) CardCmd.ApplyKeyword(card, CardKeyword.Retain);
///
/// CanonicalVars: OstyDamageVar(7m, Move) → base_damage 7.
/// CanonicalTags: OstyAttack → cosmetic glow tag (not carried in CardSpec here).
/// ExtraHoverTips: Retain keyword hover-tip → pure UI (the card itself is NOT
///   Retain; the keyword is *applied* to a chosen hand card). Ignored.
/// ShouldGlowRedInternal => Owner.IsOstyMissing — pure UI, ignored.
///
/// Osty-missing guard is faithful: when the Osty is absent/dead the attack
/// half is a no-op (matching `!CheckMissingWithAnim`); the retain-select half
/// still runs (C# runs it unconditionally after the if-block).
///
/// The retain-select half is a PLAYER CHOICE with ZERO RNG. It surfaces as a
/// [`sts2_core::ChoiceKind::ApplyRetainInHand`] pending choice over the
/// non-Retain hand cards (the prior port auto-picked the first such card, which
/// desynced whole-run parity vs the C# headless agent's pick). When `<= 1`
/// candidate exists C# auto-applies (`CardSelectCmd.FromHand`'s
/// `cards.Count <= MinSelect` fast path), so the lone candidate gets Retain
/// directly with no prompt; 2+ candidates → open the choice in hand order
/// (C# `FromHand` passes the filtered hand with NO OrderBy). The granted
/// keyword rides the per-instance `keyword_override` (mirroring
/// `card_effects::apply_ethereal_keyword`).
// C# source: ../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Cards/Snap.cs
pub(crate) fn play_snap(state: &mut CombatState, target: Option<CreatureUid>) -> EffectResult {
    let t = target.expect("Snap: Enemy-targeted");

    // C# `!Osty.CheckMissingWithAnim(Owner)` — only attack when the Osty is
    // present and alive. Mirror via the player's pet creature.
    let osty_alive = state
        .player
        .pet_uid
        .and_then(|uid| state.creatures.iter().find(|c| c.id == uid.0))
        .is_some_and(|c| c.current_hp > 0);
    if osty_alive {
        // OstyDamage.BaseValue: 7 base, +3 on upgrade. Dealt FromOsty so the
        // hit reads the Osty's dealer-side modifiers (see Poke).
        let dmg = if current_card_upgraded(state) { 10 } else { 7 };
        let _ = deal_damage_from_osty_to_enemy(state, t, dmg);
        sts2_core::attack_command::end_card_command(state);
    }

    // C# `CardSelectCmd.FromHand` early-returns when `CombatManager.IsOverOrEnding`,
    // so a lethal Snap never opens its Retain prompt — combat goes straight to the
    // reward instead of stranding Rust in a phantom `card_select`.
    if sts2_core::combat_loop::combat_is_ending(state) {
        return EffectResult::Resolved;
    }

    // CardSelectCmd.FromHand(select 1, filter: !Retain) → ApplyKeyword(Retain).
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
                .is_some_and(|c| !card_effects::card_retain_inst(c))
        })
        .collect();
    match candidates.as_slice() {
        [] => {}
        // FromHand auto-applies to the lone candidate (cards.Count <= MinSelect).
        &[uid] => {
            let idx = uid.0.get() as usize - 1;
            if let Some(card) = state.cards.get_mut(idx) {
                // ADDITIVE keyword grant (C# `CardCmd.ApplyKeyword(card, Retain)`):
                // preserve the card's intrinsic keywords (e.g. base Prolong's
                // EXHAUST) instead of replacing them with a bare RETAIN — else
                // `card_exhausts_inst` (override-first) would stop exhausting it.
                card_effects::apply_retain_keyword(card);
            }
        }
        // 2+ candidates: open a single-pick choice in hand order.
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
                sts2_core::ChoiceKind::ApplyRetainInHand,
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
        id: CardId::Snap,
        name: "SNAP",
        cost: 1,
        card_type: CardType::Attack,
        target_type: TargetType::Enemy, // C# AnyEnemy → single-player collapse
        rarity: CardRarity::Common,
        base_damage: 7, // OstyDamageVar(7m, ValueProp.Move)
        base_block: 0,
        keywords: CardKeywords::NONE,
        ..CardSpec::base()
    }
}

#[allow(clippy::unnecessary_wraps, dead_code)]
pub(crate) fn upgrade() -> Option<CardUpgradeDelta> {
    // C# OnUpgrade: OstyDamage.UpgradeValueBy(3m) → 7 → 10.
    Some(CardUpgradeDelta {
        cost: None,
        base_damage: Some(10),
        base_block: None,
        keywords: None,
    })
}
