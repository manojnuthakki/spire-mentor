//! The combat's Fur Coat mark survives the wire (sts2-agent fork,
//! `fur-coat-midfight-summons`): a state rebuilt from the engine's own wire
//! still clamps an enemy added mid-fight to hp 1, as the original does.

use sts2_core::{CombatState, MonsterKind};

fn summon_hp(state: &mut CombatState) -> i32 {
    let uid = state.spawn_enemy(MonsterKind::FuzzyWurmCrawler);
    state
        .creatures
        .iter()
        .find(|c| c.id == uid.0)
        .expect("summon")
        .current_hp
}

fn base() -> CombatState {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut s = CombatState::default();
    s.player.current_hp = 80;
    s.player.max_hp = 80;
    s.init_player_creature();
    let _ = s.spawn_enemy(MonsterKind::FuzzyWurmCrawler);
    s
}

#[test]
fn a_marked_combat_clamps_a_summon_after_a_wire_round_trip() {
    let mut original = base();
    original.fur_coat_marked_room = true;
    let wire = sts2_wire::emit::combat_state_to_value(&original).expect("emits");
    assert_eq!(wire["fur_coat_marked_room"], serde_json::json!(true));
    let mut restored = sts2_wire::state_from_json_str(&wire.to_string()).expect("rebuilds");
    assert!(restored.fur_coat_marked_room);
    assert_eq!(summon_hp(&mut original), 1);
    assert_eq!(summon_hp(&mut restored), 1);
}

#[test]
fn an_unmarked_combat_writes_no_key_and_keeps_a_summons_hp() {
    let original = base();
    let wire = sts2_wire::emit::combat_state_to_value(&original).expect("emits");
    assert!(wire.get("fur_coat_marked_room").is_none(), "{wire}");
    let mut restored = sts2_wire::state_from_json_str(&wire.to_string()).expect("rebuilds");
    assert!(!restored.fur_coat_marked_room);
    assert!(summon_hp(&mut restored) > 1);
}
