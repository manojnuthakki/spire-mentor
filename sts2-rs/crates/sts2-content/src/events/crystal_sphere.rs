//! The Crystal Sphere minigame's run side (lane F's F8, `crystal-sphere`): a
//! divination reveals items, a revealed curse goes into the deck at once, and
//! the last divination offers the revealed items as one custom reward set.
//! The grid and its rules are `sts2_core::crystal_sphere`.
//!
//! C# `CompleteMinigame` → `OneOffSynchronizer.OfferCrystalSphereRewards`:
//! every revealed item's `ToReward(owner, rng)` runs first, in the order the
//! items were revealed (a potion item rolls its potion there: one `NextItem`
//! over `GetPotionOptions` of its rarity); then `RewardsSet.GenerateWithoutOffering`
//! populates every reward in that order (a gold reward draws
//! `NextInt(amount, amount + 1)`, a relic reward a rarity float and the next
//! grab-bag relic of it, a card reward its three cards), all on the event's RNG;
//! then the set is sorted by `RewardsSetIndex` (gold, potion, relic, card).

use sts2_core::cards::CardId;
use sts2_core::crystal_sphere::{DivineError, ItemKind, SphereCardRarity, Tool};
use sts2_core::potions::PotionRarity;
use sts2_core::runstate::{OfferedReward, RelicRarity};
use sts2_core::{RelicId, RelicReward, RunState};

/// `RewardsSetIndex` of each reward kind (`GoldReward` 1, `PotionReward` 2,
/// `RelicReward` 3, `CardReward` 5).
fn set_index(r: &OfferedReward) -> u8 {
    match r {
        OfferedReward::Gold(_) => 1,
        OfferedReward::Potion(_) => 2,
        OfferedReward::Relic(_) | OfferedReward::RandomRelic { .. } => 3,
        _ => 5,
    }
}

/// `RunManager::take_action`'s `CrystalSphereDivine` callback
/// ([`sts2_core::run_manager::ContentCallbacks::crystal_sphere_divine`]).
///
/// # Errors
/// The grid's refusals (`DivineError`): a cell off the grid or already clear,
/// or no divination left.
pub fn divine(rs: &mut RunState, x: i32, y: i32, big: bool) -> Result<bool, DivineError> {
    let tool = if big { Tool::Big } else { Tool::Small };
    let sphere = rs.crystal_sphere.as_mut().ok_or(DivineError::Finished)?;
    let revealed = sphere.divine(x, y, tool)?;
    let finished = sphere.is_finished();
    let curses = revealed
        .iter()
        .filter(|&&i| sphere.items()[i].kind == ItemKind::Curse)
        .count();
    for _ in 0..curses {
        // C# `CrystalSphereCurse.RevealItem`: `AddCurseToDeck<Doubt>`.
        if let Some(doubt) = crate::cards::spec_index::card_pod_from_id(CardId::Doubt, false) {
            rs.add_card_to_deck(doubt, sts2_core::relic_effects::AddSource::EventGain);
        }
    }
    if finished {
        finish(rs);
    }
    Ok(finished)
}

/// Close the minigame and offer what it revealed (see the module docs).
fn finish(rs: &mut RunState) {
    let Some(sphere) = rs.crystal_sphere.take() else {
        return;
    };
    let character = rs.character_id;
    let mut rng = sphere.rng.clone();
    let kinds: Vec<ItemKind> = sphere
        .revealed()
        .iter()
        .map(|&i| sphere.items()[i].kind)
        .filter(|k| *k != ItemKind::Curse)
        .collect();
    // `ToReward`, in revealed order: only a potion item draws here.
    let potions: Vec<Option<sts2_core::potions::PotionKind>> = kinds
        .iter()
        .map(|k| match k {
            ItemKind::Potion { rare } => crate::potions::all::roll_potion_of_rarity(
                &mut rng,
                character,
                if *rare {
                    PotionRarity::Rare
                } else {
                    PotionRarity::Common
                },
            ),
            _ => None,
        })
        .collect();
    // `Populate`, in revealed order.
    let mut offered: Vec<OfferedReward> = Vec::new();
    for (kind, potion) in kinds.iter().zip(potions) {
        match kind {
            ItemKind::Gold { big } => {
                let amount = if *big { 30 } else { 10 };
                let drawn = rng.next_int_exclusive(amount, amount + 1);
                offered.push(OfferedReward::Gold(drawn));
            }
            ItemKind::Potion { .. } => {
                if let Some(p) = potion {
                    offered.push(OfferedReward::Potion(Some(p)));
                }
            }
            ItemKind::Relic => {
                let rarity: RelicRarity = sts2_core::runstate::roll_relic_rarity(&mut rng);
                let id = rs
                    .pop_relic_from_grab_bag(rarity)
                    .unwrap_or(RelicId::Circlet);
                offered.push(OfferedReward::Relic(RelicReward::new(id)));
            }
            ItemKind::CardReward(r) => {
                let rarity = match r {
                    SphereCardRarity::Common => crate::cards::CardRarity::Common,
                    SphereCardRarity::Uncommon => crate::cards::CardRarity::Uncommon,
                    SphereCardRarity::Rare => crate::cards::CardRarity::Rare,
                };
                let cards = crate::cards::spec_index::roll_card_reward_of_rarity(
                    rs, &mut rng, rarity, 3, character,
                );
                offered.push(OfferedReward::Card {
                    options: Some(cards.iter().map(|c| c.card_id).collect()),
                    upgraded: cards.iter().map(|c| c.upgraded).collect(),
                    no_upgrade_roll: false,
                });
            }
            ItemKind::Curse => {}
        }
    }
    // `Rewards.Sort(RewardsSetIndex)`: stable for a list this short.
    offered.sort_by_key(set_index);
    if rs.interactive_rewards {
        rs.offer_custom_rewards(offered, true);
    }
}
