//! Shared-pool events wave B (R–Z). Bead `sts2-rs-6ynt.4.5`.
//!
//! Port pin: `sts2-rl-agent@01c094d:sts2_env/events/shared.py:1323`+
//! (`Reflections` through `ZenWeaver`).
//!
//! Each event below carries an `EventSpec` matching the Python
//! `EventModel` subclass. Effect bodies port the *single-page* outcome
//! shape verbatim where the surface exists; complex deferred surfaces
//! (deck mutation, multi-page progression, RNG-rolled gold, multi-card
//! choice, enchant/transform/upgrade primitives) ship as documented
//! divergences with sentinel handler behaviour. Follow-up sub-beads:
//!
//! - `6ynt.4.deck-add` — single-card-add (curses, Metamorphosis,
//!   Greed, PoorSleep, Clumsy, Guilty, Enlightenment, MadScience,
//!   LanternKey). `Reflections` (downgrade/upgrade + deck-duplication +
//!   BadLuck) is wired against the real deck-mutation surfaces.
//! - `6ynt.4.multi-page` — multi-page progressions
//!   (`Trial`, `RoundTeaParty.pick_fight`, `TheLanternKey.keep_key`).
//!   `TinkerTime` and — since GH #487 — `TabletOfTruth` are modelled.
//! - `6ynt.4.rng` — `RunState.rng` for gold variance + dynamic relic
//!   pools (`SunkenStatue`, `SunkenTreasury`, `ThisOrThat`, `Tanx`,
//!   `Tezcatara`, `Vakuu`).
//! - `6ynt.4.choice` — `request_card_choice` surface (Purity-style
//!   multi-card-choice prompts).
//!
//! `Reward::Potion` variant is still unported (`runstate.rs::Reward`
//! doc-comment); `Wellspring.bottle` uses `Reward::Card(empty)` as a
//! placeholder until that variant lands.

use sts2_core::cards::{CardId, CardType};
use sts2_core::dotnet_rng::Rng;
use sts2_core::events::{EventCombatRequest, EventEncounter};
use sts2_core::{
    CardReward, EventEffect, EventId, EventOptionInfo, EventVarSpec, PotionReward, RelicId, Reward,
    RunState,
};

use crate::cards::colorless::helpers::{set_mad_science_variant, TinkerTimeRider};
use crate::cards::spec_index::card_pod_from_id;

use super::{
    always_allowed, deck_has_basic_card, default_option_info_from_spec, round_tea_party_allowed,
    EventOptionSpec, EventOutcome, EventSpec,
};

// ── Reflections ──────────────────────────────────────────────────────

/// `Reflections::touch` — Python downgrades 2 then upgrades 4 cards
/// (`shared.py:1345-1359`). Order matters: a card downgraded in the
/// first step becomes eligible for the upgrade in the second. On a
/// fresh deck (no upgraded cards) the downgrade is a no-op and the
/// net effect is +4 upgrades. Bead `6ynt.4.deck-add`.
fn reflections_touch(state: &mut RunState) -> EventOutcome {
    // C# `Reflections.cs:33-59` (TouchAMirror): downgrade 2 then upgrade 4
    // RANDOM cards (`Rng.NextItem`), no player pick. The upgrade leg must use
    // the always-immediate primitive, NOT the interactive-aware
    // `upgrade_random_cards` (which would queue a card_select under the
    // workbench/lockstep `interactive_card_upgrade` flag).
    let downgraded = state.downgrade_random_cards(2);
    let upgraded = state.upgrade_random_cards_stable(4);
    EventOutcome::finished(format!(
        "Touched a mirror — downgraded {downgraded}, upgraded {upgraded} card(s)."
    ))
}

/// `Reflections::shatter` — Python duplicates the entire deck and
/// adds a BadLuck curse (`shared.py:1361-1376`). Each clone gets a
/// fresh instance UID (drawn from the rewards RNG stream) so pile
/// bookkeeping can tell the copies apart, matching Python's
/// `CardInstance.clone(new_id)`.
fn reflections_shatter(state: &mut RunState) -> EventOutcome {
    let original_len = state.master_deck.len();
    let mut clones = Vec::with_capacity(original_len);
    for i in 0..original_len {
        let seed = state.rng.rewards.next_int_exclusive(0, i32::MAX) as u64;
        clones.push(state.master_deck[i].clone_with_seed(seed));
    }
    state.master_deck.extend(clones);
    let gained_curse =
        if let Some(curse) = crate::cards::spec_index::card_pod_from_id(CardId::BadLuck, false) {
            state.master_deck.push(curse);
            true
        } else {
            false
        };
    EventOutcome::finished(format!(
        "Shattered a mirror — duplicated {original_len} card(s){}.",
        if gained_curse {
            ", gained a Bad Luck curse"
        } else {
            ""
        }
    ))
}

const REFLECTIONS_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "touch",
        loc_key: "REFLECTIONS.pages.INITIAL.options.TOUCH_A_MIRROR",
        default_label: "Touch a Mirror",
        default_description: "Downgrade 2, upgrade 4 cards.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: reflections_touch,
    },
    EventOptionSpec {
        option_id: "shatter",
        loc_key: "REFLECTIONS.pages.INITIAL.options.SHATTER",
        default_label: "Shatter",
        default_description: "Duplicate entire deck, gain Bad Luck curse.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: reflections_shatter,
    },
];

pub const REFLECTIONS: EventSpec = EventSpec {
    id: EventId::Reflections,
    python_name: "Reflections",
    is_allowed: always_allowed,
    generate_options: default_option_info_from_spec,
    options: REFLECTIONS_OPTIONS,
};

// ── RoundTeaParty ────────────────────────────────────────────────────

/// `RoundTeaParty.pick_fight` damage (`shared.py:1417`, lose_hp(11)).
pub const ROUND_TEA_PARTY_FIGHT_DAMAGE: i32 = 11;

fn round_tea_party_enjoy_tea(state: &mut RunState) -> EventOutcome {
    let missing = state.max_hp - state.current_hp;
    state.heal(missing);
    state.offer_relic_rewards(RelicId::RoyalPoison, 1);
    EventOutcome::finished("Gained Royal Poison relic, healed to full.")
}

/// `PickFight`: C# `RoundTeaParty.cs:54-58` calls `SetEventState` with a single
/// `ContinueFight` follow-up option and does NOTHING else — no damage, no relic.
/// The self-damage + relic land in [`round_tea_party_continue_fight`]. Returning
/// a not-finished outcome keeps the run-loop in `Phase::Event` (multi-page
/// integration, bead `sts2-rs-m3n0g.3`).
fn round_tea_party_pick_fight(state: &mut RunState) -> EventOutcome {
    state.event_state.page = 1;
    EventOutcome {
        finished: false,
        description: "You ready yourself for a fight.".to_string(),
        effects: Vec::new(),
    }
}

/// `ContinueFight`: C# `RoundTeaParty.cs:67-70` — 11 unblockable self-damage,
/// then `RelicCmd.Obtain(RelicFactory.PullNextRelicFromFront(player))` (a DIRECT
/// obtain of the next front relic at a `rewards`-rolled rarity), then finish.
///
/// **Approximation:** C# `PullFromFront` uses filter `_ => true` (no owned-skip),
/// whereas [`RunState::pop_relic_from_grab_bag`] skips already-owned relics. This
/// is page-1 only — the effect-parity harness resolves a single option per entry,
/// so `pick_fight` (the page-0 option it tests) never reaches here, and the exact
/// pulled relic is not cross-engine-validated at this pin.
fn round_tea_party_continue_fight(state: &mut RunState) -> EventOutcome {
    let lost = state.lose_hp(ROUND_TEA_PARTY_FIGHT_DAMAGE);
    let rarity = state.roll_reward_relic_rarity();
    if let Some(id) = state.pop_relic_from_grab_bag(rarity) {
        state.grant_event_relic(id);
    }
    EventOutcome::finished(format!("Took {lost} damage, claimed a relic."))
}

/// Number of page-0 options (`EnjoyTea`, `PickFight`). The static option list
/// also carries the page-1 `ContinueFight` handler, which must NOT be presented
/// at page 0 — otherwise the entry option count diverges from C#'s 2.
const ROUND_TEA_PARTY_PAGE0_OPTIONS: usize = 2;

/// Multi-page presenter: page 0 = the two initial options; page 1 = the single
/// `ContinueFight` follow-up opened by [`round_tea_party_pick_fight`].
fn round_tea_party_generate_options(state: &RunState, spec: &EventSpec) -> Vec<EventOptionInfo> {
    if state.event_state.page == 1 {
        return vec![EventOptionInfo {
            option_id: "continue_fight",
            loc_key: "ROUND_TEA_PARTY.pages.PICK_FIGHT.options.CONTINUE_FIGHT",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: "Continue the Fight".to_string(),
            description: "Take 11 damage, claim a relic.".to_string(),
            enabled: true,
        }];
    }
    spec.options[..ROUND_TEA_PARTY_PAGE0_OPTIONS]
        .iter()
        .map(|opt| EventOptionInfo {
            option_id: opt.option_id,
            loc_key: opt.loc_key,
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: opt.default_label.to_string(),
            description: opt.default_description.to_string(),
            enabled: true,
        })
        .collect()
}

const ROUND_TEA_PARTY_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "enjoy_tea",
        loc_key: "ROUND_TEA_PARTY.pages.INITIAL.options.ENJOY_TEA",
        default_label: "Enjoy Tea",
        default_description: "Gain Royal Poison relic, heal to full HP.",
        default_vars: &[("Relic", EventVarSpec::Text("Royal Poison"))],
        vars_fn: None,
        title_vars_fn: None,
        handler: round_tea_party_enjoy_tea,
    },
    EventOptionSpec {
        option_id: "pick_fight",
        loc_key: "ROUND_TEA_PARTY.pages.INITIAL.options.PICK_FIGHT",
        default_label: "Pick a Fight",
        default_description: "Take 11 damage, gain a relic.",
        default_vars: &[(
            "Damage",
            EventVarSpec::Int(ROUND_TEA_PARTY_FIGHT_DAMAGE as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: round_tea_party_pick_fight,
    },
    // ── page 1: the ContinueFight follow-up ──
    EventOptionSpec {
        option_id: "continue_fight",
        loc_key: "ROUND_TEA_PARTY.pages.PICK_FIGHT.options.CONTINUE_FIGHT",
        default_label: "Continue the Fight",
        default_description: "Take 11 damage, claim a relic.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: round_tea_party_continue_fight,
    },
];

pub const ROUND_TEA_PARTY: EventSpec = EventSpec {
    id: EventId::RoundTeaParty,
    python_name: "RoundTeaParty",
    is_allowed: round_tea_party_allowed,
    generate_options: round_tea_party_generate_options,
    options: ROUND_TEA_PARTY_OPTIONS,
};

// ── SapphireSeed ─────────────────────────────────────────────────────

/// `SapphireSeed.eat` heal (`shared.py:1446`, heal(9)).
pub const SAPPHIRE_SEED_EAT_HEAL: i32 = 9;

fn sapphire_seed_eat(state: &mut RunState) -> EventOutcome {
    // C# Eat: heal 9, then `CardSelectCmd.FromDeckForUpgrade` pick-1 →
    // `CardCmd.Upgrade`. Route through `upgrade_random_cards` (interactive mode
    // → `pending_deck_upgrades` card_select, matching C#) rather than
    // `offer_upgrade_cards_reward`, whose `Reward::UpgradeCards` the run loop
    // never surfaces — see `aroma_maintain_control`. Bead
    // `sts2-rs-upgrade-event-select`.
    let healed = state.heal(SAPPHIRE_SEED_EAT_HEAL);
    let _ = state.upgrade_random_cards(1);
    EventOutcome::finished(format!("Healed {healed} HP; pick a card to upgrade."))
}

/// Base `EnchantmentModel.CanEnchant` (Sown has no override): not
/// Status/Curse, unenchanted, not Unplayable — X-cost stays eligible.
fn sown_can_enchant(c: &sts2_core::cards::Card) -> bool {
    use sts2_core::cards::CardType;
    !matches!(c.card_type, CardType::Status | CardType::Curse)
        && c.enchantments.is_empty()
        && (c.cost >= 0 || c.is_x_cost())
}

fn sapphire_seed_plant(state: &mut RunState) -> EventOutcome {
    // C# Plant: `FromDeckForEnchantment(Sown, 1)` pick-1 over the
    // `sown.CanEnchant` pool → `CardCmd.Enchant<Sown>(card, 1)`. The
    // grid auto-takes at count <= MinSelect(=1), so a single-eligible
    // deck enchants without a card_select step.
    use sts2_core::cards::EnchantmentId;
    let eligible = state
        .master_deck
        .iter()
        .filter(|c| sown_can_enchant(c))
        .count();
    if state.interactive_card_transform && eligible > 1 {
        state
            .pending_deck_enchants
            .push(sts2_core::PendingDeckEnchant {
                enchantment: EnchantmentId::Sown,
                amount: 1,
                type_restriction: None,
                secondary_type: None,
            });
        EventOutcome::finished("Pick a card to enchant with Sown.")
    } else {
        if let Some(card) = state.master_deck.iter_mut().find(|c| sown_can_enchant(c)) {
            card.enchantments.push((EnchantmentId::Sown, 1));
        }
        EventOutcome::finished("Enchanted a card with Sown.")
    }
}

const SAPPHIRE_SEED_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "eat",
        loc_key: "SAPPHIRE_SEED.pages.INITIAL.options.EAT",
        default_label: "Eat",
        default_description: "Heal 9 HP, upgrade 1 card.",
        default_vars: &[("Heal", EventVarSpec::Int(SAPPHIRE_SEED_EAT_HEAL as i64))],
        vars_fn: None,
        title_vars_fn: None,
        handler: sapphire_seed_eat,
    },
    EventOptionSpec {
        option_id: "plant",
        loc_key: "SAPPHIRE_SEED.pages.INITIAL.options.PLANT",
        default_label: "Plant the Seed",
        default_description: "Enchant a card with Sown.",
        default_vars: &[("Enchantment", EventVarSpec::Text("Sown"))],
        vars_fn: None,
        title_vars_fn: None,
        handler: sapphire_seed_plant,
    },
];

pub const SAPPHIRE_SEED: EventSpec = EventSpec {
    id: EventId::SapphireSeed,
    python_name: "SapphireSeed",
    is_allowed: always_allowed,
    generate_options: default_option_info_from_spec,
    options: SAPPHIRE_SEED_OPTIONS,
};

// ── SelfHelpBook ─────────────────────────────────────────────────────

/// C# `SelfHelpBook._sharpAmount` / `_nimbleAmount` / `_swiftAmount`
/// (all 2).
pub const SELF_HELP_BOOK_ENCHANT_AMOUNT: i32 = 2;

/// Deck-card predicate for one SelfHelpBook option: C# `DeckFilter` =
/// `card.Type == type && enchantment.CanEnchant(card)`. The base
/// `EnchantmentModel.CanEnchant` rejects Status/Curse/Quest (implied by
/// the type match here), existing enchantments, and Unplayable — but NOT
/// X-cost (that exclusion is `Slither.CanEnchant`-specific), so
/// `cost == -1` attacks (Whirlwind) stay eligible. `Nimble.CanEnchant`
/// additionally requires `card.GainsBlock` (`Nimble.cs:7-14`, proxied by
/// `base_block > 0`) — the real DLL offers only the deck's block-gaining
/// Skills for the Nimble pick (2026-07-18 shadow-replay census).
fn self_help_card_eligible(
    c: &sts2_core::cards::Card,
    card_type: sts2_core::cards::CardType,
    enchantment: sts2_core::cards::EnchantmentId,
) -> bool {
    c.card_type == card_type
        && c.enchantments.is_empty()
        && (c.cost >= 0 || c.is_x_cost())
        && (enchantment != sts2_core::cards::EnchantmentId::Nimble || c.base_block > 0)
}

/// C# `PlayerHasCardsAvailable<T>(owner, type)`.
fn self_help_has_eligible(
    state: &RunState,
    card_type: sts2_core::cards::CardType,
    enchantment: sts2_core::cards::EnchantmentId,
) -> bool {
    state
        .master_deck
        .iter()
        .any(|c| self_help_card_eligible(c, card_type, enchantment))
}

/// C# `SelectAndEnchant<T>(2, typeRestriction, …)`:
/// `CardSelectCmd.FromDeckForEnchantment` pick-1 over the type-filtered
/// deck, then `CardCmd.Enchant<T>(card, 2)`. `CardSelectorPrefs(prompt,
/// 1)` sets MinSelect == MaxSelect == 1, and `FromSimpleGrid` auto-takes
/// the whole pool at `count <= MinSelect` — so a single-eligible deck
/// enchants WITHOUT surfacing a card_select; only 2+ candidates open the
/// interactive pick.
fn self_help_select_and_enchant(
    state: &mut RunState,
    enchantment: sts2_core::cards::EnchantmentId,
    card_type: sts2_core::cards::CardType,
    finished: &str,
) -> EventOutcome {
    let eligible = state
        .master_deck
        .iter()
        .filter(|c| self_help_card_eligible(c, card_type, enchantment))
        .count();
    if state.interactive_card_transform && eligible > 1 {
        state
            .pending_deck_enchants
            .push(sts2_core::PendingDeckEnchant {
                enchantment,
                amount: SELF_HELP_BOOK_ENCHANT_AMOUNT,
                type_restriction: Some(card_type),
                secondary_type: None,
            });
    } else if let Some(card) = state
        .master_deck
        .iter_mut()
        .find(|c| self_help_card_eligible(c, card_type, enchantment))
    {
        // Auto mode (parity-fuzz / headless) takes the first eligible
        // card; the 1-eligible interactive case is C#'s grid auto-take.
        card.enchantments
            .push((enchantment, SELF_HELP_BOOK_ENCHANT_AMOUNT));
    }
    EventOutcome::finished(finished.to_owned())
}

fn self_help_book_read_back(state: &mut RunState) -> EventOutcome {
    self_help_select_and_enchant(
        state,
        sts2_core::cards::EnchantmentId::Sharp,
        sts2_core::cards::CardType::Attack,
        "Enchanted an Attack with Sharp +2.",
    )
}

fn self_help_book_read_passage(state: &mut RunState) -> EventOutcome {
    self_help_select_and_enchant(
        state,
        sts2_core::cards::EnchantmentId::Nimble,
        sts2_core::cards::CardType::Skill,
        "Enchanted a Skill with Nimble +2.",
    )
}

fn self_help_book_read_entire(state: &mut RunState) -> EventOutcome {
    self_help_select_and_enchant(
        state,
        sts2_core::cards::EnchantmentId::Swift,
        sts2_core::cards::CardType::Power,
        "Enchanted a Power with Swift +2.",
    )
}

fn self_help_book_skip(_state: &mut RunState) -> EventOutcome {
    EventOutcome::finished("Nothing worth reading — closed the book.")
}

/// C# `GenerateInitialOptions`: with at least one enchantable type the
/// page always shows the three READ options in fixed order, locking
/// (null handler ⇒ `enabled: false`) the ones with no eligible card;
/// with none it collapses to the single NO_OPTIONS skip.
fn self_help_book_options(state: &RunState, _spec: &EventSpec) -> Vec<EventOptionInfo> {
    use sts2_core::cards::{CardType, EnchantmentId};
    let attack = self_help_has_eligible(state, CardType::Attack, EnchantmentId::Sharp);
    let skill = self_help_has_eligible(state, CardType::Skill, EnchantmentId::Nimble);
    let power = self_help_has_eligible(state, CardType::Power, EnchantmentId::Swift);
    if !(attack || skill || power) {
        return vec![EventOptionInfo {
            option_id: "no_options",
            loc_key: "SELF_HELP_BOOK.pages.INITIAL.options.NO_OPTIONS",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: "Close the Book".to_string(),
            description: "Nothing to enchant.".to_string(),
            enabled: true,
        }];
    }
    vec![
        EventOptionInfo {
            option_id: "read_back",
            loc_key: "SELF_HELP_BOOK.pages.INITIAL.options.READ_THE_BACK",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: "Read the Back".to_string(),
            description: "Enchant an Attack with Sharp +2.".to_string(),
            enabled: attack,
        },
        EventOptionInfo {
            option_id: "read_passage",
            loc_key: "SELF_HELP_BOOK.pages.INITIAL.options.READ_PASSAGE",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: "Read a Passage".to_string(),
            description: "Enchant a Skill with Nimble +2.".to_string(),
            enabled: skill,
        },
        EventOptionInfo {
            option_id: "read_entire",
            loc_key: "SELF_HELP_BOOK.pages.INITIAL.options.READ_ENTIRE_BOOK",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: "Read Entire Book".to_string(),
            description: "Enchant a Power with Swift +2.".to_string(),
            enabled: power,
        },
    ]
}

const SELF_HELP_BOOK_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "read_back",
        loc_key: "SELF_HELP_BOOK.pages.INITIAL.options.READ_THE_BACK",
        default_label: "Read the Back",
        default_description: "Enchant an Attack with Sharp +2.",
        default_vars: &[
            ("Enchantment1", EventVarSpec::Text("Sharp")),
            (
                "Enchantment1Amount",
                EventVarSpec::Int(SELF_HELP_BOOK_ENCHANT_AMOUNT as i64),
            ),
        ],
        vars_fn: None,
        title_vars_fn: None,
        handler: self_help_book_read_back,
    },
    EventOptionSpec {
        option_id: "read_passage",
        loc_key: "SELF_HELP_BOOK.pages.INITIAL.options.READ_PASSAGE",
        default_label: "Read a Passage",
        default_description: "Enchant a Skill with Nimble +2.",
        default_vars: &[
            ("Enchantment2", EventVarSpec::Text("Nimble")),
            (
                "Enchantment2Amount",
                EventVarSpec::Int(SELF_HELP_BOOK_ENCHANT_AMOUNT as i64),
            ),
        ],
        vars_fn: None,
        title_vars_fn: None,
        handler: self_help_book_read_passage,
    },
    EventOptionSpec {
        option_id: "read_entire",
        loc_key: "SELF_HELP_BOOK.pages.INITIAL.options.READ_ENTIRE_BOOK",
        default_label: "Read Entire Book",
        default_description: "Enchant a Power with Swift +2.",
        default_vars: &[
            ("Enchantment3", EventVarSpec::Text("Swift")),
            (
                "Enchantment3Amount",
                EventVarSpec::Int(SELF_HELP_BOOK_ENCHANT_AMOUNT as i64),
            ),
        ],
        vars_fn: None,
        title_vars_fn: None,
        handler: self_help_book_read_entire,
    },
    EventOptionSpec {
        option_id: "no_options",
        loc_key: "SELF_HELP_BOOK.pages.INITIAL.options.NO_OPTIONS",
        default_label: "Close the Book",
        default_description: "Nothing to enchant.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: self_help_book_skip,
    },
];

pub const SELF_HELP_BOOK: EventSpec = EventSpec {
    id: EventId::SelfHelpBook,
    python_name: "SelfHelpBook",
    is_allowed: always_allowed,
    generate_options: self_help_book_options,
    options: SELF_HELP_BOOK_OPTIONS,
};

// ── SpiritGrafter ────────────────────────────────────────────────────

/// `SpiritGrafter.let_it_in` heal (`shared.py:1567`, heal(25)).
pub const SPIRIT_GRAFTER_HEAL: i32 = 25;
/// `SpiritGrafter.Rejection` damage. C# `RejectionHpLoss` BaseValue = 10
/// unblockable (`SpiritGrafter.cs:24,52`); the Python pin's `lose_hp(9)`
/// (`shared.py:1591`) drifted, and C# is the source of truth — so 10.
pub const SPIRIT_GRAFTER_DAMAGE: i32 = 10;

fn spirit_grafter_let_it_in(state: &mut RunState) -> EventOutcome {
    // C# `SpiritGrafter.LetItIn`: heal 25, then `CreateCard<Metamorphosis>` →
    // add to deck. Unconditional (no player choice, no RNG).
    let healed = state.heal(SPIRIT_GRAFTER_HEAL);
    if let Some(card) = card_pod_from_id(CardId::Metamorphosis, false) {
        state.master_deck.push(card);
    }
    EventOutcome::finished(format!("Healed {healed} HP, gained Metamorphosis."))
}

fn spirit_grafter_rejection(state: &mut RunState) -> EventOutcome {
    // C# `SpiritGrafter.Rejection`: `FromDeckForUpgrade` pick-1 → `Upgrade`,
    // THEN 10 unblockable damage. `upgrade_random_cards(1)` queues the pick via
    // `pending_deck_upgrades` in interactive mode (matching C#'s player choice,
    // no RNG) and falls back to an auto rewards-stream upgrade headless — same
    // shape as `sapphire_seed_eat`.
    //
    // C# applies the 10 damage AFTER the upgrade card-select completes; we apply
    // it here so the FINAL state is correct, but the `pending_deck_upgrades`
    // counter carries no post-effect (only `PendingDeckTransform` has
    // `post_hp_loss`), so an intermediate event-effect-parity snapshot taken
    // mid-select sees the damage one step early. Final-state-faithful; the
    // post-select deferral is a separate run-loop surface.
    let _ = state.upgrade_random_cards(1);
    let lost = state.lose_hp(SPIRIT_GRAFTER_DAMAGE);
    EventOutcome::finished(format!("Upgraded a card, took {lost} damage."))
}

const SPIRIT_GRAFTER_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "let_it_in",
        loc_key: "SPIRIT_GRAFTER.pages.INITIAL.options.LET_IT_IN",
        default_label: "Let It In",
        default_description: "Heal 25 HP, gain Metamorphosis card.",
        default_vars: &[(
            "LetItInHealAmount",
            EventVarSpec::Int(SPIRIT_GRAFTER_HEAL as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: spirit_grafter_let_it_in,
    },
    EventOptionSpec {
        option_id: "rejection",
        loc_key: "SPIRIT_GRAFTER.pages.INITIAL.options.REJECTION",
        default_label: "Rejection",
        default_description: "Remove 1 card, take 9 damage.",
        default_vars: &[(
            "RejectionHpLoss",
            EventVarSpec::Int(SPIRIT_GRAFTER_DAMAGE as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: spirit_grafter_rejection,
    },
];

pub const SPIRIT_GRAFTER: EventSpec = EventSpec {
    id: EventId::SpiritGrafter,
    python_name: "SpiritGrafter",
    is_allowed: always_allowed,
    generate_options: default_option_info_from_spec,
    options: SPIRIT_GRAFTER_OPTIONS,
};

// ── SunkenStatue ─────────────────────────────────────────────────────

/// `SunkenStatue.dive` base gold (C# `GoldVar(111)`). `CalculateVars`
/// adds `base.Rng.NextInt(-10, 11)` (max-exclusive → `[-10, 10]`), so the
/// granted gold is `111 ± 10`. The prior port pinned the midpoint (111),
/// granting the base regardless of the roll and desyncing the gold total
/// vs C# (run-parity `map/state/gold`, e.g. Defect RP10 cs=236 rs=229 —
/// C# rolled +7). Sub-bead `6ynt.4.rng`.
pub const SUNKEN_STATUE_DIVE_GOLD: i32 = 111;
/// C# `SunkenStatue._goldVariance` (`±NextInt(-10, 11)` → `[-10, 10]`).
pub const SUNKEN_STATUE_GOLD_VARIANCE: i32 = 10;
/// `SunkenStatue.dive` damage (`shared.py:1638`, lose_hp(7)).
pub const SUNKEN_STATUE_DIVE_DAMAGE: i32 = 7;
/// `Id.Entry` for the event's own RNG stream. NON-shared event
/// (`EventModel.IsShared` default false), so the C# event-Rng seed is
/// `RunState.Rng.Seed + NetId + hash(Entry)`; mirror that as
/// `Rng::with_name(seed + NetId, Entry)`.
pub const SUNKEN_STATUE_STREAM: &str = "SUNKEN_STATUE";

/// The `CalculateVars` gold roll: `111 + NextInt(-10, 11)` = read #1 of
/// the event's own (private) stream. Re-derived per call — the stream is
/// private to this event, so re-deriving preserves in-stream order
/// (mirrors `whispering_rolled_gold`).
fn sunken_statue_rolled_gold(state: &RunState) -> i32 {
    // v0.107.1 single-player event seed: additive player term is 0
    // (`EventModel.cs:238` GetPlayerSlotIndex == 0; v0.103.2 used NetId == 1).
    let mut rng = sts2_core::dotnet_rng::Rng::with_name(state.rng.seed, SUNKEN_STATUE_STREAM);
    SUNKEN_STATUE_DIVE_GOLD
        + rng.next_int(-SUNKEN_STATUE_GOLD_VARIANCE, SUNKEN_STATUE_GOLD_VARIANCE)
}

fn sunken_statue_grab_sword(state: &mut RunState) -> EventOutcome {
    state.grant_event_relic(RelicId::SwordOfStone);
    EventOutcome::finished("Gained Sword of Stone relic.")
}

fn sunken_statue_dive(state: &mut RunState) -> EventOutcome {
    let gold = sunken_statue_rolled_gold(state);
    state.gain_gold(gold);
    let lost = state.lose_hp(SUNKEN_STATUE_DIVE_DAMAGE);
    EventOutcome::finished(format!("Gained {gold} gold, took {lost} damage."))
}

const SUNKEN_STATUE_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "grab_sword",
        loc_key: "SUNKEN_STATUE.pages.INITIAL.options.GRAB_SWORD",
        default_label: "Grab the Sword",
        default_description: "Gain Sword of Stone relic.",
        default_vars: &[("Relic", EventVarSpec::Text("Sword of Stone"))],
        vars_fn: None,
        title_vars_fn: None,
        handler: sunken_statue_grab_sword,
    },
    EventOptionSpec {
        option_id: "dive",
        loc_key: "SUNKEN_STATUE.pages.INITIAL.options.DIVE_INTO_WATER",
        default_label: "Dive Into Water",
        default_description: "Gain ~111 gold, take 7 damage.",
        default_vars: &[(
            "HpLoss",
            EventVarSpec::Int(SUNKEN_STATUE_DIVE_DAMAGE as i64),
        )],
        vars_fn: Some(|s| vec![("Gold", sunken_statue_rolled_gold(s).into())]),
        title_vars_fn: None,
        handler: sunken_statue_dive,
    },
];

pub const SUNKEN_STATUE: EventSpec = EventSpec {
    id: EventId::SunkenStatue,
    python_name: "SunkenStatue",
    is_allowed: always_allowed,
    generate_options: default_option_info_from_spec,
    options: SUNKEN_STATUE_OPTIONS,
};

// ── SunkenTreasury ───────────────────────────────────────────────────

/// `SunkenTreasury.first_chest` gold (`shared.py:1658`, 60 ±8).
pub const SUNKEN_TREASURY_SMALL_GOLD: i32 = 60;
/// `SunkenTreasury.second_chest` gold (`shared.py:1659`, 333 ±30).
pub const SUNKEN_TREASURY_LARGE_GOLD: i32 = 333;

fn sunken_treasury_first_chest(state: &mut RunState) -> EventOutcome {
    state.gain_gold(SUNKEN_TREASURY_SMALL_GOLD);
    EventOutcome::finished(format!("Gained {SUNKEN_TREASURY_SMALL_GOLD} gold."))
}

fn sunken_treasury_second_chest(state: &mut RunState) -> EventOutcome {
    // C# `SunkenTreasury.SecondChest`: gain gold, then
    // `AddCurseToDeck<Greed>` — an unconditional deck append. (The ±30 gold
    // variance off the event Rng is a separate roll still pinned flat to the
    // base — `6ynt.4.rng`.)
    state.gain_gold(SUNKEN_TREASURY_LARGE_GOLD);
    if let Some(curse) = card_pod_from_id(CardId::Greed, false) {
        state.master_deck.push(curse);
    }
    EventOutcome::finished(format!(
        "Gained {SUNKEN_TREASURY_LARGE_GOLD} gold, gained a Greed curse."
    ))
}

const SUNKEN_TREASURY_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "first_chest",
        loc_key: "SUNKEN_TREASURY.pages.INITIAL.options.FIRST_CHEST",
        default_label: "Small Chest",
        default_description: "Gain ~60 gold.",
        default_vars: &[(
            "SmallChestGold",
            EventVarSpec::Int(SUNKEN_TREASURY_SMALL_GOLD as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: sunken_treasury_first_chest,
    },
    EventOptionSpec {
        option_id: "second_chest",
        loc_key: "SUNKEN_TREASURY.pages.INITIAL.options.SECOND_CHEST",
        default_label: "Large Chest",
        default_description: "Gain ~333 gold + Greed curse.",
        default_vars: &[(
            "LargeChestGold",
            EventVarSpec::Int(SUNKEN_TREASURY_LARGE_GOLD as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: sunken_treasury_second_chest,
    },
];

pub const SUNKEN_TREASURY: EventSpec = EventSpec {
    id: EventId::SunkenTreasury,
    python_name: "SunkenTreasury",
    is_allowed: always_allowed,
    generate_options: default_option_info_from_spec,
    options: SUNKEN_TREASURY_OPTIONS,
};

// ── TabletOfTruth ────────────────────────────────────────────────────

/// `TabletOfTruth.smash` heal (`shared.py:1715`, heal(20)).
pub const TABLET_OF_TRUTH_SMASH_HEAL: i32 = 20;
/// First-step decipher cost (`shared.py:1699`, _DECIPHER_COSTS[0] = 3).
pub const TABLET_OF_TRUTH_FIRST_DECIPHER_COST: i32 = 3;
/// `Id.Entry` for the event's own RNG stream. NON-shared event
/// (`EventModel.IsShared` default false), so the C# event-Rng seed is
/// `RunState.Rng.Seed + NetId + hash(Entry)`; mirror that as
/// `Rng::with_name(seed + NetId, Entry)` (same pattern as Sunken Statue).
pub const TABLET_OF_TRUTH_STREAM: &str = "TABLET_OF_TRUTH";

fn tablet_of_truth_smash(state: &mut RunState) -> EventOutcome {
    let healed = state.heal(TABLET_OF_TRUTH_SMASH_HEAL);
    EventOutcome::finished(format!("Smashed tablet, healed {healed} HP."))
}

/// Number of Deciphers that ends the event (C# `if (DecipherCount == 5)`).
pub const TABLET_OF_TRUTH_MAX_DECIPHERS: u8 = 5;

/// Max-HP price of the Decipher on pages 0..=3, i.e. the INITIAL page's
/// `DecipherMaxHpLoss` DynamicVar (3) followed by C# `GetDecipherCost()`
/// for `DecipherCount` 1/2/3. The FIFTH decipher (page 4) is priced
/// `MaxHp - 1` and lives in [`tablet_of_truth_decipher_cost`].
const TABLET_OF_TRUTH_DECIPHER_COSTS: [i32; 4] = [TABLET_OF_TRUTH_FIRST_DECIPHER_COST, 6, 12, 24];

/// Loc key of the DECIPHER option on each page. Page 0 is the INITIAL
/// page's `DECIPHER_1`; pages 1..=4 are the escalation pages C#
/// `SetEventState` names `TABLET_OF_TRUTH.pages.DECIPHER_{DecipherCount}`
/// AFTER the increment, so page N carries key `DECIPHER_N`.
const TABLET_OF_TRUTH_DECIPHER_KEYS: [&str; TABLET_OF_TRUTH_MAX_DECIPHERS as usize] = [
    "TABLET_OF_TRUTH.pages.INITIAL.options.DECIPHER_1",
    "TABLET_OF_TRUTH.pages.DECIPHER_1.options.DECIPHER",
    "TABLET_OF_TRUTH.pages.DECIPHER_2.options.DECIPHER",
    "TABLET_OF_TRUTH.pages.DECIPHER_3.options.DECIPHER",
    "TABLET_OF_TRUTH.pages.DECIPHER_4.options.DECIPHER",
];

/// What the NEXT Decipher costs in Max HP, at the page the event is on.
///
/// `event_state.page` is C# `TabletOfTruth.DecipherCount` — 0 on the
/// initial page, bumped once per Decipher. C# re-binds the
/// `DecipherMaxHpLoss` DynamicVar from `GetDecipherCost()` when it builds
/// each escalation page, and the handler spends whatever that var holds,
/// so the price is a pure function of the page (plus current Max HP on the
/// last one). GH #487: modelling only the first page priced EVERY page at
/// 3, which is how the run policy came to rate "lose all but 1 Max HP" at
/// p=94%.
#[must_use]
pub fn tablet_of_truth_decipher_cost(state: &RunState) -> i32 {
    match TABLET_OF_TRUTH_DECIPHER_COSTS.get(state.event_state.page as usize) {
        Some(&fixed) => fixed,
        // C# `GetDecipherCost()` case 4: `owner.Creature.MaxHp - 1`.
        None => state.max_hp - 1,
    }
}

fn tablet_of_truth_decipher(state: &mut RunState) -> EventOutcome {
    // C# `TabletOfTruth.Decipher`: `LoseMaxHpAndUpgrade(DecipherMaxHpLoss)`,
    // then `DecipherCount++`, then either finish (count == 5) or present the
    // next page with the re-priced Decipher and a Give Up.
    let count = state.event_state.page;
    let cost = tablet_of_truth_decipher_cost(state);
    state.event_state.page = count.saturating_add(1);

    // `LoseMaxHpAndUpgrade`'s guard is `if (!(hp < MaxHp))` — a price at or
    // above Max HP does NOT lose `cost`: it drops you to 1 Max HP and then
    // calls `CreatureCmd.Kill`. Reachable on the fixed pages when Max HP has
    // already fallen to the price (the option is the one C# flags
    // `ThatWillKillPlayerIf`), never on the last page, whose `MaxHp - 1` is
    // below Max HP by construction.
    if cost >= state.max_hp {
        let to_lose = state.max_hp - 1;
        state.lose_max_hp(to_lose);
        // A direct kill, not damage: `CreatureCmd.Kill` bypasses the HP-loss
        // pipeline, so this must NOT route through `lose_hp` (TungstenRod
        // would shave a point off and leave the player standing at 1).
        state.current_hp = 0;
        return EventOutcome::finished(format!(
            "Deciphered the tablet at {cost} Max HP — it took everything."
        ));
    }
    state.lose_max_hp(cost);

    let upgraded = if count + 1 == TABLET_OF_TRUTH_MAX_DECIPHERS {
        // `_decipherCount == 4` (i.e. this is the FIFTH decipher): C# upgrades
        // EVERY upgradable card in the deck and draws nothing from the event
        // stream, so the cursor must not advance here.
        state
            .master_deck
            .iter_mut()
            .filter(|c| c.is_upgradable())
            .fold(0usize, |n, c| n + usize::from(c.upgrade()))
    } else {
        // C#: `base.Rng.NextItem(deck.Where(IsUpgradable))` — a single
        // `NextInt(0, count)` read of the private "TABLET_OF_TRUTH" stream,
        // indexing the deck-ordered upgradable list. `NextItem` == a uniform
        // index draw, so this matches it read-for-read. The stream is C#
        // `base.Rng`, which lives for the whole event, so it is taken from
        // and stored back into the multi-page scratch (the TinkerTime /
        // SlipperyBridge contract) instead of being rebuilt at counter 0 per
        // page. v0.107.1 single-player event seed: additive player term is 0
        // (`EventModel.cs:238` GetPlayerSlotIndex == 0; v0.103.2 used
        // NetId == 1).
        let candidates: Vec<usize> = state
            .master_deck
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.is_upgradable().then_some(i))
            .collect();
        if candidates.is_empty() {
            0
        } else {
            let mut rng = state
                .event_state
                .rng
                .take()
                .unwrap_or_else(|| Rng::with_name(state.rng.seed, TABLET_OF_TRUTH_STREAM));
            let j = rng.next_int_exclusive(0, candidates.len() as i32) as usize;
            state.event_state.rng = Some(rng);
            usize::from(state.master_deck[candidates[j]].upgrade())
        }
    };

    let finished = state.event_state.page >= TABLET_OF_TRUTH_MAX_DECIPHERS;
    EventOutcome {
        finished,
        description: format!("Lost {cost} Max HP, upgraded {upgraded} card(s)."),
        effects: Vec::new(),
    }
}

/// C# `TabletOfTruth.GiveUp` — walk away from an escalation page. Only
/// offered on pages 1..=4; the initial page's second option is Smash.
fn tablet_of_truth_give_up(_state: &mut RunState) -> EventOutcome {
    EventOutcome::finished("Left the tablet unfinished.")
}

/// Present the page the event is actually on.
///
/// Page 0 is `[Decipher, Smash]` (C# `GenerateInitialOptions`); every later
/// page is `[Decipher, Give Up]` (C# `SetEventState`), with the Decipher
/// re-priced and re-keyed. The static `options` list below carries all
/// three so `dispatch::resolve` can still key a handler by `option_id`.
fn tablet_of_truth_options(state: &RunState, spec: &EventSpec) -> Vec<EventOptionInfo> {
    let page = state.event_state.page;
    let cost = tablet_of_truth_decipher_cost(state);
    let last = usize::from(page) + 1 == TABLET_OF_TRUTH_MAX_DECIPHERS as usize;
    let decipher = EventOptionInfo {
        option_id: "decipher",
        loc_key: TABLET_OF_TRUTH_DECIPHER_KEYS
            [(page as usize).min(TABLET_OF_TRUTH_DECIPHER_KEYS.len() - 1)],
        label: "Decipher".to_string(),
        description: if last {
            format!("Lose {cost} Max HP, upgrade every card.")
        } else {
            format!("Lose {cost} Max HP, upgrade a card.")
        },
        enabled: true,
        vars: vec![("DecipherMaxHpLoss", sts2_core::EventVar::Int(cost.into()))],
        title_vars: Vec::new(),
    };
    let second = if page == 0 {
        default_option_info_from_spec(state, spec)
            .into_iter()
            .find(|o| o.option_id == "smash")
    } else {
        default_option_info_from_spec(state, spec)
            .into_iter()
            .find(|o| o.option_id == "give_up")
    };
    let mut out = vec![decipher];
    out.extend(second);
    out
}

const TABLET_OF_TRUTH_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "decipher",
        loc_key: "TABLET_OF_TRUTH.pages.INITIAL.options.DECIPHER_1",
        default_label: "Decipher",
        default_description: "Lose 3 Max HP, upgrade a card.",
        default_vars: &[(
            "DecipherMaxHpLoss",
            EventVarSpec::Int(TABLET_OF_TRUTH_FIRST_DECIPHER_COST as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: tablet_of_truth_decipher,
    },
    EventOptionSpec {
        option_id: "smash",
        loc_key: "TABLET_OF_TRUTH.pages.INITIAL.options.SMASH",
        default_label: "Smash",
        default_description: "Heal 20 HP.",
        default_vars: &[(
            "SmashHPGain",
            EventVarSpec::Int(TABLET_OF_TRUTH_SMASH_HEAL as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: tablet_of_truth_smash,
    },
    EventOptionSpec {
        option_id: "give_up",
        // C# keys Give Up off the PAGE-LESS `pages.DECIPHER` row — the same
        // string on every escalation page, unlike the Decipher beside it.
        loc_key: "TABLET_OF_TRUTH.pages.DECIPHER.options.GIVE_UP",
        default_label: "Give Up",
        default_description: "Walk away.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: tablet_of_truth_give_up,
    },
];

pub const TABLET_OF_TRUTH: EventSpec = EventSpec {
    id: EventId::TabletOfTruth,
    python_name: "TabletOfTruth",
    is_allowed: always_allowed,
    generate_options: tablet_of_truth_options,
    options: TABLET_OF_TRUTH_OPTIONS,
};

// ── Tanx ─────────────────────────────────────────────────────────────

/// Default Tanx weapon-relic picks (`shared.py:1774-1789`). Python
/// rolls 3 from a 9-relic pool via `rng.up_front`; pinned to first 3
/// alphabetical until `RunState.rng` lands (sub-bead `6ynt.4.rng`).
pub const TANX_WEAPON_1: RelicId = RelicId::Claws;
pub const TANX_WEAPON_2: RelicId = RelicId::Crossbow;
pub const TANX_WEAPON_3: RelicId = RelicId::IronClub;

fn tanx_weapon_1(state: &mut RunState) -> EventOutcome {
    state.offer_relic_rewards(TANX_WEAPON_1, 1);
    EventOutcome::finished("Obtained a weapon relic from Tanx.")
}

fn tanx_weapon_2(state: &mut RunState) -> EventOutcome {
    state.offer_relic_rewards(TANX_WEAPON_2, 1);
    EventOutcome::finished("Obtained a weapon relic from Tanx.")
}

fn tanx_weapon_3(state: &mut RunState) -> EventOutcome {
    state.offer_relic_rewards(TANX_WEAPON_3, 1);
    EventOutcome::finished("Obtained a weapon relic from Tanx.")
}

const TANX_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "weapon_1",
        loc_key: "",
        default_label: "Claws",
        default_description: "Gain a weapon relic.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: tanx_weapon_1,
    },
    EventOptionSpec {
        option_id: "weapon_2",
        loc_key: "",
        default_label: "Crossbow",
        default_description: "Gain a weapon relic.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: tanx_weapon_2,
    },
    EventOptionSpec {
        option_id: "weapon_3",
        loc_key: "",
        default_label: "Iron Club",
        default_description: "Gain a weapon relic.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: tanx_weapon_3,
    },
];

pub const TANX: EventSpec = EventSpec {
    id: EventId::Tanx,
    python_name: "Tanx",
    is_allowed: always_allowed,
    generate_options: default_option_info_from_spec,
    options: TANX_OPTIONS,
};

// ── Tezcatara ────────────────────────────────────────────────────────

/// Default Tezcatara comfort-relic picks. C# `Tezcatara.cs` rolls one relic
/// from each of three themed pools via `base.Rng.NextItem`; the Rust port
/// pins each pool to its first C#-array member until the per-pool RNG roll
/// lands (the multi-pool `NextItem` sequencing is deferred). Pool 1 is the
/// `[VeryHotCocoa, YummyCookie]` pair, with `NutritiousSoup` added ONLY when
/// the deck holds a basic Strike (see [`tezcatara_deck_has_basic_strike`]).
pub const TEZCATARA_COMFORT_1: RelicId = RelicId::NutritiousSoup;
/// Pool-1 fallback when the deck has no basic Strike (`NutritiousSoup` is then
/// absent from the C# pool, so the pool-1 representative is `VeryHotCocoa`).
pub const TEZCATARA_COMFORT_1_NO_SOUP: RelicId = RelicId::VeryHotCocoa;
pub const TEZCATARA_COMFORT_2: RelicId = RelicId::BiiigHug;
pub const TEZCATARA_COMFORT_3: RelicId = RelicId::GoldenCompass;

/// C# `Tezcatara.GenerateInitialOptions` (`Tezcatara.cs:200-205`): in
/// v0.107.1 `NutritiousSoup` is no longer a static member of pool 1 — it is
/// added back only when
/// `Deck.Cards.Any(c => c.Tags.Contains(CardTag.Strike) && c.Rarity ==
/// CardRarity.Basic)`. Mirrors the `amalgamator_allowed` basic-Strike idiom
/// (Strike tag + rank-1 rarity via the registered rank fn). The `None`
/// (registry-not-installed) arm conservatively returns `true` — matching the
/// other deck-rarity gates in this crate.
#[must_use]
pub fn tezcatara_deck_has_basic_strike(state: &RunState) -> bool {
    use sts2_core::cards::CardTag;
    let rank = sts2_core::lookup_card_rarity_rank();
    state
        .master_deck
        .iter()
        .any(|c| c.has_tag(CardTag::Strike) && rank.map_or(true, |f| f(c.card_id) == 1))
}

/// Tezcatara option list: pool-1 label reflects whether `NutritiousSoup` is
/// offered (deck has a basic Strike) or the pool falls back to `VeryHotCocoa`.
fn tezcatara_generate_options(state: &RunState, _spec: &EventSpec) -> Vec<EventOptionInfo> {
    let soup = tezcatara_deck_has_basic_strike(state);
    vec![
        EventOptionInfo {
            option_id: "comfort_1",
            loc_key: "",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: if soup {
                "Nutritious Soup"
            } else {
                "Very Hot Cocoa"
            }
            .to_string(),
            description: "Gain a comfort relic.".to_string(),
            enabled: true,
        },
        EventOptionInfo {
            option_id: "comfort_2",
            loc_key: "",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: "Biiig Hug".to_string(),
            description: "Gain a comfort relic.".to_string(),
            enabled: true,
        },
        EventOptionInfo {
            option_id: "comfort_3",
            loc_key: "",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: "Golden Compass".to_string(),
            description: "Gain a comfort relic.".to_string(),
            enabled: true,
        },
    ]
}

fn tezcatara_comfort_1(state: &mut RunState) -> EventOutcome {
    // C# pool 1 is `[VeryHotCocoa, YummyCookie]` with `NutritiousSoup` added
    // only when the deck holds a basic Strike (`Tezcatara.cs:201`). Gate the
    // pool-1 representative accordingly. The NutritiousSoup RELIC combat effect
    // (Strikes cost 0 + Eternal + 3 extra dmg) is tracked separately as bead
    // `sts2-rs-ajjri` (depends on §8 TezcatarasEmber) — only the EVENT-side
    // offer gate is implemented here.
    let relic = if tezcatara_deck_has_basic_strike(state) {
        TEZCATARA_COMFORT_1
    } else {
        TEZCATARA_COMFORT_1_NO_SOUP
    };
    state.offer_relic_rewards(relic, 1);
    EventOutcome::finished("Obtained a relic from Tezcatara.")
}

fn tezcatara_comfort_2(state: &mut RunState) -> EventOutcome {
    state.offer_relic_rewards(TEZCATARA_COMFORT_2, 1);
    EventOutcome::finished("Obtained a relic from Tezcatara.")
}

fn tezcatara_comfort_3(state: &mut RunState) -> EventOutcome {
    state.offer_relic_rewards(TEZCATARA_COMFORT_3, 1);
    EventOutcome::finished("Obtained a relic from Tezcatara.")
}

const TEZCATARA_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "comfort_1",
        loc_key: "",
        default_label: "Nutritious Soup",
        default_description: "Gain a comfort relic.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: tezcatara_comfort_1,
    },
    EventOptionSpec {
        option_id: "comfort_2",
        loc_key: "",
        default_label: "Biiig Hug",
        default_description: "Gain a comfort relic.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: tezcatara_comfort_2,
    },
    EventOptionSpec {
        option_id: "comfort_3",
        loc_key: "",
        default_label: "Golden Compass",
        default_description: "Gain a comfort relic.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: tezcatara_comfort_3,
    },
];

pub const TEZCATARA: EventSpec = EventSpec {
    id: EventId::Tezcatara,
    python_name: "Tezcatara",
    is_allowed: always_allowed,
    generate_options: tezcatara_generate_options,
    options: TEZCATARA_OPTIONS,
};

// ── TheLanternKey ────────────────────────────────────────────────────

/// `TheLanternKey.return_key` gold (`shared.py:1865`, gain_gold(100)).
pub const THE_LANTERN_KEY_RETURN_GOLD: i32 = 100;

fn the_lantern_key_return_key(state: &mut RunState) -> EventOutcome {
    state.gain_gold(THE_LANTERN_KEY_RETURN_GOLD);
    EventOutcome::finished(format!("Gained {THE_LANTERN_KEY_RETURN_GOLD} gold."))
}

/// `KeepTheKey`: C# `TheLanternKey.cs:38-41` calls `SetEventState` with a single
/// `Fight` follow-up option and does NOTHING else — the MysteriousKnight combat
/// and the LanternKey reward live entirely behind the page-1 `fight` option.
/// Returning a not-finished outcome keeps the run-loop in `Phase::Event`
/// (multi-page integration, bead `sts2-rs-m3n0g.3`).
fn the_lantern_key_keep_key(state: &mut RunState) -> EventOutcome {
    state.event_state.page = 1;
    EventOutcome {
        finished: false,
        description: "You pocket the key. A knight steps from the shadows.".to_string(),
        effects: Vec::new(),
    }
}

/// `Fight` (page 1): C# `EnterCombatWithoutExitingEvent<MysteriousKnightEvent
/// Encounter>` with a `SpecialCardReward(LanternKey)` and
/// `shouldResumeAfterCombat: false`.
///
/// Bead `sts2-rs-ck9z6` (GH #166): enters a real `Phase::Combat` against
/// the MysteriousKnight (FlailKnight base + Strength(6) + Plating(6)). The
/// LanternKey card is the combat room's extra reward — granted on a clean
/// victory only (a player death routes to `Phase::RunOver`). No resume:
/// the event finishes once the fight resolves. Returns a NOT-finished
/// outcome so the dispatcher keeps the event room open across the combat.
fn the_lantern_key_fight(state: &mut RunState) -> EventOutcome {
    state.event_combat = Some(EventCombatRequest::no_resume(
        EventEncounter::MysteriousKnight,
        EventId::TheLanternKey,
        vec![Reward::Card(CardReward::from_options(vec![
            CardId::LanternKey,
        ]))],
    ));
    EventOutcome {
        finished: false,
        description: "A knight steps from the shadows. Fight!".to_string(),
        effects: Vec::new(),
    }
}

/// Number of page-0 options (`ReturnTheKey`, `KeepTheKey`). The static option
/// list also carries the page-1 `Fight` handler, which must NOT be presented at
/// page 0 — otherwise the entry option count diverges from C#'s 2.
const THE_LANTERN_KEY_PAGE0_OPTIONS: usize = 2;

/// Multi-page presenter: page 0 = the two initial options; page 1 = the single
/// `Fight` follow-up opened by [`the_lantern_key_keep_key`].
fn the_lantern_key_generate_options(state: &RunState, spec: &EventSpec) -> Vec<EventOptionInfo> {
    if state.event_state.page == 1 {
        return vec![EventOptionInfo {
            option_id: "fight",
            loc_key: "THE_LANTERN_KEY.pages.KEEP_THE_KEY.options.FIGHT",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: "Fight".to_string(),
            description: "Fight the knight for the Lantern Key.".to_string(),
            enabled: true,
        }];
    }
    spec.options[..THE_LANTERN_KEY_PAGE0_OPTIONS]
        .iter()
        .map(|opt| EventOptionInfo {
            option_id: opt.option_id,
            loc_key: opt.loc_key,
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: opt.default_label.to_string(),
            description: opt.default_description.to_string(),
            enabled: true,
        })
        .collect()
}

const THE_LANTERN_KEY_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "return_key",
        loc_key: "THE_LANTERN_KEY.pages.INITIAL.options.RETURN_THE_KEY",
        default_label: "Return the Key",
        default_description: "Gain 100 gold.",
        default_vars: &[(
            "Gold",
            EventVarSpec::Int(THE_LANTERN_KEY_RETURN_GOLD as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: the_lantern_key_return_key,
    },
    EventOptionSpec {
        option_id: "keep_key",
        loc_key: "THE_LANTERN_KEY.pages.INITIAL.options.KEEP_THE_KEY",
        default_label: "Keep the Key",
        default_description: "Fight for Lantern Key card.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: the_lantern_key_keep_key,
    },
    // ── page 1: the Fight follow-up ──
    EventOptionSpec {
        option_id: "fight",
        loc_key: "THE_LANTERN_KEY.pages.KEEP_THE_KEY.options.FIGHT",
        default_label: "Fight",
        default_description: "Fight the knight for the Lantern Key.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: the_lantern_key_fight,
    },
];

pub const THE_LANTERN_KEY: EventSpec = EventSpec {
    id: EventId::TheLanternKey,
    python_name: "TheLanternKey",
    is_allowed: always_allowed,
    generate_options: the_lantern_key_generate_options,
    options: THE_LANTERN_KEY_OPTIONS,
};

// ── ThisOrThat ───────────────────────────────────────────────────────

/// `ThisOrThat.plain` damage (`shared.py:1906`, lose_hp(6)).
pub const THIS_OR_THAT_PLAIN_DAMAGE: i32 = 6;
/// `ThisOrThat.plain` gold (`shared.py:1895`, rng_int(41,69)).
/// Pinned to midpoint 55 until `RunState.rng` lands.
pub const THIS_OR_THAT_PLAIN_GOLD: i32 = 55;

/// `ThisOrThat` Plain gold — C# `CalculateVars` rolls `Rng.NextInt(41, 69)`
/// (max-exclusive ⇒ 41..=68) on the event's OWN RNG, which is an INDEPENDENT
/// per-event stream `Rng(RunState.Rng.Seed + NetId + hash(Id.Entry))`
/// (`EventModel.cs:193`; default `IsShared == false` ⇒ the single-player NetId
/// offset of 1 applies, same as Neow). Because the stream is independent, this
/// only sets the gold VALUE — it does NOT consume the shared run RNG, so no
/// downstream desync. The earlier port used a fixed `55`, diverging the gold
/// total (seed PARITY11: gold cs=178 rs=167). Bead `sts2-rs-thisorthat-gold`.
fn this_or_that_gold(state: &RunState) -> i32 {
    // v0.107.1 single-player event seed: additive player term is 0
    // (`EventModel.cs:238` GetPlayerSlotIndex == 0; v0.103.2 used NetId == 1).
    let mut rng = sts2_core::DotNetRng::with_name(state.rng.seed, "THIS_OR_THAT");
    rng.next_int_exclusive(41, 69)
}

fn this_or_that_plain(state: &mut RunState) -> EventOutcome {
    let lost = state.lose_hp(THIS_OR_THAT_PLAIN_DAMAGE);
    let gold = this_or_that_gold(state);
    state.gain_gold(gold);
    EventOutcome::finished(format!("Took {lost} damage, gained {gold} gold."))
}

fn this_or_that_ornate(state: &mut RunState) -> EventOutcome {
    // C# Ornate (ThisOrThat.cs:43-49): `RelicCmd.Obtain(
    // RelicFactory.PullNextRelicFromFront(owner))` — a DIRECT obtain whose
    // rarity is `RollRarity(PlayerRng.Rewards)` (one float: <0.5 Common /
    // <0.83 Uncommon / else Rare) then a front pull from the player grab bag —
    // followed by `CardPileCmd.AddCurseToDeck<Clumsy>` (a raw deck append, no
    // reward screen). The earlier port pushed a placeholder `Reward::Relic`
    // (wrong relic id, never granted) and skipped the curse, diverging both
    // `relics` and `deck_size` vs C# (seed PARITY11 Ironclad: C# gained
    // PetrifiedToad + a Clumsy curse; Rust gained neither). Same
    // roll-then-front-pull shape as `unrest_site_kill`. Bead
    // `sts2-rs-thisorthat-ornate`.
    let rarity = state.roll_reward_relic_rarity();
    let granted = state.pop_relic_from_grab_bag(rarity);
    if let Some(id) = granted {
        state.obtain_relic(id);
    }
    if let Some(curse) = crate::cards::spec_index::card_pod_from_id(CardId::Clumsy, false) {
        state.master_deck.push(curse);
    }
    EventOutcome::finished(format!(
        "Gained {} and a Clumsy curse.",
        granted.map_or("no relic (bag empty)", |id| id.to_python_name())
    ))
}

const THIS_OR_THAT_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "plain",
        loc_key: "THIS_OR_THAT.pages.INITIAL.options.PLAIN",
        default_label: "Plain",
        default_description: "Take 6 damage, gain ~55 gold.",
        default_vars: &[(
            "HpLoss",
            EventVarSpec::Int(THIS_OR_THAT_PLAIN_DAMAGE as i64),
        )],
        vars_fn: Some(|s| vec![("Gold", this_or_that_gold(s).into())]),
        title_vars_fn: None,
        handler: this_or_that_plain,
    },
    EventOptionSpec {
        option_id: "ornate",
        loc_key: "THIS_OR_THAT.pages.INITIAL.options.ORNATE",
        default_label: "Ornate",
        default_description: "Gain a relic + Clumsy curse.",
        default_vars: &[("Curse", EventVarSpec::Text("Clumsy"))],
        vars_fn: None,
        title_vars_fn: None,
        handler: this_or_that_ornate,
    },
];

pub const THIS_OR_THAT: EventSpec = EventSpec {
    id: EventId::ThisOrThat,
    python_name: "ThisOrThat",
    // C# `ThisOrThat` has NO `IsAllowed` override → always allowed (any act,
    // any gold). The prior `act2_and_gold_100_allowed` gate was a Python-sim
    // fabrication (the Python pin conflated it with WelcomeToWongos' act-2/
    // 100-gold shape), which wrongly excluded ThisOrThat from the act-1 event
    // pool — so the act-1 event-pick walk skipped it and drew the NEXT event,
    // desyncing the whole-run event sequence vs C# (seed PARITY11: C# drew
    // ThisOrThat, Rust drew TabletOfTruth). Bead `sts2-rs-thisorthat-isallowed`.
    is_allowed: always_allowed,
    generate_options: default_option_info_from_spec,
    options: THIS_OR_THAT_OPTIONS,
};

// ── TinkerTime ───────────────────────────────────────────────────────
//
// Multi-page shared event (C# `TinkerTime.cs`, v0.103.2). Three pages:
//   0 INITIAL          → 1 option (choose_card_type).
//   1 CHOOSE_CARD_TYPE → `[Attack, Skill, Power].TakeRandom(2, Rng)` → 2 opts.
//   2 CHOOSE_RIDER     → riders-for-chosen-type `.TakeRandom(2, Rng)` → 2 opts.
// Picking a rider creates a MadScience card moded to (type, rider) and adds it
// to the deck (C# `RunState.CreateCard<MadScience>` + `CardPileCmd.Add(...,
// PileType.Deck)`).
//
// RNG parity (bead `sts2-rs-2guyj`): `TakeRandom(n, rng) ==
// list.UnstableShuffle(rng).Take(n)` (`IEnumerableExtensions.cs`); our
// [`Rng::shuffle`] IS `UnstableShuffle`, so a draw = shuffle in source order
// then take the first n. The event's own stream (C# `EventModel.Rng`,
// v0.107.1 `EventModel.cs:238`: `Rng(Seed + (IsShared ? 0 :
// GetPlayerSlotIndex(Owner)) + deterministicHash(Id.Entry))`; TinkerTime does
// not override `IsShared` so the player term applies, but in single-player
// `GetPlayerSlotIndex == 0` (v0.103.2 used `Owner.NetId == 1`,
// `EventModel.cs:193`). The stream is seeded once on the first page and
// persisted in `event_state.rng` so its counter advances continuously across
// the two draws.

/// [`CardType`] → TinkerTime scratch code (Attack=0, Skill=1, Power=2), matching
/// `pack_tinker_time`'s mode indexing.
fn tt_type_code(t: CardType) -> i16 {
    match t {
        CardType::Skill => 1,
        CardType::Power => 2,
        _ => 0,
    }
}

fn tt_type_from_code(code: i16) -> CardType {
    match code {
        1 => CardType::Skill,
        2 => CardType::Power,
        _ => CardType::Attack,
    }
}

/// C# rider pool for the chosen card type (`ChooseRiderEffect`), in source
/// order — the `TakeRandom` input order is parity-load-bearing.
fn tt_riders_for(t: CardType) -> [TinkerTimeRider; 3] {
    match t {
        CardType::Attack => [
            TinkerTimeRider::Sapping,
            TinkerTimeRider::Violence,
            TinkerTimeRider::Choking,
        ],
        CardType::Skill => [
            TinkerTimeRider::Energized,
            TinkerTimeRider::Wisdom,
            TinkerTimeRider::Chaos,
        ],
        // Power (and any fallback).
        _ => [
            TinkerTimeRider::Expertise,
            TinkerTimeRider::Curious,
            TinkerTimeRider::Improvement,
        ],
    }
}

/// The card type a rider belongs to (inverse of [`tt_riders_for`]).
fn tt_rider_type(r: TinkerTimeRider) -> CardType {
    match r {
        TinkerTimeRider::Sapping | TinkerTimeRider::Violence | TinkerTimeRider::Choking => {
            CardType::Attack
        }
        TinkerTimeRider::Energized | TinkerTimeRider::Wisdom | TinkerTimeRider::Chaos => {
            CardType::Skill
        }
        // Power riders + the `None` fallback.
        _ => CardType::Power,
    }
}

/// `(option_id, loc_key, label)` for a card-type page option. The game names
/// these rows Weapon / Protector / Gadget, not Attack / Skill / Power — one
/// reason to render from `loc_key` rather than the label here.
fn tt_type_option(t: CardType) -> (&'static str, &'static str, &'static str) {
    match t {
        CardType::Skill => (
            "skill",
            "TINKER_TIME.pages.CHOOSE_CARD_TYPE.options.SKILL",
            "Skill",
        ),
        CardType::Power => (
            "power",
            "TINKER_TIME.pages.CHOOSE_CARD_TYPE.options.POWER",
            "Power",
        ),
        _ => (
            "attack",
            "TINKER_TIME.pages.CHOOSE_CARD_TYPE.options.ATTACK",
            "Attack",
        ),
    }
}

/// `(option_id, loc_key, label)` for a rider page option. `None` is the
/// no-rider fallback and has no row of its own.
fn tt_rider_option(r: TinkerTimeRider) -> (&'static str, &'static str, &'static str) {
    match r {
        TinkerTimeRider::Sapping => (
            "sapping",
            "TINKER_TIME.pages.CHOOSE_RIDER.options.SAPPING",
            "Sapping",
        ),
        TinkerTimeRider::Violence => (
            "violence",
            "TINKER_TIME.pages.CHOOSE_RIDER.options.VIOLENCE",
            "Violence",
        ),
        TinkerTimeRider::Choking => (
            "choking",
            "TINKER_TIME.pages.CHOOSE_RIDER.options.CHOKING",
            "Choking",
        ),
        TinkerTimeRider::Energized => (
            "energized",
            "TINKER_TIME.pages.CHOOSE_RIDER.options.ENERGIZED",
            "Energized",
        ),
        TinkerTimeRider::Wisdom => (
            "wisdom",
            "TINKER_TIME.pages.CHOOSE_RIDER.options.WISDOM",
            "Wisdom",
        ),
        TinkerTimeRider::Chaos => (
            "chaos",
            "TINKER_TIME.pages.CHOOSE_RIDER.options.CHAOS",
            "Chaos",
        ),
        TinkerTimeRider::Expertise => (
            "expertise",
            "TINKER_TIME.pages.CHOOSE_RIDER.options.EXPERTISE",
            "Expertise",
        ),
        TinkerTimeRider::Curious => (
            "curious",
            "TINKER_TIME.pages.CHOOSE_RIDER.options.CURIOUS",
            "Curious",
        ),
        TinkerTimeRider::Improvement => (
            "improvement",
            "TINKER_TIME.pages.CHOOSE_RIDER.options.IMPROVEMENT",
            "Improvement",
        ),
        TinkerTimeRider::None => ("none", "", "None"),
    }
}

fn tt_option(
    option_id: &'static str,
    loc_key: &'static str,
    label: &str,
    description: &str,
) -> EventOptionInfo {
    EventOptionInfo {
        option_id,
        loc_key,
        vars: Vec::new(),
        title_vars: Vec::new(),
        label: label.to_string(),
        description: description.to_string(),
        enabled: true,
    }
}

/// Multi-page presenter: renders the current page's option list from
/// `event_state.page` + `event_state.scratch`.
fn tinker_time_generate_options(state: &RunState, _spec: &EventSpec) -> Vec<EventOptionInfo> {
    match state.event_state.page {
        1 => state
            .event_state
            .scratch
            .iter()
            .map(|&code| {
                let (id, loc_key, label) = tt_type_option(tt_type_from_code(code));
                tt_option(
                    id,
                    loc_key,
                    label,
                    "Create a Mad Science card of this type.",
                )
            })
            .collect(),
        2 => state
            .event_state
            .scratch
            .iter()
            .map(|&code| {
                let (id, loc_key, label) = tt_rider_option(TinkerTimeRider::from_u8(code as u8));
                tt_option(id, loc_key, label, "Attach this rider effect.")
            })
            .collect(),
        // Page 0 (and any unexpected cursor) → the single initial option.
        _ => vec![tt_option(
            "choose_card_type",
            "TINKER_TIME.pages.INITIAL.options.CHOOSE_CARD_TYPE",
            "Tinker",
            "Choose the base type for Mad Science.",
        )],
    }
}

fn tinker_time_not_finished(description: &str) -> EventOutcome {
    EventOutcome {
        finished: false,
        description: description.to_string(),
        effects: Vec::new(),
    }
}

/// Take the event RNG out of scratch, creating it on first use (seeded per
/// `EventModel.cs:193`). The caller MUST store it back so the counter persists
/// across pages — `take()` here leaves `event_state.rng == None` mid-handler.
fn tinker_time_take_rng(state: &mut RunState) -> Rng {
    state.event_state.rng.take().unwrap_or_else(|| {
        // C# `base.Id.Entry` is the UPPER_SNAKE model id "TINKER_TIME" (the loc
        // table prefix + EVENT model id), NOT the PascalCase python_name. The
        // event RNG (v0.107.1 `EventModel.cs:238`) hashes that entry, so the
        // seed name MUST be "TINKER_TIME" for cross-engine parity. v0.107.1
        // single-player additive player term is 0 (GetPlayerSlotIndex == 0;
        // v0.103.2 used `Owner.NetId == 1`). Bead `sts2-rs-2guyj`; the 107-oracle
        // RNG re-trace is tracked by `sts2-rs-nbxaw`.
        Rng::with_name(state.rng.seed, "TINKER_TIME")
    })
}

/// INITIAL → draw `[Attack, Skill, Power].TakeRandom(2)`, store the two offered
/// types, advance to the card-type page (not finished).
fn tinker_time_choose_card_type(state: &mut RunState) -> EventOutcome {
    let mut rng = tinker_time_take_rng(state);
    let mut types = [CardType::Attack, CardType::Skill, CardType::Power];
    rng.shuffle(&mut types);
    state.event_state.rng = Some(rng);
    state.event_state.scratch = vec![tt_type_code(types[0]), tt_type_code(types[1])];
    state.event_state.page = 1;
    tinker_time_not_finished("Pick the base card type.")
}

/// CHOOSE_CARD_TYPE pick → draw riders-for-type `TakeRandom(2)`, store the two
/// offered riders, advance to the rider page (not finished).
fn tinker_time_pick_type(state: &mut RunState, chosen: CardType) -> EventOutcome {
    let mut riders = tt_riders_for(chosen);
    let mut rng = tinker_time_take_rng(state);
    rng.shuffle(&mut riders);
    state.event_state.rng = Some(rng);
    state.event_state.scratch = vec![i16::from(riders[0].to_u8()), i16::from(riders[1].to_u8())];
    state.event_state.page = 2;
    tinker_time_not_finished("Pick the rider effect.")
}

fn tinker_time_pick_attack(state: &mut RunState) -> EventOutcome {
    tinker_time_pick_type(state, CardType::Attack)
}
fn tinker_time_pick_skill(state: &mut RunState) -> EventOutcome {
    tinker_time_pick_type(state, CardType::Skill)
}
fn tinker_time_pick_power(state: &mut RunState) -> EventOutcome {
    tinker_time_pick_type(state, CardType::Power)
}

/// CHOOSE_RIDER pick → create the moded MadScience card and add it to the deck.
/// The morph is packed into the card's `VarKey::Generic` slot via
/// [`set_mad_science_variant`]; because `master_deck` cards are cloned fresh
/// into each combat (run_bridge `enter_combat`) and never reset on the run
/// side, the variant is run-persistent across combats. Bead `sts2-rs-2guyj`.
fn tinker_time_grant(rider: TinkerTimeRider) -> EventOutcome {
    let mode = tt_rider_type(rider);
    let mut card = card_pod_from_id(CardId::MadScience, false)
        .expect("MadScience is registered in COLORLESS_SPECS");
    set_mad_science_variant(&mut card, mode, rider);
    EventOutcome::finished("Tinkered — created a Mad Science card.")
        .with_effects(vec![EventEffect::AddCardToDeck(card)])
}

fn tt_rider_sapping(_s: &mut RunState) -> EventOutcome {
    tinker_time_grant(TinkerTimeRider::Sapping)
}
fn tt_rider_violence(_s: &mut RunState) -> EventOutcome {
    tinker_time_grant(TinkerTimeRider::Violence)
}
fn tt_rider_choking(_s: &mut RunState) -> EventOutcome {
    tinker_time_grant(TinkerTimeRider::Choking)
}
fn tt_rider_energized(_s: &mut RunState) -> EventOutcome {
    tinker_time_grant(TinkerTimeRider::Energized)
}
fn tt_rider_wisdom(_s: &mut RunState) -> EventOutcome {
    tinker_time_grant(TinkerTimeRider::Wisdom)
}
fn tt_rider_chaos(_s: &mut RunState) -> EventOutcome {
    tinker_time_grant(TinkerTimeRider::Chaos)
}
fn tt_rider_expertise(_s: &mut RunState) -> EventOutcome {
    tinker_time_grant(TinkerTimeRider::Expertise)
}
fn tt_rider_curious(_s: &mut RunState) -> EventOutcome {
    tinker_time_grant(TinkerTimeRider::Curious)
}
fn tt_rider_improvement(_s: &mut RunState) -> EventOutcome {
    tinker_time_grant(TinkerTimeRider::Improvement)
}

/// Flat option table across ALL pages (the dispatcher keys `option_id` here;
/// [`tinker_time_generate_options`] decides which subset each page presents).
const TINKER_TIME_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "choose_card_type",
        loc_key: "TINKER_TIME.pages.INITIAL.options.CHOOSE_CARD_TYPE",
        default_label: "Tinker",
        default_description: "Choose the base type for Mad Science.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: tinker_time_choose_card_type,
    },
    EventOptionSpec {
        option_id: "attack",
        loc_key: "TINKER_TIME.pages.CHOOSE_CARD_TYPE.options.ATTACK",
        default_label: "Attack",
        default_description: "Create an Attack.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: tinker_time_pick_attack,
    },
    EventOptionSpec {
        option_id: "skill",
        loc_key: "TINKER_TIME.pages.CHOOSE_CARD_TYPE.options.SKILL",
        default_label: "Skill",
        default_description: "Create a Skill.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: tinker_time_pick_skill,
    },
    EventOptionSpec {
        option_id: "power",
        loc_key: "TINKER_TIME.pages.CHOOSE_CARD_TYPE.options.POWER",
        default_label: "Power",
        default_description: "Create a Power.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: tinker_time_pick_power,
    },
    EventOptionSpec {
        option_id: "sapping",
        loc_key: "TINKER_TIME.pages.CHOOSE_RIDER.options.SAPPING",
        default_label: "Sapping",
        default_description: "Attach the Sapping rider.",
        default_vars: &[
            ("SappingWeak", EventVarSpec::Int(2)),
            ("SappingVulnerable", EventVarSpec::Int(2)),
        ],
        vars_fn: None,
        title_vars_fn: None,
        handler: tt_rider_sapping,
    },
    EventOptionSpec {
        option_id: "violence",
        loc_key: "TINKER_TIME.pages.CHOOSE_RIDER.options.VIOLENCE",
        default_label: "Violence",
        default_description: "Attach the Violence rider.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: tt_rider_violence,
    },
    EventOptionSpec {
        option_id: "choking",
        loc_key: "TINKER_TIME.pages.CHOOSE_RIDER.options.CHOKING",
        default_label: "Choking",
        default_description: "Attach the Choking rider.",
        default_vars: &[("ChokingDamage", EventVarSpec::Int(6))],
        vars_fn: None,
        title_vars_fn: None,
        handler: tt_rider_choking,
    },
    EventOptionSpec {
        option_id: "energized",
        loc_key: "TINKER_TIME.pages.CHOOSE_RIDER.options.ENERGIZED",
        default_label: "Energized",
        default_description: "Attach the Energized rider.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: tt_rider_energized,
    },
    EventOptionSpec {
        option_id: "wisdom",
        loc_key: "TINKER_TIME.pages.CHOOSE_RIDER.options.WISDOM",
        default_label: "Wisdom",
        default_description: "Attach the Wisdom rider.",
        default_vars: &[("WisdomCards", EventVarSpec::Int(3))],
        vars_fn: None,
        title_vars_fn: None,
        handler: tt_rider_wisdom,
    },
    EventOptionSpec {
        option_id: "chaos",
        loc_key: "TINKER_TIME.pages.CHOOSE_RIDER.options.CHAOS",
        default_label: "Chaos",
        default_description: "Attach the Chaos rider.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: tt_rider_chaos,
    },
    EventOptionSpec {
        option_id: "expertise",
        loc_key: "TINKER_TIME.pages.CHOOSE_RIDER.options.EXPERTISE",
        default_label: "Expertise",
        default_description: "Attach the Expertise rider.",
        default_vars: &[
            ("ExpertiseStrength", EventVarSpec::Int(2)),
            ("ExpertiseDexterity", EventVarSpec::Int(2)),
        ],
        vars_fn: None,
        title_vars_fn: None,
        handler: tt_rider_expertise,
    },
    EventOptionSpec {
        option_id: "curious",
        loc_key: "TINKER_TIME.pages.CHOOSE_RIDER.options.CURIOUS",
        default_label: "Curious",
        default_description: "Attach the Curious rider.",
        default_vars: &[("CuriousReduction", EventVarSpec::Int(1))],
        vars_fn: None,
        title_vars_fn: None,
        handler: tt_rider_curious,
    },
    EventOptionSpec {
        option_id: "improvement",
        loc_key: "TINKER_TIME.pages.CHOOSE_RIDER.options.IMPROVEMENT",
        default_label: "Improvement",
        default_description: "Attach the Improvement rider.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: tt_rider_improvement,
    },
];

pub const TINKER_TIME: EventSpec = EventSpec {
    id: EventId::TinkerTime,
    python_name: "TinkerTime",
    is_allowed: always_allowed,
    generate_options: tinker_time_generate_options,
    options: TINKER_TIME_OPTIONS,
};

// ── Trial ────────────────────────────────────────────────────────────

fn trial_accept(_state: &mut RunState) -> EventOutcome {
    // Python: rolls merchant/noble/nondescript and presents a guilty/
    // innocent choice that branches into curse-add + relic / heal /
    // upgrade / transform. Multi-page collapses to flavor-only;
    // 6ynt.4.multi-page lands the branches.
    //
    // v0.107.1 RNG-stream migration (bead `…wb8lk.12`): C# `Trial.cs`
    // moved its `CardCmd.TransformToRandom` draw (NondescriptInnocent) from
    // `RunState.Rng.Niche` to the event-local `base.Rng`, and the trial-type
    // pick already uses `base.Rng.NextInt(3)`. NO-OP here: the entire
    // multi-page judgment body (the `NextInt(3)` pick + the transform draws)
    // is unimplemented, so there is no niche draw to migrate. When the
    // multi-page progression lands it must seed off the "TRIAL" event stream
    // (see `roll_transform_replacement_event_stream`), not `rng.niche`.
    EventOutcome::finished(
        "Accepted the trial — multi-page judgment progression deferred to 6ynt.4.multi-page.",
    )
}

fn trial_reject(_state: &mut RunState) -> EventOutcome {
    // Python: presents a `double_down` next-option that calls
    // `lose_run()`. Multi-page collapses to flavor-only;
    // 6ynt.4.multi-page lands the run-loss path.
    EventOutcome::finished(
        "Rejected the trial — multi-page abandon-run path deferred to 6ynt.4.multi-page.",
    )
}

const TRIAL_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "accept",
        loc_key: "TRIAL.pages.INITIAL.options.ACCEPT",
        default_label: "Accept the Trial",
        default_description: "Judge a defendant.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: trial_accept,
    },
    EventOptionSpec {
        option_id: "reject",
        loc_key: "TRIAL.pages.INITIAL.options.REJECT",
        default_label: "Reject",
        default_description: "Reconsider (or abandon run).",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: trial_reject,
    },
];

pub const TRIAL: EventSpec = EventSpec {
    id: EventId::Trial,
    python_name: "Trial",
    is_allowed: always_allowed,
    generate_options: default_option_info_from_spec,
    options: TRIAL_OPTIONS,
};

// ── UnrestSite ───────────────────────────────────────────────────────

/// `UnrestSite.kill` max-HP loss (`shared.py:2269`, lose_max_hp(8)).
pub const UNREST_SITE_KILL_MAX_HP_LOSS: i32 = 8;

/// `UnrestSite.is_allowed` — `current_hp <= max_hp * 0.70`
/// (`shared.py:2247`). The percentage threshold uses integer math
/// (`max_hp * 70 / 100`) to avoid f64 in `const fn` context.
fn unrest_site_allowed(state: &RunState) -> bool {
    state.current_hp <= state.max_hp * 70 / 100
}

fn unrest_site_rest(state: &mut RunState) -> EventOutcome {
    // C# Rest: heal the missing HP (`DynamicVars.Heal` = max - current,
    // set by CalculateVars), then `CardPileCmd.AddCursesToDeck(PoorSleep)`
    // — a raw deck append, no reward screen.
    let missing = state.max_hp - state.current_hp;
    let healed = state.heal(missing);
    if let Some(curse) = crate::cards::spec_index::card_pod_from_id(CardId::PoorSleep, false) {
        state.master_deck.push(curse);
    }
    EventOutcome::finished(format!("Healed {healed} HP, gained a Poor Sleep curse."))
}

fn unrest_site_kill(state: &mut RunState) -> EventOutcome {
    let lost = UNREST_SITE_KILL_MAX_HP_LOSS;
    state.max_hp = state.max_hp.saturating_sub(lost);
    if state.current_hp > state.max_hp {
        state.current_hp = state.max_hp;
    }
    // C# Kill: `RelicCmd.Obtain(RelicFactory.PullNextRelicFromFront(
    // owner))` — DIRECT obtain, no reward screen. The rarity is
    // `RollRarity(PlayerRng.Rewards)` (one float: <0.5 Common,
    // <0.83 Uncommon, else Rare), then a front pull from the grab bag.
    let rarity = state.roll_reward_relic_rarity();
    let granted = state.pop_relic_from_grab_bag(rarity);
    if let Some(id) = granted {
        state.obtain_relic(id);
    }
    EventOutcome::finished(format!(
        "Lost {lost} Max HP, gained {}.",
        granted.map_or("no relic (bag empty)", |id| id.to_python_name())
    ))
}

const UNREST_SITE_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "rest",
        loc_key: "UNREST_SITE.pages.INITIAL.options.REST",
        default_label: "Rest",
        default_description: "Heal to full, gain Poor Sleep curse.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: unrest_site_rest,
    },
    EventOptionSpec {
        option_id: "kill",
        loc_key: "UNREST_SITE.pages.INITIAL.options.KILL",
        default_label: "Kill",
        default_description: "Lose 8 Max HP, gain a relic.",
        default_vars: &[(
            "MaxHpLoss",
            EventVarSpec::Int(UNREST_SITE_KILL_MAX_HP_LOSS as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: unrest_site_kill,
    },
];

pub const UNREST_SITE: EventSpec = EventSpec {
    id: EventId::UnrestSite,
    python_name: "UnrestSite",
    is_allowed: unrest_site_allowed,
    generate_options: default_option_info_from_spec,
    options: UNREST_SITE_OPTIONS,
};

// ── Vakuu ────────────────────────────────────────────────────────────

/// Default Vakuu relic picks (`shared.py:2291-2293`). Pinned to first
/// alphabetical of each themed pool until `RunState.rng` lands.
pub const VAKUU_RELIC_1: RelicId = RelicId::BloodSoakedRose;
pub const VAKUU_RELIC_2: RelicId = RelicId::PreservedFog;
pub const VAKUU_RELIC_3: RelicId = RelicId::ChoicesParadox;

fn vakuu_relic_1(state: &mut RunState) -> EventOutcome {
    state.offer_relic_rewards(VAKUU_RELIC_1, 1);
    EventOutcome::finished("Obtained a relic from Vakuu.")
}

fn vakuu_relic_2(state: &mut RunState) -> EventOutcome {
    state.offer_relic_rewards(VAKUU_RELIC_2, 1);
    EventOutcome::finished("Obtained a relic from Vakuu.")
}

fn vakuu_relic_3(state: &mut RunState) -> EventOutcome {
    state.offer_relic_rewards(VAKUU_RELIC_3, 1);
    EventOutcome::finished("Obtained a relic from Vakuu.")
}

const VAKUU_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "relic_1",
        loc_key: "",
        default_label: "Blood Soaked Rose",
        default_description: "Gain a relic.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: vakuu_relic_1,
    },
    EventOptionSpec {
        option_id: "relic_2",
        loc_key: "",
        default_label: "Preserved Fog",
        default_description: "Gain a relic.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: vakuu_relic_2,
    },
    EventOptionSpec {
        option_id: "relic_3",
        loc_key: "",
        default_label: "Choices Paradox",
        default_description: "Gain a relic.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: vakuu_relic_3,
    },
];

pub const VAKUU: EventSpec = EventSpec {
    id: EventId::Vakuu,
    python_name: "Vakuu",
    is_allowed: always_allowed,
    generate_options: default_option_info_from_spec,
    options: VAKUU_OPTIONS,
};

// ── Wellspring ───────────────────────────────────────────────────────

fn wellspring_bottle(state: &mut RunState) -> EventOutcome {
    // C# Bottle: ONE `Rewards.NextItem` over the FULL character++shared
    // potion pool, offered pre-populated (`OfferCustom(new PotionReward(
    // potionModel))` — no rarity float, no Populate roll). Routed through
    // `procure_potion` so the Sozu veto still applies (the veto in C# is
    // the reward screen's obtain hook; the roll itself always happens).
    let kind = crate::potions::all::roll_event_potion_full_pool(
        &mut state.rng.rewards,
        state.character_id,
    );
    if let Some(kind) = kind {
        if state.interactive_rewards {
            // `OfferCustom`: the claim procures it (Sozu's veto applies there).
            state.offer_custom_rewards(vec![sts2_core::OfferedReward::Potion(Some(kind))], true);
        } else {
            let _ = state.procure_potion(PotionReward::with_kind(kind));
        }
    }
    EventOutcome::finished("Gained a random potion.")
}

fn wellspring_bathe(state: &mut RunState) -> EventOutcome {
    // C# `Wellspring.Bathe` (`Wellspring.cs:44`):
    //   1. `CardSelectCmd.FromDeckForRemoval(count: 1)` — pick 1 card to remove,
    //   2. `CardPileCmd.RemoveFromDeck(cards)`,
    //   3. `AddGuilty(1)` — add 1 Guilty curse AFTER the removal.
    // The Guilty is added LAST, so it must NOT be in the removal pool — defer it
    // via `pending_post_removal_cards` (drained when the removal resolves).
    use sts2_core::cards::CardId;
    let guilty = crate::cards::spec_index::card_pod_from_id(CardId::Guilty, false);
    if state.interactive_card_removal {
        // Surface a removal `card_select` (run loop offers RemoveDeckCard per
        // deck card while `pending_deck_removals > 0`); the deferred Guilty
        // lands once the pick completes.
        state.pending_deck_removals += 1;
        if let Some(card) = guilty {
            state.pending_post_removal_cards.push(card);
        }
    } else {
        // Parity-fuzz / headless auto mode: remove from the deck front + add the
        // Guilty immediately (matches the non-interactive `remove_cards_from_deck`
        // convention other events use).
        let _ = state.remove_cards_from_deck(1);
        if let Some(card) = guilty {
            state.master_deck.push(card);
        }
    }
    EventOutcome::finished("Bathed in the spring — removed a card and gained a Guilty curse.")
}

const WELLSPRING_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "bottle",
        loc_key: "WELLSPRING.pages.INITIAL.options.BOTTLE",
        default_label: "Bottle",
        default_description: "Gain a random potion.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: wellspring_bottle,
    },
    EventOptionSpec {
        option_id: "bathe",
        loc_key: "WELLSPRING.pages.INITIAL.options.BATHE",
        default_label: "Bathe",
        default_description: "Remove 1 card, gain Guilty curse.",
        // C# `Wellspring.Bathe` adds exactly one Guilty (`AddGuilty(1)`).
        default_vars: &[("BatheCurses", EventVarSpec::Int(1))],
        vars_fn: None,
        title_vars_fn: None,
        handler: wellspring_bathe,
    },
];

pub const WELLSPRING: EventSpec = EventSpec {
    id: EventId::Wellspring,
    python_name: "Wellspring",
    is_allowed: always_allowed,
    generate_options: default_option_info_from_spec,
    options: WELLSPRING_OPTIONS,
};

// ── WoodCarvings ─────────────────────────────────────────────────────

/// Shared body for Wood Carvings' Bird/Torus: queue (interactive) or apply
/// (auto) a Basic-card → fixed-replacement transform. C# `Bird`/`Torus`
/// (`WoodCarvings.cs:56/81`): `CardSelectCmd.FromDeckGeneric(prefs(
/// TransformSelectionPrompt, 1), c => c.IsTransformable && c.Rarity ==
/// Basic)` then `CardCmd.TransformTo<Peck/ToricToughness>` — a FIXED
/// replacement (no RNG read), applied as remove-original +
/// append-at-tail (the Deck-pile transform shape).
fn wood_carvings_transform(
    state: &mut RunState,
    target_id: CardId,
    finished: &str,
) -> EventOutcome {
    let Some(target) = crate::cards::spec_index::card_pod_from_id(target_id, false) else {
        return EventOutcome::finished(finished.to_owned());
    };
    if state.interactive_card_transform {
        state
            .pending_deck_transforms
            .push(sts2_core::PendingDeckTransform {
                basic_only: true,
                fixed_target: Some(target),
                ..Default::default()
            });
    } else {
        // Auto mode (parity-fuzz / headless): transform the first Basic
        // deck card (rarity via the C#-discriminant rank fn; Basic == 1).
        let rank = sts2_core::lookup_card_rarity_rank();
        let idx = state.master_deck.iter().position(|c| match rank {
            Some(f) => f(c.card_id) == 1,
            None => true,
        });
        if let Some(idx) = idx {
            state.master_deck.remove(idx);
            state.master_deck.push(target);
        }
    }
    EventOutcome::finished(finished.to_owned())
}

fn wood_carvings_bird(state: &mut RunState) -> EventOutcome {
    wood_carvings_transform(state, CardId::Peck, "Transformed a Basic card into Peck.")
}

fn wood_carvings_snake(state: &mut RunState) -> EventOutcome {
    // C# `Snake` (`WoodCarvings.cs:67`): `CardSelectCmd.FromDeckForEnchantment(
    // Slither, 1)` then `CardCmd.Enchant<Slither>(card, 1m)`.
    use sts2_core::cards::EnchantmentId;
    if state.interactive_card_transform {
        state
            .pending_deck_enchants
            .push(sts2_core::PendingDeckEnchant {
                enchantment: EnchantmentId::Slither,
                amount: 1,
                type_restriction: None,
                secondary_type: None,
            });
    } else {
        // Auto mode: enchant the first eligible card (mirror of
        // `RunManager::enchant_eligible_deck_indices`'s filter).
        use sts2_core::cards::CardType;
        if let Some(card) = state.master_deck.iter_mut().find(|c| {
            !matches!(c.card_type, CardType::Status | CardType::Curse)
                && c.enchantments.is_empty()
                && c.cost >= 0
        }) {
            card.enchantments.push((EnchantmentId::Slither, 1));
        }
    }
    EventOutcome::finished("Enchanted a card with Slither.")
}

fn wood_carvings_torus(state: &mut RunState) -> EventOutcome {
    wood_carvings_transform(
        state,
        CardId::ToricToughness,
        "Transformed a Basic card into Toric Toughness.",
    )
}

const WOOD_CARVINGS_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "bird",
        loc_key: "WOOD_CARVINGS.pages.INITIAL.options.BIRD",
        default_label: "Bird",
        default_description: "Transform a Basic card into Peck.",
        default_vars: &[("BirdCard", EventVarSpec::Text("Peck"))],
        vars_fn: None,
        title_vars_fn: None,
        handler: wood_carvings_bird,
    },
    EventOptionSpec {
        option_id: "snake",
        loc_key: "WOOD_CARVINGS.pages.INITIAL.options.SNAKE",
        default_label: "Snake",
        default_description: "Enchant a card with Slither.",
        default_vars: &[("SnakeEnchantment", EventVarSpec::Text("Slither"))],
        vars_fn: None,
        title_vars_fn: None,
        handler: wood_carvings_snake,
    },
    EventOptionSpec {
        option_id: "torus",
        loc_key: "WOOD_CARVINGS.pages.INITIAL.options.TORUS",
        default_label: "Torus",
        default_description: "Transform a Basic card into Toric Toughness.",
        default_vars: &[("ToricCard", EventVarSpec::Text("Toric Toughness"))],
        vars_fn: None,
        title_vars_fn: None,
        handler: wood_carvings_torus,
    },
];

pub const WOOD_CARVINGS: EventSpec = EventSpec {
    id: EventId::WoodCarvings,
    python_name: "WoodCarvings",
    is_allowed: deck_has_basic_card,
    generate_options: default_option_info_from_spec,
    options: WOOD_CARVINGS_OPTIONS,
};

// ── ZenWeaver ────────────────────────────────────────────────────────

/// `ZenWeaver.is_allowed` — `gold >= 125` (`shared.py:2501`).
pub const ZEN_WEAVER_MIN_GOLD: i32 = 125;
/// `breathing` cost (`shared.py:2516`, lose_gold(50)).
pub const ZEN_WEAVER_BREATHING_COST: i32 = 50;
/// `emotional` cost (`shared.py:2536`, lose_gold(125)).
pub const ZEN_WEAVER_EMOTIONAL_COST: i32 = 125;
/// `acupuncture` cost (`shared.py:2566`, lose_gold(250)).
pub const ZEN_WEAVER_ACUPUNCTURE_COST: i32 = 250;

fn zen_weaver_allowed(state: &RunState) -> bool {
    state.gold >= ZEN_WEAVER_MIN_GOLD
}

fn zen_weaver_breathing(state: &mut RunState) -> EventOutcome {
    // C# `ZenWeaver.BreathingTechniques`: `LoseGold(50)`, then two
    // `CreateCard<Enlightenment>` added to the deck. Unconditional adds.
    state.gold = state.gold.saturating_sub(ZEN_WEAVER_BREATHING_COST);
    for _ in 0..2 {
        if let Some(card) = card_pod_from_id(CardId::Enlightenment, false) {
            state.master_deck.push(card);
        }
    }
    EventOutcome::finished(format!(
        "Paid {ZEN_WEAVER_BREATHING_COST}g, gained 2 Enlightenment cards."
    ))
}

fn zen_weaver_emotional(state: &mut RunState) -> EventOutcome {
    // C# `ZenWeaver.EmotionalAwareness`: `FromDeckForRemoval(1)` → remove, then
    // `LoseGold(125)`. Interactive host queues the removal `card_select`; the
    // headless/parity-fuzz path removes from the deck front (same convention as
    // `wellspring_bathe`).
    if state.gold < ZEN_WEAVER_EMOTIONAL_COST {
        return EventOutcome::finished("Could not afford Emotional Awareness.");
    }
    zen_weaver_remove(state, 1);
    state.gold = state.gold.saturating_sub(ZEN_WEAVER_EMOTIONAL_COST);
    EventOutcome::finished(format!(
        "Paid {ZEN_WEAVER_EMOTIONAL_COST}g, removed a card."
    ))
}

fn zen_weaver_acupuncture(state: &mut RunState) -> EventOutcome {
    // C# `ZenWeaver.ArachnidAcupuncture`: `FromDeckForRemoval(2)` → remove, then
    // `LoseGold(250)`.
    if state.gold < ZEN_WEAVER_ACUPUNCTURE_COST {
        return EventOutcome::finished("Could not afford Arachnid Acupuncture.");
    }
    zen_weaver_remove(state, 2);
    state.gold = state.gold.saturating_sub(ZEN_WEAVER_ACUPUNCTURE_COST);
    EventOutcome::finished(format!(
        "Paid {ZEN_WEAVER_ACUPUNCTURE_COST}g, removed 2 cards."
    ))
}

/// Remove `count` deck cards via the C# `FromDeckForRemoval` shape: queue an
/// interactive `card_select` when the host is interactive, else remove from the
/// deck front (the headless/parity-fuzz convention). No RNG — the player picks.
fn zen_weaver_remove(state: &mut RunState, count: u32) {
    if state.interactive_card_removal {
        state.pending_deck_removals += count;
    } else {
        let _ = state.remove_cards_from_deck(count as usize);
    }
}

const ZEN_WEAVER_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "breathing",
        loc_key: "ZEN_WEAVER.pages.INITIAL.options.BREATHING_TECHNIQUES",
        default_label: "Breathing Techniques",
        default_description: "Pay 50g, gain 2 Enlightenment cards.",
        default_vars: &[(
            "BreathingTechniquesCost",
            EventVarSpec::Int(ZEN_WEAVER_BREATHING_COST as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: zen_weaver_breathing,
    },
    EventOptionSpec {
        option_id: "emotional",
        loc_key: "ZEN_WEAVER.pages.INITIAL.options.EMOTIONAL_AWARENESS",
        default_label: "Emotional Awareness",
        default_description: "Pay 125g, remove 1 card.",
        default_vars: &[(
            "EmotionalAwarenessCost",
            EventVarSpec::Int(ZEN_WEAVER_EMOTIONAL_COST as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: zen_weaver_emotional,
    },
    EventOptionSpec {
        option_id: "acupuncture",
        loc_key: "ZEN_WEAVER.pages.INITIAL.options.ARACHNID_ACUPUNCTURE",
        default_label: "Arachnid Acupuncture",
        default_description: "Pay 250g, remove 2 cards.",
        default_vars: &[(
            "ArachnidAcupunctureCost",
            EventVarSpec::Int(ZEN_WEAVER_ACUPUNCTURE_COST as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: zen_weaver_acupuncture,
    },
];

pub const ZEN_WEAVER: EventSpec = EventSpec {
    id: EventId::ZenWeaver,
    python_name: "ZenWeaver",
    is_allowed: zen_weaver_allowed,
    generate_options: default_option_info_from_spec,
    options: ZEN_WEAVER_OPTIONS,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_state() -> RunState {
        RunState::new()
    }

    /// `Nimble.CanEnchant` requires `card.GainsBlock` (`Nimble.cs:7-14`):
    /// SelfHelpBook's Skill pick offers only block-gaining Skills — a
    /// blockless Skill (Battle Trance) is ineligible while Defend stays in.
    /// Surfaced by the 2026-07-18 shadow-replay census (real DLL offered
    /// only the deck's Defends; the engine also offered blockless Skills).
    #[test]
    fn self_help_nimble_requires_block_gaining_skill() {
        crate::cards::dispatch::ensure_installed();
        use sts2_core::cards::{CardId, CardType, EnchantmentId};
        let defend = crate::cards::spec_index::card_pod_from_id(CardId::DefendIronclad, false)
            .expect("Defend registered");
        let trance = crate::cards::spec_index::card_pod_from_id(CardId::BattleTrance, false)
            .expect("Battle Trance registered");
        assert!(self_help_card_eligible(
            &defend,
            CardType::Skill,
            EnchantmentId::Nimble
        ));
        assert!(!self_help_card_eligible(
            &trance,
            CardType::Skill,
            EnchantmentId::Nimble
        ));
    }

    /// `sts2-rs-thisorthat-isallowed`: C# `ThisOrThat` has no `IsAllowed`
    /// override, so it is eligible in EVERY act regardless of gold (the prior
    /// Python-derived act-2/100-gold gate wrongly excluded it from the act-1
    /// pool, desyncing the whole-run event sequence).
    #[test]
    fn this_or_that_is_allowed_in_act_1_with_no_gold() {
        let mut rs = fresh_state();
        rs.act_index = 1;
        rs.gold = 0;
        assert!(
            (THIS_OR_THAT.is_allowed)(&rs),
            "ThisOrThat is always allowed (C# has no IsAllowed gate)",
        );
    }

    /// `sts2-rs-thisorthat-gold`: Plain rolls gold from the event's own RNG —
    /// C# `CalculateVars` `Rng.NextInt(41, 69)` (max-exclusive ⇒ 41..=68), a
    /// deterministic function of the run seed (independent per-event stream).
    #[test]
    fn this_or_that_plain_rolls_gold_in_range() {
        let mut rs = fresh_state();
        let gold_before = rs.gold;
        let _ = this_or_that_plain(&mut rs);
        let gained = rs.gold - gold_before;
        assert!(
            (41..=68).contains(&gained),
            "Plain gold roll must land in [41, 68], got {gained}",
        );
    }

    /// `sts2-rs-thisorthat-ornate`: Ornate always adds exactly one Clumsy curse
    /// to the deck (C# `AddCurseToDeck<Clumsy>`, a raw deck append independent
    /// of the relic pull), and never queues a stale `Reward::Relic`.
    #[test]
    fn this_or_that_ornate_adds_clumsy_curse_no_pending_reward() {
        crate::cards::dispatch::install_default_registry();
        let mut rs = fresh_state();
        let deck_before = rs.master_deck.len();
        let _ = this_or_that_ornate(&mut rs);
        assert_eq!(
            rs.master_deck.len(),
            deck_before + 1,
            "Ornate appends exactly one card (the Clumsy curse)",
        );
        assert_eq!(
            rs.master_deck.last().map(|c| c.card_id),
            Some(sts2_core::cards::CardId::Clumsy),
            "the appended card is Clumsy",
        );
        assert!(
            rs.pending_rewards.is_empty(),
            "Ornate grants the relic directly (no stale Reward::Relic)",
        );
    }

    // ── per-event spec sentinels ──────────────────────────────────

    #[test]
    fn wood_carvings_interactive_queues_deck_picks() {
        // Interactive hosts: Bird/Torus queue a Basic-only fixed-target
        // transform (Peck / ToricToughness, no RNG); Snake queues a
        // 1-stack Slither enchant. Resolution is exercised by the
        // run_manager tests; this pins the handler→queue wiring.
        crate::cards::dispatch::ensure_installed();
        let mut rs = fresh_state();
        rs.interactive_card_transform = true;
        seed_deck(&mut rs, 3);
        wood_carvings_bird(&mut rs);
        wood_carvings_torus(&mut rs);
        wood_carvings_snake(&mut rs);
        assert_eq!(rs.pending_deck_transforms.len(), 2);
        assert!(rs.pending_deck_transforms.iter().all(|t| t.basic_only));
        assert_eq!(
            rs.pending_deck_transforms[0]
                .fixed_target
                .as_ref()
                .map(|c| c.card_id),
            Some(CardId::Peck)
        );
        assert_eq!(
            rs.pending_deck_transforms[1]
                .fixed_target
                .as_ref()
                .map(|c| c.card_id),
            Some(CardId::ToricToughness)
        );
        assert_eq!(rs.pending_deck_enchants.len(), 1);
        assert_eq!(
            rs.pending_deck_enchants[0].enchantment,
            sts2_core::cards::EnchantmentId::Slither
        );
        assert_eq!(rs.pending_deck_enchants[0].amount, 1);
        // No deck mutation until the player picks.
        assert_eq!(rs.master_deck.len(), 3);
        assert!(rs.master_deck.iter().all(|c| c.enchantments.is_empty()));
    }

    #[test]
    fn wood_carvings_auto_mode_transforms_first_basic_and_enchants_first_eligible() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = fresh_state();
        seed_deck(&mut rs, 2);
        wood_carvings_bird(&mut rs);
        // First Basic (Strike at slot 0) removed, Peck appended at tail.
        assert_eq!(rs.master_deck.len(), 2);
        assert_eq!(rs.master_deck[1].card_id, CardId::Peck);
        wood_carvings_snake(&mut rs);
        // First eligible card gains 1 Slither stack.
        assert_eq!(
            rs.master_deck[0].enchantments.as_slice(),
            &[(sts2_core::cards::EnchantmentId::Slither, 1)]
        );
    }

    #[test]
    fn reflections_spec_matches_python_pin() {
        assert_eq!(REFLECTIONS.python_name, "Reflections");
        assert_eq!(REFLECTIONS.options.len(), 2);
        let rs = fresh_state();
        assert!((REFLECTIONS.is_allowed)(&rs));
    }

    /// Seed a deck of `n` un-upgraded StrikeIronclad cards.
    fn seed_deck(rs: &mut RunState, n: usize) {
        for _ in 0..n {
            rs.master_deck.push(
                crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false)
                    .expect("strike exists"),
            );
        }
    }

    fn push_card(rs: &mut RunState, id: CardId) {
        rs.master_deck
            .push(crate::cards::spec_index::card_pod_from_id(id, false).expect("card exists"));
    }

    #[test]
    fn self_help_book_options_lock_per_missing_card_type() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = fresh_state();
        // Attacks only — READ_THE_BACK live, the other two locked
        // (C# null-handler options still rendered).
        seed_deck(&mut rs, 2);
        let opts = (SELF_HELP_BOOK.generate_options)(&rs, &SELF_HELP_BOOK);
        assert_eq!(opts.len(), 3);
        assert!(opts[0].enabled, "Sharp/Attack live");
        assert!(!opts[1].enabled, "Nimble/Skill locked (no Skill in deck)");
        assert!(!opts[2].enabled, "Swift/Power locked (no Power in deck)");
    }

    #[test]
    fn self_help_book_options_skill_only_still_shows_three_options() {
        // Skill-only deck (no Attack, no Power). Kills the 340:26 `||`→`&&`
        // mutant: `attack || skill || power` mis-mutated to
        // `attack || (skill && power)` (the second `||` only, `&&` binding
        // tighter). With attack=false, skill=true, power=false the original
        // `false||true||false` = true ⇒ 3 READ options, but the mutant
        // `false||(true&&false)` = false ⇒ collapses to the single skip.
        // (The Attacks-only `lock_per_missing_card_type` test cannot catch
        // this: there attack=true short-circuits both forms to true.)
        crate::cards::dispatch::ensure_installed();
        let mut rs = fresh_state();
        push_card(&mut rs, CardId::DefendIronclad); // sole eligible: a Skill
        let opts = (SELF_HELP_BOOK.generate_options)(&rs, &SELF_HELP_BOOK);
        assert_eq!(
            opts.len(),
            3,
            "one eligible Skill must still render the 3 READ options"
        );
        assert!(!opts[0].enabled, "Sharp/Attack locked (no Attack)");
        assert!(opts[1].enabled, "Nimble/Skill live");
        assert!(!opts[2].enabled, "Swift/Power locked (no Power)");
    }

    #[test]
    fn self_help_book_options_collapse_to_skip_when_nothing_enchantable() {
        crate::cards::dispatch::ensure_installed();
        let rs = fresh_state();
        let opts = (SELF_HELP_BOOK.generate_options)(&rs, &SELF_HELP_BOOK);
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].option_id, "no_options");
        assert!(opts[0].enabled);
    }

    #[test]
    fn self_help_book_interactive_queues_type_restricted_enchant() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = fresh_state();
        rs.interactive_card_transform = true;
        seed_deck(&mut rs, 2); // two Attacks → interactive pick
        self_help_book_read_back(&mut rs);
        assert_eq!(rs.pending_deck_enchants.len(), 1);
        let pending = &rs.pending_deck_enchants[0];
        assert_eq!(pending.enchantment, sts2_core::cards::EnchantmentId::Sharp);
        assert_eq!(pending.amount, SELF_HELP_BOOK_ENCHANT_AMOUNT);
        assert_eq!(
            pending.type_restriction,
            Some(sts2_core::cards::CardType::Attack)
        );
        // No deck mutation until the player picks.
        assert!(rs.master_deck.iter().all(|c| c.enchantments.is_empty()));
    }

    #[test]
    fn self_help_book_single_eligible_auto_enchants_without_select() {
        // C# `FromSimpleGrid` auto-takes the pool at count <= MinSelect
        // (== 1 here): one eligible Skill enchants directly, no
        // card_select step even on an interactive host.
        crate::cards::dispatch::ensure_installed();
        let mut rs = fresh_state();
        rs.interactive_card_transform = true;
        seed_deck(&mut rs, 2);
        push_card(&mut rs, CardId::DefendIronclad); // sole Skill
        self_help_book_read_passage(&mut rs);
        assert!(rs.pending_deck_enchants.is_empty());
        let defend = rs
            .master_deck
            .iter()
            .find(|c| c.card_id == CardId::DefendIronclad)
            .expect("defend still in deck");
        assert_eq!(
            defend.enchantments.as_slice(),
            &[(
                sts2_core::cards::EnchantmentId::Nimble,
                SELF_HELP_BOOK_ENCHANT_AMOUNT
            )]
        );
    }

    #[test]
    fn self_help_book_auto_mode_enchants_first_eligible_of_type() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = fresh_state();
        push_card(&mut rs, CardId::DefendIronclad);
        seed_deck(&mut rs, 2);
        self_help_book_read_back(&mut rs);
        // First Attack (slot 1) gets Sharp; the Skill at slot 0 untouched.
        assert!(rs.master_deck[0].enchantments.is_empty());
        assert_eq!(
            rs.master_deck[1].enchantments.as_slice(),
            &[(
                sts2_core::cards::EnchantmentId::Sharp,
                SELF_HELP_BOOK_ENCHANT_AMOUNT
            )]
        );
        assert!(rs.master_deck[2].enchantments.is_empty());
    }

    #[test]
    fn reflections_touch_upgrades_four_cards() {
        let mut rs = fresh_state();
        seed_deck(&mut rs, 8);
        let outcome = (REFLECTIONS.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        // Fresh deck has nothing to downgrade; exactly 4 get upgraded.
        let upgraded = rs.master_deck.iter().filter(|c| c.upgraded).count();
        assert_eq!(upgraded, 4);
    }

    #[test]
    fn reflections_shatter_duplicates_deck_and_adds_curse() {
        let mut rs = fresh_state();
        seed_deck(&mut rs, 5);
        let outcome = (REFLECTIONS.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        // 5 originals + 5 clones + 1 BadLuck curse.
        assert_eq!(rs.master_deck.len(), 11);
        assert_eq!(rs.master_deck.last().unwrap().card_id, CardId::BadLuck);
        // Clones carry distinct instance UIDs from their sources.
        assert_ne!(rs.master_deck[0].uid, rs.master_deck[5].uid);
    }

    #[test]
    fn round_tea_party_enjoy_tea_heals_and_queues_relic() {
        let mut rs = fresh_state();
        rs.current_hp = 10;
        let outcome = (ROUND_TEA_PARTY.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.current_hp, rs.max_hp);
        assert_eq!(rs.relic_rewards.len(), 1);
        assert_eq!(rs.relic_rewards[0].relic_id, RelicId::RoyalPoison);
    }

    #[test]
    fn round_tea_party_pick_fight_opens_continue_fight_page_without_damage() {
        let mut rs = fresh_state();
        let hp_before = rs.current_hp;
        // PickFight (page 0) only opens the page — C# SetEventState, no effect.
        let outcome = (ROUND_TEA_PARTY.options[1].handler)(&mut rs);
        assert!(!outcome.finished, "pick_fight is a multi-page step");
        assert_eq!(rs.current_hp, hp_before, "no damage until continue_fight");
        assert!(rs.pending_rewards.is_empty());
        assert_eq!(rs.event_state.page, 1);
        // Page 1 presents exactly the single ContinueFight follow-up.
        let page1 = round_tea_party_generate_options(&rs, &ROUND_TEA_PARTY);
        assert_eq!(page1.len(), 1);
        assert_eq!(page1[0].option_id, "continue_fight");
        // ContinueFight: 11 unblockable self-damage, finishes.
        let outcome = (ROUND_TEA_PARTY.options[2].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.current_hp, hp_before - ROUND_TEA_PARTY_FIGHT_DAMAGE);
    }

    #[test]
    fn round_tea_party_page0_presents_two_options() {
        let rs = fresh_state();
        let opts = round_tea_party_generate_options(&rs, &ROUND_TEA_PARTY);
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0].option_id, "enjoy_tea");
        assert_eq!(opts[1].option_id, "pick_fight");
    }

    #[test]
    fn sapphire_seed_eat_heals_9_and_offers_upgrade_pick() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = fresh_state();
        rs.current_hp = 10;
        rs.interactive_card_upgrade = true;
        seed_deck(&mut rs, 1);
        let outcome = (SAPPHIRE_SEED.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.current_hp, 19);
        // Interactive host: a pick-1 upgrade card_select, not a stale reward.
        assert!(
            rs.pending_rewards.is_empty(),
            "no stale UpgradeCards reward"
        );
        assert_eq!(
            rs.pending_deck_upgrades, 1,
            "queues a pick-1 upgrade card_select"
        );
    }

    #[test]
    fn sapphire_seed_plant_interactive_queues_unrestricted_sown_enchant() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = fresh_state();
        rs.interactive_card_transform = true;
        seed_deck(&mut rs, 2);
        let outcome = (SAPPHIRE_SEED.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.pending_deck_enchants.len(), 1);
        let pending = &rs.pending_deck_enchants[0];
        assert_eq!(pending.enchantment, sts2_core::cards::EnchantmentId::Sown);
        assert_eq!(pending.amount, 1);
        assert_eq!(pending.type_restriction, None);
    }

    #[test]
    fn sapphire_seed_plant_single_eligible_auto_enchants() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = fresh_state();
        rs.interactive_card_transform = true;
        seed_deck(&mut rs, 1);
        let outcome = (SAPPHIRE_SEED.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert!(rs.pending_deck_enchants.is_empty());
        assert_eq!(
            rs.master_deck[0].enchantments.as_slice(),
            &[(sts2_core::cards::EnchantmentId::Sown, 1)]
        );
    }

    #[test]
    fn spirit_grafter_let_it_in_heals_25_and_adds_metamorphosis() {
        let mut rs = fresh_state();
        rs.current_hp = 30;
        let deck_before = rs.master_deck.len();
        let outcome = (SPIRIT_GRAFTER.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.current_hp, 55);
        assert_eq!(rs.master_deck.len(), deck_before + 1);
        assert!(rs
            .master_deck
            .iter()
            .any(|c| c.card_id == CardId::Metamorphosis));
    }

    #[test]
    fn spirit_grafter_rejection_loses_10_hp_and_upgrades() {
        let mut rs = fresh_state();
        seed_deck(&mut rs, 3);
        let hp_before = rs.current_hp;
        let upgraded_before = rs.master_deck.iter().filter(|c| c.upgraded).count();
        let outcome = (SPIRIT_GRAFTER.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.current_hp, hp_before - 10);
        // Headless (non-interactive) mode upgrades one deck card immediately.
        let upgraded_after = rs.master_deck.iter().filter(|c| c.upgraded).count();
        assert_eq!(upgraded_after, upgraded_before + 1);
    }

    #[test]
    fn sunken_statue_grab_sword_offers_sword_of_stone() {
        let mut rs = fresh_state();
        let outcome = (SUNKEN_STATUE.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.relic_rewards.len(), 1);
        assert_eq!(rs.relic_rewards[0].relic_id, RelicId::SwordOfStone);
    }

    #[test]
    fn sunken_statue_dive_gains_111_pm_10_gold_loses_7_hp() {
        let mut rs = fresh_state();
        let gold_before = rs.gold;
        let hp_before = rs.current_hp;
        // The granted gold is the deterministic CalculateVars roll
        // (`111 + NextInt(-10, 11)`), re-derivable from the same stream.
        let expected_roll = sunken_statue_rolled_gold(&rs);
        let outcome = (SUNKEN_STATUE.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.gold, gold_before + expected_roll);
        assert!(
            (SUNKEN_STATUE_DIVE_GOLD - SUNKEN_STATUE_GOLD_VARIANCE
                ..=SUNKEN_STATUE_DIVE_GOLD + SUNKEN_STATUE_GOLD_VARIANCE)
                .contains(&expected_roll),
            "rolled gold {expected_roll} out of 111±10 band"
        );
        assert_eq!(rs.current_hp, hp_before - SUNKEN_STATUE_DIVE_DAMAGE);
    }

    #[test]
    fn sunken_treasury_first_chest_gains_60() {
        let mut rs = fresh_state();
        let gold_before = rs.gold;
        let outcome = (SUNKEN_TREASURY.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.gold, gold_before + SUNKEN_TREASURY_SMALL_GOLD);
    }

    #[test]
    fn sunken_treasury_second_chest_gains_333() {
        let mut rs = fresh_state();
        let gold_before = rs.gold;
        let outcome = (SUNKEN_TREASURY.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.gold, gold_before + SUNKEN_TREASURY_LARGE_GOLD);
    }

    #[test]
    fn tablet_of_truth_smash_heals_20() {
        let mut rs = fresh_state();
        rs.current_hp = 30;
        let outcome = (TABLET_OF_TRUTH.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.current_hp, 50);
    }

    #[test]
    fn tablet_of_truth_decipher_loses_max_hp_3() {
        let mut rs = fresh_state();
        let max_before = rs.max_hp;
        let outcome = (TABLET_OF_TRUTH.options[0].handler)(&mut rs);
        assert!(
            !outcome.finished,
            "the first Decipher opens page 1, it does not end the event"
        );
        assert_eq!(rs.max_hp, max_before - 3);
    }

    #[test]
    fn tablet_of_truth_decipher_upgrades_one_card() {
        let mut rs = fresh_state();
        seed_deck(&mut rs, 8);
        let upgradable_before = rs.master_deck.iter().filter(|c| c.is_upgradable()).count();
        assert!(upgradable_before > 0, "fixture needs an upgradable card");
        let upgraded_before = rs.master_deck.iter().filter(|c| c.upgraded).count();
        let outcome = (TABLET_OF_TRUTH.options[0].handler)(&mut rs);
        assert!(!outcome.finished);
        let upgraded_after = rs.master_deck.iter().filter(|c| c.upgraded).count();
        assert_eq!(
            upgraded_after,
            upgraded_before + 1,
            "decipher upgrades exactly one deck card"
        );
    }

    /// GH #487. The escalation is the whole point of the event: a model that
    /// prices every page at 3 tells the run policy that the last Decipher is
    /// the same cheap deal as the first, and it walked a live A9 run to 1/1
    /// Max HP at p=94% on exactly that reading.
    #[test]
    fn tablet_of_truth_decipher_price_escalates_3_6_12_24_then_all_but_one() {
        let mut rs = fresh_state();
        rs.max_hp = 200;
        rs.current_hp = 200;
        seed_deck(&mut rs, 30);
        let mut prices = Vec::new();
        for step in 0..TABLET_OF_TRUTH_MAX_DECIPHERS {
            assert_eq!(rs.event_state.page, step);
            prices.push(tablet_of_truth_decipher_cost(&rs));
            let before = rs.max_hp;
            let outcome = (TABLET_OF_TRUTH.options[0].handler)(&mut rs);
            assert_eq!(before - rs.max_hp, *prices.last().unwrap());
            assert_eq!(
                outcome.finished,
                step + 1 == TABLET_OF_TRUTH_MAX_DECIPHERS,
                "only the fifth Decipher ends the event"
            );
        }
        // 200 → 197 → 191 → 179 → 155, then the last page prices itself at
        // `MaxHp - 1` = 154 and leaves exactly 1.
        assert_eq!(prices, vec![3, 6, 12, 24, 154]);
        assert_eq!(rs.max_hp, 1);
        assert_eq!(rs.current_hp, 1);
    }

    /// The fifth Decipher upgrades the WHOLE deck (`_decipherCount == 4`
    /// branch), not one card.
    #[test]
    fn tablet_of_truth_last_page_upgrades_every_upgradable_card() {
        let mut rs = fresh_state();
        rs.max_hp = 200;
        rs.current_hp = 200;
        seed_deck(&mut rs, 10);
        rs.event_state.page = 4;
        let upgradable = rs.master_deck.iter().filter(|c| c.is_upgradable()).count();
        assert!(upgradable > 1, "fixture needs several upgradable cards");
        let outcome = (TABLET_OF_TRUTH.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(
            rs.master_deck.iter().filter(|c| c.is_upgradable()).count(),
            0,
            "every upgradable card is upgraded on the last page"
        );
    }

    /// `LoseMaxHpAndUpgrade`'s `if (!(hp < MaxHp))` branch: the price at or
    /// above Max HP drops you to 1 Max HP and then KILLS you — it does not
    /// merely clamp. This is the branch the option's `ThatWillKillPlayerIf`
    /// flags, and the one a lethal-choice veto has to be able to preview.
    #[test]
    fn tablet_of_truth_decipher_at_or_above_max_hp_kills() {
        let mut rs = fresh_state();
        rs.max_hp = 6;
        rs.current_hp = 6;
        rs.event_state.page = 1; // page 1 costs 6
        let outcome = (TABLET_OF_TRUTH.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.max_hp, 1);
        assert_eq!(rs.current_hp, 0, "CreatureCmd.Kill, not a clamp");
    }

    /// Page 0 offers Smash; every escalation page swaps it for Give Up, and
    /// Give Up ends the event without touching Max HP.
    #[test]
    fn tablet_of_truth_pages_swap_smash_for_give_up() {
        let mut rs = fresh_state();
        let initial = (TABLET_OF_TRUTH.generate_options)(&rs, &TABLET_OF_TRUTH);
        assert_eq!(
            initial.iter().map(|o| o.option_id).collect::<Vec<_>>(),
            vec!["decipher", "smash"]
        );
        assert_eq!(
            initial[0].loc_key,
            "TABLET_OF_TRUTH.pages.INITIAL.options.DECIPHER_1"
        );
        rs.event_state.page = 2;
        let later = (TABLET_OF_TRUTH.generate_options)(&rs, &TABLET_OF_TRUTH);
        assert_eq!(
            later.iter().map(|o| o.option_id).collect::<Vec<_>>(),
            vec!["decipher", "give_up"]
        );
        assert_eq!(
            later[0].loc_key,
            "TABLET_OF_TRUTH.pages.DECIPHER_2.options.DECIPHER"
        );
        assert_eq!(
            later[0].vars,
            vec![("DecipherMaxHpLoss", sts2_core::EventVar::Int(12))],
            "the page's own price reaches the display layer, not the initial 3"
        );
        let max_before = rs.max_hp;
        let outcome = (TABLET_OF_TRUTH.options[2].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.max_hp, max_before);
    }

    #[test]
    fn tanx_weapon_options_queue_distinct_relics() {
        for (idx, expected) in [(0, TANX_WEAPON_1), (1, TANX_WEAPON_2), (2, TANX_WEAPON_3)] {
            let mut rs = fresh_state();
            let outcome = (TANX.options[idx].handler)(&mut rs);
            assert!(outcome.finished);
            assert_eq!(rs.relic_rewards.len(), 1);
            assert_eq!(rs.relic_rewards[0].relic_id, expected);
        }
    }

    /// Seed a single basic Strike so `tezcatara_deck_has_basic_strike` is true.
    fn seed_basic_strike(rs: &mut RunState) {
        crate::cards::dispatch::ensure_installed();
        rs.master_deck.push(
            crate::cards::spec_index::card_pod_from_id(
                sts2_core::cards::CardId::StrikeIronclad,
                false,
            )
            .expect("strike exists"),
        );
    }

    #[test]
    fn tezcatara_three_pools_queue_distinct_relics() {
        // With a basic Strike in the deck, pool 1 includes NutritiousSoup.
        for (idx, expected) in [
            (0, TEZCATARA_COMFORT_1),
            (1, TEZCATARA_COMFORT_2),
            (2, TEZCATARA_COMFORT_3),
        ] {
            let mut rs = fresh_state();
            seed_basic_strike(&mut rs);
            let outcome = (TEZCATARA.options[idx].handler)(&mut rs);
            assert!(outcome.finished);
            assert_eq!(rs.relic_rewards[0].relic_id, expected);
        }
    }

    /// v0.107.1 (`Tezcatara.cs:201`): NutritiousSoup is offered ONLY when the
    /// deck holds a basic Strike; otherwise pool 1 falls back to VeryHotCocoa.
    #[test]
    fn tezcatara_soup_gated_on_basic_strike() {
        // No basic Strike → pool-1 representative is VeryHotCocoa, NOT soup.
        let mut rs = fresh_state();
        let outcome = (TEZCATARA.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.relic_rewards[0].relic_id, TEZCATARA_COMFORT_1_NO_SOUP);

        // With a basic Strike → soup is offered.
        let mut rs2 = fresh_state();
        seed_basic_strike(&mut rs2);
        let outcome2 = (TEZCATARA.options[0].handler)(&mut rs2);
        assert!(outcome2.finished);
        assert_eq!(rs2.relic_rewards[0].relic_id, TEZCATARA_COMFORT_1);
    }

    /// A non-basic Strike (Common rarity, e.g. PommelStrike) carries the Strike
    /// tag but must NOT satisfy the basic-Strike gate.
    #[test]
    fn tezcatara_non_basic_strike_does_not_open_soup() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = fresh_state();
        rs.master_deck.push(
            crate::cards::spec_index::card_pod_from_id(
                sts2_core::cards::CardId::PommelStrike,
                false,
            )
            .expect("pommel strike exists"),
        );
        assert!(
            !tezcatara_deck_has_basic_strike(&rs),
            "Common Strike-tagged card is not a basic Strike",
        );
    }

    /// The option-list label reflects the gate (soup vs cocoa).
    #[test]
    fn tezcatara_option_label_tracks_soup_gate() {
        let mut rs = fresh_state();
        let no_soup = (TEZCATARA.generate_options)(&rs, &TEZCATARA);
        assert_eq!(no_soup[0].label, "Very Hot Cocoa");

        seed_basic_strike(&mut rs);
        let soup = (TEZCATARA.generate_options)(&rs, &TEZCATARA);
        assert_eq!(soup[0].label, "Nutritious Soup");
    }

    #[test]
    fn the_lantern_key_return_gains_100_gold() {
        let mut rs = fresh_state();
        let gold_before = rs.gold;
        let outcome = (THE_LANTERN_KEY.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.gold, gold_before + 100);
    }

    #[test]
    fn the_lantern_key_keep_opens_fight_page_then_grants_lantern_key() {
        let mut rs = fresh_state();
        // KeepTheKey (page 0) only opens the page — C# SetEventState, no effect.
        let outcome = (THE_LANTERN_KEY.options[1].handler)(&mut rs);
        assert!(!outcome.finished, "keep_key is a multi-page step");
        assert!(rs.pending_rewards.is_empty());
        assert_eq!(rs.event_state.page, 1);
        // Page 1 presents exactly the single Fight follow-up.
        let page1 = the_lantern_key_generate_options(&rs, &THE_LANTERN_KEY);
        assert_eq!(page1.len(), 1);
        assert_eq!(page1[0].option_id, "fight");
        // Fight (bead `sts2-rs-ck9z6`, GH #166): stages a real no-resume
        // MysteriousKnight combat whose victory reward is the LanternKey card.
        let outcome = (THE_LANTERN_KEY.options[2].handler)(&mut rs);
        assert!(!outcome.finished, "fight defers finish to post-combat");
        assert!(
            rs.pending_rewards.is_empty(),
            "card reward waits for victory"
        );
        let req = rs.event_combat.as_ref().expect("fight staged");
        assert!(!req.resume);
        assert_eq!(
            req.encounter,
            sts2_core::events::EventEncounter::MysteriousKnight
        );
        assert_eq!(req.victory_rewards.len(), 1);
        assert!(matches!(req.victory_rewards[0], Reward::Card(_)));
    }

    #[test]
    fn the_lantern_key_page0_presents_two_options() {
        let rs = fresh_state();
        let opts = the_lantern_key_generate_options(&rs, &THE_LANTERN_KEY);
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0].option_id, "return_key");
        assert_eq!(opts[1].option_id, "keep_key");
    }

    #[test]
    fn this_or_that_plain_loses_6_gains_rolled_gold() {
        let mut rs = fresh_state();
        let gold_before = rs.gold;
        let hp_before = rs.current_hp;
        let outcome = (THIS_OR_THAT.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.current_hp, hp_before - 6);
        // Gold is rolled (C# CalculateVars NextInt(41,69)), not a fixed 55.
        let gained = rs.gold - gold_before;
        assert!(
            (41..=68).contains(&gained),
            "rolled gold in [41,68], got {gained}"
        );
    }

    #[test]
    fn unrest_site_is_allowed_gates_on_70pct_max_hp() {
        let mut rs = fresh_state();
        rs.max_hp = 100;
        rs.current_hp = 71;
        assert!(!(UNREST_SITE.is_allowed)(&rs));
        rs.current_hp = 70;
        assert!((UNREST_SITE.is_allowed)(&rs));
        rs.current_hp = 30;
        assert!((UNREST_SITE.is_allowed)(&rs));
    }

    #[test]
    fn unrest_site_rest_heals_to_full() {
        let mut rs = fresh_state();
        rs.max_hp = 100;
        rs.current_hp = 30;
        let outcome = (UNREST_SITE.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.current_hp, rs.max_hp);
    }

    #[test]
    fn unrest_site_kill_loses_8_max_hp_and_directly_obtains_grab_bag_relic() {
        let mut rs = fresh_state();
        // One-relic-per-bucket synthetic bag: whatever rarity the
        // Rewards roll lands, exactly one of these is granted directly.
        let mut by_rarity = std::collections::BTreeMap::new();
        by_rarity.insert(sts2_core::RelicRarity::Common, vec![RelicId::PrayerWheel]);
        by_rarity.insert(sts2_core::RelicRarity::Uncommon, vec![RelicId::Nunchaku]);
        by_rarity.insert(sts2_core::RelicRarity::Rare, vec![RelicId::RoyalPoison]);
        rs.populate_relic_grab_bag_by_rarity(by_rarity, vec![], vec![]);
        let max_before = rs.max_hp;
        let outcome = (UNREST_SITE.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.max_hp, max_before - 8);
        // Direct obtain (C# RelicCmd.Obtain) — no reward screen.
        assert!(rs.pending_rewards.is_empty());
        assert!(rs.relic_rewards.is_empty());
        assert_eq!(rs.relics.len(), 1);
        // The granted relic is drained from the bag.
        assert!(!rs.relic_grab_bag.contains(&rs.relics[0]));
    }

    #[test]
    fn unrest_site_rest_heals_to_full_and_adds_poor_sleep() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = fresh_state();
        rs.max_hp = 80;
        rs.current_hp = 40;
        let outcome = (UNREST_SITE.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.current_hp, 80);
        assert_eq!(
            rs.master_deck.last().map(|c| c.card_id),
            Some(CardId::PoorSleep)
        );
    }

    #[test]
    fn vakuu_three_pools_queue_distinct_relics() {
        for (idx, expected) in [(0, VAKUU_RELIC_1), (1, VAKUU_RELIC_2), (2, VAKUU_RELIC_3)] {
            let mut rs = fresh_state();
            let outcome = (VAKUU.options[idx].handler)(&mut rs);
            assert!(outcome.finished);
            assert_eq!(rs.relic_rewards[0].relic_id, expected);
        }
    }

    #[test]
    fn wellspring_bottle_queues_potion_reward() {
        let mut rs = fresh_state();
        let outcome = (WELLSPRING.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.pending_rewards.len(), 1);
        assert!(matches!(rs.pending_rewards[0], Reward::Potion(_)));
    }

    #[test]
    fn zen_weaver_is_allowed_gates_on_125_gold() {
        let mut rs = fresh_state();
        rs.gold = 124;
        assert!(!(ZEN_WEAVER.is_allowed)(&rs));
        rs.gold = 125;
        assert!((ZEN_WEAVER.is_allowed)(&rs));
    }

    #[test]
    fn zen_weaver_breathing_spends_50_and_adds_two_enlightenment() {
        let mut rs = fresh_state();
        rs.gold = 200;
        let deck_before = rs.master_deck.len();
        let outcome = (ZEN_WEAVER.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.gold, 150);
        assert_eq!(rs.master_deck.len(), deck_before + 2);
        assert_eq!(
            rs.master_deck
                .iter()
                .filter(|c| c.card_id == CardId::Enlightenment)
                .count(),
            2
        );
    }

    #[test]
    fn zen_weaver_emotional_spends_125_and_removes_one_card() {
        let mut rs = fresh_state();
        rs.gold = 300;
        seed_deck(&mut rs, 3);
        let deck_before = rs.master_deck.len();
        let outcome = (ZEN_WEAVER.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.gold, 175);
        assert_eq!(rs.master_deck.len(), deck_before - 1);
    }

    #[test]
    fn zen_weaver_acupuncture_spends_250_and_removes_two_cards() {
        let mut rs = fresh_state();
        rs.gold = 500;
        seed_deck(&mut rs, 3);
        let deck_before = rs.master_deck.len();
        let outcome = (ZEN_WEAVER.options[2].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.gold, 250);
        assert_eq!(rs.master_deck.len(), deck_before - 2);
    }

    #[test]
    fn zen_weaver_emotional_no_op_when_unaffordable() {
        let mut rs = fresh_state();
        rs.gold = 100;
        let outcome = (ZEN_WEAVER.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.gold, 100);
    }

    // ── flavor-only stubs (just confirm they return finished) ─────

    #[test]
    fn deferred_stubs_finish_without_panic() {
        let stubs = [
            REFLECTIONS.options[0].handler,
            REFLECTIONS.options[1].handler,
            SAPPHIRE_SEED.options[1].handler,
            SELF_HELP_BOOK.options[3].handler,
            TRIAL.options[0].handler,
            TRIAL.options[1].handler,
            WELLSPRING.options[1].handler,
            WOOD_CARVINGS.options[0].handler,
            WOOD_CARVINGS.options[1].handler,
            WOOD_CARVINGS.options[2].handler,
        ];
        for handler in stubs {
            let mut rs = fresh_state();
            let outcome = handler(&mut rs);
            assert!(outcome.finished);
        }
    }

    // ── TinkerTime multi-page event (bead `sts2-rs-2guyj`) ─────────────

    use crate::cards::colorless::helpers::pack_tinker_time;
    use sts2_core::cards::VarKey;

    /// Decode a MadScience card's packed `(mode_idx, rider)` morph.
    fn decode_morph(card: &sts2_core::cards::Card) -> (i16, TinkerTimeRider) {
        let packed = card.combat_var(VarKey::Generic);
        (
            (packed >> 4) as i16,
            TinkerTimeRider::from_u8((packed & 0xF) as u8),
        )
    }

    /// Drive the full 3-page flow (always picking option 0) and assert it
    /// grants a MadScience whose stored morph is internally consistent
    /// (the chosen rider belongs to the chosen card type) and that the
    /// per-event scratch is cleared on finish.
    #[test]
    fn tinker_time_three_page_flow_grants_consistent_moded_mad_science() {
        let mut rs = fresh_state();
        rs.rng.seed = 777;
        rs.enter_event_room(EventId::TinkerTime);

        // Page 0: a single "choose_card_type" option.
        let p0 = crate::events::dispatch::present(&rs).unwrap();
        assert_eq!(p0.len(), 1);
        assert_eq!(p0[0].option_id, "choose_card_type");

        let out = crate::events::dispatch::resolve(&mut rs, "choose_card_type").unwrap();
        assert!(!out.finished, "intermediate page must NOT finish the event");
        assert!(rs.current_event.is_some(), "event still active mid-flow");
        assert_eq!(rs.event_state.page, 1);

        // Page 1: exactly 2 distinct card-type options drawn from 3.
        let p1 = crate::events::dispatch::present(&rs).unwrap();
        assert_eq!(p1.len(), 2, "TakeRandom(2) of [Attack,Skill,Power]");
        assert_ne!(p1[0].option_id, p1[1].option_id);
        for o in &p1 {
            assert!(matches!(o.option_id, "attack" | "skill" | "power"));
        }
        let type_pick = p1[0].option_id;
        let out = crate::events::dispatch::resolve(&mut rs, type_pick).unwrap();
        assert!(!out.finished);
        assert_eq!(rs.event_state.page, 2);

        // Page 2: exactly 2 distinct rider options for the chosen type.
        let p2 = crate::events::dispatch::present(&rs).unwrap();
        assert_eq!(p2.len(), 2, "TakeRandom(2) of the type's 3 riders");
        assert_ne!(p2[0].option_id, p2[1].option_id);
        let rider_pick = p2[0].option_id;

        let before = rs.master_deck.len();
        let out = crate::events::dispatch::resolve(&mut rs, rider_pick).unwrap();
        assert!(out.finished, "picking a rider finishes the event");
        assert!(rs.current_event.is_none(), "event room exited on grant");
        assert_eq!(rs.master_deck.len(), before + 1, "exactly one card added");

        let card = rs.master_deck.last().unwrap();
        assert_eq!(card.card_id, CardId::MadScience);
        let (mode_idx, rider) = decode_morph(card);
        assert_ne!(rider, TinkerTimeRider::None, "a real rider was attached");
        // The rider must belong to the chosen card type.
        assert_eq!(
            mode_idx,
            tt_type_code(tt_rider_type(rider)),
            "stored mode matches the rider's owning type",
        );

        // Scratch is cleared on exit.
        assert_eq!(rs.event_state.page, 0);
        assert!(rs.event_state.scratch.is_empty());
        assert!(rs.event_state.rng.is_none());
    }

    /// Cross-engine RNG-parity fixture, anchored to the REAL C# engine.
    ///
    /// Driven through the C# headless oracle (`sts2-pycli/src/Sts2Headless`,
    /// the real `sts2.dll` via Harmony) at run seed `"PARITY1"` with
    /// `STS2_TRACE_RNG=1`, the TinkerTime event's own RNG stream (C#
    /// `EventModel.Rng`, `EventModel.cs:193`) has `Seed == 3665743748`, and
    /// the card-type page — the first `[Attack,Skill,Power].TakeRandom(2)` —
    /// offers `[POWER, ATTACK]` in that order. Reproduce:
    /// ```text
    /// printf '%s\n' \
    ///  '{"cmd":"start_run","character":"Ironclad","seed":"PARITY1"}' \
    ///  '{"cmd":"enter_room","type":"event","event":"TINKER_TIME"}' \
    ///  '{"cmd":"action","action":"choose_option","args":{"option_index":0}}' \
    ///  '{"cmd":"quit"}' \
    ///  | STS2_TRACE_RNG=1 dotnet bin/Debug/net9.0/Sts2Headless.dll
    /// ```
    /// This reconstructs the SAME event stream in Rust (deriving the run
    /// master seed that yields C#'s event `Seed`) and asserts Rust produces
    /// the identical event seed AND card-type offer — proving the seed
    /// formula (`seed + NetId + hash("TINKER_TIME")`), the `UnstableShuffle`,
    /// and the source-order `[Attack,Skill,Power]` array all match C#. (This
    /// is what caught the original `"TinkerTime"` vs `"TINKER_TIME"` seed-name
    /// bug.) The rider page + final morph read the same validated stream and
    /// are pinned — the C# headless auto-resolves the rider without surfacing
    /// it (bead `sts2-rs-1hiht`), so they can't be read from the oracle
    /// directly. Bead `sts2-rs-2guyj`.
    ///
    /// IGNORED on v0.107.1 (bead `sts2-rs-nbxaw`): this fixture was captured
    /// from main's v0.103.2 oracle, where the event seed folds in `NetId == 1`
    /// AND the PRNG is `System.Random`. v0.107.1 drops the player term to 0
    /// (`EventModel.cs:238` GetPlayerSlotIndex) and swaps the PRNG to
    /// Xoshiro256** (MegaRandom, commit e797fe1d3), so BOTH the event seed for
    /// a given run AND the draw sequence from a given seed differ — the pinned
    /// `[POWER, ATTACK]` / `[improvement, curious]` offer orders no longer hold.
    /// Re-derive against the v0.107.1 oracle before re-enabling.
    #[test]
    #[ignore = "v0.103.2-oracle fixture (NetId=1 + System.Random); re-derive vs v0.107.1 oracle — sts2-rs-nbxaw"]
    fn tinker_time_rng_parity_vs_csharp_headless() {
        // C# ground truth (Sts2Headless, seed "PARITY1", STS2_TRACE_RNG=1).
        const CSHARP_EVENT_SEED: u32 = 3_665_743_748;

        // Derive the run master seed that reproduces C#'s event Seed. v0.107.1
        // single-player additive player term is 0, so
        // event_seed == master + hash("TINKER_TIME")  (mod 2^32).
        let hash = sts2_core::deterministic_hash_code("TINKER_TIME") as u32;
        let master = CSHARP_EVENT_SEED.wrapping_sub(hash) as i32;

        let mut rs = fresh_state();
        rs.rng.seed = master;
        rs.enter_event_room(EventId::TinkerTime);

        // Page 0 → card-type draw. Rust's event stream MUST seed identically.
        crate::events::dispatch::resolve(&mut rs, "choose_card_type").unwrap();
        assert_eq!(
            rs.event_state.rng.as_ref().unwrap().seed(),
            CSHARP_EVENT_SEED,
            "Rust event RNG stream seeds identically to the C# engine",
        );

        // Card-type offer must match the C#-observed order [POWER, ATTACK].
        let type_ids: Vec<&str> = crate::events::dispatch::present(&rs)
            .unwrap()
            .iter()
            .map(|o| o.option_id)
            .collect();
        assert_eq!(
            type_ids,
            vec!["power", "attack"],
            "matches C# card-type offer",
        );

        // Pick POWER (C# headless auto-picks index 0). Rider page + final
        // morph come off the same validated stream.
        crate::events::dispatch::resolve(&mut rs, "power").unwrap();
        let rider_ids: Vec<&str> = crate::events::dispatch::present(&rs)
            .unwrap()
            .iter()
            .map(|o| o.option_id)
            .collect();
        // Power riders [Expertise,Curious,Improvement].TakeRandom(2) off the
        // continued C# stream (trace draws max=3 r=0, max=2 r=1).
        assert_eq!(rider_ids, vec!["improvement", "curious"]);

        let rider_pick = rider_ids[0].to_string();
        crate::events::dispatch::resolve(&mut rs, &rider_pick).unwrap();
        let card = rs.master_deck.last().unwrap();
        let (mode_idx, rider) = decode_morph(card);
        assert_eq!(mode_idx, 2, "Power"); // Power = mode_idx 2
        assert_eq!(
            card.combat_var(VarKey::Generic),
            pack_tinker_time(CardType::Power, rider),
        );
    }

    /// The two `TakeRandom` draws share ONE event RNG stream whose counter
    /// advances across pages — re-seeding per page would re-draw from the
    /// start. Pins that the rider draw depends on the card-type draw having
    /// already consumed RNG (different seeds → the flow stays deterministic
    /// but the page-2 offer set is a function of the continued stream).
    #[test]
    fn tinker_time_event_rng_persists_across_pages() {
        let mut rs = fresh_state();
        rs.rng.seed = 12345;
        rs.enter_event_room(EventId::TinkerTime);
        crate::events::dispatch::resolve(&mut rs, "choose_card_type").unwrap();
        // After page 0 the stream exists and its counter has advanced past
        // the initial state (3-element shuffle = 2 swaps).
        let rng = rs.event_state.rng.clone().expect("event rng created");
        // v0.107.1 single-player event seed has additive player term 0, so a
        // fresh stream is `Rng::with_name(seed, "TINKER_TIME")` at counter 0.
        let fresh = Rng::with_name(rs.rng.seed, "TINKER_TIME");
        assert_ne!(rng, fresh, "counter advanced by the first TakeRandom draw");
    }

    // ── mutant-coverage pins (sts2-content cargo-mutants survivors) ──

    /// Kills `sown_can_enchant -> true` (line 176) and both `&&` → `||`
    /// swaps (lines 178/179): an enchanted card fails the
    /// `enchantments.is_empty()` clause, a Curse fails the type clause.
    /// Each clause must AND, so the predicate is false in each case —
    /// `true` / `||` mutants would wrongly report eligible.
    #[test]
    fn sown_can_enchant_requires_all_clauses() {
        crate::cards::dispatch::ensure_installed();
        use sts2_core::cards::EnchantmentId;
        // Plain un-enchanted Attack → eligible.
        let strike =
            crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false).unwrap();
        assert!(sown_can_enchant(&strike), "fresh Attack is enchantable");
        // Already enchanted → the `enchantments.is_empty()` clause fails.
        let mut enchanted = strike;
        enchanted.enchantments.push((EnchantmentId::Sown, 1));
        assert!(
            !sown_can_enchant(&enchanted),
            "an already-enchanted card is NOT eligible (&& not ||, not const true)"
        );
        // A Curse → the type clause fails.
        let curse = crate::cards::spec_index::card_pod_from_id(CardId::BadLuck, false).unwrap();
        assert!(
            !sown_can_enchant(&curse),
            "a Curse is NOT eligible (&& not ||, not const true)"
        );
    }

    /// Kills `self_help_book_read_entire -> Default::default()` (line 319):
    /// the handler must actually enchant a Power with Swift +2, not return
    /// an empty outcome. Auto-mode (non-interactive) mutates the deck card.
    #[test]
    fn self_help_book_read_entire_enchants_power_with_swift() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = fresh_state();
        push_card(&mut rs, CardId::Inflame); // an Ironclad Power
        let power_idx = rs
            .master_deck
            .iter()
            .position(|c| c.card_type == sts2_core::cards::CardType::Power)
            .expect("fixture seeds a Power");
        let outcome = self_help_book_read_entire(&mut rs);
        assert!(outcome.finished);
        assert_eq!(
            rs.master_deck[power_idx].enchantments.as_slice(),
            &[(
                sts2_core::cards::EnchantmentId::Swift,
                SELF_HELP_BOOK_ENCHANT_AMOUNT
            )],
            "read_entire enchants the Power (not a default no-op)"
        );
    }

    /// Kills the `sunken_statue_rolled_gold` arithmetic mutants
    /// (line 479: `+`→`-`, `delete -`): the granted gold must equal the
    /// base PLUS the signed `[-10, 10]` roll. Computed against an
    /// INDEPENDENT raw-RNG draw so a mutated handler can't agree with a
    /// mutated expectation.
    #[test]
    fn sunken_statue_rolled_gold_is_base_plus_signed_variance() {
        let rs = fresh_state();
        // v0.107.1 single-player event seed: additive player term is 0 (mirrors
        // `sunken_statue_rolled_gold` — `EventModel.cs:238` GetPlayerSlotIndex == 0).
        let mut raw = sts2_core::dotnet_rng::Rng::with_name(rs.rng.seed, SUNKEN_STATUE_STREAM);
        let variance = raw.next_int(-SUNKEN_STATUE_GOLD_VARIANCE, SUNKEN_STATUE_GOLD_VARIANCE);
        assert_eq!(
            sunken_statue_rolled_gold(&rs),
            SUNKEN_STATUE_DIVE_GOLD + variance,
            "rolled gold = 111 + NextInt(-10, 10) (kills +/- and delete-neg mutants)"
        );
    }

    /// Kills `tablet_of_truth_decipher` `current_hp > max_hp` → `==` / `<`
    /// (line 585): with current_hp seeded AT the pre-decipher max, after the
    /// 3 Max-HP loss it sits ABOVE the new max, so the original clamps it
    /// down; the `==`/`<` mutants leave it stranded above max_hp.
    #[test]
    fn tablet_of_truth_decipher_clamps_current_hp_to_reduced_max() {
        let mut rs = fresh_state();
        rs.max_hp = 50;
        rs.current_hp = 50; // == max, so after -3 it sits ABOVE the new max
        let _ = tablet_of_truth_decipher(&mut rs);
        assert_eq!(rs.max_hp, 47);
        assert_eq!(
            rs.current_hp, 47,
            "current_hp clamps to the reduced max (> comparison, not == / <)"
        );
    }

    /// Kills `unrest_site_kill` `current_hp > max_hp` → `==` / `<`
    /// (line 984): same clamp shape as the tablet.
    #[test]
    fn unrest_site_kill_clamps_current_hp_to_reduced_max() {
        let mut rs = fresh_state();
        rs.max_hp = 60;
        rs.current_hp = 60; // == max, so after -8 it sits ABOVE the new max
        let _ = unrest_site_kill(&mut rs);
        assert_eq!(rs.max_hp, 52);
        assert_eq!(
            rs.current_hp, 52,
            "current_hp clamps to the reduced max (> comparison, not == / <)"
        );
    }

    /// Kills `wellspring_bathe` `pending_deck_removals += 1` → `-=` / `*=`
    /// (line 1108): from a non-zero base the increment must land at base+1,
    /// which `-=` (base-1) and `*=` (base) both miss.
    #[test]
    fn wellspring_bathe_interactive_increments_pending_removals() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = fresh_state();
        rs.interactive_card_removal = true;
        rs.pending_deck_removals = 2;
        let outcome = wellspring_bathe(&mut rs);
        assert!(outcome.finished);
        assert_eq!(
            rs.pending_deck_removals, 3,
            "bathe adds exactly one removal request (+=, not -= / *=)"
        );
        assert_eq!(
            rs.pending_post_removal_cards.last().map(|c| c.card_id),
            Some(CardId::Guilty),
            "the deferred Guilty curse is queued post-removal"
        );
    }

    /// Kills `wood_carvings_snake` auto-mode `&&` → `||` swaps (lines
    /// 1210/1211). Deck front is an ALREADY-ENCHANTED, cost-positive Attack
    /// — eligible on the type & cost clauses but NOT on `is_empty()`. The
    /// original `type && is_empty && cost>=0` is false there, so the Slither
    /// skips to the fresh Strike behind it. Either `&&`→`||` swap makes the
    /// front card match first (giving it a second stack), which both
    /// assertions catch. (A Curse fixture can't kill these — its negative
    /// cost masks the `||` flip.)
    #[test]
    fn wood_carvings_snake_auto_skips_enchanted_card_enchants_fresh() {
        crate::cards::dispatch::ensure_installed();
        use sts2_core::cards::EnchantmentId;
        let mut rs = fresh_state();
        // slot 0: an Attack already carrying Sown — type ok, cost ok,
        // is_empty() FALSE.
        let mut enchanted =
            crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false).unwrap();
        enchanted.enchantments.push((EnchantmentId::Sown, 1));
        rs.master_deck.push(enchanted);
        seed_deck(&mut rs, 1); // slot 1: fresh Strike, fully eligible
        wood_carvings_snake(&mut rs);
        assert_eq!(
            rs.master_deck[0].enchantments.as_slice(),
            &[(EnchantmentId::Sown, 1)],
            "the already-enchanted front card is skipped (all && clauses hold; no || )"
        );
        assert_eq!(
            rs.master_deck[1].enchantments.as_slice(),
            &[(EnchantmentId::Slither, 1)],
            "the fresh Strike behind it gains Slither"
        );
    }

    /// Kills `zen_weaver_emotional` affordability `<` → `<=` (line 1280):
    /// at EXACTLY the cost the player can afford, so the spend happens.
    /// `<=` would refuse at the exact price, leaving gold untouched.
    #[test]
    fn zen_weaver_emotional_affords_at_exact_cost() {
        let mut rs = fresh_state();
        rs.gold = ZEN_WEAVER_EMOTIONAL_COST; // exactly 125
        let outcome = zen_weaver_emotional(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.gold, 0, "exact-cost gold is spent (< guard, not <=)");
    }

    /// Kills `zen_weaver_acupuncture` affordability `<` → `==` / `<=`
    /// (line 1290): at EXACTLY the cost the spend happens. `<=` refuses at
    /// the exact price; `==` takes the can't-afford branch when equal —
    /// both leave gold untouched here.
    #[test]
    fn zen_weaver_acupuncture_affords_at_exact_cost() {
        let mut rs = fresh_state();
        rs.gold = ZEN_WEAVER_ACUPUNCTURE_COST; // exactly 250
        let outcome = zen_weaver_acupuncture(&mut rs);
        assert!(outcome.finished);
        assert_eq!(
            rs.gold, 0,
            "exact-cost gold is spent (< guard, not == / <=)"
        );
    }

    #[test]
    fn all_wave_b_specs_python_names_match_event_ids() {
        let specs = [
            (&REFLECTIONS, "Reflections"),
            (&ROUND_TEA_PARTY, "RoundTeaParty"),
            (&SAPPHIRE_SEED, "SapphireSeed"),
            (&SELF_HELP_BOOK, "SelfHelpBook"),
            (&SPIRIT_GRAFTER, "SpiritGrafter"),
            (&SUNKEN_STATUE, "SunkenStatue"),
            (&SUNKEN_TREASURY, "SunkenTreasury"),
            (&TABLET_OF_TRUTH, "TabletOfTruth"),
            (&TANX, "Tanx"),
            (&TEZCATARA, "Tezcatara"),
            (&THE_LANTERN_KEY, "TheLanternKey"),
            (&THIS_OR_THAT, "ThisOrThat"),
            (&TINKER_TIME, "TinkerTime"),
            (&TRIAL, "Trial"),
            (&UNREST_SITE, "UnrestSite"),
            (&VAKUU, "Vakuu"),
            (&WELLSPRING, "Wellspring"),
            (&WOOD_CARVINGS, "WoodCarvings"),
            (&ZEN_WEAVER, "ZenWeaver"),
        ];
        for (spec, expected_name) in specs {
            assert_eq!(spec.python_name, expected_name);
            assert_eq!(spec.id.python_name(), expected_name);
            assert!(!spec.options.is_empty(), "{expected_name} has no options");
        }
    }
}
