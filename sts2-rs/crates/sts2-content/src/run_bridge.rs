//! Combat ↔ Run bridge — P9.0.c (bead `sts2-rs-tjn0v`).
//!
//! [`enter_combat`] materializes a fresh [`CombatState`] from the
//! per-run [`RunState`] + an [`EncounterSpec`] (the run-side identity
//! of a fight, see `sts2-rs-5mfyd`). [`exit_combat`] writes the
//! post-combat scalars that the run-side cares about (current HP,
//! per-relic counters) back into [`RunState`].
//!
//! The bridge lives in `sts2-content` (not `sts2-core`) because it
//! consumes the content-side [`build_ai`] dispatch — the dep direction
//! is `content → core`, so core can't reference content-side AI
//! factories directly. The encounter spec itself lives in
//! `sts2-core::encounters` (POD only: `kind` + `hp`), so this module
//! is the natural seam.
//!
//! Mirrors a thin slice of the Python pin
//! `sts2-rl-agent@01c094d:sts2_env/run/run_manager.py::enter_combat /
//! _setup_combat`. Out of scope at this pin (deferred to later P9
//! beads):
//!
//! - **Reward generation** on combat clear — that's P9.1.e
//!   (`sts2-rs-jyfvu`); `exit_combat` does not push any rewards.
//! - **Potion belt transfer** — `RunState` has no `potions`
//!   inventory field at the current pin; the combat-side
//!   [`sts2_core::PlayerCombatState::potions`] starts empty and is
//!   populated by potion-rewards once that lands.
//! - **`apply_pending_combat_gold_loss` drain** — obsolete/dead. It is
//!   exposed on `RunState` but has NO production caller (only its own
//!   unit tests reference it). DEBT's turn-end gold loss now mutates the
//!   live `state.gold` directly (C# `Debt.OnTurnEndInHand` ->
//!   `PlayerCmd.LoseGold`), and `exit_combat` reconciles run gold via
//!   `rs.gold = combat.gold`. The accumulator and this drain are dead
//!   and slated for removal.
//! - **RNG stream init** — [`enter_combat_from_roster`] clones the run's
//!   `rs.rng` ([`sts2_core::run_rng_set::RunRngSet`]) into the combat and
//!   [`exit_combat`] writes the advanced cursors back, so the combat reads
//!   the run's cumulative `MonsterAi` / `Shuffle` / `Niche` streams exactly
//!   as C#'s single persistent `RunState.Rng` does (bead `sts2-rs-jenwp`,
//!   M3b monster_ai). The legacy non-stream `CombatState::rng` is seeded
//!   deterministically from the run's master seed.
//! - **Relic mirror (`rs.relics` → `state.relics`)** — wired (bead
//!   `sts2-rs-pfiwm`). `RunState` tracks relics by
//!   [`sts2_core::RelicId`] (the generated Python-pin enum, 294
//!   variants) while [`sts2_core::Relic::kind`] is typed
//!   [`sts2_core::RelicKind`] (the combat-side enum, 290 variants).
//!   The two enums share variant names 1-to-1, so
//!   [`sts2_core::RelicId::to_relic_kind`] is a pure name projection
//!   (the only `None` arms are the five W9 Neow reward relics with no
//!   combat behavior — bead `sts2-rs-4rwv4`). `enter_combat`
//!   materializes one combat-side `Relic` row per run relic, in
//!   pickup order, seeding `counter` from `rs.relic_counters` (falling
//!   back to [`sts2_core::RelicKind::initial_counter`]); listeners
//!   self-guard on `state.relics`, so no per-relic wiring is needed.
//!   `exit_combat` flushes each combat relic's `counter` back into
//!   `rs.relic_counters`.

use std::num::NonZeroU32;

use smallvec::SmallVec;

use sts2_core::encounters::EncounterSpec;
use sts2_core::monsters::MonsterKind;
use sts2_core::runstate::RunState;
use sts2_core::{CombatState, Creature, CreatureUid, IntentHistory, MonsterId};

use crate::monster_dispatch::{build_ai_with_ascension, MonsterSpawnArgs, SpawnPosition};

/// Build a fresh combat from `rs` + `encounter`.
///
/// What's wired:
/// 1. **Player HP / max HP**: copied from `rs.current_hp` /
///    `rs.max_hp` into `state.player`, then `init_player_creature`
///    mirrors them onto the player-Creature row.
/// 2. **Master deck → draw pile**: each card in `rs.master_deck` is
///    cloned and pushed via [`CombatState::add_card_to_draw_pile`],
///    which assigns a fresh [`sts2_core::CardUid`]. The draw pile is
///    LIFO at insert time (matches Python `insert(0, …)`); the
///    P9.1 shuffle bead reshuffles before turn 1.
/// 3. **Relics**: each `rs.relics` entry is mirrored into a combat-side
///    `Relic` row via [`sts2_core::RelicId::to_relic_kind`], in pickup
///    order, with `counter` seeded from `rs.relic_counters` (see the
///    module-level relic-mirror note). Bead `sts2-rs-pfiwm`.
/// 4. **Act snapshot**: `state.act_index` is set from `rs.act_index`
///    so listeners gated on act (LavaRock, FurCoat) see the right
///    value (`sts2-rs-mi67e`).
/// 5. **Enemies**: each [`encounter.members`] entry is spawned with
///    its declared `hp` (no RNG roll — the spec carries the resolved
///    value), tagged with its [`MonsterKind`], and wired with an AI
///    from [`build_ai`] using a derived [`SpawnPosition`]. On-spawn
///    self-buffs (`apply_creature_spawn_buffs`) fire per push.
/// Build the [`CombatState`] for an event-nested combat
/// ([`sts2_core::events::EventCombatRequest`]). Maps the request's
/// [`sts2_core::events::EventEncounter`] to its monster, rolls the
/// monster's starting HP off the run's shared `niche` stream, and runs the
/// shared [`enter_combat_from_roster`] assembly path — the spawn buffs
/// (`apply_creature_spawn_buffs`) apply each dummy's
/// `BattlewornDummyTimeLimit(3)` and MysteriousKnight's
/// `Strength(6)`/`Plating(6)` automatically.
///
/// **RNG parity:** C# `CombatState.CreateCreature` rolls each enemy's HP via
/// `creature.SetUniqueMonsterHpValue(side, RunState.Rng.Niche)` — one
/// `Niche` draw per enemy, even when `MinInitialHp == MaxInitialHp`
/// (BattleFriend V1/V2/V3 = 75/150/300, MysteriousKnight = 101 from
/// FlailKnight, FakeMerchantMonster = 165). [`roll_unique_max_hps`] is the
/// faithful mirror (same stream, same two-pass uniqueness), so this advances
/// the shared cursor exactly like a normal encounter entry and keeps the
/// event→combat path parity-aligned. The room type is `Unknown` (an
/// event-nested combat is not a Monster/Elite/Boss map room). Bead
/// `sts2-rs-ck9z6` (GH #166).
#[must_use]
pub fn build_event_combat(
    rs: &mut RunState,
    req: &sts2_core::events::EventCombatRequest,
) -> CombatState {
    use sts2_core::events::{BattlewornDummySetting, EventEncounter};
    use sts2_core::runstate::RoomType;
    let kind = match req.encounter {
        EventEncounter::BattlewornDummy(BattlewornDummySetting::Setting1) => {
            MonsterKind::BattleFriendV1
        }
        EventEncounter::BattlewornDummy(BattlewornDummySetting::Setting2) => {
            MonsterKind::BattleFriendV2
        }
        EventEncounter::BattlewornDummy(BattlewornDummySetting::Setting3) => {
            MonsterKind::BattleFriendV3
        }
        EventEncounter::MysteriousKnight => MonsterKind::MysteriousKnight,
        EventEncounter::FakeMerchant => MonsterKind::FakeMerchantMonster,
    };
    let hp =
        sts2_core::monsters::roll_unique_max_hps(&[kind], &mut rs.rng.niche, rs.ascension_level)
            .first()
            .copied()
            .unwrap_or(0);
    let roster = [(kind, MonsterSpawnArgs::None, hp)];
    enter_combat_from_roster(rs, &roster, RoomType::Unknown)
}

#[must_use]
pub fn enter_combat(rs: &RunState, encounter: &EncounterSpec) -> CombatState {
    // Legacy [`EncounterSpec`] path: members carry pinned HP and no
    // spawn args. The EncounterDef-based run-loop builder
    // (`run_callbacks::build_combat_for_room_act1`) calls
    // [`enter_combat_from_roster`] directly with rolled HP + real
    // [`MonsterSpawnArgs`]; this wrapper keeps the spec-driven callers
    // (scenario fixtures, the `select_encounter` fallback for Acts 2–3)
    // working unchanged.
    let roster: Vec<(MonsterKind, MonsterSpawnArgs, i32)> = encounter
        .members
        .iter()
        .map(|m| (m.kind, MonsterSpawnArgs::None, m.hp))
        .collect();
    // Spec-driven path has no map/room context, so the room-type flags stay
    // unset (the prior `..default()` behavior). The live full-run builder
    // (`run_callbacks::build_combat_for_room`) carries the real `RoomType`.
    enter_combat_from_roster(rs, &roster, sts2_core::runstate::RoomType::Unknown)
}

/// The [`SpawnPosition`] of roster entry `index` in a roster of `len`
/// monsters: entry 0 is the front, the last entry of a multi-monster roster is
/// the back, and `index` is the roster index. Encounters hand C# monsters in
/// this order (`EncounterModel.GenerateMonsters`), and per-slot openers key on
/// it, e.g. `ToadpolesWeak` makes its first toad `IsFront` (SPIKEN) and the
/// second the back one (WHIRL), `Toadpole.cs:79-80`.
#[must_use]
pub fn spawn_position(index: usize, len: usize) -> SpawnPosition {
    SpawnPosition {
        alone: len == 1,
        front: index == 0,
        back: len > 1 && index == len - 1,
        index: u8::try_from(index).unwrap_or(u8::MAX),
    }
}

/// Spawn a resolved `roster` of `(kind, spawn_args, max_hp)` triples into
/// `state` as its enemies, in roster order: each monster's AI is built from
/// its spawn args and [`spawn_position`] at `state.ascension`, then every
/// monster's on-spawn powers are applied
/// ([`CombatState::apply_creature_spawn_buffs`]). Returns the new enemies'
/// uids in roster order.
///
/// The ONE place a fight's opening roster becomes creatures. The run path
/// ([`enter_combat_from_roster`]), the `sts2-cli` scenario builder and the
/// C#-oracle scenario builder (`sts2-wire`) all call it, so they cannot drift
/// apart again: the scenario builder used to give every monster slot 0's
/// position and drop the spawn args (GH #762).
///
/// Ordering follows C#. `CombatRoom.EnterInternal` adds the player before
/// `StartCombat` creates and adds every monster, and each monster's
/// `AfterAddedToRoom` runs only once all of them are in the room. So the
/// on-spawn powers go on after the whole roster is pushed, and the player's
/// mirror creature is wired first if the caller has not done it yet
/// ([`CombatState::init_player_creature`], which reads the player's HP, block
/// and powers at that moment): a monster that debuffs its opponents on spawn
/// (Rocket's Surrounded, `Rocket.cs:76`) needs the player there to land on.
///
/// HP is NOT rolled here. Callers roll it with
/// [`sts2_core::monsters::roll_unique_max_hps`] off the run's `niche` stream
/// (C# `Creature.SetUniqueMonsterHpValue`), which is the stream the combat
/// carries forward for a mid-fight summon's HP.
pub fn spawn_roster(
    state: &mut CombatState,
    roster: &[(MonsterKind, MonsterSpawnArgs, i32)],
) -> SmallVec<[CreatureUid; 8]> {
    let n = roster.len();
    let mut uids: SmallVec<[CreatureUid; 8]> = SmallVec::with_capacity(n);
    for (i, &(kind, args, hp)) in roster.iter().enumerate() {
        let raw = u32::try_from(state.creatures.len() + 1).expect("creature arena fits in u32");
        let id = NonZeroU32::new(raw).expect("non-zero");
        state.creatures.push(Creature {
            id,
            current_hp: hp,
            max_hp: hp,
            block: 0,
            powers: SmallVec::new(),
            species: Some(kind),
            monster_id: MonsterId(python_name(kind)),
            intent: None,
            intent_history: IntentHistory::default(),
            ai: Some(build_ai_with_ascension(
                kind,
                args,
                spawn_position(i, n),
                state.ascension,
            )),
        });
        let uid = CreatureUid(id);
        state.enemy_uids.push(uid);
        uids.push(uid);
    }
    if state.player.creature_uid.is_none() {
        let _ = state.init_player_creature();
    }
    for &uid in &uids {
        state.apply_creature_spawn_buffs(uid);
    }
    uids
}

/// Build a fresh combat from `rs` + a resolved `roster` of
/// `(kind, spawn_args, hp)` triples.
///
/// Identical run-side setup to [`enter_combat`] (player HP, master-deck →
/// draw pile, relic mirror, act snapshot, Setup→PlayerTurnStart advance);
/// the only difference is that each enemy's HP and [`MonsterSpawnArgs`]
/// come pre-resolved from the caller rather than from a pinned
/// [`EncounterSpec`]. This lets the run loop roll roster randomizers +
/// per-monster starting HP from the content [`crate::encounters::EncounterDef`]
/// table while reusing one combat-assembly path.
#[must_use]
pub fn enter_combat_from_roster(
    rs: &RunState,
    roster: &[(MonsterKind, MonsterSpawnArgs, i32)],
    room_type: sts2_core::runstate::RoomType,
) -> CombatState {
    use sts2_core::runstate::RoomType;
    // Carry the run's ascension level into combat so monster HP
    // (ToughEnemies, >= A8) and attack damage (DeadlyEnemies, >= A9)
    // scale per the C# `GetValueIfAscension` branches. Without this the
    // combat-side `state.ascension` stayed 0 and every enemy was spawned
    // at non-ascension stats regardless of the run's ascension.
    // Carry the run's RNG into the combat. C# has ONE persistent
    // `RunState.Rng` (a `RunRngSet`) for the whole run; combat reads its
    // named sub-streams cumulatively — enemy intents off `RunRng.MonsterAi`
    // (`MonsterModel.RollMove` → `RunRng.MonsterAi`), the deck shuffle off
    // `RunRng.Shuffle` (`PopulateCombatState(RunState.Rng.Shuffle, …)`), HP
    // off `RunRng.Niche`. Cloning `rs.rng` in (and writing it back in
    // [`exit_combat`]) makes the combat advance those same streams instead of
    // the default seed-0 set the builder used to leave in place — the latter
    // gave every full-run combat wrong enemy intents / draw order (the
    // `monster_ai` half of bead `sts2-rs-jenwp` M3b; the run_parity lockstep
    // diverged on enemy damage at the first combat). `rs.rng.niche` was
    // already advanced by the caller's per-monster HP roll, so the clone
    // captures the post-HP cursor. `state.rng` (the legacy non-stream combat
    // RNG, no C# analog) is seeded deterministically from the run's master
    // seed to match the combat-parity builder (`sts2-py` lib.rs).
    let mut state = CombatState {
        ascension: rs.ascension_level,
        rng: sts2_core::dotnet_rng::Rng::new(rs.rng.seed),
        run_rng: rs.rng.clone(),
        ..CombatState::default()
    };

    // Carry the run's character class into combat. C# combats know the
    // player's `Character`; the Rust full-run combat builder left
    // `player.character == None` (the `CombatState::default()` value),
    // which silently broke character-gated combat-start setup. The most
    // load-bearing case is the Defect orb belt: `orbs::channel_orb` only
    // grants the canonical 3-slot belt when
    // `player.character == Some(Defect)` — otherwise a zero-capacity belt
    // gets the Ironclad-style TEMPORARY 1-slot belt (`AddSlots(1)`), so the
    // Defect's second channel (Cracked Core's combat-start Lightning, then
    // a Zap) overflow-EVOKES the first orb instead of holding both. That
    // surfaced in run_parity as an early "enemies_hp" divergence
    // (e.g. RP40 Nibbit `cs=[42] rs=[33]`: Rust evoked a Lightning on the
    // 2nd channel for 9 immediate damage C# never dealt). The combat-from-
    // dict builder (`sts2-py` lib.rs) already sets `character: Some(..)`;
    // mirror it here. Bead `sts2-rs-jenwp` (M3b combat-RNG / Defect orbs).
    state.player.character = Some(rs.character_id);

    state.player.current_hp = rs.current_hp;
    state.player.max_hp = rs.max_hp;
    // Bead `sts2-rs-RP17`: snapshot the run gold into the combat so combat-side
    // relics (`SealOfGold`) can read/spend it; `exit_combat` writes it back.
    state.gold = rs.gold;
    let _ = state.init_player_creature();

    // C# `Player.PopulateCombatState` appends the deck to the draw pile in
    // `Deck.Cards` (master-deck) order via `DrawPile.AddInternal` and THEN
    // `RandomizeOrderInternal` shuffles it. `add_card_to_draw_pile` inserts
    // at the FRONT (top-of-draw semantics for mid-combat card generation),
    // so iterating the deck forward would build a *reversed* pre-shuffle
    // pile — and the same Fisher-Yates (`run_rng.shuffle`, same cursor)
    // applied to a reversed list yields a different opening hand than C#.
    // Feed the deck in REVERSE so the front-inserts land the pre-shuffle
    // pile in forward master-deck order, matching C#'s append. Bead
    // `sts2-rs-jenwp` (M3b — first-combat draw parity).
    let mut deck_card_uids: Vec<sts2_core::CardUid> = rs
        .master_deck
        .iter()
        .rev()
        .map(|card| state.add_card_to_draw_pile(*card))
        .collect();
    // sts2-agent fork: each deck card's combat uid, by deck index (C#
    // `DeckVersion`), so the exit finds a combat card's permanent card by
    // identity, not by its combat state.
    deck_card_uids.reverse();
    state.deck_card_uids = deck_card_uids;

    // Bead `sts2-rs-01o`: seed the combat-side potion belt from the
    // run-level belt so potions procured out of combat (Lost Coffer,
    // PhialHolster, Tiny Mailbox …) are drinkable this fight. Capped at
    // the combat-side `MAX_POTIONS` ceiling; `exit_combat` writes the
    // survivors back.
    state.player.potions.clear();
    for &kind in rs.potions.iter().take(sts2_core::potions::MAX_POTIONS) {
        state.player.potions.push(kind);
    }
    // Physical slot mapping rides along (C# `_potionSlots` hole
    // semantics) so a mid-combat drink + later drop keeps slot order.
    // A desynced run-level mapping (fixture pushed `rs.potions` raw)
    // falls back to the compact 0..len identity.
    state.player.potion_slot_indices.clear();
    if rs.potion_slot_indices.len() == rs.potions.len() {
        for &s in rs
            .potion_slot_indices
            .iter()
            .take(sts2_core::potions::MAX_POTIONS)
        {
            state.player.potion_slot_indices.push(s);
        }
    } else {
        for i in 0..state.player.potions.len() {
            state.player.potion_slot_indices.push(i as u8);
        }
    }
    // Bead `sts2-rs-eysmt`: carry the run's potion-slot cap (C#
    // `Player.MaxPotionCount`, default `initialMaxPotionSlotCount == 3`)
    // into combat so slot-filling potions (EntropicBrew / Alchemize /
    // DelicateFrond) fill only up to the player's real slot count rather
    // than the hard `MAX_POTIONS` = 5. Clamped to `MAX_POTIONS` and to
    // `u8` since the combat-side belt's inline storage is `MAX_POTIONS`.
    state.player.potion_slots =
        u8::try_from((rs.max_potion_slots.max(0) as usize).min(sts2_core::potions::MAX_POTIONS))
            .unwrap_or(sts2_core::potions::MAX_POTIONS as u8);

    // Relic mirror (bead `sts2-rs-pfiwm`): materialize every
    // run-carried relic into a combat-side [`sts2_core::Relic`] row so
    // its registered listeners fire. The per-relic listeners self-guard
    // via `player_owns_relic(state, kind)` and the global dispatch
    // registry is installed once, so a non-empty `state.relics` keyed
    // by [`sts2_core::RelicKind`] is all that is required — no per-relic
    // wiring here.
    //
    // - **Order** is preserved from `rs.relics`: relic trigger order is
    //   observable (C# iterates the owned list in pickup order; see
    //   `sts2-rs-m4jaj` for the owned-list dispatch order).
    // - **Counter** is seeded from the run-side `relic_counters`
    //   (cross-combat countdown / accumulator state — EmberTea,
    //   Nunchaku, LavaLamp's took-damage flag …), falling back to
    //   [`sts2_core::RelicKind::initial_counter`] for a relic that has
    //   never been through a combat (the run side starts
    //   `relic_counters` empty and does not initialize at pickup).
    // - The five W9 Neow reward relics map to `None` via
    //   [`sts2_core::RelicId::to_relic_kind`] (no combat behavior) and
    //   are skipped (bead `sts2-rs-4rwv4`).
    //
    // This subsumes the former LavaLamp-only shim; LavaLamp now flows
    // through the general path (its `reset_lava_lamp_flag` run-side
    // reset before combat still applies).
    //
    // The rows are collected first and handed over in one `extend`: every
    // `RelicStore` write rebuilds its per-combat relic plan (M01).
    let mut relics = Vec::with_capacity(rs.relics.len());
    for (i, &relic_id) in rs.relics.iter().enumerate() {
        let Some(kind) = relic_id.to_relic_kind() else {
            continue;
        };
        let counter = rs
            .relic_counters
            .get(&relic_id)
            .copied()
            .unwrap_or_else(|| kind.initial_counter());
        let raw = u32::try_from(i + 1).expect("relic count fits in u32");
        relics.push(sts2_core::Relic {
            id: NonZeroU32::new(raw).expect("non-zero"),
            kind,
            counter,
            // ToyBox wax/melt state rides along so the combat-side hook
            // filter (C# `CombatState.cs:130-137`) sees the same disabled
            // relics the run does. A melted relic still occupies its slot.
            is_wax: rs.wax_relics.contains(&relic_id),
            is_melted: rs.melted_relics.contains(&relic_id),
        });
    }
    state.relics.extend(relics);

    state.act_index = rs.act_index;

    spawn_roster(&mut state, roster);

    // Bead `sts2-rs-9i1c2`: advance through Setup → PlayerTurnStart →
    // PlayerPlayPhase so the returned state is in the resting phase the
    // dispatch table's `play_card` / `end_turn_dispatch` callers expect.
    // Mirrors Python's `enter_combat` which calls `start_combat` +
    // `start_player_turn` before returning to the agent. Without this,
    // the first `combat_loop::play_card` debug-asserts on
    // `state.phase == PlayerPlayPhase` (combat_loop.rs:619-ish family).
    // Plumb the room-type flags onto the combat BEFORE the opening hand is
    // drawn inside `start_player_turn` (→ `dispatch_modify_hand_draw`). C#
    // gates these off `RunState.CurrentRoom.RoomType` directly; this Rust
    // two-layer split snapshots the flag onto `CombatState` at setup. Setting
    // them on the RETURNED state would be too late for round-1 draw modifiers
    // — BoomingConch's Ancient relic adds +2 cards on round 1 of an Elite
    // fight via `ModifyHandDraw` (C#
    // `MegaCrit.Sts2.Core.Models.Relics/BoomingConch.cs`: `RoundNumber > 1`
    // OR `CurrentRoom.RoomType != RoomType.Elite` → no bonus). `is_boss`
    // likewise gates SwordOfStone's elite-victory counter and the
    // StoneCracker boss draw-pile-upgrade.
    state.is_elite = room_type == RoomType::Elite;
    state.is_boss = room_type == RoomType::Boss;

    let _ = sts2_core::combat_loop::start_combat(&mut state);
    let _ = sts2_core::combat_loop::start_player_turn(&mut state);

    state
}

/// Write combat results back to `rs`.
///
/// What's wired (thin slice):
/// - `rs.current_hp ← state.player.current_hp` (clamped to
///   `rs.max_hp` defensively — combat-side healing shouldn't exceed
///   max_hp, but the floor is what we read back regardless).
///
/// What is **not** wired here (per module docs):
/// - Gold drops (no Reward layer at this pin).
/// - Reward queueing (`offer_card_reward` etc. — P9.1.e).
/// - `pending_combat_gold_loss` drain — already a `RunState` method
///   and called explicitly by the DEBT-relic flow.
///
/// What IS wired:
/// - Relic-counter flushback — each run-carried relic's combat-side
///   `counter` is written back into `rs.relic_counters` keyed by
///   `RelicId` (symmetric with the `enter_combat` mirror). Bead
///   `sts2-rs-pfiwm`.
/// - FishingRod every-3rd-combat deck upgrade (bead
///   `sts2-rs-v0107-content-port-wb8lk.6`).
///
/// FishingRod's `Combats` DynamicVar period (v0.107.1 `FishingRod.cs:40`,
/// `new DynamicVar("Combats", 3m)`).
const FISHING_ROD_COMBATS: i32 = 3;

pub fn exit_combat(rs: &mut RunState, combat: &CombatState) {
    // Write the combat-side max_hp back to the run BEFORE clamping current_hp.
    // C# shares one Player object across CombatManager/RunState, so in-combat
    // max-HP gains (Feed kill -> CreatureCmd.GainMaxHp, FruitJuice potion, etc.)
    // persist run-wide with no writeback. Rust's two-layer split copies max_hp
    // IN at enter_combat but previously never copied it OUT, silently dropping
    // every combat-side max-HP change and even clamping current_hp to the stale
    // value. `combat.player.max_hp` is kept in sync by `creature_gain_max_hp`.
    rs.max_hp = combat.player.max_hp;
    rs.current_hp = combat.player.current_hp.min(rs.max_hp);

    // Bead `sts2-rs-01o`: write the combat-side potion belt back to the
    // run-level belt, so potions drunk this fight are gone and any kept
    // ones persist to the next room. Combat-only potions added at fight
    // start (e.g. PetrifiedToad's Potion-Shaped Rock) currently round-
    // trip too — a faithfulness nuance left for a follow-up.
    rs.potions = combat.player.potions.iter().copied().collect();
    // Carry the physical slot mapping back too — holes drunk this fight
    // must keep steering where the next reward-claim drop lands.
    rs.potion_slot_indices =
        if combat.player.potion_slot_indices.len() == combat.player.potions.len() {
            combat.player.potion_slot_indices.iter().copied().collect()
        } else {
            (0..rs.potions.len() as u8).collect()
        };

    // Relic-counter flushback (bead `sts2-rs-pfiwm`): write each
    // run-carried relic's combat-side `counter` back into
    // `rs.relic_counters` keyed by [`sts2_core::RelicId`], so
    // cross-combat countdown / accumulator state persists to the next
    // room. This is the symmetric inverse of the `enter_combat` mirror
    // and subsumes the former LavaLamp-only flushback — e.g. the
    // run-side `modify_card_reward_options_late` listener (bead
    // `sts2-rs-67co3`) reads LavaLamp's post-combat "took damage" flag,
    // and EmberTea / Nunchaku persist their decremented countdowns.
    //
    // Keyed by walking `rs.relics` (the run's own owned list): each
    // `RelicKind` maps back to a unique `RelicId`, and combat-only rows
    // not carried by the run are ignored. Disjoint-field borrow of
    // `rs.relics` (read) and `rs.relic_counters` (write).
    for &relic_id in &rs.relics {
        let Some(kind) = relic_id.to_relic_kind() else {
            continue;
        };
        if let Some(r) = combat.relics.iter().find(|r| r.kind == kind) {
            rs.relic_counters.insert(relic_id, r.counter);
        }
    }

    // Bead `sts2-rs-run-rest-option`: PumpkinCandle (v0.107.1)
    // `AfterCombatEnd` decrements its `KindleCount` by 1, floored at 0
    // (C# `MegaCrit.Sts2.Core.Models.Relics/PumpkinCandle.cs:71-75`:
    // `KindleCount = Math.Max(KindleCount - 1, 0)`). The count lives in
    // `rs.relic_counters[PumpkinCandle]` (mirrored into combat by
    // `enter_combat` and flushed back above); apply the per-combat
    // decay here so the +1-max-energy window lasts exactly 5 combats
    // per Rekindle. Owner-gated. Runs before post-combat reward
    // fan-out, so the pickup-combat's Rekindle (a post-combat reward)
    // isn't clobbered.
    if rs.owns_active_relic(sts2_core::RelicId::PumpkinCandle) {
        if let Some(c) = rs
            .relic_counters
            .get_mut(&sts2_core::RelicId::PumpkinCandle)
        {
            *c = (*c - 1).max(0);
        }
    }

    // Bead `sts2-rs-nfoh0`: ForbiddenGrimoirePower.AfterCombatEnd. C#
    // `ForbiddenGrimoirePower.cs` loops `Amount` times calling
    // `room.AddExtraReward(player, new CardRemovalReward(player))` — i.e. each
    // stack adds one "remove a card from your deck" reward to the post-combat
    // reward set. In this codebase's two-layer split the combat-side
    // `after_combat_end` power hook is read-only (`&CombatState`) and cannot
    // reach the run-level reward queue, so the `exit_combat` seam — the
    // designated place combat-end state becomes run effects — is where the
    // translation belongs (mirroring C#'s `CombatRoom` bridging combat + its
    // rewards). Read the player's stacks via the mirror+legacy union and stage
    // that many card-removal picks; `RunManager::drain_pending_rewards`
    // resolves them through `remove_cards_from_deck`. This generalizes to the
    // other `AddExtraReward` powers (Heist / Swipe / Royalties / TheHunt) as
    // they are ported — each adds its own arm here.
    let grimoire_stacks = combat.player_power_amount(sts2_core::PowerKind::ForbiddenGrimoire);
    // sts2-agent fork: under `interactive_rewards` each `AddExtraReward` power
    // stages its reward on `rs.room_extra_rewards` for the room's rewards
    // screen (C# appends `CombatRoom.ExtraRewards` to the room's set) instead
    // of settling it here.
    let extras = rs.interactive_rewards && combat.player_won;
    if !extras && grimoire_stacks > 0 {
        rs.add_card_removal_reward(grimoire_stacks as u32);
    }

    // Bead `sts2-rs-RP15`: combat gold-reward proportion. C#
    // `CombatRoom.cs:216` `GoldProportion = 1 - EscapedCreatures.Count /
    // Encounter.MonstersWithSlots.Count` — every monster that FLEES
    // (`Intent::Escape`, e.g. a ThievingHopper after a steal) shrinks the gold
    // reward; all fled ⇒ 0 gold. Total = the spawned enemy roster
    // (`enemy_uids`; mid-combat summons aren't in C#'s `MonstersWithSlots`, but
    // an escaping summon is a non-issue here). The gold roll
    // (`apply_combat_gold_and_potion` → `roll_encounter_gold`) reads this.
    //
    // v0.107.1 GremlinMercNormal override (bead `…wb8lk.12`):
    // `GremlinMercNormal.CalculateGoldProportion` (`GremlinMercNormal.cs:50-68`)
    // replaces the base `1 - escaped/total` for that ONE encounter — 1.0 if no
    // FatGremlin escaped, 0.5 if a FatGremlin escaped WITHOUT stealing (was 0%
    // under the base formula), 0.0 if it escaped WITH stolen gold. STILL NOT
    // wired here, but the reason has narrowed: bead
    // `sts2-rs-surprise-death-spawn` ported `SurprisePower.AfterDeath`
    // (`monsters::act1_underdocks::surprise_after_death_spawn`), so the
    // SneakyGremlin + FatGremlin now spawn and the FatGremlin's `Intent::Escape`
    // can fire — the `fat_gremlin_escaped` branch is REACHABLE from here on.
    // What is still missing is a per-creature escape record (the engine only
    // counts `combat.escaped_creatures`), which is what the 0.5-vs-0.0 split
    // keys off; `ThieveryPower.Steal` accumulation IS ported now (GH #368.4,
    // and GH #604 wired its `HeistPower` gold-back below).
    // Follow-up; the base formula below is what actually runs.
    let total_monsters = combat.enemy_uids.len();
    rs.last_combat_gold_proportion = if total_monsters == 0 {
        1.0
    } else {
        1.0 - f64::from(combat.escaped_creatures) / total_monsters as f64
    };

    // Bead `sts2-rs-RP17`: write back the combat-time gold (SealOfGold's
    // per-turn `-5` spends). `enter_combat` snapshotted `rs.gold` in and nothing
    // outside combat touched it meanwhile, so this applies exactly the in-combat
    // delta (a no-op for combats with no gold-spending relic). Runs BEFORE the
    // reward gold (`apply_combat_gold_and_potion`), matching C# (gold spent
    // during the fight, rewards added after).
    rs.gold = combat.gold;

    // Royalties combat-victory gold (C# `RoyaltiesPower.AfterCombatEnd` →
    // `room.AddExtraReward(player, new GoldReward(Amount))`). The Regent's
    // `RoyaltiesCard` applies N stacks of `Royalties` mid-combat (Gold.BaseValue
    // = 30); on a WON combat each stack is worth 1 gold, auto-collected with the
    // rest of the reward set. This is the `Royalties` arm the `AddExtraReward`
    // TODO above (Heist / Swipe / Royalties / TheHunt) flagged as pending — a
    // flat grant (no RNG, stream-faithful). Granted here AFTER the RP17
    // `rs.gold = combat.gold` writeback so it isn't clobbered, and gated on
    // victory (a lost combat has no rewards screen, so the extra reward is never
    // collected — mirrors C# and the `royalties_gold_on_victory` helper's
    // `_on_victory` contract). The base reward gold is added separately by
    // `apply_combat_gold_and_potion`; final = base + royalties, matching C#.
    if combat.player_won {
        let royalties_stacks = combat.player_power_amount(sts2_core::PowerKind::Royalties);
        if royalties_stacks > 0 && !extras {
            rs.gain_gold(sts2_core::powers::regent::royalties_gold_on_victory(
                royalties_stacks,
            ));
        }
    }

    // GH #604: `HeistPower.BeforeDeath` gold-back. C# `HeistPower.cs:19-28`
    // adds a `GoldReward(Amount, wasGoldStolenBack: true)` to the combat room
    // when the gremlin CARRYING the merc's stolen gold is killed; a gremlin
    // that flees keeps it. The engine banks the amount on
    // `CombatState::heist_recovered_gold` at the damage-death chokepoint
    // (`damage.rs`, next to the `SwipePower` card reclaim), so all that is left
    // here is settling it into run gold. This is the `Heist` arm of the
    // `AddExtraReward` TODO the `Royalties` block above started working
    // through.
    //
    // Victory-gated for the same reason as Royalties: an extra reward is
    // collected off the rewards screen, which a lost combat never shows. Flat
    // (no RNG), so — like Royalties — it cannot desync the `rewards` stream.
    // Runs AFTER the RP17 `rs.gold = combat.gold` writeback so it is not
    // clobbered by it.
    if combat.player_won && combat.heist_recovered_gold > 0 && !extras {
        rs.gain_gold(combat.heist_recovered_gold);
    }

    // Run-master propagation for scaling cards (C# `DeckVersion.BuffFromPlay`).
    // The Scythe's `OnPlay` buffs BOTH its combat copy AND the run-master deck
    // card (`(base.DeckVersion as TheScythe)?.BuffFromPlay(intValue)`,
    // `TheScythe.cs:73`) — `IncreasedDamage` / `CurrentDamage` are
    // `[SavedProperty]`, so the growth persists for the whole run. In this
    // two-layer split the combat copy's run-persistent `base_damage` carries
    // the accumulated value; fold it back onto the corresponding master-deck
    // card so the next combat starts boosted. Bead `sts2-rs-RP114`.
    propagate_scaling_cards_to_master_deck(rs, combat);

    // Bead `sts2-rs-RP13`: ThievingHopper / SwipePower run-deck steal. C#
    // `SwipePower.Steal` permanently removes the stolen card from the run deck
    // (`CardPileCmd.RemoveFromDeck(card.DeckVersion)`); a thief that DIES
    // restores it (`SwipePower.BeforeDeath`), which the combat already recorded
    // as `reclaimed`. sts2-agent fork: under the rewards screen a restored card
    // is a special card reward, out of the deck until claimed, and the room's
    // extra rewards are staged with it. Both run after the deck's persistent
    // changes (a scaled Scythe) reach its cards, so a returned card carries
    // them (C# returns the updated `DeckVersion`), and before Fishing Rod's
    // combat-end upgrade, which picks only from the deck.
    settle_swiped_cards_and_extras(rs, combat, extras);

    // Flush the combat's RNG streams back to the run. C# uses ONE
    // `RunState.Rng` for the whole run, so a combat's advances to
    // `MonsterAi` / `Shuffle` / `Niche` (intents, draws, mid-combat summon
    // HP) persist to the next room. [`enter_combat_from_roster`] cloned
    // `rs.rng` in; this writes the post-combat cursors back. Streams the
    // combat never touched (`run_manager`, `up_front`, the player reward
    // streams, …) round-trip unchanged, so this is the exact symmetric
    // inverse of the carry-in. Bead `sts2-rs-jenwp` (M3b monster_ai).
    rs.rng = combat.run_rng.clone();

    // Bead `sts2-rs-v0107-content-port-wb8lk.6`: FishingRod.AfterCombatEnd
    // (v0.107.1 `FishingRod.cs:42-60`). On every normal (`RoomType.Monster`)
    // combat the relic's `CombatsSeen` counter ticks; on every 3rd one it
    // upgrades a random upgradable deck card via the `Niche` stream
    // (`Rng.Niche.NextItem(Deck.Cards.Where(IsUpgradable))`). Elite/Boss
    // rooms are excluded (C# early-returns when `RoomType != Monster`).
    // Run-side because it mutates the master deck (combat-end → run effect),
    // mirroring the Grimoire/Swipe arms above.
    //
    // MUST run AFTER the `rs.rng = combat.run_rng.clone()` flushback above:
    // C# reads the single persistent `RunState.Rng.Niche` in `AfterCombatEnd`,
    // i.e. the cursor AS ADVANCED by the just-finished combat (mid-combat
    // niche draws — summon HP, etc.). Pre-flushback `rs.rng.niche` is still the
    // stale pre-combat cursor (in-combat draws advanced `combat.run_rng`, the
    // clone). Drawing here, after the flushback, both (a) reads the correct
    // post-combat cursor and (b) lets the FishingRod advance persist into the
    // run stream — pre-flushback it would have been clobbered by line 509.
    // C# reaches `Hook.AfterCombatEnd` (where FishingRod ticks) only via
    // `CombatManager.EndCombatInternal`, which fires on a WON combat.
    // `ProcessPendingLoss` (defeat) never calls it, so a lost combat neither
    // advances `CombatsSeen` nor rolls the Niche upgrade. Gate on `player_won`
    // (run-starter-upgrade-flip, part B).
    if combat.player_won
        && rs.owns_active_relic(sts2_core::RelicId::FishingRod)
        && !combat.is_elite
        && !combat.is_boss
    {
        let combats_seen = {
            let counter = rs
                .relic_counters
                .entry(sts2_core::RelicId::FishingRod)
                .or_insert(0);
            *counter = counter.saturating_add(1);
            *counter
        };
        if combats_seen % FISHING_ROD_COMBATS == 0 {
            // `PileType.Deck.GetPile(Owner).Cards.Where(IsUpgradable)` — deck
            // order preserved so the Niche index maps to the same card C#
            // would pick. `NextItem` is `NextInt(0, count)` (maxExclusive),
            // i.e. `next_int(0, count - 1)` inclusive (see `dotnet_rng`).
            let upgradable: Vec<usize> = rs
                .master_deck
                .iter()
                .enumerate()
                .filter(|(_, c)| c.is_upgradable())
                .map(|(i, _)| i)
                .collect();
            if let Ok(last) = i32::try_from(upgradable.len().wrapping_sub(1)) {
                if !upgradable.is_empty() {
                    let idx = rs.rng.niche.next_int(0, last) as usize;
                    let deck_idx = upgradable[idx];
                    rs.master_deck[deck_idx].upgrade();
                }
            }
        }
    }

    // `ToyBox.AfterCombatEnd` (`MegaCrit.Sts2.Core.Models.Relics/ToyBox.cs:99-112`):
    //
    // ```
    // CombatsSeen++;
    // if (CombatsSeen % DynamicVars["Combats"] == 0) {
    //     var r = Owner.Relics.FirstOrDefault(r => r != null && r.IsWax && !r.IsMelted);
    //     if (r != null) await RelicCmd.Melt(r);
    // }
    // ```
    //
    // Increment FIRST, then the modulo test — so the first melt lands on the
    // 3rd combat, not the 4th. `CombatsSeen` rides in `rs.relic_counters`
    // (mirrored into / out of combat like every other run-carried counter).
    // Same `player_won` gate as FishingRod above: `Hook.AfterCombatEnd` is
    // reached only from `CombatManager.EndCombatInternal`, which a defeat
    // (`ProcessPendingLoss`) never calls.
    if combat.player_won && rs.owns_active_relic(sts2_core::RelicId::ToyBox) {
        let combats_seen = {
            let counter = rs
                .relic_counters
                .entry(sts2_core::RelicId::ToyBox)
                .or_insert(0);
            *counter = counter.saturating_add(1);
            *counter
        };
        if combats_seen % crate::relics::shop_event::TOY_BOX_COMBATS == 0 {
            // Melting does NOT remove the relic — it stays in `rs.relics`
            // (and so keeps counting for relic-count effects and the shop's
            // already-owned exclusion); it only drops out of every hook
            // listener list. `None` once all 4 wax relics are gone.
            let _ = rs.melt_next_wax_relic();
        }
    }
}

/// Cards whose run-persistent `base_damage` accumulates mid-combat and must be
/// folded back onto the master-deck card at `exit_combat` (C#
/// `DeckVersion.BuffFromPlay`). At this pin the only such card is The Scythe
/// (`TheScythe.cs`); add new ids here as more `DeckVersion`-buffing scalers are
/// ported.
const fn is_run_persistent_damage_scaler(id: sts2_core::cards::CardId) -> bool {
    matches!(id, sts2_core::cards::CardId::TheScythe)
}

/// The stolen cards and the room's extra rewards at a combat's exit. Each
/// stolen card is found in `master_deck` by [`sts2_core::resolve_stolen_deck_index`]:
/// its deck link ([`sts2_core::SwipeStolen::deck_index`], C# `DeckVersion`), or
/// for a combat without one by `(card_id, upgraded)`. A card a surviving or escaped thief
/// keeps (`SwipePower.Steal` → `RemoveFromDeck`) leaves the deck for good. With
/// `extras` (sts2-agent fork, `interactive_rewards`, a won fight) the room's
/// extra rewards are staged in the order the game adds them: those the fight
/// added ([`CombatState::room_extra_rewards`]: a killed Heist carrier's gold, a
/// killed thief's card, The Hunt's card reward), then the combat-end ones in
/// the player's power order (`Hook.AfterCombatEnd`: Royalties' gold, one
/// removal per Forbidden Grimoire stack); a killed thief's card leaves the deck
/// as a special card reward, in it again only once claimed. Without `extras` a
/// killed thief's card stays in the deck. Bead `sts2-rs-RP13`.
fn settle_swiped_cards_and_extras(rs: &mut RunState, combat: &CombatState, extras: bool) {
    use sts2_core::{CombatExtraReward, OfferedReward};
    let mut taken: Vec<usize> = combat.unreclaimed_stolen_deck_indices(&rs.master_deck);
    if extras {
        for extra in &combat.room_extra_rewards {
            let offered = match *extra {
                CombatExtraReward::Gold(amount) => OfferedReward::Gold(amount),
                CombatExtraReward::RoomCard => OfferedReward::RoomCard,
                CombatExtraReward::SpecialCard {
                    card_id,
                    upgraded,
                    deck_index,
                } => {
                    let Some(idx) = sts2_core::resolve_stolen_deck_index(
                        &rs.master_deck,
                        deck_index,
                        card_id,
                        upgraded,
                        &taken,
                    ) else {
                        continue;
                    };
                    taken.push(idx);
                    OfferedReward::SpecialCard(rs.master_deck[idx])
                }
            };
            rs.room_extra_rewards.push(offered);
        }
        for puid in combat.player_power_uids() {
            let Some(p) = combat.powers.get_by_id(puid.0) else {
                continue;
            };
            match p.kind {
                sts2_core::PowerKind::Royalties if p.amount > 0 => {
                    rs.room_extra_rewards.push(OfferedReward::Gold(
                        sts2_core::powers::regent::royalties_gold_on_victory(p.amount),
                    ));
                }
                sts2_core::PowerKind::ForbiddenGrimoire => {
                    for _ in 0..p.amount.max(0) {
                        rs.room_extra_rewards.push(OfferedReward::RemoveCard);
                    }
                }
                _ => {}
            }
        }
    }
    taken.sort_unstable();
    for idx in taken.into_iter().rev() {
        rs.master_deck.remove(idx);
    }
}

/// Fold each combat-side scaling card's accumulated run-persistent `base_damage`
/// back onto its master-deck source (C# `DeckVersion.BuffFromPlay`).
///
/// Each master card is paired with its own combat copy through the combat's
/// deck link ([`CombatState::deck_card_uids`], C# `DeckVersion`), so a
/// combat-only change to the copy (an upgrade) doesn't break the pairing.
/// Only a combat built without a run deck falls back to identity matching:
/// same-`(card_id, upgraded)` instances paired in their stable orders (the
/// arena holds the deck cards reversed, generated cards after them). The master
/// card's `base_damage` takes the combat copy's accumulated value, guarded by
/// `max` so a never-played Scythe can't shrink it.
fn propagate_scaling_cards_to_master_deck(rs: &mut RunState, combat: &CombatState) {
    use sts2_core::cards::CardId;

    // sts2-agent fork: with the combat's deck links, each master card takes
    // its own combat copy's value (C# `DeckVersion.BuffFromPlay`), whatever
    // combat-only change (an upgrade) that copy went through.
    if !combat.deck_card_uids.is_empty() && combat.deck_card_uids.len() == rs.master_deck.len() {
        for (m, uid) in rs.master_deck.iter_mut().zip(&combat.deck_card_uids) {
            if !is_run_persistent_damage_scaler(m.card_id) {
                continue;
            }
            if let Some(c) = combat.cards.get(uid.get() as usize - 1) {
                if c.card_id == m.card_id {
                    m.base_damage = m.base_damage.max(c.base_damage);
                }
            }
        }
        return;
    }

    // Distinct scaler ids present in the master deck (tiny set; usually 0–1).
    let mut scaler_ids: SmallVec<[CardId; 2]> = SmallVec::new();
    for c in &rs.master_deck {
        if is_run_persistent_damage_scaler(c.card_id) && !scaler_ids.contains(&c.card_id) {
            scaler_ids.push(c.card_id);
        }
    }

    for id in scaler_ids {
        // Combat copies of this id, in combat-arena (reverse master) order,
        // bucketed by upgrade flag so an upgraded and non-upgraded Scythe in
        // the same deck pair to their own kind.
        let mut combat_base_up: SmallVec<[i32; 2]> = SmallVec::new();
        let mut combat_base_down: SmallVec<[i32; 2]> = SmallVec::new();
        for c in combat.cards.iter().rev() {
            if c.card_id != id {
                continue;
            }
            if c.upgraded {
                combat_base_up.push(c.base_damage);
            } else {
                combat_base_down.push(c.base_damage);
            }
        }

        let (mut next_up, mut next_down) = (0usize, 0usize);
        for m in &mut rs.master_deck {
            if m.card_id != id {
                continue;
            }
            let combat_base = if m.upgraded {
                let v = combat_base_up.get(next_up).copied();
                next_up += 1;
                v
            } else {
                let v = combat_base_down.get(next_down).copied();
                next_down += 1;
                v
            };
            if let Some(combat_base) = combat_base {
                // Only grow — never shrink. The combat copy started at the
                // master value and accumulated; a missing/unplayed copy leaves
                // the master untouched.
                m.base_damage = m.base_damage.max(combat_base);
            }
        }
    }
}

/// Reset the LavaLamp per-combat-flag. Mirrors Python
/// `LavaLamp.before_combat_start` (`shop_event.py:451-452`):
/// `self._took_damage = False` at the start of each combat.
///
/// Call this before [`enter_combat`] when constructing a run-loop
/// driver that hasn't routed `after_room_entered` through the
/// existing run-side reset hook. Idempotent: a no-op when the run
/// doesn't carry LavaLamp.
///
/// Bead `sts2-rs-o62e7`.
pub fn reset_lava_lamp_flag(rs: &mut RunState) {
    if rs.owns_active_relic(sts2_core::RelicId::LavaLamp) {
        rs.relic_counters.insert(sts2_core::RelicId::LavaLamp, 0);
    }
}

/// Map a [`MonsterKind`] to the Python-pin stable name string used by
/// `MonsterId`. Wraps the existing `MonsterKind::python_name` so the
/// bridge has a single seam if the mapping ever needs to diverge.
#[inline]
fn python_name(kind: MonsterKind) -> &'static str {
    kind.python_name()
}

/// v0.107.1 `GremlinMercNormal.CalculateGoldProportion`
/// (`GremlinMercNormal.cs:50-68`): the per-encounter override of the base
/// `1 - escaped/total` gold-reward proportion.
///
/// - No FatGremlin escaped ⇒ `1.0` (full gold).
/// - FatGremlin escaped, gold NOT stolen ⇒ `0.5` (v0.107.1 change — the base
///   formula previously gave 0% here).
/// - FatGremlin escaped WITH stolen gold ⇒ `0.0`.
///
/// Captured as a pure helper so the v0.107.1 value is documented and tested.
/// It is NOT yet wired into [`after_combat`]'s proportion calc. The death-spawn
/// half of the trigger IS now modeled (`SurprisePower.AfterDeath` →
/// `monsters::act1_underdocks::surprise_after_death_spawn`, bead
/// `sts2-rs-surprise-death-spawn`), so a FatGremlin can spawn and flee; what is
/// still missing is a per-creature escape record and the `ThieveryPower.Steal`
/// accumulation that `MarkGoldStolen` keys off. Bead `…wb8lk.12`.
#[must_use]
pub fn gremlin_merc_gold_proportion(fat_gremlin_escaped: bool, gold_was_stolen: bool) -> f64 {
    if !fat_gremlin_escaped {
        return 1.0;
    }
    if gold_was_stolen {
        0.0
    } else {
        0.5
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use sts2_core::encounters::{act1_boss_pool, act1_normal_pool, select_first};

    fn fresh_run() -> RunState {
        RunState {
            current_hp: 72,
            max_hp: 80,
            ..RunState::default()
        }
    }

    #[test]
    fn gremlin_merc_gold_proportion_v0107_branches() {
        // v0.107.1 `GremlinMercNormal.CalculateGoldProportion`. Returned values
        // are exact constants, but clippy's `float_cmp` lint forbids `==`.
        let close = |a: f64, b: f64| (a - b).abs() < f64::EPSILON;
        // No FatGremlin escape ⇒ full gold (gold-stolen flag is irrelevant).
        assert!(close(gremlin_merc_gold_proportion(false, false), 1.0));
        assert!(close(gremlin_merc_gold_proportion(false, true), 1.0));
        // Escaped without stealing ⇒ 50% (was 0% under the base formula).
        assert!(close(gremlin_merc_gold_proportion(true, false), 0.5));
        // Escaped with stolen gold ⇒ 0%.
        assert!(close(gremlin_merc_gold_proportion(true, true), 0.0));
    }

    #[test]
    fn enter_combat_returns_state_in_player_play_phase() {
        // Bead `sts2-rs-9i1c2`: enter_combat must advance Setup →
        // PlayerTurnStart → PlayerPlayPhase before returning so the
        // first `combat_loop::play_card` from the dispatch table
        // doesn't trip the `debug_assert_eq!(state.phase ==
        // PlayerPlayPhase)` guard. Mirrors Python's
        // `enter_combat → start_combat → start_player_turn` sequence.
        let rs = fresh_run();
        let encounter = select_first(act1_normal_pool());
        let state = enter_combat(&rs, encounter);
        assert_eq!(state.phase, sts2_core::CombatPhase::PlayerPlayPhase);
        assert!(state.round >= 1, "start_combat sets round=1");
    }

    /// GitHub issue #215 regression: against TheInsatiable, with the Sandpit
    /// countdown at 1 (the next enemy turn eats the player) and a
    /// `FranticEscape` status in hand, the deployed combat MCTS was NOT
    /// playing FranticEscape and losing. Root cause: in an observed-lost
    /// position the `lost_position_override` swung for maximum 1-ply damage
    /// (`FranticEscape` deals 0, so a Strike always outscored it) and the
    /// player was eaten that very turn — even though playing FranticEscape
    /// (Sandpit +1) survives.
    ///
    /// Re-scoped by the 2026-08-09 A8 audit (FINDINGS §3): this position is
    /// observed-lost but NOT solver-proven, so the override no longer
    /// engages at all — the old every-child-`wins==0` trigger was firing on
    /// depth-capped boss plies that were demonstrably winnable. The #215
    /// user-visible guarantee ("play FranticEscape, don't get eaten") is
    /// now carried by two layers, both pinned here:
    ///
    /// 1. **Unproven (the real search):** `pick_root_action` — the
    ///    `loss_gradient` backup orders the loss band by turns survived /
    ///    damage dealt, so the surviving FranticEscape line outranks the
    ///    lines the Sandpit eats this very turn.
    /// 2. **Proven (rigged):** the override fires and its GH #215
    ///    survival-dominant key still picks FranticEscape over the
    ///    chip-damage Strike.
    #[test]
    fn insatiable_sandpit_lost_position_plays_frantic_escape_not_damage() {
        use sts2_core::card_effects::apply_power_to_creature;
        use sts2_core::cards::CardId;
        use sts2_core::encounters::{EncounterMember, EncounterSpec};
        use sts2_core::potions::CharacterId;
        use sts2_core::powers::PowerKind;
        use sts2_core::{Action, MonsterKind};
        use sts2_mcts::{Mcts, MctsConfig};

        // Process-global registries: the override's survival probe plays
        // FranticEscape (Sandpit +1) and drives an enemy turn, so card
        // dispatch + relic listeners must be installed. All idempotent.
        crate::cards::dispatch::ensure_installed();
        crate::relics::dispatch::ensure_installed();
        sts2_core::relic_effects::ensure_installed();

        // Enter the TheInsatiable boss fight with the Ironclad starter deck.
        static INSATIABLE_MEMBERS: &[EncounterMember] = &[EncounterMember {
            kind: MonsterKind::TheInsatiable,
            hp: 321,
        }];
        static INSATIABLE_SPEC: EncounterSpec = EncounterSpec {
            name: "the_insatiable_boss",
            members: INSATIABLE_MEMBERS,
        };
        let mut rs = fresh_run();
        crate::run_callbacks::populate_starter_deck(&mut rs, CharacterId::Ironclad);
        let mut state = enter_combat(&rs, &INSATIABLE_SPEC);

        // The Insatiable has liquified and the Sandpit is down to 1: the
        // NEXT enemy turn eats the player unless FranticEscape buys a turn.
        let boss = state.enemy_uids[0];
        apply_power_to_creature(&mut state, boss, PowerKind::Sandpit, 1);

        // Isolate Sandpit as the ONLY lethal source: keep the player well
        // above the boss's attack so only the countdown can kill them, and
        // keep the boss far above lethal so no rollout finds a win (⇒ the
        // position is observed-lost and the override engages).
        state.player.current_hp = 200;
        state.player.max_hp = 200;
        state.sync_creature_current_hp_from_player();

        // Deterministic two-card hand: FranticEscape at index 0, a Strike at
        // index 1; 1 energy makes both individually playable. (Overwrites
        // the drawn starter hand — root actions become exactly Play(FE) /
        // Play(Strike) / EndTurn.)
        let push_card = |state: &mut CombatState, id: CardId| -> sts2_core::CardUid {
            let mut card = crate::cards::spec_index::card_pod_from_id(id, false).expect("spec");
            let uid = sts2_core::CardUid::new(state.cards.len() as u32 + 1).expect("nz");
            card.uid = uid;
            state.cards.push(card);
            uid
        };
        let fe = push_card(&mut state, CardId::FranticEscape);
        let strike = push_card(&mut state, CardId::StrikeIronclad);
        state.player.hand = smallvec::smallvec![fe, strike];
        state.player.energy = 1;

        // Sanity: FranticEscape (survives) vs Strike (eaten) is exactly the
        // survival split the override must resolve.
        let mut fe_line = state.clone();
        let _ = sts2_core::apply(
            &mut fe_line,
            Action::PlayCard {
                hand_index: 0,
                target: None,
            },
        );
        let _ = sts2_core::apply(&mut fe_line, Action::EndTurn);
        assert!(
            !fe_line.is_over || fe_line.player_won,
            "playing FranticEscape must survive the Sandpit turn"
        );
        let mut strike_line = state.clone();
        let _ = sts2_core::apply(
            &mut strike_line,
            Action::PlayCard {
                hand_index: 1,
                target: Some(0),
            },
        );
        let _ = sts2_core::apply(&mut strike_line, Action::EndTurn);
        assert!(
            strike_line.is_over && !strike_line.player_won,
            "playing a Strike must be eaten by the Sandpit countdown"
        );

        // Run the real combat MCTS. The boss is unkillable within this
        // budget and the Sandpit eats the player on the very next enemy turn
        // in nearly every rollout (the heuristic rollout won't reliably keep
        // escaping), so the root is observed-lost — but NOT solver-proven.
        let cfg = MctsConfig {
            max_simulations: 2000,
            seed: 7,
            use_transposition_table: false,
            ..MctsConfig::default()
        };
        let mut mcts = Mcts::new(state, cfg);
        // Layer 1 — the deployed pick (`try_search_n` → `pick_root_action`):
        // no proof ⇒ no override; the loss-gradient-ordered Q must surface
        // the surviving FranticEscape line on its own.
        let pick = mcts.search();
        assert!(
            !mcts.root_proven_loss(),
            "rollout-observed losses must not constitute a solver proof"
        );
        assert!(
            mcts.lost_position_override().is_none(),
            "A8 trigger: the override must not engage on an unproven loss"
        );
        assert!(
            matches!(pick, Action::PlayCard { hand_index: 0, .. }),
            "the search itself must play FranticEscape (hand index 0) to \
             survive the Sandpit, not swing for chip damage — got {pick:?}"
        );

        // Layer 2 — with a (rigged) solver proof, the override engages and
        // its GH #215 survival-dominant key must make the same pick.
        let root_idx = mcts.tree.root.0 as usize;
        mcts.tree.arena[root_idx].proven_loss = true;
        let rescued = mcts
            .lost_position_override()
            .expect("a proven-loss root with children must fire the override");
        assert!(
            matches!(rescued, Action::PlayCard { hand_index: 0, .. }),
            "the override's survival key must play FranticEscape — got {rescued:?}"
        );
    }

    #[test]
    fn enter_combat_copies_player_hp_and_max_hp() {
        let rs = fresh_run();
        let encounter = select_first(act1_normal_pool());
        let state = enter_combat(&rs, encounter);
        assert_eq!(state.player.current_hp, 72);
        assert_eq!(state.player.max_hp, 80);
        // Player-Creature mirror also reflects HP.
        let uid = state
            .player
            .creature_uid
            .expect("player-Creature initialized");
        let pc = state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .expect("player-Creature row");
        assert_eq!(pc.current_hp, 72);
        assert_eq!(pc.max_hp, 80);
    }

    #[test]
    fn enter_combat_spawns_encounter_members_with_declared_hp_and_ai() {
        let rs = fresh_run();
        let encounter = select_first(act1_normal_pool());
        let state = enter_combat(&rs, encounter);
        assert_eq!(state.enemy_uids.len(), 1);
        let enemy_uid = state.enemy_uids[0];
        let enemy = state
            .creatures
            .iter()
            .find(|c| c.id == enemy_uid.0)
            .expect("Nibbit row");
        assert_eq!(enemy.species, Some(MonsterKind::Nibbit));
        assert_eq!(enemy.current_hp, 45);
        assert_eq!(enemy.max_hp, 45);
        assert!(enemy.ai.is_some(), "Nibbit AI wired");
    }

    /// End-to-end ascension plumbing (FuzzyWurmCrawler vertical slice).
    /// The run's `ascension_level` must reach combat: `state.ascension`
    /// is populated, and the enemy's attack damage scales via
    /// `build_ai_with_ascension` (C# `AcidGoopDamage` 4 → 6 at A9+).
    /// Before this slice `state.ascension` stayed 0 and the wurm always
    /// telegraphed its non-ascension 4 damage regardless of the run.
    #[test]
    fn enter_combat_from_roster_propagates_ascension_to_state_and_attack_damage() {
        use sts2_core::Intent;

        let acid_goop_damage = |state: &CombatState| {
            let enemy = state
                .creatures
                .iter()
                .find(|c| c.id == state.enemy_uids[0].0)
                .expect("wurm row");
            match enemy
                .ai
                .as_ref()
                .expect("wurm AI wired")
                .current_move()
                .intents[0]
            {
                Intent::Attack { damage } => damage,
                other => panic!("FIRST_ACID_GOOP must be an Attack, got {other:?}"),
            }
        };

        let roster = [(MonsterKind::FuzzyWurmCrawler, MonsterSpawnArgs::None, 57)];

        let base = enter_combat_from_roster(
            &fresh_run(),
            &roster,
            sts2_core::runstate::RoomType::Monster,
        );
        assert_eq!(base.ascension, 0, "A0 run carries ascension 0 into combat");
        assert_eq!(
            acid_goop_damage(&base),
            4,
            "A0 AcidGoop damage is the base 4"
        );

        let rs9 = RunState {
            ascension_level: 9,
            ..fresh_run()
        };
        let deadly =
            enter_combat_from_roster(&rs9, &roster, sts2_core::runstate::RoomType::Monster);
        assert_eq!(
            deadly.ascension, 9,
            "A9 run carries ascension 9 into combat"
        );
        assert_eq!(
            acid_goop_damage(&deadly),
            6,
            "A9 (DeadlyEnemies) scales AcidGoop damage to 6"
        );
    }

    /// Room-type flags are plumbed onto the combat from the entering room's
    /// `RoomType`. `is_elite`/`is_boss` are set at combat setup (before the
    /// opening draw) so round-1 draw modifiers like BoomingConch's elite +2
    /// (`MegaCrit.Sts2.Core.Models.Relics/BoomingConch.cs`) and the
    /// boss-gated SwordOfStone / StoneCracker hooks see the right value.
    #[test]
    fn enter_combat_from_roster_plumbs_room_type_flags() {
        use sts2_core::runstate::RoomType;

        let roster = [(MonsterKind::FuzzyWurmCrawler, MonsterSpawnArgs::None, 57)];

        let elite = enter_combat_from_roster(&fresh_run(), &roster, RoomType::Elite);
        assert!(elite.is_elite, "Elite room sets is_elite");
        assert!(!elite.is_boss, "Elite room leaves is_boss false");

        let boss = enter_combat_from_roster(&fresh_run(), &roster, RoomType::Boss);
        assert!(boss.is_boss, "Boss room sets is_boss");
        assert!(!boss.is_elite, "Boss room leaves is_elite false");

        let normal = enter_combat_from_roster(&fresh_run(), &roster, RoomType::Monster);
        assert!(!normal.is_elite, "Monster room leaves is_elite false");
        assert!(!normal.is_boss, "Monster room leaves is_boss false");
    }

    #[test]
    fn enter_combat_boss_pool_spawns_vantom_at_173() {
        let rs = fresh_run();
        let encounter = select_first(act1_boss_pool());
        let state = enter_combat(&rs, encounter);
        assert_eq!(state.enemy_uids.len(), 1);
        let enemy = state
            .creatures
            .iter()
            .find(|c| c.id == state.enemy_uids[0].0)
            .expect("Vantom row");
        assert_eq!(enemy.species, Some(MonsterKind::Vantom));
        assert_eq!(enemy.current_hp, 173);
        assert!(enemy.ai.is_some(), "Vantom AI wired");
    }

    #[test]
    fn enter_combat_mirrors_run_relics_into_combat() {
        // Bead `sts2-rs-pfiwm`: every run-carried relic is
        // materialized into a combat-side `Relic` row (keyed by
        // `RelicKind`) so its listeners fire.
        let mut rs = fresh_run();
        rs.relics.push(sts2_core::RelicId::BurningBlood);
        let state = enter_combat(&rs, select_first(act1_normal_pool()));
        assert!(
            state
                .relics
                .iter()
                .any(|r| r.kind == sts2_core::RelicKind::BurningBlood),
            "BurningBlood mirrored into combat"
        );
    }

    #[test]
    fn enter_combat_preserves_relic_order() {
        // Relic trigger order is observable — the mirror must keep
        // `rs.relics` pickup order.
        let mut rs = fresh_run();
        rs.relics.push(sts2_core::RelicId::BurningBlood);
        rs.relics.push(sts2_core::RelicId::Anchor);
        rs.relics.push(sts2_core::RelicId::Vajra);
        let state = enter_combat(&rs, select_first(act1_normal_pool()));
        let kinds: Vec<_> = state.relics.iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            vec![
                sts2_core::RelicKind::BurningBlood,
                sts2_core::RelicKind::Anchor,
                sts2_core::RelicKind::Vajra,
            ]
        );
    }

    /// TinkerTime grants a MadScience whose `(type, rider)` morph is packed
    /// into the master_deck card's `VarKey::Generic` slot. That morph must
    /// (1) flow into combat — `enter_combat` clones the deck card — and
    /// (2) survive a combat boundary: after `exit_combat`, a SECOND combat
    /// still sees the moded card. This is the run-persistent storage the
    /// TinkerTime port relies on (no end-of-combat reset on the run side).
    /// Bead `sts2-rs-2guyj`.
    #[test]
    fn mad_science_morph_survives_combat_boundary() {
        use crate::cards::colorless::helpers::{
            pack_tinker_time, set_mad_science_variant, TinkerTimeRider,
        };
        use sts2_core::cards::{CardId, CardType, VarKey};

        let mut rs = fresh_run();
        // Grant a moded MadScience exactly as the TinkerTime rider handler does.
        let mut card = crate::cards::spec_index::card_pod_from_id(CardId::MadScience, false)
            .expect("MadScience registered in COLORLESS_SPECS");
        set_mad_science_variant(&mut card, CardType::Power, TinkerTimeRider::Expertise);
        rs.master_deck.push(card);
        let expected = pack_tinker_time(CardType::Power, TinkerTimeRider::Expertise);

        // (1) The morph flows into the first combat's card store.
        let combat1 = enter_combat(&rs, select_first(act1_normal_pool()));
        let in_combat1 = combat1
            .cards
            .iter()
            .find(|c| c.card_id == CardId::MadScience)
            .expect("MadScience cloned into combat");
        assert_eq!(in_combat1.combat_var(VarKey::Generic), expected);

        // Boundary: leave combat.
        exit_combat(&mut rs, &combat1);

        // The run-side deck card still carries the morph...
        let deck_card = rs
            .master_deck
            .iter()
            .find(|c| c.card_id == CardId::MadScience)
            .expect("MadScience still in the deck");
        assert_eq!(
            deck_card.combat_var(VarKey::Generic),
            expected,
            "morph survives exit_combat",
        );

        // (2) ...and a SECOND combat still sees the moded card.
        let combat2 = enter_combat(&rs, select_first(act1_normal_pool()));
        let in_combat2 = combat2
            .cards
            .iter()
            .find(|c| c.card_id == CardId::MadScience)
            .expect("MadScience cloned into the second combat");
        assert_eq!(
            in_combat2.combat_var(VarKey::Generic),
            expected,
            "morph survives the combat boundary",
        );
    }

    #[test]
    fn enter_combat_mirrors_neow_relics_as_inert_markers() {
        // 2026-07-17 soft-drop census wave 3: the last W9 Neow reward
        // deferrals (PhialHolster / NeowsBones / HeftyTablet) gained
        // combat-inert marker kinds (following WingedBoots /
        // NeowsTalisman from wave 2), so `RelicId::to_relic_kind` is
        // now total and EVERY run-side relic materializes a combat row.
        // The markers have zero combat listeners — the row is a no-op —
        // but its presence keeps the state-dict registry, the pycli
        // adapter, and run-bridged combat consistent.
        let mut rs = fresh_run();
        rs.relics.push(sts2_core::RelicId::BurningBlood);
        rs.relics.push(sts2_core::RelicId::PhialHolster); // inert marker (wave 3)
        rs.relics.push(sts2_core::RelicId::WingedBoots); // inert marker (wave 2)
        rs.relics.push(sts2_core::RelicId::Anchor);
        let state = enter_combat(&rs, select_first(act1_normal_pool()));
        let kinds: Vec<_> = state.relics.iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            vec![
                sts2_core::RelicKind::BurningBlood,
                sts2_core::RelicKind::PhialHolster,
                sts2_core::RelicKind::WingedBoots,
                sts2_core::RelicKind::Anchor,
            ],
            "every relic mirrors into combat; inert markers are no-op rows"
        );
    }

    #[test]
    fn enter_combat_seeds_counter_from_initial_counter_when_uncounted() {
        // A countdown relic the run has never recorded a counter for
        // seeds from `RelicKind::initial_counter()` (EmberTea = 5), not 0.
        let mut rs = fresh_run();
        rs.relics.push(sts2_core::RelicId::EmberTea);
        let state = enter_combat(&rs, select_first(act1_normal_pool()));
        let ember = state
            .relics
            .iter()
            .find(|r| r.kind == sts2_core::RelicKind::EmberTea)
            .expect("EmberTea mirrored");
        assert_eq!(ember.counter, 5, "seeds from initial_counter, not 0");
    }

    #[test]
    fn enter_combat_prefers_recorded_counter_over_initial() {
        // Once a relic has been through a combat, the run-side
        // `relic_counters` value (e.g. EmberTea decremented to 3) wins
        // over the fresh `initial_counter`.
        let mut rs = fresh_run();
        rs.relics.push(sts2_core::RelicId::EmberTea);
        rs.relic_counters.insert(sts2_core::RelicId::EmberTea, 3);
        let state = enter_combat(&rs, select_first(act1_normal_pool()));
        let ember = state
            .relics
            .iter()
            .find(|r| r.kind == sts2_core::RelicKind::EmberTea)
            .expect("EmberTea mirrored");
        assert_eq!(ember.counter, 3, "recorded counter wins");
    }

    /// Relic-parity gate (bead `sts2-rs-pfiwm`): a relic-bearing run,
    /// entered into combat through the new mirror, reproduces the C#
    /// decompilation's combat-start grants for a sampled handful of
    /// relics. This is the end-to-end proof the bridge actually makes
    /// relics *fire* in run-bridged combat — not merely that rows
    /// exist — with magnitudes pinned to the authoritative C# values
    /// (`../sts2-wiki/decompiled/v0.103.2/.../Relics/<Relic>.cs`), not
    /// the Python pin. The full live C# console-harness A/B
    /// (`feedback_csharp_console_harness_beats_python_pin`) lives in
    /// the sister `sts2-pycli` repo; this in-repo gate pins the same
    /// source-of-truth values without the cross-repo .NET round-trip.
    ///
    /// Each relic's `before_combat_start` listener fires from
    /// `enter_combat`'s `start_combat` call; the assertion reads the
    /// post-`enter_combat` (turn-1 PlayerPlayPhase) observable.
    #[test]
    fn relic_parity_gate_combat_start_grants_match_csharp() {
        use sts2_core::PowerKind;

        // Install the process-global relic dispatch registries so the
        // per-relic `before_combat_start` listeners actually fire.
        crate::relics::dispatch::ensure_installed();
        sts2_core::relic_effects::ensure_installed();

        // Power grants (Owner = player). C# `PowerCmd.Apply<…>` at
        // `BeforeCombatStart` with the relic's `CanonicalVars` amount.
        let power_cases = [
            // Vajra.cs:16,25 — PowerVar<StrengthPower>(1m).
            (sts2_core::RelicId::Vajra, PowerKind::Strength, 1),
            // BronzeScales.cs:16,25 — PowerVar<ThornsPower>(3m).
            (sts2_core::RelicId::BronzeScales, PowerKind::Thorns, 3),
            // OddlySmoothStone.cs:16,25 — PowerVar<DexterityPower>(1m).
            (
                sts2_core::RelicId::OddlySmoothStone,
                PowerKind::Dexterity,
                1,
            ),
        ];
        for (relic, power, expected) in power_cases {
            let mut rs = fresh_run();
            rs.relics.push(relic);
            let state = enter_combat(&rs, select_first(act1_normal_pool()));
            assert_eq!(
                state.player_power_amount(power),
                expected,
                "{relic:?}: expected {expected} {power:?} at combat start (C# value)"
            );
            // Control: the same combat without the relic grants nothing.
            let bare = enter_combat(&fresh_run(), select_first(act1_normal_pool()));
            assert_eq!(
                bare.player_power_amount(power),
                0,
                "{power:?} only present because {relic:?} fired"
            );
        }

        // Anchor.cs:15,22 — GainBlock(10, Unpowered) at BeforeCombatStart.
        // The block must survive into turn 1 (Anchor's entire purpose).
        let mut rs = fresh_run();
        rs.relics.push(sts2_core::RelicId::Anchor);
        let state = enter_combat(&rs, select_first(act1_normal_pool()));
        assert_eq!(
            state.player.block, 10,
            "Anchor grants 10 block at combat start (C# value)"
        );
        let bare = enter_combat(&fresh_run(), select_first(act1_normal_pool()));
        assert_eq!(
            bare.player.block, 0,
            "block only present because Anchor fired"
        );
    }

    #[test]
    fn exit_combat_flushes_relic_counters_back_to_run() {
        // The combat-side counter (e.g. EmberTea decremented this
        // fight) is written back keyed by RelicId for the next room.
        let mut rs = fresh_run();
        rs.relics.push(sts2_core::RelicId::EmberTea);
        let mut state = enter_combat(&rs, select_first(act1_normal_pool()));
        // Simulate the relic's per-combat decrement.
        for r in &mut state.relics {
            if r.kind == sts2_core::RelicKind::EmberTea {
                r.counter = 4;
            }
        }
        exit_combat(&mut rs, &state);
        assert_eq!(
            rs.relic_counters.get(&sts2_core::RelicId::EmberTea),
            Some(&4),
            "decremented counter flushed back to run"
        );
    }

    #[test]
    fn enter_combat_seeds_potion_belt_from_run_state() {
        // Bead `sts2-rs-01o`: potions held at the run level are seeded
        // into the combat-side belt so they're drinkable this fight.
        let mut rs = fresh_run();
        rs.potions.push(sts2_core::potions::PotionKind::FirePotion);
        rs.potions
            .push(sts2_core::potions::PotionKind::EnergyPotion);
        let state = enter_combat(&rs, select_first(act1_normal_pool()));
        assert_eq!(
            state.player.potions.as_slice(),
            &[
                sts2_core::potions::PotionKind::FirePotion,
                sts2_core::potions::PotionKind::EnergyPotion,
            ]
        );
    }

    #[test]
    fn exit_combat_writes_potion_belt_back_to_run_state() {
        // Bead `sts2-rs-01o`: potions drunk in combat are gone; kept
        // ones persist to the next room.
        let mut rs = fresh_run();
        rs.potions.push(sts2_core::potions::PotionKind::FirePotion);
        rs.potions
            .push(sts2_core::potions::PotionKind::EnergyPotion);
        let mut state = enter_combat(&rs, select_first(act1_normal_pool()));
        // Simulate drinking the Fire Potion (remove first slot).
        state.player.potions.remove(0);

        exit_combat(&mut rs, &state);
        assert_eq!(
            rs.potions,
            vec![sts2_core::potions::PotionKind::EnergyPotion],
            "drunk potion removed; remaining potion persists"
        );
    }

    #[test]
    fn belt_slot_hole_from_combat_drink_steers_next_reward_claim() {
        // C# `Player._potionSlots` keeps a null HOLE where a potion was
        // drunk; the next add fills the FIRST hole and the visible belt
        // lists non-empty slots in slot order. So: hold [A@0, B@1], drink
        // A in combat, claim drop C after → belt reads [C, B], not [B, C]
        // (shadow census v8 seeds 1012/1013/1028/1035/1044/1051 — every
        // pair reversed at the post-combat reward).
        use sts2_core::potions::PotionKind;
        let mut rs = fresh_run();
        assert!(rs.add_potion_to_belt(PotionKind::FirePotion));
        assert!(rs.add_potion_to_belt(PotionKind::EnergyPotion));
        let mut state = enter_combat(&rs, select_first(act1_normal_pool()));
        // Drink the Fire Potion (visible index 0) via the slot-aware
        // helper the use_potion path calls.
        assert_eq!(state.player.remove_potion_at(0), PotionKind::FirePotion);
        exit_combat(&mut rs, &state);
        assert_eq!(rs.potions, vec![PotionKind::EnergyPotion]);
        // The post-combat drop fills the emptied slot 0 → lists FIRST.
        assert!(rs.claim_potion_reward(PotionKind::BlockPotion));
        assert_eq!(
            rs.potions,
            vec![PotionKind::BlockPotion, PotionKind::EnergyPotion],
            "reward potion fills the drunk slot 0 and lists before the held potion"
        );
    }

    #[test]
    fn enter_combat_propagates_act_index() {
        let mut rs = fresh_run();
        rs.act_index = 3;
        let state = enter_combat(&rs, select_first(act1_normal_pool()));
        assert_eq!(state.act_index, 3);
    }

    #[test]
    fn exit_combat_writes_back_current_hp() {
        let mut rs = fresh_run();
        let mut state = enter_combat(&rs, select_first(act1_normal_pool()));
        // Simulate damage during combat.
        state.player.current_hp = 41;

        exit_combat(&mut rs, &state);
        assert_eq!(rs.current_hp, 41);
    }

    #[test]
    fn exit_combat_writes_back_max_hp_gain() {
        // Combat-side max-HP gains (Feed kill, FruitJuice, …) must persist to the
        // run. enter_combat carries max_hp IN; exit_combat must carry it back OUT,
        // and clamp current_hp to the FRESH max — not the stale pre-combat value.
        let mut rs = fresh_run();
        let before = rs.max_hp;
        let mut state = enter_combat(&rs, select_first(act1_normal_pool()));
        // Simulate an in-combat +3 max-HP gain that also raises current_hp.
        state.player.max_hp = before + 3;
        state.player.current_hp = before + 3;

        exit_combat(&mut rs, &state);
        assert_eq!(rs.max_hp, before + 3, "in-combat +3 max_hp persists to run");
        assert_eq!(
            rs.current_hp,
            before + 3,
            "current_hp clamps to the fresh (raised) max, not the stale one"
        );
    }

    #[test]
    fn exit_combat_clamps_current_hp_to_max_hp() {
        let mut rs = fresh_run();
        let mut state = enter_combat(&rs, select_first(act1_normal_pool()));
        // Hypothetical over-heal beyond max_hp — bridge clamps.
        state.player.current_hp = 999;
        exit_combat(&mut rs, &state);
        assert_eq!(rs.current_hp, rs.max_hp);
    }

    // ─── sts2-rs-o62e7: LavaLamp firing-site shim ─────────────────────

    #[test]
    fn enter_combat_seeds_lava_lamp_mirror_when_run_carries_it() {
        let mut rs = fresh_run();
        rs.relics.push(sts2_core::RelicId::LavaLamp);
        let state = enter_combat(&rs, select_first(act1_normal_pool()));
        let lava_lamp = state
            .relics
            .iter()
            .find(|r| r.kind == sts2_core::RelicKind::LavaLamp)
            .expect("LavaLamp mirror row pushed by enter_combat");
        assert_eq!(lava_lamp.counter, 0, "fresh combat starts clean");
    }

    #[test]
    fn enter_combat_skips_lava_lamp_mirror_when_run_lacks_it() {
        let rs = fresh_run();
        let state = enter_combat(&rs, select_first(act1_normal_pool()));
        assert!(
            !state
                .relics
                .iter()
                .any(|r| r.kind == sts2_core::RelicKind::LavaLamp),
            "no LavaLamp ownership → no mirror row"
        );
    }

    #[test]
    fn exit_combat_flushes_lava_lamp_flag_back_to_run_state() {
        crate::relics::dispatch::ensure_installed();
        sts2_core::relic_effects::ensure_installed();
        let mut rs = fresh_run();
        rs.relics.push(sts2_core::RelicId::LavaLamp);
        let mut state = enter_combat(&rs, select_first(act1_normal_pool()));
        // Simulate the combat-side listener firing on a damage event:
        // the dispatcher routes through `on_after_damage_received_lava_lamp`,
        // which latches `Relic.counter = 1`.
        let ctx = sts2_core::hooks::DamageCtx {
            // LavaLamp gates on `target == Player`; dealer is
            // unused so any variant is fine. Mirror the "unknown
            // dealer" shape from the Phase-2 fixture: `Player`.
            dealer: sts2_core::hooks::CreatureRef::Player,
            target: sts2_core::hooks::CreatureRef::Player,
            props: sts2_core::hooks::ValueProps::NONE,
        };
        let _ = sts2_core::relic_effects::dispatch_after_damage_received(&mut state, &ctx, 5);
        exit_combat(&mut rs, &state);
        assert_eq!(
            rs.relic_counters[&sts2_core::RelicId::LavaLamp],
            1,
            "damage taken → counter mirrored to run state"
        );
    }

    #[test]
    fn exit_combat_writes_zero_for_clean_combat() {
        let mut rs = fresh_run();
        rs.relics.push(sts2_core::RelicId::LavaLamp);
        let state = enter_combat(&rs, select_first(act1_normal_pool()));
        // No damage dispatched — counter stays at 0.
        exit_combat(&mut rs, &state);
        assert_eq!(
            rs.relic_counters[&sts2_core::RelicId::LavaLamp],
            0,
            "clean combat → counter is 0"
        );
    }

    /// **Full chain (real damage)** — a real combat damage event routed
    /// through `apply_damage_to_player` auto-fires the registered
    /// `after_damage_received` listener (no manual dispatch), latching the
    /// combat-side flag; `exit_combat` mirrors it to the run state; and the
    /// run-side `modify_card_reward_options_late` listener then leaves the
    /// reward cards un-upgraded. Locks in the end-to-end o62e7 wiring.
    #[test]
    fn lava_lamp_full_chain_real_damage_blocks_reward_upgrade() {
        use sts2_core::cards::CardId;
        crate::relics::dispatch::ensure_installed();
        sts2_core::relic_effects::ensure_installed();
        let mut rs = fresh_run();
        rs.relics.push(sts2_core::RelicId::LavaLamp);
        let mut state = enter_combat(&rs, select_first(act1_normal_pool()));
        // Real (non-unblockable) damage through the combat pipeline — the
        // pipeline calls dispatch_after_damage_received itself.
        let _ = sts2_core::combat_loop::apply_damage_to_player(
            &mut state,
            sts2_core::hooks::CreatureRef::Player,
            5,
            sts2_core::hooks::ValueProps::MOVE,
            None,
        );
        let counter = state
            .relics
            .iter()
            .find(|r| r.kind == sts2_core::RelicKind::LavaLamp)
            .expect("mirror row present")
            .counter;
        assert_eq!(counter, 1, "real damage auto-latches the LavaLamp flag");
        exit_combat(&mut rs, &state);
        assert_eq!(
            rs.relic_counters[&sts2_core::RelicId::LavaLamp],
            1,
            "flag flushed to run state"
        );
        // Reward path: took damage → cards NOT upgraded.
        let mut cards =
            vec![
                crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false)
                    .expect("strike pod"),
            ];
        let _ =
            sts2_core::run_effects::dispatch_modify_card_reward_options_late(&mut rs, &mut cards);
        assert!(
            !cards[0].upgraded,
            "took damage → reward cards stay un-upgraded"
        );
    }

    /// **Full chain (clean combat)** — no damage taken leaves the flag at
    /// 0 through `exit_combat`, and the reward listener upgrades the
    /// upgradable reward cards.
    #[test]
    fn lava_lamp_full_chain_clean_combat_upgrades_reward() {
        use sts2_core::cards::CardId;
        crate::relics::dispatch::ensure_installed();
        sts2_core::relic_effects::ensure_installed();
        let mut rs = fresh_run();
        rs.relics.push(sts2_core::RelicId::LavaLamp);
        let state = enter_combat(&rs, select_first(act1_normal_pool()));
        // No damage dispatched.
        exit_combat(&mut rs, &state);
        assert_eq!(rs.relic_counters[&sts2_core::RelicId::LavaLamp], 0);
        let mut cards =
            vec![
                crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false)
                    .expect("strike pod"),
            ];
        let _ =
            sts2_core::run_effects::dispatch_modify_card_reward_options_late(&mut rs, &mut cards);
        assert!(
            cards[0].upgraded,
            "no damage → reward cards upgraded by LavaLamp"
        );
    }

    #[test]
    fn after_damage_received_lava_lamp_gates_on_unblockable() {
        crate::relics::dispatch::ensure_installed();
        sts2_core::relic_effects::ensure_installed();
        let mut rs = fresh_run();
        rs.relics.push(sts2_core::RelicId::LavaLamp);
        let mut state = enter_combat(&rs, select_first(act1_normal_pool()));
        let player_uid = state
            .player
            .creature_uid
            .expect("player-Creature initialized");
        // UNBLOCKABLE damage must NOT trip the flag (Python parity).
        let ctx = sts2_core::hooks::DamageCtx {
            dealer: sts2_core::hooks::CreatureRef::Enemy(player_uid),
            target: sts2_core::hooks::CreatureRef::Player,
            props: sts2_core::hooks::ValueProps::UNBLOCKABLE,
        };
        let _ = sts2_core::relic_effects::dispatch_after_damage_received(&mut state, &ctx, 5);
        let lava_lamp = state
            .relics
            .iter()
            .find(|r| r.kind == sts2_core::RelicKind::LavaLamp)
            .expect("mirror row present");
        assert_eq!(
            lava_lamp.counter, 0,
            "UNBLOCKABLE damage must not latch the flag"
        );
    }

    #[test]
    fn reset_lava_lamp_flag_clears_run_state_slot_when_owned() {
        let mut rs = fresh_run();
        rs.relics.push(sts2_core::RelicId::LavaLamp);
        rs.relic_counters.insert(sts2_core::RelicId::LavaLamp, 1);
        reset_lava_lamp_flag(&mut rs);
        assert_eq!(rs.relic_counters[&sts2_core::RelicId::LavaLamp], 0);
    }

    #[test]
    fn reset_lava_lamp_flag_is_noop_when_unowned() {
        let mut rs = fresh_run();
        // No LavaLamp in rs.relics.
        reset_lava_lamp_flag(&mut rs);
        assert!(!rs
            .relic_counters
            .contains_key(&sts2_core::RelicId::LavaLamp));
    }

    /// End-to-end: combat damage on LavaLamp owner → exit_combat
    /// flushes the flag → run-side modify_card_reward_options_late
    /// listener SUPPRESSES upgrades (damage taken). Then a clean
    /// combat → upgrades fire. Exercises both o62e7 (firing site) +
    /// 67co3 (consumer) in one flow.
    #[test]
    fn lava_lamp_end_to_end_dirty_then_clean_combat() {
        crate::relics::dispatch::ensure_installed();
        sts2_core::relic_effects::ensure_installed();

        let mut rs = fresh_run();
        rs.relics.push(sts2_core::RelicId::LavaLamp);

        // === Combat 1: dirty (player takes damage) ===
        reset_lava_lamp_flag(&mut rs);
        let mut state = enter_combat(&rs, select_first(act1_normal_pool()));
        let ctx = sts2_core::hooks::DamageCtx {
            dealer: sts2_core::hooks::CreatureRef::Player,
            target: sts2_core::hooks::CreatureRef::Player,
            props: sts2_core::hooks::ValueProps::NONE,
        };
        let _ = sts2_core::relic_effects::dispatch_after_damage_received(&mut state, &ctx, 3);
        exit_combat(&mut rs, &state);
        let mut cards = vec![sts2_core::cards::Card {
            card_id: sts2_core::cards::CardId::StrikeIronclad,
            upgraded: false,
            ..sts2_core::cards::Card::pod_defaults()
        }];
        let _ =
            sts2_core::run_effects::dispatch_modify_card_reward_options_late(&mut rs, &mut cards);
        assert!(
            !cards[0].upgraded,
            "dirty combat → card reward NOT upgraded"
        );

        // === Combat 2: clean (no damage) ===
        reset_lava_lamp_flag(&mut rs);
        let state = enter_combat(&rs, select_first(act1_normal_pool()));
        // No damage dispatched.
        exit_combat(&mut rs, &state);
        let mut cards = vec![sts2_core::cards::Card {
            card_id: sts2_core::cards::CardId::StrikeIronclad,
            upgraded: false,
            ..sts2_core::cards::Card::pod_defaults()
        }];
        let _ =
            sts2_core::run_effects::dispatch_modify_card_reward_options_late(&mut rs, &mut cards);
        assert!(cards[0].upgraded, "clean combat → card reward upgraded");
    }

    /// Bead `sts2-rs-RP114`: The Scythe's `IncreasedDamage` is run-persistent
    /// (C# `[SavedProperty]` + `DeckVersion.BuffFromPlay`). Playing it in
    /// combat grows its `base_damage`; `exit_combat` must fold that growth back
    /// onto the master-deck card so a subsequent `enter_combat` starts boosted.
    #[test]
    fn the_scythe_growth_persists_across_combat_round_trip() {
        use crate::cards::necrobinder::the_scythe::play_the_scythe;
        use sts2_core::cards::CardId;

        crate::cards::dispatch::ensure_installed();

        let mut rs = fresh_run();
        rs.master_deck.push(
            crate::cards::spec_index::card_pod_from_id(CardId::TheScythe, false)
                .expect("scythe spec"),
        );
        // Sanity: master starts at the spec base.
        assert_eq!(rs.master_deck[0].base_damage, 13);

        // === Combat 1 ===
        let mut state = enter_combat(&rs, select_first(act1_normal_pool()));
        // The combat copy starts at the master's base.
        let scythe_idx = state
            .cards
            .iter()
            .position(|c| c.card_id == CardId::TheScythe)
            .expect("scythe cloned into combat");
        assert_eq!(state.cards[scythe_idx].base_damage, 13);

        // Drive two plays through the real handler.
        let enemy = state.enemy_uids[0];
        let uid = state.cards[scythe_idx].uid;
        state.player.play.push(uid);
        play_the_scythe(&mut state, Some(enemy));
        state.player.play.pop();
        state.player.play.push(uid);
        play_the_scythe(&mut state, Some(enemy));
        state.player.play.pop();
        assert_eq!(
            state.cards[scythe_idx].base_damage, 21,
            "two plays: 13 → 17 → 21 (v0.107.1 Increase 4)"
        );

        exit_combat(&mut rs, &state);
        assert_eq!(
            rs.master_deck[0].base_damage, 21,
            "growth folded back onto the master-deck Scythe"
        );

        // === Combat 2 starts boosted ===
        let state2 = enter_combat(&rs, select_first(act1_normal_pool()));
        let scythe2 = state2
            .cards
            .iter()
            .find(|c| c.card_id == CardId::TheScythe)
            .expect("scythe cloned into combat 2");
        assert_eq!(
            scythe2.base_damage, 21,
            "next combat's Scythe copy starts from the accumulated value"
        );
    }

    /// Bead `sts2-rs-nfoh0`: ForbiddenGrimoirePower.AfterCombatEnd. A player
    /// carrying ForbiddenGrimoire(N) at combat end stages N card-removal picks
    /// on the run-state reward queue via the `exit_combat` seam.
    #[test]
    fn exit_combat_stages_forbidden_grimoire_card_removals() {
        let mut rs = fresh_run();
        let mut combat = enter_combat(&rs, select_first(act1_normal_pool()));
        let player_uid = combat
            .player
            .creature_uid
            .expect("player-Creature initialized");
        sts2_core::card_effects::apply_power_to_creature(
            &mut combat,
            player_uid,
            sts2_core::PowerKind::ForbiddenGrimoire,
            2,
        );

        exit_combat(&mut rs, &combat);

        assert_eq!(
            rs.pending_rewards,
            vec![sts2_core::runstate::Reward::RemoveCard { count: 2 }],
            "ForbiddenGrimoire(2) stages 2 card-removal picks at combat end"
        );
    }

    /// No ForbiddenGrimoire → no card-removal reward staged.
    #[test]
    fn exit_combat_stages_no_removal_without_grimoire() {
        let mut rs = fresh_run();
        let combat = enter_combat(&rs, select_first(act1_normal_pool()));
        exit_combat(&mut rs, &combat);
        assert!(
            !rs.pending_rewards
                .iter()
                .any(|r| matches!(r, sts2_core::runstate::Reward::RemoveCard { .. })),
            "no Grimoire → no card-removal reward"
        );
    }

    /// A won fight's exit with the player holding `powers` (applied in that
    /// order) and the fight's own extras `added` (sts2-agent fork).
    fn exit_holding(
        powers: &[(sts2_core::PowerKind, i32)],
        added: &[sts2_core::CombatExtraReward],
        screen: bool,
    ) -> RunState {
        let mut rs = fresh_run();
        rs.interactive_rewards = screen;
        let mut combat = enter_combat(&rs, select_first(act1_normal_pool()));
        combat.player_won = true;
        combat.room_extra_rewards = added.to_vec();
        combat.heist_recovered_gold = added
            .iter()
            .map(|r| match r {
                sts2_core::CombatExtraReward::Gold(g) => *g,
                _ => 0,
            })
            .sum();
        let player_uid = combat
            .player
            .creature_uid
            .expect("player-Creature initialized");
        for &(kind, amount) in powers {
            sts2_core::card_effects::apply_power_to_creature(&mut combat, player_uid, kind, amount);
        }
        exit_combat(&mut rs, &combat);
        rs
    }

    /// sts2-agent fork: under `interactive_rewards` a won room's extra rewards
    /// (C# `CombatRoom.AddExtraReward`) wait on its rewards screen, one per
    /// call, in the game's order: what the fight added (two Heist carriers'
    /// gold kept apart, The Hunt's card reward), then the combat-end ones in the
    /// player's power order (Forbidden Grimoire's removal, Royalties' gold).
    #[test]
    fn exit_combat_stages_a_rooms_extra_rewards_in_order() {
        use sts2_core::{CombatExtraReward as C, OfferedReward as O, PowerKind};
        let gold_before = fresh_run().gold;
        let added = [C::Gold(10), C::RoomCard, C::Gold(20)];
        let rs = exit_holding(
            &[
                (PowerKind::ForbiddenGrimoire, 1),
                (PowerKind::Royalties, 30),
            ],
            &added,
            true,
        );
        assert_eq!(rs.gold, gold_before, "no gold settled at the exit");
        assert!(
            rs.pending_rewards.is_empty(),
            "no removal queued at the exit"
        );
        assert_eq!(
            rs.room_extra_rewards,
            vec![
                O::Gold(10),
                O::RoomCard,
                O::Gold(20),
                O::RemoveCard,
                O::Gold(30)
            ]
        );
        let rs = exit_holding(
            &[
                (PowerKind::Royalties, 30),
                (PowerKind::ForbiddenGrimoire, 1),
            ],
            &[],
            true,
        );
        assert_eq!(rs.room_extra_rewards, vec![O::Gold(30), O::RemoveCard]);
        // Without the screen they settle at once, as before.
        let auto = exit_holding(&[(PowerKind::Royalties, 30)], &[C::Gold(25)], false);
        assert_eq!(auto.gold, gold_before + 55);
        assert!(auto.room_extra_rewards.is_empty());
    }

    /// sts2-agent fork: a killed thief's card (C# `SwipePower.BeforeDeath`)
    /// leaves the deck and comes back as a special card reward to claim; the
    /// fight records it when the thief dies.
    #[test]
    fn exit_combat_offers_a_killed_thiefs_card_as_a_special_card_reward() {
        crate::cards::dispatch::ensure_installed();
        let mut rs = fresh_run();
        rs.interactive_rewards = true;
        let stolen =
            crate::cards::spec_index::card_pod_from_id(sts2_core::cards::CardId::Bash, false)
                .expect("spec");
        rs.master_deck.push(stolen);
        let mut combat = enter_combat(&rs, select_first(act1_normal_pool()));
        combat.player_won = true;
        let thief = combat.enemy_uids[0];
        combat.record_swipe_steal(thief, stolen.card_id, stolen.upgraded);
        combat.reclaim_swipe_on_death(thief);
        assert_eq!(
            combat.room_extra_rewards,
            vec![sts2_core::CombatExtraReward::SpecialCard {
                card_id: stolen.card_id,
                upgraded: stolen.upgraded,
                deck_index: None,
            }]
        );
        let deck = rs.master_deck.len();
        exit_combat(&mut rs, &combat);
        assert_eq!(
            rs.master_deck.len(),
            deck - 1,
            "out of the deck until claimed"
        );
        assert_eq!(
            rs.room_extra_rewards,
            vec![sts2_core::OfferedReward::SpecialCard(stolen)]
        );
    }

    /// sts2-agent fork: a card a killed thief returns carries the persistent
    /// changes the fight made to its deck version (a played Scythe's +4), as an
    /// untaken one does.
    #[test]
    fn a_returned_card_keeps_its_scaled_damage() {
        use crate::cards::necrobinder::the_scythe::play_the_scythe;
        use sts2_core::cards::CardId;
        crate::cards::dispatch::ensure_installed();
        let played_scythe = |stolen: bool| {
            let mut rs = fresh_run();
            rs.interactive_rewards = true;
            rs.master_deck.push(
                crate::cards::spec_index::card_pod_from_id(CardId::TheScythe, false)
                    .expect("scythe spec"),
            );
            let mut state = enter_combat(&rs, select_first(act1_normal_pool()));
            state.player_won = true;
            let idx = state
                .cards
                .iter()
                .position(|c| c.card_id == CardId::TheScythe)
                .expect("scythe in combat");
            let enemy = state.enemy_uids[0];
            let uid = state.cards[idx].uid;
            state.player.play.push(uid);
            play_the_scythe(&mut state, Some(enemy));
            state.player.play.pop();
            if stolen {
                state.record_swipe_steal(enemy, CardId::TheScythe, false);
                state.reclaim_swipe_on_death(enemy);
            }
            exit_combat(&mut rs, &state);
            rs
        };
        let kept = played_scythe(false);
        let kept_damage = kept
            .master_deck
            .iter()
            .find(|c| c.card_id == CardId::TheScythe)
            .expect("in the deck")
            .base_damage;
        assert_eq!(kept_damage, 17);
        let returned = played_scythe(true);
        assert!(returned
            .master_deck
            .iter()
            .all(|c| c.card_id != CardId::TheScythe));
        let [sts2_core::OfferedReward::SpecialCard(card)] = returned.room_extra_rewards.as_slice()
        else {
            panic!("one special card: {:?}", returned.room_extra_rewards);
        };
        assert_eq!(card.base_damage, kept_damage);
    }

    /// `RoyaltiesPower.AfterCombatEnd` grants `Amount` gold on a WON combat
    /// (Regent's `RoyaltiesCard` stacks 30 → +30 gold). C# `RoyaltiesPower.cs`.
    #[test]
    fn exit_combat_grants_royalties_gold_on_victory() {
        let mut rs = fresh_run();
        let gold_before = rs.gold;
        let mut combat = enter_combat(&rs, select_first(act1_normal_pool()));
        combat.player_won = true;
        let player_uid = combat
            .player
            .creature_uid
            .expect("player-Creature initialized");
        sts2_core::card_effects::apply_power_to_creature(
            &mut combat,
            player_uid,
            sts2_core::PowerKind::Royalties,
            30,
        );

        exit_combat(&mut rs, &combat);

        assert_eq!(
            rs.gold,
            gold_before + 30,
            "Royalties(30) on victory grants 30 gold"
        );
    }

    /// Royalties gold is NOT granted on a LOST combat (no rewards screen) —
    /// mirrors C# (the extra `GoldReward` is never collected) and the
    /// `royalties_gold_on_victory` helper's `_on_victory` contract.
    #[test]
    fn exit_combat_no_royalties_gold_on_loss() {
        let mut rs = fresh_run();
        let gold_before = rs.gold;
        let mut combat = enter_combat(&rs, select_first(act1_normal_pool()));
        combat.player_won = false;
        let player_uid = combat
            .player
            .creature_uid
            .expect("player-Creature initialized");
        sts2_core::card_effects::apply_power_to_creature(
            &mut combat,
            player_uid,
            sts2_core::PowerKind::Royalties,
            30,
        );

        exit_combat(&mut rs, &combat);

        assert_eq!(rs.gold, gold_before, "no Royalties gold on a lost combat");
    }

    /// Bead `sts2-rs-RP13`: a SwipePower thief that escapes/survives keeps the
    /// stolen card — `exit_combat` removes it from the run `master_deck`
    /// (mirroring C# `SwipePower.Steal` → `RemoveFromDeck`, never restored).
    #[test]
    fn exit_combat_removes_escaped_swiped_card_from_master_deck() {
        use sts2_core::cards::CardId;
        let mut rs = fresh_run();
        for _ in 0..2 {
            rs.master_deck.push(
                crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false)
                    .expect("strike spec"),
            );
        }
        let mut combat = enter_combat(&rs, select_first(act1_normal_pool()));
        let before = rs.master_deck.len();
        let victim = rs.master_deck[0].card_id;
        let victim_up = rs.master_deck[0].upgraded;
        let same = |c: &sts2_core::cards::Card| c.card_id == victim && c.upgraded == victim_up;
        let count_before = rs.master_deck.iter().filter(|c| same(c)).count();
        let thief = *combat.enemy_uids.first().expect("combat has an enemy");

        combat.record_swipe_steal(thief, victim, victim_up);
        exit_combat(&mut rs, &combat);

        assert_eq!(
            rs.master_deck.len(),
            before - 1,
            "an escaped thief's stolen card is removed from the run deck"
        );
        assert_eq!(
            rs.master_deck.iter().filter(|c| same(c)).count(),
            count_before - 1,
            "exactly one matching (card_id, upgraded) card is removed"
        );
    }

    /// Bead `sts2-rs-RP13`: a SwipePower thief that DIES restores the stolen
    /// card (C# `SwipePower.BeforeDeath` → `AddCard`); the reclaimed entry is
    /// NOT removed from the run `master_deck` at `exit_combat`.
    #[test]
    fn exit_combat_sets_gold_proportion_from_escaped_creatures() {
        // C# `CombatRoom.GoldProportion = 1 - escaped / total` (`CombatRoom.cs:216`).
        // Bead `sts2-rs-RP15`.
        let mut rs = fresh_run();
        let mut combat = enter_combat(&rs, select_first(act1_normal_pool()));
        let total = combat.enemy_uids.len();
        assert!(total >= 1, "fixture combat has at least one enemy");

        // No escapes → full gold.
        combat.escaped_creatures = 0;
        exit_combat(&mut rs, &combat);
        assert!((rs.last_combat_gold_proportion - 1.0).abs() < 1e-9);

        // Every monster fled → 0 proportion (no gold).
        combat.escaped_creatures = total as u32;
        exit_combat(&mut rs, &combat);
        assert!(
            rs.last_combat_gold_proportion.abs() < 1e-9,
            "all fled ⇒ GoldProportion 0, got {}",
            rs.last_combat_gold_proportion
        );
        // And the gold roll yields nothing (no Rewards read) at proportion 0.
        assert_eq!(
            rs.roll_encounter_gold(sts2_core::runstate::RoomType::Monster, 0.0),
            None
        );
    }

    /// GH #604: C# `HeistPower.BeforeDeath` hands the merc's stolen gold back
    /// as a `GoldReward` when the carrier is killed. `exit_combat` settles the
    /// engine-side ledger into run gold — on a WON combat only, since an extra
    /// reward is collected off a rewards screen a lost combat never shows.
    #[test]
    fn exit_combat_returns_heist_gold_on_victory_only() {
        let mut rs = fresh_run();
        let mut combat = enter_combat(&rs, select_first(act1_normal_pool()));
        combat.gold = rs.gold;
        combat.heist_recovered_gold = 42;

        let before = rs.gold;
        combat.player_won = false;
        exit_combat(&mut rs, &combat);
        assert_eq!(
            rs.gold, before,
            "a lost combat never reaches the rewards screen"
        );

        let mut rs = fresh_run();
        let mut combat = enter_combat(&rs, select_first(act1_normal_pool()));
        combat.gold = rs.gold;
        combat.heist_recovered_gold = 42;
        let before = rs.gold;
        combat.player_won = true;
        exit_combat(&mut rs, &combat);
        assert_eq!(
            rs.gold,
            before + 42,
            "killing the carrier returns exactly the gold it was holding"
        );
    }

    #[test]
    fn exit_combat_keeps_reclaimed_swiped_card_in_master_deck() {
        use sts2_core::cards::CardId;
        let mut rs = fresh_run();
        for _ in 0..2 {
            rs.master_deck.push(
                crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false)
                    .expect("strike spec"),
            );
        }
        let mut combat = enter_combat(&rs, select_first(act1_normal_pool()));
        let before = rs.master_deck.len();
        let victim = rs.master_deck[0].card_id;
        let victim_up = rs.master_deck[0].upgraded;
        let thief = *combat.enemy_uids.first().expect("combat has an enemy");

        combat.record_swipe_steal(thief, victim, victim_up);
        // Thief died → SwipePower.BeforeDeath restored the card.
        combat.reclaim_swipe_on_death(thief);
        exit_combat(&mut rs, &combat);

        assert_eq!(
            rs.master_deck.len(),
            before,
            "a killed thief's stolen card stays in the run deck"
        );
    }

    /// Phase-0 combat-redesign gate (`docs/plans/2026-06-09-combat-phase0-foundations-implementation.md`,
    /// Task 1): the `relic_parity_gate_*` test above proves relics fire
    /// at `enter_combat`. This one proves the relic-bearing
    /// `CombatState` carries that difference *into a real MCTS search* —
    /// i.e. the cloned state searched in rollouts reflects the relic's
    /// effect — which is the Phase-0 acceptance for "combat-relic
    /// effects surface in MCTS rollouts." Same encounter + seed; the
    /// only difference is Vajra (+1 Strength at combat start), so the
    /// root value must differ from the bare control.
    #[test]
    #[serial_test::serial(relic_registry)]
    fn vajra_changes_search_root_value_vs_bare() {
        use sts2_core::potions::CharacterId;
        use sts2_mcts::{Mcts, MctsConfig};

        // Process-global channels: the search clones the state and
        // plays cards / fires relic listeners during rollouts, so the
        // card-dispatch and relic registries must be installed before
        // the search runs. All idempotent.
        crate::cards::dispatch::ensure_installed();
        crate::relics::dispatch::ensure_installed();
        sts2_core::relic_effects::ensure_installed();

        let cfg = MctsConfig {
            max_simulations: 2000,
            seed: 7,
            use_transposition_table: false,
            ..MctsConfig::default()
        };

        // Both arms get the real 10-card Ironclad starter deck so the
        // search has meaningful card-play actions (with an empty deck
        // the only legal move is EndTurn and every rollout is a forced
        // loss — relics cannot change a no-offense outcome). The arms
        // differ ONLY by the Vajra relic (+1 Strength at combat start),
        // which raises Strike/Bash damage and must shift the search's
        // root value.
        let mut rs = fresh_run();
        crate::run_callbacks::populate_starter_deck(&mut rs, CharacterId::Ironclad);
        let mut bare_rs = fresh_run();
        crate::run_callbacks::populate_starter_deck(&mut bare_rs, CharacterId::Ironclad);
        rs.relics.push(sts2_core::RelicId::Vajra);

        let with_relic = enter_combat(&rs, select_first(act1_normal_pool()));
        let bare = enter_combat(&bare_rs, select_first(act1_normal_pool()));

        // Sanity: the relic actually applied at combat start, and both
        // arms have a non-empty deck (so the search is non-trivial).
        assert_eq!(
            with_relic.player_power_amount(sts2_core::PowerKind::Strength),
            1
        );
        assert_eq!(bare.player_power_amount(sts2_core::PowerKind::Strength), 0);
        assert!(
            !with_relic.player.draw.is_empty() || !with_relic.player.hand.is_empty(),
            "starter deck must populate the combat draw pile/hand"
        );

        // The search must see the difference: root value with
        // +Strength should differ from bare. `Mcts::new` consumes the
        // state, so the sanity reads above happen first.
        let mut search_relic = Mcts::new(with_relic, cfg);
        search_relic.search();
        let q_relic = search_relic
            .root_q()
            .expect("relic-arm search produced a root value");

        let mut search_bare = Mcts::new(bare, cfg);
        search_bare.search();
        let q_bare = search_bare
            .root_q()
            .expect("bare-arm search produced a root value");

        assert!(
            (q_relic - q_bare).abs() > 1e-4,
            "search root value identical with vs without Vajra ({q_relic} vs {q_bare}) — relics not surfacing in search"
        );
    }

    // ─── cargo-mutants coverage: enter_combat_from_roster field/op kills ──

    /// The run's RNG streams must be carried into the combat verbatim:
    /// `state.run_rng` is a clone of `rs.rng` (same master seed) and the
    /// legacy `state.rng` is `Rng::new(rs.rng.seed)`. With a non-default run
    /// seed, deleting either field from the `CombatState { … }` expression
    /// (which falls back to the `..default()` seed-0 RNGs) is observable.
    /// Bead `sts2-rs-jenwp` (M3b combat-RNG carry-in).
    #[test]
    fn enter_combat_from_roster_carries_run_rng_streams() {
        const SEED: i32 = 0x5EED;
        let rs = RunState {
            rng: sts2_core::run_rng_set::RunRngSet::new(SEED),
            ..fresh_run()
        };
        let roster = [(MonsterKind::FuzzyWurmCrawler, MonsterSpawnArgs::None, 57)];
        let state = enter_combat_from_roster(&rs, &roster, sts2_core::runstate::RoomType::Monster);

        // `run_rng` field present → clone of rs.rng, NOT the default seed-0 set.
        assert_eq!(
            state.run_rng.seed, SEED,
            "run_rng must be the run's RNG bundle (seed carried in), not the default seed-0 set"
        );
        // `rng` field present → `Rng::new(rs.rng.seed)`, NOT the default seed-0.
        assert_eq!(
            state.rng.seed(),
            SEED as u32,
            "legacy combat rng must be seeded from the run's master seed, not 0"
        );
    }

    /// `SpawnPosition.alone = (n == 1)` drives the Nibbit AI's opening move:
    /// a SOLO Nibbit opens on BUTT; a Nibbit in a pair does not. Flipping the
    /// `==` (to `!=`) inverts `alone` and shifts the opening state, so the move
    /// id distinguishes original from mutant.
    #[test]
    fn enter_combat_from_roster_spawn_position_alone_drives_nibbit_opener() {
        let solo = [(MonsterKind::Nibbit, MonsterSpawnArgs::None, 45)];
        let state =
            enter_combat_from_roster(&fresh_run(), &solo, sts2_core::runstate::RoomType::Monster);
        let move_id = nibbit_opening_move(&state, 0);
        assert_eq!(
            move_id, "BUTT",
            "a SOLO Nibbit (alone == true) opens on BUTT"
        );
    }

    /// `SpawnPosition.front = (i == 0)` drives the (non-alone) Nibbit opener:
    /// the FRONT (index 0) Nibbit of a pair opens on SLICE, the back one on
    /// HISS. Flipping the `==` inverts which index counts as front.
    #[test]
    fn enter_combat_from_roster_spawn_position_front_drives_nibbit_opener() {
        let pair = [
            (MonsterKind::Nibbit, MonsterSpawnArgs::None, 45),
            (MonsterKind::Nibbit, MonsterSpawnArgs::None, 45),
        ];
        let state =
            enter_combat_from_roster(&fresh_run(), &pair, sts2_core::runstate::RoomType::Monster);
        // front (alone == false, front == true) → SLICE
        assert_eq!(
            nibbit_opening_move(&state, 0),
            "SLICE",
            "the front Nibbit (i == 0, not alone) opens on SLICE"
        );
        // back (alone == false, front == false) → HISS
        assert_eq!(
            nibbit_opening_move(&state, 1),
            "HISS",
            "a non-front, non-alone Nibbit opens on HISS"
        );
    }

    /// Read the opening move id of the `slot`-th spawned enemy, assuming it's a
    /// Nibbit (whose initial DSL state is chosen by alone/front).
    fn nibbit_opening_move(state: &CombatState, slot: usize) -> &'static str {
        let uid = state.enemy_uids[slot];
        state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .expect("nibbit row")
            .ai
            .as_ref()
            .expect("nibbit AI wired")
            .current_move()
            .id
    }

    /// `monster_id` is set from `python_name(kind)` — the stable Python-pin
    /// name string. Replacing `python_name` with a constant ("xyzzy" / "")
    /// changes the spawned creature's `monster_id`, so pinning it to the real
    /// name kills both constant-return mutants.
    #[test]
    fn enter_combat_from_roster_sets_monster_id_from_python_name() {
        let roster = [(MonsterKind::Nibbit, MonsterSpawnArgs::None, 45)];
        let state = enter_combat_from_roster(
            &fresh_run(),
            &roster,
            sts2_core::runstate::RoomType::Monster,
        );
        let uid = state.enemy_uids[0];
        let enemy = state
            .creatures
            .iter()
            .find(|c| c.id == uid.0)
            .expect("nibbit row");
        assert_eq!(
            enemy.monster_id,
            MonsterId("Nibbit"),
            "monster_id must come from MonsterKind::python_name, not a placeholder"
        );
    }

    /// `exit_combat`'s gold-proportion is `1 - escaped / total`. A multi-monster
    /// roster with a PARTIAL escape (1 of 2 fled) makes the division load-bearing:
    /// `1 - 1/2 = 0.5`, where swapping `/` for `*` would give `1 - 1*2 = -1`. The
    /// existing single-enemy test can't see this (`1/1 == 1*1`). Bead
    /// `sts2-rs-RP15`.
    #[test]
    fn exit_combat_gold_proportion_partial_escape_two_monsters() {
        let mut rs = fresh_run();
        let roster = [
            (MonsterKind::FuzzyWurmCrawler, MonsterSpawnArgs::None, 57),
            (MonsterKind::FuzzyWurmCrawler, MonsterSpawnArgs::None, 57),
        ];
        let mut combat =
            enter_combat_from_roster(&rs, &roster, sts2_core::runstate::RoomType::Monster);
        assert_eq!(combat.enemy_uids.len(), 2, "two-monster roster");

        combat.escaped_creatures = 1;
        exit_combat(&mut rs, &combat);
        assert!(
            (rs.last_combat_gold_proportion - 0.5).abs() < 1e-9,
            "1 of 2 fled ⇒ proportion 0.5, got {}",
            rs.last_combat_gold_proportion
        );
    }

    /// `is_run_persistent_damage_scaler` must return `true` ONLY for The Scythe.
    /// A `-> true` mutant would treat every card as a scaler and propagate any
    /// combat-side `base_damage` growth onto the master deck. Seed a NON-scaler
    /// card (Strike), bump its combat copy's `base_damage`, and assert the
    /// master row is left untouched. Bead `sts2-rs-RP114`.
    #[test]
    fn non_scaler_card_growth_not_propagated_to_master_deck() {
        use sts2_core::cards::CardId;
        let mut rs = fresh_run();
        rs.master_deck.push(
            crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false)
                .expect("strike spec"),
        );
        let master_base = rs.master_deck[0].base_damage;

        let mut state = enter_combat(&rs, select_first(act1_normal_pool()));
        let idx = state
            .cards
            .iter()
            .position(|c| c.card_id == CardId::StrikeIronclad)
            .expect("strike cloned into combat");
        // Simulate combat-side growth that MUST NOT leak to the master deck for
        // a non-scaler card.
        state.cards[idx].base_damage = master_base + 99;

        exit_combat(&mut rs, &state);
        assert_eq!(
            rs.master_deck[0].base_damage, master_base,
            "a non-scaler (Strike) must NOT propagate base_damage growth to the master deck"
        );
    }

    /// `settle_swiped_cards_and_extras` matches the stolen card by
    /// `(card_id && upgraded)`. With two same-id cards differing only by upgrade,
    /// stealing the UPGRADED one must remove the upgraded copy — never the
    /// un-upgraded one. An `&&`→`||` mutant matches on id alone and would strip
    /// the wrong (first-found) copy. Bead `sts2-rs-RP13`.
    #[test]
    fn exit_combat_swipe_removes_exact_upgraded_match_not_just_id() {
        use sts2_core::cards::CardId;
        let mut rs = fresh_run();
        // index 0: un-upgraded Strike, index 1: upgraded Strike.
        rs.master_deck.push(
            crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false)
                .expect("strike spec"),
        );
        rs.master_deck.push(
            crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, true)
                .expect("strike+ spec"),
        );
        let mut combat = enter_combat(&rs, select_first(act1_normal_pool()));
        let thief = *combat.enemy_uids.first().expect("combat has an enemy");

        // Steal the UPGRADED Strike.
        combat.record_swipe_steal(thief, CardId::StrikeIronclad, true);
        exit_combat(&mut rs, &combat);

        assert_eq!(rs.master_deck.len(), 1, "exactly one card removed");
        assert!(
            !rs.master_deck[0].upgraded,
            "the upgraded Strike was stolen → only the un-upgraded copy remains"
        );
    }

    /// `propagate_scaling_cards_to_master_deck` only collects ids for which
    /// `is_run_persistent_damage_scaler(id) && !already_seen`. With `&&`→`||`,
    /// EVERY distinct master-deck id would be collected as a "scaler"; the loop
    /// then pairs a non-scaler's combat copy onto its master row and would leak
    /// its growth. A deck of {Scythe, Strike} where the Strike copy grew lets the
    /// Strike leak under the mutant but stay put under the original.
    #[test]
    fn propagate_only_collects_actual_scaler_ids() {
        use crate::cards::necrobinder::the_scythe::play_the_scythe;
        use sts2_core::cards::CardId;
        crate::cards::dispatch::ensure_installed();

        let mut rs = fresh_run();
        rs.master_deck.push(
            crate::cards::spec_index::card_pod_from_id(CardId::TheScythe, false)
                .expect("scythe spec"),
        );
        rs.master_deck.push(
            crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false)
                .expect("strike spec"),
        );
        let strike_master_base = rs
            .master_deck
            .iter()
            .find(|c| c.card_id == CardId::StrikeIronclad)
            .unwrap()
            .base_damage;

        let mut state = enter_combat(&rs, select_first(act1_normal_pool()));
        // Grow the Strike combat copy (a NON-scaler) — must NOT propagate.
        let strike_idx = state
            .cards
            .iter()
            .position(|c| c.card_id == CardId::StrikeIronclad)
            .expect("strike in combat");
        state.cards[strike_idx].base_damage = strike_master_base + 50;
        // Also actually play the Scythe so the scaler path has real work.
        let scythe_idx = state
            .cards
            .iter()
            .position(|c| c.card_id == CardId::TheScythe)
            .expect("scythe in combat");
        let enemy = state.enemy_uids[0];
        let uid = state.cards[scythe_idx].uid;
        state.player.play.push(uid);
        play_the_scythe(&mut state, Some(enemy));
        state.player.play.pop();

        exit_combat(&mut rs, &state);

        let strike_master_after = rs
            .master_deck
            .iter()
            .find(|c| c.card_id == CardId::StrikeIronclad)
            .unwrap()
            .base_damage;
        assert_eq!(
            strike_master_after, strike_master_base,
            "Strike is not a scaler → its combat growth must not reach the master deck"
        );
        let scythe_master = rs
            .master_deck
            .iter()
            .find(|c| c.card_id == CardId::TheScythe)
            .unwrap()
            .base_damage;
        assert!(
            scythe_master > 13,
            "the Scythe (real scaler) still propagated its growth"
        );
    }

    /// Pairing index advance in `propagate_scaling_cards_to_master_deck`:
    /// two same-bucket (un-upgraded) Scythes are paired to their own combat
    /// copies positionally via `next_down += 1`. Give the two combat copies
    /// DIFFERENT accumulated values; the per-master pairing must advance the
    /// cursor so each master gets its own copy's value (and `max`-guarded so
    /// neither shrinks). An `+=`→`-=`/`*=` mutant stalls/garbles the cursor and
    /// mis-pairs (both masters read combat copy 0). Bead `sts2-rs-RP114`.
    #[test]
    fn propagate_pairs_two_scythes_positionally() {
        use sts2_core::cards::CardId;

        let mut rs = fresh_run();
        // Two un-upgraded Scythes in the master deck.
        for _ in 0..2 {
            rs.master_deck.push(
                crate::cards::spec_index::card_pod_from_id(CardId::TheScythe, false)
                    .expect("scythe spec"),
            );
        }

        let mut state = enter_combat(&rs, select_first(act1_normal_pool()));
        // Combat-arena order is reverse master order; both copies are
        // un-upgraded so they bucket together. Assign distinct grown values.
        let scythe_idxs: Vec<usize> = state
            .cards
            .iter()
            .enumerate()
            .filter(|(_, c)| c.card_id == CardId::TheScythe)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(scythe_idxs.len(), 2, "two Scythe combat copies");
        // Give them clearly different accumulated values, both above base (13).
        state.cards[scythe_idxs[0]].base_damage = 40;
        state.cards[scythe_idxs[1]].base_damage = 25;

        exit_combat(&mut rs, &state);

        // The two master Scythes must receive the two distinct combat values
        // (as a multiset) — proving the pairing cursor advanced. A stalled
        // cursor would give {40, 40}; the original gives {40, 25}.
        let mut master_vals: Vec<i32> = rs
            .master_deck
            .iter()
            .filter(|c| c.card_id == CardId::TheScythe)
            .map(|c| c.base_damage)
            .collect();
        master_vals.sort_unstable();
        assert_eq!(
            master_vals,
            vec![25, 40],
            "each master Scythe paired to its own combat copy's value (cursor advanced)"
        );
    }

    /// Same positional-pairing proof for the UPGRADED bucket (`next_up += 1`,
    /// the other arm of the if/else). Two upgraded Scythes with distinct grown
    /// combat values must each pair to their own copy; a stalled/garbled
    /// `next_up` cursor (`+=`→`-=`/`*=`) collapses both to combat copy 0.
    #[test]
    fn propagate_pairs_two_upgraded_scythes_positionally() {
        use sts2_core::cards::CardId;

        let mut rs = fresh_run();
        for _ in 0..2 {
            rs.master_deck.push(
                crate::cards::spec_index::card_pod_from_id(CardId::TheScythe, true)
                    .expect("scythe+ spec"),
            );
        }

        let mut state = enter_combat(&rs, select_first(act1_normal_pool()));
        let scythe_idxs: Vec<usize> = state
            .cards
            .iter()
            .enumerate()
            .filter(|(_, c)| c.card_id == CardId::TheScythe && c.upgraded)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(scythe_idxs.len(), 2, "two upgraded Scythe combat copies");
        let upgraded_base = state.cards[scythe_idxs[0]].base_damage;
        state.cards[scythe_idxs[0]].base_damage = upgraded_base + 30;
        state.cards[scythe_idxs[1]].base_damage = upgraded_base + 10;

        exit_combat(&mut rs, &state);

        let mut master_vals: Vec<i32> = rs
            .master_deck
            .iter()
            .filter(|c| c.card_id == CardId::TheScythe)
            .map(|c| c.base_damage)
            .collect();
        master_vals.sort_unstable();
        assert_eq!(
            master_vals,
            vec![upgraded_base + 10, upgraded_base + 30],
            "each upgraded master Scythe paired to its own combat copy (next_up advanced)"
        );
    }

    /// FishingRod (v0.107.1): every 3rd *normal* combat upgrades a random
    /// upgradable deck card. The first two combats must NOT upgrade; the
    /// third must. Bead `sts2-rs-v0107-content-port-wb8lk.6`.
    #[test]
    fn fishing_rod_upgrades_a_card_only_on_the_third_normal_combat() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RoomType;

        let roster = [(MonsterKind::FuzzyWurmCrawler, MonsterSpawnArgs::None, 57)];
        let mut rs = fresh_run();
        rs.relics.push(sts2_core::RelicId::FishingRod);
        rs.master_deck =
            vec![
                crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false)
                    .expect("Strike POD"),
            ];
        assert!(
            rs.master_deck[0].is_upgradable(),
            "starting Strike is upgradable"
        );

        // Combats 1 and 2: counter ticks but no upgrade fires. FishingRod's
        // C# hook fires via EndCombatInternal (a WON combat) only, so each
        // fixture combat is marked won.
        for combat_n in 1i32..=2 {
            let mut state = enter_combat_from_roster(&rs, &roster, RoomType::Monster);
            state.player_won = true;
            exit_combat(&mut rs, &state);
            assert!(
                !rs.master_deck[0].upgraded,
                "no upgrade after normal combat #{combat_n}"
            );
            assert_eq!(
                rs.relic_counters.get(&sts2_core::RelicId::FishingRod),
                Some(&combat_n),
                "CombatsSeen ticks each normal combat"
            );
        }

        // Combat 3: CombatsSeen % 3 == 0 -> upgrade the (only) upgradable card.
        let mut state = enter_combat_from_roster(&rs, &roster, RoomType::Monster);
        state.player_won = true;
        exit_combat(&mut rs, &state);
        assert!(
            rs.master_deck[0].upgraded,
            "FishingRod upgrades a deck card on the 3rd normal combat"
        );

        // A LOST combat must NOT tick CombatsSeen nor upgrade (C#
        // ProcessPendingLoss never reaches Hook.AfterCombatEnd). Reset the
        // upgrade flag and run a 4th combat as a loss: counter stays at 3.
        rs.master_deck[0] =
            crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false)
                .expect("Strike POD");
        let mut lost = enter_combat_from_roster(&rs, &roster, RoomType::Monster);
        lost.player_won = false;
        exit_combat(&mut rs, &lost);
        assert!(
            !rs.master_deck[0].upgraded,
            "a lost combat leaves the deck untouched"
        );
        assert_eq!(
            rs.relic_counters.get(&sts2_core::RelicId::FishingRod),
            Some(&3),
            "CombatsSeen does not tick on a lost combat"
        );
    }

    /// FishingRod's Niche pick must read the POST-combat cursor and PERSIST
    /// its advance into the run stream (regression for the
    /// `…wb8lk.6` blocker: the draw was placed BEFORE the
    /// `rs.rng = combat.run_rng.clone()` flushback, so it read the stale
    /// pre-combat cursor AND was clobbered by the flushback — desyncing the
    /// run-wide niche stream after every firing). A multi-card upgradable deck
    /// makes the picked index non-trivial; the assertions hold regardless of
    /// whether this particular combat advanced niche.
    #[test]
    fn fishing_rod_niche_draw_reads_post_combat_cursor_and_persists() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RoomType;

        let roster = [(MonsterKind::FuzzyWurmCrawler, MonsterSpawnArgs::None, 57)];
        let mut rs = fresh_run();
        rs.relics.push(sts2_core::RelicId::FishingRod);
        // Four distinct upgradable cards so the Niche index 0..=3 selects a
        // specific, identifiable card.
        rs.master_deck = [
            CardId::StrikeIronclad,
            CardId::DefendIronclad,
            CardId::Bash,
            CardId::Anger,
        ]
        .into_iter()
        .map(|id| crate::cards::spec_index::card_pod_from_id(id, false).expect("upgradable POD"))
        .collect();
        assert!(
            rs.master_deck.iter().all(sts2_core::Card::is_upgradable),
            "all four seed cards are upgradable"
        );

        // Combats 1 and 2: counter ticks, no upgrade. Each marked won so
        // FishingRod's win-gated hook fires.
        for _ in 1i32..=2 {
            let mut state = enter_combat_from_roster(&rs, &roster, RoomType::Monster);
            state.player_won = true;
            exit_combat(&mut rs, &state);
        }

        // Combat 3 triggers. Capture the post-combat niche cursor and the index
        // the FIXED code must pick (probe a clone of the flushed-in stream).
        let mut state = enter_combat_from_roster(&rs, &roster, RoomType::Monster);
        state.player_won = true;
        let post_combat_counter = state.run_rng.niche.counter();
        let last = i32::try_from(rs.master_deck.len() - 1).unwrap();
        let expected_idx = state.run_rng.clone().niche.next_int(0, last) as usize;
        exit_combat(&mut rs, &state);

        // (a) the card the POST-combat cursor selects is the one upgraded.
        for (i, card) in rs.master_deck.iter().enumerate() {
            assert_eq!(
                card.upgraded,
                i == expected_idx,
                "only deck card {expected_idx} (post-combat Niche pick) is upgraded; card {i} mismatch"
            );
        }
        // (b) the FishingRod draw advanced the RUN niche stream by exactly one
        // past the flushback value — i.e. the advance persisted (pre-fix it was
        // discarded, leaving the counter at the flushback value).
        assert_eq!(
            rs.rng.niche.counter(),
            post_combat_counter + 1,
            "FishingRod's niche advance persists into the run stream"
        );
    }

    /// FishingRod ignores Elite/Boss combats (`RoomType != Monster`): the
    /// counter never ticks and no card is upgraded across 3 elite fights.
    #[test]
    fn fishing_rod_does_not_count_elite_or_boss_combats() {
        use sts2_core::cards::CardId;
        use sts2_core::runstate::RoomType;

        let roster = [(MonsterKind::FuzzyWurmCrawler, MonsterSpawnArgs::None, 57)];
        let mut rs = fresh_run();
        rs.relics.push(sts2_core::RelicId::FishingRod);
        rs.master_deck =
            vec![
                crate::cards::spec_index::card_pod_from_id(CardId::StrikeIronclad, false)
                    .expect("Strike POD"),
            ];

        for room in [RoomType::Elite, RoomType::Boss, RoomType::Elite] {
            let state = enter_combat_from_roster(&rs, &roster, room);
            exit_combat(&mut rs, &state);
        }
        assert!(
            !rs.master_deck[0].upgraded,
            "Elite/Boss combats never trigger FishingRod"
        );
        assert_eq!(
            rs.relic_counters
                .get(&sts2_core::RelicId::FishingRod)
                .copied()
                .unwrap_or(0),
            0,
            "CombatsSeen does not tick on Elite/Boss rooms"
        );
    }

    /// `ToyBox.AfterCombatEnd` (`MegaCrit.Sts2.Core.Models.Relics/ToyBox.cs:99-112`)
    /// increments `CombatsSeen` FIRST, then melts on `% Combats == 0` — so the
    /// melts land on combats 3/6/9/12, and each one takes the FIRST still-unmelted
    /// wax relic in INVENTORY order (`Owner.Relics.FirstOrDefault(...)`), skipping
    /// the non-wax relic in the middle. Melting never removes anything from the
    /// inventory.
    #[test]
    fn toy_box_melts_wax_relics_in_inventory_order_every_third_combat() {
        use sts2_core::runstate::RoomType;
        use sts2_core::RelicId;
        let roster = [(MonsterKind::FuzzyWurmCrawler, MonsterSpawnArgs::None, 57)];
        let mut rs = fresh_run();
        rs.relics.push(RelicId::ToyBox);
        // [A wax, B wax, C not-wax, D wax, E wax] in pickup order.
        let wax_a = RelicId::Akabeko;
        let wax_b = RelicId::Vajra;
        let plain_c = RelicId::Anchor;
        let wax_d = RelicId::Lantern;
        let wax_e = RelicId::PenNib;
        for id in [wax_a, wax_b, plain_c, wax_d, wax_e] {
            rs.relics.push(id);
        }
        for id in [wax_a, wax_b, wax_d, wax_e] {
            rs.wax_relics.insert(id);
        }

        let mut order: Vec<RelicId> = Vec::new();
        for combat_n in 1i32..=12 {
            let mut state = enter_combat_from_roster(&rs, &roster, RoomType::Monster);
            // The relic mirror must carry wax/melt state INTO combat.
            for (id, expect_wax) in [(wax_a, true), (plain_c, false)] {
                let kind = id.to_relic_kind().expect("mapped");
                let r = state
                    .relics
                    .iter()
                    .find(|r| r.kind == kind)
                    .expect("mirrored");
                assert_eq!(r.is_wax, expect_wax, "wax mirrored for {id:?}");
                assert_eq!(
                    r.is_melted,
                    rs.melted_relics.contains(&id),
                    "melt mirrored for {id:?}"
                );
            }
            state.player_won = true;
            let before = rs.melted_relics.len();
            exit_combat(&mut rs, &state);
            assert_eq!(
                rs.relic_counters.get(&RelicId::ToyBox),
                Some(&combat_n),
                "CombatsSeen ticks every won combat"
            );
            if rs.melted_relics.len() > before {
                let fresh = *rs
                    .melted_relics
                    .iter()
                    .find(|id| !order.contains(id))
                    .expect("newly melted");
                order.push(fresh);
                assert_eq!(combat_n % 3, 0, "melts only on every 3rd combat");
            }
        }

        assert_eq!(
            order,
            vec![wax_a, wax_b, wax_d, wax_e],
            "inventory order, non-wax skipped"
        );
        assert!(
            !rs.melted_relics.contains(&plain_c),
            "non-wax relic never melts"
        );
        assert_eq!(rs.relics.len(), 6, "melting keeps every relic equipped");
        assert!(
            !rs.owns_active_relic(wax_a) && rs.owns_active_relic(plain_c),
            "melted relics drop out of the run-side hook guard only"
        );
    }
}
