//! F6 step35 (review F2): the damage preview stages the card as if
//! `PenNib.BeforeCardPlayed` had run, so it equals the damage the play deals
//! at the ninth, tenth and eleventh attack.
use std::num::NonZeroU32;
use sts2_content::cards::spec_index::card_pod_from_id;
use sts2_core::cards::CardId;
use sts2_core::combat_loop::play_card;
use sts2_core::{CardUid, CombatPhase, CombatState, MonsterKind, Relic, RelicKind};

fn board(counter: i32) -> (CombatState, CardUid) {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut s = CombatState::default();
    s.player.current_hp = 80;
    s.player.max_hp = 80;
    s.player.energy = 9;
    s.init_player_creature();
    let e = s.spawn_enemy(MonsterKind::Nibbit);
    let enemy = s.creatures.iter_mut().find(|c| c.id == e.0).unwrap();
    enemy.current_hp = 500;
    enemy.max_hp = 500;
    s.phase = CombatPhase::PlayerPlayPhase;
    let mut r = Relic::new(NonZeroU32::new(900).unwrap(), RelicKind::PenNib);
    r.counter = counter;
    s.relics.push(r);
    let mut c = card_pod_from_id(CardId::StrikeIronclad, false).unwrap();
    c.uid = CardUid::new(1).unwrap();
    s.cards.push(c);
    s.player.hand.push(c.uid);
    (s, c.uid)
}

#[test]
fn the_preview_matches_the_hit_at_the_ninth_tenth_and_eleventh_attack() {
    for (counter, want) in [(8, 6), (9, 12), (10, 6)] {
        let (mut s, _) = board(counter);
        let preview = sts2_wire::emit::combat::card_damage_preview(&s)[0][&0];
        let before = s
            .creatures
            .iter()
            .find(|c| c.id == s.enemy_uids[0].0)
            .unwrap()
            .current_hp;
        let _ = play_card(&mut s, 0, Some(0));
        let after = s
            .creatures
            .iter()
            .find(|c| c.id == s.enemy_uids[0].0)
            .unwrap()
            .current_hp;
        assert_eq!(before - after, want, "the hit at count {counter}");
        assert_eq!(preview, want, "the preview at count {counter}");
    }
}
