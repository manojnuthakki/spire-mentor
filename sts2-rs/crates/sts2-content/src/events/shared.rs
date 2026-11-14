//! Shared-pool events. Bead `sts2-rs-6ynt.4.1` ships two
//! proof-of-shape entries (`LostWisp`, `TrashHeap`); the remaining
//! 38 shared events land via sub-beads `6ynt.4.4` / `6ynt.4.5`.
//!
//! Port pin:
//! `sts2-rl-agent@01c094d:sts2_env/events/shared.py`.

use sts2_core::events::{BattlewornDummySetting, EventCombatRequest, EventEncounter};
use sts2_core::{
    CardReward, EventId, EventOptionInfo, EventOutcome, EventVarSpec, PotionReward, RelicId,
    RelicReward, Reward, RunState,
};

use super::{
    always_allowed, amalgamator_allowed, byrdonis_nest_allowed, default_option_info_from_spec,
    option_title_vars, option_vars, punch_off_allowed, two_page_option_info, EventOptionSpec,
    EventSpec,
};

// ── LostWisp ─────────────────────────────────────────────────────────

/// Default `LostWisp` gold-search payout. Python's
/// `LostWisp.calculate_vars` randomizes this within ±15 of 60
/// (`sts2-rl-agent@01c094d:sts2_env/events/shared.py:1066`); this
/// bead pins the deterministic midpoint until a `RunState.rng`
/// surface lands. Documented divergence — sub-bead `6ynt.4.rng`
/// will swap in `rng.up_front.next_int(-15, 16)`.
pub const LOST_WISP_GOLD: i32 = 60;

fn lost_wisp_generate_options(_state: &RunState, _spec: &EventSpec) -> Vec<EventOptionInfo> {
    vec![
        EventOptionInfo {
            option_id: "claim",
            loc_key: "LOST_WISP.pages.INITIAL.options.CLAIM",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: "Claim".to_string(),
            description: "Gain Lost Wisp relic + Decay curse.".to_string(),
            enabled: true,
        },
        EventOptionInfo {
            option_id: "search",
            loc_key: "LOST_WISP.pages.INITIAL.options.SEARCH",
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: "Search".to_string(),
            description: format!("Gain {LOST_WISP_GOLD} gold."),
            enabled: true,
        },
    ]
}

/// `LostWisp.cs::Claim`: add Decay through `CardPileCmd.AddCursesToDeck`
/// before obtaining Lost Wisp and finishing. The existing deck-add path runs
/// the modification and after-add hooks; relic acquisition stays on the
/// existing reward continuation (direct grants drain it in the same action).
fn lost_wisp_claim(state: &mut RunState) -> EventOutcome {
    let decay = crate::cards::spec_index::card_pod_from_id(sts2_core::cards::CardId::Decay, false)
        .expect("Decay has a card spec");
    state.add_card_to_deck(decay, sts2_core::relic_effects::AddSource::EventGain);
    state.offer_relic_rewards(RelicId::LostWisp, 1);
    EventOutcome::finished("Added Decay and gained Lost Wisp relic.")
}

fn lost_wisp_search(state: &mut RunState) -> EventOutcome {
    state.gain_gold(LOST_WISP_GOLD);
    EventOutcome::finished(format!("Gained {LOST_WISP_GOLD} gold."))
}

const LOST_WISP_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "claim",
        loc_key: "LOST_WISP.pages.INITIAL.options.CLAIM",
        default_label: "Claim",
        default_description: "Gain Lost Wisp relic + Decay curse.",
        default_vars: &[
            ("Curse", EventVarSpec::Text("Decay")),
            ("Relic", EventVarSpec::Text("Lost Wisp")),
        ],
        vars_fn: None,
        title_vars_fn: None,
        handler: lost_wisp_claim,
    },
    EventOptionSpec {
        option_id: "search",
        loc_key: "LOST_WISP.pages.INITIAL.options.SEARCH",
        default_label: "Search",
        default_description: "Gain ~60 gold.",
        default_vars: &[("Gold", EventVarSpec::Int(LOST_WISP_GOLD as i64))],
        vars_fn: None,
        title_vars_fn: None,
        handler: lost_wisp_search,
    },
];

/// `LostWisp` static spec. Port pin:
/// `sts2-rl-agent@01c094d:sts2_env/events/shared.py:1053`.
pub const LOST_WISP: EventSpec = EventSpec {
    id: EventId::LostWisp,
    python_name: "LostWisp",
    is_allowed: always_allowed,
    generate_options: lost_wisp_generate_options,
    options: LOST_WISP_OPTIONS,
};

// ── TrashHeap ────────────────────────────────────────────────────────

/// Build an event's own non-shared RNG stream. v0.107.1 `EventModel` seeds
/// `base.Rng = new Rng(RunState.Rng.Seed + (!IsShared ? GetPlayerSlotIndex(Owner) : 0)
/// + GetDeterministicHashCode(Id.Entry))` (`EventModel.cs:238`). In single-player
/// `GetPlayerSlotIndex(Owner) == Players.IndexOf(Owner) == 0` (`RunState.cs:620`),
/// so the additive player term is **0** — v0.103.2 used `Owner.NetId == 1` here
/// (`EventModel.cs:193`); the v0.107.1 change from `NetId` to `GetPlayerSlotIndex`
/// drops it. [`Rng::with_name`] folds in `deterministic_hash_code(entry)`, matching
/// the C# hash term exactly. `entry` is the event's `Id.Entry` (its L10N key root,
/// e.g. `"TRASH_HEAP"`).
fn event_rng(state: &RunState, entry: &str) -> sts2_core::dotnet_rng::Rng {
    sts2_core::dotnet_rng::Rng::with_name(state.rng.seed, entry)
}

/// `TrashHeap`'s `Id.Entry` (L10N key root) — names its own event stream.
const TRASH_HEAP_STREAM: &str = "TRASH_HEAP";

/// DiveIn's fixed 5-relic `base.Rng.NextItem(Relics)` pool, in C# source
/// order (`TrashHeap.cs:19-26`).
const TRASH_HEAP_RELICS: [RelicId; 5] = [
    RelicId::DarkstonePeriapt,
    RelicId::DreamCatcher,
    RelicId::HandDrill,
    RelicId::MawBank,
    RelicId::TheBoot,
];

/// Grab's fixed 10-card `base.Rng.NextItem(Cards)` pool, in C# source
/// order (`TrashHeap.cs:28-40`). `HelloWorld` is the `HelloWorldCard`
/// CardId variant.
const TRASH_HEAP_CARDS: [sts2_core::cards::CardId; 10] = [
    sts2_core::cards::CardId::Caltrops,
    sts2_core::cards::CardId::Clash,
    sts2_core::cards::CardId::Distraction,
    sts2_core::cards::CardId::DualWield,
    sts2_core::cards::CardId::Entrench,
    sts2_core::cards::CardId::HelloWorldCard,
    sts2_core::cards::CardId::Outmaneuver,
    sts2_core::cards::CardId::Rebound,
    sts2_core::cards::CardId::RipAndTear,
    sts2_core::cards::CardId::Stack,
];

/// `TrashHeap` precondition: `current_hp > 5`. Port pin:
/// `sts2-rl-agent@01c094d:sts2_env/events/shared.py:2036`.
fn trash_heap_allowed(state: &RunState) -> bool {
    state.current_hp > 5
}

fn trash_heap_dive_in(state: &mut RunState) -> EventOutcome {
    let lost = state.lose_hp(8);
    // C# DiveIn (`TrashHeap.cs:62-68`): `RelicCmd.Obtain(base.Rng.NextItem(
    // Relics))` — a SPECIFIC relic from the fixed 5-pool off the event's
    // own stream, granted DIRECTLY (not a rarity-rolled relic *reward*).
    // The preceding `CreatureCmd.Damage` consumes no event RNG, so the
    // `NextItem` is read #1 of the stream (skip 0). The old code pushed a
    // `Reward::Relic(LOST_WISP)` placeholder that the drain never granted
    // (Reward::Relic isn't a drain variant) — wrong relic *and* a phantom
    // relic-reward screen. Bead `sts2-rs-RP28`.
    let mut rng = event_rng(state, TRASH_HEAP_STREAM);
    let idx = rng.next_int_exclusive(0, TRASH_HEAP_RELICS.len() as i32) as usize;
    state.obtain_relic(TRASH_HEAP_RELICS[idx]);
    EventOutcome::finished(format!("Took {lost} damage, gained a relic."))
}

fn trash_heap_grab(state: &mut RunState) -> EventOutcome {
    state.gain_gold(100);
    // C# Grab (`TrashHeap.cs:70-76`): `CardPileCmd.Add(CreateCard(
    // base.Rng.NextItem(Cards)), Deck)` — a SPECIFIC card from the fixed
    // 10-pool off the event stream, added DIRECTLY. `GainGold` consumes no
    // event RNG ⇒ the `NextItem` is skip 0. The old `Reward::Card(empty)`
    // push surfaced a random 3-card reward pick (extra step + wrong cards +
    // deck-size/phase desync). Bead `sts2-rs-RP28`.
    let mut rng = event_rng(state, TRASH_HEAP_STREAM);
    let idx = rng.next_int_exclusive(0, TRASH_HEAP_CARDS.len() as i32) as usize;
    if let Some(card) = crate::cards::spec_index::card_pod_from_id(TRASH_HEAP_CARDS[idx], false) {
        state.add_card_to_deck(card, sts2_core::relic_effects::AddSource::EventGain);
    }
    EventOutcome::finished("Gained 100 gold and a card.")
}

const TRASH_HEAP_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "dive_in",
        loc_key: "TRASH_HEAP.pages.INITIAL.options.DIVE_IN",
        default_label: "Dive In",
        default_description: "Take 8 damage, gain a relic.",
        default_vars: &[("HpLoss", EventVarSpec::Int(8))],
        vars_fn: None,
        title_vars_fn: None,
        handler: trash_heap_dive_in,
    },
    EventOptionSpec {
        option_id: "grab",
        loc_key: "TRASH_HEAP.pages.INITIAL.options.GRAB",
        default_label: "Grab",
        default_description: "Gain 100 gold + a card.",
        default_vars: &[("Gold", EventVarSpec::Int(100))],
        vars_fn: None,
        title_vars_fn: None,
        handler: trash_heap_grab,
    },
];

/// `TrashHeap` static spec. Port pin:
/// `sts2-rl-agent@01c094d:sts2_env/events/shared.py:2029`.
pub const TRASH_HEAP: EventSpec = EventSpec {
    id: EventId::TrashHeap,
    python_name: "TrashHeap",
    is_allowed: trash_heap_allowed,
    generate_options: default_option_info_from_spec,
    options: TRASH_HEAP_OPTIONS,
};

// ════════════════════════════════════════════════════════════════════
// Wave A — bead `sts2-rs-6ynt.4.4` (19 events, A-N order from
// shared.py at pin `01c094d`). Single-page handlers only; multi-page
// `next_options` chains + deferred reward variants (TransformCards /
// UpgradeCards / RemoveCard / AddCards / Potion) are pinned in the
// per-event doc-comments and EventId variants in
// `sts2-core/src/events.rs`.
// ════════════════════════════════════════════════════════════════════

// ── AbyssalBaths ─────────────────────────────────────────────────────

pub const ABYSSAL_BATHS_MAX_HP_GAIN: i32 = 2;
pub const ABYSSAL_BATHS_INITIAL_DAMAGE: i32 = 3;
pub const ABYSSAL_BATHS_ABSTAIN_HEAL: i32 = 10;

/// C# `Damage` var: 3, raised by 1 after each soak (`OnImmerse`).
/// `event_state.page` counts the soaks (0 on the initial page).
fn abyssal_baths_damage(state: &RunState) -> i32 {
    ABYSSAL_BATHS_INITIAL_DAMAGE + i32::from(state.event_state.page)
}

/// C# `OnImmerse`: gain 2 max HP, take the Damage var, raise it by 1; then
/// `SetEventState` with Linger and Exit Baths (sts2-agent fork: the engine
/// finished the event here).
fn abyssal_baths_soak(state: &mut RunState) -> EventOutcome {
    state.gain_max_hp(ABYSSAL_BATHS_MAX_HP_GAIN);
    let dmg = state.lose_hp(abyssal_baths_damage(state));
    state.event_state.page = state.event_state.page.saturating_add(1);
    EventOutcome {
        finished: false,
        description: format!("+{ABYSSAL_BATHS_MAX_HP_GAIN} Max HP, took {dmg} damage."),
        effects: Vec::new(),
    }
}

fn abyssal_baths_immerse(state: &mut RunState) -> EventOutcome {
    abyssal_baths_soak(state)
}

fn abyssal_baths_linger(state: &mut RunState) -> EventOutcome {
    abyssal_baths_soak(state)
}

fn abyssal_baths_exit(_state: &mut RunState) -> EventOutcome {
    EventOutcome::finished("Left the baths.")
}

fn abyssal_baths_abstain(state: &mut RunState) -> EventOutcome {
    let healed = state.heal(ABYSSAL_BATHS_ABSTAIN_HEAL);
    EventOutcome::finished(format!("Healed {healed} HP."))
}

fn abyssal_baths_vars(state: &RunState) -> Vec<(&'static str, sts2_core::EventVar)> {
    vec![("Damage", abyssal_baths_damage(state).into())]
}

/// Page 0: Immerse and Abstain; after a soak: Linger and Exit Baths.
fn abyssal_baths_generate_options(state: &RunState, spec: &EventSpec) -> Vec<EventOptionInfo> {
    let page = if state.event_state.page == 0 {
        &spec.options[..2]
    } else {
        &spec.options[2..]
    };
    page.iter()
        .map(|opt| EventOptionInfo {
            option_id: opt.option_id,
            label: opt.default_label.to_string(),
            description: opt.default_description.to_string(),
            enabled: true,
            loc_key: opt.loc_key,
            vars: option_vars(state, opt),
            title_vars: option_title_vars(state, opt),
        })
        .collect()
}

const ABYSSAL_BATHS_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "immerse",
        loc_key: "ABYSSAL_BATHS.pages.INITIAL.options.IMMERSE",
        default_label: "Immerse",
        default_description: "+2 Max HP, take 3 damage",
        default_vars: &[
            ("MaxHp", EventVarSpec::Int(ABYSSAL_BATHS_MAX_HP_GAIN as i64)),
            (
                "Damage",
                EventVarSpec::Int(ABYSSAL_BATHS_INITIAL_DAMAGE as i64),
            ),
        ],
        vars_fn: None,
        title_vars_fn: None,
        handler: abyssal_baths_immerse,
    },
    EventOptionSpec {
        option_id: "abstain",
        loc_key: "ABYSSAL_BATHS.pages.INITIAL.options.ABSTAIN",
        default_label: "Abstain",
        default_description: "Heal 10 HP",
        default_vars: &[("Heal", EventVarSpec::Int(ABYSSAL_BATHS_ABSTAIN_HEAL as i64))],
        vars_fn: None,
        title_vars_fn: None,
        handler: abyssal_baths_abstain,
    },
    EventOptionSpec {
        option_id: "linger",
        loc_key: "ABYSSAL_BATHS.pages.ALL.options.LINGER",
        default_label: "Linger",
        default_description: "+2 Max HP, take more damage",
        default_vars: &[("MaxHp", EventVarSpec::Int(ABYSSAL_BATHS_MAX_HP_GAIN as i64))],
        vars_fn: Some(abyssal_baths_vars),
        title_vars_fn: None,
        handler: abyssal_baths_linger,
    },
    EventOptionSpec {
        option_id: "exit_baths",
        loc_key: "ABYSSAL_BATHS.pages.ALL.options.EXIT_BATHS",
        default_label: "Exit Baths",
        default_description: "Leave.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: abyssal_baths_exit,
    },
];

/// `AbyssalBaths` spec. Pin: `shared.py:298`.
pub const ABYSSAL_BATHS: EventSpec = EventSpec {
    id: EventId::AbyssalBaths,
    python_name: "AbyssalBaths",
    is_allowed: always_allowed,
    generate_options: abyssal_baths_generate_options,
    options: ABYSSAL_BATHS_OPTIONS,
};

// ── Amalgamator ──────────────────────────────────────────────────────

/// Remove up to `count` Basic, removable, `tag`-tagged cards from the deck
/// — C# `Amalgamator.IsValid` (`Amalgamator.cs:69-80`): `Tags.Contains(tag)
/// && Rarity == Basic && IsRemovable`. Returns how many were removed. C#
/// has the player pick via `CardSelectCmd.FromDeckForRemoval`: upgrades and
/// enchantments distinguish copies. Reuse the existing removal queue with
/// its offer restricted by `IsValid`; zero/fewer/exact-count candidates
/// resolve immediately. Noninteractive mode retains filtered first matches.
fn amalgamator_remove_basic_tagged(
    state: &mut RunState,
    tag: sts2_core::cards::CardTag,
    count: usize,
) -> usize {
    if state.interactive_card_removal {
        state.pending_basic_removal_tag = Some(tag);
        let removed = state.remove_cards_from_deck(count);
        if state.pending_deck_removals == 0 {
            state.pending_basic_removal_tag = None;
        }
        return removed;
    }
    let rank = sts2_core::lookup_card_rarity_rank();
    let removable = sts2_core::lookup_card_removable();
    let is_basic = |id| rank.map_or(true, |f| f(id) == 1);
    let is_removable = |c: &sts2_core::Card| removable.map_or(true, |f| f(c));
    let mut removed = 0;
    let mut i = 0;
    while removed < count && i < state.master_deck.len() {
        let c = &state.master_deck[i];
        if c.has_tag(tag) && is_basic(c.card_id) && is_removable(c) {
            state.master_deck.remove(i);
            removed += 1;
        } else {
            i += 1;
        }
    }
    removed
}

// C# Amalgamator (`Amalgamator.cs:41-67`): remove 2 player-selected Basic
// removable Strikes/Defends (`FromDeckForRemoval`, no RNG), THEN
// `CreateCard<UltimateStrike/Defend>` + `Add(Deck)`. The old
// `Reward::Card(empty)` push surfaced an unrelated random 3-card reward
// pick and never removed the Strikes (wrong deck + extra step). Bead
// `sts2-rs-RP28`.
fn amalgamator_combine_strikes(state: &mut RunState) -> EventOutcome {
    amalgamator_remove_basic_tagged(state, sts2_core::cards::CardTag::Strike, 2);
    amalgamator_add_after_removal(state, sts2_core::cards::CardId::UltimateStrike);
    EventOutcome::finished("Removed 2 Strikes, added Ultimate Strike.")
}

fn amalgamator_combine_defends(state: &mut RunState) -> EventOutcome {
    amalgamator_remove_basic_tagged(state, sts2_core::cards::CardTag::Defend, 2);
    amalgamator_add_after_removal(state, sts2_core::cards::CardId::UltimateDefend);
    EventOutcome::finished("Removed 2 Defends, added Ultimate Defend.")
}

fn amalgamator_add_after_removal(state: &mut RunState, id: sts2_core::cards::CardId) {
    if let Some(card) = crate::cards::spec_index::card_pod_from_id(id, false) {
        if state.pending_deck_removals > 0 {
            // As in Wellspring: CardPileCmd.Add and its hooks run only after
            // the selected instances have left the deck (including Fysh gold).
            state.pending_post_removal_cards.push(card);
        } else {
            state.add_card_to_deck(card, sts2_core::relic_effects::AddSource::EventGain);
        }
    }
}

const AMALGAMATOR_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "combine_strikes",
        loc_key: "AMALGAMATOR.pages.INITIAL.options.COMBINE_STRIKES",
        default_label: "Combine Strikes",
        default_description: "Remove 2 Strikes, gain Ultimate Strike",
        default_vars: &[("Card1", EventVarSpec::Text("Ultimate Strike"))],
        vars_fn: None,
        title_vars_fn: None,
        handler: amalgamator_combine_strikes,
    },
    EventOptionSpec {
        option_id: "combine_defends",
        loc_key: "AMALGAMATOR.pages.INITIAL.options.COMBINE_DEFENDS",
        default_label: "Combine Defends",
        default_description: "Remove 2 Defends, gain Ultimate Defend",
        default_vars: &[("Card2", EventVarSpec::Text("Ultimate Defend"))],
        vars_fn: None,
        title_vars_fn: None,
        handler: amalgamator_combine_defends,
    },
];

/// `Amalgamator` spec. Pin: `shared.py:357`. `is_allowed` deck-count
/// check deferred (no run-side deck on RunState yet); defaults to
/// always-allowed.
pub const AMALGAMATOR: EventSpec = EventSpec {
    id: EventId::Amalgamator,
    python_name: "Amalgamator",
    is_allowed: amalgamator_allowed,
    generate_options: default_option_info_from_spec,
    options: AMALGAMATOR_OPTIONS,
};

// ── AromaOfChaos ─────────────────────────────────────────────────────

/// `Id.Entry` for the event's own RNG stream (non-shared ⇒ seed adds
/// the single-player NetId, same shape as WhisperingHollow).
pub const AROMA_OF_CHAOS_STREAM: &str = "AROMA_OF_CHAOS";

fn aroma_let_go(state: &mut RunState) -> EventOutcome {
    // C# LetGo: `CardSelectCmd.FromDeckForTransformation` pick-1 →
    // `CardCmd.TransformToRandom(card, base.Rng)`. CanonicalVars is
    // empty (no CalculateVars roll), so the transform is read #1 of
    // the event's own stream — skip 0.
    if state.interactive_card_transform {
        state
            .pending_deck_transforms
            .push(sts2_core::PendingDeckTransform {
                event_stream_roll: Some((AROMA_OF_CHAOS_STREAM, 0)),
                ..Default::default()
            });
        EventOutcome::finished("Pick a card to transform.")
    } else {
        // Auto mode: transform the first deck card off the same stream.
        if let Some(original) = state.master_deck.first().map(|c| c.card_id) {
            if let Some(new_id) = crate::cards::spec_index::roll_transform_replacement_event_stream(
                state,
                state.character_id,
                original,
                AROMA_OF_CHAOS_STREAM,
                0,
            ) {
                if let Some(card) = crate::cards::spec_index::card_pod_from_id(new_id, false) {
                    state.master_deck.remove(0);
                    state.add_transformed_card_to_deck(card);
                }
            }
        }
        EventOutcome::finished("Transformed a card.")
    }
}

fn aroma_maintain_control(state: &mut RunState) -> EventOutcome {
    // C# MaintainControl: `CardSelectCmd.FromDeckForUpgrade` pick-1 →
    // `CardCmd.Upgrade`. Route through `upgrade_random_cards`, which under the
    // harness's interactive mode queues a `pending_deck_upgrades` card_select
    // (matching C#'s player pick, no RNG) and in parity mode auto-upgrades off
    // the rewards stream — the SAME surface the sibling FromDeckForUpgrade
    // events use (SpiritGrafter / Trial). The earlier `offer_upgrade_cards_reward`
    // queued a `Reward::UpgradeCards` the run loop never surfaces as a
    // card_select, so Rust skipped the pick and advanced straight to map
    // (whole-run seed PARITY15: cs='card_select' rs='map'). Bead
    // `sts2-rs-upgrade-event-select`.
    let _ = state.upgrade_random_cards(1);
    EventOutcome::finished("Pick a card to upgrade.")
}

const AROMA_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "let_go",
        loc_key: "AROMA_OF_CHAOS.pages.INITIAL.options.LET_GO",
        default_label: "Let Go",
        default_description: "Transform 1 card",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: aroma_let_go,
    },
    EventOptionSpec {
        option_id: "maintain_control",
        loc_key: "AROMA_OF_CHAOS.pages.INITIAL.options.MAINTAIN_CONTROL",
        default_label: "Maintain Control",
        default_description: "Upgrade 1 card",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: aroma_maintain_control,
    },
];

/// `AromaOfChaos` spec. Pin: `shared.py:449`. Audit doc 04-events.md
/// erroneously lists `act2.py` — actual source is `shared.py`.
pub const AROMA_OF_CHAOS: EventSpec = EventSpec {
    id: EventId::AromaOfChaos,
    python_name: "AromaOfChaos",
    is_allowed: always_allowed,
    generate_options: default_option_info_from_spec,
    options: AROMA_OPTIONS,
};

// ── BattlewornDummy ──────────────────────────────────────────────────

/// Stage the BattlewornDummy event→combat fight (bead `sts2-rs-ck9z6`,
/// GH #166). Each setting enters a real `Phase::Combat` against the
/// matching `BattleFriend` training dummy and resumes the event after the
/// fight; the per-setting reward is granted by [`battleworn_dummy_resume`]
/// — but ONLY on a clean victory, since the dummy's
/// `BattlewornDummyTimeLimit` can run out (C# `RanOutOfTime` → no reward).
///
/// Returns a NOT-finished outcome so the dispatcher leaves the event room
/// open across the combat; `RunManager::finish_event_combat` exits it on
/// victory. Mirrors C# `BattlewornDummy.Settings` →
/// `EnterCombatWithoutExitingEvent<BattlewornDummyEventEncounter>
/// (shouldResumeAfterCombat: true)`.
fn battleworn_enter_fight(state: &mut RunState, setting: BattlewornDummySetting) -> EventOutcome {
    state.event_combat = Some(EventCombatRequest::resuming(
        EventEncounter::BattlewornDummy(setting),
        EventId::BattlewornDummy,
    ));
    EventOutcome {
        finished: false,
        description: "The training dummy powers on. Fight!".to_string(),
        effects: Vec::new(),
    }
}

fn battleworn_setting_1(state: &mut RunState) -> EventOutcome {
    battleworn_enter_fight(state, BattlewornDummySetting::Setting1)
}

/// `BattlewornDummy`'s `Id.Entry` (L10N key root) — names its own event stream.
const BATTLEWORN_DUMMY_STREAM: &str = "BATTLEWORN_DUMMY";

fn battleworn_setting_2(state: &mut RunState) -> EventOutcome {
    battleworn_enter_fight(state, BattlewornDummySetting::Setting2)
}

fn battleworn_setting_3(state: &mut RunState) -> EventOutcome {
    battleworn_enter_fight(state, BattlewornDummySetting::Setting3)
}

/// Resume the BattlewornDummy event after its nested combat (C#
/// `BattlewornDummy.Resume`). `clean_victory` is `false` when the dummy
/// ran out of time (`RanOutOfTime`) — then NO reward is granted, matching
/// the C# `DEFEAT` branch. On a clean victory each setting grants the
/// reward it previously modeled at instant-victory time:
/// - Setting1: one potion rolled via `PlayerRng.Rewards.NextItem` over the
///   full character++shared pool (one int read, rolled here, pre-populated).
/// - Setting2: `Where(IsUpgradable).StableShuffle(Rng.Niche).Take(2)` →
///   `CardCmd.Upgrade` (immediate, no pick).
/// - Setting3: `RelicCmd.Obtain(RelicFactory.PullNextRelicFromFront)` —
///   roll the rarity on the rewards stream, then obtain the next front
///   relic of that rarity.
///
/// **Approximation (Setting3):** C# `PullFromFront` filters `_ => true`
/// (no owned-skip), whereas [`RunState::pop_relic_from_grab_bag`] skips
/// already-owned relics — immaterial here (only BurningBlood is owned and
/// it is not in the common bucket). Bead `sts2-rs-ck9z6`.
pub fn battleworn_dummy_resume(
    state: &mut RunState,
    setting: BattlewornDummySetting,
    clean_victory: bool,
) {
    if !clean_victory {
        // RanOutOfTime → the dummy fled; no reward.
        return;
    }
    match setting {
        BattlewornDummySetting::Setting1 => {
            // C# `BattlewornDummy.Resume` Setting1: ONE `PlayerRng.Rewards.NextItem`
            // over the FULL character++shared potion pool, handed to
            // `OfferCustom([new PotionReward(potion.ToMutable(), owner)])`. The
            // reward arrives pre-populated, so `PotionReward.Populate` never rolls
            // the rarity float: an empty sentinel (float + rarity item) left the
            // Rewards cursor one read ahead and picked another potion
            // (`potion-reward-model-differs`). Same shape as TheLegendsWereTrue.
            if let Some(kind) = crate::potions::all::roll_event_potion_full_pool(
                &mut state.rng.rewards,
                state.character_id,
            ) {
                if state.interactive_rewards {
                    state.offer_custom_rewards(
                        vec![sts2_core::OfferedReward::Potion(Some(kind))],
                        true,
                    );
                } else {
                    state
                        .pending_rewards
                        .push(Reward::Potion(PotionReward::with_kind(kind)));
                }
            }
        }
        BattlewornDummySetting::Setting2 => {
            // C# `BattlewornDummy.cs:97` (v0.107.1):
            // `Where(IsUpgradable).StableShuffle(base.Rng).Take(2)` →
            // `CardCmd.Upgrade` — random, no player pick, upgraded immediately.
            // v0.107.1 migrated the shuffle stream from `Rng.Niche` (v0.103.2)
            // to the event's own `base.Rng`; this is the FIRST (and only)
            // `base.Rng` read of the event, so a fresh `event_rng` at cursor 0
            // matches whether rolled inline or on Resume.
            let mut rng = event_rng(state, BATTLEWORN_DUMMY_STREAM);
            let _ = state.upgrade_random_cards_stable_with_rng(2, &mut rng);
        }
        BattlewornDummySetting::Setting3 => {
            let rarity = state.roll_reward_relic_rarity();
            if let Some(id) = state.pop_relic_from_grab_bag(rarity) {
                state.grant_event_relic(id);
            }
        }
    }
}

/// The HP the setting's dummy will have, for the option that previews it.
///
/// C# `BattlewornDummy.GenerateInitialOptions` binds `Setting{1,2,3}Hp` to
/// `ScaleHpForMultiplayer(ModelDb.Monster<BattleFriendV{1,2,3}>()
/// .MinInitialHp, …, playerCount, actIndex)`, and
/// [`sts2_core::monsters::MonsterKind::min_initial_hp`] is that inner read —
/// the scaling wrapper is identity at one player, which the sim always is.
fn battleworn_dummy_hp(state: &RunState, setting: BattlewornDummySetting) -> i32 {
    use sts2_core::monsters::MonsterKind;
    let kind = match setting {
        BattlewornDummySetting::Setting1 => MonsterKind::BattleFriendV1,
        BattlewornDummySetting::Setting2 => MonsterKind::BattleFriendV2,
        BattlewornDummySetting::Setting3 => MonsterKind::BattleFriendV3,
    };
    kind.min_initial_hp(state.ascension_level)
}

const BATTLEWORN_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "setting_1",
        loc_key: "BATTLEWORN_DUMMY.pages.INITIAL.options.SETTING_1",
        default_label: "Setting 1 (Easy)",
        default_description: "Win: gain a potion",
        default_vars: &[],
        vars_fn: Some(|s| {
            vec![(
                "Setting1Hp",
                battleworn_dummy_hp(s, BattlewornDummySetting::Setting1).into(),
            )]
        }),
        title_vars_fn: None,
        handler: battleworn_setting_1,
    },
    EventOptionSpec {
        option_id: "setting_2",
        loc_key: "BATTLEWORN_DUMMY.pages.INITIAL.options.SETTING_2",
        default_label: "Setting 2 (Medium)",
        default_description: "Win: upgrade 2 cards",
        default_vars: &[],
        vars_fn: Some(|s| {
            vec![(
                "Setting2Hp",
                battleworn_dummy_hp(s, BattlewornDummySetting::Setting2).into(),
            )]
        }),
        title_vars_fn: None,
        handler: battleworn_setting_2,
    },
    EventOptionSpec {
        option_id: "setting_3",
        loc_key: "BATTLEWORN_DUMMY.pages.INITIAL.options.SETTING_3",
        default_label: "Setting 3 (Hard)",
        default_description: "Win: gain a relic",
        default_vars: &[],
        vars_fn: Some(|s| {
            vec![(
                "Setting3Hp",
                battleworn_dummy_hp(s, BattlewornDummySetting::Setting3).into(),
            )]
        }),
        title_vars_fn: None,
        handler: battleworn_setting_3,
    },
];

/// `BattlewornDummy` spec. Pin: `shared.py:513`. Combat-encounter
/// kickoff is implicit (each setting is a placeholder-rewarded
/// shortcut today; the encounter-setup leg lands when
/// `event_combat_setup` plumbing arrives).
pub const BATTLEWORN_DUMMY: EventSpec = EventSpec {
    id: EventId::BattlewornDummy,
    python_name: "BattlewornDummy",
    is_allowed: always_allowed,
    generate_options: default_option_info_from_spec,
    options: BATTLEWORN_OPTIONS,
};

// ── Bugslayer ────────────────────────────────────────────────────────

// C# `Bugslayer.AddAndPreview<T>` (`Bugslayer.cs:40-45`): both options
// `CreateCard<T>` then `CardPileCmd.Add(card, Deck)` — each adds ONE specific
// card directly to the deck (Extermination → Exterminate, Squash → Squash),
// NOT a card-reward choice. The old `Reward::Card(empty)` push surfaced a
// random 3-card reward pick (an extra decision step + wrong cards), diverging
// the deck size and phase from C# (already back in map with the granted card).
// Same fix as `byrdonis_take` above. Bead `sts2-rs-RP26`.
fn bugslayer_add_card(state: &mut RunState, card_id: sts2_core::cards::CardId) {
    if let Some(card) = crate::cards::spec_index::card_pod_from_id(card_id, false) {
        state.add_card_to_deck(card, sts2_core::relic_effects::AddSource::EventGain);
    }
}

fn bugslayer_exterminate(state: &mut RunState) -> EventOutcome {
    bugslayer_add_card(state, sts2_core::cards::CardId::Exterminate);
    EventOutcome::finished("Added Exterminate to the deck.")
}

fn bugslayer_squash(state: &mut RunState) -> EventOutcome {
    bugslayer_add_card(state, sts2_core::cards::CardId::Squash);
    EventOutcome::finished("Added Squash to the deck.")
}

const BUGSLAYER_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "exterminate",
        loc_key: "BUGSLAYER.pages.INITIAL.options.EXTERMINATION",
        default_label: "Extermination",
        default_description: "Gain Exterminate card",
        default_vars: &[("Card1", EventVarSpec::Text("Exterminate"))],
        vars_fn: None,
        title_vars_fn: None,
        handler: bugslayer_exterminate,
    },
    EventOptionSpec {
        option_id: "squash",
        loc_key: "BUGSLAYER.pages.INITIAL.options.SQUASH",
        default_label: "Squash",
        default_description: "Gain Squash card",
        default_vars: &[("Card2", EventVarSpec::Text("Squash"))],
        vars_fn: None,
        title_vars_fn: None,
        handler: bugslayer_squash,
    },
];

/// `Bugslayer` spec. Pin: `shared.py:552`.
pub const BUGSLAYER: EventSpec = EventSpec {
    id: EventId::Bugslayer,
    python_name: "Bugslayer",
    is_allowed: always_allowed,
    generate_options: default_option_info_from_spec,
    options: BUGSLAYER_OPTIONS,
};

// ── ByrdonisNest ─────────────────────────────────────────────────────

pub const BYRDONIS_NEST_MAX_HP_GAIN: i32 = 7;

fn byrdonis_eat(state: &mut RunState) -> EventOutcome {
    state.gain_max_hp(BYRDONIS_NEST_MAX_HP_GAIN);
    EventOutcome::finished(format!("Gained {BYRDONIS_NEST_MAX_HP_GAIN} Max HP."))
}

fn byrdonis_take(state: &mut RunState) -> EventOutcome {
    // C# `ByrdonisNest.Take`: `CreateCard<ByrdonisEgg>` then
    // `CardPileCmd.Add(card, Deck)` — adds the ONE specific Byrdonis Egg card
    // to the deck, NOT a card-reward choice. The old `Reward::Card` push
    // surfaced a 3-card reward pick (an extra decision step + wrong card),
    // diverging the deck size and phase from C# (which is already in map with
    // the Egg). Add it directly, matching the other specific-card grants
    // (e.g. Overgrowth's Spoils Map).
    if let Some(card) =
        crate::cards::spec_index::card_pod_from_id(sts2_core::cards::CardId::ByrdonisEgg, false)
    {
        state.add_card_to_deck(card, sts2_core::relic_effects::AddSource::EventGain);
    }
    EventOutcome::finished("Added Byrdonis Egg to the deck.")
}

const BYRDONIS_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "eat",
        loc_key: "BYRDONIS_NEST.pages.INITIAL.options.EAT",
        default_label: "Eat",
        default_description: "+7 Max HP",
        default_vars: &[("MaxHp", EventVarSpec::Int(BYRDONIS_NEST_MAX_HP_GAIN as i64))],
        vars_fn: None,
        title_vars_fn: None,
        handler: byrdonis_eat,
    },
    EventOptionSpec {
        option_id: "take",
        loc_key: "BYRDONIS_NEST.pages.INITIAL.options.TAKE",
        default_label: "Take the Egg",
        default_description: "Gain Byrdonis Egg card",
        default_vars: &[("Card", EventVarSpec::Text("Byrdonis Egg"))],
        vars_fn: None,
        title_vars_fn: None,
        handler: byrdonis_take,
    },
];

/// `ByrdonisNest` spec. Pin: `shared.py:586`. `is_allowed`'s
/// `has_event_pet()` precondition deferred (no event-pet tracking
/// on RunState yet); defaults to always-allowed.
pub const BYRDONIS_NEST: EventSpec = EventSpec {
    id: EventId::ByrdonisNest,
    python_name: "ByrdonisNest",
    is_allowed: byrdonis_nest_allowed,
    generate_options: default_option_info_from_spec,
    options: BYRDONIS_OPTIONS,
};

// ── ColorfulPhilosophers ─────────────────────────────────────────────

/// `ColorfulPhilosophers`' `Id.Entry`: names its own event stream.
const COLORFUL_PHILOSOPHERS_STREAM: &str = "COLORFUL_PHILOSOPHERS";

/// C# `CardPoolColorOrder` (`ColorfulPhilosophers.cs`): the character pools
/// in the event's order, each with its option (keyed by the pool's
/// `EnergyColorName`).
const PHILOSOPHER_POOLS: [(sts2_core::potions::CharacterId, &str); 5] = [
    (sts2_core::potions::CharacterId::Necrobinder, "necrobinder"),
    (sts2_core::potions::CharacterId::Ironclad, "ironclad"),
    (sts2_core::potions::CharacterId::Regent, "regent"),
    (sts2_core::potions::CharacterId::Silent, "silent"),
    (sts2_core::potions::CharacterId::Defect, "defect"),
];

/// C# `GenerateInitialOptions`: the pools of `CardPoolColorOrder` other than
/// the owner's (every character pool is unlocked here, as Kaleidoscope's
/// roll assumes), then `list.RemoveAt(Rng.NextInt(list.Count))` on the
/// event's `Rng` until three are left. The event's `Rng` is read nowhere
/// else, so a fresh stream at cursor 0 gives the same draws each time the
/// options are built. sts2-agent fork (pool-placeholders).
fn philosopher_option_ids(state: &RunState) -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = PHILOSOPHER_POOLS
        .iter()
        .filter(|(c, _)| *c != state.character_id)
        .map(|(_, id)| *id)
        .collect();
    let mut rng = event_rng(state, COLORFUL_PHILOSOPHERS_STREAM);
    while ids.len() > 3 {
        let i = rng.next_int_exclusive(0, ids.len() as i32) as usize;
        ids.remove(i);
    }
    ids
}

fn philosopher_generate_options(state: &RunState, spec: &EventSpec) -> Vec<EventOptionInfo> {
    let ids = philosopher_option_ids(state);
    default_option_info_from_spec(state, spec)
        .into_iter()
        .filter(|o| ids.contains(&o.option_id))
        .collect()
}

/// C# `OfferRewards(pool)`: `RewardsCmd.OfferCustom` with three card rewards
/// from the chosen pool, one per rarity
/// ([`roll_colorful_philosophers_card_rewards`](crate::cards::spec_index::roll_colorful_philosophers_card_rewards)).
fn philosopher_pool(
    state: &mut RunState,
    character: sts2_core::potions::CharacterId,
) -> EventOutcome {
    let rewards =
        crate::cards::spec_index::roll_colorful_philosophers_card_rewards(state, character);
    if state.interactive_rewards {
        let set = rewards
            .into_iter()
            .map(|(options, upgraded)| sts2_core::OfferedReward::Card {
                options: Some(options),
                upgraded,
                no_upgrade_roll: false,
            })
            .collect();
        state.offer_custom_rewards(set, true);
    } else {
        for (options, upgraded) in rewards {
            state
                .pending_rewards
                .push(Reward::Card(CardReward::from_resolved(options, upgraded)));
        }
    }
    EventOutcome::finished("Received card rewards from a philosopher's pool.")
}

fn philosopher_necrobinder(state: &mut RunState) -> EventOutcome {
    philosopher_pool(state, sts2_core::potions::CharacterId::Necrobinder)
}

fn philosopher_ironclad(state: &mut RunState) -> EventOutcome {
    philosopher_pool(state, sts2_core::potions::CharacterId::Ironclad)
}

fn philosopher_regent(state: &mut RunState) -> EventOutcome {
    philosopher_pool(state, sts2_core::potions::CharacterId::Regent)
}

fn philosopher_silent(state: &mut RunState) -> EventOutcome {
    philosopher_pool(state, sts2_core::potions::CharacterId::Silent)
}

fn philosopher_defect(state: &mut RunState) -> EventOutcome {
    philosopher_pool(state, sts2_core::potions::CharacterId::Defect)
}

const fn philosopher_option(
    option_id: &'static str,
    loc_key: &'static str,
    handler: fn(&mut RunState) -> EventOutcome,
) -> EventOptionSpec {
    EventOptionSpec {
        option_id,
        loc_key,
        default_label: "Philosopher",
        default_description: "Card rewards from this character's pool",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler,
    }
}

const PHILOSOPHER_OPTIONS: &[EventOptionSpec] = &[
    philosopher_option(
        "necrobinder",
        "COLORFUL_PHILOSOPHERS.pages.INITIAL.options.NECROBINDER",
        philosopher_necrobinder,
    ),
    philosopher_option(
        "ironclad",
        "COLORFUL_PHILOSOPHERS.pages.INITIAL.options.IRONCLAD",
        philosopher_ironclad,
    ),
    philosopher_option(
        "regent",
        "COLORFUL_PHILOSOPHERS.pages.INITIAL.options.REGENT",
        philosopher_regent,
    ),
    philosopher_option(
        "silent",
        "COLORFUL_PHILOSOPHERS.pages.INITIAL.options.SILENT",
        philosopher_silent,
    ),
    philosopher_option(
        "defect",
        "COLORFUL_PHILOSOPHERS.pages.INITIAL.options.DEFECT",
        philosopher_defect,
    ),
];

/// `ColorfulPhilosophers` spec. Pin: `shared.py:618`; C#
/// `ColorfulPhilosophers.cs`: three of the other characters' pools
/// ([`philosopher_generate_options`]).
pub const COLORFUL_PHILOSOPHERS: EventSpec = EventSpec {
    id: EventId::ColorfulPhilosophers,
    python_name: "ColorfulPhilosophers",
    is_allowed: always_allowed,
    generate_options: philosopher_generate_options,
    options: PHILOSOPHER_OPTIONS,
};

// ── ColossalFlower ───────────────────────────────────────────────────

pub const COLOSSAL_FLOWER_FIRST_GOLD: i32 = 35;
pub const COLOSSAL_FLOWER_FIRST_DAMAGE: i32 = 5;
pub const COLOSSAL_FLOWER_HP_GATE: i32 = 19;

/// C# `ColossalFlower._prizeCosts` / `_prizeDamage` (`ColossalFlower.cs:20-22`):
/// gold prize and reach-deeper damage per dig depth (`NumberOfDigs`).
const COLOSSAL_FLOWER_PRIZE_COSTS: [i32; 3] = [35, 75, 135];
const COLOSSAL_FLOWER_PRIZE_DAMAGE: [i32; 3] = [5, 6, 7];

fn colossal_flower_allowed(state: &RunState) -> bool {
    state.current_hp >= COLOSSAL_FLOWER_HP_GATE
}

/// Current `NumberOfDigs`, clamped to the 3-tier tables.
fn colossal_flower_digs(state: &RunState) -> usize {
    (state.event_state.page as usize).min(2)
}

fn colossal_flower_extract(state: &mut RunState) -> EventOutcome {
    // C# `ExtractCurrentPrize` / `ExtractInstead` (identical mechanics):
    // gain `_prizeCosts[NumberOfDigs]` and finish — 35 / 75 / 135 gold at
    // dig depth 0 / 1 / 2.
    let gold = COLOSSAL_FLOWER_PRIZE_COSTS[colossal_flower_digs(state)];
    state.gain_gold(gold);
    EventOutcome::finished(format!("Extracted {gold} gold."))
}

fn colossal_flower_reach_deeper(state: &mut RunState) -> EventOutcome {
    // C# `ReachDeeper` (`ColossalFlower.cs:60-80`): unblockable
    // `_prizeDamage[NumberOfDigs]` damage, `NumberOfDigs++`, then present
    // the next 2-option page. After the second dig the option pair becomes
    // ExtractInstead (135 gold) / ObtainPollinousCore — the relic branch
    // deals `_prizeDamage[2]` again and grants PollinousCore
    // (`ObtainPollinousCore`, `ColossalFlower.cs:100-105`). The engine
    // previously finished after ONE dig ("multi-page deferred"), ending the
    // event two pages before the DLL (shadow census v8 seeds 1009/1046).
    let digs = colossal_flower_digs(state);
    let lost = state.lose_hp(COLOSSAL_FLOWER_PRIZE_DAMAGE[digs]);
    if digs >= 2 {
        // Page-3 option 2 = ObtainPollinousCore: second 7-damage hit
        // already applied above; grant the relic and finish.
        state.obtain_relic(sts2_core::RelicId::PollinousCore);
        return EventOutcome::finished(format!(
            "Took {lost} damage and pried out the Pollinous Core."
        ));
    }
    state.event_state.page = state.event_state.page.saturating_add(1);
    EventOutcome {
        finished: false,
        description: format!("Took {lost} damage, dug deeper."),
        effects: Vec::new(),
    }
}

const COLOSSAL_FLOWER_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "extract",
        loc_key: "COLOSSAL_FLOWER.pages.INITIAL.options.EXTRACT_CURRENT_PRIZE_1",
        default_label: "Extract Prize",
        default_description: "Gain 35 gold",
        default_vars: &[(
            "Prize1",
            EventVarSpec::Int(COLOSSAL_FLOWER_FIRST_GOLD as i64),
        )],
        vars_fn: None,
        title_vars_fn: None,
        handler: colossal_flower_extract,
    },
    EventOptionSpec {
        option_id: "reach_deeper",
        loc_key: "COLOSSAL_FLOWER.pages.INITIAL.options.REACH_DEEPER_1",
        default_label: "Reach Deeper",
        default_description: "Take 5 damage for better reward",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: colossal_flower_reach_deeper,
    },
];

/// `ColossalFlower` spec. Pin: `shared.py:664`.
pub const COLOSSAL_FLOWER: EventSpec = EventSpec {
    id: EventId::ColossalFlower,
    python_name: "ColossalFlower",
    is_allowed: colossal_flower_allowed,
    generate_options: default_option_info_from_spec,
    options: COLOSSAL_FLOWER_OPTIONS,
};

// ── Darv ─────────────────────────────────────────────────────────────

// Reached in a run through the boss-relic surface; as a forced event it presents
// the relics the Ancient rolled (`RunState::roll_ancient_event_offers`), like the
// other Ancients (see `ancient_generate_options`). The earlier separate port of
// `Darv.GenerateInitialOptions` rolled other relics than the game's (Step53 gate)
// and carried blank text keys.

/// `Darv` spec. Pin: `shared.py:740`; C# `Darv.cs`.
pub const DARV: EventSpec = EventSpec {
    id: EventId::Darv,
    python_name: "Darv",
    is_allowed: always_allowed,
    generate_options: ancient_generate_options,
    options: ANCIENT_OPTIONS,
};

// ── DenseVegetation ──────────────────────────────────────────────────

/// C# `HpLossVar(8m)` (`DenseVegetation.cs` CanonicalVars) — the Python pin's
/// 11 + card-removal leg was drift; C# TrudgeOn is 8 unblockable damage +
/// the CalculateVars gold grant.
pub const DENSE_VEGETATION_TRUDGE_DAMAGE: i32 = 8;
pub const DENSE_VEGETATION_HEAL_PCT_NUM: i32 = 3;
pub const DENSE_VEGETATION_HEAL_PCT_DEN: i32 = 10;

/// C# `CalculateVars`: `Gold = Rng.NextInt(61, 100)` (max-exclusive ->
/// [61, 99]) off the event's own stream (`IsShared => true`, so the seed is
/// `run_seed + hash("DENSE_VEGETATION")` with no NetId — same shape as
/// [`super::act2_hive::JUNGLE_MAZE_ADVENTURE`]'s gold swing).
fn dense_vegetation_gold(state: &RunState) -> i32 {
    let mut rng = sts2_core::dotnet_rng::Rng::with_name(state.rng.seed, "DENSE_VEGETATION");
    rng.next_int(61, 99)
}

fn dense_vegetation_trudge(state: &mut RunState) -> EventOutcome {
    // C# TrudgeOn: 8 unblockable damage, then `PlayerCmd.GainGold(Gold)`
    // (`DenseVegetation.cs` TrudgeOn — the VFX block consumes only the
    // non-deterministic `Rng.Chaotic`, never the event stream).
    let gold = dense_vegetation_gold(state);
    let dmg = state.lose_hp(DENSE_VEGETATION_TRUDGE_DAMAGE);
    state.gain_gold(gold);
    EventOutcome::finished(format!("Took {dmg} damage, gained {gold} gold."))
}

/// The Rest heal the option advertises: 30% of max HP, the base
/// [`dense_vegetation_rest`] then applies. Split out so the choice screen can
/// print the number, which is what C# `CalculateVars` does.
fn dense_vegetation_rest_heal(state: &RunState) -> i32 {
    state.max_hp * DENSE_VEGETATION_HEAL_PCT_NUM / DENSE_VEGETATION_HEAL_PCT_DEN
}

fn dense_vegetation_rest(state: &mut RunState) -> EventOutcome {
    // C# Rest: `PlayerCmd.MimicRestSiteHeal` -> `ExecuteRestSiteHeal`
    // (isMimicked:true) = the FULL rest-site heal (30% max HP through the
    // ModifyRestSiteHealAmount hooks) INCLUDING the reward fan-out
    // (`Hook.ModifyRestSiteHealRewards` -> Tiny Mailbox potions etc.). Route
    // through `rest_site_heal`, which fires `after_rest_site_heal` AND grants
    // the offered rewards — the prior bare `heal()` skipped both, so a mimicked
    // rest with Tiny Mailbox gained no potion (sts2-rs-RP23). Then
    // `SetEventState` opens the REST page with its one option, FIGHT
    // (sts2-agent fork, `event-fight-unbridged`).
    let base = state.max_hp * DENSE_VEGETATION_HEAL_PCT_NUM / DENSE_VEGETATION_HEAL_PCT_DEN;
    let healed = state.rest_site_heal(base);
    state.event_state.page = 1;
    EventOutcome {
        finished: false,
        description: format!("Healed {healed} HP. Something stirs in the vegetation."),
        effects: Vec::new(),
    }
}

/// REST page's FIGHT: C# `EnterCombatWithoutExitingEvent<DenseVegetation
/// EventEncounter>([], shouldResumeAfterCombat: false)` — a Monster room's
/// fight with no extras; the event ends with it. sts2-agent fork
/// (`event-fight-unbridged`).
fn dense_vegetation_fight(state: &mut RunState) -> EventOutcome {
    state.event_combat = Some(EventCombatRequest::no_resume(
        EventEncounter::DenseVegetation,
        EventId::DenseVegetation,
        Vec::new(),
    ));
    EventOutcome {
        finished: false,
        description: "Wrigglers burst from the vegetation. Fight!".to_string(),
        effects: Vec::new(),
    }
}

/// Page 0: TRUDGE_ON, REST; page 1 (after REST): FIGHT.
fn dense_vegetation_generate_options(state: &RunState, spec: &EventSpec) -> Vec<EventOptionInfo> {
    two_page_option_info(state, spec, 2)
}

const DENSE_VEGETATION_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "trudge_on",
        loc_key: "DENSE_VEGETATION.pages.INITIAL.options.TRUDGE_ON",
        default_label: "Trudge On",
        default_description: "Take 8 damage, gain 61-99 gold",
        default_vars: &[(
            "HpLoss",
            EventVarSpec::Int(DENSE_VEGETATION_TRUDGE_DAMAGE as i64),
        )],
        vars_fn: Some(|s| vec![("Gold", dense_vegetation_gold(s).into())]),
        title_vars_fn: None,
        handler: dense_vegetation_trudge,
    },
    EventOptionSpec {
        option_id: "rest",
        loc_key: "DENSE_VEGETATION.pages.INITIAL.options.REST",
        default_label: "Rest",
        default_description: "Heal, then fight",
        default_vars: &[],
        vars_fn: Some(|s| vec![("Heal", dense_vegetation_rest_heal(s).into())]),
        title_vars_fn: None,
        handler: dense_vegetation_rest,
    },
    // ── page 1 (REST): the Fight follow-up ──
    EventOptionSpec {
        option_id: "fight",
        loc_key: "DENSE_VEGETATION.pages.REST.options.FIGHT",
        default_label: "Fight",
        default_description: "Fight the Wrigglers.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: dense_vegetation_fight,
    },
];

/// `DenseVegetation` spec. Pin: `shared.py:803`.
pub const DENSE_VEGETATION: EventSpec = EventSpec {
    id: EventId::DenseVegetation,
    python_name: "DenseVegetation",
    is_allowed: always_allowed,
    generate_options: dense_vegetation_generate_options,
    options: DENSE_VEGETATION_OPTIONS,
};

// ── DoorsOfLightAndDark ──────────────────────────────────────────────

/// `DoorsOfLightAndDark`'s `Id.Entry` (L10N key root) — names its event stream.
const DOORS_OF_LIGHT_AND_DARK_STREAM: &str = "DOORS_OF_LIGHT_AND_DARK";

fn doors_light(state: &mut RunState) -> EventOutcome {
    // C# `DoorsOfLightAndDark.cs:28` (v0.107.1):
    // `Where(IsUpgradable).StableShuffle(base.Rng).Take(N)` → `CardCmd.Upgrade`
    // — random, no player pick, upgraded IMMEDIATELY. v0.107.1 migrated the
    // shuffle stream from `RunState.Rng.Niche` (v0.103.2) to the event's own
    // `base.Rng`; this is the FIRST (and only) `base.Rng` read of the event
    // (CanonicalVars = `CardsVar(2)`, no entry roll) → fresh `event_rng` at
    // cursor 0 matches.
    let mut rng = event_rng(state, DOORS_OF_LIGHT_AND_DARK_STREAM);
    let _ = state.upgrade_random_cards_stable_with_rng(2, &mut rng);
    EventOutcome::finished("Upgraded 2 cards.")
}

fn doors_dark(state: &mut RunState) -> EventOutcome {
    // C# `DoorsOfLightAndDark` dark door (`DoorsOfLightAndDark.cs:40-41`):
    // `CardSelectCmd.FromDeckForRemoval(count 1)` → `CardPileCmd.RemoveFromDeck`
    // — the player REMOVES one deck card. The old `Reward::Card(empty)` push
    // surfaced a 3-card ADD choice (the semantic opposite), diverging deck
    // size/contents from C#. Queue a single removal pick instead. Bead
    // `sts2-rs-RP27`.
    state.pending_rewards.push(Reward::RemoveCard { count: 1 });
    EventOutcome::finished("Remove 1 card.")
}

const DOORS_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "light",
        loc_key: "DOORS_OF_LIGHT_AND_DARK.pages.INITIAL.options.LIGHT",
        default_label: "Door of Light",
        default_description: "Upgrade 2 random cards",
        default_vars: &[("Cards", EventVarSpec::Int(2))],
        vars_fn: None,
        title_vars_fn: None,
        handler: doors_light,
    },
    EventOptionSpec {
        option_id: "dark",
        loc_key: "DOORS_OF_LIGHT_AND_DARK.pages.INITIAL.options.DARK",
        default_label: "Door of Dark",
        default_description: "Remove 1 card",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: doors_dark,
    },
];

/// `DoorsOfLightAndDark` spec. Pin: `shared.py:862`.
pub const DOORS_OF_LIGHT_AND_DARK: EventSpec = EventSpec {
    id: EventId::DoorsOfLightAndDark,
    python_name: "DoorsOfLightAndDark",
    is_allowed: always_allowed,
    generate_options: default_option_info_from_spec,
    options: DOORS_OPTIONS,
};

// ── DrowningBeacon ───────────────────────────────────────────────────

pub const DROWNING_BEACON_MAX_HP_LOSS: i32 = 13;

fn drowning_bottle(state: &mut RunState) -> EventOutcome {
    // C# `DrowningBeacon.BottleOption` (`DrowningBeacon.cs:37-44`):
    // `RewardsCmd.OfferCustom([new PotionReward(GlowwaterPotion.ToMutable())])`
    // — a SPECIFIC, fixed potion (Glowwater), no RNG roll, no rarity. The
    // prior `Reward::Card(empty)` placeholder both granted the wrong reward
    // type AND silently dropped the potion at drain time (a Card sentinel
    // surfaces a card-reward screen, not a potion). Push a populated
    // `Reward::Potion` so the drain grants Glowwater directly. `OfferCustom`
    // does not route through the Sozu procure-veto, so this is a direct push.
    let glowwater = sts2_core::potions::PotionKind::GlowwaterPotion;
    if state.interactive_rewards {
        state.offer_custom_rewards(
            vec![sts2_core::OfferedReward::Potion(Some(glowwater))],
            true,
        );
        return EventOutcome::finished("Gained Glowwater Potion.");
    }
    state
        .pending_rewards
        .push(Reward::Potion(PotionReward::with_kind(glowwater)));
    EventOutcome::finished("Gained Glowwater Potion.")
}

fn drowning_climb(state: &mut RunState) -> EventOutcome {
    state.lose_max_hp(DROWNING_BEACON_MAX_HP_LOSS);
    state.grant_event_relic(RelicId::FresnelLens);
    EventOutcome::finished(format!(
        "Lost {DROWNING_BEACON_MAX_HP_LOSS} Max HP, gained Fresnel Lens relic."
    ))
}

const DROWNING_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "bottle",
        loc_key: "DROWNING_BEACON.pages.INITIAL.options.BOTTLE",
        default_label: "Bottle the Water",
        default_description: "Gain Glowwater Potion",
        default_vars: &[("Potion", EventVarSpec::Text("Glowwater Potion"))],
        vars_fn: None,
        title_vars_fn: None,
        handler: drowning_bottle,
    },
    EventOptionSpec {
        option_id: "climb",
        loc_key: "DROWNING_BEACON.pages.INITIAL.options.CLIMB",
        default_label: "Climb",
        default_description: "Lose 13 Max HP, gain Fresnel Lens relic",
        default_vars: &[
            ("Relic", EventVarSpec::Text("Fresnel Lens")),
            (
                "HpLoss",
                EventVarSpec::Int(DROWNING_BEACON_MAX_HP_LOSS as i64),
            ),
        ],
        vars_fn: None,
        title_vars_fn: None,
        handler: drowning_climb,
    },
];

/// `DrowningBeacon` spec. Pin: `shared.py:903`.
pub const DROWNING_BEACON: EventSpec = EventSpec {
    id: EventId::DrowningBeacon,
    python_name: "DrowningBeacon",
    is_allowed: always_allowed,
    generate_options: default_option_info_from_spec,
    options: DROWNING_OPTIONS,
};

// ── GraveOfTheForgotten ──────────────────────────────────────────────

fn grave_confront(state: &mut RunState) -> EventOutcome {
    // Installed GraveOfTheForgotten.Confront awaits the hooked Decay add
    // BEFORE constructing the SoulsPower candidate list. Neither is a reward.
    if let Some(card) =
        crate::cards::spec_index::card_pod_from_id(sts2_core::cards::CardId::Decay, false)
    {
        state.add_card_to_deck(card, sts2_core::relic_effects::AddSource::EventGain);
    }
    let eligible = state
        .master_deck
        .iter()
        .filter(|c| souls_power_can_enchant(c))
        .count();
    if state.interactive_card_transform && eligible > 1 {
        state
            .pending_deck_enchants
            .push(sts2_core::PendingDeckEnchant {
                enchantment: sts2_core::cards::EnchantmentId::SoulsPower,
                amount: 1,
                type_restriction: None,
                secondary_type: None,
            });
    } else if let Some(card) = state
        .master_deck
        .iter_mut()
        .find(|c| souls_power_can_enchant(c))
    {
        // FromDeckForEnchantment auto-takes all at <= MinSelect(1).
        // The existing instance keyword reducer removes Exhaust for SoulsPower.
        card.set_enchantment(sts2_core::cards::EnchantmentId::SoulsPower, 1);
    }
    EventOutcome::finished("Added Decay and enchanted a card with Soul's Power.")
}

fn grave_accept(state: &mut RunState) -> EventOutcome {
    state.offer_relic_rewards(RelicId::ForgottenSoul, 1);
    EventOutcome::finished("Gained Forgotten Soul relic.")
}

const GRAVE_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "confront",
        loc_key: "GRAVE_OF_THE_FORGOTTEN.pages.INITIAL.options.CONFRONT",
        default_label: "Confront",
        default_description: "Gain Decay curse, enchant a card",
        default_vars: &[
            ("Curse", EventVarSpec::Text("Decay")),
            ("Enchantment", EventVarSpec::Text("Soul's Power")),
        ],
        vars_fn: None,
        title_vars_fn: None,
        handler: grave_confront,
    },
    EventOptionSpec {
        option_id: "accept",
        loc_key: "GRAVE_OF_THE_FORGOTTEN.pages.INITIAL.options.ACCEPT",
        default_label: "Accept",
        default_description: "Gain Forgotten Soul relic",
        default_vars: &[("Relic", EventVarSpec::Text("Forgotten Soul"))],
        vars_fn: None,
        title_vars_fn: None,
        handler: grave_accept,
    },
];

/// C# `SoulsPower.CanEnchant`: the base `EnchantmentModel.CanEnchant` (not
/// Status/Curse, no existing enchantment, playable) and the card's own
/// Exhaust keyword (`GetKeywordsWithSources(Local)`). sts2-agent fork.
fn souls_power_can_enchant(c: &sts2_core::cards::Card) -> bool {
    use sts2_core::cards::CardType;
    !c.card_id.is_quest()
        && !matches!(c.card_type, CardType::Status | CardType::Curse)
        && c.enchantments.is_empty()
        && (c.cost >= 0 || c.is_x_cost())
        && sts2_core::card_effects::card_exhausts_inst(c)
}

/// C# `GraveOfTheForgotten.IsAllowed` / `HasEnchantableCards` use the
/// same Souls eligibility as the Confront option, including own Exhaust.
fn grave_allowed(state: &RunState) -> bool {
    state.master_deck.iter().any(souls_power_can_enchant)
}

/// C# `GraveOfTheForgotten.GenerateInitialOptions`: without a deck card
/// `SoulsPower.CanEnchant` accepts (`HasEnchantableCards`), Confront is the
/// handler-less `CONFRONT_LOCKED`. sts2-agent fork.
fn grave_generate_options(state: &RunState, spec: &EventSpec) -> Vec<EventOptionInfo> {
    let mut opts = default_option_info_from_spec(state, spec);
    super::lock_option_if(
        &mut opts,
        "confront",
        !grave_allowed(state),
        "GRAVE_OF_THE_FORGOTTEN.pages.INITIAL.options.CONFRONT_LOCKED",
    );
    opts
}

/// `GraveOfTheForgotten` spec. Pin: `shared.py:937`.
pub const GRAVE_OF_THE_FORGOTTEN: EventSpec = EventSpec {
    id: EventId::GraveOfTheForgotten,
    python_name: "GraveOfTheForgotten",
    is_allowed: grave_allowed,
    generate_options: grave_generate_options,
    options: GRAVE_OPTIONS,
};

// ── HungryForMushrooms ───────────────────────────────────────────────

pub const HUNGRY_FRAGRANT_DAMAGE: i32 = 15;

fn mushrooms_big(state: &mut RunState) -> EventOutcome {
    state.grant_event_relic(RelicId::BigMushroom);
    EventOutcome::finished("Gained Big Mushroom relic.")
}

fn mushrooms_fragrant(state: &mut RunState) -> EventOutcome {
    let dmg = state.lose_hp(HUNGRY_FRAGRANT_DAMAGE);
    state.grant_event_relic(RelicId::FragrantMushroom);
    EventOutcome::finished(format!(
        "Took {dmg} damage, gained Fragrant Mushroom relic."
    ))
}

const MUSHROOMS_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "big",
        loc_key: "HUNGRY_FOR_MUSHROOMS.pages.INITIAL.options.BIG_MUSHROOM",
        default_label: "Big Mushroom",
        default_description: "Gain Big Mushroom relic",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: mushrooms_big,
    },
    EventOptionSpec {
        option_id: "fragrant",
        loc_key: "HUNGRY_FOR_MUSHROOMS.pages.INITIAL.options.FRAGRANT_MUSHROOM",
        default_label: "Fragrant Mushroom",
        default_description: "Take 15 damage, gain Fragrant Mushroom relic",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: mushrooms_fragrant,
    },
];

/// `HungryForMushrooms` spec. Pin: `shared.py:993`.
pub const HUNGRY_FOR_MUSHROOMS: EventSpec = EventSpec {
    id: EventId::HungryForMushrooms,
    python_name: "HungryForMushrooms",
    is_allowed: always_allowed,
    generate_options: default_option_info_from_spec,
    options: MUSHROOMS_OPTIONS,
};

// ── InfestedAutomaton ────────────────────────────────────────────────

// C# InfestedAutomaton (`InfestedAutomaton.cs:28-52`): each option creates
// ONE card via `CardFactory.CreateForReward(owner, 1, ForNonCombatWithDefault
// Odds([Character.CardPool], predicate))` off `PlayerRng.Rewards` and adds it
// DIRECTLY to the deck — NOT a 3-card player choice. The predicate restricts
// the pool (Study = `Type == Power`; TouchCore = `cost == 0 && !CostsX`); the
// rarity is rolled off base RegularEncounter odds (no pity, no upgrade roll).
// The old `Reward::Card(empty)` push surfaced a random unfiltered 3-card pick
// (extra step + wrong cards + Rewards-stream desync). Bead `sts2-rs-RP28`.
fn infested_study(state: &mut RunState) -> EventOutcome {
    if let Some(id) = crate::cards::spec_index::roll_infested_study_card(state) {
        if let Some(card) = crate::cards::spec_index::card_pod_from_id(id, false) {
            state.add_card_to_deck(card, sts2_core::relic_effects::AddSource::EventGain);
        }
    }
    EventOutcome::finished("Gained a random Power card.")
}

fn infested_touch_core(state: &mut RunState) -> EventOutcome {
    if let Some(id) = crate::cards::spec_index::roll_infested_touch_core_card(state) {
        if let Some(card) = crate::cards::spec_index::card_pod_from_id(id, false) {
            state.add_card_to_deck(card, sts2_core::relic_effects::AddSource::EventGain);
        }
    }
    EventOutcome::finished("Gained a random 0-cost card.")
}

const INFESTED_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "study",
        loc_key: "INFESTED_AUTOMATON.pages.INITIAL.options.STUDY",
        default_label: "Study",
        default_description: "Gain a random Power card",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: infested_study,
    },
    EventOptionSpec {
        option_id: "touch_core",
        loc_key: "INFESTED_AUTOMATON.pages.INITIAL.options.TOUCH_CORE",
        default_label: "Touch Core",
        default_description: "Gain a random 0-cost card",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: infested_touch_core,
    },
];

/// `InfestedAutomaton` spec. Pin: `shared.py:1029`.
pub const INFESTED_AUTOMATON: EventSpec = EventSpec {
    id: EventId::InfestedAutomaton,
    python_name: "InfestedAutomaton",
    is_allowed: always_allowed,
    generate_options: default_option_info_from_spec,
    options: INFESTED_OPTIONS,
};

// ── Nonupeipe, Orobas, Pael: Ancients entered as events ──────────────
//
// A run reaches an Ancient through the boss-relic surface
// (`RunState::roll_boss_relic_offers`); these specs serve the harness's forced
// room entry (`enter_room type event`), where the host presents the Ancient as
// an ordinary event whose three options are the relics its `OptionPool` rolled
// off the per-event RNG (the same roll and conditions as the run's Ancient,
// `RunState::roll_ancient_event_offers`). Picking one is C# `RelicOption`'s
// direct `RelicCmd.Obtain`, as Darv's. Lane F's F8 (`ancient-forced-as-event`).

/// The loc keys of the rolled options, interned: `EventOptionInfo::loc_key` is
/// `&'static str` and the set is small and bounded (3 ancients x their pools).
fn ancient_loc_key(event: &str, relic: RelicId) -> &'static str {
    use std::collections::BTreeMap;
    use std::sync::{Mutex, OnceLock};
    static KEYS: OnceLock<Mutex<BTreeMap<String, &'static str>>> = OnceLock::new();
    let key = format!("{event}.pages.INITIAL.options.{}", relic.to_python_name());
    let mut map = KEYS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .expect("loc key table");
    if let Some(k) = map.get(&key) {
        return k;
    }
    let leaked: &'static str = Box::leak(key.clone().into_boxed_str());
    map.insert(key, leaked);
    leaked
}

fn ancient_of_event(id: EventId) -> sts2_core::run_room_gen::AncientId {
    use sts2_core::run_room_gen::AncientId;
    match id {
        EventId::Nonupeipe => AncientId::Nonupeipe,
        EventId::Orobas => AncientId::Orobas,
        EventId::Pael => AncientId::Pael,
        EventId::Darv => AncientId::Darv,
        EventId::Tanx => AncientId::Tanx,
        EventId::Tezcatara => AncientId::Tezcatara,
        EventId::Vakuu => AncientId::Vakuu,
        other => unreachable!("{other:?} is not a forced-entry Ancient stub"),
    }
}

fn ancient_event_name(id: EventId) -> &'static str {
    ancient_of_event(id).entry()
}

pub(super) fn ancient_generate_options(state: &RunState, spec: &EventSpec) -> Vec<EventOptionInfo> {
    let offers = state.roll_ancient_event_offers(ancient_of_event(spec.id));
    ["relic_1", "relic_2", "relic_3"]
        .iter()
        .zip(offers.iter())
        .map(|(id, relic)| EventOptionInfo {
            option_id: id,
            // The game's own row: `<ANCIENT>.pages.INITIAL.options.<RELIC>`.
            loc_key: ancient_loc_key(ancient_event_name(spec.id), *relic),
            vars: Vec::new(),
            title_vars: Vec::new(),
            label: format!("{relic:?}"),
            description: "Gain this relic.".to_string(),
            enabled: true,
        })
        .collect()
}

fn ancient_grant(state: &mut RunState, slot: usize) -> EventOutcome {
    let id = state.current_event.expect("an Ancient event is open");
    let relic = state.roll_ancient_event_offers(ancient_of_event(id))[slot];
    state.grant_event_relic(relic);
    EventOutcome::finished("Obtained a relic from the Ancient.")
}

fn ancient_relic_1(state: &mut RunState) -> EventOutcome {
    ancient_grant(state, 0)
}

fn ancient_relic_2(state: &mut RunState) -> EventOutcome {
    ancient_grant(state, 1)
}

fn ancient_relic_3(state: &mut RunState) -> EventOutcome {
    ancient_grant(state, 2)
}

pub(super) const ANCIENT_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "relic_1",
        loc_key: "",
        default_label: "Relic 1",
        default_description: "Gain a relic",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: ancient_relic_1,
    },
    EventOptionSpec {
        option_id: "relic_2",
        loc_key: "",
        default_label: "Relic 2",
        default_description: "Gain a relic",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: ancient_relic_2,
    },
    EventOptionSpec {
        option_id: "relic_3",
        loc_key: "",
        default_label: "Relic 3",
        default_description: "Gain a relic",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: ancient_relic_3,
    },
];

/// `Nonupeipe` spec. Pin: `shared.py:1096`; C# `Nonupeipe.cs`.
pub const NONUPEIPE: EventSpec = EventSpec {
    id: EventId::Nonupeipe,
    python_name: "Nonupeipe",
    is_allowed: always_allowed,
    generate_options: ancient_generate_options,
    options: ANCIENT_OPTIONS,
};

/// `Orobas` spec. Pin: `shared.py:1143`; C# `Orobas.cs`.
pub const OROBAS: EventSpec = EventSpec {
    id: EventId::Orobas,
    python_name: "Orobas",
    is_allowed: always_allowed,
    generate_options: ancient_generate_options,
    options: ANCIENT_OPTIONS,
};

/// `Pael` spec. Pin: `shared.py`; C# `Pael.cs`.
pub const PAEL: EventSpec = EventSpec {
    id: EventId::Pael,
    python_name: "Pael",
    is_allowed: always_allowed,
    generate_options: ancient_generate_options,
    options: ANCIENT_OPTIONS,
};

// ── PunchOff ─────────────────────────────────────────────────────────

fn punchoff_nab(state: &mut RunState) -> EventOutcome {
    // C# `PunchOff.Nab` (`PunchOff.cs:108-119`): `AddCurseToDeck<Injury>` (one
    // specific Injury curse added directly to the deck) THEN a `RelicReward`.
    // The old `Reward::Card(empty)` surfaced a random 3-card ADD choice instead
    // of forcing the Injury curse. Add it directly; keep the relic reward.
    // Bead `sts2-rs-RP27`. (The placeholder `Astrolabe` relic id predates this
    // and is a separate stand-in, out of scope here.)
    if let Some(card) =
        crate::cards::spec_index::card_pod_from_id(sts2_core::cards::CardId::Injury, false)
    {
        state.add_card_to_deck(card, sts2_core::relic_effects::AddSource::EventGain);
    }
    if state.interactive_rewards {
        // C# `PunchOff.Nab`: `OfferCustom([new RelicReward(player)])`, a relic
        // rolled when offered (the placeholder below stands in for it).
        state.offer_custom_rewards(
            vec![sts2_core::OfferedReward::RandomRelic {
                rarity: None,
                is_wax: false,
            }],
            true,
        );
        return EventOutcome::finished("Added Injury to the deck and gained a relic.");
    }
    state
        .pending_rewards
        .push(Reward::Relic(RelicReward::new(RelicId::Astrolabe)));
    EventOutcome::finished("Added Injury to the deck and gained a relic.")
}

fn punchoff_take_them(state: &mut RunState) -> EventOutcome {
    // C# `PunchOff.TakeThem` (`PunchOff.cs`): stops the punching and
    // `SetEventState`s the I_CAN_TAKE_THEM page with its one option, FIGHT;
    // nothing is granted here (sts2-agent fork, `event-fight-unbridged`).
    state.event_state.page = 1;
    EventOutcome {
        finished: false,
        description: "The constructs turn to face you.".to_string(),
        effects: Vec::new(),
    }
}

/// I_CAN_TAKE_THEM page's FIGHT: C# `PunchOff.Fight` →
/// `EnterCombatWithoutExitingEvent<PunchOffEventEncounter>([RelicReward,
/// PotionReward], shouldResumeAfterCombat: false)` — the event's combat
/// layout fought as a Monster room, its rewards the room's plus a relic and a
/// potion (no card beyond the room's). sts2-agent fork
/// (`event-fight-unbridged`).
fn punchoff_fight(state: &mut RunState) -> EventOutcome {
    state.event_combat = Some(EventCombatRequest::no_resume(
        EventEncounter::PunchOff,
        EventId::PunchOff,
        vec![
            sts2_core::OfferedReward::RandomRelic {
                rarity: None,
                is_wax: false,
            },
            sts2_core::OfferedReward::Potion(None),
        ],
    ));
    EventOutcome {
        finished: false,
        description: "Fight the constructs!".to_string(),
        effects: Vec::new(),
    }
}

/// Page 0: NAB, I_CAN_TAKE_THEM; page 1 (after I_CAN_TAKE_THEM): FIGHT.
fn punchoff_generate_options(state: &RunState, spec: &EventSpec) -> Vec<EventOptionInfo> {
    two_page_option_info(state, spec, 2)
}

const PUNCHOFF_OPTIONS: &[EventOptionSpec] = &[
    EventOptionSpec {
        option_id: "nab",
        loc_key: "PUNCH_OFF.pages.INITIAL.options.NAB",
        default_label: "Nab",
        default_description: "Gain Injury curse + relic reward",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: punchoff_nab,
    },
    EventOptionSpec {
        option_id: "take_them",
        loc_key: "PUNCH_OFF.pages.INITIAL.options.I_CAN_TAKE_THEM",
        default_label: "I Can Take Them",
        default_description: "Fight for relic + potion",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: punchoff_take_them,
    },
    // ── page 1 (I_CAN_TAKE_THEM): the Fight follow-up ──
    EventOptionSpec {
        option_id: "fight",
        loc_key: "PUNCH_OFF.pages.I_CAN_TAKE_THEM.options.FIGHT",
        default_label: "Fight",
        default_description: "Fight the constructs for a relic + potion.",
        default_vars: &[],
        vars_fn: None,
        title_vars_fn: None,
        handler: punchoff_fight,
    },
];

/// `PunchOff` spec. Pin: `shared.py:1268`. `is_allowed`: C#
/// `TotalFloor >= 6` at the pull ([`punch_off_allowed`]).
pub const PUNCH_OFF: EventSpec = EventSpec {
    id: EventId::PunchOff,
    python_name: "PunchOff",
    is_allowed: punch_off_allowed,
    generate_options: punchoff_generate_options,
    options: PUNCHOFF_OPTIONS,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grave_allowed_requires_own_exhaust_and_souls_eligibility() {
        use crate::cards::spec_index::card_pod_from_id;
        use sts2_core::cards::CardId;
        crate::cards::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.master_deck = vec![card_pod_from_id(CardId::StrikeIronclad, false).unwrap()];
        assert!(!(GRAVE_OF_THE_FORGOTTEN.is_allowed)(&rs), "no own Exhaust");
        rs.master_deck = vec![card_pod_from_id(CardId::Offering, false).unwrap()];
        assert!((GRAVE_OF_THE_FORGOTTEN.is_allowed)(&rs));
        assert!(grave_generate_options(&rs, &GRAVE_OF_THE_FORGOTTEN)[0].enabled);
        rs.master_deck[0]
            .enchantments
            .push((sts2_core::cards::EnchantmentId::Swift, 1));
        assert!(
            !(GRAVE_OF_THE_FORGOTTEN.is_allowed)(&rs),
            "already enchanted"
        );
        assert!(!grave_generate_options(&rs, &GRAVE_OF_THE_FORGOTTEN)[0].enabled);
        rs.master_deck = vec![card_pod_from_id(CardId::Offering, false).unwrap()];
        rs.master_deck[0].cost = -2;
        assert!(!(GRAVE_OF_THE_FORGOTTEN.is_allowed)(&rs), "unplayable");
        rs.master_deck.clear();
        assert!(!(GRAVE_OF_THE_FORGOTTEN.is_allowed)(&rs));
    }

    // ── BattlewornDummy event→combat (bead `sts2-rs-ck9z6`, GH #166) ─────
    // Each setting now STAGES a real fight (NOT a finished outcome); the
    // per-setting Resume reward is granted by `battleworn_dummy_resume` and
    // ONLY on a clean victory (a timed-out dummy yields nothing).

    #[test]
    fn battleworn_setting_1_stages_fight_then_resume_queues_potion() {
        let mut rs = RunState::new();
        let outcome = battleworn_setting_1(&mut rs);
        assert!(!outcome.finished, "the fight defers the event finish");
        let req = rs.event_combat.as_ref().expect("fight staged");
        assert!(req.resume);
        assert_eq!(
            req.encounter,
            EventEncounter::BattlewornDummy(BattlewornDummySetting::Setting1)
        );
        assert!(rs.pending_rewards.is_empty(), "no reward before victory");
        // Clean victory → one potion reward.
        battleworn_dummy_resume(&mut rs, BattlewornDummySetting::Setting1, true);
        assert_eq!(rs.pending_rewards.len(), 1);
        assert!(matches!(rs.pending_rewards[0], Reward::Potion(_)));
    }

    /// `potion-reward-model-differs` (TQTCLDYPU1@950): Setting1's potion is ONE
    /// `Rewards.NextItem` over the full pool, rolled at Resume and already
    /// populated (no rarity float, no sentinel left for the drain to re-roll).
    /// Fail-first: the sentinel reward had `potion_kind == None` and the
    /// Rewards cursor had not moved.
    #[test]
    fn battleworn_setting_1_rolls_one_full_pool_item_pre_populated() {
        let mut rs = RunState::new();
        let mut twin = rs.rng.rewards.clone();
        let before = rs.rng.rewards.counter();
        battleworn_dummy_resume(&mut rs, BattlewornDummySetting::Setting1, true);
        assert_eq!(
            rs.rng.rewards.counter() - before,
            1,
            "one int read, no rarity float"
        );
        let pool = crate::potions::all::roll_event_potion_full_pool(&mut twin, rs.character_id);
        let Reward::Potion(p) = &rs.pending_rewards[0] else {
            panic!("a potion reward");
        };
        assert_eq!(
            p.potion_kind, pool,
            "the item the full pool's one read picks"
        );
        assert!(p.potion_kind.is_some());
    }

    /// Negative control: a timed-out dummy (RanOutOfTime) rolls nothing.
    #[test]
    fn battleworn_setting_1_out_of_time_rolls_nothing() {
        let mut rs = RunState::new();
        let before = rs.rng.rewards.counter();
        battleworn_dummy_resume(&mut rs, BattlewornDummySetting::Setting1, false);
        assert_eq!(rs.rng.rewards.counter(), before);
        assert!(rs.pending_rewards.is_empty());
    }

    /// Negative control: Setting2 (upgrades on the event's own stream) leaves
    /// the Rewards stream alone.
    #[test]
    fn battleworn_setting_2_leaves_the_rewards_stream() {
        let mut rs = RunState::new();
        let before = rs.rng.rewards.counter();
        battleworn_dummy_resume(&mut rs, BattlewornDummySetting::Setting2, true);
        assert_eq!(rs.rng.rewards.counter(), before);
    }

    /// Each setting's option previews the HP of the dummy it will stage, so
    /// the number the player reads has to be the number the fight uses.
    #[test]
    fn battleworn_options_preview_each_settings_dummy_hp() {
        let rs = RunState::new();
        let hp: Vec<i64> = BATTLEWORN_OPTIONS
            .iter()
            .map(
                |opt| match crate::events::option_vars(&rs, opt).as_slice() {
                    [(_, sts2_core::EventVar::Int(v))] => *v,
                    other => panic!("expected one Int var, got {other:?}"),
                },
            )
            .collect();
        // C# `BattleFriendV{1,2,3}.MinInitialHp` — 75 / 150 / 300, no
        // ascension branch and no multiplayer scaling at one player.
        assert_eq!(hp, vec![75, 150, 300]);
    }

    #[test]
    fn battleworn_setting_3_resume_obtains_a_front_relic_directly() {
        crate::cards::dispatch::ensure_installed();
        use sts2_core::runstate::RelicRarity;
        let mut rs = RunState::new();
        rs.direct_event_relic_grant = true;
        // Seed every rarity bucket so whichever rarity the rewards stream rolls
        // finds a front relic to pull.
        rs.relic_grab_bag_by_rarity
            .insert(RelicRarity::Common, vec![RelicId::Astrolabe]);
        rs.relic_grab_bag_by_rarity
            .insert(RelicRarity::Uncommon, vec![RelicId::BlackStar]);
        rs.relic_grab_bag_by_rarity
            .insert(RelicRarity::Rare, vec![RelicId::CallingBell]);
        // The handler only stages the fight — no relic yet.
        let outcome = battleworn_setting_3(&mut rs);
        assert!(!outcome.finished);
        let before = rs.relics.len();
        // Clean victory → direct obtain (RelicCmd.Obtain), not a queued offer.
        battleworn_dummy_resume(&mut rs, BattlewornDummySetting::Setting3, true);
        assert_eq!(rs.relics.len(), before + 1);
        assert!(rs.pending_rewards.is_empty());
    }

    #[test]
    fn battleworn_resume_timeout_grants_nothing() {
        // RanOutOfTime (clean_victory = false) → no reward for any setting.
        let mut rs = RunState::new();
        battleworn_dummy_resume(&mut rs, BattlewornDummySetting::Setting1, false);
        assert!(rs.pending_rewards.is_empty());
        assert!(rs.relics.is_empty());
    }

    #[test]
    fn aroma_let_go_interactive_queues_event_stream_transform() {
        let mut rs = RunState::new();
        rs.interactive_card_transform = true;
        let outcome = (AROMA_OF_CHAOS.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.pending_deck_transforms.len(), 1);
        let pending = &rs.pending_deck_transforms[0];
        assert_eq!(pending.event_stream_roll, Some((AROMA_OF_CHAOS_STREAM, 0)));
        assert_eq!(pending.post_hp_loss, 0);
        assert!(pending.fixed_target.is_none());
    }

    /// v0.107.1 (`BattlewornDummy.cs:97`): the medium-setting upgrade shuffle
    /// migrated from `RunState.Rng.Niche` to the event's own `base.Rng`. The
    /// handler must upgrade 2 cards WITHOUT advancing the run-wide niche stream
    /// (the niche cursor would move under the v0.103.2 path).
    #[test]
    fn battleworn_setting_2_upgrades_off_event_stream_not_niche() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.master_deck.clear();
        for _ in 0..4 {
            rs.master_deck.push(
                crate::cards::spec_index::card_pod_from_id(
                    sts2_core::cards::CardId::StrikeIronclad,
                    false,
                )
                .expect("strike exists"),
            );
        }
        let niche_before = rs.rng.niche.counter();
        // The handler now only stages the fight (event→combat bridge,
        // `sts2-rs-ck9z6`); the upgrade lands on the clean-victory Resume.
        let outcome = battleworn_setting_2(&mut rs);
        assert!(!outcome.finished, "the fight defers the event finish");
        battleworn_dummy_resume(&mut rs, BattlewornDummySetting::Setting2, true);
        let upgraded = rs.master_deck.iter().filter(|c| c.upgraded).count();
        assert_eq!(upgraded, 2, "exactly 2 cards upgraded");
        assert_eq!(
            rs.rng.niche.counter(),
            niche_before,
            "v0.107.1 draws the shuffle from the event stream, not the niche stream",
        );
    }

    /// v0.107.1 (`DoorsOfLightAndDark.cs:28`): the Light-door upgrade shuffle
    /// migrated from `RunState.Rng.Niche` to the event's own `base.Rng`.
    #[test]
    fn doors_light_upgrades_off_event_stream_not_niche() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.master_deck.clear();
        for _ in 0..4 {
            rs.master_deck.push(
                crate::cards::spec_index::card_pod_from_id(
                    sts2_core::cards::CardId::StrikeIronclad,
                    false,
                )
                .expect("strike exists"),
            );
        }
        let niche_before = rs.rng.niche.counter();
        let outcome = doors_light(&mut rs);
        assert!(outcome.finished);
        let upgraded = rs.master_deck.iter().filter(|c| c.upgraded).count();
        assert_eq!(upgraded, 2, "Door of Light upgrades 2 cards");
        assert_eq!(
            rs.rng.niche.counter(),
            niche_before,
            "v0.107.1 draws the shuffle from the event stream, not the niche stream",
        );
    }

    /// The two event-local upgrade streams are seeded by distinct event Ids
    /// (`BATTLEWORN_DUMMY` vs `DOORS_OF_LIGHT_AND_DARK`), so for the same run
    /// seed the two events generally pick DIFFERENT cards — proving the draw is
    /// keyed on the event entry, not a shared stream. (Guards against both
    /// events accidentally collapsing onto one stream.)
    #[test]
    fn battleworn_and_doors_use_distinct_event_streams() {
        // RunState::new()'s run seed is fixed, so these draws are deterministic
        // (no flakiness). Distinct entry names seed distinct streams ⇒ the
        // full-range first draws differ.
        let mut a = event_rng(&RunState::new(), BATTLEWORN_DUMMY_STREAM);
        let mut b = event_rng(&RunState::new(), DOORS_OF_LIGHT_AND_DARK_STREAM);
        assert_ne!(
            a.next_int(0, i32::MAX - 1),
            b.next_int(0, i32::MAX - 1),
            "distinct event entries seed distinct streams",
        );
    }

    #[test]
    fn aroma_maintain_control_offers_one_upgrade_pick() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.master_deck.push(
            crate::cards::spec_index::card_pod_from_id(
                sts2_core::cards::CardId::StrikeIronclad,
                false,
            )
            .expect("strike exists"),
        );
        // Interactive mode (the run_parity / workbench host): MaintainControl
        // must queue a pick-1 upgrade card_select, NOT a stale UpgradeCards
        // reward the run loop never surfaces.
        rs.interactive_card_upgrade = true;
        let outcome = (AROMA_OF_CHAOS.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert!(
            rs.pending_rewards.is_empty(),
            "no stale UpgradeCards reward queued",
        );
        assert_eq!(
            rs.pending_deck_upgrades, 1,
            "queues a pick-1 upgrade card_select (C# FromDeckForUpgrade)",
        );
    }

    #[test]
    fn lost_wisp_spec_matches_python_pin() {
        assert_eq!(LOST_WISP.python_name, "LostWisp");
        assert_eq!(LOST_WISP.id.python_name(), "LostWisp");
        assert_eq!(LOST_WISP.options.len(), 2);
        assert_eq!(LOST_WISP.options[0].option_id, "claim");
        assert_eq!(LOST_WISP.options[1].option_id, "search");
    }

    #[test]
    fn lost_wisp_search_grants_60_gold() {
        let mut rs = RunState::new();
        let before = rs.gold;
        let outcome = (LOST_WISP.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.gold, before + LOST_WISP_GOLD);
    }

    #[test]
    fn lost_wisp_claim_queues_relic_reward() {
        let mut rs = RunState::new();
        let outcome = (LOST_WISP.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.relic_rewards.len(), 1);
        assert_eq!(rs.relic_rewards[0].relic_id, RelicId::LostWisp);
    }

    #[test]
    fn colossal_flower_three_dig_chain_ends_in_pollinous_core() {
        // C# `ColossalFlower.ReachDeeper` (`ColossalFlower.cs:60-80`): dig 1
        // deals 5, dig 2 deals 6, each presenting another 2-option page; the
        // third page's second option (`ObtainPollinousCore`) deals 7 and
        // grants the relic directly. The engine previously finished after
        // one dig (shadow census v8 seeds 1009/1046: DLL on page 2 while
        // the shadow was back at map_select).
        let mut rs = RunState::new();
        let hp_before = rs.current_hp;
        let reach_deeper = COLOSSAL_FLOWER.options[1].handler;

        let o1 = reach_deeper(&mut rs);
        assert!(!o1.finished, "dig 1 must present another page");
        assert_eq!(rs.current_hp, hp_before - 5);

        let o2 = reach_deeper(&mut rs);
        assert!(!o2.finished, "dig 2 must present the final page");
        assert_eq!(rs.current_hp, hp_before - 5 - 6);

        let o3 = reach_deeper(&mut rs);
        assert!(o3.finished, "third reach-deeper is ObtainPollinousCore");
        assert_eq!(rs.current_hp, hp_before - 5 - 6 - 7);
        assert_eq!(rs.relics, vec![RelicId::PollinousCore]);
    }

    #[test]
    fn colossal_flower_extract_pays_the_current_dig_tier() {
        // C# `_prizeCosts` = [35, 75, 135] indexed by `NumberOfDigs` —
        // extracting after one dig pays 75, after two digs 135
        // (`ExtractInstead`).
        let mut rs = RunState::new();
        let gold_before = rs.gold;
        let _ = (COLOSSAL_FLOWER.options[1].handler)(&mut rs);
        let out = (COLOSSAL_FLOWER.options[0].handler)(&mut rs);
        assert!(out.finished);
        assert_eq!(rs.gold, gold_before + 75, "one dig -> 75 gold tier");

        let mut rs2 = RunState::new();
        let gold2 = rs2.gold;
        let _ = (COLOSSAL_FLOWER.options[1].handler)(&mut rs2);
        let _ = (COLOSSAL_FLOWER.options[1].handler)(&mut rs2);
        let out2 = (COLOSSAL_FLOWER.options[0].handler)(&mut rs2);
        assert!(out2.finished);
        assert_eq!(
            rs2.gold,
            gold2 + 135,
            "two digs -> 135 gold (ExtractInstead)"
        );
    }

    #[test]
    fn trash_heap_dive_in_loses_8_hp_and_obtains_pool_relic_directly() {
        let mut rs = RunState::new();
        let hp_before = rs.current_hp;
        let outcome = (TRASH_HEAP.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.current_hp, hp_before - 8);
        // C# `RelicCmd.Obtain(base.Rng.NextItem(Relics))` — a SPECIFIC relic
        // from the fixed 5-pool, granted directly (no relic-reward screen).
        assert!(rs.pending_rewards.is_empty(), "no phantom relic reward");
        assert_eq!(rs.relics.len(), 1, "exactly one relic obtained directly");
        assert!(
            TRASH_HEAP_RELICS.contains(&rs.relics[0]),
            "obtained relic {:?} must be from the DiveIn pool",
            rs.relics[0],
        );
    }

    #[test]
    fn trash_heap_grab_gains_100_gold_and_adds_pool_card_directly() {
        let mut rs = RunState::new();
        let gold_before = rs.gold;
        let deck_before = rs.master_deck.len();
        let outcome = (TRASH_HEAP.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.gold, gold_before + 100);
        // C# `CardPileCmd.Add(CreateCard(base.Rng.NextItem(Cards)), Deck)` —
        // a SPECIFIC card from the fixed 10-pool, added directly (no reward
        // pick).
        assert!(rs.pending_rewards.is_empty(), "no phantom card reward");
        assert_eq!(
            rs.master_deck.len(),
            deck_before + 1,
            "exactly one card added"
        );
        let added = rs.master_deck.last().map(|c| c.card_id).unwrap();
        assert!(
            TRASH_HEAP_CARDS.contains(&added),
            "added card {added:?} must be from the Grab pool",
        );
    }

    /// The TrashHeap pick is seed-derived (the event RNG stream hashes the
    /// run seed + `Id.Entry`), so it's deterministic, not wall-clock random:
    /// two runs from the same seed pick the same card.
    #[test]
    fn trash_heap_grab_is_deterministic_for_a_seed() {
        let pick = || {
            let mut rs = RunState::new();
            (TRASH_HEAP.options[1].handler)(&mut rs);
            rs.master_deck.last().map(|c| c.card_id).unwrap()
        };
        assert_eq!(pick(), pick());
    }

    #[test]
    fn infested_study_adds_one_power_card_directly() {
        let mut rs = RunState::new();
        let before = rs.master_deck.len();
        let outcome = (INFESTED_AUTOMATON.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.master_deck.len(), before + 1, "exactly one card added");
        assert!(rs.pending_rewards.is_empty(), "no phantom card reward");
        let added = rs.master_deck.last().map(|c| c.card_id).unwrap();
        assert_eq!(
            crate::cards::spec_index::card_pod_from_id(added, false).map(|c| c.card_type),
            Some(sts2_core::cards::CardType::Power),
            "Study grants a Power-type card",
        );
    }

    #[test]
    fn infested_touch_core_adds_one_zero_cost_card_directly() {
        let mut rs = RunState::new();
        let before = rs.master_deck.len();
        let outcome = (INFESTED_AUTOMATON.options[1].handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.master_deck.len(), before + 1, "exactly one card added");
        assert!(rs.pending_rewards.is_empty(), "no phantom card reward");
        let added = rs.master_deck.last().map(|c| c.card_id).unwrap();
        assert_eq!(
            crate::cards::spec_index::card_pod_from_id(added, false).map(|c| c.cost),
            Some(0),
            "TouchCore grants a 0-cost (non-X) card",
        );
    }

    /// Amalgamator CombineStrikes removes 2 Basic Strikes and adds Ultimate
    /// Strike directly in noninteractive mode — net deck size −1, no
    /// phantom card reward. Interactive instance selection is covered by
    /// tests/amalgamator_selection.rs.
    #[test]
    fn amalgamator_combine_strikes_swaps_two_strikes_for_ultimate() {
        use sts2_core::cards::{CardId, CardTag};
        let mut rs = RunState::new();
        // Seed a deck with 3 Basic Strikes so the precondition holds.
        rs.master_deck.clear();
        for _ in 0..3 {
            if let Some(c) =
                crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false)
            {
                rs.master_deck.push(c);
            }
        }
        let before = rs.master_deck.len();
        let outcome = (AMALGAMATOR.options[0].handler)(&mut rs);
        assert!(outcome.finished);
        assert!(rs.pending_rewards.is_empty(), "no phantom card reward");
        assert_eq!(rs.master_deck.len(), before - 1, "−2 Strikes, +1 Ultimate");
        assert_eq!(
            rs.master_deck
                .iter()
                .filter(|c| c.has_tag(CardTag::Strike) && c.card_id == CardId::StrikeIronclad)
                .count(),
            1,
            "two Basic Strikes removed",
        );
        assert_eq!(
            rs.master_deck
                .iter()
                .filter(|c| c.card_id == CardId::UltimateStrike)
                .count(),
            1,
            "one Ultimate Strike added",
        );
    }

    #[test]
    fn trash_heap_is_allowed_gates_on_hp_above_5() {
        let mut rs = RunState::new();
        rs.current_hp = 6;
        assert!((TRASH_HEAP.is_allowed)(&rs));
        rs.current_hp = 5;
        assert!(!(TRASH_HEAP.is_allowed)(&rs));
        rs.current_hp = 1;
        assert!(!(TRASH_HEAP.is_allowed)(&rs));
    }

    #[test]
    fn lost_wisp_generate_options_interpolates_gold() {
        let rs = RunState::new();
        let opts = (LOST_WISP.generate_options)(&rs, &LOST_WISP);
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0].option_id, "claim");
        assert_eq!(opts[1].option_id, "search");
        assert!(opts[1].description.contains(&LOST_WISP_GOLD.to_string()));
    }

    /// pool-placeholders: Colorful Philosophers offers three of the other
    /// characters' pools in `CardPoolColorOrder` (Necrobinder, Ironclad,
    /// Regent, Silent, Defect), keyed by the pool's `EnergyColorName`, one of
    /// the four removed by the event's `Rng.NextInt(4)`; the chosen option
    /// offers three card rewards from that pool (Common, Uncommon, Rare).
    #[test]
    fn colorful_philosophers_offers_three_other_character_pools() {
        use sts2_core::potions::CharacterId;
        let order = ["necrobinder", "ironclad", "regent", "silent", "defect"];
        let mut removed_seen = std::collections::BTreeSet::new();
        for seed in 0..24 {
            let mut rs = RunState::new();
            rs.rng.seed = seed;
            rs.character_id = CharacterId::Ironclad;
            let opts = (COLORFUL_PHILOSOPHERS.generate_options)(&rs, &COLORFUL_PHILOSOPHERS);
            let ids: Vec<&str> = opts.iter().map(|o| o.option_id).collect();
            let mut want: Vec<&str> = order.iter().copied().filter(|o| *o != "ironclad").collect();
            let mut rng = sts2_core::dotnet_rng::Rng::with_name(seed, "COLORFUL_PHILOSOPHERS");
            let removed = want.remove(rng.next_int_exclusive(0, 4) as usize);
            removed_seen.insert(removed);
            assert_eq!(ids, want, "seed {seed}");
            for o in &opts {
                assert_eq!(
                    o.loc_key,
                    format!(
                        "COLORFUL_PHILOSOPHERS.pages.INITIAL.options.{}",
                        o.option_id.to_uppercase()
                    )
                );
                assert!(o.enabled);
            }
        }
        assert_eq!(removed_seen.len(), 4, "each pool can be the one removed");
        crate::cards::dispatch::install_default_registry();
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        let opt = COLORFUL_PHILOSOPHERS
            .options
            .iter()
            .find(|o| o.option_id == "defect")
            .unwrap();
        let outcome = (opt.handler)(&mut rs);
        assert!(outcome.finished);
        assert_eq!(rs.pending_rewards.len(), 3);
        for (reward, rank) in rs.pending_rewards.iter().zip([2u8, 3, 4]) {
            let Reward::Card(c) = reward else {
                panic!("a card reward, got {reward:?}")
            };
            assert_eq!(c.options.len(), 3);
            for id in &c.options {
                assert_eq!(crate::cards::spec_index::card_rarity_rank_csharp(*id), rank);
                assert!(
                    crate::cards::spec_index::specs_for_character(CharacterId::Defect)
                        .iter()
                        .any(|f| f().id == *id)
                );
            }
        }
    }

    // ── Wave A coverage (bead sts2-rs-6ynt.4.4) ──────────────────────

    /// Spec sanity for every wave-A event: id+python_name agree;
    /// option_ids match the Python pin literals; each handler runs
    /// without panicking.
    #[test]
    fn wave_a_specs_match_python_pins() {
        let cases: &[(&EventSpec, EventId, &str, &[&str])] = &[
            (
                &ABYSSAL_BATHS,
                EventId::AbyssalBaths,
                "AbyssalBaths",
                &["immerse", "abstain", "linger", "exit_baths"],
            ),
            (
                &AMALGAMATOR,
                EventId::Amalgamator,
                "Amalgamator",
                &["combine_strikes", "combine_defends"],
            ),
            (
                &AROMA_OF_CHAOS,
                EventId::AromaOfChaos,
                "AromaOfChaos",
                &["let_go", "maintain_control"],
            ),
            (
                &BATTLEWORN_DUMMY,
                EventId::BattlewornDummy,
                "BattlewornDummy",
                &["setting_1", "setting_2", "setting_3"],
            ),
            (
                &BUGSLAYER,
                EventId::Bugslayer,
                "Bugslayer",
                &["exterminate", "squash"],
            ),
            (
                &BYRDONIS_NEST,
                EventId::ByrdonisNest,
                "ByrdonisNest",
                &["eat", "take"],
            ),
            (
                &COLORFUL_PHILOSOPHERS,
                EventId::ColorfulPhilosophers,
                "ColorfulPhilosophers",
                &["necrobinder", "ironclad", "regent", "silent", "defect"],
            ),
            (
                &COLOSSAL_FLOWER,
                EventId::ColossalFlower,
                "ColossalFlower",
                &["extract", "reach_deeper"],
            ),
            (
                &DARV,
                EventId::Darv,
                "Darv",
                &["relic_1", "relic_2", "relic_3"],
            ),
            (
                &DENSE_VEGETATION,
                EventId::DenseVegetation,
                "DenseVegetation",
                &["trudge_on", "rest", "fight"],
            ),
            (
                &DOORS_OF_LIGHT_AND_DARK,
                EventId::DoorsOfLightAndDark,
                "DoorsOfLightAndDark",
                &["light", "dark"],
            ),
            (
                &DROWNING_BEACON,
                EventId::DrowningBeacon,
                "DrowningBeacon",
                &["bottle", "climb"],
            ),
            (
                &GRAVE_OF_THE_FORGOTTEN,
                EventId::GraveOfTheForgotten,
                "GraveOfTheForgotten",
                &["confront", "accept"],
            ),
            (
                &HUNGRY_FOR_MUSHROOMS,
                EventId::HungryForMushrooms,
                "HungryForMushrooms",
                &["big", "fragrant"],
            ),
            (
                &INFESTED_AUTOMATON,
                EventId::InfestedAutomaton,
                "InfestedAutomaton",
                &["study", "touch_core"],
            ),
            (
                &NONUPEIPE,
                EventId::Nonupeipe,
                "Nonupeipe",
                &["relic_1", "relic_2", "relic_3"],
            ),
            (
                &OROBAS,
                EventId::Orobas,
                "Orobas",
                &["relic_1", "relic_2", "relic_3"],
            ),
            (
                &PAEL,
                EventId::Pael,
                "Pael",
                &["relic_1", "relic_2", "relic_3"],
            ),
            (
                &PUNCH_OFF,
                EventId::PunchOff,
                "PunchOff",
                &["nab", "take_them", "fight"],
            ),
        ];
        assert_eq!(cases.len(), 19, "wave A is 19 events");
        for (spec, id, name, option_ids) in cases {
            assert_eq!(spec.id, *id, "{name}: id mismatch");
            assert_eq!(spec.python_name, *name);
            assert_eq!(spec.id.python_name(), *name);
            assert_eq!(spec.options.len(), option_ids.len(), "{name}: option count");
            for (opt, want) in spec.options.iter().zip(option_ids.iter()) {
                assert_eq!(opt.option_id, *want, "{name}: option_id");
            }
            for opt in spec.options {
                let mut rs = RunState::new();
                // The Ancient stubs roll their relic from the event the handler
                // belongs to (`resolve` has bound it): lane F's F8.
                if matches!(
                    *id,
                    EventId::Nonupeipe | EventId::Orobas | EventId::Pael | EventId::Darv
                ) {
                    rs.current_event = Some(*id);
                }
                let outcome = (opt.handler)(&mut rs);
                // BattlewornDummy options STAGE an event→combat fight (bead
                // `sts2-rs-ck9z6`, GH #166): a NOT-finished outcome that sets
                // `event_combat`, with the event resumed after the fight.
                if *id == EventId::BattlewornDummy {
                    assert!(
                        !outcome.finished,
                        "{name}/{}: stages a fight",
                        opt.option_id
                    );
                    assert!(
                        rs.event_combat.is_some(),
                        "{name}/{}: must stage event_combat",
                        opt.option_id
                    );
                } else if *id == EventId::AbyssalBaths
                    && matches!(opt.option_id, "immerse" | "linger")
                {
                    // Each soak opens the Linger / Exit Baths page
                    // (C# `AbyssalBaths.Immerse`, `Linger`).
                    assert!(
                        !outcome.finished,
                        "{name}/{}: presents the next page",
                        opt.option_id
                    );
                } else if matches!(
                    (*id, opt.option_id),
                    (EventId::DenseVegetation, "rest" | "fight")
                        | (EventId::PunchOff, "take_them" | "fight")
                ) {
                    // Rest / I_CAN_TAKE_THEM open the one-option FIGHT page;
                    // FIGHT stages the no-resume fight (sts2-agent fork).
                    assert!(
                        !outcome.finished,
                        "{name}/{}: a page or a fight",
                        opt.option_id
                    );
                } else if *id == EventId::ColossalFlower && opt.option_id == "reach_deeper" {
                    // Multi-page dig chain (C# `ColossalFlower.ReachDeeper`):
                    // the first call presents page 2, not a finished event.
                    assert!(
                        !outcome.finished,
                        "{name}/{}: dig 1 presents another page",
                        opt.option_id
                    );
                } else {
                    assert!(
                        outcome.finished,
                        "{name}/{}: must be finished",
                        opt.option_id
                    );
                }
                assert!(
                    !outcome.description.is_empty(),
                    "{name}/{}: empty desc",
                    opt.option_id
                );
            }
        }
    }

    #[test]
    fn abyssal_baths_immerse_gains_2_max_hp_and_loses_3_hp() {
        let mut rs = RunState::new();
        let (max_hp_before, hp_before) = (rs.max_hp, rs.current_hp);
        let outcome = (ABYSSAL_BATHS.options[0].handler)(&mut rs);
        // The game opens the Linger / Exit Baths page (sts2-agent fork).
        assert!(!outcome.finished);
        assert_eq!(rs.event_state.page, 1);
        assert_eq!(rs.max_hp, max_hp_before + ABYSSAL_BATHS_MAX_HP_GAIN);
        // gain_max_hp bumps current_hp too (+2), then lose_hp removes 3:
        // net current_hp delta = +2 - 3 = -1.
        assert_eq!(rs.current_hp, hp_before - 1);
    }

    #[test]
    fn abyssal_baths_abstain_heals_capped_at_max_hp() {
        let mut rs = RunState::new();
        rs.current_hp = 75;
        let _ = (ABYSSAL_BATHS.options[1].handler)(&mut rs);
        // 75 + 10 = 85, capped at max_hp=80.
        assert_eq!(rs.current_hp, rs.max_hp);
    }

    #[test]
    fn byrdonis_eat_gains_7_max_hp() {
        let mut rs = RunState::new();
        let before = rs.max_hp;
        (BYRDONIS_NEST.options[0].handler)(&mut rs);
        assert_eq!(rs.max_hp, before + BYRDONIS_NEST_MAX_HP_GAIN);
    }

    /// C# `ByrdonisNest.Take` adds the single specific Byrdonis Egg card to the
    /// deck (`CreateCard<ByrdonisEgg>` → `CardPileCmd.Add(.., Deck)`) — NOT a
    /// card-reward choice. Regression for the run-parity `map/state/phase`
    /// cluster (Rust surfaced a 3-card reward, one decision step behind C#).
    #[test]
    fn byrdonis_take_adds_one_byrdonis_egg_directly() {
        let mut rs = RunState::new();
        let before = rs.master_deck.len();
        let outcome = (BYRDONIS_NEST.options[1].handler)(&mut rs);
        assert_eq!(
            rs.master_deck.len(),
            before + 1,
            "exactly one card added, no reward offering"
        );
        assert_eq!(
            rs.master_deck.last().map(|c| c.card_id),
            Some(sts2_core::cards::CardId::ByrdonisEgg),
            "the added card is the Byrdonis Egg"
        );
        assert!(
            rs.pending_rewards.is_empty(),
            "no card-reward choice surfaced"
        );
        let _ = outcome;
    }

    /// C# `Bugslayer.AddAndPreview<T>` adds ONE specific card directly to the
    /// deck per option (Exterminate / Squash) — NOT a card-reward choice.
    /// Regression for the run-parity deck-composition cluster (Regent:1021:
    /// Rust surfaced a 3-card reward, one decision step behind C#). Bead
    /// `sts2-rs-RP26`.
    #[test]
    fn bugslayer_options_add_one_specific_card_directly() {
        for (idx, want) in [
            (0usize, sts2_core::cards::CardId::Exterminate),
            (1usize, sts2_core::cards::CardId::Squash),
        ] {
            let mut rs = RunState::new();
            let before = rs.master_deck.len();
            let _ = (BUGSLAYER.options[idx].handler)(&mut rs);
            assert_eq!(
                rs.master_deck.len(),
                before + 1,
                "exactly one card added, no reward offering"
            );
            assert_eq!(
                rs.master_deck.last().map(|c| c.card_id),
                Some(want),
                "option {idx} adds the specific card"
            );
            assert!(
                rs.pending_rewards.is_empty(),
                "no card-reward choice surfaced"
            );
        }
    }

    #[test]
    fn colossal_flower_extract_gains_35_gold() {
        let mut rs = RunState::new();
        let before = rs.gold;
        (COLOSSAL_FLOWER.options[0].handler)(&mut rs);
        assert_eq!(rs.gold, before + COLOSSAL_FLOWER_FIRST_GOLD);
    }

    #[test]
    fn colossal_flower_allowed_only_at_hp_ge_19() {
        let mut rs = RunState::new();
        rs.current_hp = COLOSSAL_FLOWER_HP_GATE;
        assert!((COLOSSAL_FLOWER.is_allowed)(&rs));
        rs.current_hp = COLOSSAL_FLOWER_HP_GATE - 1;
        assert!(!(COLOSSAL_FLOWER.is_allowed)(&rs));
    }

    #[test]
    fn drowning_climb_loses_13_max_hp_and_queues_fresnel_lens() {
        let mut rs = RunState::new();
        let before = rs.max_hp;
        (DROWNING_BEACON.options[1].handler)(&mut rs);
        assert_eq!(rs.max_hp, before - DROWNING_BEACON_MAX_HP_LOSS);
        assert_eq!(rs.relic_rewards.len(), 1);
        assert_eq!(rs.relic_rewards[0].relic_id, RelicId::FresnelLens);
    }

    #[test]
    fn hungry_fragrant_loses_15_hp_and_queues_fragrant_mushroom() {
        let mut rs = RunState::new();
        let hp_before = rs.current_hp;
        (HUNGRY_FOR_MUSHROOMS.options[1].handler)(&mut rs);
        assert_eq!(rs.current_hp, hp_before - HUNGRY_FRAGRANT_DAMAGE);
        assert_eq!(rs.relic_rewards.len(), 1);
        assert_eq!(rs.relic_rewards[0].relic_id, RelicId::FragrantMushroom);
    }

    #[test]
    fn grave_accept_queues_forgotten_soul() {
        let mut rs = RunState::new();
        (GRAVE_OF_THE_FORGOTTEN.options[1].handler)(&mut rs);
        assert_eq!(rs.relic_rewards.len(), 1);
        assert_eq!(rs.relic_rewards[0].relic_id, RelicId::ForgottenSoul);
    }

    /// Bead `sts2-rs-RP27`: the audited card-grant/removal events match C#
    /// (specific curse added directly, or a removal pick) instead of surfacing
    /// a phantom 3-card ADD reward. Regression for the deck-composition class.
    #[test]
    fn doors_dark_offers_one_removal_not_a_card_add() {
        let mut rs = RunState::new();
        let before = rs.master_deck.len();
        (DOORS_OF_LIGHT_AND_DARK.options[1].handler)(&mut rs);
        assert_eq!(rs.master_deck.len(), before, "dark door adds no card");
        assert_eq!(
            rs.pending_rewards,
            vec![Reward::RemoveCard { count: 1 }],
            "dark door is a removal, not a card reward"
        );
    }

    #[test]
    fn grave_confront_adds_decay_directly_no_card_reward() {
        let mut rs = RunState::new();
        let before = rs.master_deck.len();
        (GRAVE_OF_THE_FORGOTTEN.options[0].handler)(&mut rs);
        assert_eq!(rs.master_deck.len(), before + 1, "exactly one card added");
        assert_eq!(
            rs.master_deck.last().map(|c| c.card_id),
            Some(sts2_core::cards::CardId::Decay),
        );
        assert!(rs.pending_rewards.is_empty(), "no phantom card reward");
    }

    #[test]
    fn punchoff_nab_adds_injury_directly_plus_relic() {
        let mut rs = RunState::new();
        let before = rs.master_deck.len();
        (PUNCH_OFF.options[0].handler)(&mut rs);
        assert_eq!(rs.master_deck.len(), before + 1);
        assert_eq!(
            rs.master_deck.last().map(|c| c.card_id),
            Some(sts2_core::cards::CardId::Injury),
        );
        assert!(
            rs.pending_rewards
                .iter()
                .all(|r| !matches!(r, Reward::Card(_))),
            "no card reward; only the relic reward",
        );
        assert!(rs
            .pending_rewards
            .iter()
            .any(|r| matches!(r, Reward::Relic(_))));
    }

    #[test]
    fn punchoff_take_them_grants_relic_and_potion_not_a_card() {
        // C# TakeThem only opens the I_CAN_TAKE_THEM page; its FIGHT stages
        // the fight whose extras are a relic and a potion, no card.
        let mut rs = RunState::new();
        let before = rs.master_deck.len();
        let outcome = (PUNCH_OFF.options[1].handler)(&mut rs);
        assert!(!outcome.finished);
        assert_eq!(rs.event_state.page, 1);
        assert!(rs.pending_rewards.is_empty());
        let page1 = (PUNCH_OFF.generate_options)(&rs, &PUNCH_OFF);
        assert_eq!(page1.len(), 1);
        assert_eq!(page1[0].option_id, "fight");
        let outcome = (PUNCH_OFF.options[2].handler)(&mut rs);
        assert!(!outcome.finished);
        assert_eq!(rs.master_deck.len(), before, "take_them adds no card");
        let req = rs.event_combat.as_ref().expect("the fight staged");
        assert!(!req.resume);
        assert_eq!(req.encounter, EventEncounter::PunchOff);
        assert_eq!(
            req.victory_rewards,
            vec![
                sts2_core::OfferedReward::RandomRelic {
                    rarity: None,
                    is_wax: false
                },
                sts2_core::OfferedReward::Potion(None),
            ],
            "a relic and a potion, no card"
        );
    }

    #[test]
    fn dense_vegetation_trudge_loses_8_hp_and_gains_event_gold() {
        let mut rs = RunState::new();
        let hp_before = rs.current_hp;
        let gold_before = rs.gold;
        (DENSE_VEGETATION.options[0].handler)(&mut rs);
        assert_eq!(rs.current_hp, hp_before - DENSE_VEGETATION_TRUDGE_DAMAGE);
        // C# CalculateVars gold roll: NextInt(61, 100) max-exclusive.
        let gained = rs.gold - gold_before;
        assert!((61..=99).contains(&gained), "gold in [61,99], got {gained}");
        assert!(rs.pending_rewards.is_empty(), "no card-removal placeholder");
    }

    #[test]
    fn pael_relic_choices_all_grant_a_rolled_pael_relic() {
        // Lane F's F8: Pael's options are its rolled pool (no longer three
        // fixed `PAELS_*` stubs); each slot grants the relic it presents.
        for slot in 0..3 {
            let mut rs = RunState::new();
            rs.direct_event_relic_grant = true;
            rs.current_event = Some(EventId::Pael);
            let offers = rs.roll_ancient_event_offers(sts2_core::run_room_gen::AncientId::Pael);
            let relic = offers[slot];
            let held = rs.relics.len();
            (PAEL.options[slot].handler)(&mut rs);
            assert_eq!(rs.relics.len(), held + 1);
            assert_eq!(rs.relics.last(), Some(&relic));
        }
    }

    #[test]
    fn wave_a_dispatches_through_resolve_for_every_event() {
        // End-to-end smoke through the dispatcher: every wave-A event
        // enters its room, presents options, resolves the first one,
        // and clears current_event.
        use crate::events::dispatch::{present, resolve};
        let ids = [
            EventId::AbyssalBaths,
            EventId::Amalgamator,
            EventId::AromaOfChaos,
            EventId::BattlewornDummy,
            EventId::Bugslayer,
            EventId::ByrdonisNest,
            EventId::ColorfulPhilosophers,
            EventId::ColossalFlower,
            EventId::Darv,
            EventId::DenseVegetation,
            EventId::DoorsOfLightAndDark,
            EventId::DrowningBeacon,
            EventId::GraveOfTheForgotten,
            EventId::HungryForMushrooms,
            EventId::InfestedAutomaton,
            EventId::Nonupeipe,
            EventId::Orobas,
            EventId::Pael,
            EventId::PunchOff,
        ];
        for id in ids {
            let mut rs = RunState::new();
            // Set HP high enough to pass ColossalFlower's gate.
            rs.current_hp = 80;
            rs.enter_event_room(id);
            let opts = present(&rs).unwrap_or_else(|| panic!("{id:?}: present returned None"));
            assert!(!opts.is_empty(), "{id:?}: no options");
            let first_opt_id = opts[0].option_id;
            let outcome = resolve(&mut rs, first_opt_id)
                .unwrap_or_else(|| panic!("{id:?}: resolve returned None"));
            // BattlewornDummy's first option STAGES an event→combat fight
            // (bead `sts2-rs-ck9z6`, GH #166): a NOT-finished outcome that
            // leaves the event open across the nested combat.
            if id == EventId::AbyssalBaths {
                // Immerse opens the Linger / Exit Baths page (sts2-agent fork).
                assert!(!outcome.finished, "{id:?}: opens its next page");
                assert_eq!(rs.current_event, Some(id), "{id:?}: event stays open");
            } else if id == EventId::BattlewornDummy {
                assert!(!outcome.finished, "{id:?}: stages a fight");
                assert!(rs.event_combat.is_some(), "{id:?}: staged event_combat");
                assert_eq!(rs.current_event, Some(id), "{id:?}: event stays open");
            } else {
                assert!(outcome.finished, "{id:?}: should finish");
                assert!(
                    rs.current_event.is_none(),
                    "{id:?}: current_event not cleared"
                );
            }
        }
    }

    // ── Amalgamator (mutant coverage) ────────────────────────────────

    /// Build a deck card with an explicit `card_id` (drives the installed
    /// rank/removable lookups) and an explicit tag set.
    fn deck_card(
        card_id: sts2_core::cards::CardId,
        tag: sts2_core::cards::CardTag,
    ) -> sts2_core::cards::Card {
        let mut c = sts2_core::cards::Card::pod_defaults();
        c.card_id = card_id;
        c.tags = sts2_core::cards::CardTags::one(tag);
        c
    }

    #[test]
    fn amalgamator_removes_basic_strikes_only_up_to_count() {
        // Kills the `removed += 1` arithmetic mutants (`-=`, `*=`): two basic
        // Strikes must yield exactly `removed == 2`.
        crate::cards::dispatch::ensure_installed();
        use sts2_core::cards::{CardId, CardTag};
        let mut rs = RunState::new();
        rs.master_deck.clear();
        rs.master_deck
            .push(deck_card(CardId::StrikeIronclad, CardTag::Strike));
        rs.master_deck
            .push(deck_card(CardId::StrikeIronclad, CardTag::Strike));
        let removed = amalgamator_remove_basic_tagged(&mut rs, CardTag::Strike, 2);
        assert_eq!(removed, 2, "both basic Strikes removed");
        assert!(rs.master_deck.is_empty(), "deck emptied");
    }

    #[test]
    fn amalgamator_requires_basic_rarity_for_removal() {
        // Kills `== 1` → `!= 1` on the `is_basic` predicate: a Basic (rank 1)
        // Strike IS removed under `==`, but a `!=` mutant would skip it.
        crate::cards::dispatch::ensure_installed();
        use sts2_core::cards::{CardId, CardTag};
        let mut rs = RunState::new();
        rs.master_deck.clear();
        rs.master_deck
            .push(deck_card(CardId::StrikeIronclad, CardTag::Strike));
        let removed = amalgamator_remove_basic_tagged(&mut rs, CardTag::Strike, 2);
        assert_eq!(removed, 1, "the single basic Strike is removed");
        assert!(rs.master_deck.is_empty());
    }

    #[test]
    fn amalgamator_filter_is_conjunction_not_disjunction() {
        // Kills both `&&` → `||` mutants. A non-basic (Common rank) but
        // Strike-tagged + removable card passes the tag and removable legs
        // yet FAILS the basic leg, so the original AND removes nothing.
        // Either `||` mutant would (wrongly) remove it.
        crate::cards::dispatch::ensure_installed();
        use sts2_core::cards::{CardId, CardTag};
        let mut rs = RunState::new();
        rs.master_deck.clear();
        // PerfectedStrike is Common (rank 2), removable, here tagged Strike.
        rs.master_deck
            .push(deck_card(CardId::PerfectedStrike, CardTag::Strike));
        let removed = amalgamator_remove_basic_tagged(&mut rs, CardTag::Strike, 2);
        assert_eq!(removed, 0, "non-basic Strike must NOT be removed");
        assert_eq!(rs.master_deck.len(), 1, "deck untouched");
    }

    // ── DenseVegetation rest (mutant coverage) ───────────────────────

    #[test]
    fn dense_vegetation_rest_heals_30_percent_of_max_hp() {
        // Kills the `max_hp * NUM / DEN` arithmetic mutants (`*`→`+`/`/`,
        // `/`→`%`/`*`). With max_hp=80 the base is 80*3/10 = 24; current_hp
        // is set low so the heal is base-bound (not max-HP-capped).
        let mut rs = RunState::new();
        rs.max_hp = 80;
        rs.current_hp = 1;
        let outcome = dense_vegetation_rest(&mut rs);
        // C# Rest opens the REST page (FIGHT); the event goes on.
        assert!(!outcome.finished);
        assert_eq!(rs.event_state.page, 1);
        // 24 HP healed: 1 -> 25. The `+`/`*` mutants overshoot (>=79 healed),
        // the `/`/`%` mutants undershoot (<=2 healed); all differ from 24.
        assert_eq!(rs.current_hp, 25, "healed exactly 30% of max HP");
    }

    // event-deck-add-hooks: an event's deck add is the game's CardPileCmd.Add
    // to the Deck (or AddCurseToDeck): Hook.ModifyCardBeingAddedToDeck, then
    // AfterCardChangedPiles per card (Lucky Fysh +15 gold each, Darkstone
    // Periapt +6 max HP per curse).

    fn hooked(relics: &[sts2_core::relics::RelicId]) -> RunState {
        crate::cards::dispatch::install_default_registry();
        crate::relics::dispatch::ensure_installed();
        let mut rs = RunState::new();
        rs.relics.extend_from_slice(relics);
        rs.gold = 500;
        rs
    }

    fn fysh_periapt() -> RunState {
        use sts2_core::relics::RelicId;
        hooked(&[RelicId::LuckyFysh, RelicId::DarkstonePeriapt])
    }

    /// Run `handler` on `rs`; assert `cards` cards, `curses` of them curses,
    /// went in with their hooks (gold and max HP beside the event's own).
    fn assert_hooked_adds(
        mut rs: RunState,
        handler: fn(&mut RunState) -> EventOutcome,
        cards: i32,
        curses: i32,
        event_gold: i32,
    ) -> RunState {
        let (deck, gold, max_hp) = (rs.master_deck.len() as i32, rs.gold, rs.max_hp);
        let _ = handler(&mut rs);
        assert_eq!(rs.master_deck.len() as i32 - deck, cards, "cards added");
        assert_eq!(
            rs.gold - gold - event_gold,
            15 * cards,
            "Lucky Fysh per card"
        );
        assert_eq!(
            rs.max_hp - max_hp,
            6 * curses,
            "Darkstone Periapt per curse"
        );
        rs
    }

    #[test]
    fn event_deck_adds_run_the_deck_add_hooks_shared() {
        assert_hooked_adds(fysh_periapt(), trash_heap_grab, 1, 0, 100);
        assert_hooked_adds(fysh_periapt(), bugslayer_exterminate, 1, 0, 0);
        assert_hooked_adds(fysh_periapt(), byrdonis_take, 1, 0, 0);
        assert_hooked_adds(fysh_periapt(), grave_confront, 1, 1, 0);
        assert_hooked_adds(fysh_periapt(), infested_study, 1, 0, 0);
        assert_hooked_adds(fysh_periapt(), infested_touch_core, 1, 0, 0);
    }
}
