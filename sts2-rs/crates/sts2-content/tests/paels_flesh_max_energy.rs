//! Pael's Flesh raises max energy from the player's third turn on.
//!
//! The game's `PaelsFlesh.ModifyMaxEnergy` adds its 1 energy to the owner's
//! max energy once the player's turn number is 3 or more; the turn start's
//! energy reset fills energy to that max. It grants no energy on top of the
//! reset, so on turn 3 a three-energy character has max energy 4 and 4 energy
//! (not 3 max and 5 energy, as a separate turn-start gain would give).

use std::num::NonZeroU32;

use sts2_core::combat_loop::{effective_max_energy, start_player_turn};
use sts2_core::{CombatPhase, CombatState, Relic, RelicKind};

use sts2_content::cards::dispatch as content_dispatch;

fn state_with_paels_flesh(round: u16) -> CombatState {
    let mut state = CombatState {
        rng: sts2_core::dotnet_rng::Rng::new(0_i32),
        ..Default::default()
    };
    state.player.current_hp = 80;
    state.player.max_hp = 80;
    state.player.energy = 0;
    let _ = state.init_player_creature();
    state.sync_player_creature();
    state.relics.push(Relic {
        id: NonZeroU32::new(1).expect("non-zero"),
        kind: RelicKind::PaelsFlesh,
        counter: 0,
        is_wax: false,
        is_melted: false,
    });
    state.phase = CombatPhase::PlayerTurnStart;
    state.round = round;
    state
}

#[test]
fn paels_flesh_adds_max_energy_from_the_third_turn() {
    content_dispatch::install_default_registry();
    for round in [1_u16, 2] {
        let mut state = state_with_paels_flesh(round);
        let _ = start_player_turn(&mut state);
        assert_eq!(effective_max_energy(&state), 3, "round {round}: max energy");
        assert_eq!(state.player.max_energy, 3, "round {round}: max energy");
        assert_eq!(state.player.energy, 3, "round {round}: energy");
    }
    let mut state = state_with_paels_flesh(3);
    let _ = start_player_turn(&mut state);
    assert_eq!(effective_max_energy(&state), 4, "round 3: max energy");
    assert_eq!(state.player.max_energy, 4, "round 3: max energy");
    assert_eq!(
        state.player.energy, 4,
        "round 3: energy is the max, not max + 1"
    );
}
