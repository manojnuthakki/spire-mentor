//! Galvanic lifecycle controls from the installed DLL (F6 step12).
//! Fixtures may seed an occupied affliction slot directly; that establishes
//! selective behavior, not a natural acquisition path or measured frequency.

use smallvec::smallvec;
use sts2_core::card_afflictions::{CardAffliction, CardAfflictionKind};
use sts2_core::card_effects::{self, spawn_card_into_hand, spawn_card_into_pile};
use sts2_core::cards::{self, Card, CardId, CardType, KeywordOverride, PileKind, TargetType};
use sts2_core::combat_loop;
use sts2_core::hooks::{self, CardCtx};
use sts2_core::{
    CombatPhase, CombatState, Creature, CreatureUid, IntentHistory, MonsterId, MonsterKind,
    PowerKind,
};

fn board() -> (CombatState, CreatureUid) {
    card_effects::ensure_installed();
    let mut state = CombatState::default();
    state.player.current_hp = 80;
    state.player.max_hp = 80;
    state.player.energy = 10;
    state.phase = CombatPhase::PlayerPlayPhase;
    state.init_player_creature();
    let owner = CreatureUid::new(2).unwrap();
    state.creatures.push(Creature {
        id: owner.0,
        current_hp: 80,
        max_hp: 80,
        block: 0,
        powers: smallvec![],
        species: Some(MonsterKind::GlobeHead),
        intent: None,
        intent_history: IntentHistory::default(),
        monster_id: MonsterId("GlobeHead"),
        ai: None,
    });
    state.enemy_uids.push(owner);
    card_effects::apply_power_to_creature(&mut state, owner, PowerKind::Galvanic, 6);
    (state, owner)
}

fn power_card() -> Card {
    Card {
        card_id: CardId::Inflame,
        card_type: CardType::Power,
        target_type: TargetType::Self_,
        cost: 0,
        ..Card::pod_defaults()
    }
}

fn seeded_card(state: &mut CombatState, card: Card, pile: PileKind) -> sts2_core::CardUid {
    let uid = sts2_core::CardUid::new(u32::try_from(state.cards.len() + 1).unwrap()).unwrap();
    state.cards.push(Card { uid, ..card });
    match pile {
        PileKind::Hand => state.player.hand.push(uid),
        PileKind::Draw => state.player.draw.push(uid),
        PileKind::Discard => state.player.discard.push(uid),
        PileKind::Exhaust => state.player.exhaust.push(uid),
        PileKind::Play => state.player.play.push(uid),
    }
    uid
}

fn mark(state: &CombatState, uid: sts2_core::CardUid) -> Option<CardAffliction> {
    state.cards[uid.get() as usize - 1].affliction
}

fn ctx(state: &CombatState, uid: sts2_core::CardUid) -> CardCtx {
    let card = state.cards[uid.get() as usize - 1];
    CardCtx {
        card_id: Some(card.card_id),
        card_uid: Some(uid),
        card_tags: card.tags,
        card_type: Some(card.card_type),
    }
}

#[test]
fn galvanic_start_covers_all_five_piles_and_preserves_native_keywords() {
    let (mut state, _) = board();
    let mut uids = Vec::new();
    for pile in [
        PileKind::Hand,
        PileKind::Draw,
        PileKind::Discard,
        PileKind::Exhaust,
        PileKind::Play,
    ] {
        uids.push(seeded_card(
            &mut state,
            Card {
                keyword_override: Some(KeywordOverride::ETHEREAL),
                ..power_card()
            },
            pile,
        ));
    }
    combat_loop::start_combat(&mut state);
    for uid in uids {
        assert_eq!(
            mark(&state, uid),
            Some(CardAffliction::new(CardAfflictionKind::Galvanized, 6))
        );
        assert_eq!(
            state.cards[uid.get() as usize - 1].keyword_override,
            Some(KeywordOverride::ETHEREAL)
        );
    }
}

#[test]
fn galvanic_start_stacks_same_marker_but_preserves_competing_affliction() {
    let (mut state, _) = board();
    let same = seeded_card(
        &mut state,
        Card {
            affliction: Some(CardAffliction::new(CardAfflictionKind::Galvanized, 3)),
            ..power_card()
        },
        PileKind::Draw,
    );
    let other = seeded_card(
        &mut state,
        Card {
            affliction: Some(CardAffliction::new(CardAfflictionKind::Weighted, 2)),
            ..power_card()
        },
        PileKind::Hand,
    );
    combat_loop::start_combat(&mut state);
    assert_eq!(mark(&state, same).unwrap().stacks, 9);
    assert_eq!(
        mark(&state, other),
        Some(CardAffliction::new(CardAfflictionKind::Weighted, 2))
    );
}

#[test]
fn galvanic_entry_covers_hand_overflow_and_exhaust_without_restacking_clones() {
    let (mut state, _) = board();
    for _ in 0..cards::HAND_LIMIT {
        seeded_card(&mut state, Card::pod_defaults(), PileKind::Hand);
    }
    let overflow = spawn_card_into_hand(&mut state, power_card());
    assert!(state.player.discard.contains(&overflow));
    assert_eq!(mark(&state, overflow).unwrap().stacks, 6);
    let exhausted = spawn_card_into_pile(&mut state, power_card(), card_effects::PileKind::Exhaust);
    assert_eq!(mark(&state, exhausted).unwrap().stacks, 6);
    let template = Card {
        affliction: Some(CardAffliction::new(CardAfflictionKind::Galvanized, 3)),
        ..power_card()
    };
    let clone = spawn_card_into_hand(&mut state, template);
    assert_eq!(mark(&state, clone).unwrap().stacks, 3);
}

#[test]
fn galvanic_transform_into_exhaust_marks_the_fresh_power() {
    let (mut state, _) = board();
    let old = seeded_card(&mut state, Card::pod_defaults(), PileKind::Exhaust);
    let new = state.transform_card(old, power_card()).unwrap();
    assert_eq!(
        mark(&state, new),
        Some(CardAffliction::new(CardAfflictionKind::Galvanized, 6))
    );
}

#[test]
fn galvanic_selects_actual_marker_and_current_power_amount() {
    let (mut state, owner) = board();
    // Privileged marked Skill deliberately falsifies the old type equivalence.
    let uid = seeded_card(
        &mut state,
        Card {
            card_type: CardType::Skill,
            affliction: Some(CardAffliction::new(CardAfflictionKind::Galvanized, 2)),
            ..power_card()
        },
        PileKind::Play,
    );
    card_effects::apply_power_to_creature(&mut state, owner, PowerKind::Galvanic, 3);
    let event = ctx(&state, uid);
    hooks::dispatch_after_card_played(&mut state, &event);
    assert_eq!(state.player.current_hp, 71);
    assert_eq!(mark(&state, uid).unwrap().stacks, 2);
}

#[test]
fn galvanic_unmarked_or_other_afflicted_power_and_missing_uid_do_not_damage() {
    let (mut state, _) = board();
    for affliction in [
        None,
        Some(CardAffliction::new(CardAfflictionKind::Weighted, 2)),
    ] {
        let uid = seeded_card(
            &mut state,
            Card {
                affliction,
                ..power_card()
            },
            PileKind::Play,
        );
        let event = ctx(&state, uid);
        hooks::dispatch_after_card_played(&mut state, &event);
        assert_eq!(state.player.current_hp, 80);
    }
    hooks::dispatch_after_card_played(
        &mut state,
        &CardCtx {
            card_type: Some(CardType::Power),
            ..CardCtx::default()
        },
    );
    assert_eq!(state.player.current_hp, 80);
}

#[test]
fn galvanic_missing_uid_never_uses_a_card_type_fallback() {
    let (mut state, _) = board();
    hooks::dispatch_after_card_played(
        &mut state,
        &CardCtx {
            card_type: Some(CardType::Power),
            ..CardCtx::default()
        },
    );
    assert_eq!(state.player.current_hp, 80);
}

#[test]
fn galvanic_marked_damage_uses_block_and_each_owner_current_amount() {
    let (mut state, owner) = board();
    let second = CreatureUid::new(3).unwrap();
    let mut other = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .unwrap()
        .clone();
    other.id = second.0;
    other.powers.clear();
    state.creatures.push(other);
    state.enemy_uids.push(second);
    card_effects::apply_power_to_creature(&mut state, second, PowerKind::Galvanic, 4);
    let uid = seeded_card(
        &mut state,
        Card {
            affliction: Some(CardAffliction::new(CardAfflictionKind::Galvanized, 99)),
            ..power_card()
        },
        PileKind::Play,
    );
    state.player.block = 7;
    let event = ctx(&state, uid);
    hooks::dispatch_after_card_played(&mut state, &event);
    assert_eq!(state.player.block, 0);
    assert_eq!(state.player.current_hp, 77);
    assert_eq!(mark(&state, uid).unwrap().stacks, 99);
}

#[test]
fn galvanic_dead_removed_owner_keeps_marker_but_cannot_damage_later() {
    let (mut state, owner) = board();
    let uid = seeded_card(
        &mut state,
        Card {
            affliction: Some(CardAffliction::new(CardAfflictionKind::Galvanized, 2)),
            ..power_card()
        },
        PileKind::Play,
    );
    state
        .creatures
        .iter_mut()
        .find(|c| c.id == owner.0)
        .unwrap()
        .current_hp = 0;
    assert!(state.is_removed_corpse(owner));
    let event = ctx(&state, uid);
    hooks::dispatch_after_card_played(&mut state, &event);
    assert_eq!(state.player.current_hp, 80);
    assert_eq!(mark(&state, uid).unwrap().stacks, 2);
}

#[test]
fn galvanic_draw_and_move_do_not_restack_existing_marker() {
    let (mut state, _) = board();
    let uid = seeded_card(
        &mut state,
        Card {
            affliction: Some(CardAffliction::new(CardAfflictionKind::Galvanized, 2)),
            ..power_card()
        },
        PileKind::Draw,
    );
    state.draw_into_hand(1);
    assert!(state.player.hand.contains(&uid));
    cards::move_card(&mut state.player, uid, PileKind::Hand, PileKind::Discard).unwrap();
    state.move_card_to_hand(uid);
    assert_eq!(mark(&state, uid).unwrap().stacks, 2);
}

#[test]
fn galvanic_manual_and_auto_power_repeats_read_stored_mark_before_removal() {
    for autoplay in [false, true] {
        let (mut state, _) = board();
        let uid = seeded_card(
            &mut state,
            Card {
                base_replay_count: 1,
                affliction: Some(CardAffliction::new(CardAfflictionKind::Galvanized, 2)),
                ..power_card()
            },
            if autoplay {
                PileKind::Draw
            } else {
                PileKind::Hand
            },
        );
        if autoplay {
            assert!(combat_loop::auto_play_specific_card(&mut state, uid, false));
        } else {
            combat_loop::play_card(&mut state, 0, None);
        }
        assert_eq!(state.player.current_hp, 68, "autoplay={autoplay}");
        assert!(!state.player.play.contains(&uid));
        assert_eq!(mark(&state, uid).unwrap().stacks, 2);
    }
}

#[test]
fn galvanic_explicit_removal_preserves_marks_and_stops_future_damage() {
    let (mut state, owner) = board();
    let uid = seeded_card(
        &mut state,
        Card {
            affliction: Some(CardAffliction::new(CardAfflictionKind::Galvanized, 2)),
            ..power_card()
        },
        PileKind::Play,
    );
    card_effects::remove_creature_power(&mut state, owner, PowerKind::Galvanic);
    let event = ctx(&state, uid);
    hooks::dispatch_after_card_played(&mut state, &event);
    assert_eq!(state.player.current_hp, 80);
    assert_eq!(mark(&state, uid).unwrap().stacks, 2);
}

#[test]
fn galvanic_entry_preserves_hex_parent_and_clears_stale_hex_before_enemy_listener() {
    for pile in [
        card_effects::PileKind::Hand,
        card_effects::PileKind::Exhaust,
    ] {
        let (mut state, _) = board();
        let player = state.player.creature_uid.unwrap();
        card_effects::apply_power_to_creature(&mut state, player, PowerKind::Hex, 3);
        let uid = spawn_card_into_pile(&mut state, power_card(), pile);
        assert_eq!(
            mark(&state, uid),
            Some(CardAffliction::new(CardAfflictionKind::Hexed, 3))
        );
        let event = ctx(&state, uid);
        hooks::dispatch_after_card_played(&mut state, &event);
        assert_eq!(state.player.current_hp, 80);
    }
    let (mut state, _) = board();
    let stale = Card {
        affliction: Some(CardAffliction::new(CardAfflictionKind::Hexed, 3)),
        ..power_card()
    };
    let uid = spawn_card_into_hand(&mut state, stale);
    assert_eq!(
        mark(&state, uid),
        Some(CardAffliction::new(CardAfflictionKind::Galvanized, 6))
    );
}

#[test]
fn galvanic_retained_dead_owner_still_enters_and_runs_one_callback() {
    // Privileged retained owner tests the existing attachment projection;
    // this is not a claim that Globe Head naturally obtains Illusion.
    let (mut state, owner) = board();
    card_effects::apply_power_to_creature(&mut state, owner, PowerKind::Illusion, 1);
    state
        .creatures
        .iter_mut()
        .find(|c| c.id == owner.0)
        .unwrap()
        .current_hp = 0;
    assert!(!state.is_removed_corpse(owner));
    let uid = spawn_card_into_hand(&mut state, power_card());
    assert_eq!(mark(&state, uid).unwrap().stacks, 6);
    let event = ctx(&state, uid);
    hooks::dispatch_after_card_played(&mut state, &event);
    assert_eq!(state.player.current_hp, 74);
}

#[test]
fn galvanic_inferno_finishes_started_callback_and_skips_later_detached_owner() {
    let (mut state, owner) = board();
    let second = CreatureUid::new(3).unwrap();
    let mut other = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .unwrap()
        .clone();
    other.id = second.0;
    other.current_hp = 2;
    other.powers.clear();
    state.creatures.push(other);
    state.enemy_uids.push(second);
    state
        .creatures
        .iter_mut()
        .find(|c| c.id == owner.0)
        .unwrap()
        .current_hp = 2;
    card_effects::apply_power_to_creature(&mut state, second, PowerKind::Galvanic, 4);
    let player = state.player.creature_uid.unwrap();
    card_effects::apply_power_to_creature(&mut state, player, PowerKind::Inferno, 3);
    let uid = seeded_card(
        &mut state,
        Card {
            affliction: Some(CardAffliction::new(CardAfflictionKind::Galvanized, 2)),
            ..power_card()
        },
        PileKind::Play,
    );
    let event = ctx(&state, uid);
    hooks::dispatch_after_card_played(&mut state, &event);
    assert!(state.is_removed_corpse(owner));
    assert!(state.is_removed_corpse(second));
    assert_eq!(state.player.current_hp, 74);
    assert_eq!(mark(&state, uid).unwrap().stacks, 2);
}

#[test]
fn galvanic_start_stacks_both_owners_and_entry_uses_enemy_roster_order() {
    let (mut state, owner) = board();
    let second = CreatureUid::new(3).unwrap();
    let mut other = state
        .creatures
        .iter()
        .find(|c| c.id == owner.0)
        .unwrap()
        .clone();
    other.id = second.0;
    other.powers.clear();
    state.creatures.push(other);
    state.enemy_uids.insert(0, second);
    card_effects::apply_power_to_creature(&mut state, second, PowerKind::Galvanic, 4);
    let before_start = seeded_card(&mut state, power_card(), PileKind::Draw);
    combat_loop::start_combat(&mut state);
    assert_eq!(mark(&state, before_start).unwrap().stacks, 10);
    let hand = spawn_card_into_hand(&mut state, power_card());
    let exhaust = spawn_card_into_pile(&mut state, power_card(), card_effects::PileKind::Exhaust);
    assert_eq!(mark(&state, hand).unwrap().stacks, 4);
    assert_eq!(mark(&state, exhaust).unwrap().stacks, 4);
}

#[test]
fn galvanic_terminal_entry_skips_affliction_without_repairing_general_insertion() {
    let (mut state, _) = board();
    state.is_over = true;
    let uid = spawn_card_into_hand(&mut state, power_card());
    // The shared insertion helper still inserts after terminal; that general
    // engine gap is separate. Only Galvanic's affliction application is scoped.
    assert!(state.player.hand.contains(&uid));
    assert_eq!(mark(&state, uid), None);
}

#[test]
fn galvanic_captured_power_removal_does_not_cancel_attached_owner_callback() {
    fn remove_galvanic_on_entry(
        state: &mut CombatState,
        owner: CreatureUid,
        _: sts2_core::CardUid,
    ) -> card_effects::EffectResult {
        card_effects::remove_creature_power(state, owner, PowerKind::Galvanic);
        card_effects::EffectResult::Resolved
    }
    let (mut state, owner) = board();
    // A deliberately registered earlier callback isolates capture semantics;
    // it is not a claim that Withering Presence naturally removes Galvanic.
    sts2_core::monster_power_effects::register_after_card_entered_combat_for_power(
        PowerKind::WitheringPresence,
        remove_galvanic_on_entry,
    );
    card_effects::apply_power_to_creature(&mut state, owner, PowerKind::WitheringPresence, 1);
    let uid = seeded_card(
        &mut state,
        Card {
            affliction: Some(CardAffliction::new(CardAfflictionKind::Galvanized, 2)),
            ..power_card()
        },
        PileKind::Play,
    );
    let event = ctx(&state, uid);
    hooks::dispatch_after_card_played(&mut state, &event);
    let creature = state.creatures.iter().find(|c| c.id == owner.0).unwrap();
    assert!(creature.powers.iter().all(|uid| state
        .powers
        .get_by_id(uid.0)
        .is_none_or(|p| p.kind != PowerKind::Galvanic)));
    assert!(!state.is_removed_corpse(owner));
    assert_eq!(state.player.current_hp, 74);
}
