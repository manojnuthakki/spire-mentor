//! Hand-authored from the C# decompilation (character-port skill).
use super::prelude::*;

/// Reave (Common, 1 energy, Attack). Deal `DamageVar`(9) damage to the
/// target, then generate `CardsVar`(1) Soul token(s) into the Draw pile.
/// When Reave is upgraded the generated Soul(s) are themselves upgraded.
//
// C# source:
// ../sts2-wiki/decompiled/v0.107.1/MegaCrit.Sts2.Core.Models.Cards/Reave.cs
//   OnPlay: DamageCmd.Attack(Damage.BaseValue).Targeting(Target);
//           souls = Soul.Create(Owner, Cards.IntValue=1);
//           if (IsUpgraded) foreach soul -> CardCmd.Upgrade(soul);
//           CardPileCmd.AddGeneratedCardsToCombat(souls, PileType.Draw,
//               base.Owner, CardPilePosition.Random);
//   OnUpgrade: Damage.UpgradeValueBy(2m)  (9 -> 11; spec-level, via upgrade()).
pub(crate) fn play_reave(state: &mut CombatState, target: Option<CreatureUid>) -> EffectResult {
    let t = target.expect("Reave: AnyEnemy-targeted");
    let _ = deal_damage_player_to_enemy(state, t, current_damage(state));
    sts2_core::attack_command::end_card_command(state);

    // C# Cards.IntValue == 1; Reave's OnUpgrade does not touch CardsVar, so
    // always generate exactly one Soul. If Reave is upgraded, the spawned
    // Soul is upgraded too (CardCmd.Upgrade on each generated Soul).
    let upgrade_souls = current_card_upgraded(state);
    let mut soul = card_from_spec(&status_specs::soul());
    soul.upgraded = upgrade_souls;
    // v0.107.1 `AddGeneratedCardsToCombat(souls, PileType.Draw, base.Owner,
    // CardPilePosition.Random)` inserts the Soul at a RANDOM draw-pile depth
    // `Rng.Shuffle.NextInt(targetPile.Cards.Count + 1)` (`CardPileCmd.cs:515`),
    // consuming one Shuffle-stream read — NOT the v0.103.2 default Bottom.
    // Matches the sibling Necrobinder soul-spawners (grave_warden, dirge,
    // severance, capture_spirit, glimpse_beyond).
    let _ = spawn_card_into_draw_pile_random_position(state, soul);

    EffectResult::Resolved
}

#[must_use]
pub(crate) const fn spec() -> CardSpec {
    CardSpec {
        id: CardId::Reave,
        name: "REAVE",
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
    // C# OnUpgrade: Damage.UpgradeValueBy(2) -> 9 + 2 = 11.
    Some(CardUpgradeDelta {
        cost: None,
        base_damage: Some(11),
        base_block: None,
        keywords: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mint_into_play(state: &mut CombatState, id: CardId) -> sts2_core::CardUid {
        let mut card = crate::cards::spec_index::card_pod_from_id(id, false).expect("spec exists");
        let raw = u32::try_from(state.cards.len() + 1).expect("fits");
        let uid = sts2_core::CardUid::new(raw).expect("nz");
        card.uid = uid;
        state.cards.push(card);
        state.player.play.push(uid);
        uid
    }

    fn mint_into_draw(state: &mut CombatState, id: CardId) -> sts2_core::CardUid {
        let mut card = crate::cards::spec_index::card_pod_from_id(id, false).expect("spec exists");
        let raw = u32::try_from(state.cards.len() + 1).expect("fits");
        let uid = sts2_core::CardUid::new(raw).expect("nz");
        card.uid = uid;
        state.cards.push(card);
        state.player.draw.push(uid);
        uid
    }

    /// v0.107.1 `Reave.OnPlay` spawns the generated Soul via
    /// `AddGeneratedCardsToCombat(.., CardPilePosition.Random)`, which inserts
    /// at `Rng.Shuffle.NextInt(targetPile.Cards.Count + 1)` (`CardPileCmd.cs:515`)
    /// — a random draw-pile depth that consumes exactly ONE Shuffle-stream read.
    /// The v0.103.2 default `Bottom` (regressed here) appended the Soul to the
    /// pile bottom and consumed NO RNG, desyncing both draw order and every
    /// later card's Shuffle stream.
    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn reave_spawns_soul_at_random_draw_position_consuming_one_shuffle_read() {
        let mut state = CombatState::default();
        state.run_rng = sts2_core::run_rng_set::RunRngSet::new(12345);
        // Keep the player alive so combat is not flagged as ending (a pending
        // loss would short-circuit the soul-spawn Shuffle read).
        state.player.current_hp = 50;
        let enemy = state.spawn_enemy(sts2_core::MonsterKind::Nibbit);
        // Keep the enemy alive through Reave's 9 damage so combat does NOT end
        // (a lethal soul-spawn correctly short-circuits the Shuffle read).
        if let Some(c) = state.creatures.iter_mut().find(|c| c.id == enemy.0) {
            c.current_hp = 99;
            c.max_hp = 99;
        }
        // A multi-card draw pile so a non-bottom insertion is observable.
        for _ in 0..4 {
            mint_into_draw(&mut state, CardId::StrikeNecrobinder);
        }
        let len_before = state.player.draw.len();

        // Predict the insert index off a clone of the live Shuffle stream:
        // Random => NextInt(Count + 1) == next_int(0, len) (inclusive).
        let mut predicted_stream = state.run_rng.shuffle.clone();
        let expected_idx = predicted_stream.next_int(0, len_before as i32) as usize;

        mint_into_play(&mut state, CardId::Reave);
        let r = play_reave(&mut state, Some(enemy));
        assert_eq!(r, EffectResult::Resolved);

        // Exactly one Shuffle read consumed (Bottom would consume none).
        assert_eq!(
            state.run_rng.shuffle, predicted_stream,
            "Reave consumes exactly one Rng.Shuffle read (CardPilePosition.Random), not Bottom"
        );
        // The Soul landed at the predicted random index, not the pile bottom.
        assert_eq!(
            state.player.draw.len(),
            len_before + 1,
            "one Soul added to the draw pile"
        );
        let soul_uid = state.player.draw[expected_idx];
        assert_eq!(
            state.cards[soul_uid.0.get() as usize - 1].card_id,
            CardId::Soul,
            "generated Soul sits at the random-position index, not appended at bottom"
        );
    }
}
