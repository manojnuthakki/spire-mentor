//! Act-1-pool events. Bead `sts2-rs-6ynt.4.2` ships the 4 act1
//! events listed in `sts2-rl-agent@01c094d:sts2_env/events/act1.py`:
//! `BrainLeech`, `RoomFullOfCheese`, `TheLegendsWereTrue`, `TeaMaster`.
//!
//! Port pin:
//! `sts2-rl-agent@01c094d:sts2_env/events/act1.py`.
//!
//! Deferred surfaces (carried as doc-pin divergences, mirroring the
//! `LostWisp.claim` "Decay curse deferred" pattern from
//! `6ynt.4.1`):
//!
//! - **`current_act_index`**: now wired (bead `sts2-rs-1hiht`, M5).
//!   C# `IsAllowed` gates these on `CurrentActIndex` (0-based): `BrainLeech` /
//!   `RoomFullOfCheese` / `TeaMaster` on `< 2` (Acts 1–2) and
//!   `TheLegendsWereTrue` on `== 0` (Act 1). Translated to the 1-based Rust
//!   `RunState::act_index` (`< 3` / `== 1`). Required for event-selection
//!   parity: the shared pool spans all acts, so the cursor walk must skip
//!   act-gated events to land on C#'s pick.
//! - **`RunState::lose_gold`**: no dedicated method; `TeaMaster`
//!   deducts gold via `state.gold.saturating_sub(amount)` (works
//!   today — the only outstanding item is formalizing a named
//!   `lose_gold` surface, same shape as `RunState.gain_gold`).
//! - **`Reward::Potion`**: LANDED (bead `sts2-rs-ib04`). The
//!   `TheLegendsWereTrue.find_exit` potion-reward leg now pushes a
//!   real `Reward::Potion(PotionReward)` (was a
//!   `Reward::Card(CardReward::empty())` placeholder).
//! - **`RunState::deck`**: read-only at the current pin. The
//!   `TheLegendsWereTrue` `len(deck) > 0` precondition is elided
//!   today (always treated as satisfied for the act1 pool).

use sts2_core::{
    CardReward, EventId, EventOptionInfo, EventOutcome, EventVarSpec, PotionReward, RelicId,
    RelicReward, Reward, RunState,
};

use super::{allowed_act_1_or_2, default_option_info_from_spec, EventOptionSpec, EventSpec};

// ── BrainLeech ───────────────────────────────────────────────────────

/// `BrainLeech.rip` HP cost. Port pin: `act1.py:42`.
pub const BRAIN_LEECH_RIP_DAMAGE: i32 = 5;

fn brain_leech_share_knowledge(state: &mut RunState) -> EventOutcome {
    // C# ShareKnowledge: `CardFactory.CreateForReward(owner, 5,
    // ForNonCombatWithDefaultOdds(owner.Character.CardPool))` — 5 cards
    // each [base-odds rarity float, NextItem] on Rewards, NO pity and NO
    // upgrade float (NoUpgradeRoll) — then a pick-1 grid
    // (`FromSimpleGridForRewards`, Cancelable=false) whose pick is
    // appended directly to the deck.
    let option_ids = crate::cards::spec_index::roll_character_reward_options_no_upgrade(state, 5);
    // C# `CreateForReward` (CardFactory.cs:72-92) fires
    // `Hook.TryModifyCardRewardOptions` + `AfterModifyingCardRewardOptions` on
    // the generated slate — `ForNonCombatWithDefaultOdds` sets `NoUpgradeRoll`
    // (so no per-card upgrade float, preserving the RNG sequence the roller
    // already matches) but NOT `NoModifyHooks`. So card-reward-modify relics
    // fire: `SilverCrucible.TryModifyCardRewardOptionsLate`
    // (SilverCrucible.cs:94-115) clones+Upgrades every upgradable offered card
    // while its 3-reward budget lasts, and `AfterModifyingCardRewardOptions`
    // (cs:117-125) increments `TimesUsed`. Route the rolled ids (materialized
    // as Card PODs) through `offer_card_reward` — the same
    // `modify_card_reward_options_late` dispatch the Gorge handler and the
    // post-combat reward use — so the offered upgrade state matches C#, then
    // carry each option's `.upgraded` flag into the select. Without this the
    // picked card was always granted unupgraded (parity `deck_upgrade_afterlife`:
    // C# `AFTERLIFE|1` vs Rust `AFTERLIFE|0` under SilverCrucible).
    let option_cards: Vec<sts2_core::Card> = option_ids
        .into_iter()
        .filter_map(|id| crate::cards::spec_index::card_pod_from_id(id, false))
        .collect();
    let option_cards = state.offer_card_reward(option_cards);
    // `Cancelable = false` (`BrainLeech.cs:68`): the grid has no skip.
    let mut select = sts2_core::runstate::PendingCardSelect::from_cards(&option_cards);
    select.can_skip = false;
    state.pending_card_selects.push(select);
    EventOutcome::finished("Pick 1 of 5 character cards to gain.")
}

fn brain_leech_rip(state: &mut RunState) -> EventOutcome {
    // C# Rip: 5 unblockable damage, then `RewardsCmd.OfferCustom` with a
    // pre-populated 3-option COLORLESS CardReward
    // (`ForNonCombatWithDefaultOdds(ColorlessCardPool)`): per card one
    // base-odds rarity float + one NextItem on Rewards, no pity, no
    // upgrade float (NoUpgradeRoll). Rolled here (OfferCustom populates
    // at option time); the drain surfaces the options verbatim.
    let lost = state.lose_hp(BRAIN_LEECH_RIP_DAMAGE);
    let options = crate::cards::spec_index::roll_colorless_reward_options_no_upgrade(state, 3);
    if state.interactive_rewards {
        state.offer_custom_rewards(
            vec![sts2_core::OfferedReward::Card {
                options: Some(options),
                upgraded: Vec::new(),
            }],
            true,
        );
        return EventOutcome::finished(format!("Took {lost} damage, gained a colorless card."));
    }
    state
        .pending_rewards
        .push(Reward::Card(CardReward::from_options(options)));
    EventOutcome::finished(format!("Took {lost} damage, gained a colorless card."))
}

const BRAIN_LEECH_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "share_knowledge",
        loc_key: "BRAIN_LEECH.pages.INITIAL.options.SHARE_KNOWLEDGE",
        default_label: "Share Knowledge",
        default_description: "Pick 1 of 5 character cards to gain.",
        default_vars: &[
            ("CardChoiceCount", EventVarSpec::Int(1)),
            ("FromCardChoiceCount", EventVarSpec::Int(5)),
        ],
        vars_fn: None,
        title_vars_fn: None,
        handler: brain_leech_share_knowledge,
    },
    EventOptionSpec {
        option_id: "rip",
        loc_key: "BRAIN_LEECH.pages.INITIAL.options.RIP",
        default_label: "Rip",
        default_description: "Take 5 damage, gain a colorless card.",
        default_vars: &[(
            "RipHpLoss",
            EventVarSpec::Int(BRAIN_LEECH_RIP_DAMAGE as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: brain_leech_rip,
    },
];

/// `BrainLeech` static spec. Port pin:
/// `sts2-rl-agent@01c094d:sts2_env/events/act1.py:23`.
pub const BRAIN_LEECH: EventSpec = EventSpec {
    id: EventId::BrainLeech,
    python_name: "BrainLeech",
    // C# `CurrentActIndex < 2` (Acts 1–2). Bead `sts2-rs-1hiht`.
    is_allowed: allowed_act_1_or_2,
    generate_options: default_option_info_from_spec,
    options: BRAIN_LEECH_OPTIONS,
};

// ── RoomFullOfCheese ─────────────────────────────────────────────────

/// `RoomFullOfCheese.search` HP cost. Port pin: `act1.py:75`.
pub const ROOM_FULL_OF_CHEESE_SEARCH_DAMAGE: i32 = 14;

/// `RoomFullOfCheese.Gorge` — # of Common cards generated to choose from.
pub const ROOM_FULL_OF_CHEESE_GORGE_OPTIONS: usize = 8;
/// `RoomFullOfCheese.Gorge` — # of those the player picks (added to the deck).
pub const ROOM_FULL_OF_CHEESE_GORGE_PICK: usize = 2;

fn room_full_of_cheese_gorge(state: &mut RunState) -> EventOutcome {
    // C# `Gorge` (RoomFullOfCheese.cs:38-44): generate 8 Common character
    // cards (`CardFactory.CreateForReward`, uniform odds, NoRarityModification),
    // then `FromSimpleGridForRewards(prefs(prompt, 2))` → add the 2 picked to
    // the Deck. Surfaces as a mandatory pick-2-of-8 `card_select` via
    // `RunState::pending_card_grants`. The earlier port queued a single empty
    // `CardReward` placeholder, which entered the `card_reward` phase instead
    // of `card_select` — desyncing whole-run parity (seeds RP3 / RP16: C#
    // `card_select` vs Rust `card_reward` at the Gorge pick). Bead
    // `sts2-rs-gorge-select`.
    let options =
        crate::cards::spec_index::roll_gorge_common_cards(state, ROOM_FULL_OF_CHEESE_GORGE_OPTIONS);
    // C# `CreateForReward` runs `Hook.TryModifyCardRewardOptions` on the
    // generated slate (GORGE sets NoUpgradeRoll | NoRarityModification, NOT
    // NoModifyHooks), so card-reward-modify relics fire — SilverCrucible
    // upgrades every option (and counts toward its 3-reward budget),
    // FresnelLens enchants, etc. Route through `offer_card_reward` (the same
    // `modify_card_reward_options_late` dispatch the post-combat reward uses)
    // so the offered cards match C# (RP16: SilverCrucible upgrades all 8).
    let options = state.offer_card_reward(options);
    let pick = ROOM_FULL_OF_CHEESE_GORGE_PICK.min(options.len());
    state
        .pending_card_grants
        .push(sts2_core::runstate::PendingCardGrant { options, pick });
    EventOutcome::finished("Pick 2 of 8 common cards to gain.")
}

fn room_full_of_cheese_search(state: &mut RunState) -> EventOutcome {
    let lost = state.lose_hp(ROOM_FULL_OF_CHEESE_SEARCH_DAMAGE);
    // C# `RoomFullOfCheese.Search` (RoomFullOfCheese.cs:52) grants the relic via
    // `RelicCmd.Obtain<ChosenCheese>` — immediate, no follow-up "Take relic"
    // step. Under `direct_event_relic_grant` (the run-parity / workbench flow
    // that matches C#) obtain it inline; otherwise queue it as a relic-reward
    // (Python-parity flow). Mirrors `act3_glory::grant_neow_relic`. The prior
    // port always offered it as a reward, so under the C#-matching flow Rust
    // never held ChosenCheese (run-parity `game_over/state/relics` cluster:
    // Necrobinder/Silent/Defect missing ChosenCheese).
    state.grant_event_relic(RelicId::ChosenCheese);
    EventOutcome::finished(format!("Took {lost} damage, gained Chosen Cheese relic."))
}

const ROOM_FULL_OF_CHEESE_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "gorge",
        loc_key: "ROOM_FULL_OF_CHEESE.pages.INITIAL.options.GORGE",
        default_label: "Gorge",
        default_description: "Pick 2 of 8 common cards.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: room_full_of_cheese_gorge,
    },
    EventOptionSpec {
        option_id: "search",
        loc_key: "ROOM_FULL_OF_CHEESE.pages.INITIAL.options.SEARCH",
        default_label: "Search",
        default_description: "Take 14 damage, gain Chosen Cheese relic.",
        default_vars: &[(
            "Damage",
            EventVarSpec::Int(ROOM_FULL_OF_CHEESE_SEARCH_DAMAGE as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: room_full_of_cheese_search,
    },
];

/// `RoomFullOfCheese` static spec. Port pin:
/// `sts2-rl-agent@01c094d:sts2_env/events/act1.py:53`.
pub const ROOM_FULL_OF_CHEESE: EventSpec = EventSpec {
    id: EventId::RoomFullOfCheese,
    python_name: "RoomFullOfCheese",
    // C# `CurrentActIndex < 2` (Acts 1–2). Bead `sts2-rs-1hiht`.
    is_allowed: allowed_act_1_or_2,
    generate_options: default_option_info_from_spec,
    options: ROOM_FULL_OF_CHEESE_OPTIONS,
};

// ── TheLegendsWereTrue ───────────────────────────────────────────────

/// `TheLegendsWereTrue.find_exit` HP cost. Port pin: `act1.py:115`.
pub const THE_LEGENDS_FIND_EXIT_DAMAGE: i32 = 8;

/// `TheLegendsWereTrue.is_allowed` HP gate. Port pin: `act1.py:96`.
pub const THE_LEGENDS_HP_THRESHOLD: i32 = 10;

fn the_legends_is_allowed(state: &RunState) -> bool {
    // C# `CurrentActIndex == 0 && deck.Count > 0 && CurrentHp >= 10` ⇒ Act 1 +
    // ≥10 HP (the deck is non-empty for the whole run). 0-based act → Rust Act 1.
    // Bead `sts2-rs-1hiht`.
    state.act_index == 1 && state.current_hp >= THE_LEGENDS_HP_THRESHOLD
}

fn the_legends_nab_map(state: &mut RunState) -> EventOutcome {
    // C# NabTheMap: `CardPileCmd.Add(CreateCard<SpoilsMap>, Deck)` — a
    // direct deck append (default index -1 = tail), no reward screen
    // and no RNG read.
    if let Some(card) =
        crate::cards::spec_index::card_pod_from_id(sts2_core::cards::CardId::SpoilsMap, false)
    {
        state.master_deck.push(card);
    }
    EventOutcome::finished("Gained Spoils Map card.")
}

fn the_legends_find_exit(state: &mut RunState) -> EventOutcome {
    // C# SlowlyFindAnExit: 8 unblockable damage, then ONE
    // `Rewards.NextItem` over the FULL character++shared potion pool —
    // the reward arrives pre-populated (no rarity float; RP0 rolled
    // TouchOfInsanity where the old empty-sentinel path re-rolled an
    // AttackPotion off [rarity float, int] and shifted the cursor).
    let lost = state.lose_hp(THE_LEGENDS_FIND_EXIT_DAMAGE);
    if let Some(kind) =
        crate::potions::all::roll_event_potion_full_pool(&mut state.rng.rewards, state.character_id)
    {
        if state.interactive_rewards {
            state.offer_custom_rewards(vec![sts2_core::OfferedReward::Potion(Some(kind))], true);
        } else {
            state
                .pending_rewards
                .push(Reward::Potion(PotionReward::with_kind(kind)));
        }
    }
    EventOutcome::finished(format!("Took {lost} damage, gained a potion reward."))
}

const THE_LEGENDS_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "nab_map",
        loc_key: "THE_LEGENDS_WERE_TRUE.pages.INITIAL.options.NAB_THE_MAP",
        default_label: "Nab the Map",
        default_description: "Gain Spoils Map card.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: the_legends_nab_map,
    },
    EventOptionSpec {
        option_id: "find_exit",
        loc_key: "THE_LEGENDS_WERE_TRUE.pages.INITIAL.options.SLOWLY_FIND_AN_EXIT",
        default_label: "Slowly Find an Exit",
        default_description: "Take 8 damage, gain a potion.",
        default_vars: &[(
            "Damage",
            EventVarSpec::Int(THE_LEGENDS_FIND_EXIT_DAMAGE as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: the_legends_find_exit,
    },
];

/// `TheLegendsWereTrue` static spec. Port pin:
/// `sts2-rl-agent@01c094d:sts2_env/events/act1.py:85`.
pub const THE_LEGENDS_WERE_TRUE: EventSpec = EventSpec {
    id: EventId::TheLegendsWereTrue,
    python_name: "TheLegendsWereTrue",
    is_allowed: the_legends_is_allowed,
    generate_options: default_option_info_from_spec,
    options: THE_LEGENDS_OPTIONS,
};

// ── TeaMaster ────────────────────────────────────────────────────────

/// `TeaMaster.bone_tea` cost. Port pin: `act1.py:162`.
pub const TEA_MASTER_BONE_COST: i32 = 50;
/// `TeaMaster.ember_tea` cost. Port pin: `act1.py:171`.
pub const TEA_MASTER_EMBER_COST: i32 = 150;

fn tea_master_is_allowed(state: &RunState) -> bool {
    // C# `CurrentActIndex < 2 && Gold >= 150` (Acts 1–2). Bead `sts2-rs-1hiht`.
    state.act_index < 3 && state.gold >= TEA_MASTER_EMBER_COST
}

fn tea_master_generate_options(state: &RunState, _spec: &EventSpec) -> Vec<EventOptionInfo> {
    // Python interpolates the cost into the label and disables the
    // option when gold < cost. Mirror that here (the option_id keys
    // stay literal so resolve-by-id still works).
    let gold = state.gold;
    vec![
        EventOptionInfo {
            option_id: "bone_tea",
            loc_key: "TEA_MASTER.pages.INITIAL.options.BONE_TEA",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: if gold >= TEA_MASTER_BONE_COST {
                "Bone Tea (50g)".to_string()
            } else {
                "Bone Tea".to_string()
            },
            description: "Gain Bone Tea relic.".to_string(),
            enabled: gold >= TEA_MASTER_BONE_COST,
        },
        EventOptionInfo {
            option_id: "ember_tea",
            loc_key: "TEA_MASTER.pages.INITIAL.options.EMBER_TEA",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: if gold >= TEA_MASTER_EMBER_COST {
                "Ember Tea (150g)".to_string()
            } else {
                "Ember Tea".to_string()
            },
            description: "Gain Ember Tea relic.".to_string(),
            enabled: gold >= TEA_MASTER_EMBER_COST,
        },
        EventOptionInfo {
            option_id: "discourtesy",
            loc_key: "TEA_MASTER.pages.INITIAL.options.TEA_OF_DISCOURTESY",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: "Tea of Discourtesy".to_string(),
            description: "Free: gain Tea of Discourtesy relic.".to_string(),
            enabled: true,
        },
    ]
}

/// Subtract `amount` from `state.gold`, saturating at 0. Inlined here
/// pending a dedicated `RunState::lose_gold` surface (see module
/// note).
fn lose_gold_saturating(state: &mut RunState, amount: i32) {
    state.gold = state.gold.saturating_sub(amount);
}

fn tea_master_bone_tea(state: &mut RunState) -> EventOutcome {
    lose_gold_saturating(state, TEA_MASTER_BONE_COST);
    state.relic_rewards.push(RelicReward::new(RelicId::BoneTea));
    EventOutcome::finished("Paid 50g, gained Bone Tea relic.")
}

fn tea_master_ember_tea(state: &mut RunState) -> EventOutcome {
    lose_gold_saturating(state, TEA_MASTER_EMBER_COST);
    state
        .relic_rewards
        .push(RelicReward::new(RelicId::EmberTea));
    EventOutcome::finished("Paid 150g, gained Ember Tea relic.")
}

fn tea_master_discourtesy(state: &mut RunState) -> EventOutcome {
    state
        .relic_rewards
        .push(RelicReward::new(RelicId::TeaOfDiscourtesy));
    EventOutcome::finished("Gained Tea of Discourtesy relic.")
}

const TEA_MASTER_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "bone_tea",
        loc_key: "TEA_MASTER.pages.INITIAL.options.BONE_TEA",
        default_label: "Bone Tea (50g)",
        default_description: "Gain Bone Tea relic.",
        default_vars: &[(
            "BoneTeaCost",
            EventVarSpec::Int(TEA_MASTER_BONE_COST as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: tea_master_bone_tea,
    },
    EventOptionSpec {
        option_id: "ember_tea",
        loc_key: "TEA_MASTER.pages.INITIAL.options.EMBER_TEA",
        default_label: "Ember Tea (150g)",
        default_description: "Gain Ember Tea relic.",
        default_vars: &[(
            "EmberTeaCost",
            EventVarSpec::Int(TEA_MASTER_EMBER_COST as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: tea_master_ember_tea,
    },
    EventOptionSpec {
        option_id: "discourtesy",
        loc_key: "TEA_MASTER.pages.INITIAL.options.TEA_OF_DISCOURTESY",
        default_label: "Tea of Discourtesy",
        default_description: "Free: gain Tea of Discourtesy relic.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: tea_master_discourtesy,
    },
];

/// `TeaMaster` static spec. Port pin:
/// `sts2-rl-agent@01c094d:sts2_env/events/act1.py:131`.
pub const TEA_MASTER: EventSpec = EventSpec {
    id: EventId::TeaMaster,
    python_name: "TeaMaster",
    is_allowed: tea_master_is_allowed,
    generate_options: tea_master_generate_options,
    options: TEA_MASTER_OPTIONS,
};

#[cfg(test)]
mod tests {
    use super::*;

    // ── BrainLeech ───────────────────────────────────────────────────

    #[test]
    fn brain_leech_spec_matches_python_pin() {
        assert_eq!(BRAIN_LEECH.python_name, "BrainLeech");
        assert_eq!(BRAIN_LEECH.id.python_name(), "BrainLeech");
        assert_eq!(BRAIN_LEECH.options.len(), 2);
        assert_eq!(BRAIN_LEECH.options[0].option_id, "share_knowledge");
        assert_eq!(BRAIN_LEECH.options[1].option_id, "rip");
    }

    #[test]
    fn brain_leech_rip_loses_5_hp_and_queues_card_reward() {
        let mut rs = RunState::new();
        let hp_before = rs.current_hp;
        let outcome = (BRAIN_LEECH.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.current_hp, hp_before - BRAIN_LEECH_RIP_DAMAGE);
        assert_eq!(rs.pending_rewards.len(), 1);
        assert!(matches!(rs.pending_rewards[0], Reward::Card(_)));
    }

    #[test]
    fn brain_leech_share_knowledge_queues_five_card_pick_no_damage() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.character_id = sts2_core::CharacterId::Ironclad;
        let hp_before = rs.current_hp;
        let outcome = (BRAIN_LEECH.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.current_hp, hp_before, "share_knowledge must not lose HP");
        // C# surfaces a pick-1-of-5 grid (FromSimpleGridForRewards), not
        // a reward screen.
        assert!(rs.pending_rewards.is_empty());
        assert_eq!(rs.pending_card_selects.len(), 1);
        assert_eq!(rs.pending_card_selects[0].options.len(), 5);
    }

    #[test]
    fn brain_leech_rip_queues_three_pre_resolved_colorless_options() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.character_id = sts2_core::CharacterId::Ironclad;
        let outcome = (BRAIN_LEECH.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.pending_rewards.len(), 1);
        match &rs.pending_rewards[0] {
            Reward::Card(reward) => {
                assert_eq!(reward.options.len(), 3, "pre-populated colorless 3");
                for id in &reward.options {
                    assert_eq!(
                        sts2_core::cards::character_of(*id),
                        "colorless",
                        "{id:?} must come from the colorless pool"
                    );
                }
            }
            other => panic!("expected Card reward, got {other:?}"),
        }
    }

    #[test]
    fn brain_leech_share_knowledge_silver_crucible_upgrades_offered_cards() {
        // Parity `deck_upgrade_afterlife` (A9P159 Necrobinder ASC9): C#
        // `BrainLeech.ShareKnowledge` rolls 5 cards via
        // `CardFactory.CreateForReward` with `ForNonCombatWithDefaultOdds`
        // (NoUpgradeRoll but NOT NoModifyHooks), so
        // `SilverCrucible.TryModifyCardRewardOptionsLate` fires and Upgrades
        // every upgradable offered card while its 3-reward budget lasts, and
        // `AfterModifyingCardRewardOptions` increments `TimesUsed`. The handler
        // must route the rolled slate through `offer_card_reward` so the
        // surfaced select carries those upgrade flags (previously the picked
        // card was always granted unupgraded → C# `AFTERLIFE|1` vs Rust |0).
        crate::cards::dispatch::ensure_installed();
        crate::relics::dispatch::ensure_installed();
        sts2_core::relic_effects::ensure_installed();
        let mut rs = RunState::new();
        rs.character_id = sts2_core::CharacterId::Necrobinder;
        rs.relics.push(RelicId::SilverCrucible);
        let outcome = (BRAIN_LEECH.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.pending_card_selects.len(), 1);
        let select = &rs.pending_card_selects[0];
        assert_eq!(select.options.len(), 5);
        assert_eq!(
            select.upgraded.len(),
            select.options.len(),
            "upgrade flags parallel options"
        );
        // SilverCrucible's per-reward budget is 3 rewards; this is its first,
        // so every offered card that is upgradable must be offered upgraded.
        // At least one of the 5 character cards is upgradable, so the slate
        // must contain at least one `upgraded` flag — the pre-fix behaviour
        // produced an all-false slate.
        assert!(
            select.upgraded.iter().any(|&u| u),
            "SilverCrucible must upgrade at least one offered card; got {:?}",
            select.upgraded
        );
        // And SilverCrucible's TimesUsed counter must have advanced (C#
        // `AfterModifyingCardRewardOptions` increments it once per slate).
        assert_ne!(
            rs.relic_counters.get(&RelicId::SilverCrucible).copied(),
            Some(0),
            "SilverCrucible TimesUsed must increment after offering the slate"
        );
    }

    #[test]
    fn brain_leech_share_knowledge_no_crucible_offers_unupgraded() {
        // Without SilverCrucible, ShareKnowledge offers the bare slate with
        // no offered upgrade (NoUpgradeRoll) — all flags false.
        crate::cards::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.character_id = sts2_core::CharacterId::Ironclad;
        let outcome = (BRAIN_LEECH.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.pending_card_selects.len(), 1);
        let select = &rs.pending_card_selects[0];
        assert_eq!(select.options.len(), 5);
        assert!(
            select.upgraded.iter().all(|&u| !u),
            "no offered upgrade without SilverCrucible; got {:?}",
            select.upgraded
        );
    }

    // ── RoomFullOfCheese ─────────────────────────────────────────────

    #[test]
    fn room_full_of_cheese_spec_matches_python_pin() {
        assert_eq!(ROOM_FULL_OF_CHEESE.python_name, "RoomFullOfCheese");
        assert_eq!(ROOM_FULL_OF_CHEESE.options.len(), 2);
        assert_eq!(ROOM_FULL_OF_CHEESE.options[0].option_id, "gorge");
        assert_eq!(ROOM_FULL_OF_CHEESE.options[1].option_id, "search");
    }

    #[test]
    fn room_full_of_cheese_search_loses_14_hp_and_queues_relic() {
        let mut rs = RunState::new();
        let hp_before = rs.current_hp;
        let outcome = (ROOM_FULL_OF_CHEESE.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.current_hp, hp_before - ROOM_FULL_OF_CHEESE_SEARCH_DAMAGE);
        assert_eq!(rs.relic_rewards.len(), 1);
        assert_eq!(rs.relic_rewards[0].relic_id, RelicId::ChosenCheese);
    }

    #[test]
    fn room_full_of_cheese_search_direct_grant_obtains_relic_immediately() {
        // Under `direct_event_relic_grant` (run-parity / workbench, matching
        // C# `RelicCmd.Obtain`) ChosenCheese is obtained inline — no relic
        // reward queued. Regression for the run-parity ChosenCheese cluster.
        let mut rs = RunState::new();
        rs.direct_event_relic_grant = true;
        let outcome = (ROOM_FULL_OF_CHEESE.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert!(
            rs.relics.contains(&RelicId::ChosenCheese),
            "ChosenCheese obtained immediately"
        );
        assert!(
            rs.relic_rewards.is_empty(),
            "no relic reward queued under direct grant"
        );
    }

    #[test]
    fn room_full_of_cheese_gorge_queues_pick2_of_8_card_grant_no_damage() {
        let mut rs = RunState::new();
        rs.character_id = sts2_core::CharacterId::Regent;
        rs.act_index = 1;
        let hp_before = rs.current_hp;
        let outcome = (ROOM_FULL_OF_CHEESE.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.current_hp, hp_before, "Gorge deals no damage");
        assert!(
            rs.pending_rewards.is_empty(),
            "Gorge no longer queues a card_reward placeholder",
        );
        assert_eq!(
            rs.pending_card_grants.len(),
            1,
            "Gorge queues exactly one multi-pick grant",
        );
        let grant = &rs.pending_card_grants[0];
        assert_eq!(grant.pick, ROOM_FULL_OF_CHEESE_GORGE_PICK, "pick 2");
        assert_eq!(
            grant.options.len(),
            ROOM_FULL_OF_CHEESE_GORGE_OPTIONS,
            "8 generated Common options",
        );
        // Uniform odds + NoUpgradeRoll: every generated option is a base
        // (non-upgraded) Common card (no SilverCrucible relic here).
        for c in &grant.options {
            assert!(!c.upgraded, "Gorge options are base (NoUpgradeRoll)");
        }
    }

    // ── TheLegendsWereTrue ───────────────────────────────────────────

    #[test]
    fn the_legends_were_true_spec_matches_python_pin() {
        assert_eq!(THE_LEGENDS_WERE_TRUE.python_name, "TheLegendsWereTrue");
        assert_eq!(THE_LEGENDS_WERE_TRUE.options.len(), 2);
        assert_eq!(THE_LEGENDS_WERE_TRUE.options[0].option_id, "nab_map");
        assert_eq!(THE_LEGENDS_WERE_TRUE.options[1].option_id, "find_exit");
    }

    #[test]
    fn the_legends_were_true_is_allowed_gates_on_hp() {
        let mut rs = RunState::new();
        rs.current_hp = THE_LEGENDS_HP_THRESHOLD;
        assert!((THE_LEGENDS_WERE_TRUE.is_allowed)(&rs));
        rs.current_hp = THE_LEGENDS_HP_THRESHOLD - 1;
        assert!(!(THE_LEGENDS_WERE_TRUE.is_allowed)(&rs));
    }

    #[test]
    fn the_legends_find_exit_loses_8_hp_and_queues_potion() {
        let mut rs = RunState::new();
        let hp_before = rs.current_hp;
        let outcome = (THE_LEGENDS_WERE_TRUE.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.current_hp, hp_before - THE_LEGENDS_FIND_EXIT_DAMAGE);
        assert_eq!(rs.pending_rewards.len(), 1);
        // sts2-rs-ib04: now a real `Reward::Potion` instead of the
        // earlier `Reward::Card(empty)` placeholder.
        assert!(matches!(rs.pending_rewards[0], Reward::Potion(_)));
    }

    #[test]
    fn the_legends_nab_map_appends_spoils_map_no_damage_no_reward() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let hp_before = rs.current_hp;
        let outcome = (THE_LEGENDS_WERE_TRUE.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.current_hp, hp_before);
        // C# adds the card directly to the deck tail — no reward screen.
        assert!(rs.pending_rewards.is_empty());
        assert_eq!(
            rs.master_deck.last().map(|c| c.card_id),
            Some(sts2_core::cards::CardId::SpoilsMap)
        );
    }

    // ── TeaMaster ────────────────────────────────────────────────────

    #[test]
    fn tea_master_spec_matches_python_pin() {
        assert_eq!(TEA_MASTER.python_name, "TeaMaster");
        assert_eq!(TEA_MASTER.options.len(), 3);
        assert_eq!(TEA_MASTER.options[0].option_id, "bone_tea");
        assert_eq!(TEA_MASTER.options[1].option_id, "ember_tea");
        assert_eq!(TEA_MASTER.options[2].option_id, "discourtesy");
    }

    #[test]
    fn tea_master_is_allowed_gates_on_150_gold() {
        let mut rs = RunState::new();
        rs.gold = TEA_MASTER_EMBER_COST;
        assert!((TEA_MASTER.is_allowed)(&rs));
        rs.gold = TEA_MASTER_EMBER_COST - 1;
        assert!(!(TEA_MASTER.is_allowed)(&rs));
    }

    #[test]
    fn tea_master_bone_tea_costs_50_and_queues_relic() {
        let mut rs = RunState::new();
        rs.gold = 200;
        let outcome = (TEA_MASTER.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.gold, 200 - TEA_MASTER_BONE_COST);
        assert_eq!(rs.relic_rewards.len(), 1);
        assert_eq!(rs.relic_rewards[0].relic_id, RelicId::BoneTea);
    }

    #[test]
    fn tea_master_ember_tea_costs_150_and_queues_relic() {
        let mut rs = RunState::new();
        rs.gold = 200;
        let outcome = (TEA_MASTER.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.gold, 200 - TEA_MASTER_EMBER_COST);
        assert_eq!(rs.relic_rewards.len(), 1);
        assert_eq!(rs.relic_rewards[0].relic_id, RelicId::EmberTea);
    }

    #[test]
    fn tea_master_discourtesy_is_free_and_queues_relic() {
        let mut rs = RunState::new();
        let gold_before = rs.gold;
        let outcome = (TEA_MASTER.options[2].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.gold, gold_before);
        assert_eq!(rs.relic_rewards.len(), 1);
        assert_eq!(rs.relic_rewards[0].relic_id, RelicId::TeaOfDiscourtesy);
    }

    #[test]
    fn tea_master_disallowed_in_act_3_even_with_gold() {
        // Kills `act_index < 3` → `<= 3`: at act 3 with ample gold the
        // original disallows (Acts 1–2 only), the `<=` mutant would allow.
        let mut rs = RunState::new();
        rs.gold = TEA_MASTER_EMBER_COST + 100;
        rs.act_index = 2;
        assert!((TEA_MASTER.is_allowed)(&rs), "act 2 with gold is allowed");
        rs.act_index = 3;
        assert!(
            !(TEA_MASTER.is_allowed)(&rs),
            "act 3 must be disallowed (boundary)",
        );
    }

    #[test]
    fn tea_master_options_disable_when_gold_too_low() {
        let mut rs = RunState::new();
        rs.gold = 60;
        let opts = (TEA_MASTER.generate_options)(&rs, &TEA_MASTER);
        assert_eq!(opts.len(), 3);
        assert!(opts[0].enabled, "bone_tea enabled at 60g");
        assert!(!opts[1].enabled, "ember_tea disabled at 60g");
        assert!(opts[2].enabled, "discourtesy always enabled");
    }
}
