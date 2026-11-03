//! Fetch (Necrobinder). Hand-authored C#-first port via the character-port skill.
//! C# source: ../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Cards/Fetch.cs
use super::prelude::*;

/// Fetch (Uncommon, 0 energy, Attack, AnyEnemy). The Osty attacks the target
/// for 3 (6 upgraded); then — only if Fetch has not already been played and
/// resolved this turn — the player draws 1 card. Does nothing (neither the
/// damage nor the draw) when the Osty is missing.
//
// C# OnPlay:
//   ArgumentNullException.ThrowIfNull(cardPlay.Target);
//   if (!Osty.CheckMissingWithAnim(Owner)) {
//       DamageCmd.Attack(OstyDamage.BaseValue)   // 3, +3 on upgrade → 6
//           .FromOsty(Owner.Osty, this)          // attacker is the OSTY, not the player
//           .Targeting(cardPlay.Target)
//           .WithHitFx(...)                       // cosmetic, ignored
//           .Execute(...);
//       if (!HasBeenPlayedThisTurn)
//           await CardPileCmd.Draw(ctx, Cards.BaseValue /* 1 */, Owner);
//   }
//
// CanonicalVars:
//   OstyDamageVar(3m, Move)  → base_damage 3 (cosmetic metadata; the hit is dealt
//                              by the Osty, not the player). OnUpgrade +3 → 6.
//   CardsVar(1)              → draw 1 (CardPileCmd.Draw → draw_via_state). Not
//                              touched by OnUpgrade.
// CanonicalTags: OstyAttack (cosmetic glow tag; not carried in CardSpec here).
// ShouldGlowRedInternal => Owner.IsOstyMissing — pure UI, ignored.
// ShouldGlowGoldInternal => !HasBeenPlayedThisTurn — pure UI, ignored.
//
// Both the damage AND the draw sit inside the `if (!CheckMissingWithAnim)` block,
// so when the Osty is missing the entire effect is a no-op. `deal_damage_from_
// osty_to_enemy` is itself a no-op when the Osty is absent (`osty_dealer_ref` →
// None); we gate the draw on the same Osty-alive check to reproduce that.
//
// BLOCKER (recorded, not faked): the C# draw is additionally gated on
// `!HasBeenPlayedThisTurn`, where `HasBeenPlayedThisTurn` scans
// `CombatManager.History.CardPlaysFinished` for a *finished* play of THIS card
// instance during the current turn. The engine has no per-card-instance
// finished-play history (no `times_played_this_turn`/play-history surface — see
// the VarKey table in sts2-core/src/cards.rs, which has no such slot). On the
// first play of the turn the current OnPlay has not finished, so the history is
// empty and the card draws; this handler therefore draws unconditionally, which
// is FAITHFUL for the single-play-per-turn case (the overwhelmingly common one).
// It diverges only when Fetch is returned to hand and replayed within the same
// turn — C# would then skip the draw, but this handler still draws. Implementing
// the guard requires a per-instance turn-scoped played-flag (e.g. a `VarKey::
// FetchPlayedThisTurn` set after resolve and cleared at turn cleanup, mirroring
// the `FreeThisTurn`/`Bolas` per-card turn-flag pattern).
pub(crate) fn play_fetch(state: &mut CombatState, target: Option<CreatureUid>) -> EffectResult {
    let t = target.expect("Fetch: enemy target required");

    // OstyDamage: 3 base, +3 on upgrade. Computed here (the OstyDamage is not a
    // plain player-attack stat), gated on current_card_upgraded like the sibling
    // OstyAttack cards (poke/sic_em/unleash/bone_shards). No-op if the Osty is
    // missing (deal_damage_from_osty_to_enemy resolves to a no-op).
    let dmg = if current_card_upgraded(state) { 6 } else { 3 };
    deal_damage_from_osty_to_enemy(state, t, dmg);
    sts2_core::attack_command::end_card_command(state);

    // C# `if (!HasBeenPlayedThisTurn) Draw(Cards.BaseValue=1)` — gated inside the
    // Osty-alive block. We only draw when the Osty is alive (matching the C#
    // guard placement); the HasBeenPlayedThisTurn half is the recorded blocker
    // above (drawn unconditionally on each play for lack of play-history infra).
    if osty_dealer_ref(state).is_some() {
        let _ = sts2_core::cards::draw_via_state(state, 1);
    }

    EffectResult::Resolved
}

#[must_use]
pub(crate) const fn spec() -> CardSpec {
    CardSpec {
        id: CardId::Fetch,
        name: "FETCH",
        cost: 0,
        card_type: CardType::Attack,
        target_type: TargetType::Enemy, // C# TargetType.AnyEnemy → single-player Enemy
        rarity: CardRarity::Uncommon,
        base_damage: 3, // OstyDamageVar(3m) — cosmetic; dealt via the Osty
        base_block: 0,
        keywords: CardKeywords::NONE,
        ..CardSpec::base()
    }
}

#[allow(clippy::unnecessary_wraps, dead_code)]
pub(crate) fn upgrade() -> Option<CardUpgradeDelta> {
    // C# OnUpgrade: OstyDamage.UpgradeValueBy(3m) (3 → 6). The OstyDamage is not
    // a plain player-attack stat (it is dealt via the Osty and computed in the
    // handler), so the bump is applied via current_card_upgraded rather than a
    // static spec delta — matching the sibling OstyAttack cards. Returns None to
    // keep a single source of truth for the upgraded magnitude.
    None
}
