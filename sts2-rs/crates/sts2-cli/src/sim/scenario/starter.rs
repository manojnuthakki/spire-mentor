//! `scenario::starter::build_state` — assemble a [`CombatState`] from
//! a [`Scenario`] description. Bead `sts2-rs-r1q8` generalizes A1's
//! single-monster path to support `Vec<MonsterSpec>`.

use smallvec::SmallVec;
use sts2_content::cards::ironclad_basic::ironclad_starter_deck;
use sts2_content::cards::silent::silent_starter_deck;
use sts2_core::cards::Card;
use sts2_core::{CardUid, CombatPhase, CombatState, OrbSlots, PlayerCombatState, Side, TurnEvents};

use super::Scenario;
use crate::sim::PlayerClass;

/// Build a [`CombatState`] ready for `combat_loop::start_combat`.
///
/// Spawns `scenario.monsters` the way a real run does
/// ([`sts2_content::run_bridge::spawn_roster`]): monster `i` gets
/// `CreatureUid` `i + 1`, and the player's mirror creature the next one.
///
/// Delegates to [`build_state_with_ascension`] at ascension 0 — kept
/// byte-stable for existing books/harnesses that don't model ascension.
pub fn build_state(
    scenario: &Scenario,
    seed: u64,
    player: PlayerClass,
    hp_override: Option<u32>,
    max_hp_override: Option<u32>,
) -> CombatState {
    build_state_with_ascension(scenario, seed, player, hp_override, max_hp_override, 0)
}

/// Ascension-aware variant of [`build_state`]. `ascension` is passed
/// EXPLICITLY by the caller — it is not read from `[player].ascension`
/// inside the loader, so existing harnesses whose harvested fixtures
/// carry a nonzero ascension field stay byte-stable through `build_state`.
pub fn build_state_with_ascension(
    scenario: &Scenario,
    seed: u64,
    player: PlayerClass,
    hp_override: Option<u32>,
    max_hp_override: Option<u32>,
    ascension: u8,
) -> CombatState {
    // Install ALL content-side card pools, not just the core
    // Strike/Defend/Bash/AscendersBane starters. Without this, decks
    // built via --deck-preset / --deck-file containing common cards
    // (Anger, PommelStrike, etc.) fall through to NotImplemented and
    // play as no-ops.
    sts2_content::cards::dispatch::ensure_installed();

    // The run-side character this scenario stands in for. Only starting gold
    // reads it today (see the `gold` field below); the class-specific deck /
    // max-HP resolution stays on the `PlayerClass` match right after.
    let character_id = match player {
        PlayerClass::Ironclad => sts2_core::potions::CharacterId::Ironclad,
        PlayerClass::Silent => sts2_core::potions::CharacterId::Silent,
        PlayerClass::Defect => sts2_core::potions::CharacterId::Defect,
        PlayerClass::Regent => sts2_core::potions::CharacterId::Regent,
        PlayerClass::Necrobinder => sts2_core::potions::CharacterId::Necrobinder,
    };
    let (class_default_max_hp, deck_specs) = match player {
        PlayerClass::Ironclad => (80_i32, ironclad_starter_deck()),
        PlayerClass::Silent => (70_i32, silent_starter_deck()),
        // Defect/Regent/Necrobinder are accepted at the scenario-loader
        // layer (bead `sts2-rs-5m6k`) so .run-corpus fixtures can flow
        // past the loader and surface real per-class content gaps one
        // layer deeper. Until P6 wires their starter decks, use the
        // Ironclad starter as a placeholder.
        PlayerClass::Defect | PlayerClass::Regent | PlayerClass::Necrobinder => {
            (80_i32, ironclad_starter_deck())
        }
    };
    // Bead `sts2-rs-f3g0`: `max_hp` and `current_hp` are independent.
    // Precedence: explicit `max_hp_override` > `hp_override` (back-compat
    // — pre-f3g0 the loader collapsed both onto the single `--hp` value)
    // > class default. `current_hp` then takes `hp_override` if set,
    // capped at the resolved `max_hp`, else equals `max_hp`.
    let max_hp = max_hp_override
        .map(|h| i32::try_from(h).expect("max_hp fits i32"))
        .or_else(|| hp_override.map(|h| i32::try_from(h).expect("hp fits i32")))
        .unwrap_or(class_default_max_hp);
    let current_hp = hp_override.map_or(max_hp, |h| {
        i32::try_from(h).expect("hp fits i32").min(max_hp)
    });

    let mut cards_arena: Vec<Card> = Vec::with_capacity(deck_specs.len());
    let mut draw_uids: SmallVec<[CardUid; 32]> = SmallVec::new();
    for (idx, spec) in deck_specs.iter().enumerate() {
        let raw = u32::try_from(idx + 1).expect("deck size fits u32");
        let uid = CardUid::new(raw).expect("non-zero");
        cards_arena.push(Card {
            uid,
            card_id: spec.id,
            cost: spec.cost,
            card_type: spec.card_type,
            target_type: spec.target_type,
            upgraded: false,
            base_damage: spec.base_damage,
            base_block: spec.base_block,
            ..Card::pod_defaults()
        });
        draw_uids.push(uid);
    }

    let player_state = PlayerCombatState {
        current_hp,
        max_hp,
        block: 0,
        energy: 0,
        max_energy: 3,
        hand: SmallVec::new(),
        draw: draw_uids,
        discard: SmallVec::new(),
        exhaust: SmallVec::new(),
        play: SmallVec::new(),
        stars: 0,
        powers: SmallVec::new(),
        character: None,
        potions: smallvec::SmallVec::new(),
        potion_slot_indices: SmallVec::new(),
        potion_slots: 0,
        orbs: OrbSlots::default(),
        creature_uid: None,
        pet_uid: None,
        shivs_played_this_turn: 0,
        cards_played_this_turn: [0; 5],
        powered_hits_this_turn: 0,
        powered_hits_by_target_this_turn: smallvec::SmallVec::new(),
        powered_hits_at_card_play_start: 0,
        attack_command_suppressed_this_play: false,
        vigor_latch_card: None,
        generated_cards_this_combat: 0,
        skills_played_this_turn: 0,
        card_plays_started_this_turn: 0,
        cards_first_in_series_this_turn: 0,
        request_end_turn_after_play: false,
        retain_this_turn: smallvec::SmallVec::new(),
        sly_this_turn: smallvec::SmallVec::new(),
        sly_this_combat: smallvec::SmallVec::new(),
        pending_retain_count: 0,
        stars_gained_this_turn: 0,
        pinned_top: smallvec::SmallVec::new(),
        pinned_bottom: smallvec::SmallVec::new(),
        deferred_draw_order: false,
    };

    let mut state = CombatState {
        belt_value: None,
        phase: CombatPhase::Setup,
        round: 0,
        side: Side::Player,
        rng: sts2_core::dotnet_rng::Rng::new(seed as i32),
        run_rng: sts2_core::run_rng_set::RunRngSet::new(seed as i32),
        creatures: Vec::new(),
        cards: cards_arena.into(),
        powers: Vec::new().into(),
        relics: Vec::new().into(),
        potions: Vec::new(),
        player: player_state,
        allies: SmallVec::new(),
        enemy_uids: SmallVec::new(),
        pending_choice: None,
        resolving_card: None,
        pending_play: None,
        pending_turn_setup_resume: false,
        pending_draw_resume: None,
        pending_snecko_cost_randomize: false,
        pending_cluster_resume: None,
        pending_hand_draw_resume: None,
        pending_relic_hand_draw_resume: None,
        pending_wlp_retain_resume: false,
        pending_end_turn_resume: None,
        pending_autoplay_resume: None,
        autoplay_inline_resolve: false,
        pending_curse_choice: None,
        pending_potion_hook_resume: None,
        pending_enemy_exec_resume: None,
        combat_log: TurnEvents::default(),
        swipe_stolen: Vec::new(),
        escaped_creatures: 0,
        heist_recovered_gold: 0,
        room_extra_rewards: Vec::new(),
        event_combat_timed_out: false,
        // The player's run gold, carried into combat. NOT zero: the C# parity
        // oracle builds its synthetic combats out of a real run
        // (`start` + `skip_neow` + `set_player`), so its player walks in with
        // the character's starting gold — and gold is a combat input, not just
        // a run-level scoreboard. GremlinMerc's `ThieveryPower.Steal()` reads
        // it, caps at 20/steal, and banks the total into the HeistPower the
        // FatGremlin inherits on the merc's death; SealOfGold spends 5.
        //
        // With `gold: 0` every one of those paths no-opped, so the synthetic
        // harness could not see them at all — the Underdocks encounter sweep
        // (GH #544) found the merc's Heist missing purely because of this,
        // with the engine side correct. Every character starts on 99
        // (`character_starter`), which is what the oracle has.
        gold: sts2_core::characters::character_starter(character_id).gold,
        is_over: false,
        player_won: false,
        ascension,
        master_deck: Vec::new(),
        last_added_card: None,
        is_elite: false,
        is_boss: false,
        act_index: 1,
        active_attack: None,
        energy_wasted: 0,
        cards_drawn: 0,
        pending_run_gold_loss: 0,
        last_drawn_card: None,
    };

    // Monsters are spawned exactly as a real run spawns them (GH #762):
    // `run_bridge::spawn_roster` builds each AI from the monster's roster
    // position and spawn args, wires the player's mirror creature, then applies
    // the on-spawn powers (including the ones a monster lands on the player,
    // e.g. Rocket's Surrounded). HP is rolled the run's way too: every monster
    // reads the combat's `niche` stream with C#'s uniqueness rule
    // (`roll_unique_max_hps`, `SetUniqueMonsterHpValue`), so HP is independent
    // of whatever stream drew the roster and no two enemies share a value when
    // their ranges allow it. A spec's `hp_override` replaces its rolled value
    // afterwards; the roll still happens, so `niche` sits where a run leaves it
    // for a mid-fight summon.
    let kinds: Vec<sts2_core::MonsterKind> = scenario.monsters.iter().map(|m| m.kind).collect();
    let rolled =
        sts2_core::monsters::roll_unique_max_hps(&kinds, &mut state.run_rng.niche, ascension);
    let roster: Vec<_> = scenario
        .monsters
        .iter()
        .zip(rolled)
        .map(|(m, hp)| {
            let hp = m
                .hp_override
                .map_or(hp, |h| i32::try_from(h).expect("hp fits i32"));
            (m.kind, m.spawn_args(), hp)
        })
        .collect();
    let uids = sts2_content::run_bridge::spawn_roster(&mut state, &roster);

    // GH #552: stamp each member's encounter slot. `resolve_encounter` returns
    // a bare `Vec<MonsterSpec>` — no `EncounterDef` survives to here — so the
    // slot rides on the spec, set by `specs_from_encounter_def`. It stays
    // `None` for ad-hoc `--monsters` compositions and hand-written TOML
    // scenarios, which have no encounter and therefore no `Slots` to index.
    //
    // Without this a mid-combat summon in a slotted encounter would append
    // rather than take its slot, so books and corpora built through the CLI
    // would order a summon differently from the game.
    for (uid, m) in uids.iter().zip(&scenario.monsters) {
        if let Some(slot) = m.slot {
            state.set_enemy_slot(*uid, slot);
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::scenario::{MonsterSpawnArgs, MonsterSpec, Scenario};
    use sts2_core::MonsterKind;

    fn nibbit_scenario() -> Scenario {
        Scenario::from_monsters(vec![MonsterSpec {
            slot: None,
            kind: MonsterKind::Nibbit,
            hp_override: None,
            ai_args: super::super::encounter::MonsterAiArgs {
                is_alone: true,
                is_front: true,
            },
            args: MonsterSpawnArgs::None,
        }])
    }

    /// Bead `sts2-rs-f3g0`: with separate `hp_override` and
    /// `max_hp_override`, the player starts at `hp/max_hp` rather than
    /// the collapsed `hp/hp` the pre-f3g0 loader produced.
    #[test]
    fn build_state_threads_independent_hp_and_max_hp() {
        let state = build_state(
            &nibbit_scenario(),
            7,
            PlayerClass::Ironclad,
            Some(20),
            Some(80),
        );
        assert_eq!(state.player.current_hp, 20);
        assert_eq!(state.player.max_hp, 80);
    }

    /// Back-compat: when `max_hp_override` is `None`, the loader still
    /// collapses both onto `hp_override` (the pre-f3g0 behavior used
    /// by every CLI path that doesn't pass `--max-hp`).
    #[test]
    fn build_state_collapses_when_max_hp_unset() {
        let state = build_state(&nibbit_scenario(), 7, PlayerClass::Ironclad, Some(20), None);
        assert_eq!(state.player.current_hp, 20);
        assert_eq!(state.player.max_hp, 20);
    }

    /// `hp_override > max_hp_override` is clamped to `max_hp` rather
    /// than panicking — defensive against fixture-emitter bugs.
    #[test]
    fn build_state_clamps_current_hp_to_max_hp() {
        let state = build_state(
            &nibbit_scenario(),
            7,
            PlayerClass::Ironclad,
            Some(120),
            Some(80),
        );
        assert_eq!(state.player.current_hp, 80);
        assert_eq!(state.player.max_hp, 80);
    }

    #[test]
    fn build_state_with_ascension_scales_and_stamps() {
        let scenario = nibbit_scenario();
        let a0 = build_state_with_ascension(&scenario, 7, PlayerClass::Ironclad, None, None, 0);
        let a9 = build_state_with_ascension(&scenario, 7, PlayerClass::Ironclad, None, None, 9);
        assert_eq!(a0.ascension, 0);
        assert_eq!(a9.ascension, 9);
        // A8 ToughEnemies: Nibbit's HP range is 42..=46 at A0 and
        // 44..=48 at ascension >= 8 (`MonsterKind::hp_roll`), so with
        // the same seed the rolled HP must differ.
        let a8 = build_state_with_ascension(&scenario, 7, PlayerClass::Ironclad, None, None, 8);
        assert_eq!(a8.ascension, 8);
        assert_ne!(
            a0.creatures[0].current_hp, a8.creatures[0].current_hp,
            "A8 ToughEnemies HP roll must differ from A0 for Nibbit (42..=46 vs 44..=48)"
        );
        // Byte-stability: the legacy entry point == ascension 0.
        let legacy = build_state(&scenario, 7, PlayerClass::Ironclad, None, None);
        assert_eq!(sts2_core::state_hash(&legacy), sts2_core::state_hash(&a0));
    }
}
