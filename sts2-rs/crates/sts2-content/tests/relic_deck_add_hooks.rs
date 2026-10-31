//! A relic's deck add goes through the game's deck add (CardPileCmd.Add to the
//! Deck, directly or via AddCurseToDeck): Hook.ModifyCardBeingAddedToDeck, then
//! Hook.AfterCardChangedPiles (calling-bell-curse, review round 2: F6-F8).
//!
//! - FresnelLens.TryModifyCardBeingAddedToDeck enchants only when
//!   Nimble.CanEnchant(card): EnchantmentModel.CanEnchant (not Status, Curse or
//!   Quest; no enchantment already, Nimble not stackable) and card.GainsBlock.
//! - Toxic/Molten/Frozen Egg: CardCmd.Upgrade on a clone, the card's whole
//!   OnUpgrade (cost, numbers, keywords).
//! - PreservedFog.AfterObtained awaits its three removals, then
//!   AddCurseToDeck<Folly>.

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_with_ascension,
};
use sts2_core::cards::{Card, CardId, EnchantmentId, KeywordOverride};
use sts2_core::potions::CharacterId;
use sts2_core::relics::RelicId;
use sts2_core::run_manager::{Phase, RunAction, RunManager};
use sts2_core::runstate::{OfferedReward, RelicReward};

fn on_the_map() -> RunManager {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
    let mut rm = new_run_manager_for_character_with_ascension(3, CharacterId::Ironclad, 0);
    initialize_run_setup(&mut rm);
    for _ in 0..50 {
        if rm.phase() == Phase::MapChoice {
            break;
        }
        let a = rm.get_available_actions()[0];
        rm.take_action(a);
    }
    rm.run_state.interactive_rewards = true;
    rm
}

fn obtain(rm: &mut RunManager, relic: RelicId) {
    rm.run_state
        .offer_custom_rewards(vec![OfferedReward::Relic(RelicReward::new(relic))], true);
    // Any action opens the offered set (this one is refused on the map).
    rm.take_action(RunAction::ProceedRewards);
    rm.take_action(RunAction::ClaimReward { idx: 0 });
    assert!(rm.run_state.relics.contains(&relic));
}

fn added(rm: &RunManager, id: CardId) -> Vec<Card> {
    rm.run_state
        .master_deck
        .iter()
        .filter(|c| c.card_id == id)
        .copied()
        .collect()
}

fn nimble(c: &Card) -> i32 {
    c.enchantment(EnchantmentId::Nimble)
}

// F6: Fresnel Lens enchants only a card Nimble can enchant.

/// Relax gains block (Relax.GainsBlock => true): Pael's Horn's two get Nimble 2.
#[test]
fn fresnel_lens_enchants_a_block_card_a_relic_adds() {
    let mut rm = on_the_map();
    rm.run_state.relics.push(RelicId::FresnelLens);
    obtain(&mut rm, RelicId::PaelsHorn);
    let relax = added(&rm, CardId::Relax);
    assert_eq!(relax.len(), 2);
    assert!(
        relax.iter().all(|c| nimble(c) == 2),
        "Nimble 2 on each Relax"
    );
}

/// A curse is never enchantable (EnchantmentModel.CanEnchant: type Status,
/// Curse or Quest is refused): Calling Bell's curse stays plain.
#[test]
fn fresnel_lens_leaves_curse_of_the_bell_unenchanted() {
    let mut rm = on_the_map();
    rm.run_state.relics.push(RelicId::FresnelLens);
    obtain(&mut rm, RelicId::CallingBell);
    let curse = added(&rm, CardId::CurseOfTheBell);
    assert_eq!(curse.len(), 1);
    assert_eq!(nimble(&curse[0]), 0, "no Nimble on Curse of the Bell");
    assert!(curse[0].enchantments.is_empty());
}

/// A card that gains no block is refused too (Nimble.CanEnchant): Apotheosis.
#[test]
fn fresnel_lens_leaves_a_blockless_card_unenchanted() {
    let mut rm = on_the_map();
    rm.run_state.relics.push(RelicId::FresnelLens);
    obtain(&mut rm, RelicId::JewelryBox);
    let apo = added(&rm, CardId::Apotheosis);
    assert_eq!(apo.len(), 1);
    assert_eq!(nimble(&apo[0]), 0, "no Nimble on Apotheosis");
}

// F7: an egg applies the card's whole upgrade.

/// Toxic Egg upgrades Jewelry Box's Apotheosis: Apotheosis.OnUpgrade
/// EnergyCost.UpgradeBy(-1), 2 -> 1.
#[test]
fn toxic_egg_upgrades_jewelry_box_apotheosis_to_cost_1() {
    let mut rm = on_the_map();
    rm.run_state.relics.push(RelicId::ToxicEgg);
    obtain(&mut rm, RelicId::JewelryBox);
    let apo = added(&rm, CardId::Apotheosis);
    assert_eq!(apo.len(), 1);
    assert!(apo[0].upgraded);
    assert_eq!(apo[0].cost, 1, "upgraded Apotheosis costs 1");
}

/// Apparition.OnUpgrade RemoveKeyword(Ethereal): Distinguished Cape's three
/// under Toxic Egg keep Exhaust only.
#[test]
fn toxic_egg_upgrades_distinguished_cape_apparitions_without_ethereal() {
    let mut rm = on_the_map();
    rm.run_state.relics.push(RelicId::ToxicEgg);
    obtain(&mut rm, RelicId::DistinguishedCape);
    let app = added(&rm, CardId::Apparition);
    assert_eq!(app.len(), 3);
    for c in &app {
        assert!(c.upgraded);
        assert_eq!(
            c.keyword_override,
            Some(KeywordOverride::EXHAUST),
            "upgraded Apparition: Exhaust, no Ethereal"
        );
    }
}

/// Relax.OnUpgrade Block.UpgradeValueBy(2): 15 -> 17.
#[test]
fn toxic_egg_upgrades_paels_horn_relax_to_block_17() {
    let mut rm = on_the_map();
    rm.run_state.relics.push(RelicId::ToxicEgg);
    obtain(&mut rm, RelicId::PaelsHorn);
    let relax = added(&rm, CardId::Relax);
    assert_eq!(relax.len(), 2);
    assert!(relax.iter().all(|c| c.upgraded && c.base_block == 17));
}

// F8: Preserved Fog adds Folly only after its removals.

#[test]
fn preserved_fog_adds_folly_after_the_removal_choice() {
    let mut rm = on_the_map();
    rm.run_state.interactive_card_removal = true;
    rm.run_state.relics.push(RelicId::LuckyFysh);
    rm.run_state.relics.push(RelicId::DarkstonePeriapt);
    let (gold, max_hp) = (rm.run_state.gold, rm.run_state.max_hp);
    obtain(&mut rm, RelicId::PreservedFog);
    assert_eq!(rm.run_state.pending_deck_removals, 3, "the removal is open");
    assert!(added(&rm, CardId::Folly).is_empty(), "no Folly yet");
    assert_eq!(rm.run_state.gold, gold, "no Lucky Fysh gold yet");
    assert_eq!(rm.run_state.max_hp, max_hp, "no Darkstone max HP yet");
    for _ in 0..3 {
        rm.take_action(RunAction::RemoveDeckCard { idx: 0 });
    }
    assert_eq!(rm.run_state.pending_deck_removals, 0);
    assert_eq!(
        added(&rm, CardId::Folly).len(),
        1,
        "Folly after the removals"
    );
    assert_eq!(rm.run_state.gold - gold, 15, "Lucky Fysh's gold");
    assert_eq!(
        rm.run_state.max_hp - max_hp,
        6,
        "Darkstone Periapt's max HP"
    );
}

/// Without the interactive removal the three cards go at once, then Folly,
/// with its hooks.
#[test]
fn preserved_fog_non_interactive_adds_folly_with_its_hooks() {
    let mut rm = on_the_map();
    rm.run_state.interactive_card_removal = false;
    rm.run_state.relics.push(RelicId::LuckyFysh);
    rm.run_state.relics.push(RelicId::DarkstonePeriapt);
    let (gold, max_hp, deck) = (
        rm.run_state.gold,
        rm.run_state.max_hp,
        rm.run_state.master_deck.len(),
    );
    obtain(&mut rm, RelicId::PreservedFog);
    assert_eq!(rm.run_state.master_deck.len(), deck - 3 + 1);
    assert_eq!(added(&rm, CardId::Folly).len(), 1);
    assert_eq!(rm.run_state.gold - gold, 15);
    assert_eq!(rm.run_state.max_hp - max_hp, 6);
}

// The sweep: every relic whose deck add shares push_deck_card, with Lucky Fysh,
// Darkstone Periapt, Fresnel Lens and the three eggs held.

struct Expect {
    relic: RelicId,
    card: CardId,
    count: usize,
    curse: bool,
    upgraded: bool,
    cost: i8,
    damage: i32,
    block: i32,
    nimble: i32,
    keywords: Option<KeywordOverride>,
    extra_gold: i32,
    extra_max_hp: i32,
}

#[test]
fn every_relic_deck_add_matches_the_game() {
    let none = None;
    #[rustfmt::skip]
    let cases = [
        // AddCurseToDeck: curses are never upgradable (MaxUpgradeLevel 0) nor enchantable.
        Expect { relic: RelicId::CallingBell, card: CardId::CurseOfTheBell, count: 1, curse: true, upgraded: false, cost: -1, damage: 0, block: 0, nimble: 0, keywords: none, extra_gold: 0, extra_max_hp: 0 },
        Expect { relic: RelicId::CursedPearl, card: CardId::Greed, count: 1, curse: true, upgraded: false, cost: -1, damage: 0, block: 0, nimble: 0, keywords: none, extra_gold: 333, extra_max_hp: 0 },
        Expect { relic: RelicId::BloodSoakedRose, card: CardId::Enthralled, count: 1, curse: true, upgraded: false, cost: 2, damage: 0, block: 0, nimble: 0, keywords: none, extra_gold: 0, extra_max_hp: 0 },
        Expect { relic: RelicId::PreservedFog, card: CardId::Folly, count: 1, curse: true, upgraded: false, cost: -1, damage: 0, block: 0, nimble: 0, keywords: none, extra_gold: 0, extra_max_hp: 0 },
        // Skills under Toxic Egg; Attacks under Molten Egg. Only Relax gains block.
        Expect { relic: RelicId::DistinguishedCape, card: CardId::Apparition, count: 3, curse: false, upgraded: true, cost: 1, damage: 0, block: 0, nimble: 0, keywords: Some(KeywordOverride::EXHAUST), extra_gold: 0, extra_max_hp: -9 },
        Expect { relic: RelicId::Storybook, card: CardId::BrightestFlame, count: 1, curse: false, upgraded: true, cost: 0, damage: 0, block: 0, nimble: 0, keywords: none, extra_gold: 0, extra_max_hp: 0 },
        Expect { relic: RelicId::NeowsTorment, card: CardId::NeowsFury, count: 1, curse: false, upgraded: true, cost: 1, damage: 14, block: 0, nimble: 0, keywords: none, extra_gold: 0, extra_max_hp: 0 },
        Expect { relic: RelicId::PaelsHorn, card: CardId::Relax, count: 2, curse: false, upgraded: true, cost: 3, damage: 0, block: 17, nimble: 2, keywords: none, extra_gold: 0, extra_max_hp: 0 },
        Expect { relic: RelicId::JewelryBox, card: CardId::Apotheosis, count: 1, curse: false, upgraded: true, cost: 1, damage: 0, block: 0, nimble: 0, keywords: none, extra_gold: 0, extra_max_hp: 0 },
        Expect { relic: RelicId::TanxsWhistle, card: CardId::Whistle, count: 1, curse: false, upgraded: true, cost: 3, damage: 44, block: 0, nimble: 0, keywords: none, extra_gold: 0, extra_max_hp: 0 },
    ];
    let mut failures = Vec::new();
    for e in &cases {
        let mut rm = on_the_map();
        rm.run_state.interactive_card_removal = true;
        for r in [
            RelicId::LuckyFysh,
            RelicId::DarkstonePeriapt,
            RelicId::FresnelLens,
            RelicId::ToxicEgg,
            RelicId::MoltenEgg,
            RelicId::FrozenEgg,
        ] {
            rm.run_state.relics.push(r);
        }
        let before = added(&rm, e.card).len();
        let (gold, max_hp) = (rm.run_state.gold, rm.run_state.max_hp);
        obtain(&mut rm, e.relic);
        while rm.run_state.pending_deck_removals > 0 {
            rm.take_action(RunAction::RemoveDeckCard { idx: 0 });
        }
        let cards = added(&rm, e.card);
        let mut got = Vec::new();
        if cards.len() - before != e.count {
            got.push(format!("count {}", cards.len() - before));
        }
        for c in &cards[before..] {
            let kw = if e.curse { none } else { c.keyword_override };
            if (
                c.upgraded,
                c.cost,
                c.base_damage,
                c.base_block,
                nimble(c),
                kw,
            ) != (e.upgraded, e.cost, e.damage, e.block, e.nimble, e.keywords)
            {
                got.push(format!(
                    "upgraded {} cost {} damage {} block {} nimble {} keywords {:?}",
                    c.upgraded,
                    c.cost,
                    c.base_damage,
                    c.base_block,
                    nimble(c),
                    c.keyword_override
                ));
            }
        }
        let want_gold = 15 * i32::try_from(e.count).unwrap() + e.extra_gold;
        let want_hp = if e.curse { 6 } else { 0 } + e.extra_max_hp;
        if rm.run_state.gold - gold != want_gold {
            got.push(format!(
                "gold {} (want {want_gold})",
                rm.run_state.gold - gold
            ));
        }
        if rm.run_state.max_hp - max_hp != want_hp {
            got.push(format!(
                "max hp {} (want {want_hp})",
                rm.run_state.max_hp - max_hp
            ));
        }
        if !got.is_empty() {
            failures.push(format!("{:?} {:?}: {}", e.relic, e.card, got.join("; ")));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
