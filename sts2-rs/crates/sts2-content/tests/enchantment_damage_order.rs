//! v0.107.1 Hook.ModifyDamage applies enchantments before ordinary listeners.
//! Authority: MegaCrit.Sts2.Core.Hooks/Hook.cs, DamageVar.cs,
//! CalculatedDamageVar.cs, StrikeDummy.cs and MiniatureCannon.cs.
use std::num::NonZeroU32;
use sts2_content::cards::spec_index::card_pod_from_id;
use sts2_core::cards::{CardId, EnchantmentId};
use sts2_core::hooks::{CreatureRef, DamageCtx, ValueProps};
use sts2_core::{
    CardUid, CombatState, CreatureUid, MonsterKind, Power, PowerKind, PowerUid, Relic, RelicKind,
};

fn board(id: CardId, up: bool, enchant: EnchantmentId) -> (CombatState, CreatureUid) {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut s = CombatState::default();
    s.player.current_hp = 80;
    s.player.max_hp = 80;
    s.init_player_creature();
    let target = s.spawn_enemy(MonsterKind::Nibbit);
    let enemy = s.creatures.iter_mut().find(|c| c.id == target.0).unwrap();
    enemy.current_hp = 500;
    enemy.max_hp = 500;
    let mut card = card_pod_from_id(id, up).unwrap();
    card.uid = CardUid::new(1).unwrap();
    card.set_enchantment(enchant, 1);
    s.cards.push(card);
    s.player.play.push(card.uid);
    (s, target)
}

fn power(s: &mut CombatState, owner: CreatureRef, kind: PowerKind, amount: i32) {
    let uid = PowerUid::new(s.powers.len() as u32 + 1).unwrap();
    s.powers.push(Power {
        id: uid.0,
        kind,
        amount,
        counter: 0,
        applier: None,
        skip_next_tick: false,
    });
    match owner {
        CreatureRef::Player => s.player.powers.push(uid),
        CreatureRef::Enemy(e) => s
            .creatures
            .iter_mut()
            .find(|c| c.id == e.0)
            .unwrap()
            .powers
            .push(uid),
        _ => unreachable!(),
    }
}

fn relic(s: &mut CombatState, kind: RelicKind) {
    s.relics.push(Relic {
        id: NonZeroU32::new(s.relics.len() as u32 + 1).unwrap(),
        kind,
        counter: 0,
        is_melted: false,
        is_wax: false,
    });
}

#[test]
fn enchant_order_precedes_strength_weak_vulnerable_and_vigor_identity() {
    for enchant in [
        EnchantmentId::Corrupted,
        EnchantmentId::Instinct,
        EnchantmentId::Favored,
    ] {
        let (mut s, target) = board(CardId::Anger, false, enchant);
        let ctx = DamageCtx {
            dealer: CreatureRef::Player,
            target: CreatureRef::Enemy(target),
            props: ValueProps::MOVE,
        };
        let enchanted = if enchant == EnchantmentId::Corrupted {
            9
        } else {
            12
        };
        assert_eq!(sts2_core::hooks::modify_damage(&s, 6, &ctx), enchanted);
        power(&mut s, CreatureRef::Player, PowerKind::Strength, 2);
        assert_eq!(sts2_core::hooks::modify_damage(&s, 6, &ctx), enchanted + 2);
        power(&mut s, CreatureRef::Player, PowerKind::Weak, 1);
        power(&mut s, CreatureRef::Enemy(target), PowerKind::Vulnerable, 1);
        power(&mut s, CreatureRef::Player, PowerKind::Vulnerable, 1);
        assert_eq!(
            sts2_core::hooks::modify_damage(&s, 6, &ctx),
            (enchanted + 2) * 9 / 8
        );
        let null = DamageCtx {
            target: CreatureRef::Nobody,
            ..ctx
        };
        assert_eq!(
            sts2_core::hooks::modify_damage(&s, 6, &null),
            (enchanted + 2) * 3 / 4
        );
        power(&mut s, CreatureRef::Player, PowerKind::Vigor, 4);
        s.attack_commands.vigor = Some(sts2_core::attack_command::AttackPowerLatch {
            command: 0,
            source: Some(CardUid::new(2).unwrap()),
            amount_at_start: 4,
        });
        assert_eq!(
            sts2_core::hooks::modify_damage(&s, 6, &null),
            (enchanted + 2) * 3 / 4
        );
        s.attack_commands.vigor = Some(sts2_core::attack_command::AttackPowerLatch {
            command: 0,
            source: Some(s.cards[0].uid),
            amount_at_start: 4,
        });
        assert_eq!(
            sts2_core::hooks::modify_damage(&s, 6, &null),
            (enchanted + 6) * 3 / 4
        );
        let unpowered = DamageCtx {
            props: ValueProps::MOVE.union(ValueProps::UNPOWERED),
            ..ctx
        };
        assert_eq!(sts2_core::hooks::modify_damage(&s, 6, &unpowered), 6);
    }
}

#[test]
fn enchant_order_keeps_fraction_until_after_ordinary_additions_and_caps() {
    let (mut s, target) = board(CardId::Thrash, false, EnchantmentId::Corrupted);
    power(&mut s, CreatureRef::Player, PowerKind::Strength, 1);
    power(&mut s, CreatureRef::Player, PowerKind::Weak, 1);
    power(&mut s, CreatureRef::Enemy(target), PowerKind::Vulnerable, 1);
    // ((6.5 * 1.5) + 1) * .75 * 1.5 = 12.09375, one final floor.
    assert_eq!(
        sts2_core::damage::compute_modified_damage_frac(
            &s,
            CreatureRef::Player,
            target,
            13,
            2,
            ValueProps::MOVE,
            Some(CardId::Thrash)
        ),
        12
    );
    power(&mut s, CreatureRef::Enemy(target), PowerKind::Intangible, 1);
    assert_eq!(
        sts2_core::damage::compute_modified_damage_frac(
            &s,
            CreatureRef::Player,
            target,
            13,
            2,
            ValueProps::MOVE,
            Some(CardId::Thrash)
        ),
        1
    );
}

#[test]
fn enchant_order_cannon_uses_exact_upgraded_copy_after_enchantment() {
    let (mut s, target) = board(CardId::BodySlam, false, EnchantmentId::Sharp);
    let mut played = card_pod_from_id(CardId::BodySlam, true).unwrap();
    played.uid = CardUid::new(2).unwrap();
    played.set_enchantment(EnchantmentId::Corrupted, 1);
    s.cards.push(played);
    s.player.play[0] = played.uid;
    relic(&mut s, RelicKind::MiniatureCannon);
    // Calculated Body Slam base is block, not its static base_damage.
    s.player.block = 8;
    let before = s
        .creatures
        .iter()
        .find(|c| c.id == target.0)
        .unwrap()
        .current_hp;
    sts2_core::card_effects::dispatch_for_card(&mut s, played.uid, CardId::BodySlam, Some(target));
    assert_eq!(
        before
            - s.creatures
                .iter()
                .find(|c| c.id == target.0)
                .unwrap()
                .current_hp,
        15
    );
    assert_eq!(s.cards[0].enchantment(EnchantmentId::Sharp), 1);
    assert!(!s.cards[0].upgraded);
}

#[test]
fn enchant_order_strike_dummy_and_fractional_registry_bonus_are_post_enchant() {
    let (mut s, target) = board(CardId::TwinStrike, true, EnchantmentId::Corrupted);
    relic(&mut s, RelicKind::StrikeDummy);
    relic(&mut s, RelicKind::MiniatureCannon);
    // 7 * 1.5 + Dummy3 + Cannon3 = 16.5 -> 16, not floor(13 * 1.5).
    assert_eq!(
        sts2_core::damage::compute_modified_damage(
            &s,
            CreatureRef::Player,
            target,
            7,
            ValueProps::MOVE,
            Some(CardId::TwinStrike)
        ),
        16
    );
    assert_eq!(
        sts2_core::damage::compute_modified_damage_frac(
            &s,
            CreatureRef::Player,
            target,
            7,
            1,
            ValueProps::MOVE,
            Some(CardId::TwinStrike)
        ),
        16
    );
}

#[test]
fn enchant_order_cap_flashes_use_unrounded_staged_amount() {
    use sts2_core::event_log::EventLogEntry;
    for (base, cap, flashes) in [(10, 18, 0), (9, 16, 1)] {
        let (mut s, target) = board(CardId::Anger, true, EnchantmentId::Corrupted);
        relic(&mut s, RelicKind::MiniatureCannon);
        power(
            &mut s,
            CreatureRef::Enemy(target),
            PowerKind::HardToKill,
            cap,
        );
        s.combat_log.event_log.set_recording(true);
        let damage = sts2_core::damage::compute_modified_damage_flashing(
            &mut s,
            CreatureRef::Player,
            target,
            base,
            ValueProps::MOVE,
            Some(CardId::Anger),
        );
        assert_eq!(damage, cap);
        let actual = s
            .combat_log
            .event_log
            .rows()
            .iter()
            .filter(|row| {
                matches!(
                    row.entry,
                    EventLogEntry::PowerFlashed {
                        power: PowerKind::HardToKill,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(
            actual, flashes,
            "18 does not exceed cap18; 16.5 DOES exceed cap16 before rounding"
        );
    }
}

#[test]
fn enchant_order_execution_uid_survives_autoplay_replay_and_reflection_controls() {
    let (mut s, target) = board(CardId::Anger, false, EnchantmentId::Sharp);
    let mut enchanted = card_pod_from_id(CardId::Anger, true).unwrap();
    enchanted.uid = CardUid::new(2).unwrap();
    enchanted.set_enchantment(EnchantmentId::Corrupted, 1);
    s.cards.push(enchanted);
    // A suspended outer card remains on Play while the actual source is in
    // discard: dispatch_for_card is the same body boundary used by autoplay
    // and replays. This checks that boundary, not natural offer frequency.
    s.player.discard.push(enchanted.uid);
    power(&mut s, CreatureRef::Player, PowerKind::Strength, 2);
    for _ in 0..2 {
        let before = s
            .creatures
            .iter()
            .find(|c| c.id == target.0)
            .unwrap()
            .current_hp;
        sts2_core::card_effects::dispatch_for_card(
            &mut s,
            enchanted.uid,
            CardId::Anger,
            Some(target),
        );
        assert_eq!(
            before
                - s.creatures
                    .iter()
                    .find(|c| c.id == target.0)
                    .unwrap()
                    .current_hp,
            14
        );
        assert_eq!(
            sts2_core::card_effects::resolving_card_uid(&s),
            Some(s.cards[0].uid)
        );
    }
    let reflect = DamageCtx {
        dealer: CreatureRef::Enemy(target),
        target: CreatureRef::Player,
        props: ValueProps::MOVE.union(ValueProps::UNPOWERED),
    };
    assert_eq!(sts2_core::hooks::modify_damage(&s, 5, &reflect), 5);
    s.player.play.clear();
    let ordinary = DamageCtx {
        dealer: CreatureRef::Player,
        target: CreatureRef::Nobody,
        props: ValueProps::MOVE,
    };
    assert_eq!(sts2_core::hooks::modify_damage(&s, 6, &ordinary), 8);
}
