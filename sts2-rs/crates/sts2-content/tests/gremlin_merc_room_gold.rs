//! The Gremlin Merc fight's room gold.
//!
//! The game's `GremlinMercNormal.CalculateGoldProportion` replaces the base
//! `1 - escaped / spawned`: full gold if no Fat Gremlin escaped, half if one
//! escaped but no gold was stolen, none if it escaped with stolen gold.
//! `GoldWasStolen` is set by `SurprisePower.AfterDeath` (the Merc dies, spawns
//! the Sneaky and Fat Gremlins and moves its Thievery gold onto the Fat one as
//! Heist) when that gold is above 0. At proportion 0 the room offers no gold
//! reward (and rolls none).

use sts2_core::potions::CharacterId;
use sts2_core::runstate::{RoomType, RunState};
use sts2_core::{apply, Action, CombatPhase, CombatState, MonsterKind, Power, PowerUid};

fn install() {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
    sts2_core::register_card_factory(sts2_content::cards::spec_index::card_pod_from_id);
}

/// The Merc has stolen `stolen` gold (its Thievery counter), then dies; the
/// Fat Gremlin it leaves flees on its second turn.
fn merc_fight_after_the_fat_gremlin_flees(stolen: i32) -> CombatState {
    install();
    let mut state = CombatState {
        rng: sts2_core::dotnet_rng::Rng::new(7),
        run_rng: sts2_core::run_rng_set::RunRngSet::new(7),
        ..Default::default()
    };
    state.player.character = Some(CharacterId::Ironclad);
    state.player.current_hp = 70;
    state.player.max_hp = 70;
    state.player.max_energy = 3;
    state.init_player_creature();
    let merc = state.spawn_enemy(MonsterKind::GremlinMerc);
    let pid = PowerUid::new(90).expect("nz");
    let mut thievery = Power::new(pid.0, sts2_core::PowerKind::Thievery, 20);
    thievery.counter = stolen;
    state.powers.push(thievery);
    if let Some(c) = state.creatures.iter_mut().find(|c| c.id == merc.0) {
        c.powers.push(pid);
        c.current_hp = 0;
    }
    state.phase = CombatPhase::PlayerPlayPhase;
    sts2_content::monsters::act1_underdocks::surprise_after_death_spawn(&mut state, merc);
    // Turn 1: the Fat Gremlin is stunned (SPAWNED); turn 2: it flees.
    for _ in 0..2 {
        let _ = apply(&mut state, Action::EndTurn);
    }
    assert_eq!(state.escaped_creatures, 1, "the Fat Gremlin fled");
    state
}

#[test]
fn the_fat_gremlin_fleeing_with_stolen_gold_leaves_no_room_gold() {
    let combat = merc_fight_after_the_fat_gremlin_flees(15);
    let mut rs = RunState::default();
    sts2_content::run_bridge::exit_combat(&mut rs, &combat);
    assert!(
        rs.last_combat_gold_proportion.abs() < 1e-9,
        "proportion 0, got {}",
        rs.last_combat_gold_proportion
    );
    assert_eq!(
        rs.roll_encounter_gold(RoomType::Monster, rs.last_combat_gold_proportion),
        None,
        "no gold reward"
    );
}

#[test]
fn the_fat_gremlin_fleeing_without_stolen_gold_halves_the_room_gold() {
    let combat = merc_fight_after_the_fat_gremlin_flees(0);
    let mut rs = RunState::default();
    sts2_content::run_bridge::exit_combat(&mut rs, &combat);
    assert!(
        (rs.last_combat_gold_proportion - 0.5).abs() < 1e-9,
        "proportion 0.5, got {}",
        rs.last_combat_gold_proportion
    );
}
