//! F6 step37 (fresh DLL authority, sha 08faee0c...), run-level card generation
//! the engine drew from another pool or stream than the game:
//!
//! * `PrismaticGem.ModifyCardRewardCreationOptions` unions every unlocked
//!   character's pool into a card reward (`prismatic-gem-pools`).
//! * `PandorasBox.AfterObtained` transforms each removable basic Strike/Defend
//!   through `CreateRandomCardForTransform` on `Rng.Niche`
//!   (`pandoras-box-transform`).
use sts2_content::cards::spec_index::{
    card_pod_from_id, roll_card_reward_cards, specs_for_character, COLORLESS_SPECS,
};
use sts2_core::card_effects::EffectResult;
use sts2_core::cards::CardId;
use sts2_core::potions::CharacterId;
use sts2_core::run_effects::{
    dispatch_after_obtained, dispatch_modify_card_reward_creation_options,
};
use sts2_core::runstate::{CardRewardGenerationOptions, RunState};
use sts2_core::RelicId;

fn run(relics: &[RelicId]) -> RunState {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut rs = RunState::new();
    rs.character_id = CharacterId::Ironclad;
    rs.relics.extend_from_slice(relics);
    rs
}

fn ironclad_ids() -> Vec<CardId> {
    specs_for_character(CharacterId::Ironclad)
        .iter()
        .map(|f| f().id)
        .collect()
}

fn rewards_taken(rs: &mut RunState, n: usize) -> Vec<CardId> {
    (0..n)
        .flat_map(|_| {
            roll_card_reward_cards(rs, CardRewardGenerationOptions::default(), rs.character_id)
        })
        .map(|c| c.card_id)
        .collect()
}

#[test]
fn prismatic_gem_widens_the_reward_pool_to_every_character() {
    let mut rs = run(&[RelicId::PrismaticGem]);
    let mut opts = CardRewardGenerationOptions::default();
    let res = dispatch_modify_card_reward_creation_options(&mut rs, &mut opts);
    assert_eq!(res, EffectResult::Resolved);
    assert_eq!(
        opts.character_ids,
        vec![
            CharacterId::Ironclad,
            CharacterId::Silent,
            CharacterId::Regent,
            CharacterId::Necrobinder,
            CharacterId::Defect
        ],
        "ModelDb.AllCharacters order"
    );
    let iron = ironclad_ids();
    let ids = rewards_taken(&mut rs, 60);
    assert!(
        ids.iter().any(|id| !iron.contains(id)),
        "a Prismatic Gem reward offers another character's card"
    );
}

#[test]
fn negative_control_without_the_gem_the_pool_is_the_characters() {
    let mut rs = run(&[]);
    let iron = ironclad_ids();
    let ids = rewards_taken(&mut rs, 60);
    assert!(ids.iter().all(|id| iron.contains(id)));
    let mut opts = CardRewardGenerationOptions::default();
    assert_eq!(
        dispatch_modify_card_reward_creation_options(&mut rs, &mut opts),
        EffectResult::NotImplemented
    );
    assert!(opts.character_ids.is_empty());
}

#[test]
fn prismatic_gem_respects_the_no_modification_flag_and_foreign_custom_pools() {
    let mut rs = run(&[RelicId::PrismaticGem]);
    let mut opts = CardRewardGenerationOptions {
        allow_card_pool_modifications: false,
        ..Default::default()
    };
    dispatch_modify_card_reward_creation_options(&mut rs, &mut opts);
    assert!(opts.character_ids.is_empty());
    // A custom slate that is not Dingy Rug's (no Dingy Rug held) stays.
    let mut opts = CardRewardGenerationOptions {
        has_custom_card_pool: true,
        custom_card_ids: vec![CardId::Discovery],
        character_ids: Vec::new(),
        use_default_character_pool: false,
        ..Default::default()
    };
    dispatch_modify_card_reward_creation_options(&mut rs, &mut opts);
    assert_eq!(opts.custom_card_ids, vec![CardId::Discovery]);
    assert!(opts.character_ids.is_empty());
}

#[test]
fn prismatic_gem_with_dingy_rug_composes_through_the_structured_fields() {
    // Either relic order (the dispatcher walks listeners in relic-id order,
    // the fold is order-independent): characters, then colorless.
    let mut rs = run(&[RelicId::DingyRug, RelicId::PrismaticGem]);
    let mut opts = CardRewardGenerationOptions::default();
    let res = dispatch_modify_card_reward_creation_options(&mut rs, &mut opts);
    assert_eq!(res, EffectResult::Resolved);
    assert!(!opts.has_custom_card_pool);
    assert!(opts.include_colorless);
    assert_eq!(opts.character_ids.len(), 5);
    let silent = specs_for_character(CharacterId::Silent)[0]().id;
    let colorless: Vec<CardId> = COLORLESS_SPECS.iter().map(|f| f().id).collect();
    let mut ids = Vec::new();
    for _ in 0..80 {
        ids.extend(
            roll_card_reward_cards(
                &mut rs,
                CardRewardGenerationOptions::default(),
                CharacterId::Ironclad,
            )
            .into_iter()
            .map(|c| c.card_id),
        );
    }
    assert!(
        ids.iter().any(|id| colorless.contains(id)),
        "colorless offered"
    );
    assert!(
        ids.iter().any(|id| specs_for_character(CharacterId::Silent)
            .iter()
            .any(|f| f().id == *id)),
        "{silent:?}-pool card offered"
    );
}

#[test]
fn both_relics_keep_a_custom_slate_and_a_colorless_only_pool() {
    let mut rs = run(&[RelicId::DingyRug, RelicId::PrismaticGem]);
    let mut custom = CardRewardGenerationOptions {
        has_custom_card_pool: true,
        custom_card_ids: vec![CardId::Anger],
        ..Default::default()
    };
    dispatch_modify_card_reward_creation_options(&mut rs, &mut custom);
    assert_eq!(custom.custom_card_ids, vec![CardId::Anger]);
    assert!(custom.character_ids.is_empty() && !custom.include_colorless);
    let mut colorless = CardRewardGenerationOptions {
        include_colorless: true,
        use_default_character_pool: false,
        ..Default::default()
    };
    dispatch_modify_card_reward_creation_options(&mut rs, &mut colorless);
    assert!(colorless.character_ids.is_empty());
    assert!(!colorless.use_default_character_pool && !colorless.has_custom_card_pool);
}

fn deck(rs: &mut RunState, strikes: usize, defends: usize) {
    rs.master_deck.clear();
    for (id, n) in [
        (CardId::StrikeIronclad, strikes),
        (CardId::DefendIronclad, defends),
    ] {
        for _ in 0..n {
            rs.master_deck.push(card_pod_from_id(id, false).unwrap());
        }
    }
    rs.master_deck
        .push(card_pod_from_id(CardId::Anger, false).unwrap());
}

#[test]
fn pandoras_box_transforms_on_the_niche_stream_one_read_per_card() {
    let mut rs = run(&[RelicId::PandorasBox]);
    deck(&mut rs, 5, 4);
    let (niche, rewards, transformations) = (
        rs.rng.niche.counter(),
        rs.rng.rewards.counter(),
        rs.rng.transformations.counter(),
    );
    rs.last_obtained_relic = Some(RelicId::PandorasBox);
    dispatch_after_obtained(&mut rs);
    assert_eq!(
        rs.rng.niche.counter(),
        niche + 9,
        "one Niche read per basic"
    );
    assert_eq!(
        rs.rng.rewards.counter(),
        rewards,
        "the rewards stream is untouched"
    );
    assert_eq!(rs.rng.transformations.counter(), transformations);
    assert_eq!(rs.master_deck.len(), 10);
    assert_eq!(rs.master_deck[0].card_id, CardId::Anger);
}

#[test]
fn pandoras_box_replacements_span_common_uncommon_and_rare() {
    use sts2_content::cards::spec_index::rarity_for_card_id;
    use sts2_content::cards::CardRarity;
    let mut seen = std::collections::HashSet::new();
    for seed in 0..40 {
        let mut rs = run(&[RelicId::PandorasBox]);
        rs.rng.niche = sts2_core::dotnet_rng::Rng::new(seed);
        deck(&mut rs, 5, 4);
        rs.last_obtained_relic = Some(RelicId::PandorasBox);
        dispatch_after_obtained(&mut rs);
        for c in rs.master_deck.iter().skip(1) {
            seen.insert(rarity_for_card_id(c.card_id));
        }
    }
    assert!(seen.contains(&Some(CardRarity::Common)));
    assert!(seen.contains(&Some(CardRarity::Uncommon)));
    assert!(seen.contains(&Some(CardRarity::Rare)));
}

#[test]
fn negative_control_pandoras_box_leaves_a_deck_without_basics_alone() {
    let mut rs = run(&[RelicId::PandorasBox]);
    deck(&mut rs, 0, 0);
    let niche = rs.rng.niche.counter();
    rs.last_obtained_relic = Some(RelicId::PandorasBox);
    dispatch_after_obtained(&mut rs);
    assert_eq!(rs.rng.niche.counter(), niche);
    assert_eq!(rs.master_deck.len(), 1);
}

fn push(rs: &mut RunState, id: CardId) {
    rs.master_deck.push(card_pod_from_id(id, false).unwrap());
}

#[test]
fn pandoras_box_transforms_every_characters_basics_with_the_originals_pool() {
    let mut rs = run(&[RelicId::PandorasBox]);
    rs.master_deck.clear();
    for id in [
        CardId::StrikeIronclad,
        CardId::DefendIronclad,
        CardId::StrikeSilent,
        CardId::DefendSilent,
        CardId::Bash,
    ] {
        push(&mut rs, id);
    }
    let niche = rs.rng.niche.counter();
    rs.last_obtained_relic = Some(RelicId::PandorasBox);
    dispatch_after_obtained(&mut rs);
    assert_eq!(
        rs.rng.niche.counter(),
        niche + 4,
        "one read per basic, foreign too"
    );
    assert_eq!(rs.master_deck.len(), 5);
    assert_eq!(rs.master_deck[0].card_id, CardId::Bash);
    for c in &rs.master_deck[1..] {
        assert!(![
            CardId::StrikeIronclad,
            CardId::DefendIronclad,
            CardId::StrikeSilent,
            CardId::DefendSilent
        ]
        .contains(&c.card_id));
    }
}

#[test]
fn pandoras_box_leaves_an_unremovable_strike_and_reads_only_for_the_rest() {
    use sts2_content::enchantments::apply_static_enchantment;
    let mut rs = run(&[RelicId::PandorasBox]);
    rs.master_deck.clear();
    push(&mut rs, CardId::StrikeIronclad);
    push(&mut rs, CardId::DefendIronclad);
    apply_static_enchantment(
        &mut rs.master_deck[0],
        sts2_core::cards::EnchantmentId::TezcatarasEmber,
        1,
    );
    let niche = rs.rng.niche.counter();
    rs.last_obtained_relic = Some(RelicId::PandorasBox);
    dispatch_after_obtained(&mut rs);
    assert_eq!(
        rs.rng.niche.counter(),
        niche + 1,
        "only the Defend consumes a read"
    );
    assert_eq!(rs.master_deck.len(), 2);
    assert_eq!(
        rs.master_deck[0].card_id,
        CardId::StrikeIronclad,
        "Ember'd Strike kept"
    );
    assert_ne!(rs.master_deck[1].card_id, CardId::DefendIronclad);
}
