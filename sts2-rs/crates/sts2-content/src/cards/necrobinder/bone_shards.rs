//! Bone Shards (Necrobinder). C#-first port via the character-port skill.
use super::prelude::*;

/// Bone Shards (Uncommon, 1 energy, Attack, AllEnemies). The Osty attacks
/// every enemy for 9 (12 upgraded), the player gains 9 block (12 upgraded),
/// then the Osty is killed. Does nothing if the Osty is missing.
//
// C# source: ../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Cards/BoneShards.cs
//
// C# OnPlay:
//   if (!Osty.CheckMissingWithAnim(Owner)) {
//       DamageCmd.Attack(OstyDamage.BaseValue)   // 9, +3 on upgrade → 12
//           .FromOsty(Owner.Osty, this)          // attacker is the OSTY, not the player
//           .TargetingAllOpponents(CombatState)
//           .Execute(...);
//       CreatureCmd.GainBlock(Owner.Creature, Block, cardPlay);   // 9, +3 on upgrade → 12
//       if (Owner.IsOstyAlive) CreatureCmd.Kill(Owner.Osty);
//   }
//
// CanonicalVars:
//   OstyDamageVar(9m, Move)  → base_damage 9 (Move value-prop). OnUpgrade +3 → 12.
//   BlockVar(9m, Move)       → base_block 9.                    OnUpgrade +3 → 12.
// GainsBlock => true (cosmetic flag; block is granted in OnPlay).
// CanonicalTags: OstyAttack → CardTag::OstyAttack (cosmetic glow tag; already
//                exists core-side, not carried in CardSpec metadata here).
// ShouldGlowRedInternal => Owner.IsOstyMissing — pure UI, ignored.
//
// Osty-missing guard IS faithful: when the Osty is absent/dead the whole card
// (damage + block + kill) is a no-op, matching `!CheckMissingWithAnim`.
pub(crate) fn play_bone_shards(
    state: &mut CombatState,
    _target: Option<CreatureUid>,
) -> EffectResult {
    // C# `!Osty.CheckMissingWithAnim(Owner)` — only resolve when the Osty is
    // present and alive (mirror via the player's pet creature). Matches the
    // sibling OstyAttack cards (poke/unleash).
    let osty_alive = state
        .player
        .pet_uid
        .and_then(|uid| state.creatures.iter().find(|c| c.id == uid.0))
        .is_some_and(|c| c.current_hp > 0);
    if !osty_alive {
        return EffectResult::Resolved;
    }

    // OstyDamage.BaseValue: 9 base, +3 on upgrade. BlockVar: 9 base, +3 on
    // upgrade (both OnUpgrade UpgradeValueBy(3m)). Magnitudes are computed in
    // the handler, so the upgrade bumps are gated on current_card_upgraded.
    let (dmg, block) = if current_card_upgraded(state) {
        (12, 12)
    } else {
        (9, 9)
    };

    // C# `DamageCmd.Attack(dmg).FromOsty(Owner.Osty, this).TargetingAllOpponents(...)`
    // — the Osty is the dealer for the AoE hit (Osty-side modifiers apply;
    // player-dealer-gated effects do not fire). `deal_damage_from_osty_to_all_
    // alive_enemies` resolves the dealer as the pet creature (see `osty_dealer_ref`).
    deal_damage_from_osty_to_all_alive_enemies(state, dmg);
    sts2_core::attack_command::end_card_command(state);

    // C# `CreatureCmd.GainBlock(Owner.Creature, Block, cardPlay)` — player block.
    gain_block_player(state, block);

    // C# `if (Owner.IsOstyAlive) CreatureCmd.Kill(Owner.Osty)` — sacrifice the
    // Osty after the strike. `kill_osty` zeroes its HP but retains the corpse
    // (DieForYou) so a later summon revives it. The `osty_alive` guard above is
    // the IsOstyAlive check; `kill_osty` is itself a no-op if already dead.
    state.kill_osty();

    EffectResult::Resolved
}

#[must_use]
pub(crate) const fn spec() -> CardSpec {
    CardSpec {
        id: CardId::BoneShards,
        name: "BONE_SHARDS",
        cost: 1,
        card_type: CardType::Attack,
        target_type: TargetType::AllEnemies,
        rarity: CardRarity::Uncommon,
        base_damage: 9, // OstyDamageVar(9m)
        base_block: 9,  // BlockVar(9m)
        keywords: CardKeywords::NONE,
        ..CardSpec::base()
    }
}

#[allow(clippy::unnecessary_wraps, dead_code)]
pub(crate) fn upgrade() -> Option<CardUpgradeDelta> {
    // C# OnUpgrade: OstyDamage.UpgradeValueBy(3m) (9 → 12) and
    // Block.UpgradeValueBy(3m) (9 → 12). The damage/block magnitudes are
    // computed in the handler from current_card_upgraded (the OstyDamage is not
    // a plain attack stat), so the bumps are applied there rather than via a
    // static spec delta; this returns None to keep the single source of truth.
    None
}
