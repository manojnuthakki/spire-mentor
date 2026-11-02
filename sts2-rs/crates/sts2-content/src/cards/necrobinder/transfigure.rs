//! Necrobinder Transfigure. Ported C#-first via the character-port skill.
//! C# source:
//! ../sts2-wiki/decompiled/v0.103.2/MegaCrit.Sts2.Core.Models.Cards/Transfigure.cs
//!
//! ctor: `base(1, CardType.Skill, CardRarity.Rare, TargetType.Self)`.
//! `CanonicalKeywords`: Exhaust. `CanonicalVars`: `EnergyVar(1)` (the +1 cost
//! magnitude). The ReplayStatic ExtraHoverTip is cosmetic.
//!
//! `OnPlay`:
//!   `CardSelectCmd.FromHand(prefs(prompt, 1))` — choose up to 1 hand card.
//!   For the chosen card:
//!     1. if `!EnergyCost.CostsX && EnergyCost.GetWithModifiers(None) >= 0`
//!        then `EnergyCost.AddThisCombat(1)` (bump cost +1 this combat).
//!     2. `BaseReplayCount++` (unconditionally) — card replays one extra time.
//!
//! `OnUpgrade`: `RemoveKeyword(CardKeyword.Exhaust)`. The spec carries EXHAUST;
//! the upgrade delta (central table `card_upgrades_generated.rs`,
//! bead `sts2-rs-3ebox`) clears keywords → NONE. `upgrade()` returns `None`.
use super::prelude::*;

/// Transfigure (Rare, 1 energy, Skill, Self, Exhaust). Choose a card in hand;
/// increase its cost by 1 this combat (unless X-cost / already free below 0)
/// and make it Replay (play one extra time). Upgrade: removes Exhaust.
pub(crate) fn play_transfigure(state: &mut CombatState, _t: Option<CreatureUid>) -> EffectResult {
    if sts2_core::combat_loop::combat_is_ending(state) {
        return EffectResult::Resolved;
    }
    // C# `CardSelectCmd.FromHand(prefs(prompt, 1), filter: null, this)` — a
    // forced single-pick (min=max=1) over the whole hand. This is a real
    // player-driven prompt (same family as Begone / Guards): `FromHand`
    // auto-applies (no prompt) only when `cards.Count <= MinSelect` (≤1 hand
    // card), and surfaces a `card_select` for 2+ cards. The prior port
    // auto-picked the FIRST hand card inline, which desynced whole-run parity
    // vs the C# headless agent's pick (seed PARITY125 Necrobinder A0: C#
    // entered `card_select` at the Transfigure play, Rust stayed in `combat`).
    match state.player.hand.as_slice() {
        // Empty hand → FirstOrDefault() == null → no transform.
        [] => {}
        // FromHand auto-applies to the lone hand card (cards.Count <= MinSelect).
        &[uid] => apply_transfigure_to(state, uid),
        // 2+ hand cards: open a single-pick choice in hand order (C# `FromHand`
        // passes the hand with NO OrderBy). The resolver mutates the chosen card.
        _ => {
            let options: Vec<sts2_core::ChoiceOption> = state
                .player
                .hand
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
                sts2_core::ChoiceKind::TransfigureCardInHand,
                options,
                1,
                1,
                false,
            );
        }
    }
    EffectResult::Resolved
}

/// Apply Transfigure's effect to one chosen hand card: bump its cost +1 this
/// combat (unless X-cost or already free below 0) and increment its
/// `base_replay_count`. Mirrors C# `Transfigure.OnPlay` per-card body
/// (Transfigure.cs:33-37). The cost guard `!item.EnergyCost.CostsX &&
/// item.EnergyCost.GetWithModifiers(None) >= 0` maps to `!is_x_cost()` (CostsX
/// is cost == -1 for non-curse/status) and `cost >= 0`; the `BaseReplayCount++`
/// fires unconditionally, even when the cost bump is skipped. The same body is
/// used by the auto-apply (≤1 hand card) fast path and the
/// [`sts2_core::ChoiceKind::TransfigureCardInHand`] resolver.
fn apply_transfigure_to(state: &mut CombatState, uid: sts2_core::CardUid) {
    let idx = uid.0.get() as usize - 1;
    if let Some(card) = state.cards.get_mut(idx) {
        if !card.is_x_cost() && card.cost >= 0 {
            card.cost = card.cost.saturating_add(1);
        }
        card.base_replay_count = card.base_replay_count.saturating_add(1);
    }
}

#[must_use]
pub(crate) const fn spec() -> CardSpec {
    CardSpec {
        id: CardId::Transfigure,
        name: "TRANSFIGURE",
        cost: 1,
        card_type: CardType::Skill,
        target_type: TargetType::Self_,
        rarity: CardRarity::Rare,
        base_damage: 0,
        base_block: 0,
        keywords: CardKeywords::EXHAUST,
        ..CardSpec::base()
    }
}

#[allow(clippy::unnecessary_wraps, dead_code)]
pub(crate) fn upgrade() -> Option<CardUpgradeDelta> {
    // C# OnUpgrade `RemoveKeyword(Exhaust)` lives in the central upgrade table
    // (`card_upgrades_generated.rs`, bead `sts2-rs-3ebox`): EXHAUST → NONE.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use smallvec::SmallVec;

    fn mint(state: &mut CombatState, id: CardId) -> sts2_core::CardUid {
        let mut card = crate::cards::spec_index::card_pod_from_id(id, false).expect("spec exists");
        let raw = u32::try_from(state.cards.len() + 1).expect("fits");
        let uid = sts2_core::CardUid::new(raw).expect("nz");
        card.uid = uid;
        state.cards.push(card);
        uid
    }

    /// 2+ hand cards: C# `CardSelectCmd.FromHand(prefs(prompt, 1))` surfaces a
    /// single-pick `card_select` (count > MinSelect). Resolving option index 1
    /// must mutate the SECOND card (cost +1, base_replay_count +1), not the
    /// first — proving the choice picks the chosen card, not pick-first. This is
    /// the PARITY125 Necrobinder A0 fix.
    #[test]
    fn transfigure_two_hand_cards_opens_select_and_mutates_chosen() {
        crate::cards::dispatch::install_default_registry();
        let mut state = CombatState::default();
        // Alive player -- dead trips the request_card_choice ending guard.
        state.player.current_hp = 80;
        // Two distinct, observable hand cards (both cost-1 base).
        let a = mint(&mut state, CardId::StrikeNecrobinder);
        let b = mint(&mut state, CardId::DefendNecrobinder);
        state.player.hand.extend([a, b]);
        let a_cost0 = state.cards[a.0.get() as usize - 1].cost;
        let b_cost0 = state.cards[b.0.get() as usize - 1].cost;
        // The resolving Transfigure sits on top of the Play pile.
        let trans = mint(&mut state, CardId::Transfigure);
        state.player.play.push(trans);

        let r = play_transfigure(&mut state, None);
        assert_eq!(r, EffectResult::Resolved);
        // 2+ hand cards → a single-pick choice opens (no auto-apply).
        let pc = state
            .pending_choice
            .as_ref()
            .expect("Transfigure opens a choice with 2+ hand cards");
        assert_eq!(pc.kind, sts2_core::ChoiceKind::TransfigureCardInHand);
        assert_eq!(pc.min_select, 1);
        assert_eq!(pc.max_select, 1);
        assert_eq!(
            pc.options.len(),
            2,
            "both hand cards are options, in hand order"
        );
        assert_eq!(pc.options[0].card_uid, Some(a));
        assert_eq!(pc.options[1].card_uid, Some(b));

        // Pick option 1 (the SECOND card) — must mutate `b`, not `a`.
        let selection: SmallVec<[u8; 4]> = [1u8].into_iter().collect();
        let res = sts2_core::apply(&mut state, sts2_core::Action::ResolveChoice { selection });
        assert_eq!(res, sts2_core::StepResult::Advanced);
        assert!(state.pending_choice.is_none());
        // `a` untouched; `b` got cost +1 and base_replay_count +1.
        assert_eq!(
            state.cards[a.0.get() as usize - 1].cost,
            a_cost0,
            "unchosen card untouched"
        );
        assert_eq!(state.cards[a.0.get() as usize - 1].base_replay_count, 0);
        assert_eq!(
            state.cards[b.0.get() as usize - 1].cost,
            b_cost0 + 1,
            "chosen card cost bumped +1"
        );
        assert_eq!(
            state.cards[b.0.get() as usize - 1].base_replay_count,
            1,
            "chosen card gains one replay"
        );
    }

    /// Exactly one hand card: C# `FromHand` auto-applies (count <= MinSelect)
    /// with NO prompt; Rust mutates the lone card directly.
    #[test]
    fn transfigure_one_hand_card_auto_applies_no_choice() {
        let mut state = CombatState::default();
        let a = mint(&mut state, CardId::StrikeNecrobinder);
        state.player.hand.push(a);
        let a_cost0 = state.cards[a.0.get() as usize - 1].cost;
        let trans = mint(&mut state, CardId::Transfigure);
        state.player.play.push(trans);

        let r = play_transfigure(&mut state, None);
        assert_eq!(r, EffectResult::Resolved);
        assert!(
            state.pending_choice.is_none(),
            "no choice surfaced for the lone card"
        );
        assert_eq!(
            state.cards[a.0.get() as usize - 1].cost,
            a_cost0 + 1,
            "lone card cost bumped +1"
        );
        assert_eq!(state.cards[a.0.get() as usize - 1].base_replay_count, 1);
    }

    /// Empty hand: FirstOrDefault() == null → no-op, no choice.
    #[test]
    fn transfigure_empty_hand_is_a_noop() {
        let mut state = CombatState::default();
        let trans = mint(&mut state, CardId::Transfigure);
        state.player.play.push(trans);
        let r = play_transfigure(&mut state, None);
        assert_eq!(r, EffectResult::Resolved);
        assert!(state.pending_choice.is_none());
    }
}
