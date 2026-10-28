//! Run-side events: discriminant + presentation/outcome POD set —
//! bead `sts2-rs-6ynt.4.1` (P7 events framework).
//!
//! This module is the sts2-core half of the events framework. It
//! owns:
//!
//! - [`EventId`] — the per-event discriminant (mirrors Python's
//!   `EventModel.event_id` class attribute; ~63 variants land
//!   incrementally as per-event bodies arrive under the
//!   `sts2-rs-6ynt.4.*` sub-beads).
//! - [`EventOptionInfo`] — one choice presented to the agent for a
//!   given event entry. Mirrors Python `EventOption`
//!   (`sts2-rl-agent@01c094d:sts2_env/run/events.py::EventOption`).
//! - [`EventOutcome`] — the post-resolution payload mirroring the
//!   subset of `EventResult` that this bead exercises (`finished` +
//!   `description`; `next_options` + `rewards` extend in follow-ups).
//!
//! The static [`EventSpec`] template + handler-function dispatch
//! tables live in `sts2-content::events` (mirrors the
//! `RelicSpec` / `PotionSpec` split). [`RunState`] gates entry into
//! event rooms through [`RunState::enter_event_room`]
//! ([`crate::runstate`]).

// ── EventId ───────────────────────────────────────────────────────────

/// Run-side event discriminant. Mirrors Python `EventModel.event_id`
/// at `sts2-rl-agent@01c094d:sts2_env/run/events.py:30`.
///
/// Variants land incrementally: bead `sts2-rs-6ynt.4.1` ships the
/// framework + the two proof-of-shape events
/// (`LostWisp` shared, `TrashHeap` shared); per-act sub-beads
/// (`6ynt.4.2`–`6ynt.4.7`) fill the remaining ~61 entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EventId {
    /// `LostWisp` — shared event (acts 0/1/2). Choices:
    /// `claim` (gain `LOST_WISP` relic + `Decay` curse) /
    /// `search` (gain ~60 gold). Bead `sts2-rs-6ynt.4.1`.
    LostWisp,
    /// `TrashHeap` — shared event (acts 0/1/2). Choices:
    /// `dive_in` (lose 8 HP, offer a random relic) /
    /// `grab` (gain 100 gold + offer a card reward). Bead
    /// `sts2-rs-6ynt.4.1`.
    TrashHeap,
    /// `TheArchitect` — act3 special end-of-run transition. Triggered
    /// by act-transition logic (not the random pool —
    /// `is_allowed` returns `false`). Single option `proceed`; the
    /// Python `EventResult` carries `event_combat_setup="the_architect"`
    /// + `post_combat_phase="RUN_OVER"` which the run-loop driver
    /// will read once those surfaces land. Bead `sts2-rs-6ynt.4.3`.
    TheArchitect,
    /// `WarHistorianRepy` — act3 LantenKey-gated event. Disabled
    /// (`is_allowed` returns `false` at pin `01c094d`; spawns only
    /// when the `LANTERN_KEY` card is in the deck, which the Rust
    /// port doesn't yet model). Choices: `unlock_cage` (free Repy,
    /// gain `HISTORY_COURSE` relic, remove all `LANTERN_KEY` copies
    /// from deck) / `unlock_chest` (remove `LANTERN_KEY`s, gain 2
    /// potion-rewards + 2 relic-rewards). Bead `sts2-rs-6ynt.4.3`.
    WarHistorianRepy,
    /// `Neow` — act3 starting event (relic selection). Triggered at
    /// run start by game logic (`is_allowed` returns `false`).
    /// 3 options (2 positive relics + 1 cursed relic) drawn from
    /// fixed pools by `run_state.rng.up_front`. The Rust port pins
    /// deterministic relic ids until `RunState.rng` lands. Bead
    /// `sts2-rs-6ynt.4.3`.
    Neow,
    /// `NeowsBlessing` — run-start 5-boon flow demonstrating the
    /// [`EventEffect`] declarative dispatcher landed in
    /// `sts2-rs-bv816`. Triggered at run start by game logic
    /// (`is_allowed` returns `false` so it never spawns from the
    /// room pool). The 5 boons mirror the classic blessing menu:
    /// `max_hp` (+8 max HP), `gain_gold` (+50g), `gain_relic`
    /// (queue a positive relic reward), `remove_card` (one-card
    /// removal — deck-mutation surface deferred to `6ynt.4.deck-add`),
    /// `transform_card` (one-card transform — also deferred).
    /// Bead `sts2-rs-8tkle`.
    NeowsBlessing,
    /// `DeprecatedEvent` — act3 disabled stub. Never spawns
    /// (`is_allowed = False`). Empty options in Python; the Rust
    /// spec carries a single synthetic `finish` option so the
    /// dispatcher's `every_event_id_has_a_spec` sentinel still
    /// passes. Bead `sts2-rs-6ynt.4.3`.
    DeprecatedEvent,
    /// `DeprecatedAncientEvent` — act3 disabled stub. Same shape as
    /// [`Self::DeprecatedEvent`]. Bead `sts2-rs-6ynt.4.3`.
    DeprecatedAncientEvent,
    /// `Reflections` — shared event (acts 0/1/2). Choices: `touch`
    /// (downgrade 2 + upgrade 4 cards — deck-mutation deferred) /
    /// `shatter` (duplicate deck + gain BadLuck curse — deferred).
    /// Bead `sts2-rs-6ynt.4.5`.
    Reflections,
    /// `RoundTeaParty` — shared event. Choices: `enjoy_tea` (heal to
    /// full + gain RoyalPoison relic) / `pick_fight` (multi-page
    /// fight → 11 damage + relic reward; multi-page collapses to
    /// finished). Bead `sts2-rs-6ynt.4.5`.
    RoundTeaParty,
    /// `SapphireSeed` — shared event. Choices: `eat` (heal 9 +
    /// upgrade 1 — upgrade deferred) / `plant` (enchant a card with
    /// `Sown` — enchantment surface deferred). Bead
    /// `sts2-rs-6ynt.4.5`.
    SapphireSeed,
    /// `SelfHelpBook` — shared event. 3 enchant-by-card-type options
    /// (Sharp/Nimble/Swift +2 on Attack/Skill/Power); enchantment
    /// deferred. Bead `sts2-rs-6ynt.4.5`.
    SelfHelpBook,
    /// `SpiritGrafter` — shared event. Choices: `let_it_in` (heal 25
    /// + gain Metamorphosis card — card-add deferred) / `rejection`
    /// (remove 1 card + take 9 damage — card-remove deferred). Bead
    /// `sts2-rs-6ynt.4.5`.
    SpiritGrafter,
    /// `SunkenStatue` — shared event. Choices: `grab_sword` (gain
    /// SwordOfStone relic) / `dive` (gain ~111 gold + take 7 damage;
    /// gold pinned deterministic). Bead `sts2-rs-6ynt.4.5`.
    SunkenStatue,
    /// `SunkenTreasury` — shared event. Choices: `first_chest` (~60
    /// gold) / `second_chest` (~333 gold + Greed curse — card-add
    /// deferred). Bead `sts2-rs-6ynt.4.5`.
    SunkenTreasury,
    /// `TabletOfTruth` — multi-page shared event (Decipher escalates
    /// cost 3→6→12→24→max-1; Smash heals 20). Multi-page collapses to
    /// single-page first-step here. Bead `sts2-rs-6ynt.4.5`.
    TabletOfTruth,
    /// `Tanx` — shared ancient event. 3 dynamic weapon-relic options
    /// drawn from a 9-relic pool; pinned to first 3 alphabetical
    /// until `RunState.rng` lands. Bead `sts2-rs-6ynt.4.5`.
    Tanx,
    /// `Tezcatara` — shared ancient event. 3 comfort-relic options
    /// from 3 themed pools; pinned to deterministic picks until
    /// `RunState.rng` lands. Bead `sts2-rs-6ynt.4.5`.
    Tezcatara,
    /// `TheLanternKey` — shared event. Choices: `return_key` (+100g)
    /// / `keep_key` (multi-page fight for LanternKey card — collapses
    /// to a CardReward placeholder). Bead `sts2-rs-6ynt.4.5`.
    TheLanternKey,
    /// `ThisOrThat` — shared event. Choices: `plain` (take 6 damage,
    /// gain ~55 gold) / `ornate` (gain relic + Clumsy curse —
    /// card-add deferred). Bead `sts2-rs-6ynt.4.5`.
    ThisOrThat,
    /// `TinkerTime` — multi-page shared event (pick card type → pick
    /// rider → create a moded MadScience card, added to the deck).
    /// Fully ported on the multi-page event framework (RNG-parity two
    /// `TakeRandom` draws + run-persistent morph). Bead `sts2-rs-2guyj`
    /// (framework: `sts2-rs-m3n0g.3`).
    TinkerTime,
    /// `Trial` — multi-page shared event (accept/reject, then judge
    /// merchant/noble/nondescript). Multi-page collapses to flavor-
    /// only outcome. Bead `sts2-rs-6ynt.4.5`.
    Trial,
    /// `UnrestSite` — shared event gated on `current_hp <= 70% max`.
    /// Choices: `rest` (heal to full + gain PoorSleep curse —
    /// card-add deferred) / `kill` (lose 8 Max HP + offer a relic).
    /// Bead `sts2-rs-6ynt.4.5`.
    UnrestSite,
    /// `Vakuu` — shared ancient event. 3 relic options from 3 themed
    /// pools; pinned to deterministic picks until `RunState.rng`
    /// lands. Bead `sts2-rs-6ynt.4.5`.
    Vakuu,
    /// `Wellspring` — shared event. Choices: `bottle` (gain a
    /// potion-reward — `Reward::Potion` deferred to placeholder) /
    /// `bathe` (remove 1 card + gain Guilty curse — card-mutation
    /// deferred). Bead `sts2-rs-6ynt.4.5`.
    Wellspring,
    /// `WoodCarvings` — shared event gated on owning a Basic card.
    /// Choices: `bird` (transform Basic → Peck) / `snake` (enchant a
    /// card with Slither) / `torus` (transform Basic → ToricToughness).
    /// All 3 are deck-mutation surfaces that defer to documented
    /// follow-ups. Bead `sts2-rs-6ynt.4.5`.
    WoodCarvings,
    /// `ZenWeaver` — shared event gated on `gold >= 125`. Choices:
    /// `breathing` (-50g, gain 2 Enlightenment — card-add deferred) /
    /// `emotional` (-125g, remove 1 card — deferred) / `acupuncture`
    /// (-250g, remove 2 cards — deferred). Bead `sts2-rs-6ynt.4.5`.
    ZenWeaver,
    /// `BrainLeech` — act1-pool event. Choices: `share_knowledge`
    /// (pick 1 of 5 character cards) / `rip` (take 5 damage, gain
    /// a colorless card reward). Bead `sts2-rs-6ynt.4.2`.
    BrainLeech,
    /// `RoomFullOfCheese` — act1-pool event. Choices: `gorge` (pick
    /// 2 of 8 common cards) / `search` (take 14 damage, gain Chosen
    /// Cheese relic). Bead `sts2-rs-6ynt.4.2`.
    RoomFullOfCheese,
    /// `TheLegendsWereTrue` — act1-pool event. Choices: `nab_map`
    /// (gain Spoils Map card) / `find_exit` (take 8 damage, gain a
    /// potion reward — deferred until `Reward::Potion` variant
    /// lands). Bead `sts2-rs-6ynt.4.2`.
    TheLegendsWereTrue,
    /// `TeaMaster` — act1-pool event. Choices: `bone_tea` (pay 50g,
    /// gain Bone Tea relic) / `ember_tea` (pay 150g, gain Ember Tea
    /// relic) / `discourtesy` (free, gain Tea of Discourtesy relic).
    /// Bead `sts2-rs-6ynt.4.2`.
    TeaMaster,
    // ── shared wave A (sts2-rs-6ynt.4.4) ──────────────────────────────
    /// `AbyssalBaths` — shared. Multi-page (immerse/linger); the
    /// `linger` loop is deferred until the framework grows multi-page
    /// support — `6ynt.4.4` ships single-step immerse (+2 Max HP /
    /// -3 HP) and abstain (heal 10). Bead `sts2-rs-6ynt.4.4`.
    AbyssalBaths,
    /// `Amalgamator` — shared. Combine 2 Strikes / Defends into an
    /// Ultimate variant. `is_allowed` deck check + card-choice flow
    /// deferred — the handlers queue `Reward::Card` placeholders so
    /// the run loop knows a reward was earned. Bead `sts2-rs-6ynt.4.4`.
    Amalgamator,
    /// `AromaOfChaos` — shared (per Python pin; audit doc lists
    /// `act2.py` erroneously). `let_go` queues a transform-card
    /// reward; `maintain_control` queues an upgrade-card reward.
    /// Concrete `TransformCards` / `UpgradeCards` reward variants
    /// land in a follow-up. Bead `sts2-rs-6ynt.4.4`.
    AromaOfChaos,
    /// `BattlewornDummy` — shared. 3 difficulty settings: easy →
    /// potion reward; medium → upgrade 2 cards (deferred); hard →
    /// relic reward. Bead `sts2-rs-6ynt.4.4`.
    BattlewornDummy,
    /// `Bugslayer` — shared. Gain Exterminate or Squash status card
    /// — queues a `Reward::Card` placeholder until status-card
    /// `Reward::Card.options` carries `make_exterminate()` /
    /// `make_squash()`. Bead `sts2-rs-6ynt.4.4`.
    Bugslayer,
    /// `ByrdonisNest` — shared. Eat (+7 Max HP) / Take (Byrdonis
    /// Egg card). `has_event_pet` precondition + egg-card queue
    /// deferred. Bead `sts2-rs-6ynt.4.4`.
    ByrdonisNest,
    /// `ColorfulPhilosophers` — shared. Choose a philosopher pool
    /// for 3 single-rarity card rewards. RNG-shuffled pool selection
    /// + `character_id` query deferred — the handlers queue an empty
    /// `Reward::Card` placeholder so the run loop knows reward
    /// intent. Bead `sts2-rs-6ynt.4.4`.
    ColorfulPhilosophers,
    /// `ColossalFlower` — shared. Multi-page extract/reach_deeper
    /// chain (3 dig levels then Pollinous Core). Single-step legs
    /// land in `6ynt.4.4`; multi-page chain deferred to
    /// `6ynt.4.multi-page`. `is_allowed`: `current_hp >= 19`. Bead
    /// `sts2-rs-6ynt.4.4`.
    ColossalFlower,
    /// `Darv` — shared. Ancient: 3 randomized boss-relic offers
    /// (Astrolabe/BlackStar/CallingBell/EmptyCage/RunicPyramid/
    /// SneckoEye + act-conditional). RNG/setup_attrs/Dusty Tome
    /// special-case deferred — handlers queue
    /// `Reward::Relic(ASTROLABE)` / `BLACK_STAR` / `CALLING_BELL`
    /// deterministic placeholders. Bead `sts2-rs-6ynt.4.4`.
    Darv,
    /// `DenseVegetation` — shared. Trudge On (-11 HP + remove 1
    /// card, queued placeholder) / Rest (heal max_hp*0.3, then
    /// fight). Combat-setup `event_combat_setup="dense_vegetation"`
    /// leg deferred. Bead `sts2-rs-6ynt.4.4`.
    DenseVegetation,
    /// `DoorsOfLightAndDark` — shared. Light (upgrade 2 random
    /// cards — deferred no-op) / Dark (remove 1 card — queued
    /// placeholder). Bead `sts2-rs-6ynt.4.4`.
    DoorsOfLightAndDark,
    /// `DrowningBeacon` — shared. Bottle (Glowwater Potion) /
    /// Climb (-13 Max HP, Fresnel Lens relic). Bottle queues a
    /// `Reward::Card` placeholder until `Reward::Potion` lands.
    /// Bead `sts2-rs-6ynt.4.4`.
    DrowningBeacon,
    /// `GraveOfTheForgotten` — shared. Confront (Decay + enchant
    /// `SoulsPower` on a card — deferred) / Accept (Forgotten Soul
    /// relic). Bead `sts2-rs-6ynt.4.4`.
    GraveOfTheForgotten,
    /// `HungryForMushrooms` — shared. Big (Big Mushroom relic) /
    /// Fragrant (-15 HP, Fragrant Mushroom relic). Bead
    /// `sts2-rs-6ynt.4.4`.
    HungryForMushrooms,
    /// `InfestedAutomaton` — shared. Study (random Power card) /
    /// Touch Core (random 0-cost card). Random-card-add deferred
    /// — handlers queue empty `Reward::Card` placeholders. Bead
    /// `sts2-rs-6ynt.4.4`.
    InfestedAutomaton,
    /// `Nonupeipe` — shared. Ancient: 3 randomized relic offers
    /// (Blessed Antler / Brilliant Scarf / Delicate Frond /
    /// Diamond Diadem / Fur Coat / Glitter / Jewelry Box / Looming
    /// Fruit / Signet Ring + Beautiful Bracelet conditional). RNG
    /// pool selection deferred — handlers queue deterministic
    /// `Reward::Relic` placeholders. Bead `sts2-rs-6ynt.4.4`.
    Nonupeipe,
    /// `Orobas` — shared. Ancient: 3 relic offers from disjoint
    /// pools (`Electric Shrymp` / `Glass Eye` / `Sand Castle` +
    /// `Prismatic Gem` / `Sea Glass`; `Alchemical Coffer` /
    /// `Driftwood` / `Radiant Pearl`; `Touch of Orobas` /
    /// `Archaic Tooth`). RNG/setup_attrs deferred. Bead
    /// `sts2-rs-6ynt.4.4`.
    Orobas,
    /// `Pael` — shared. Ancient: 3 relic offers from Pael's body
    /// parts. RNG/Goopy-enchant precondition deferred. Bead
    /// `sts2-rs-6ynt.4.4`.
    Pael,
    /// `PunchOff` — shared. Nab (Injury curse + RelicReward) /
    /// I Can Take Them (multi-page: fight for RelicReward +
    /// PotionReward). `is_allowed`: `total_floor >= 6` — deferred
    /// (no `total_floor` field on RunState yet, defaults to
    /// always-allowed). Multi-page `take_them` chain deferred.
    /// Bead `sts2-rs-6ynt.4.4`.
    PunchOff,
    // ── act2-pool wave A (sts2-rs-6ynt.4.6, 10 events A–R) ─────────
    /// `CrystalSphere` — act2. Pay 75g for 3 Prophesize picks, or
    /// take Debt curse for 6 picks. Prophesize / curse-add deferred.
    /// Bead `sts2-rs-6ynt.4.6`.
    CrystalSphere,
    /// `DollRoom` — act2. Pick a doll relic via random / 5-HP / 15-HP
    /// branches. Multi-page pick + RNG shuffle deferred; single-leg
    /// path grants the deterministic first doll. Bead `sts2-rs-6ynt.4.6`.
    DollRoom,
    /// `EndlessConveyor` — act2. Conveyor-belt sushi at 35g per grab.
    /// Multi-page belt loop + RNG-weighted dish pool deferred. Bead
    /// `sts2-rs-6ynt.4.6`.
    EndlessConveyor,
    /// `FakeMerchant` — act2. Buy fake relics for 50g each, or throw
    /// Foul Potion to start a fight. Multi-page inventory + combat
    /// setup deferred. Bead `sts2-rs-6ynt.4.6`.
    FakeMerchant,
    /// `FieldOfManSizedHoles` — act2. Resist (curse + remove) vs
    /// Enter (Perfect Fit enchant). Both legs deferred to
    /// `6ynt.4.deck-add`. Bead `sts2-rs-6ynt.4.6`.
    FieldOfManSizedHoles,
    /// `JungleMazeAdventure` — act2. Solo (-18 HP, +150g) vs Join
    /// (+50g). Fully ported (gold ranges pinned to midpoints until
    /// `6ynt.4.rng`). Bead `sts2-rs-6ynt.4.6`.
    JungleMazeAdventure,
    /// `LuminousChoir` — act2. Reach (curse + remove, deferred) vs
    /// Tribute (~125g for a deterministic CIRCLET relic placeholder).
    /// Bead `sts2-rs-6ynt.4.6`.
    LuminousChoir,
    /// `MorphicGrove` — act2. Group (-100g, transform 2 cards —
    /// transform deferred) vs Loner (+5 Max HP, fully ported). Bead
    /// `sts2-rs-6ynt.4.6`.
    MorphicGrove,
    /// `PotionCourier` — act2. Grab (3 Foul Potions) vs Ransack
    /// (1 uncommon potion). Both deferred to `6ynt.4.potion-add`.
    /// Bead `sts2-rs-6ynt.4.6`.
    PotionCourier,
    /// `RanwidTheElder` — act2. Give potion/100g/relic for a relic.
    /// Relic-pool roll + potion/relic remove deferred; gold leg
    /// charges 100g and defers the reward roll. Bead `sts2-rs-6ynt.4.6`.
    RanwidTheElder,
    // ── act2-pool wave B (sts2-rs-6ynt.4.7, 9 events R-Z) ──────────
    /// `RelicTrader` — act2. 3 trade-slots; swap an owned relic for
    /// a new one. Relic-list mutation + RNG-roll deferred — handlers
    /// queue a relic-reward placeholder. Bead `sts2-rs-6ynt.4.7`.
    RelicTrader,
    /// `SlipperyBridge` — act2 multi-page. Overcome (lose a random
    /// card — deferred) vs Hold On (escalating damage starting at 3).
    /// Single-step hold_on shipped; multi-page progression deferred
    /// to `6ynt.4.multi-page`. Bead `sts2-rs-6ynt.4.7`.
    SlipperyBridge,
    /// `SpiralingWhirlpool` — act2. Observe (enchant a card with
    /// Spiral — deferred) vs Drink (heal 33% of max HP, fully ported).
    /// Bead `sts2-rs-6ynt.4.7`.
    SpiralingWhirlpool,
    /// `StoneOfAllTime` — act2. Lift (+10 Max HP, potion-discard
    /// deferred) vs Push (-6 HP, Vigorous +8 enchant deferred).
    /// Bead `sts2-rs-6ynt.4.7`.
    StoneOfAllTime,
    /// `Symbiote` — act2. Approach (enchant a card with Corrupted —
    /// deferred) vs Kill with Fire (transform 1 card — deferred).
    /// Bead `sts2-rs-6ynt.4.7`.
    Symbiote,
    /// `TheFutureOfPotions` — act2. 3 trade-slots; discard a held
    /// potion for upgraded-rarity-matched card rewards.
    /// Potion-discard + character-card generation deferred — handlers
    /// queue a card-reward placeholder. Bead `sts2-rs-6ynt.4.7`.
    TheFutureOfPotions,
    /// `WaterloggedScriptorium` — act2. Bloody Ink (+6 Max HP, fully
    /// ported) / Tentacle Quill (-65g, enchant 1 card with Steady —
    /// deferred) / Prickly Sponge (-155g, enchant 2 cards — deferred).
    /// Bead `sts2-rs-6ynt.4.7`.
    WaterloggedScriptorium,
    /// `WelcomeToWongos` — act2. 4 options: Bargain Bin (-100g, common
    /// relic placeholder) / Featured Item (-200g, rare relic
    /// placeholder) / Mystery Box (-300g, Wongo's Mystery Ticket) /
    /// Leave (downgrade a random upgraded card — deferred). Wongo
    /// points/badge milestone deferred. Bead `sts2-rs-6ynt.4.7`.
    WelcomeToWongos,
    /// `WhisperingHollow` — act2. Gold (-50g, 2 potion-reward
    /// placeholders) vs Hug (-9 HP, transform 1 card — deferred).
    /// Bead `sts2-rs-6ynt.4.7`.
    WhisperingHollow,
}

impl EventId {
    /// Python class name for this event (matches the `event_id` class
    /// attribute on the `EventModel` subclass in
    /// `sts2-rl-agent@01c094d:sts2_env/events/`). Stable across the
    /// port; used by parity dumps and the (future) parity harness's
    /// event-mode replay format.
    #[must_use]
    pub const fn python_name(self) -> &'static str {
        match self {
            Self::LostWisp => "LostWisp",
            Self::TrashHeap => "TrashHeap",
            Self::TheArchitect => "TheArchitect",
            Self::WarHistorianRepy => "WarHistorianRepy",
            Self::Neow => "Neow",
            Self::NeowsBlessing => "NeowsBlessing",
            Self::DeprecatedEvent => "DeprecatedEvent",
            Self::DeprecatedAncientEvent => "DeprecatedAncientEvent",
            Self::BrainLeech => "BrainLeech",
            Self::RoomFullOfCheese => "RoomFullOfCheese",
            Self::TheLegendsWereTrue => "TheLegendsWereTrue",
            Self::TeaMaster => "TeaMaster",
            Self::AbyssalBaths => "AbyssalBaths",
            Self::Amalgamator => "Amalgamator",
            Self::AromaOfChaos => "AromaOfChaos",
            Self::BattlewornDummy => "BattlewornDummy",
            Self::Bugslayer => "Bugslayer",
            Self::ByrdonisNest => "ByrdonisNest",
            Self::ColorfulPhilosophers => "ColorfulPhilosophers",
            Self::ColossalFlower => "ColossalFlower",
            Self::Darv => "Darv",
            Self::DenseVegetation => "DenseVegetation",
            Self::DoorsOfLightAndDark => "DoorsOfLightAndDark",
            Self::DrowningBeacon => "DrowningBeacon",
            Self::GraveOfTheForgotten => "GraveOfTheForgotten",
            Self::HungryForMushrooms => "HungryForMushrooms",
            Self::InfestedAutomaton => "InfestedAutomaton",
            Self::Nonupeipe => "Nonupeipe",
            Self::Orobas => "Orobas",
            Self::Pael => "Pael",
            Self::PunchOff => "PunchOff",
            Self::Reflections => "Reflections",
            Self::RoundTeaParty => "RoundTeaParty",
            Self::SapphireSeed => "SapphireSeed",
            Self::SelfHelpBook => "SelfHelpBook",
            Self::SpiritGrafter => "SpiritGrafter",
            Self::SunkenStatue => "SunkenStatue",
            Self::SunkenTreasury => "SunkenTreasury",
            Self::TabletOfTruth => "TabletOfTruth",
            Self::Tanx => "Tanx",
            Self::Tezcatara => "Tezcatara",
            Self::TheLanternKey => "TheLanternKey",
            Self::ThisOrThat => "ThisOrThat",
            Self::TinkerTime => "TinkerTime",
            Self::Trial => "Trial",
            Self::UnrestSite => "UnrestSite",
            Self::Vakuu => "Vakuu",
            Self::Wellspring => "Wellspring",
            Self::WoodCarvings => "WoodCarvings",
            Self::ZenWeaver => "ZenWeaver",
            Self::CrystalSphere => "CrystalSphere",
            Self::DollRoom => "DollRoom",
            Self::EndlessConveyor => "EndlessConveyor",
            Self::FakeMerchant => "FakeMerchant",
            Self::FieldOfManSizedHoles => "FieldOfManSizedHoles",
            Self::JungleMazeAdventure => "JungleMazeAdventure",
            Self::LuminousChoir => "LuminousChoir",
            Self::MorphicGrove => "MorphicGrove",
            Self::PotionCourier => "PotionCourier",
            Self::RanwidTheElder => "RanwidTheElder",
            Self::RelicTrader => "RelicTrader",
            Self::SlipperyBridge => "SlipperyBridge",
            Self::SpiralingWhirlpool => "SpiralingWhirlpool",
            Self::StoneOfAllTime => "StoneOfAllTime",
            Self::Symbiote => "Symbiote",
            Self::TheFutureOfPotions => "TheFutureOfPotions",
            Self::WaterloggedScriptorium => "WaterloggedScriptorium",
            Self::WelcomeToWongos => "WelcomeToWongos",
            Self::WhisperingHollow => "WhisperingHollow",
        }
    }

    /// Reverse of [`Self::python_name`] — resolve a Python class name
    /// back to its [`EventId`]. Returns `None` for unknown names.
    ///
    /// Used by the `pick_event` driver
    /// ([`sts2-content::events::dispatch::pick_event`]) to look up the
    /// `EventId` for each `&'static str` entry in the act's
    /// `event_ids` / `event_ids_shuffled` pool. Bead `sts2-rs-b0hjz`
    /// (P9.2.w event-framework scaffolding).
    #[must_use]
    pub fn from_python_name(name: &str) -> Option<EventId> {
        for id in Self::all() {
            if id.python_name() == name {
                return Some(*id);
            }
        }
        None
    }

    /// Enumeration of every [`EventId`] variant in declaration order.
    /// Used by the registry-completeness lints in
    /// `sts2-content::events::dispatch::tests`.
    #[must_use]
    pub const fn all() -> &'static [EventId] {
        // EVERY variant, in declaration order. This list previously held only
        // a manually-appended subset, which silently broke
        // `from_python_name()` for the missing variants — and since
        // `dispatch::pick_event` builds its pool via
        // `filter_map(EventId::from_python_name)`, every missing variant was
        // DROPPED from the act event pool, compacting slot indices and
        // desyncing the `events_visited` cursor walk vs C# (RP3: Rust offered
        // SapphireSeed where C# offered RoomFullOfCheese). The
        // `every_event_id_has_a_spec` sentinel in
        // `sts2-content::events::dispatch::tests` enforces that each entry
        // here has a wired EventSpec.
        &[
            Self::LostWisp,
            Self::TrashHeap,
            Self::TheArchitect,
            Self::WarHistorianRepy,
            Self::Neow,
            Self::NeowsBlessing,
            Self::DeprecatedEvent,
            Self::DeprecatedAncientEvent,
            Self::Reflections,
            Self::RoundTeaParty,
            Self::SapphireSeed,
            Self::SelfHelpBook,
            Self::SpiritGrafter,
            Self::SunkenStatue,
            Self::SunkenTreasury,
            Self::TabletOfTruth,
            Self::Tanx,
            Self::Tezcatara,
            Self::TheLanternKey,
            Self::ThisOrThat,
            Self::TinkerTime,
            Self::Trial,
            Self::UnrestSite,
            Self::Vakuu,
            Self::Wellspring,
            Self::WoodCarvings,
            Self::ZenWeaver,
            Self::BrainLeech,
            Self::RoomFullOfCheese,
            Self::TheLegendsWereTrue,
            Self::TeaMaster,
            Self::AbyssalBaths,
            Self::Amalgamator,
            Self::AromaOfChaos,
            Self::BattlewornDummy,
            Self::Bugslayer,
            Self::ByrdonisNest,
            Self::ColorfulPhilosophers,
            Self::ColossalFlower,
            Self::Darv,
            Self::DenseVegetation,
            Self::DoorsOfLightAndDark,
            Self::DrowningBeacon,
            Self::GraveOfTheForgotten,
            Self::HungryForMushrooms,
            Self::InfestedAutomaton,
            Self::Nonupeipe,
            Self::Orobas,
            Self::Pael,
            Self::PunchOff,
            Self::CrystalSphere,
            Self::DollRoom,
            Self::EndlessConveyor,
            Self::FakeMerchant,
            Self::FieldOfManSizedHoles,
            Self::JungleMazeAdventure,
            Self::LuminousChoir,
            Self::MorphicGrove,
            Self::PotionCourier,
            Self::RanwidTheElder,
            Self::RelicTrader,
            Self::SlipperyBridge,
            Self::SpiralingWhirlpool,
            Self::StoneOfAllTime,
            Self::Symbiote,
            Self::TheFutureOfPotions,
            Self::WaterloggedScriptorium,
            Self::WelcomeToWongos,
            Self::WhisperingHollow,
        ]
    }
}

// ── EventOptionInfo / EventOutcome ────────────────────────────────────

/// One choice presented to the agent when an event materializes its
/// option set. Mirrors Python
/// `sts2-rl-agent@01c094d:sts2_env/run/events.py::EventOption`
/// (`option_id` + `label` + `description` + `enabled`).
///
/// `label` and `description` are `String` rather than `&'static str`
/// because some events (e.g. `LostWisp`) interpolate dynamic values
/// (`"Gain {gold} gold"`); spec-side defaults are static and the
/// dispatcher up-converts to owned strings at presentation time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventOptionInfo {
    /// Stable per-choice id (matches the Python `option_id` literal —
    /// e.g. `"claim"` / `"search"` / `"dive_in"` / `"grab"`).
    pub option_id: &'static str,
    /// Short human label shown on the choice button.
    pub label: String,
    /// Long-form choice description ("Gain 60 gold", etc).
    pub description: String,
    /// Whether the choice is currently selectable (mirrors Python's
    /// `enabled`; gated by `is_allowed`-style preconditions).
    pub enabled: bool,
    /// Localization path for this choice, WITHOUT the trailing field —
    /// e.g. `"WELLSPRING.pages.INITIAL.options.BOTTLE"`. A display layer
    /// appends `.title` / `.description` and looks the result up in the
    /// game's own `localization_eng/events.json`, so the player reads the
    /// game's wording rather than [`label`](Self::label) /
    /// [`description`](Self::description), which are developer shorthand
    /// ("Gain a random potion." where the game says "Procure 1 random
    /// Potion.").
    ///
    /// Empty when the choice has no row in that table: the Ancient / Neow
    /// relic offers (their text comes from `ancients.json` / `relics.json`
    /// via the relic itself) and a handful of options whose loc row is
    /// picked per-run from a set (Colorful Philosophers' character pools,
    /// Endless Conveyor's dishes). Fall back to `label` / `description`
    /// there.
    pub loc_key: &'static str,
    /// Values for the `{VarName}` placeholders in the localized template,
    /// as `(name, value)` — e.g. `("Gold", EventVar::Int(61))` for
    /// `"Gain {Gold} Gold."`. The game substitutes these before display;
    /// so must anything rendering [`loc_key`](Self::loc_key). A template
    /// whose vars are not all supplied cannot be rendered faithfully —
    /// the display layer is expected to fall back rather than print a
    /// bare variable name.
    pub vars: Vec<(&'static str, EventVar)>,
    /// Values that apply to the option's TITLE only, overriding
    /// [`vars`](Self::vars) where the names collide. Empty for all but one
    /// option in the game: The Future of Potions' `{Rarity}` is the POTION's
    /// rarity in its title and the CARD's in its description, and those
    /// differ for Event and Token potions.
    pub title_vars: Vec<(&'static str, EventVar)>,
}

/// One substituted value in an event option's localized template.
///
/// The int/text split is load-bearing, not cosmetic: the template DSL has a
/// `{Var:plural:card|cards}` form that compares the value to 1, so a count
/// handed over as text silently takes the plural branch. Names of things
/// (`{Relic}`, `{Curse}`, `{Enchantment}`) are text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventVar {
    /// A number — counts, gold, HP. Eligible for `:plural:`.
    Int(i64),
    /// A name or phrase, already in display form ("Fresnel Lens").
    Text(String),
}

/// The `const`-constructible twin of [`EventVar`], for the values a static
/// [`EventOptionSpec`](../../sts2_content/events/struct.EventOptionSpec.html)
/// already knows without a run to consult — a fixed HP cost, the name of the
/// curse an option always adds. Values that depend on the run (a gold roll,
/// which relic was offered) need a dynamic `generate_options` instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventVarSpec {
    /// A number — counts, gold, HP. Eligible for `:plural:`.
    Int(i64),
    /// A name or phrase, already in display form.
    Text(&'static str),
}

impl EventVarSpec {
    /// Owned form, for a presented [`EventOptionInfo`].
    #[must_use]
    pub fn to_var(self) -> EventVar {
        match self {
            Self::Int(v) => EventVar::Int(v),
            Self::Text(v) => EventVar::Text(v.to_string()),
        }
    }
}

impl From<i64> for EventVar {
    fn from(v: i64) -> Self {
        Self::Int(v)
    }
}

impl From<i32> for EventVar {
    fn from(v: i32) -> Self {
        Self::Int(i64::from(v))
    }
}

impl From<String> for EventVar {
    fn from(v: String) -> Self {
        Self::Text(v)
    }
}

impl From<&str> for EventVar {
    fn from(v: &str) -> Self {
        Self::Text(v.to_string())
    }
}

/// Result of resolving a single event choice. Mirrors the subset of
/// `sts2-rl-agent@01c094d:sts2_env/run/events.py::EventResult` that
/// this framework bead exercises:
///
/// - `finished` — `true` iff this choice closed the event (single-page
///   events always `true`; the framework currently only supports
///   single-page events — multi-page `next_options` land in a
///   follow-up).
/// - `description` — flavor / outcome text for the run log.
///
/// `next_options` and `rewards` are intentionally absent: per-event
/// handlers mutate [`RunState::pending_rewards`] / [`RunState::gold`]
/// / etc. directly today (matches the non-deferred branch of Python
/// `_should_defer_event_rewards`). The fields will be added when a
/// real multi-page event lands.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct EventOutcome {
    /// Whether this choice closed the event.
    pub finished: bool,
    /// Flavor / outcome description for the run log.
    pub description: String,
    /// Declarative effects to apply to [`crate::RunState`] when this
    /// outcome resolves. Bead `sts2-rs-bv816`. Defaults to empty —
    /// per-event handlers that mutate `RunState` directly leave this
    /// untouched; the dispatcher walks this list and applies each
    /// [`EventEffect`] via [`apply_event_effects`] before clearing
    /// `current_event`.
    pub effects: Vec<EventEffect>,
}

impl EventOutcome {
    /// Construct a finished outcome with the given description and
    /// no effects. Per-event handlers that mutate [`crate::RunState`]
    /// directly (legacy `6ynt.4.x` pattern) use this constructor and
    /// keep `effects` empty.
    #[must_use]
    pub fn finished<S: Into<String>>(description: S) -> Self {
        Self {
            finished: true,
            description: description.into(),
            effects: Vec::new(),
        }
    }

    /// Builder: attach a declarative effect list to this outcome.
    /// Replaces any previously-set effects.
    #[must_use]
    pub fn with_effects(mut self, effects: Vec<EventEffect>) -> Self {
        self.effects = effects;
        self
    }
}

// ── MultiPageEventState ───────────────────────────────────────────────

/// Scratch state for a multi-page event (the active event in
/// [`crate::RunState::current_event`]). Bead `sts2-rs-2guyj` /
/// `sts2-rs-m3n0g.3`.
///
/// Single-page events never touch this — their handler returns a
/// `finished` [`EventOutcome`] on the first choice and the dispatcher
/// exits the room. Multi-page events (TinkerTime first) return a
/// **not-finished** outcome from intermediate-page handlers; the
/// dispatcher leaves `current_event` set, so the next
/// [`crate::RunState`]-side presentation reads this scratch to render the
/// next page, and the next `resolve` drives the chosen handler.
///
/// Reset to [`Self::default`] on [`crate::RunState::enter_event_room`]
/// and cleared on [`crate::RunState::exit_event_room`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MultiPageEventState {
    /// 0-based page cursor. `0` is the initial page (what
    /// `GenerateInitialOptions` would render). Each intermediate-page
    /// handler bumps this so the event's `generate_options` knows which
    /// page to render next.
    pub page: u8,
    /// The event's own RNG stream (C# `EventModel.Rng`, seeded per
    /// `EventModel.cs:193`). Created lazily by the first handler that
    /// draws and persisted across page resolutions so its counter
    /// advances across draws (e.g. TinkerTime's two `TakeRandom`s).
    pub rng: Option<crate::dotnet_rng::Rng>,
    /// Generic per-event scratch slots, interpreted by the active
    /// event's handlers (e.g. TinkerTime stores the two offered
    /// card-type codes on page 1, then the two offered rider codes on
    /// page 2).
    pub scratch: Vec<i16>,
}

// ── EventEffect ────────────────────────────────────────────────────────

/// Declarative event-outcome effect. Bead `sts2-rs-bv816` (P9.2.e.1).
///
/// Per-event handlers can choose between two styles for resolving a
/// player's choice:
///
/// 1. **Direct mutation** — touch [`crate::RunState`] in the handler
///    body and return an [`EventOutcome`] with an empty `effects` Vec.
///    This is the legacy `6ynt.4.x` pattern; every currently-ported
///    event uses it.
/// 2. **Declarative effects** — return an [`EventOutcome`] carrying a
///    `Vec<EventEffect>`; the dispatcher (`sts2-content::events::
///    dispatch::resolve`) applies each effect via
///    [`Self::apply`] before clearing `current_event`. This is the
///    preferred shape for new events; it keeps the effect ledger
///    inspectable (replay/eval can read the effect list off the
///    outcome) and removes the per-handler ownership of `&mut RunState`.
///
/// `Lose*` variants take a positive magnitude; `Gain*` variants take
/// a positive amount. Negative deltas are not allowed in either
/// variant — the dispatch path matches on variant identity, not sign.
/// Tests pin this convention.
///
/// `#[non_exhaustive]` so downstream events can grow new variants
/// (potion grants, deck-transform handles, etc.) without breaking
/// every match arm.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventEffect {
    /// Add gold to the player's pile. Calls
    /// [`crate::RunState::gain_gold`] with a positive delta.
    GainGold(i32),
    /// Subtract gold (saturating at 0 — Python clamps too).
    /// Implemented via `gain_gold(-amount)`.
    LoseGold(i32),
    /// Heal the player. Calls [`crate::RunState::heal`]; clamped to
    /// `max_hp` per the heal contract.
    GainHp(i32),
    /// Damage the player by `amount`. Calls
    /// [`crate::RunState::lose_hp`].
    LoseHp(i32),
    /// Increase max HP by `amount` (also bumps current HP via
    /// `gain_max_hp`'s contract).
    GainMaxHp(i32),
    /// Decrease max HP by `amount`. Implemented via
    /// [`crate::RunState::lose_max_hp`] (C#-faithful: only clamps current
    /// HP down when the new max falls below it — NOT `gain_max_hp(-amount)`,
    /// which over-subtracts current HP when `current < max`).
    LoseMaxHp(i32),
    /// Grant a relic. Appends to [`crate::RunState::relics`] and
    /// stamps [`crate::RunState::last_obtained_relic`]. Does **not**
    /// fire the `after_obtained` dispatcher — the canonical route is
    /// [`crate::RunState::accept_relic_reward`] for that; events that
    /// want hook fan-out should queue a [`crate::RelicReward`] via
    /// [`crate::RunState::offer_relic_rewards`] and let the agent
    /// accept it, rather than granting via this effect.
    GainRelic(crate::RelicId),
    /// Append a fully-constructed [`crate::cards::Card`] to
    /// [`crate::RunState::master_deck`]. Used for curses (Pain,
    /// Doubt, Injury, …) and blessings. The caller constructs the
    /// `Card` via the appropriate `sts2-content` factory so the
    /// effect carries the full POD (uid, type, costs, …).
    AddCardToDeck(crate::cards::Card),
}

impl EventEffect {
    /// Apply this effect to `rs`. Idempotent on bounds (heal clamps to
    /// `max_hp`, gold floors at 0).
    pub fn apply(&self, rs: &mut crate::RunState) {
        match self {
            Self::GainGold(n) => rs.gain_gold(*n),
            Self::LoseGold(n) => {
                // Python's `RunState.lose_gold` floors at 0; Rust's
                // `gain_gold` is pure-additive, so we clamp here.
                let actual = (*n).min(rs.gold).max(0);
                rs.gain_gold(-actual);
            }
            Self::GainHp(n) => {
                let _ = rs.heal(*n);
            }
            Self::LoseHp(n) => {
                let _ = rs.lose_hp(*n);
            }
            Self::GainMaxHp(n) => rs.gain_max_hp(*n),
            Self::LoseMaxHp(n) => rs.lose_max_hp(*n),
            Self::GainRelic(id) => {
                rs.relics.push(*id);
                rs.last_obtained_relic = Some(*id);
            }
            Self::AddCardToDeck(card) => rs.master_deck.push(*card),
        }
    }
}

/// Apply every effect in `effects` to `rs`, in order. Mirrors the
/// imperative effect-loop in Python's `_resolve_event_outcome`
/// (`run_manager.py`). Bead `sts2-rs-bv816`.
pub fn apply_event_effects(rs: &mut crate::RunState, effects: &[EventEffect]) {
    for effect in effects {
        effect.apply(rs);
    }
}

// ── Event→combat bridge (bead `sts2-rs-ck9z6`, GH #166) ─────────────────

/// `BattlewornDummy` difficulty setting — selects which `BattleFriend`
/// training dummy the event spawns and which victory reward
/// `BattlewornDummy.Resume` grants. Mirrors C#
/// `BattlewornDummyEventEncounter.DummySetting`
/// (`MegaCrit.Sts2.Core.Models.Encounters/BattlewornDummyEventEncounter.cs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BattlewornDummySetting {
    /// Setting 1 (Easy) — `BattleFriendV1` (75 HP) → win grants a potion.
    Setting1,
    /// Setting 2 (Medium) — `BattleFriendV2` (150 HP) → win upgrades 2 cards.
    Setting2,
    /// Setting 3 (Hard) — `BattleFriendV3` (300 HP) → win grants a relic.
    Setting3,
}

/// Which event-nested combat encounter to fight. Mirrors the
/// `EncounterModel` passed to C#
/// `EventModel.EnterCombatWithoutExitingEvent`. The content-side
/// `build_event_combat` callback maps each variant to a concrete monster
/// roster; the `resume_event_after_combat` callback reads it back to
/// grant the right post-victory reward.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventEncounter {
    /// `BattlewornDummyEventEncounter` (BattleFriend V1/V2/V3 by setting).
    BattlewornDummy(BattlewornDummySetting),
    /// `MysteriousKnightEventEncounter` (TheLanternKey "Fight").
    MysteriousKnight,
    /// `FakeMerchantEventEncounter` (FakeMerchant "throw foul potion").
    FakeMerchant,
}

impl EventEncounter {
    /// Whether the fight's room offers rewards (C# `EncounterModel.ShouldGiveRewards`;
    /// `false` only for `BattlewornDummyEventEncounter`, whose room offers an
    /// empty set: its extra rewards are never offered). sts2-agent fork.
    #[must_use]
    pub const fn gives_rewards(&self) -> bool {
        !matches!(self, Self::BattlewornDummy(_))
    }
}

/// A request, raised by an event-option handler, to enter a real
/// [`crate::run_manager::Phase::Combat`] against an [`EventEncounter`]
/// and (optionally) resume the parent event afterward.
///
/// Mirrors C# `EventModel.EnterCombatWithoutExitingEvent(encounter,
/// extraRewards, shouldResumeAfterCombat)`
/// (`EventModel.cs:480`): the handler stages this on
/// [`crate::RunState::event_combat`] instead of granting the reward
/// directly, and [`crate::run_manager::RunManager`] consumes it to drive
/// the fight and grant the reward **only on victory** (a `BattlewornDummy`
/// can run out of time → no reward, mirroring `RanOutOfTime`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventCombatRequest {
    /// Which encounter to fight.
    pub encounter: EventEncounter,
    /// Whether to resume the parent event after combat (C#
    /// `shouldResumeAfterCombat`). When `true` (BattlewornDummy) the
    /// `resume_event_after_combat` callback grants the per-setting reward
    /// on a clean victory. When `false` (TheLanternKey / FakeMerchant) the
    /// combat-room `victory_rewards` are granted directly on victory.
    pub resume: bool,
    /// The event that launched this combat — keyed for the resume
    /// dispatch (C# `CombatRoom.ParentEventId`).
    pub parent_event: EventId,
    /// Extra rewards granted ONLY on a clean victory when `resume` is
    /// `false` (C# `extraRewards` added via `CombatRoom.AddExtraReward`):
    /// TheLanternKey's LanternKey card, FakeMerchant's FakeMerchantsRug.
    pub victory_rewards: Vec<crate::runstate::Reward>,
}

impl EventCombatRequest {
    /// Build a resume-after-combat request (BattlewornDummy) with no
    /// direct victory rewards — the reward is rolled in the resume
    /// callback per setting.
    #[must_use]
    pub fn resuming(encounter: EventEncounter, parent_event: EventId) -> Self {
        Self {
            encounter,
            resume: true,
            parent_event,
            victory_rewards: Vec::new(),
        }
    }

    /// Build a no-resume request (TheLanternKey / FakeMerchant) whose
    /// `victory_rewards` are granted directly on a clean victory.
    #[must_use]
    pub fn no_resume(
        encounter: EventEncounter,
        parent_event: EventId,
        victory_rewards: Vec<crate::runstate::Reward>,
    ) -> Self {
        Self {
            encounter,
            resume: false,
            parent_event,
            victory_rewards,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_id_python_name_round_trips() {
        for id in EventId::all() {
            let name = id.python_name();
            assert!(!name.is_empty(), "EventId::{id:?} missing python_name");
        }
        assert_eq!(EventId::LostWisp.python_name(), "LostWisp");
        assert_eq!(EventId::TrashHeap.python_name(), "TrashHeap");
    }

    #[test]
    fn event_id_all_matches_variant_count() {
        // `all()` must enumerate EVERY EventId variant: `from_python_name`
        // (and through it `dispatch::pick_event`'s pool construction) walks
        // `all()`, so a missing variant is silently DROPPED from the act
        // event pool — compacting slot indices and desyncing the
        // `events_visited` cursor vs C# (RP3 run-parity divergence,
        // 2026-06-12). Bump this count in the same commit that adds a
        // variant to the enum + `all()` + its `spec_for` arm.
        assert_eq!(EventId::all().len(), 69);
    }

    #[test]
    fn event_id_is_copy_hashable() {
        fn assert_copy<T: Copy + std::hash::Hash + Eq>() {}
        assert_copy::<EventId>();
    }

    // -- EventEffect + apply_event_effects (bead `sts2-rs-bv816`) ----

    #[test]
    fn event_outcome_finished_helper_starts_with_no_effects() {
        let out = EventOutcome::finished("Took 8 damage.");
        assert!(out.effects.is_empty(), "default effects list is empty");
    }

    #[test]
    fn event_outcome_with_effects_builder_attaches_list() {
        let out = EventOutcome::finished("Found gold and a relic.").with_effects(vec![
            EventEffect::GainGold(60),
            EventEffect::GainRelic(crate::RelicId::BurningBlood),
        ]);
        assert_eq!(out.effects.len(), 2);
        assert!(matches!(out.effects[0], EventEffect::GainGold(60)));
    }

    #[test]
    fn apply_event_effects_gold_gain_and_loss() {
        let mut rs = crate::RunState::new();
        rs.gold = 50;
        apply_event_effects(
            &mut rs,
            &[EventEffect::GainGold(60), EventEffect::LoseGold(30)],
        );
        assert_eq!(rs.gold, 80);
    }

    #[test]
    fn apply_event_effects_lose_gold_clamps_at_zero() {
        // RunState::gain_gold floors at 0 by contract; LoseGold maps
        // to gain_gold(-N), so the clamp transfers.
        let mut rs = crate::RunState::new();
        rs.gold = 10;
        apply_event_effects(&mut rs, &[EventEffect::LoseGold(40)]);
        assert_eq!(rs.gold, 0, "lose_gold beyond pile must floor at 0");
    }

    #[test]
    fn apply_event_effects_hp_gain_clamps_at_max_hp() {
        let mut rs = crate::RunState::new();
        rs.max_hp = 80;
        rs.current_hp = 70;
        apply_event_effects(&mut rs, &[EventEffect::GainHp(50)]);
        assert_eq!(rs.current_hp, 80, "heal clamps at max_hp");
    }

    #[test]
    fn apply_event_effects_lose_hp_reduces_current_hp() {
        let mut rs = crate::RunState::new();
        rs.current_hp = 80;
        apply_event_effects(&mut rs, &[EventEffect::LoseHp(12)]);
        assert_eq!(rs.current_hp, 68);
    }

    #[test]
    fn apply_event_effects_gain_max_hp_bumps_max_and_current() {
        let mut rs = crate::RunState::new();
        rs.max_hp = 80;
        rs.current_hp = 60;
        apply_event_effects(&mut rs, &[EventEffect::GainMaxHp(8)]);
        assert_eq!(rs.max_hp, 88);
        // gain_max_hp also bumps current_hp by the same delta.
        assert_eq!(rs.current_hp, 68);
    }

    #[test]
    fn apply_event_effects_lose_max_hp_is_csharp_faithful() {
        // current == max: current clamps down with the max.
        let mut rs = crate::RunState::new();
        rs.max_hp = 80;
        rs.current_hp = 80;
        apply_event_effects(&mut rs, &[EventEffect::LoseMaxHp(10)]);
        assert_eq!(rs.max_hp, 70);
        assert_eq!(rs.current_hp, 70);

        // current < max (e.g. A2 WearyTraveler start): losing max HP that
        // stays above current must leave current UNTOUCHED — the parity bug
        // (LoseMaxHp routed through gain_max_hp) over-subtracted it.
        let mut rs = crate::RunState::new();
        rs.max_hp = 80;
        rs.current_hp = 64;
        apply_event_effects(&mut rs, &[EventEffect::LoseMaxHp(12)]);
        assert_eq!(rs.max_hp, 68);
        assert_eq!(rs.current_hp, 64);
    }

    #[test]
    fn apply_event_effects_gain_relic_pushes_and_stamps_last_obtained() {
        let mut rs = crate::RunState::new();
        apply_event_effects(
            &mut rs,
            &[EventEffect::GainRelic(crate::RelicId::BurningBlood)],
        );
        assert_eq!(rs.relics, vec![crate::RelicId::BurningBlood]);
        assert_eq!(rs.last_obtained_relic, Some(crate::RelicId::BurningBlood));
    }

    #[test]
    fn apply_event_effects_add_card_appends_to_master_deck() {
        let mut rs = crate::RunState::new();
        let card = crate::cards::Card::pod_defaults();
        apply_event_effects(&mut rs, &[EventEffect::AddCardToDeck(card)]);
        assert_eq!(rs.master_deck.len(), 1);
        assert_eq!(rs.master_deck[0].card_id, card.card_id);
    }

    #[test]
    fn apply_event_effects_runs_in_listed_order() {
        // Ordering is observable when consecutive effects target the
        // same pool (gold +60 then -30 ≠ -30 then +60 if the floor
        // clamp would otherwise intervene).
        let mut rs = crate::RunState::new();
        rs.gold = 0;
        apply_event_effects(
            &mut rs,
            &[EventEffect::GainGold(60), EventEffect::LoseGold(40)],
        );
        assert_eq!(rs.gold, 20);

        let mut rs = crate::RunState::new();
        rs.gold = 0;
        apply_event_effects(
            &mut rs,
            &[EventEffect::LoseGold(40), EventEffect::GainGold(60)],
        );
        // Lose-first floors at 0; +60 yields 60, NOT 20.
        assert_eq!(rs.gold, 60);
    }

    #[test]
    fn event_outcome_finished_helper() {
        let out = EventOutcome::finished("Took 8 damage.");
        assert!(out.finished);
        assert_eq!(out.description, "Took 8 damage.");
    }
}
