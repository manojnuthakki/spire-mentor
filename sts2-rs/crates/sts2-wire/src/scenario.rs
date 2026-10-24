//! Building a fresh combat from a scenario spec — the one path shared by the
//! Python parity oracle (`sts2-py`) and the browser workbench
//! (`sts2-workbench` → wasm).
//!
//! [`build_scenario_state`] used to live in `sts2-py` as `build_scenario_state_impl`,
//! reachable only through PyO3. The browser SimLab sandbox needs the exact same
//! construction (a "New fight" board, and the opener roll for an enemy whose
//! intent the user left unspecified), and a duplicate would silently drift from
//! the oracle the first time either side changed. So it lives here, in the wire
//! crate both hosts already depend on; `sts2-py` now delegates to it, which
//! keeps the C#-parity fixtures and the browser byte-identical by construction.

use crate::state_dict;
use smallvec::SmallVec;
use sts2_content::cards::dispatch as card_dispatch;
use sts2_content::monster_dispatch::MonsterSpawnArgs;
use sts2_core::cards::Card;
use sts2_core::combat_loop::{start_combat, start_player_turn};
use sts2_core::{
    CardUid, CombatPhase, CombatState as CoreCombatState, MonsterKind, PlayerCombatState, Side,
    TurnEvents,
};

fn starter_args_for_move_state(kind: MonsterKind, move_state: &str) -> Option<MonsterSpawnArgs> {
    match kind {
        MonsterKind::ScrollOfBiting => {
            let starter_idx = match move_state {
                "CHOMP" => 0,
                "CHEW" => 1,
                "MORE_TEETH" => 2,
                _ => return None,
            };
            Some(MonsterSpawnArgs::ScrollOfBiting { starter_idx })
        }
        // Bead `sts2-rs-z4jsg`: `DecimillipedeElite.GenerateMonsters`
        // (DecimillipedeElite.cs:37-50) rolls `StarterMoveIdx =
        // Rng.NextInt(3)` on the per-encounter RNG for the Front segment
        // and rotates +1/+2 onto Middle/Back; the segment machine's
        // initial-state switch (DecimillipedeSegment.cs:171-176) is
        // 0→WRITHE_MOVE, 1→BULK_MOVE, _→CONSTRICT_MOVE — the inverse
        // mapped here (keep in lock-step with
        // `create_decimillipede_segment_ai`). The synthetic builder
        // can't replicate the encounter RNG seed, so it adopts the
        // rotation C# actually rolled, exactly like ScrollsOfBiting.
        MonsterKind::DecimillipedeSegment
        | MonsterKind::DecimillipedeSegmentFront
        | MonsterKind::DecimillipedeSegmentMiddle
        | MonsterKind::DecimillipedeSegmentBack => {
            let starter_idx = match move_state {
                "WRITHE_MOVE" => 0,
                "BULK_MOVE" => 1,
                "CONSTRICT_MOVE" => 2,
                _ => return None,
            };
            Some(MonsterSpawnArgs::DecimillipedeSegment { starter_idx })
        }
        // GH #544: the two Act-1 Underdocks encounters whose `GenerateMonsters`
        // rolls a starter-move ROTATION on the per-encounter RNG, the same
        // shape as ScrollsOfBiting and Decimillipede above.
        //
        // `CorpseSlug.EnsureCorpseSlugsStartWithDifferentMoves` draws one
        // `NextInt(3)` and walks consecutive indices across the slugs;
        // `CorpseSlug.GenerateMoveStateMachine` maps `StarterMoveIdx % 3` to
        // WHIP_SLAP / GLOMP / GOOP.
        //
        // NOTE the ids are C#'s `MoveState` names as the oracle reports them
        // (`_MOVE`-suffixed), NOT the Rust state ids — this function keys off
        // `get_state`'s `monsters[].move_state`. The two differ for both
        // monsters below.
        MonsterKind::CorpseSlug => {
            let starter_idx = match move_state {
                "WHIP_SLAP_MOVE" => 0,
                "GLOMP_MOVE" => 1,
                "GOOP_MOVE" => 2,
                _ => return None,
            };
            Some(MonsterSpawnArgs::CorpseSlug { starter_idx })
        }
        // `TwoTailedRatsNormal` rolls one `NextInt(3)` and hands the three rats
        // `n`, `n+1`, `n+2`; `TwoTailedRat.GenerateMoveStateMachine` maps
        // `StarterMoveIndex % 3` to SCRATCH / DISEASE_BITE / SCREECH (and -1,
        // for a mid-combat summon, to the RAND branch — which is what the
        // `_ => return None` fallthrough leaves in place).
        //
        // This is not cosmetic even though the three rats are identical: the
        // RAND branch is `CannotRepeat`, so which move a rat just performed
        // decides what its next roll may return, and a rotation offset that
        // disagrees with C#'s desyncs the `monster_ai` stream.
        MonsterKind::TwoTailedRat => {
            let slot = match move_state {
                "SCRATCH_MOVE" => sts2_content::monsters::act1_underdocks::TwoTailedRatSlot::First,
                "DISEASE_BITE_MOVE" => {
                    sts2_content::monsters::act1_underdocks::TwoTailedRatSlot::Second
                }
                "SCREECH_MOVE" => sts2_content::monsters::act1_underdocks::TwoTailedRatSlot::Third,
                _ => return None,
            };
            Some(MonsterSpawnArgs::TwoTailedRat { slot })
        }
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_scenario_state(
    seed: u64,
    character: &str,
    deck_ids: &[String],
    hp: i32,
    max_hp: i32,
    relic_ids: &[String],
    potion_ids: &[String],
    encounter: &str,
    ascension: u8,
    monster_override: Option<&[String]>,
    monster_move_states: Option<&[Option<String>]>,
) -> Result<CoreCombatState, String> {
    // Map the scenario's capitalized class string to a `CharacterId`. The
    // deck loader (`resolve_card_name` below) already searches every
    // character's spec slice, so any ported character can build a scenario;
    // the only character-specific field is `player.character`, set below so
    // class-gated generators (OrobicAcid, deck-search potions) behave. Was
    // Ironclad-only in v1 (bead `sts2-rs-q6bix`); lifted so the Regent
    // coverage pack can A/B its 88 cards against the C# oracle.
    let character_id = match character {
        "Ironclad" => sts2_core::CharacterId::Ironclad,
        "Silent" => sts2_core::CharacterId::Silent,
        "Defect" => sts2_core::CharacterId::Defect,
        "Regent" => sts2_core::CharacterId::Regent,
        "Necrobinder" => sts2_core::CharacterId::Necrobinder,
        other => {
            return Err(format!(
                "build_scenario_state: unknown character {other:?} (expected one of \
                 Ironclad/Silent/Defect/Regent/Necrobinder)"
            ));
        }
    };
    if hp <= 0 || max_hp <= 0 || hp > max_hp {
        return Err(format!(
            "build_scenario_state: invalid hp={hp}/max_hp={max_hp}"
        ));
    }

    // Bead `sts2-rs-7hhd` / `sts2-rs-ov4`: populate the PlayCard dispatch
    // registry before the first `Action::PlayCard` can fire. Idempotent.
    card_dispatch::install_default_registry();
    // Parity coverage (2026-05-29): also install the relic + potion
    // dispatch registries so that relics pushed into `state.relics` below
    // actually fire their `before_combat_start` / reactive listeners
    // during `start_combat`, and held potions resolve their on-drink
    // bodies. Without the relic install, raw-pushed relics are inert
    // (e.g. BronzeScales Thorns never applies) and the parity harness
    // can't compare relic behavior. All three installs are idempotent.
    sts2_content::relics::dispatch::install_default_registry();
    sts2_content::potions::dispatch::install_default_registry();

    // Resolve encounter across ALL act registries (Act 1 → 2 → 3) so the
    // all-acts coverage sweep can build Act-2/Act-3 rosters, not just Act-1
    // ids like `SHRINKER_BEETLE_WEAK` / `VANTOM_BOSS`. Bead `sts2-rs-zdjjx`.
    let enc = sts2_content::encounters::find(encounter)
        .ok_or_else(|| format!("unknown encounter id {encounter:?}"))?;

    // Build the runtime card arena from the deck-id list. Each id is
    // looked up via `resolve_card_name`, which already supports the
    // canonical UPPER_SNAKE form used by T6's `IC_CARDS` constant.
    let mut draw: SmallVec<[CardUid; 32]> = SmallVec::new();
    let mut cards_arena: Vec<Card> = Vec::with_capacity(deck_ids.len());
    for (idx, name) in deck_ids.iter().enumerate() {
        // Sweep v2: an optional "+N" suffix ("BASH+1") requests an upgraded
        // card. Parsed here (resolve_card_name only knows base ids) and
        // applied below via the canonical `Card::upgrade()` delta path.
        // STS2 cards are single-level (C# MaxUpgradeLevel default 1), so
        // only "+1" is accepted — "+0"/"+2"/etc. are rejected rather than
        // silently clamped. The `is_upgradable()` gate below additionally
        // covers the curse/status (MaxUpgradeLevel 0) and already-upgraded
        // cases, degrading those "+1"s to base.
        let (base_name, plus_levels) = match name.split_once('+') {
            Some((base, lvl)) => {
                let n: u32 = lvl.parse().map_err(|_| {
                    format!("bad upgrade suffix in card id {name:?}: expected +<integer>")
                })?;
                if n != 1 {
                    return Err(format!(
                        "bad upgrade suffix in card id {name:?}: STS2 cards are single-level, expected +1"
                    ));
                }
                (base, n)
            }
            None => (name.as_str(), 0),
        };
        let spec = sts2_content::names::resolve_card_name(base_name, None)
            .map_err(|e| format!("unknown card id {base_name:?}: {e:?}"))?;
        let raw = u32::try_from(idx + 1).map_err(|_| "deck size overflows u32".to_string())?;
        let uid = CardUid::new(raw).ok_or_else(|| "uid non-zero".to_string())?;
        draw.push(uid);
        let mut card = Card {
            uid,
            card_id: spec.id,
            cost: spec.cost,
            card_type: spec.card_type,
            target_type: spec.target_type,
            upgraded: false,
            base_damage: spec.base_damage,
            base_block: spec.base_block,
            // Mirror `card_from_spec` (status.rs): populate the canonical
            // CardTags. Without this, tag-gated handlers diverge from C#
            // (e.g. FastenPower.ModifyBlockAdditive only folds onto block
            // from `CardTag::Defend` sources). q6bix coverage-sweep fix.
            tags: sts2_content::cards::status::tags_for_card_id(spec.id),
            ..Card::pod_defaults()
        };
        if plus_levels >= 1 && card.is_upgradable() {
            card.upgrade();
        }
        cards_arena.push(card);
    }

    // Resolve the held-potion list (canonical UPPER_SNAKE ids, e.g.
    // `FIRE_POTION`) into `PotionKind`s for `player.potions`. Potions are
    // placed directly into the player's belt — the same target the C#
    // headless `set_player` "potions" arg fills (`RunSimulator.cs:433`) —
    // so both engines start combat with byte-identical potion slots. The
    // on-drink RNG/effect dispatch lives in `sts2-content::potions` and
    // fires when a `UsePotion`/`DrinkPotion` action is applied; nothing
    // here perturbs combat RNG at setup.
    let mut potions_vec: SmallVec<[sts2_core::PotionKind; sts2_core::MAX_POTIONS]> =
        SmallVec::new();
    for name in potion_ids {
        let kind = state_dict::potion_kind_from_python_name(name)
            .ok_or_else(|| format!("unknown potion id {name:?}"))?;
        potions_vec.push(kind);
    }

    let player = PlayerCombatState {
        hand: SmallVec::new(),
        draw,
        discard: SmallVec::new(),
        exhaust: SmallVec::new(),
        play: SmallVec::new(),
        energy: 0,
        max_energy: 3,
        stars: 0,
        current_hp: hp,
        max_hp,
        block: 0,
        powers: SmallVec::new(),
        // Character-card generators (OrobicAcid, deck-search potions)
        // early-return on a `None` character; set it to the scenario's
        // character (parsed above) so they generate vs C#. q6bix fix;
        // generalized beyond Ironclad for the Regent coverage pack.
        character: Some(character_id),
        // Compact identity slot mapping — a parity fixture's belt starts
        // hole-free (0..len); holes only develop from in-combat drinks.
        potion_slot_indices: (0..potions_vec.len() as u8).collect(),
        potions: potions_vec,
        // C# `Player.initialMaxPotionSlotCount == 3` and the headless
        // harness `set_player` never resizes `_potionSlots`
        // (RunSimulator.cs:471-498), so a scenario player has exactly 3
        // potion slots. Seeding the cap here keeps slot-filling potions
        // (EntropicBrew / Alchemize) from over-filling to MAX_POTIONS=5
        // and surfacing `use_potion` slots 3/4 that C# never emits
        // (bead sts2-rs-eysmt).
        potion_slots: 3,
        orbs: sts2_core::OrbSlots::default(),
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
        pinned_bottom: SmallVec::new(),
        deferred_draw_order: false,
    };

    let rng = sts2_core::dotnet_rng::Rng::new((seed) as i32);
    // Build the per-stream RNG bundle up-front. Monster HP rolls below
    // consume from `run_rng.niche`, mirroring C#
    // `CombatState.CreateCreature` (`SetUniqueMonsterHpValue(...,
    // RunState.Rng.Niche)`) instead of the legacy monolithic combat
    // RNG, so a shuffle won't perturb the next HP roll and vice versa.
    // Bead `sts2-rs-943w7`. The bundle is moved into `state.run_rng`
    // after the HP-roll loop completes.
    let mut run_rng = sts2_core::run_rng_set::RunRngSet::new(seed as i32);

    // sts2-rs-3010t: when the harness passes an explicit monster_list
    // (resolved on the C# side post-EnterRoom), use it verbatim so
    // randomized encounters (FLYCONID_NORMAL, SLITHERING_STRANGLER_NORMAL,
    // SLIMES_*) reach roster parity without aligning the two RNGs.
    // Each entry must be a canonical Pascal-case monster name
    // (`MonsterKind::python_name`), matching the ids the C# headless
    // `get_state` surface emits under `monsters[].monster_id`.
    //
    // Per-monster spawn-args from the encounter table (Inklet slot,
    // Nibbit position, KinFollower slot) are dropped under the
    // override path — those scaffolding bits would need a richer
    // protocol from C# to round-trip, which is out of 3010t's scope.
    // Per-monster (kind, spawn-args) pairs. The encounter-table path
    // carries explicit MonsterSpawnArgs (Nibbit group-position fix-up,
    // Inklet slot, KinFollower slot, etc.); the harness-override path
    // (sts2-rs-3010t) defaults to `MonsterSpawnArgs::None` since C#
    // doesn't round-trip per-monster spawn args today.
    let roster: Vec<(MonsterKind, MonsterSpawnArgs)> = match monster_override {
        Some(names) => names
            .iter()
            .map(|n| {
                state_dict::monster_kind_from_python_name(n)
                    .map(|k| (k, MonsterSpawnArgs::None))
                    .ok_or_else(|| format!("unknown monster id {n:?}"))
            })
            .collect::<Result<_, _>>()?,
        None => enc.monsters.to_vec(),
    };

    // sts2-rs-u3wjj: when the coverage sweep forwards C#'s observed
    // per-monster initial move-state ids, force-match any monster whose
    // starting move is picked by an RNG-branched `StarterMoveIdx`
    // (ScrollsOfBiting{Weak,Normal} roll `StarterMoveIdx = Rng.NextInt(3)`
    // then rotate +1/+2 across the roster). The synthetic scenario builder
    // doesn't replicate C#'s per-encounter RNG seeding (a fresh
    // `Rng(runState.Seed + TotalFloor + hash(id))`), so it can't roll the
    // same rotation itself — instead it adopts the rotation C# actually
    // rolled. `starter_args_for_move_state` returns `None` for monsters
    // with no starter knob, leaving the encounter-table default intact, so
    // this is a no-op for every non-rotating encounter.
    let roster: Vec<(MonsterKind, MonsterSpawnArgs)> = match monster_move_states {
        Some(states) => roster
            .into_iter()
            .enumerate()
            .map(|(i, (kind, args))| {
                let forced = states
                    .get(i)
                    .and_then(Option::as_deref)
                    .and_then(|sid| starter_args_for_move_state(kind, sid))
                    .unwrap_or(args);
                (kind, forced)
            })
            .collect(),
        None => roster,
    };

    // C# assigns enemy HP in TWO passes: `CombatState.CreateCreature` rolls
    // every enemy's `MaxHp` off `RunState.Rng.Niche`
    // (`Creature.SetUniqueMonsterHpValue`), excluding the RAW `MaxHp` of all
    // same-side creatures already created (any kind), then a later
    // `AfterAddedToRoom` pass applies the Decimillipede even-unique fix-up /
    // Door sentinel off the mixed (earlier-finalized, later-raw) teammate
    // set. `roll_unique_max_hps` performs both passes faithfully; for every
    // non-Decimillipede roster it is byte-identical to the old inline
    // `roll → finalize` loop (same niche reads, identity finalize). Beads
    // `sts2-rs-q6bix` / `8mpo1`.
    let roster_kinds: Vec<MonsterKind> = roster.iter().map(|(k, _)| *k).collect();
    let max_hps =
        sts2_core::monsters::roll_unique_max_hps(&roster_kinds, &mut run_rng.niche, ascension);
    // Spawned below by the run path's `run_bridge::spawn_roster` (position,
    // spawn args, AI, on-spawn powers), once the state exists.
    let roster: Vec<(MonsterKind, MonsterSpawnArgs, i32)> = roster
        .into_iter()
        .zip(max_hps)
        .map(|((kind, args), hp)| (kind, args, hp))
        .collect();

    // Build relics. We push directly into `state.relics` (matching the
    // pattern in `state_dict::state_from_dict_impl`); on-obtain hooks
    // aren't dispatched here, which is fine for the v1 SAFE_RELICS list
    // (`BURNING_BLOOD`'s only effect is a rest-heal hook, no combat
    // perturbation). Expand to `RunManager::force_add_relic` semantics
    // if SAFE_RELICS ever grows to include on-obtain combat effects.
    let mut relics_vec: Vec<sts2_core::Relic> = Vec::with_capacity(relic_ids.len());
    for (idx, name) in relic_ids.iter().enumerate() {
        let kind = state_dict::relic_kind_from_python_name(name)
            .ok_or_else(|| format!("unknown relic id {name:?}"))?;
        let raw = u32::try_from(idx + 1).map_err(|_| "relic index overflows u32".to_string())?;
        let id = std::num::NonZeroU32::new(raw).ok_or_else(|| "relic id non-zero".to_string())?;
        // Seed the combat counter from the relic's canonical fresh-obtain
        // value. Countdown relics (EmberTea/BoneTea/TeaOfDiscourtesy/
        // SwordOfStone) gate their listeners on `counter > 0`; hardcoding
        // `0` here silently disabled them on the only combat the scenario
        // builds — EMBER_TEA dropped its Strength(2), BONE_TEA its hand
        // upgrade (bead `sts2-rs-8s5wf`). Zero-based accumulator relics are
        // unaffected (`initial_counter` returns 0 for them).
        relics_vec.push(sts2_core::Relic {
            id,
            kind,
            counter: kind.initial_counter(),
            is_wax: false,
            is_melted: false,
        });
    }

    let is_elite = matches!(enc.tier, sts2_content::encounters::EncounterTier::Elite);
    let is_boss = matches!(enc.tier, sts2_content::encounters::EncounterTier::Boss);

    // Apply Pantograph's boss-room heal to the player's starting HP.
    // C# fires `Hook.AfterRoomEntered(runState, combatRoom)` on entering a
    // boss room; `Pantograph.AfterRoomEntered` (`Pantograph.cs:22-26`, run-side
    // listener — mirrored in Rust at
    // `sts2-content/src/relics/uncommon.rs::listener_pantograph`) heals 25 HP
    // when `room_type == Boss && current_hp > 0`. That listener operates on a
    // `RunState`, NOT a `CombatState`, so it cannot be reached via the
    // combat-side `relic_effects` bus (an earlier port wrongly dispatched the
    // combat channel here — that both failed to heal AND fired unrelated
    // combat-side boss listeners, regressing non-Pantograph boss fixtures;
    // bead `sts2-rs-nkvht`). Pantograph is the only boss-room-entry run relic
    // with a *combat-observable* effect (the others touch gold/cards/map state
    // the scenario builder does not surface), so we apply it directly here,
    // clamped to `max_hp` exactly like `RunState::heal`.
    let player_hp_final = if is_boss
        && hp > 0
        && relics_vec
            .iter()
            .any(|r| r.kind == sts2_core::RelicKind::Pantograph)
    {
        (hp + sts2_content::relics::uncommon::PANTOGRAPH_HEAL).min(max_hp)
    } else {
        hp
    };

    let mut player = player;
    player.current_hp = player_hp_final;

    let mut state = CoreCombatState {
        belt_value: None,
        phase: CombatPhase::Setup,
        round: 0,
        side: Side::Player,
        rng,
        run_rng,
        creatures: Vec::new(),
        cards: cards_arena.into(),
        powers: Vec::new().into(),
        relics: relics_vec.into(),
        potions: Vec::new(),
        player,
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
        event_combat_timed_out: false,
        // The player's run gold, carried into combat — see the same field in
        // `sts2-cli::sim::scenario::starter`. The C# parity oracle builds its
        // synthetic combats out of a real run, so its player walks in on the
        // character's starting gold, and gold is a genuine combat input:
        // GremlinMerc's `ThieveryPower.Steal()` caps at 20 a steal and banks
        // the total into the HeistPower the FatGremlin inherits when the merc
        // dies, and SealOfGold spends 5. At `gold: 0` both paths no-opped, so
        // the harness reported the engine missing a power it computes
        // correctly (GH #544, found by the Underdocks encounter sweep).
        gold: sts2_core::characters::character_starter(character_id).gold,
        is_over: false,
        player_won: false,
        ascension,
        master_deck: Vec::new(),
        last_added_card: None,
        is_elite,
        is_boss,
        act_index: 1,
        active_attack: None,
        energy_wasted: 0,
        cards_drawn: 0,
        pending_run_gold_loss: 0,
        last_drawn_card: None,
    };

    // Monsters 1..n first, then the player's mirror creature, then every
    // monster's on-spawn powers (so Rocket's Surrounded lands on the player,
    // sts2-rs-q6bix): the order `spawn_roster` shares with the run path and
    // the `sts2-cli` scenario builder.
    sts2_content::run_bridge::spawn_roster(&mut state, &roster);

    // GH #552: stamp the encounter's slot model, so a mid-combat summon lands
    // in the turn position C# gives it. Inert for the ~80% of encounters that
    // declare no `Slots`, and a pure no-op for the initial roster either way
    // (every shipped `GenerateMonsters` is already slot-ascending). Must come
    // after the roster is in `enemy_uids` and before anything can summon.
    state.assign_encounter_slots(enc.member_slots());

    // Parity coverage (2026-05-29): the player's mirror Creature (the
    // canonical d03 path the real engine uses) is initialized by
    // `start_combat` itself (idempotent guard at combat_loop.rs:138,
    // fired before the `before_combat_start` relic listeners at step 4).
    // Relic combat-start grants (BronzeScales Thorns, Vajra Strength,
    // OddlySmoothStone Dexterity, …) call
    // `apply_power_to_creature(player.creature_uid, …)`; what previously
    // blocked them here was the *relic dispatch registry* not being
    // installed (only the card registry was) — see the
    // `relics::dispatch::install_default_registry()` call above. No
    // explicit `init_player_creature` is needed; `start_combat` does it.
    start_combat(&mut state);
    start_player_turn(&mut state);
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::starter_args_for_move_state;
    use sts2_core::MonsterKind;

    #[test]
    fn starter_args_for_move_state_maps_scroll_states() {
        // sts2-rs-u3wjj: the inverse of `create_scroll_of_biting_ai`'s
        // `StarterMoveIdx % 3` selection. Keep in lock-step.
        use sts2_content::monster_dispatch::MonsterSpawnArgs as A;
        assert!(matches!(
            starter_args_for_move_state(MonsterKind::ScrollOfBiting, "CHOMP"),
            Some(A::ScrollOfBiting { starter_idx: 0 })
        ));
        assert!(matches!(
            starter_args_for_move_state(MonsterKind::ScrollOfBiting, "CHEW"),
            Some(A::ScrollOfBiting { starter_idx: 1 })
        ));
        assert!(matches!(
            starter_args_for_move_state(MonsterKind::ScrollOfBiting, "MORE_TEETH"),
            Some(A::ScrollOfBiting { starter_idx: 2 })
        ));
        // Unknown state / non-rotating kind → no override.
        assert!(starter_args_for_move_state(MonsterKind::ScrollOfBiting, "WAT").is_none());
        assert!(starter_args_for_move_state(MonsterKind::Nibbit, "CHOMP").is_none());
    }
}
