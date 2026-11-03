//! Hand-authored via the character-port skill (C#-first).
use super::prelude::*;

/// Graveblast (Common, 1 energy, Attack). Deal 4 damage to a single enemy,
/// then choose a card from your discard pile and return it to your hand.
/// Exhaust.
///
/// C# `OnPlay`: `DamageCmd.Attack(Damage.BaseValue).Targeting(Target)` then a
/// `CardSelectCmd.FromSimpleGrid` over the discard pile (select 1) and
/// `CardPileCmd.Add(selected, PileType.Hand)` — a PLAYER CHOICE with ZERO RNG
/// (Graveblast.cs:30-35). This surfaces as a
/// [`sts2_core::ChoiceKind::AddFromDiscardToHand`] pending choice so the agent
/// picks (mirroring `LiquidMemories`/`drink_liquid_memories`, but WITHOUT the
/// free-this-turn stamp — Graveblast returns the card at its normal cost). The
/// prior port auto-picked the first discard card (`pick_first_in_pile`), which
/// desynced whole-run parity vs the C# headless agent's selection. Empty
/// discard → no-op; exactly 1 card → `FromSimpleGrid` auto-returns it
/// (`cards.Count <= MinSelect`, CardSelectCmd.cs:232-234) with no prompt; 2+
/// cards → open the choice in discard-pile order (C# passes `…Cards` with NO
/// OrderBy). `OnUpgrade` bumps damage 4 → 6 and removes Exhaust (delta in
/// `card_upgrades_generated.rs`).
// C# source: ../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Cards/Graveblast.cs
pub(crate) fn play_graveblast(
    state: &mut CombatState,
    target: Option<CreatureUid>,
) -> EffectResult {
    let t = target.expect("Graveblast: Enemy-targeted");
    let _ = deal_damage_player_to_enemy(state, t, current_damage(state));
    sts2_core::attack_command::end_card_command(state);
    // A lethal Graveblast ends combat before the discard select: C#
    // `CardSelectCmd.FromSimpleGrid` returns empty when `IsEnding`
    // (CardSelectCmd.cs:222-224), so suppress the select here too.
    if sts2_core::combat_loop::combat_is_ending(state) {
        return EffectResult::Resolved;
    }
    // CardSelectCmd.FromSimpleGrid(discard, 1) → CardPileCmd.Add(_, Hand).
    match state.player.discard.as_slice() {
        [] => {}
        // FromSimpleGrid auto-returns the lone card (CardSelectCmd.cs:232-234) —
        // move directly, no prompt, no RNG.
        &[uid] => {
            card_effects::move_card_to_pile(state, uid, PileKind::Hand, false);
        }
        // 2+ cards: open a single-pick choice in discard-pile order.
        _ => {
            let options: Vec<sts2_core::ChoiceOption> = state
                .player
                .discard
                .iter()
                .filter_map(|&uid| {
                    let card = state.cards.iter().find(|c| c.uid == uid)?;
                    Some(sts2_core::ChoiceOption {
                        card_id: card.card_id,
                        upgraded: card.upgraded,
                        card_uid: Some(uid),
                        source_pile: Some(sts2_core::cards::PileKind::Discard),
                    })
                })
                .collect();
            state.request_card_choice(
                sts2_core::ChoiceKind::AddFromDiscardToHand,
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
        id: CardId::Graveblast,
        name: "GRAVEBLAST",
        cost: 1,
        card_type: CardType::Attack,
        target_type: TargetType::Enemy, // C# TargetType.AnyEnemy → single-player collapse
        rarity: CardRarity::Common,
        base_damage: 4, // DamageVar(4m, ValueProp.Move)
        base_block: 0,
        keywords: CardKeywords::EXHAUST, // CanonicalKeywords { Exhaust }
        ..CardSpec::base()
    }
}

#[allow(clippy::unnecessary_wraps, dead_code)]
pub(crate) fn upgrade() -> Option<CardUpgradeDelta> {
    // C# OnUpgrade: DynamicVars.Damage.UpgradeValueBy(2m) → 4 → 6, and
    // RemoveKeyword(CardKeyword.Exhaust). Delta lives in the central
    // card_upgrades_generated.rs table (base_damage Some(6), keywords
    // Some(CardKeywords::NONE)); per-card upgrade() returns None.
    None
}
