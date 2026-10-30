//! The next act's Ancient room runs its room-entry hooks.
//!
//! After an act's boss, the game's `RunManager.EnterAct` sets the new act
//! and the player enters its first map point, the Ancient's `EventRoom`:
//! `EnterRoomInternal` runs `Hook.AfterRoomEntered` there, so Maw Bank
//! (`MawBank.AfterRoomEntered`, the base room, nothing bought) pays 12 gold.
//! The engine's act advance (`advance_boss_act`) healed for the Ancient but
//! ran no room-entry hook (39RE6GZJPL, X1WLDAFFQ7).

use sts2_content::run_callbacks::{
    initialize_run_setup, new_run_manager_for_character_with_ascension,
};
use sts2_core::potions::CharacterId;
use sts2_core::relics::RelicId;
use sts2_core::run_manager::{ActionResult, ForcedRoom, Phase, RunAction, RunManager};

fn install() {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    sts2_content::potions::dispatch::ensure_installed();
}

/// Beat act 1's boss, take the card reward, and return the gold the run
/// held on the boss's card reward and on the next act's Ancient choice.
fn gold_across_the_act_advance(maw_bank: bool) -> (i32, i32) {
    let mut rm: RunManager =
        new_run_manager_for_character_with_ascension(7, CharacterId::Ironclad, 0);
    initialize_run_setup(&mut rm);
    for _ in 0..50 {
        if rm.phase() == Phase::MapChoice {
            break;
        }
        let a = rm.get_available_actions()[0];
        rm.take_action(a);
    }
    assert_eq!(rm.phase(), Phase::MapChoice);
    if maw_bank {
        rm.run_state.obtain_relic(RelicId::MawBank);
    }
    assert_eq!(
        rm.force_enter_room(ForcedRoom::Combat("CEREMONIAL_BEAST_BOSS")),
        ActionResult::Advanced
    );
    assert_eq!(rm.phase(), Phase::Combat);
    let _ = rm.resolve_combat_fast(0.0, true);
    for _ in 0..10 {
        if rm.phase() == Phase::CardReward {
            break;
        }
        let a = rm.get_available_actions()[0];
        rm.take_action(a);
    }
    assert_eq!(rm.phase(), Phase::CardReward);
    assert_eq!(rm.run_state.act_index, 1);
    let before = rm.run_state.gold;
    rm.take_action(RunAction::PickCardReward { idx: None });
    assert_eq!(rm.phase(), Phase::BossRelic, "the next act's Ancient");
    assert_eq!(rm.run_state.act_index, 2);
    (before, rm.run_state.gold)
}

#[test]
fn maw_bank_pays_on_entering_the_next_acts_ancient_room() {
    install();
    let (before, after) = gold_across_the_act_advance(true);
    assert_eq!(after - before, 12, "Maw Bank pays at the Ancient");
    let (before, after) = gold_across_the_act_advance(false);
    assert_eq!(after - before, 0, "nothing else pays there");
}
