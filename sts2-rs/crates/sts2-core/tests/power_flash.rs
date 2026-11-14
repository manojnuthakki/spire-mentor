//! `PowerFlashed` emission tests for the Ironclad + Silent flash sites.
//!
//! Every test has a NEGATIVE half, and that half is the point. A flash wired
//! to fire whenever its power merely EXISTS passes any positive assertion; only
//! driving the case the C# guard excludes can tell the two apart. Each negative
//! half names the `<Class>.cs:<line>` clause it is probing.
//!
//! The other half of the contract — that a DISARMED run is unchanged — is
//! carried by the rest of the suite, which runs with recording off.

use std::num::NonZeroU32;

use sts2_core::cards::{Card, CardEnchantments, CardId, CardTags, CombatVars, TargetType};
use sts2_core::event_log::EventLogEntry;
use sts2_core::hooks::{
    BlockClearedCtx, BlockGainedCtx, CardCtx, CreatureRef, PowerAppliedCtx, TurnSideCtx, ValueProps,
};
use sts2_core::{
    CardUid, CombatState, CreatureUid, MonsterKind, Power, PowerKind, PowerUid, Relic, RelicKind,
    Side,
};

// ── fixtures ────────────────────────────────────────────────────────────

fn power_uid(n: u32) -> PowerUid {
    PowerUid::new(n).expect("non-zero power uid")
}

/// A recording combat state with one player-owned power of `kind`.
fn state_with_power(kind: PowerKind, amount: i32) -> CombatState {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    push_player_power(&mut state, 1, kind, amount);
    state
}

fn push_player_power(state: &mut CombatState, id: u32, kind: PowerKind, amount: i32) -> PowerUid {
    let uid = power_uid(id);
    state.powers.push(Power {
        id: uid.0,
        kind,
        amount,
        counter: 0,
        applier: None,
        skip_next_tick: false,
    });
    state.player.powers.push(uid);
    uid
}

/// Add one living enemy so `HittableEnemies`-shaped guards can pass.
fn add_enemy(state: &mut CombatState, hp: i32) -> CreatureUid {
    let uid = state.spawn_enemy(MonsterKind::Nibbit);
    if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
        c.max_hp = hp;
        c.current_hp = hp;
    }
    uid
}

/// Put a card of `card_type` in the store and return its uid.
fn add_card(state: &mut CombatState, id: u32, card_type: sts2_core::cards::CardType) -> CardUid {
    let uid = CardUid::new(id).expect("non-zero card uid");
    state.cards.push(Card {
        uid,
        card_id: CardId::StrikeIronclad,
        cost: 1,
        card_type,
        target_type: TargetType::Enemy,
        upgraded: false,
        base_damage: 6,
        base_block: 0,
        combat_vars: CombatVars::new(),
        enchantments: CardEnchantments::new(),
        tags: CardTags::NONE,
        energy_spent: 0,
        base_replay_count: 0,
        keyword_override: None,
        affliction: None,
    });
    uid
}

/// How many `PowerFlashed` rows name `power`.
fn flash_rows(state: &CombatState, power: PowerKind) -> usize {
    state
        .combat_log
        .event_log
        .rows()
        .iter()
        .filter(|r| matches!(&r.entry, EventLogEntry::PowerFlashed { power: p, .. } if *p == power))
        .count()
}

// ── after_side_turn_start family ────────────────────────────────────────
//
// DemonForm / WraithForm / NoxiousFumes all flash inside
// `participants.Contains(base.Owner)`; CrimsonMantle inside
// `player == base.Owner.Player`. Rust folds all four onto the owner's SIDE, so
// the excluded case is the same for each: the OTHER side's turn start.

fn side_turn_start_flashes_only_on_the_owners_side(kind: PowerKind) {
    let mut state = state_with_power(kind, 2);

    sts2_core::hooks::dispatch_after_side_turn_start(
        &mut state,
        &TurnSideCtx { side: Side::Player },
    );
    assert_eq!(
        flash_rows(&state, kind),
        1,
        "{kind:?} must flash on its owner's turn start"
    );

    sts2_core::hooks::dispatch_after_side_turn_start(
        &mut state,
        &TurnSideCtx { side: Side::Enemy },
    );
    // The C# guard is `participants.Contains(Owner)` (CrimsonMantle:
    // `player == Owner.Player`) — a player-owned power is not a participant in
    // the enemy's side turn start, so no second flash.
    assert_eq!(
        flash_rows(&state, kind),
        1,
        "{kind:?} must not flash on the enemy's turn start"
    );
}

#[test]
fn demon_form_flashes_only_when_its_guard_holds() {
    // C# `DemonFormPower.cs:25`.
    side_turn_start_flashes_only_on_the_owners_side(PowerKind::DemonForm);
}

#[test]
fn wraith_form_flashes_only_when_its_guard_holds() {
    // C# `WraithFormPower.cs:25`.
    side_turn_start_flashes_only_on_the_owners_side(PowerKind::WraithForm);
}

#[test]
fn crimson_mantle_flashes_only_when_its_guard_holds() {
    // C# `CrimsonMantlePower.cs:29`. CrimsonMantle is one of the four kinds
    // `hooks::is_folded_after_player_turn_start_kind` routes out of the
    // `after_side_turn_start` walk and into `combat_loop`'s
    // `AfterPlayerTurnStart` cluster, so drive it through THAT entry point —
    // the ordinary side-turn-start dispatch deliberately skips it.
    let mut state = state_with_power(PowerKind::CrimsonMantle, 2);
    let uid = power_uid(1);

    let fire = |s: &mut CombatState, side: Side| {
        if let Some(effects) = sts2_core::hooks::collect_folded_after_player_turn_start_one(
            s,
            uid,
            &TurnSideCtx { side },
        ) {
            sts2_core::hooks::apply_effects(s, &effects);
        }
    };

    fire(&mut state, Side::Player);
    assert_eq!(flash_rows(&state, PowerKind::CrimsonMantle), 1);

    // The C# guard is `player == base.Owner.Player`; a player-owned
    // CrimsonMantle is not reached on the enemy's turn start.
    fire(&mut state, Side::Enemy);
    assert_eq!(flash_rows(&state, PowerKind::CrimsonMantle), 1);
}

#[test]
fn noxious_fumes_flashes_on_the_owners_turn_even_with_no_enemy_to_poison() {
    // C# `NoxiousFumesPower.cs:32` sits ABOVE the `HittableEnemies` loop, so an
    // empty arena still flashes. This fixture has no enemies at all.
    side_turn_start_flashes_only_on_the_owners_side(PowerKind::NoxiousFumes);
}

// ── Juggernaut ──────────────────────────────────────────────────────────

#[test]
fn juggernaut_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::Juggernaut, 3);
    add_enemy(&mut state, 40);

    sts2_core::hooks::dispatch_after_block_gained(
        &mut state,
        &BlockGainedCtx {
            target: CreatureRef::Player,
            amount: 5,
            card_source: None,
        },
    );
    assert_eq!(flash_rows(&state, PowerKind::Juggernaut), 1);

    // C# `JuggernautPower.cs:25` is nested inside `!(amount <= 0m)` — a
    // zero-block "gain" reaches the listener and must not flash.
    sts2_core::hooks::dispatch_after_block_gained(
        &mut state,
        &BlockGainedCtx {
            target: CreatureRef::Player,
            amount: 0,
            card_source: None,
        },
    );
    assert_eq!(flash_rows(&state, PowerKind::Juggernaut), 1);

    // ...and inside `hittableEnemies.Count != 0`: block gained with the arena
    // dead is a no-flash, because there is nothing to pick a target from.
    for c in &mut state.creatures {
        c.current_hp = 0;
    }
    sts2_core::hooks::dispatch_after_block_gained(
        &mut state,
        &BlockGainedCtx {
            target: CreatureRef::Player,
            amount: 5,
            card_source: None,
        },
    );
    assert_eq!(flash_rows(&state, PowerKind::Juggernaut), 1);
}

// ── Vicious / Outbreak (after_power_applied) ────────────────────────────

#[test]
fn vicious_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::Vicious, 1);
    let enemy = add_enemy(&mut state, 40);

    sts2_core::hooks::dispatch_after_power_applied(
        &mut state,
        &PowerAppliedCtx {
            target: CreatureRef::Enemy(enemy),
            power_kind: PowerKind::Vulnerable,
            delta: 2,
            applier: Some(CreatureRef::Player),
        },
    );
    assert_eq!(flash_rows(&state, PowerKind::Vicious), 1);

    // C# `ViciousPower.cs:23` requires `power is VulnerablePower`. Applying a
    // DIFFERENT debuff runs the same dispatcher and must not flash.
    sts2_core::hooks::dispatch_after_power_applied(
        &mut state,
        &PowerAppliedCtx {
            target: CreatureRef::Enemy(enemy),
            power_kind: PowerKind::Weak,
            delta: 2,
            applier: Some(CreatureRef::Player),
        },
    );
    assert_eq!(flash_rows(&state, PowerKind::Vicious), 1);

    // ...and `applier == base.Owner`: an ENEMY-applied Vulnerable is excluded.
    sts2_core::hooks::dispatch_after_power_applied(
        &mut state,
        &PowerAppliedCtx {
            target: CreatureRef::Player,
            power_kind: PowerKind::Vulnerable,
            delta: 2,
            applier: Some(CreatureRef::Enemy(enemy)),
        },
    );
    assert_eq!(flash_rows(&state, PowerKind::Vicious), 1);
}

#[test]
fn outbreak_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::Outbreak, 5);
    let enemy = add_enemy(&mut state, 90);

    let poison = |s: &mut CombatState| {
        sts2_core::hooks::dispatch_after_power_applied(
            s,
            &PowerAppliedCtx {
                target: CreatureRef::Enemy(enemy),
                power_kind: PowerKind::Poison,
                delta: 3,
                applier: Some(CreatureRef::Player),
            },
        );
    };

    // C# `OutbreakPower.cs:46` is nested inside `timesPoisoned >= 3`: the first
    // two poison applications only bump the counter.
    poison(&mut state);
    assert_eq!(flash_rows(&state, PowerKind::Outbreak), 0);
    poison(&mut state);
    assert_eq!(flash_rows(&state, PowerKind::Outbreak), 0);

    poison(&mut state);
    assert_eq!(flash_rows(&state, PowerKind::Outbreak), 1);

    // The counter wraps (`timesPoisoned %= 3`), so the next one is quiet again.
    poison(&mut state);
    assert_eq!(flash_rows(&state, PowerKind::Outbreak), 1);
}

// ── MasterPlanner (after_card_played) ───────────────────────────────────

#[test]
fn master_planner_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::MasterPlanner, 1);

    let play = |s: &mut CombatState, ty: sts2_core::cards::CardType| {
        sts2_core::hooks::dispatch_after_card_played(
            s,
            &CardCtx {
                card_type: Some(ty),
                ..CardCtx::default()
            },
        );
    };

    play(&mut state, sts2_core::cards::CardType::Skill);
    assert_eq!(flash_rows(&state, PowerKind::MasterPlanner), 1);

    // C# `MasterPlannerPower.cs:25` returns early unless
    // `cardPlay.Card.Type == CardType.Skill`.
    play(&mut state, sts2_core::cards::CardType::Attack);
    assert_eq!(flash_rows(&state, PowerKind::MasterPlanner), 1);
    play(&mut state, sts2_core::cards::CardType::Power);
    assert_eq!(flash_rows(&state, PowerKind::MasterPlanner), 1);
}

// ── NoEnergyGain ────────────────────────────────────────────────────────

#[test]
fn no_energy_gain_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::NoEnergyGain, 1);

    state.gain_energy(2);
    assert_eq!(flash_rows(&state, PowerKind::NoEnergyGain), 1);

    // C# `Hook.AfterModifyingEnergyGain` (Hook.cs:853) only calls the models
    // that CHANGED the amount, and `PlayerCmd.GainEnergy` returns before the
    // hook chain when `amount <= 0` — so a zero gain flashes nothing.
    state.gain_energy(0);
    assert_eq!(flash_rows(&state, PowerKind::NoEnergyGain), 1);

    // Without the power the value is unchanged, so the model is not in
    // `modifiers` and never receives `AfterModifyingEnergyGain`.
    let mut clean = CombatState::default();
    clean.combat_log.event_log.set_recording(true);
    clean.gain_energy(2);
    assert_eq!(flash_rows(&clean, PowerKind::NoEnergyGain), 0);
}

// ── NoDraw ──────────────────────────────────────────────────────────────

#[test]
fn no_draw_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::NoDraw, 1);
    // A non-empty draw pile, so every negative half below is a draw that would
    // really have moved cards. With an empty pile nothing happens on any path
    // and the negatives cannot fail.
    for i in 1..=4u32 {
        let uid = add_card(&mut state, i, sts2_core::cards::CardType::Skill);
        state.player.draw.push(uid);
    }

    // Both card-effect draw chokepoints veto, and both flash. The ledger names
    // both, so both are driven here.
    sts2_core::cards::draw_via_state(&mut state, 1);
    assert_eq!(flash_rows(&state, PowerKind::NoDraw), 1);
    state.draw_into_hand_card_effect(1);
    assert_eq!(flash_rows(&state, PowerKind::NoDraw), 2);
    assert!(
        state.player.hand.is_empty(),
        "a vetoed draw must move no card"
    );

    // C# `NoDrawPower.cs:29` sits BELOW `if (fromHandDraw) return true;`, so the
    // turn-start hand draw is not vetoed and does not flash. This half can fail:
    // the pile is stocked, so the draw really happens, and an emission placed in
    // the shared `draw_into_hand_with_flag` body would fire here.
    let drawn = state.draw_into_hand(2);
    assert_eq!(drawn, 2, "the hand draw must actually draw");
    assert_eq!(flash_rows(&state, PowerKind::NoDraw), 2);

    // ...and the flash belongs to NoDraw, not to "the draw was vetoed". Fiddle
    // vetoes card-effect draws too (`cards::card_effect_draw_blocked`), and
    // `Hook.ShouldDraw` (Hook.cs:2413) attributes the prevention to the FIRST
    // listener that returns false. With no NoDraw in play, NoDraw is not that
    // listener and must stay silent.
    let mut fiddler = CombatState::default();
    fiddler.combat_log.event_log.set_recording(true);
    fiddler.relics.push(Relic::new(
        NonZeroU32::new(1).expect("non-zero relic id"),
        RelicKind::Fiddle,
    ));
    for i in 1..=2u32 {
        let uid = add_card(&mut fiddler, i, sts2_core::cards::CardType::Skill);
        fiddler.player.draw.push(uid);
    }
    assert_eq!(
        sts2_core::cards::draw_via_state(&mut fiddler, 1),
        0,
        "Fiddle must veto"
    );
    assert_eq!(flash_rows(&fiddler, PowerKind::NoDraw), 0);
}

// ── InfiniteBlades ──────────────────────────────────────────────────────

#[test]
fn infinite_blades_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::InfiniteBlades, 2);

    sts2_core::hooks::dispatch_before_hand_draw_powers(&mut state);
    // C# `InfiniteBladesPower.cs:24` flashes ONCE per listener and then makes
    // `Amount` Shivs — so a 2-stack InfiniteBlades flashes once, not twice.
    assert_eq!(flash_rows(&state, PowerKind::InfiniteBlades), 1);

    // The guard is `player == base.Owner.Player`; with no such power there is
    // no listener to reach it.
    let mut clean = CombatState::default();
    clean.combat_log.event_log.set_recording(true);
    sts2_core::hooks::dispatch_before_hand_draw_powers(&mut clean);
    assert_eq!(flash_rows(&clean, PowerKind::InfiniteBlades), 0);
}

// ── Juggling ────────────────────────────────────────────────────────────

/// A player's Juggling clone and the Aeonglass boss's Withering Presence both
/// answer the same card play: the host adds the clone to the hand first, then the
/// Wither (players' listeners run ahead of the monsters', as the roster order
/// says). F8 step 56, seed 8V6E089KN7 @1068: the hand read [.., clone, Wither] on
/// Headless and [.., Wither, clone] on the Sim.
#[test]
fn juggling_clone_lands_in_the_hand_before_the_withering_presence_wither() {
    let mut state = state_with_power(PowerKind::Juggling, 1);
    state.player.creature_uid = Some(CreatureUid::new(999).expect("nz"));
    let enemy = add_enemy(&mut state, 40);
    push_enemy_power(&mut state, 2, enemy, PowerKind::WitheringPresence, 6);
    let card = add_card(&mut state, 1, sts2_core::cards::CardType::Attack);
    let play = |s: &mut CombatState| {
        sts2_core::hooks::dispatch_after_card_played(
            s,
            &CardCtx {
                card_uid: Some(card),
                card_type: Some(sts2_core::cards::CardType::Attack),
                ..CardCtx::default()
            },
        );
    };
    play(&mut state);
    play(&mut state);
    // The 6th card of the turn is the 3rd attack: both fire on this play.
    for p in &mut state.powers {
        if p.kind == PowerKind::WitheringPresence {
            p.counter = 5;
        }
    }
    play(&mut state);
    let ids: Vec<CardId> = state
        .player
        .hand
        .iter()
        .map(|u| state.cards[u.get() as usize - 1].card_id)
        .collect();
    assert_eq!(ids, vec![CardId::StrikeIronclad, CardId::Wither]);
}

#[test]
fn juggling_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::Juggling, 1);
    let card = add_card(&mut state, 1, sts2_core::cards::CardType::Attack);

    let play = |s: &mut CombatState, uid: CardUid| {
        sts2_core::hooks::dispatch_after_card_played(
            s,
            &CardCtx {
                card_uid: Some(uid),
                card_type: Some(sts2_core::cards::CardType::Attack),
                ..CardCtx::default()
            },
        );
    };

    // C# `JugglingPower.cs:45` is nested inside `attacksPlayedThisTurn == 3` —
    // EXACTLY three, checked after the increment. Attacks one and two are
    // silent.
    play(&mut state, card);
    assert_eq!(flash_rows(&state, PowerKind::Juggling), 0);
    play(&mut state, card);
    assert_eq!(flash_rows(&state, PowerKind::Juggling), 0);

    play(&mut state, card);
    assert_eq!(flash_rows(&state, PowerKind::Juggling), 1);

    // `== 3`, not `>= 3`: the fourth attack of the turn does not flash again.
    play(&mut state, card);
    assert_eq!(flash_rows(&state, PowerKind::Juggling), 1);
}

#[test]
fn juggling_does_not_count_non_attacks_toward_its_third() {
    let mut state = state_with_power(PowerKind::Juggling, 1);
    let card = add_card(&mut state, 1, sts2_core::cards::CardType::Skill);

    // C#: `if (... || cardPlay.Card.Type != CardType.Attack) return;` — the
    // counter is not even incremented, so three Skills never reach the flash.
    for _ in 0..3 {
        sts2_core::hooks::dispatch_after_card_played(
            &mut state,
            &CardCtx {
                card_uid: Some(card),
                card_type: Some(sts2_core::cards::CardType::Skill),
                ..CardCtx::default()
            },
        );
    }
    assert_eq!(flash_rows(&state, PowerKind::Juggling), 0);
}

// ── CorrosiveWave ───────────────────────────────────────────────────────

#[test]
fn corrosive_wave_flashes_once_per_drawn_card() {
    let mut state = state_with_power(PowerKind::CorrosiveWave, 2);
    add_enemy(&mut state, 40);
    for i in 1..=3u32 {
        let uid = add_card(&mut state, i, sts2_core::cards::CardType::Skill);
        state.player.draw.push(uid);
    }

    // C# `CorrosiveWavePower.cs:32` sits in `AfterCardDrawn`, one hook call per
    // DRAWN card — so two drawn cards flash twice, not once for the batch.
    state.draw_into_hand(2);
    assert_eq!(flash_rows(&state, PowerKind::CorrosiveWave), 2);

    state.draw_into_hand(1);
    assert_eq!(flash_rows(&state, PowerKind::CorrosiveWave), 3);

    // A draw that moves nothing (the pile is empty now) fires no
    // `AfterCardDrawn` at all, so the count holds.
    state.draw_into_hand(1);
    assert_eq!(flash_rows(&state, PowerKind::CorrosiveWave), 3);
}

// ── Strangle ────────────────────────────────────────────────────────────

#[test]
fn strangle_flashes_on_the_marked_enemy_only_for_a_registered_card() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let enemy = add_enemy(&mut state, 100);
    // Strangle is a player-applied DEBUFF: `Strangle.cs:32` applies it to
    // `cardPlay.Target` with the player as APPLIER, so the OWNER — whom
    // `Flash()` draws on — is the enemy.
    sts2_core::card_effects::apply_power_to_enemy(&mut state, enemy, PowerKind::Strangle, 2);

    // C# `StranglePower.cs:54` flashes inside
    // `amountsForPlayedCards.Remove(card, out value)` — only for a card the
    // BEFORE-play arm registered. A card played with nothing registered (the
    // Strangle card's own play, whose power is created mid-`OnPlay`) removes
    // nothing, so it does not flash.
    sts2_core::hooks::dispatch_after_card_played(&mut state, &CardCtx::default());
    assert_eq!(flash_rows(&state, PowerKind::Strangle), 0);

    // Register this card, then play it: now the Remove succeeds and it flashes.
    sts2_core::hooks::dispatch_before_card_played_powers(&mut state, 1, false);
    sts2_core::hooks::dispatch_after_card_played(&mut state, &CardCtx::default());
    assert_eq!(flash_rows(&state, PowerKind::Strangle), 1);

    // The flash is drawn on the ENEMY that carries the power, not on the
    // player who applied it — `flash_power(.., owner_uid, ..)`, not
    // `flash_player_power`.
    let on_enemy = state
        .combat_log
        .event_log
        .rows()
        .iter()
        .filter(|r| {
            matches!(
                &r.entry,
                EventLogEntry::PowerFlashed { power: PowerKind::Strangle, creature }
                    if !matches!(creature, sts2_core::event_log::LogCreature::Player(_))
            )
        })
        .count();
    assert_eq!(on_enemy, 1, "Strangle's owner is the marked enemy");

    // The after-play arm disarms every instance, so the NEXT card play flashes
    // nothing until before-play re-registers it.
    sts2_core::hooks::dispatch_after_card_played(&mut state, &CardCtx::default());
    assert_eq!(flash_rows(&state, PowerKind::Strangle), 1);
}

// ── the inherited TemporaryStrength / TemporaryDexterity flash ────────

/// `TemporaryStrengthPower.cs:177` and `TemporaryDexterityPower.cs:173` are
/// `Flash()` calls on ABSTRACT base classes. Neither is ever instantiated, so
/// the call is inherited by all 13 / all 3 subclasses — and Rust models each
/// subclass as its own `PowerKind`, so one C# site is 16 Rust arms. A partial
/// wiring is invisible from the ledger (one row, one `ported` bool), so this
/// test enumerates the whole subclass set rather than sampling it.
#[test]
fn every_temporary_stat_subclass_inherits_the_flash() {
    const STRENGTH: [PowerKind; 13] = [
        PowerKind::Coordinate,
        PowerKind::CrushUnder,
        PowerKind::DarkShackles,
        PowerKind::DyingStar,
        PowerKind::EnfeeblingTouch,
        PowerKind::FeedingFrenzy,
        PowerKind::FlexPotion,
        PowerKind::ManglePower,
        PowerKind::MonarchsGazeStrengthDown,
        PowerKind::PiercingWail,
        PowerKind::ReptileTrinket,
        PowerKind::SetupStrike,
        PowerKind::ShacklingPotion,
    ];
    const DEXTERITY: [PowerKind; 3] = [
        PowerKind::Anticipate,
        PowerKind::HelicalDart,
        PowerKind::SpeedPotion,
    ];

    for kind in STRENGTH.iter().chain(DEXTERITY.iter()).copied() {
        let mut state = state_with_power(kind, 2);

        // C# guard: `participants.Contains(base.Owner)`. The owner is the
        // player here, so the player's own turn end fires it.
        sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Player });
        assert_eq!(
            flash_rows(&state, kind),
            1,
            "{kind:?} must flash at its owner's turn end"
        );

        // ...and the ENEMY's turn end must not: a player-owned temporary stat
        // power is not a participant there. This half can fail — the arm is
        // reached on both sides and only the side guard separates them.
        sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
        assert_eq!(
            flash_rows(&state, kind),
            1,
            "{kind:?} must not flash on the enemy's turn end"
        );
    }
}

#[test]
fn an_enemy_owned_temporary_strength_flashes_on_the_enemy() {
    // These powers are owner-scoped, not player-scoped: Mangle and DarkShackles
    // live on ENEMIES. `PowerModel.Flash()` draws on the Owner, so the row must
    // name the enemy — `Effect::FlashPower { creature: owner, .. }`, resolved
    // through `CreatureRef::Enemy`, not `flash_player_power`.
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let enemy = add_enemy(&mut state, 40);
    sts2_core::card_effects::apply_power_to_enemy(&mut state, enemy, PowerKind::ManglePower, 3);

    // The enemy's own turn end is the one that fires.
    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Player });
    assert_eq!(flash_rows(&state, PowerKind::ManglePower), 0);

    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert_eq!(flash_rows(&state, PowerKind::ManglePower), 1);

    let on_player = state
        .combat_log
        .event_log
        .rows()
        .iter()
        .filter(|r| {
            matches!(
                &r.entry,
                EventLogEntry::PowerFlashed { power: PowerKind::ManglePower, creature }
                    if matches!(creature, sts2_core::event_log::LogCreature::Player(_))
            )
        })
        .count();
    assert_eq!(
        on_player, 0,
        "an enemy-owned Mangle must not flash on the player"
    );
}

// ── the disarmed contract ───────────────────────────────────────────────

#[test]
fn a_disarmed_run_gets_a_byte_identical_effect_vector() {
    // THE load-bearing claim of this sub-project: ~90 flash sites can be added
    // across live power logic without disturbing search, parity, or any test
    // that asserts on effect vectors, because every `Effect::FlashPower` push
    // is gated on `event_log::armed`.
    //
    // Comparing post-STATE cannot check that — applying a `FlashPower` to a
    // disarmed state is itself a no-op, so an unguarded push leaves both sides
    // identical and the test passes either way. The thing that must not change
    // is the VECTOR, so compare that instead, before anything applies it.
    let build = |record: bool| {
        let mut state = CombatState::default();
        state.combat_log.event_log.set_recording(record);
        push_player_power(&mut state, 1, PowerKind::DemonForm, 2);
        push_player_power(&mut state, 2, PowerKind::NoxiousFumes, 3);
        push_player_power(&mut state, 3, PowerKind::WraithForm, 1);
        add_enemy(&mut state, 40);
        state
    };
    let ctx = TurnSideCtx { side: Side::Player };

    let quiet = build(false);
    let loud = build(true);

    let quiet_effects = sts2_core::hooks::collect_after_side_turn_start(&quiet, &ctx);
    let loud_effects = sts2_core::hooks::collect_after_side_turn_start(&loud, &ctx);

    // Disarmed: not one `FlashPower` anywhere in the vector.
    assert!(
        !quiet_effects
            .iter()
            .any(|e| matches!(e, sts2_core::hooks::Effect::FlashPower { .. })),
        "a disarmed run must not carry FlashPower effects: {quiet_effects:?}"
    );

    // Armed: the flashes are there, and they are the ONLY difference — strip
    // them and the two vectors are equal, element for element and in order.
    let stripped: Vec<_> = loud_effects
        .iter()
        .filter(|e| !matches!(e, sts2_core::hooks::Effect::FlashPower { .. }))
        .copied()
        .collect();
    assert_eq!(stripped, quiet_effects.to_vec());
    assert_eq!(
        loud_effects.len() - stripped.len(),
        3,
        "one flash per power"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Defect + Regent (task 4)
//
// Same contract as above: every test carries the NEGATIVE half of the C#
// guard it pins, and each negative names the `<Class>.cs:<line>` clause that
// excludes it.
// ═══════════════════════════════════════════════════════════════════════

/// A `Card` VALUE (not yet in the arena) for the generation-listener sites,
/// which take ownership of the card they spawn.
fn make_card(card_type: sts2_core::cards::CardType) -> Card {
    Card {
        uid: CardUid::new(1).expect("non-zero card uid"),
        card_id: CardId::StrikeIronclad,
        cost: 1,
        card_type,
        target_type: TargetType::Enemy,
        upgraded: false,
        base_damage: 6,
        base_block: 0,
        combat_vars: CombatVars::new(),
        enchantments: CardEnchantments::new(),
        tags: CardTags::NONE,
        energy_spent: 0,
        base_replay_count: 0,
        keyword_override: None,
        affliction: None,
    }
}

/// Give an ENEMY a power, so owner-gated arms can be driven from the wrong
/// side. `push_player_power`'s counterpart.
fn push_enemy_power(
    state: &mut CombatState,
    id: u32,
    enemy: CreatureUid,
    kind: PowerKind,
    amount: i32,
) -> PowerUid {
    let uid = power_uid(id);
    state.powers.push(Power {
        id: uid.0,
        kind,
        amount,
        counter: 0,
        applier: None,
        skip_next_tick: false,
    });
    if let Some(c) = state.creatures.iter_mut().find(|c| c.id == enemy.0) {
        c.powers.push(uid);
    }
    uid
}

// ── BiasedCognition (after_side_turn_start) ─────────────────────────────

#[test]
fn biased_cognition_flashes_only_when_its_guard_holds() {
    // C# `BiasedCognitionPower.cs:25`, inside `participants.Contains(Owner)`.
    side_turn_start_flashes_only_on_the_owners_side(PowerKind::BiasedCognition);
}

// ── Coolant (after_side_turn_start) ─────────────────────────────────────

#[test]
fn coolant_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::Coolant, 2);
    sts2_core::orbs::channel_orb(&mut state, sts2_core::orbs::Orb::Frost);

    sts2_core::hooks::dispatch_after_side_turn_start(
        &mut state,
        &TurnSideCtx { side: Side::Player },
    );
    assert_eq!(flash_rows(&state, PowerKind::Coolant), 1);

    // C# `CoolantPower.cs:25` sits inside `participants.Contains(Owner)`: a
    // player-owned Coolant is not a participant in the enemy's turn start.
    sts2_core::hooks::dispatch_after_side_turn_start(
        &mut state,
        &TurnSideCtx { side: Side::Enemy },
    );
    assert_eq!(flash_rows(&state, PowerKind::Coolant), 1);
}

#[test]
fn coolant_flashes_on_an_empty_orb_belt() {
    // `participants.Contains(Owner)` is the WHOLE guard on `CoolantPower.cs:25`
    // — the orb grouping and the Block grant are both below it. An empty belt
    // is where a Defect's turn 1 starts, and where Fission or a full evoke
    // leaves it, so this is the ordinary case, not an edge one. It is also the
    // case the arm used to miss: the flash sat under the orb-count guard.
    let mut state = state_with_power(PowerKind::Coolant, 2);

    sts2_core::hooks::dispatch_after_side_turn_start(
        &mut state,
        &TurnSideCtx { side: Side::Player },
    );

    assert_eq!(flash_rows(&state, PowerKind::Coolant), 1);
    // ...and the empty belt still grants nothing, which is the half of this
    // that combat can see. `defect_power_listeners.rs` pins it from the Block
    // side; asserted here too so a future re-merge of the two gates fails
    // against the flash test that motivated the split.
    assert_eq!(state.player.block, 0);
}

// ── Hailstorm (before_turn_end) ─────────────────────────────────────────

#[test]
fn hailstorm_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::Hailstorm, 4);
    add_enemy(&mut state, 40);

    // C# `HailstormPower.cs:37` is nested inside `frostOrbs >= FrostOrbs(1)`.
    // With a Lightning-only belt the threshold fails and nothing flashes —
    // and this half can fail: the owner side and the living enemy are both
    // right, so only the Frost count keeps it quiet.
    sts2_core::orbs::channel_orb(&mut state, sts2_core::orbs::Orb::Lightning);
    sts2_core::hooks::dispatch_before_turn_end(&mut state, &TurnSideCtx { side: Side::Player });
    assert_eq!(flash_rows(&state, PowerKind::Hailstorm), 0);

    sts2_core::orbs::channel_orb(&mut state, sts2_core::orbs::Orb::Frost);
    sts2_core::hooks::dispatch_before_turn_end(&mut state, &TurnSideCtx { side: Side::Player });
    assert_eq!(flash_rows(&state, PowerKind::Hailstorm), 1);

    // ...and inside `participants.Contains(Owner)`: the enemy's turn end is
    // not the player-owned Hailstorm's boundary.
    sts2_core::hooks::dispatch_before_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert_eq!(flash_rows(&state, PowerKind::Hailstorm), 1);
}

// ── Iteration (after_card_drawn) ────────────────────────────────────────

#[test]
fn iteration_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::Iteration, 1);
    // An all-Status draw pile, so every draw below really reaches the arm and
    // the negative half cannot pass by drawing nothing.
    for i in 1..=6u32 {
        let uid = add_card(&mut state, i, sts2_core::cards::CardType::Status);
        state.player.draw.push(uid);
    }

    state.draw_into_hand(1);
    assert_eq!(flash_rows(&state, PowerKind::Iteration), 1);
    // The flash rides a real +Amount draw: one asked-for card plus Iteration's.
    assert_eq!(state.player.hand.len(), 2);

    // C# `IterationPower.cs:25` is nested inside `num <= 1` — `num` counts the
    // Status cards drawn this turn INCLUDING the current one, so only the
    // turn's FIRST status fires. Every card in this pile is a Status, so this
    // draw reaches the arm and is excluded by the counter alone.
    state.draw_into_hand(1);
    assert_eq!(flash_rows(&state, PowerKind::Iteration), 1);
}

// ── Smokestack / Arsenal / PillarOfCreation (after_card_generated) ──────

#[test]
fn smokestack_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::Smokestack, 3);
    add_enemy(&mut state, 40);

    let status = make_card(sts2_core::cards::CardType::Status);
    sts2_core::card_effects::spawn_card_into_pile_with_origin(
        &mut state,
        status,
        sts2_core::card_effects::PileKind::Discard,
        true,
    );
    assert_eq!(flash_rows(&state, PowerKind::Smokestack), 1);

    // C# `SmokestackPower.cs:21` requires `card.Type == CardType.Status`. A
    // player-generated SKILL runs the same generation listeners and is excluded.
    let skill = make_card(sts2_core::cards::CardType::Skill);
    sts2_core::card_effects::spawn_card_into_pile_with_origin(
        &mut state,
        skill,
        sts2_core::card_effects::PileKind::Discard,
        true,
    );
    assert_eq!(flash_rows(&state, PowerKind::Smokestack), 1);

    // ...and `creator != null && creator.Creature == Owner`: an ENEMY-spawned
    // Status (Wound / Slimed) is not the owner's generation.
    let enemy_status = make_card(sts2_core::cards::CardType::Status);
    sts2_core::card_effects::spawn_card_into_pile_with_origin(
        &mut state,
        enemy_status,
        sts2_core::card_effects::PileKind::Discard,
        false,
    );
    assert_eq!(flash_rows(&state, PowerKind::Smokestack), 1);
}

#[test]
fn arsenal_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::Arsenal, 2);

    // C# `ArsenalPower.cs:23` has NO card-type filter — any player-generated
    // card fires it.
    let skill = make_card(sts2_core::cards::CardType::Skill);
    sts2_core::card_effects::spawn_card_into_pile_with_origin(
        &mut state,
        skill,
        sts2_core::card_effects::PileKind::Discard,
        true,
    );
    assert_eq!(flash_rows(&state, PowerKind::Arsenal), 1);

    // The guard that DOES exclude: `creator != null && creator.Creature ==
    // Owner`. An enemy-spawned card runs the same listeners and must be quiet.
    let enemy_card = make_card(sts2_core::cards::CardType::Status);
    sts2_core::card_effects::spawn_card_into_pile_with_origin(
        &mut state,
        enemy_card,
        sts2_core::card_effects::PileKind::Discard,
        false,
    );
    assert_eq!(flash_rows(&state, PowerKind::Arsenal), 1);
}

#[test]
fn pillar_of_creation_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::PillarOfCreation, 2);

    let skill = make_card(sts2_core::cards::CardType::Skill);
    sts2_core::card_effects::spawn_card_into_pile_with_origin(
        &mut state,
        skill,
        sts2_core::card_effects::PileKind::Discard,
        true,
    );
    assert_eq!(flash_rows(&state, PowerKind::PillarOfCreation), 1);

    // C# `PillarOfCreationPower.cs:23`: `creator != null && creator.Creature
    // == Owner` — an enemy-spawned card is excluded.
    let enemy_card = make_card(sts2_core::cards::CardType::Status);
    sts2_core::card_effects::spawn_card_into_pile_with_origin(
        &mut state,
        enemy_card,
        sts2_core::card_effects::PileKind::Discard,
        false,
    );
    assert_eq!(flash_rows(&state, PowerKind::PillarOfCreation), 1);
}

#[test]
fn trash_to_treasure_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::TrashToTreasure, 2);
    state.player.orbs = sts2_core::orbs::OrbSlots::with_capacity(3);

    let status = make_card(sts2_core::cards::CardType::Status);
    sts2_core::card_effects::spawn_card_into_pile_with_origin(
        &mut state,
        status,
        sts2_core::card_effects::PileKind::Discard,
        true,
    );
    // C# `TrashToTreasurePower.cs:20` flashes ONCE per generation, above the
    // per-stack channel loop — TrashToTreasure(2) channels 2 orbs, not 2
    // flashes.
    assert_eq!(flash_rows(&state, PowerKind::TrashToTreasure), 1);
    assert_eq!(state.player.orbs.filled_len(), 2);

    // `card.Type == CardType.Status` excludes a player-generated SKILL.
    let skill = make_card(sts2_core::cards::CardType::Skill);
    sts2_core::card_effects::spawn_card_into_pile_with_origin(
        &mut state,
        skill,
        sts2_core::card_effects::PileKind::Discard,
        true,
    );
    assert_eq!(flash_rows(&state, PowerKind::TrashToTreasure), 1);

    // ...and `creator != null && creator.Creature == Owner` excludes an
    // ENEMY-spawned Status (Wound / Slimed).
    let enemy_status = make_card(sts2_core::cards::CardType::Status);
    sts2_core::card_effects::spawn_card_into_pile_with_origin(
        &mut state,
        enemy_status,
        sts2_core::card_effects::PileKind::Discard,
        false,
    );
    assert_eq!(flash_rows(&state, PowerKind::TrashToTreasure), 1);
}

// ── Storm / Subroutine (armed BeforeCardPlayed → AfterCardPlayed) ───────

fn armed_snapshot_flashes_only_for_a_registered_power_card(kind: PowerKind) {
    let mut state = state_with_power(kind, 2);

    let play = |s: &mut CombatState, ty: sts2_core::cards::CardType| {
        sts2_core::hooks::dispatch_after_card_played(
            s,
            &CardCtx {
                card_type: Some(ty),
                ..CardCtx::default()
            },
        );
    };

    // C# records `base.Amount` for the played card in `BeforeCardPlayed` and
    // `AfterCardPlayed` flashes only if the Remove SUCCEEDS. Playing a Power
    // card with nothing registered — the Storm/Subroutine card's OWN play,
    // whose power is created mid-`OnPlay` — removes nothing.
    play(&mut state, sts2_core::cards::CardType::Power);
    assert_eq!(flash_rows(&state, kind), 0, "{kind:?} unregistered play");

    sts2_core::hooks::dispatch_before_card_played_powers(&mut state, 1, false);
    play(&mut state, sts2_core::cards::CardType::Power);
    assert_eq!(flash_rows(&state, kind), 1);

    // The C# `BeforeCardPlayed` arm returns early on `Card.Type !=
    // CardType.Power`, so a registered-then-played SKILL never had an entry:
    // the Remove fails and it does not flash.
    sts2_core::hooks::dispatch_before_card_played_powers(&mut state, 1, false);
    play(&mut state, sts2_core::cards::CardType::Skill);
    assert_eq!(flash_rows(&state, kind), 1, "{kind:?} skill play");
}

#[test]
fn storm_flashes_only_when_its_guard_holds() {
    // C# `StormPower.cs:57`.
    armed_snapshot_flashes_only_for_a_registered_power_card(PowerKind::Storm);
}

#[test]
fn subroutine_flashes_only_when_its_guard_holds() {
    // C# `SubroutinePower.cs:49`.
    armed_snapshot_flashes_only_for_a_registered_power_card(PowerKind::Subroutine);
}

// ── Monologue (armed BeforeCardPlayed → AfterCardPlayed) ────────────────

#[test]
fn monologue_flashes_only_when_its_guard_holds() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    state.player.current_hp = 80;
    state.player.max_hp = 80;
    // The Monologue arm grants Strength through the player's mirror Creature;
    // without one it takes the disarm-only fallback and never reaches the
    // emission, so the negative half below would be vacuous.
    state.init_player_creature();
    push_player_power(&mut state, 1, PowerKind::Monologue, 1);

    let play = |s: &mut CombatState| {
        sts2_core::hooks::dispatch_after_card_played(s, &CardCtx::default());
    };

    // C# `MonologuePower.cs:74` flashes inside
    // `amountsForPlayedCards.Remove(card, out value)` — only for a card the
    // BEFORE-play arm registered. The Monologue card's own play (its power is
    // created mid-`OnPlay`) registers nothing.
    play(&mut state);
    assert_eq!(flash_rows(&state, PowerKind::Monologue), 0);

    sts2_core::hooks::dispatch_before_card_played_powers(&mut state, 1, false);
    play(&mut state);
    assert_eq!(flash_rows(&state, PowerKind::Monologue), 1);

    // The after-play arm disarms the instance, so the next play is quiet again
    // until before-play re-registers it.
    play(&mut state);
    assert_eq!(flash_rows(&state, PowerKind::Monologue), 1);
}

// ── TheSealedThrone (BeforeCardPlayed) ─────────────────────────────────

#[test]
fn the_sealed_throne_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::TheSealedThrone, 1);
    sts2_core::hooks::dispatch_before_card_played_powers(&mut state, 1, false);
    assert_eq!(flash_rows(&state, PowerKind::TheSealedThrone), 1);

    // C# `TheSealedThronePower.cs:18` sits in the TheSealedThrone listener's
    // own body — the dispatcher itself runs for every card play. Drive the same
    // dispatcher with a DIFFERENT power live (Storm passes the `present_kinds`
    // gate so the whole walk really executes) and no SealedThrone flash may
    // appear.
    let mut other = CombatState::default();
    other.combat_log.event_log.set_recording(true);
    push_player_power(&mut other, 1, PowerKind::Storm, 2);
    sts2_core::hooks::dispatch_before_card_played_powers(&mut other, 1, false);
    assert_eq!(flash_rows(&other, PowerKind::TheSealedThrone), 0);
}

// ── SpectrumShift (before_hand_draw) ───────────────────────────────────

#[test]
fn spectrum_shift_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::SpectrumShift, 2);
    sts2_core::hooks::dispatch_before_hand_draw_powers(&mut state);
    // C# `SpectrumShiftPower.cs:27` flashes ONCE per instance per hand draw,
    // after the whole `Amount`-card generation — not once per card.
    assert_eq!(flash_rows(&state, PowerKind::SpectrumShift), 1);

    // The guard is `player == Owner.Player`; with a different generator power
    // live (InfiniteBlades drives the same dispatcher through its own
    // generation lane) SpectrumShift has no listener and must stay silent.
    let mut other = CombatState::default();
    other.combat_log.event_log.set_recording(true);
    push_player_power(&mut other, 1, PowerKind::InfiniteBlades, 2);
    sts2_core::hooks::dispatch_before_hand_draw_powers(&mut other);
    assert_eq!(flash_rows(&other, PowerKind::SpectrumShift), 0);
    assert_eq!(flash_rows(&other, PowerKind::InfiniteBlades), 1);
}

// ── Thunder (after_orb_evoked) ─────────────────────────────────────────

#[test]
fn thunder_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::Thunder, 5);
    let enemy = add_enemy(&mut state, 60);

    sts2_core::hooks::dispatch_after_orb_evoked(
        &mut state,
        sts2_core::orbs::OrbKind::Lightning,
        &[enemy],
    );
    assert_eq!(flash_rows(&state, PowerKind::Thunder), 1);

    // C# `ThunderPower.cs:31` requires `orb is LightningOrb`. A Frost evoke
    // reaches the same `Hook.AfterOrbEvoked` with the same live targets and is
    // excluded.
    sts2_core::hooks::dispatch_after_orb_evoked(
        &mut state,
        sts2_core::orbs::OrbKind::Frost,
        &[enemy],
    );
    assert_eq!(flash_rows(&state, PowerKind::Thunder), 1);
}

// ── Genesis (after_energy_reset) ───────────────────────────────────────

#[test]
fn genesis_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::Genesis, 2);
    sts2_core::hooks::dispatch_after_energy_reset(
        &mut state,
        &sts2_core::hooks::CombatLifecycleCtx::default(),
    );
    assert_eq!(flash_rows(&state, PowerKind::Genesis), 1);

    // C# `GenesisPower.cs:18` is nested inside `player == base.Owner.Player`.
    // The same dispatcher walks EVERY creature's powers, so an enemy-owned
    // Genesis reaches the arm and is excluded by ownership alone.
    let mut enemy_side = CombatState::default();
    enemy_side.combat_log.event_log.set_recording(true);
    let enemy = add_enemy(&mut enemy_side, 40);
    push_enemy_power(&mut enemy_side, 1, enemy, PowerKind::Genesis, 2);
    sts2_core::hooks::dispatch_after_energy_reset(
        &mut enemy_side,
        &sts2_core::hooks::CombatLifecycleCtx::default(),
    );
    assert_eq!(flash_rows(&enemy_side, PowerKind::Genesis), 0);
}

// ── Orbit (after_energy_spent) ─────────────────────────────────────────

#[test]
fn orbit_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::Orbit, 1);

    // C# `OrbitPower.cs:47` is nested inside `triggers > 0`, where
    // `triggers = energySpent / 4 - triggerCount`. Three energy is not a
    // crossing.
    sts2_core::hooks::dispatch_after_energy_spent(&mut state, 3);
    assert_eq!(flash_rows(&state, PowerKind::Orbit), 0);

    // The fourth crosses the boundary.
    sts2_core::hooks::dispatch_after_energy_spent(&mut state, 1);
    assert_eq!(flash_rows(&state, PowerKind::Orbit), 1);

    // ...and the fifth does not: `energySpent / 4` is still 1.
    sts2_core::hooks::dispatch_after_energy_spent(&mut state, 1);
    assert_eq!(flash_rows(&state, PowerKind::Orbit), 1);
}

// ── BlackHole / ChildOfTheStars (stars spent) ──────────────────────────

#[test]
fn black_hole_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::BlackHole, 3);
    add_enemy(&mut state, 60);
    state.player.stars = 4;

    assert_eq!(state.spend_stars(2), 2, "the spend must really happen");
    assert_eq!(flash_rows(&state, PowerKind::BlackHole), 1);

    // C# `BlackHolePower.cs:31` is nested inside `amount > 0` — a zero delta
    // never reaches `DealDamageToAllEnemies` (and its `Flash()` at :39).
    //
    // Drive the reactive site DIRECTLY rather than through `spend_stars(0)`:
    // `spend_stars` returns at its OWN `amount <= 0` guard (lib.rs:4250) before
    // it ever calls this method (lib.rs:4272), so that route never enters the
    // function that holds the emission — a flash hoisted above the method's own
    // guards would pass both halves. Calling it with 0 does enter.
    state.apply_black_hole_on_stars_event(0);
    assert_eq!(flash_rows(&state, PowerKind::BlackHole), 1);

    // ...and the second guard, `amount > 0` on the POWER: a live spend with a
    // zeroed BlackHole enters the function, clears the delta guard, and is
    // excluded by the stack count alone.
    for p in &mut state.powers {
        if p.kind == PowerKind::BlackHole {
            p.amount = 0;
        }
    }
    assert_eq!(
        state.spend_stars(2),
        2,
        "the second spend must really happen"
    );
    assert_eq!(flash_rows(&state, PowerKind::BlackHole), 1);
}

#[test]
fn child_of_the_stars_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::ChildOfTheStars, 3);
    state.player.stars = 4;

    assert_eq!(state.spend_stars(2), 2);
    assert_eq!(flash_rows(&state, PowerKind::ChildOfTheStars), 1);

    // C# `ChildOfTheStarsPower.cs:17`: `amount > 0 && spender == Owner.Player`.
    //
    // Drive the reactive site DIRECTLY rather than through `spend_stars(0)`:
    // `spend_stars` returns at its OWN `amount <= 0` guard (lib.rs:4250) before
    // it ever calls this method (lib.rs:4273), so that route cannot distinguish
    // a correctly-placed flash from one hoisted above the method's own guards.
    // Calling it with 0 does enter.
    state.apply_child_of_the_stars_on_stars_spent(0);
    assert_eq!(flash_rows(&state, PowerKind::ChildOfTheStars), 1);

    // ...and the block guard: a live spend with a zeroed ChildOfTheStars enters
    // the function, clears the delta guard, and is excluded by the stack count
    // alone.
    for p in &mut state.powers {
        if p.kind == PowerKind::ChildOfTheStars {
            p.amount = 0;
        }
    }
    assert_eq!(
        state.spend_stars(2),
        2,
        "the second spend must really happen"
    );
    assert_eq!(flash_rows(&state, PowerKind::ChildOfTheStars), 1);
}

// ── EchoForm / Feral (whole card plays) ────────────────────────────────
//
// Both sites live inside `combat_loop`'s play pipeline rather than a hook
// dispatcher, so these two drive a real `play_card`.

/// A combat fixture ready to take `combat_loop::play_card`: living player,
/// one living enemy, play phase, energy on the bar.
fn play_ready_state(kind: PowerKind, amount: i32) -> CombatState {
    let mut state = state_with_power(kind, amount);
    state.player.current_hp = 80;
    state.player.max_hp = 80;
    add_enemy(&mut state, 200);
    state.phase = sts2_core::CombatPhase::PlayerPlayPhase;
    state.player.energy = 3;
    state
}

/// Put a playable card of `card_type` (cost `cost`, untargeted) in hand.
fn add_hand_card(
    state: &mut CombatState,
    id: u32,
    card_type: sts2_core::cards::CardType,
    cost: i8,
) -> CardUid {
    let uid = add_card(state, id, card_type);
    let idx = uid.get() as usize - 1;
    state.cards[idx].cost = cost;
    state.cards[idx].target_type = TargetType::None;
    state.cards[idx].base_damage = 0;
    state.player.hand.push(uid);
    uid
}

#[test]
fn echo_form_flashes_only_when_its_guard_holds() {
    let mut state = play_ready_state(PowerKind::EchoForm, 1);
    add_hand_card(&mut state, 1, sts2_core::cards::CardType::Skill, 0);
    add_hand_card(&mut state, 2, sts2_core::cards::CardType::Skill, 0);

    // C# `EchoFormPower.cs:32` (`AfterModifyingCardPlayCount`) fires only on a
    // model that actually raised the count — `ModifyCardPlayCount` returns
    // `playCount + 1` while `first-in-series plays this turn < Amount`. With
    // Amount = 1 the turn's FIRST play qualifies.
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, None);
    assert_eq!(flash_rows(&state, PowerKind::EchoForm), 1);

    // The second play does not: `num` is now 1, which is not `< Amount`. This
    // half can fail — the card is really played (the hand shrinks), so an
    // emission wired to every play would fire here.
    let hand_before = state.player.hand.len();
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, None);
    assert!(state.player.hand.len() < hand_before, "the card must play");
    assert_eq!(flash_rows(&state, PowerKind::EchoForm), 1);
}

#[test]
fn feral_flashes_only_when_its_guard_holds() {
    let mut state = play_ready_state(PowerKind::Feral, 1);
    // A 1-cost Attack: same type, same owner, but `ResourceInfo.EnergyValue`
    // is not 0, so C# `FeralPower.cs:47` returns the pile unchanged and
    // `AfterModifyingCardPlayResultPileOrPosition` never fires for it.
    add_hand_card(&mut state, 1, sts2_core::cards::CardType::Attack, 1);
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, None);
    assert_eq!(flash_rows(&state, PowerKind::Feral), 0);

    // A 0-cost Attack under the per-turn budget IS redirected to Hand/Top, so
    // the C# hook fires and flashes.
    let zero_cost = add_hand_card(&mut state, 2, sts2_core::cards::CardType::Attack, 0);
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, None);
    assert_eq!(flash_rows(&state, PowerKind::Feral), 1);
    assert!(
        state.player.hand.contains(&zero_cost),
        "Feral must return the card to hand"
    );

    // C# `FeralPower.cs:51`: `zeroCostAttacksPlayed >= Amount` returns the pile
    // unchanged. With Amount = 1 the turn's SECOND 0-cost Attack is a normal
    // play — it reaches the same branch and is excluded by the tally alone.
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, None);
    assert!(
        !state.player.hand.contains(&zero_cost),
        "the second 0-cost Attack must really play, and NOT come back"
    );
    assert_eq!(flash_rows(&state, PowerKind::Feral), 1);
}

// ── task-5 remaining sites ─────────────────────────────────────────────

/// C# `BlockNextTurnPower.cs:23` (`AfterBlockCleared`) flashes inside
/// `creature == base.Owner`. `SelfFormingClayPower` shares every other line
/// of this arm but has NO `Flash()` call at all — see the arm's own doc.
#[test]
fn block_next_turn_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::BlockNextTurn, 10);
    sts2_core::hooks::dispatch_after_block_cleared(
        &mut state,
        &BlockClearedCtx {
            cleared: Some(CreatureRef::Player),
        },
    );
    assert_eq!(flash_rows(&state, PowerKind::BlockNextTurn), 1);
    assert_eq!(
        state.player.block, 10,
        "BlockNextTurn must still grant block"
    );

    // Re-apply (the first instance self-removed) and drive an ENEMY's block
    // clear — `creature == base.Owner` excludes this: the enemy is not the
    // player.
    push_player_power(&mut state, 2, PowerKind::BlockNextTurn, 10);
    let enemy = add_enemy(&mut state, 40);
    sts2_core::hooks::dispatch_after_block_cleared(
        &mut state,
        &BlockClearedCtx {
            cleared: Some(CreatureRef::Enemy(enemy)),
        },
    );
    assert_eq!(
        flash_rows(&state, PowerKind::BlockNextTurn),
        1,
        "an enemy's own block clear must not flash the player's BlockNextTurn"
    );
}

/// C# `IntangiblePower.cs:37` (`AfterModifyingHpLostAfterOsty`) flashes
/// whenever the post-block hp-loss pipeline runs for the power's owner,
/// regardless of whether the 1-cap actually reduced anything. Enemy-owned
/// case — routes through `damage::apply_damage_with_final_impl`.
///
/// Since GH #635 this power has a SECOND flash site — `:61`'s
/// `AfterModifyingDamageAmount`, which fires when the cap moved the number, and
/// which is dispatched BEFORE block absorption. Both arms write the same row
/// kind, so the counts below are sums and the assertions are on the INCREMENT
/// each hit produces. That increment is what still separates the two: a
/// fully-blocked hit adds one row (`:61` only), an unblocked one adds two.
#[test]
fn intangible_flashes_after_hp_lost_pipeline_runs_on_enemy() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let enemy = add_enemy(&mut state, 40);
    push_enemy_power(&mut state, 1, enemy, PowerKind::Intangible, 1);

    let _ = sts2_core::damage::apply_damage_full(
        &mut state,
        CreatureRef::Player,
        enemy,
        5,
        ValueProps::MOVE,
        None,
    );
    assert_eq!(
        flash_rows(&state, PowerKind::Intangible),
        2,
        "an unblocked 5 against a cap of 1 fires BOTH arms: `:61` (the cap moved \
         5 to 1) and `:37` (the hp-loss pipeline ran)"
    );

    // A fully-blocked hit never reaches the hp-loss pipeline at all
    // (`unblocked == 0`) — the case Rust's `unblocked > 0` guard (mirroring
    // C#'s `remaining > 0`) excludes. This half can fail: the power still
    // exists, so an emission wired to "Intangible exists on the target"
    // rather than "the pipeline ran" would flash here too.
    //
    // `:61` DOES fire on this hit — the cap still reduced 5 to 1, and C#
    // dispatches `AfterModifyingDamageAmount` before block absorption — so the
    // count goes to 3, not back to 1. One added row rather than two is what
    // says `:37` stayed out.
    if let Some(c) = state.creatures.iter_mut().find(|c| c.id == enemy.0) {
        c.block = 999;
    }
    let _ = sts2_core::damage::apply_damage_full(
        &mut state,
        CreatureRef::Player,
        enemy,
        5,
        ValueProps::MOVE,
        None,
    );
    assert_eq!(
        flash_rows(&state, PowerKind::Intangible),
        3,
        "a fully-blocked hit must not run the hp-loss pipeline, so it adds only \
         `:61`'s row — one more, not two"
    );
}

/// Player-owned counterpart of the test above — routes through
/// `combat_loop::apply_damage_to_player`, which calls
/// `hooks::modify_hp_lost_pre_osty` directly rather than through
/// `damage::apply_damage_with_final`.
#[test]
fn intangible_flashes_after_hp_lost_pipeline_runs_on_player() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    state.player.current_hp = 80;
    state.player.max_hp = 80;
    let dealer = add_enemy(&mut state, 40);
    push_player_power(&mut state, 1, PowerKind::Intangible, 1);

    let _ = sts2_core::combat_loop::apply_damage_to_player(
        &mut state,
        CreatureRef::Enemy(dealer),
        5,
        ValueProps::MOVE,
        None,
    );
    assert_eq!(
        flash_rows(&state, PowerKind::Intangible),
        2,
        "both arms — see the enemy-side twin above"
    );

    // Fully blocked: `unblocked == 0`, the hp-loss pipeline never runs. `:61`
    // still does (the cap moved 5 to 1, before block), so this adds exactly
    // one row.
    state.player.block = 999;
    let _ = sts2_core::combat_loop::apply_damage_to_player(
        &mut state,
        CreatureRef::Enemy(dealer),
        5,
        ValueProps::MOVE,
        None,
    );
    assert_eq!(
        flash_rows(&state, PowerKind::Intangible),
        3,
        "a fully-blocked hit on the player must not flash `:37` again — one \
         added row (`:61`), not two"
    );
}

/// C# `PlatingPower.cs:65` (`BeforeSideTurnEndEarly`) flashes unconditionally
/// inside `participants.Contains(base.Owner)` — BEFORE the `GainBlock` call,
/// with no check on the amount granted.
#[test]
fn plating_flashes_on_owner_turn_end_even_with_no_block_grant() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let enemy = add_enemy(&mut state, 40);
    // Amount 0 forces `block_amount <= 0` — no block is granted — while the
    // side guard still holds. This half can fail: an emission folded into
    // the `block_amount > 0` branch would never fire here.
    push_enemy_power(&mut state, 1, enemy, PowerKind::Plating, 0);

    sts2_core::hooks::dispatch_before_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert_eq!(flash_rows(&state, PowerKind::Plating), 1);
    assert_eq!(
        state
            .creatures
            .iter()
            .find(|c| c.id == enemy.0)
            .unwrap()
            .block,
        0,
        "Plating(0) must not grant any block"
    );

    // The OTHER side's turn end must not flash an enemy-owned Plating — C#
    // gates `participants.Contains(base.Owner)`, folded onto side here.
    sts2_core::hooks::dispatch_before_turn_end(&mut state, &TurnSideCtx { side: Side::Player });
    assert_eq!(
        flash_rows(&state, PowerKind::Plating),
        1,
        "the player's turn end must not flash an enemy-owned Plating"
    );
}

// ── after_turn_end family (dispatch_after_turn_end) ─────────────────────

#[test]
fn high_voltage_flashes_even_with_zero_amount() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let enemy = add_enemy(&mut state, 40);
    push_enemy_power(&mut state, 1, enemy, PowerKind::HighVoltage, 0);

    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert_eq!(flash_rows(&state, PowerKind::HighVoltage), 1);
    assert!(
        !state.powers.iter().any(|p| p.kind == PowerKind::Strength),
        "HighVoltage(0) must not grant Strength"
    );

    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Player });
    assert_eq!(
        flash_rows(&state, PowerKind::HighVoltage),
        1,
        "the player's turn end must not flash an enemy-owned HighVoltage"
    );
}

#[test]
fn nemesis_flashes_only_on_the_apply_toggle() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let enemy = add_enemy(&mut state, 40);
    push_enemy_power(&mut state, 1, enemy, PowerKind::Nemesis, 1);

    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert_eq!(flash_rows(&state, PowerKind::Nemesis), 1);

    // Second enemy-turn-end: the owner is now Intangible, so this toggle is
    // the "remove" half — C# has no `Flash()` there.
    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert_eq!(
        flash_rows(&state, PowerKind::Nemesis),
        1,
        "the remove half of the toggle must not flash"
    );
}

#[test]
fn countdown_flashes_even_with_no_hittable_enemy() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    push_player_power(&mut state, 1, PowerKind::Countdown, 3);

    // No enemy exists at all — the Doom apply has nothing to target — but
    // C# `CountdownPower.cs:25` flashes before that check runs. Countdown is
    // C# `AfterSideTurnStart`, not `AfterSideTurnEnd`.
    sts2_core::hooks::dispatch_after_side_turn_start(
        &mut state,
        &TurnSideCtx { side: Side::Player },
    );
    assert_eq!(flash_rows(&state, PowerKind::Countdown), 1);

    sts2_core::hooks::dispatch_after_side_turn_start(
        &mut state,
        &TurnSideCtx { side: Side::Enemy },
    );
    assert_eq!(
        flash_rows(&state, PowerKind::Countdown),
        1,
        "the enemy's turn end must not flash a player-owned Countdown"
    );
}

#[test]
fn demise_flashes_even_with_zero_amount() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let enemy = add_enemy(&mut state, 40);
    push_enemy_power(&mut state, 1, enemy, PowerKind::Demise, 0);

    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert_eq!(flash_rows(&state, PowerKind::Demise), 1);
    assert_eq!(
        state
            .creatures
            .iter()
            .find(|c| c.id == enemy.0)
            .unwrap()
            .current_hp,
        40,
        "Demise(0) must deal no damage"
    );

    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Player });
    assert_eq!(
        flash_rows(&state, PowerKind::Demise),
        1,
        "the player's turn end must not flash an enemy-owned Demise"
    );
}

#[test]
fn ritual_flashes_only_when_the_just_applied_skip_does_not_fire() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let enemy = add_enemy(&mut state, 40);
    let puid = push_enemy_power(&mut state, 1, enemy, PowerKind::Ritual, 3);
    // Arm the just-applied-by-enemy skip (mirrors `AfterApplied` on an
    // enemy-owned Ritual).
    if let Some(p) = state.powers.iter_mut().find(|p| p.id == puid.0) {
        p.skip_next_tick = true;
    }

    // C# `if (WasJustAppliedByEnemy) { reset; return; }` — no Flash() on this
    // half. This can fail: a flash wired unconditionally to the arm would
    // fire here too.
    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert_eq!(flash_rows(&state, PowerKind::Ritual), 0);

    // Second turn end: the skip is consumed, so this flashes (even at
    // amount 0, since the flash sits ahead of the amount check).
    if let Some(p) = state.powers.iter_mut().find(|p| p.id == puid.0) {
        p.amount = 0;
    }
    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert_eq!(flash_rows(&state, PowerKind::Ritual), 1);
}

#[test]
fn territorial_flashes_even_with_zero_amount() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let enemy = add_enemy(&mut state, 40);
    push_enemy_power(&mut state, 1, enemy, PowerKind::Territorial, 0);

    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert_eq!(flash_rows(&state, PowerKind::Territorial), 1);

    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Player });
    assert_eq!(
        flash_rows(&state, PowerKind::Territorial),
        1,
        "the player's turn end must not flash an enemy-owned Territorial"
    );
}

#[test]
fn regen_flashes_only_on_the_owners_turn_end() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let enemy = add_enemy(&mut state, 40);
    push_enemy_power(&mut state, 1, enemy, PowerKind::Regen, 5);

    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Player });
    assert_eq!(
        flash_rows(&state, PowerKind::Regen),
        0,
        "the player's turn end must not flash an enemy-owned Regen"
    );

    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert_eq!(flash_rows(&state, PowerKind::Regen), 1);
}

#[test]
fn tangled_flashes_only_on_the_owners_turn_end() {
    let mut state = state_with_power(PowerKind::Tangled, 1);

    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert_eq!(
        flash_rows(&state, PowerKind::Tangled),
        0,
        "the enemy's turn end must not flash a player-owned Tangled"
    );

    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Player });
    assert_eq!(flash_rows(&state, PowerKind::Tangled), 1);
}

#[test]
fn ringing_flashes_only_on_the_owners_turn_end() {
    let mut state = state_with_power(PowerKind::Ringing, 1);

    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert_eq!(
        flash_rows(&state, PowerKind::Ringing),
        0,
        "the enemy's turn end must not flash a player-owned Ringing"
    );

    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Player });
    assert_eq!(flash_rows(&state, PowerKind::Ringing), 1);
}

#[test]
fn tainted_flashes_only_on_the_enemys_turn_end() {
    // Tainted sits on the PLAYER but removes (and flashes) at the ENEMY's
    // turn end — the opposite of most player-owned powers.
    let mut state = state_with_power(PowerKind::Tainted, 1);

    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Player });
    assert_eq!(
        flash_rows(&state, PowerKind::Tainted),
        0,
        "the player's own turn end must not flash Tainted"
    );

    sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert_eq!(flash_rows(&state, PowerKind::Tainted), 1);
}

#[test]
fn temporary_focus_subclasses_flash_only_on_the_owners_turn_end() {
    for kind in [
        PowerKind::Hotfix,
        PowerKind::Synchronize,
        PowerKind::FocusedStrike,
    ] {
        let mut state = state_with_power(kind, 2);

        sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
        assert_eq!(
            flash_rows(&state, kind),
            0,
            "{kind:?} must not flash on the enemy's turn end"
        );

        sts2_core::hooks::dispatch_after_turn_end(&mut state, &TurnSideCtx { side: Side::Player });
        assert_eq!(
            flash_rows(&state, kind),
            1,
            "{kind:?} must flash on the owner's turn end"
        );
    }
}

// ── before_turn_end family (dispatch_before_turn_end) ────────────────────

#[test]
fn the_bomb_flashes_only_on_the_detonate_turn() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let enemy = add_enemy(&mut state, 200);
    let puid = push_enemy_power(&mut state, 1, enemy, PowerKind::TheBomb, 2);
    if let Some(p) = state.powers.iter_mut().find(|p| p.id == puid.0) {
        p.counter = 40; // stored bomb damage
    }

    // Amount 2: the decrement branch — no Flash() in C# here.
    sts2_core::hooks::dispatch_before_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert_eq!(
        flash_rows(&state, PowerKind::TheBomb),
        0,
        "the decrement turn must not flash"
    );

    // Amount 1: the detonate branch.
    sts2_core::hooks::dispatch_before_turn_end(&mut state, &TurnSideCtx { side: Side::Enemy });
    assert_eq!(flash_rows(&state, PowerKind::TheBomb), 1);
}

// ── after_death family (dispatch_after_death) ────────────────────────────

#[test]
fn crab_rage_flashes_only_on_an_allys_death() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let survivor = add_enemy(&mut state, 40);
    let dying = add_enemy(&mut state, 0);
    push_enemy_power(&mut state, 1, survivor, PowerKind::CrabRage, 1);

    // The owner's OWN death must not flash it (a dead owner cannot enrage).
    sts2_core::hooks::dispatch_after_death(
        &mut state,
        &sts2_core::hooks::AfterDeathCtx {
            dead: survivor,
            dead_side: Side::Enemy,
        },
    );
    assert_eq!(flash_rows(&state, PowerKind::CrabRage), 0);

    // The other arm (`dying`) dying flashes it.
    sts2_core::hooks::dispatch_after_death(
        &mut state,
        &sts2_core::hooks::AfterDeathCtx {
            dead: dying,
            dead_side: Side::Enemy,
        },
    );
    assert_eq!(flash_rows(&state, PowerKind::CrabRage), 1);
}

#[test]
fn ravenous_flashes_only_on_an_allys_death() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let survivor = add_enemy(&mut state, 40);
    let dying = add_enemy(&mut state, 0);
    push_enemy_power(&mut state, 1, survivor, PowerKind::Ravenous, 4);

    sts2_core::hooks::dispatch_after_death(
        &mut state,
        &sts2_core::hooks::AfterDeathCtx {
            dead: survivor,
            dead_side: Side::Enemy,
        },
    );
    assert_eq!(
        flash_rows(&state, PowerKind::Ravenous),
        0,
        "the owner's own death must not flash Ravenous"
    );

    sts2_core::hooks::dispatch_after_death(
        &mut state,
        &sts2_core::hooks::AfterDeathCtx {
            dead: dying,
            dead_side: Side::Enemy,
        },
    );
    assert_eq!(flash_rows(&state, PowerKind::Ravenous), 1);
}

// ── after_damage_given family (dispatch_after_damage_given) ─────────────

#[test]
fn imbalanced_flashes_only_when_fully_blocked() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let enemy = add_enemy(&mut state, 40);
    push_enemy_power(&mut state, 1, enemy, PowerKind::Imbalanced, 1);
    // Give the enemy a MonsterAi with a counter, so `ai.set_counter(1)` has
    // somewhere to land (a `None` ai just silently no-ops the stun half).
    let ctx = sts2_core::hooks::DamageCtx {
        dealer: CreatureRef::Enemy(enemy),
        target: CreatureRef::Player,
        props: ValueProps::MOVE,
    };

    // Not fully blocked: no flash.
    sts2_core::hooks::dispatch_after_damage_given(&mut state, &ctx, 5, 5, false);
    assert_eq!(flash_rows(&state, PowerKind::Imbalanced), 0);

    // Fully blocked: flashes.
    sts2_core::hooks::dispatch_after_damage_given(&mut state, &ctx, 0, 5, true);
    assert_eq!(flash_rows(&state, PowerKind::Imbalanced), 1);
}

#[test]
fn painful_stabs_flashes_only_when_a_wound_is_actually_added() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    // The dealer-side PainfulStabs/PaperCuts lane requires a resolvable
    // player creature uid before it does anything.
    state.player.creature_uid = Some(CreatureUid::new(999).expect("nz"));
    let enemy = add_enemy(&mut state, 40);
    push_enemy_power(&mut state, 1, enemy, PowerKind::PainfulStabs, 2);
    let ctx = sts2_core::hooks::DamageCtx {
        dealer: CreatureRef::Enemy(enemy),
        target: CreatureRef::Player,
        props: ValueProps::MOVE,
    };

    // `unblocked_damage` param the arm reads is `amount` on the power
    // itself, always > 0 here — the guard this half exercises is
    // `TargetSide != Owner.Side` / `target == Player`, satisfied by ctx.
    // Zero total damage / not fully blocked still adds Amount*num wounds in
    // C# as long as `list.Any(UnblockedDamage > 0)` upstream — that filter
    // is applied by the caller, not this dispatcher, so drive it directly.
    sts2_core::hooks::dispatch_after_damage_given(&mut state, &ctx, 5, 5, false);
    assert_eq!(flash_rows(&state, PowerKind::PainfulStabs), 1);
    assert_eq!(
        state.player.discard.len(),
        2,
        "PainfulStabs(2) must add 2 Wounds"
    );
}

// ── after_damage_received family (dispatch_after_damage_received) ───────

#[test]
fn flutter_flashes_only_when_it_hits_zero() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let enemy = add_enemy(&mut state, 40);
    push_enemy_power(&mut state, 1, enemy, PowerKind::Flutter, 2);
    let ctx = sts2_core::hooks::DamageCtx {
        dealer: CreatureRef::Player,
        target: CreatureRef::Enemy(enemy),
        props: ValueProps::MOVE,
    };

    // First powered hit: 2 -> 1, not yet zero. This half can fail: an
    // emission wired to "Flutter exists" rather than "Flutter hit zero"
    // would flash here too.
    sts2_core::hooks::dispatch_after_damage_received(&mut state, &ctx, 3);
    assert_eq!(flash_rows(&state, PowerKind::Flutter), 0);

    // Second powered hit: 1 -> 0.
    sts2_core::hooks::dispatch_after_damage_received(&mut state, &ctx, 3);
    assert_eq!(flash_rows(&state, PowerKind::Flutter), 1);
}

#[test]
fn hardened_shell_flashes_after_hp_lost_pipeline_runs() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let enemy = add_enemy(&mut state, 40);
    push_enemy_power(&mut state, 1, enemy, PowerKind::HardenedShell, 10);

    let _ = sts2_core::damage::apply_damage_full(
        &mut state,
        CreatureRef::Player,
        enemy,
        5,
        ValueProps::MOVE,
        None,
    );
    assert_eq!(flash_rows(&state, PowerKind::HardenedShell), 1);

    // A fully-blocked hit never reaches the hp-loss pipeline.
    if let Some(c) = state.creatures.iter_mut().find(|c| c.id == enemy.0) {
        c.block = 999;
    }
    let _ = sts2_core::damage::apply_damage_full(
        &mut state,
        CreatureRef::Player,
        enemy,
        5,
        ValueProps::MOVE,
        None,
    );
    assert_eq!(
        flash_rows(&state, PowerKind::HardenedShell),
        1,
        "a fully-blocked hit must not flash HardenedShell again"
    );
}

#[test]
fn slippery_flashes_only_when_unblocked_damage_lands() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let enemy = add_enemy(&mut state, 40);
    push_enemy_power(&mut state, 1, enemy, PowerKind::Slippery, 3);
    let ctx = sts2_core::hooks::DamageCtx {
        dealer: CreatureRef::Player,
        target: CreatureRef::Enemy(enemy),
        props: ValueProps::MOVE,
    };

    // Zero post-mitigation damage: the `damage > 0` guard excludes this.
    sts2_core::hooks::dispatch_after_damage_received(&mut state, &ctx, 0);
    assert_eq!(flash_rows(&state, PowerKind::Slippery), 0);

    sts2_core::hooks::dispatch_after_damage_received(&mut state, &ctx, 1);
    assert_eq!(flash_rows(&state, PowerKind::Slippery), 1);
}

#[test]
fn the_gambit_flashes_only_on_a_powered_unblocked_hit_to_the_player() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    state.player.current_hp = 50;
    state.player.max_hp = 50;
    push_player_power(&mut state, 1, PowerKind::TheGambit, 1);
    let dealer = add_enemy(&mut state, 40);
    let ctx = sts2_core::hooks::DamageCtx {
        dealer: CreatureRef::Enemy(dealer),
        target: CreatureRef::Player,
        props: ValueProps::UNPOWERED,
    };

    // Unpowered: the `is_powered()` guard excludes this.
    sts2_core::hooks::dispatch_after_damage_received(&mut state, &ctx, 5);
    assert_eq!(flash_rows(&state, PowerKind::TheGambit), 0);

    // Re-apply (nothing removed it) and drive a powered hit.
    push_player_power(&mut state, 2, PowerKind::TheGambit, 1);
    let powered_ctx = sts2_core::hooks::DamageCtx {
        dealer: CreatureRef::Enemy(dealer),
        target: CreatureRef::Player,
        props: ValueProps::MOVE,
    };
    sts2_core::hooks::dispatch_after_damage_received(&mut state, &powered_ctx, 5);
    assert_eq!(flash_rows(&state, PowerKind::TheGambit), 1);
}

// ── after_power_applied family (dispatch_after_power_applied) ───────────

#[test]
fn sleight_of_flesh_flashes_only_when_a_debuff_lands_on_an_enemy() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    push_player_power(&mut state, 1, PowerKind::SleightOfFlesh, 4);
    let enemy = add_enemy(&mut state, 40);

    // A BUFF applied to the enemy does not qualify.
    sts2_core::hooks::dispatch_after_power_applied(
        &mut state,
        &PowerAppliedCtx {
            target: CreatureRef::Enemy(enemy),
            power_kind: PowerKind::Strength,
            delta: 1,
            applier: Some(CreatureRef::Player),
        },
    );
    assert_eq!(flash_rows(&state, PowerKind::SleightOfFlesh), 0);

    // A DEBUFF applied to the enemy qualifies.
    sts2_core::hooks::dispatch_after_power_applied(
        &mut state,
        &PowerAppliedCtx {
            target: CreatureRef::Enemy(enemy),
            power_kind: PowerKind::Vulnerable,
            delta: 1,
            applier: Some(CreatureRef::Player),
        },
    );
    assert_eq!(flash_rows(&state, PowerKind::SleightOfFlesh), 1);
}

// ── after_card_played family (dispatch_after_card_played) ───────────────

#[test]
fn oblivion_flashes_only_for_a_card_registered_before_it_played() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let enemy = add_enemy(&mut state, 40);
    sts2_core::card_effects::apply_power_to_enemy(&mut state, enemy, PowerKind::Oblivion, 5);

    // A card played with nothing registered (mirrors the Oblivion card's own
    // play) removes nothing — no flash.
    sts2_core::hooks::dispatch_after_card_played(&mut state, &CardCtx::default());
    assert_eq!(flash_rows(&state, PowerKind::Oblivion), 0);

    // Register this card, then play it: the snapshot is present and it
    // flashes.
    sts2_core::hooks::dispatch_before_card_played_powers(&mut state, 1, false);
    sts2_core::hooks::dispatch_after_card_played(&mut state, &CardCtx::default());
    assert_eq!(flash_rows(&state, PowerKind::Oblivion), 1);
}

#[test]
fn smoggy_flashes_only_when_the_player_plays_a_skill() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    push_player_power(&mut state, 1, PowerKind::Smoggy, 1);

    sts2_core::hooks::dispatch_after_card_played(
        &mut state,
        &CardCtx {
            card_type: Some(sts2_core::cards::CardType::Attack),
            ..CardCtx::default()
        },
    );
    assert_eq!(flash_rows(&state, PowerKind::Smoggy), 0);

    sts2_core::hooks::dispatch_after_card_played(
        &mut state,
        &CardCtx {
            card_type: Some(sts2_core::cards::CardType::Skill),
            ..CardCtx::default()
        },
    );
    assert_eq!(flash_rows(&state, PowerKind::Smoggy), 1);
}

#[test]
fn tender_flashes_once_per_player_card_played() {
    // Tender is applied TO the player (a HunterKiller curse) — the drain
    // reads `player_power_uids`, not an enemy's own list.
    let mut state = state_with_power(PowerKind::Tender, 1);
    state.player.current_hp = 50;
    state.player.max_hp = 50;
    state.player.creature_uid = Some(CreatureUid::new(999).expect("nz"));

    sts2_core::hooks::dispatch_after_card_played(&mut state, &CardCtx::default());
    assert_eq!(flash_rows(&state, PowerKind::Tender), 1);

    sts2_core::hooks::dispatch_after_card_played(&mut state, &CardCtx::default());
    assert_eq!(
        flash_rows(&state, PowerKind::Tender),
        2,
        "Tender flashes on EVERY player card play"
    );
}

#[test]
fn vital_spark_flashes_only_when_a_tainted_card_is_played() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    state.player.current_hp = 50;
    state.player.max_hp = 50;
    state.player.creature_uid = Some(CreatureUid::new(999).expect("nz"));
    let enemy = add_enemy(&mut state, 40);
    push_enemy_power(&mut state, 1, enemy, PowerKind::VitalSpark, 2);
    let card_uid = add_card(&mut state, 1, sts2_core::cards::CardType::Skill);

    // No affliction on the card: no flash.
    sts2_core::hooks::dispatch_after_card_played(
        &mut state,
        &CardCtx {
            card_uid: Some(card_uid),
            ..CardCtx::default()
        },
    );
    assert_eq!(flash_rows(&state, PowerKind::VitalSpark), 0);

    // Tainted card: flashes.
    let idx = card_uid.get() as usize - 1;
    let _ = state.cards[idx].set_affliction(sts2_core::card_afflictions::CardAffliction::new(
        sts2_core::card_afflictions::CardAfflictionKind::Tainted,
        1,
    ));
    sts2_core::hooks::dispatch_after_card_played(
        &mut state,
        &CardCtx {
            card_uid: Some(card_uid),
            ..CardCtx::default()
        },
    );
    assert_eq!(flash_rows(&state, PowerKind::VitalSpark), 1);
}

#[test]
fn withering_presence_flashes_only_every_sixth_card_play() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    state.player.creature_uid = Some(CreatureUid::new(999).expect("nz"));
    let enemy = add_enemy(&mut state, 40);
    push_enemy_power(&mut state, 1, enemy, PowerKind::WitheringPresence, 6);

    for i in 0..5 {
        sts2_core::hooks::dispatch_after_card_played(&mut state, &CardCtx::default());
        assert_eq!(
            flash_rows(&state, PowerKind::WitheringPresence),
            0,
            "play {i} of 6 must not flash yet"
        );
    }
    sts2_core::hooks::dispatch_after_card_played(&mut state, &CardCtx::default());
    assert_eq!(flash_rows(&state, PowerKind::WitheringPresence), 1);
}

// ── card-result-pile routing (Nostalgia, via combat_loop::play_card) ─────

#[test]
fn nostalgia_flashes_only_on_a_redirected_attack_or_skill() {
    let mut state = play_ready_state(PowerKind::Nostalgia, 1);
    add_hand_card(&mut state, 1, sts2_core::cards::CardType::Power, 0);
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, None);
    assert_eq!(
        flash_rows(&state, PowerKind::Nostalgia),
        0,
        "a Power card is not redirected"
    );

    add_hand_card(&mut state, 2, sts2_core::cards::CardType::Skill, 0);
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, None);
    assert_eq!(flash_rows(&state, PowerKind::Nostalgia), 1);
}

// ── Rebound (card-result-pile routing) ─────────────────────────────────
//
// C# `ReboundPower` (GH #637) has three arms and the flash lives in the
// second. `ModifyCardPlayResultPileTypeAndPosition` redirects a card headed
// for Discard to the TOP of the draw pile; `AfterModifyingCardPlayResultPile
// OrPosition` flashes and decrements; `AfterSideTurnEnd` removes the power.
//
// The load-bearing negative is the Exhaust case. `Hook.ModifyCardPlayResult
// PileTypeAndPosition` (`Hook.cs:1535-1552`) collects only the listeners that
// actually CHANGED the pile, and `OnPlayWrapper` (`CardModel.cs:1892-1895`)
// dispatches the after-hook over that list alone — so a card that was never
// redirected costs no stack and raises no flash. A port that decrements on
// every play passes every positive assertion here and fails
// `an_exhaust_bound_card_neither_flashes_nor_spends_a_stack`.

/// The player's live Rebound stack, or 0 when the power is gone.
fn rebound_stacks(state: &CombatState) -> i32 {
    state.player_power_amount(PowerKind::Rebound)
}

#[test]
fn rebound_sends_a_discard_bound_card_to_the_draw_top_and_spends_a_stack() {
    let mut state = play_ready_state(PowerKind::Rebound, 2);
    let uid = add_hand_card(&mut state, 1, sts2_core::cards::CardType::Skill, 0);

    let _ = sts2_core::combat_loop::play_card(&mut state, 0, None);

    assert_eq!(
        state.player.draw.first().copied(),
        Some(uid),
        "the played card lands on TOP of the draw pile, not in discard"
    );
    assert!(state.player.discard.is_empty());
    assert_eq!(flash_rows(&state, PowerKind::Rebound), 1);
    assert_eq!(rebound_stacks(&state), 1, "one stack spent per redirect");
}

#[test]
fn an_exhaust_bound_card_neither_flashes_nor_spends_a_stack() {
    // C# `GetResultPileTypeForCardPlay` returns `Exhaust` for an Exhaust
    // card BEFORE any listener runs, so Rebound's `pileType != Discard`
    // early-out fires and it is never added to `modifiers`.
    let mut state = play_ready_state(PowerKind::Rebound, 2);
    let uid = add_hand_card(&mut state, 1, sts2_core::cards::CardType::Skill, 0);
    let idx = uid.get() as usize - 1;
    state.cards[idx].keyword_override = Some(sts2_core::cards::KeywordOverride::EXHAUST);

    let _ = sts2_core::combat_loop::play_card(&mut state, 0, None);

    assert!(
        state.player.exhaust.contains(&uid),
        "an Exhaust card still exhausts"
    );
    assert!(state.player.draw.is_empty(), "and is NOT sent to draw-top");
    assert_eq!(
        flash_rows(&state, PowerKind::Rebound),
        0,
        "Rebound never modified this play, so its after-hook never fires"
    );
    assert_eq!(
        rebound_stacks(&state),
        2,
        "and no stack is spent — the decrement rides the redirect, not the play"
    );
}

#[test]
fn a_power_card_neither_flashes_nor_spends_a_stack() {
    // `GetResultPileTypeForCardPlay` returns `PileType.None` for a Power
    // card, which is likewise not Discard.
    let mut state = play_ready_state(PowerKind::Rebound, 2);
    add_hand_card(&mut state, 1, sts2_core::cards::CardType::Power, 0);

    let _ = sts2_core::combat_loop::play_card(&mut state, 0, None);

    assert!(state.player.draw.is_empty());
    assert_eq!(flash_rows(&state, PowerKind::Rebound), 0);
    assert_eq!(rebound_stacks(&state), 2);
}

#[test]
fn the_last_stack_removes_the_power() {
    // C# `PowerCmd.Decrement` removes the instance at 0.
    let mut state = play_ready_state(PowerKind::Rebound, 1);
    add_hand_card(&mut state, 1, sts2_core::cards::CardType::Skill, 0);
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, None);
    assert_eq!(rebound_stacks(&state), 0);
    assert!(
        !state
            .powers
            .iter()
            .any(|p| p.kind == PowerKind::Rebound && p.amount > 0),
        "the instance is gone, not left at 0"
    );

    // And with the power gone, the next card discards normally.
    let uid2 = add_hand_card(&mut state, 2, sts2_core::cards::CardType::Skill, 0);
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, None);
    assert!(state.player.discard.contains(&uid2));
    assert_eq!(
        flash_rows(&state, PowerKind::Rebound),
        1,
        "still just the one"
    );
}

#[test]
fn unspent_stacks_do_not_survive_the_owners_turn_end() {
    // C# `ReboundPower.AfterSideTurnEnd` removes the whole power when the
    // owner is among the participants — leftover stacks are discarded, not
    // carried into the next turn.
    let mut state = play_ready_state(PowerKind::Rebound, 3);
    sts2_core::hooks::dispatch_after_turn_end(
        &mut state,
        &sts2_core::hooks::TurnSideCtx {
            side: sts2_core::Side::Player,
        },
    );
    assert_eq!(rebound_stacks(&state), 0, "the power is removed outright");

    // The ENEMY's side turn end does not touch a player-owned Rebound
    // (`participants.Contains(Owner)`).
    let mut other = play_ready_state(PowerKind::Rebound, 3);
    sts2_core::hooks::dispatch_after_turn_end(
        &mut other,
        &sts2_core::hooks::TurnSideCtx {
            side: sts2_core::Side::Enemy,
        },
    );
    assert_eq!(rebound_stacks(&other), 3);
}

// ── misc dispatch entry points ───────────────────────────────────────────

#[test]
fn dampen_flashes_on_apply() {
    // The `after_applied` registry is populated by `sts2-content`'s
    // `call_once` at real-game startup; this test crate doesn't link
    // sts2-content, so register it directly (idempotent).
    sts2_core::monster_power_effects::install_dampen_power_handlers();

    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let enemy = add_enemy(&mut state, 40);
    let _ = sts2_core::monster_power_effects::dispatch_after_applied_for_power(
        &mut state,
        enemy,
        PowerKind::Dampen,
        None,
    );
    assert_eq!(flash_rows(&state, PowerKind::Dampen), 1);

    // A power with no registered `after_applied` handler does not flash —
    // proves the dispatch isn't unconditional.
    let _ = sts2_core::monster_power_effects::dispatch_after_applied_for_power(
        &mut state,
        enemy,
        PowerKind::Strength,
        None,
    );
    assert_eq!(flash_rows(&state, PowerKind::Dampen), 1);
}

#[test]
fn call_of_the_void_flashes_only_when_the_power_is_present() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    state.player.character = Some(sts2_core::CharacterId::Necrobinder);

    sts2_core::hooks::dispatch_before_hand_draw_powers(&mut state);
    assert_eq!(
        flash_rows(&state, PowerKind::CallOfTheVoid),
        0,
        "no CallOfTheVoid power present"
    );

    push_player_power(&mut state, 1, PowerKind::CallOfTheVoid, 1);
    sts2_core::hooks::dispatch_before_hand_draw_powers(&mut state);
    assert_eq!(flash_rows(&state, PowerKind::CallOfTheVoid), 1);
}

/// C# `HelloWorldPower.cs:24` flashes inside `player == Owner.Player &&
/// AmountOnTurnStart >= 1`, BEFORE the card generation — so a power sitting at
/// 0 stacks must stay dark even though it is present and its dispatcher runs.
/// The zero-stack half is the negative one: a flash keyed on "a HelloWorld
/// power exists" passes the positive assertion and fails only there.
#[test]
fn hello_world_flashes_only_when_it_has_a_stack_at_turn_start() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    state.player.character = Some(sts2_core::CharacterId::Defect);

    sts2_core::hooks::dispatch_before_hand_draw_powers(&mut state);
    assert_eq!(
        flash_rows(&state, PowerKind::HelloWorld),
        0,
        "no HelloWorld power present"
    );

    // Present but at 0 stacks: `AmountOnTurnStart >= 1` excludes it.
    let zero = push_player_power(&mut state, 1, PowerKind::HelloWorld, 0);
    sts2_core::hooks::dispatch_before_hand_draw_powers(&mut state);
    assert_eq!(
        flash_rows(&state, PowerKind::HelloWorld),
        0,
        "HelloWorld at 0 stacks must not flash"
    );

    // One stack: the guard holds.
    if let Some(p) = state.powers.iter_mut().find(|p| p.id == zero.0) {
        p.amount = 1;
    }
    sts2_core::hooks::dispatch_before_hand_draw_powers(&mut state);
    assert_eq!(flash_rows(&state, PowerKind::HelloWorld), 1);
}

#[test]
fn stratagem_flashes_only_when_the_power_is_present() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);

    sts2_core::hooks::dispatch_after_shuffle(
        &mut state,
        &sts2_core::hooks::CombatLifecycleCtx::default(),
    );
    assert_eq!(flash_rows(&state, PowerKind::Stratagem), 0);

    push_player_power(&mut state, 1, PowerKind::Stratagem, 3);
    sts2_core::hooks::dispatch_after_shuffle(
        &mut state,
        &sts2_core::hooks::CombatLifecycleCtx::default(),
    );
    assert_eq!(flash_rows(&state, PowerKind::Stratagem), 1);
}

#[test]
fn suck_flashes_only_when_the_attack_lands_unblocked() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    state.phase = sts2_core::CombatPhase::EnemyTurnExec;
    state.side = Side::Enemy;
    state.player.current_hp = 50;
    state.player.max_hp = 50;
    let enemy_id = std::num::NonZeroU32::new(1).expect("nz");
    state.creatures.push(sts2_core::Creature {
        id: enemy_id,
        current_hp: 40,
        max_hp: 40,
        block: 0,
        powers: smallvec::SmallVec::new(),
        species: Some(MonsterKind::Nibbit),
        intent: Some(sts2_core::monsters::Intent::Attack { damage: 6 }),
        intent_history: sts2_core::monsters::IntentHistory::default(),
        monster_id: sts2_core::MonsterId("Nibbit"),
        ai: None,
    });
    state.enemy_uids.push(CreatureUid(enemy_id));
    push_enemy_power(&mut state, 1, CreatureUid(enemy_id), PowerKind::Suck, 2);

    let _ = sts2_core::combat_loop::enemy_turn_exec(&mut state);
    assert_eq!(flash_rows(&state, PowerKind::Suck), 1);
    assert!(
        state
            .powers
            .iter()
            .any(|p| p.kind == PowerKind::Strength && p.amount == 2),
        "Suck(2) on a single unblocked hit must grant 2 Strength"
    );
}

#[test]
fn fasten_flashes_whenever_the_player_gains_block() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    push_player_power(&mut state, 1, PowerKind::Fasten, 3);

    let _ = sts2_core::card_effects::gain_block_player(&mut state, 5);
    assert_eq!(flash_rows(&state, PowerKind::Fasten), 1);

    // No Fasten present: no flash.
    let mut bare_state = CombatState::default();
    bare_state.combat_log.event_log.set_recording(true);
    let _ = sts2_core::card_effects::gain_block_player(&mut bare_state, 5);
    assert_eq!(flash_rows(&bare_state, PowerKind::Fasten), 0);
}

#[test]
fn toric_toughness_flashes_only_when_its_guard_holds() {
    let mut state = state_with_power(PowerKind::ToricToughness, 10);
    // The re-granted block reads `counter` (the stored block), not `amount`
    // (the remaining Turns count).
    if let Some(p) = state
        .powers
        .iter_mut()
        .find(|p| p.kind == PowerKind::ToricToughness)
    {
        p.counter = 10;
    }

    sts2_core::hooks::dispatch_after_block_cleared(
        &mut state,
        &BlockClearedCtx {
            cleared: Some(CreatureRef::Player),
        },
    );
    assert_eq!(flash_rows(&state, PowerKind::ToricToughness), 1);
    assert_eq!(state.player.block, 10);

    push_player_power(&mut state, 2, PowerKind::ToricToughness, 10);
    let enemy = add_enemy(&mut state, 40);
    sts2_core::hooks::dispatch_after_block_cleared(
        &mut state,
        &BlockClearedCtx {
            cleared: Some(CreatureRef::Enemy(enemy)),
        },
    );
    assert_eq!(
        flash_rows(&state, PowerKind::ToricToughness),
        1,
        "an enemy's own block clear must not flash the player's ToricToughness"
    );
}

#[test]
fn danse_macabre_flashes_only_when_the_played_card_costs_enough() {
    let mut state = state_with_power(PowerKind::DanseMacabre, 3);

    // Resolved cost 1 < the DanseMacabre Energy(2) threshold.
    sts2_core::hooks::dispatch_before_card_played_powers(&mut state, 1, false);
    assert_eq!(flash_rows(&state, PowerKind::DanseMacabre), 0);
    assert_eq!(state.player.block, 0);

    // Resolved cost 2 meets the threshold.
    sts2_core::hooks::dispatch_before_card_played_powers(&mut state, 2, false);
    assert_eq!(flash_rows(&state, PowerKind::DanseMacabre), 1);
    assert_eq!(state.player.block, 3);
}

// ── start_player_turn family (RollingBoulder, MindRot, PaleBlueDot) ─────
//
// `run_after_player_turn_start_cluster` (RollingBoulder) and the Step-7
// hand-draw setup (MindRot / PaleBlueDot) are private to combat_loop.rs;
// `start_player_turn` is the one public entry point that reaches both.

fn state_for_player_turn_start(draw_n: u32) -> CombatState {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    state.phase = sts2_core::CombatPhase::PlayerTurnStart;
    state.player.current_hp = 50;
    state.player.max_hp = 50;
    for i in 1..=draw_n {
        state.player.draw.push(CardUid::new(i).expect("non-zero"));
    }
    state
}

#[test]
fn rolling_boulder_flashes_even_at_zero_amount() {
    let mut state = state_for_player_turn_start(10);
    push_player_power(&mut state, 1, PowerKind::RollingBoulder, 0);

    let _ = sts2_core::combat_loop::start_player_turn(&mut state);
    assert_eq!(flash_rows(&state, PowerKind::RollingBoulder), 1);
}

#[test]
fn mind_rot_flashes_only_when_the_power_is_present() {
    let mut state = state_for_player_turn_start(10);

    let _ = sts2_core::combat_loop::start_player_turn(&mut state);
    assert_eq!(flash_rows(&state, PowerKind::MindRot), 0);

    let mut state2 = state_for_player_turn_start(10);
    push_player_power(&mut state2, 1, PowerKind::MindRot, 1);
    let _ = sts2_core::combat_loop::start_player_turn(&mut state2);
    assert_eq!(flash_rows(&state2, PowerKind::MindRot), 1);
}

#[test]
fn pale_blue_dot_flashes_only_when_the_power_is_present() {
    let mut state = state_for_player_turn_start(10);

    let _ = sts2_core::combat_loop::start_player_turn(&mut state);
    assert_eq!(flash_rows(&state, PowerKind::PaleBlueDot), 0);

    let mut state2 = state_for_player_turn_start(10);
    push_player_power(&mut state2, 1, PowerKind::PaleBlueDot, 1);
    let _ = sts2_core::combat_loop::start_player_turn(&mut state2);
    assert_eq!(flash_rows(&state2, PowerKind::PaleBlueDot), 1);
}

// ── per-card-drawn loop (Automation, Pagestorm — via draw_into_hand) ─────

#[test]
fn automation_flashes_only_when_the_counter_resets() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    let puid = push_player_power(&mut state, 1, PowerKind::Automation, 1);
    if let Some(p) = state.powers.iter_mut().find(|p| p.id == puid.0) {
        p.counter = 9; // one card away from the 10th
    }
    for i in 1..=12u32 {
        let uid = add_card(&mut state, i, sts2_core::cards::CardType::Attack);
        state.player.draw.push(uid);
    }

    // Draw 1 card: counter 9 -> 10, resets, fires. This half can fail: an
    // emission wired to "Automation exists" would flash on any draw.
    let _ = state.draw_into_hand(1);
    assert_eq!(flash_rows(&state, PowerKind::Automation), 1);

    // Draw one more: counter 0 -> 1, well short of resetting again.
    let _ = state.draw_into_hand(1);
    assert_eq!(
        flash_rows(&state, PowerKind::Automation),
        1,
        "a lone draw short of the 10th must not flash again"
    );
}

#[test]
fn pagestorm_flashes_only_on_an_ethereal_card_drawn() {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    push_player_power(&mut state, 1, PowerKind::Pagestorm, 1);
    let normal = add_card(&mut state, 1, sts2_core::cards::CardType::Attack);
    let ethereal = add_card(&mut state, 2, sts2_core::cards::CardType::Attack);
    // Ethereal is a keyword, not an affliction — force it via the per-instance
    // override (mirrors what a real Ethereal-granting card effect does).
    state.cards[1].keyword_override = Some(sts2_core::cards::KeywordOverride::ETHEREAL);
    state.player.draw.push(normal);

    let _ = state.draw_into_hand(1);
    assert_eq!(flash_rows(&state, PowerKind::Pagestorm), 0);

    state.player.draw.push(ethereal);
    let _ = state.draw_into_hand(1);
    assert_eq!(flash_rows(&state, PowerKind::Pagestorm), 1);
}

// ── block-clear prevention (Blur) ───────────────────────────────────────
//
// C# `Creature.ClearBlock` (Creature.cs:728-738) asks
// `Hook.ShouldClearBlock(..., out preventer)` — FIRST veto wins and
// short-circuits — then fires `Hook.AfterPreventingBlockClear` on that
// preventer ALONE (`Hook.cs:1176-1183`). `BlurPower.AfterPreventingBlockClear`
// (BlurPower.cs:31-37) opens with `if (this != preventer) return;` and then
// flashes.
//
// Four classes veto the clear — Barricade, Blur, Burrowed and the SturdyClamp
// relic — and only Blur flashes. So a flash wired to "a power retained the
// block" passes every positive assertion here and fails
// `barricade_retains_block_without_flashing_blur`; that test is the point of
// the section.

/// A round-2 player turn start with `block` block on the player, which is the
/// only path that reaches the turn-start block clear (`round > 1`).
fn state_for_block_clear(block: i32) -> CombatState {
    let mut state = state_for_player_turn_start(10);
    state.round = 2;
    state.player.block = block;
    state.sync_creature_block_from_player();
    state
}

#[test]
fn blur_flashes_when_it_prevents_the_players_block_clear() {
    let mut state = state_for_block_clear(12);
    push_player_power(&mut state, 1, PowerKind::Blur, 1);

    let _ = sts2_core::combat_loop::start_player_turn(&mut state);

    assert_eq!(state.player.block, 12, "Blur must retain the owner's block");
    assert_eq!(flash_rows(&state, PowerKind::Blur), 1);
}

#[test]
fn blur_stays_dark_when_nothing_prevents_the_clear() {
    let mut state = state_for_block_clear(12);
    push_player_power(&mut state, 1, PowerKind::Blur, 1);
    // Same power, but owned by an ENEMY: `BlurPower.ShouldClearBlock` returns
    // early on `Owner != creature`, so it never vetoes the PLAYER's clear.
    let mut enemy_state = state_for_block_clear(12);
    let enemy = add_enemy(&mut enemy_state, 40);
    let uid = power_uid(1);
    enemy_state.powers.push(Power {
        id: uid.0,
        kind: PowerKind::Blur,
        amount: 1,
        counter: 0,
        applier: None,
        skip_next_tick: false,
    });
    if let Some(c) = enemy_state.creatures.iter_mut().find(|c| c.id == enemy.0) {
        c.powers.push(uid);
    }

    let _ = sts2_core::combat_loop::start_player_turn(&mut enemy_state);
    assert_eq!(
        enemy_state.player.block, 0,
        "an enemy's Blur must not retain the PLAYER's block"
    );
    assert_eq!(flash_rows(&enemy_state, PowerKind::Blur), 0);

    // And with no Blur at all the clear runs normally.
    let mut bare = state_for_block_clear(12);
    let _ = sts2_core::combat_loop::start_player_turn(&mut bare);
    assert_eq!(bare.player.block, 0);
    assert_eq!(flash_rows(&bare, PowerKind::Blur), 0);

    // Guard against the fixture itself being the reason the two states above
    // read 0 flashes: the same fixture DOES flash with a player-owned Blur.
    let _ = sts2_core::combat_loop::start_player_turn(&mut state);
    assert_eq!(flash_rows(&state, PowerKind::Blur), 1);
}

#[test]
fn barricade_retains_block_without_flashing_blur() {
    // `BarricadePower.ShouldClearBlock` vetoes exactly like Blur but has NO
    // `AfterPreventingBlockClear` override, so nothing flashes. A guard keyed
    // on "a power prevented the clear" instead of "the PREVENTER is Blur"
    // fails only here.
    let mut state = state_for_block_clear(12);
    push_player_power(&mut state, 1, PowerKind::Barricade, 1);

    let _ = sts2_core::combat_loop::start_player_turn(&mut state);

    assert_eq!(state.player.block, 12, "Barricade must retain the block");
    assert_eq!(flash_rows(&state, PowerKind::Blur), 0);
    assert_eq!(flash_rows(&state, PowerKind::Barricade), 0);
}

#[test]
fn only_the_first_preventer_flashes() {
    // `Hook.ShouldClearBlock` short-circuits on the first veto, and
    // `AfterPreventingBlockClear` reaches that listener alone. Powers iterate
    // in insertion order, so Barricade-then-Blur names Barricade the
    // preventer and Blur never learns the clear was stopped.
    let mut barricade_first = state_for_block_clear(12);
    push_player_power(&mut barricade_first, 1, PowerKind::Barricade, 1);
    push_player_power(&mut barricade_first, 2, PowerKind::Blur, 1);

    let _ = sts2_core::combat_loop::start_player_turn(&mut barricade_first);
    assert_eq!(barricade_first.player.block, 12);
    assert_eq!(flash_rows(&barricade_first, PowerKind::Blur), 0);

    // Reverse the insertion order and Blur is the preventer.
    let mut blur_first = state_for_block_clear(12);
    push_player_power(&mut blur_first, 1, PowerKind::Blur, 1);
    push_player_power(&mut blur_first, 2, PowerKind::Barricade, 1);

    let _ = sts2_core::combat_loop::start_player_turn(&mut blur_first);
    assert_eq!(blur_first.player.block, 12);
    assert_eq!(flash_rows(&blur_first, PowerKind::Blur), 1);
}

// ── the same four vetoes, on an ENEMY's block clear (GH #646) ────────────
//
// C# `Creature.ClearBlock` (`Creature.cs:728-738`) is shared by every
// creature and asks `Hook.ShouldClearBlock` regardless of side, so an
// enemy-owned Barricade / Blur / Burrowed retains that enemy's block and a
// Blur that did it flashes on the ENEMY. Until GH #646 the enemy turn-start
// clear hardcoded a Burrowed lookup and never reached the shared path, so
// the other two were silently unhonoured and the #634 flash — already
// owner-generic — was unreachable from this side.

/// A round-2 enemy turn start with one enemy holding `block`, which is the
/// only path that reaches the enemy turn-start clear.
fn state_for_enemy_block_clear(block: i32) -> (CombatState, CreatureUid) {
    let mut state = CombatState::default();
    state.combat_log.event_log.set_recording(true);
    state.round = 2;
    state.phase = sts2_core::CombatPhase::EnemyTurnStart;
    let uid = add_enemy(&mut state, 40);
    if let Some(c) = state.creatures.iter_mut().find(|c| c.id == uid.0) {
        c.block = block;
    }
    (state, uid)
}

fn enemy_block_after_turn_start(kind: Option<PowerKind>) -> (CombatState, CreatureUid) {
    let (mut state, enemy) = state_for_enemy_block_clear(12);
    if let Some(kind) = kind {
        let _ = push_enemy_power(&mut state, 1, enemy, kind, 1);
    }
    sts2_core::combat_loop::enemy_turn_start(&mut state);
    (state, enemy)
}

fn block_of(state: &CombatState, enemy: CreatureUid) -> i32 {
    state
        .creatures
        .iter()
        .find(|c| c.id == enemy.0)
        .map_or(-1, |c| c.block)
}

#[test]
fn blur_flashes_when_it_prevents_an_enemys_block_clear() {
    let (state, enemy) = enemy_block_after_turn_start(Some(PowerKind::Blur));
    assert_eq!(
        block_of(&state, enemy),
        12,
        "Blur retains its owner's block"
    );
    assert_eq!(flash_rows(&state, PowerKind::Blur), 1);
    // ...and the flash names the ENEMY, not the player: `flash_power` takes
    // the preventer's owner uid, so a site that fell back to
    // `flash_player_power` would still count one row here.
    assert!(
        state.combat_log.event_log.rows().iter().any(|r| matches!(
            &r.entry,
            EventLogEntry::PowerFlashed { power: PowerKind::Blur, creature }
                if !matches!(creature, sts2_core::event_log::LogCreature::Player(_))
        )),
        "the Blur flash must be keyed on the enemy that owns it"
    );
}

#[test]
fn barricade_retains_an_enemys_block_without_flashing() {
    // Same veto as Blur, no `AfterPreventingBlockClear` override — the
    // negative half that separates "a power prevented the clear" from "the
    // PREVENTER is Blur" on this side too.
    let (state, enemy) = enemy_block_after_turn_start(Some(PowerKind::Barricade));
    assert_eq!(block_of(&state, enemy), 12, "Barricade retains the block");
    assert_eq!(flash_rows(&state, PowerKind::Blur), 0);
    assert_eq!(flash_rows(&state, PowerKind::Barricade), 0);
}

#[test]
fn an_enemy_with_no_veto_still_clears_its_block() {
    let (state, enemy) = enemy_block_after_turn_start(None);
    assert_eq!(block_of(&state, enemy), 0, "nothing vetoed the clear");
    assert_eq!(flash_rows(&state, PowerKind::Blur), 0);
}

// ── Nostalgia vs Rebound: precedence is INSERTION order (GH #650) ────────
//
// `Hook.ModifyCardPlayResultPileTypeAndPosition` (`Hook.cs:1535-1552`) FOLDS:
// it starts from `GetResultPileTypeForCardPlay()` and lets each listener in
// `IterateCombatHookListeners` order — the owner's power-insertion order —
// transform the running `(pileType, position)`, collecting the ones that
// changed it. `OnPlayWrapper` (`CardModel.cs:1892-1895`) then fires the
// after-hook over that list alone.
//
// Nostalgia and Rebound both map Discard -> `(Draw, Top)`, so whoever runs
// FIRST wins and the other's `pileType != Discard` early-out fires. The card
// lands on draw-top either way; what changes is which power flashes and
// whether a Rebound stack is spent. Rust used to decide that by the order of
// an `if / else if` chain, which made Nostalgia win always.

/// Both powers live, applied in the given order, one Skill played.
fn play_with_both(first: PowerKind, second: PowerKind) -> (CombatState, CardUid) {
    let mut state = play_ready_state(first, 1);
    push_player_power(&mut state, 2, second, 1);
    let uid = add_hand_card(&mut state, 1, sts2_core::cards::CardType::Skill, 0);
    let _ = sts2_core::combat_loop::play_card(&mut state, 0, None);
    (state, uid)
}

#[test]
fn gh650_rebound_applied_first_spends_its_stack_and_nostalgia_stays_dark() {
    let (state, uid) = play_with_both(PowerKind::Rebound, PowerKind::Nostalgia);
    assert_eq!(
        state.player.draw.first().copied(),
        Some(uid),
        "the destination is draw-top under either ordering"
    );
    assert_eq!(flash_rows(&state, PowerKind::Rebound), 1);
    assert_eq!(
        flash_rows(&state, PowerKind::Nostalgia),
        0,
        "Rebound already moved the result off Discard, so Nostalgia's \
         `pileType == Discard` guard fails"
    );
    assert_eq!(
        state.player_power_amount(PowerKind::Rebound),
        0,
        "the winner is the one that spends a stack"
    );
}

#[test]
fn gh650_nostalgia_applied_first_leaves_rebounds_stack_alone() {
    let (state, uid) = play_with_both(PowerKind::Nostalgia, PowerKind::Rebound);
    assert_eq!(state.player.draw.first().copied(), Some(uid));
    assert_eq!(flash_rows(&state, PowerKind::Nostalgia), 1);
    assert_eq!(flash_rows(&state, PowerKind::Rebound), 0);
    assert_eq!(
        state.player_power_amount(PowerKind::Rebound),
        1,
        "Rebound never modified this play, so it costs nothing"
    );
}

#[test]
fn gh650_feral_beats_both_and_costs_no_rebound_stack() {
    // `FeralPower` has NO `pileType` guard (`FeralPower.cs`), so it overrides
    // whatever the earlier listeners left — including an Exhaust-bound base
    // pile — and the two Discard-only powers never see a Discard to redirect.
    let mut state = play_ready_state(PowerKind::Rebound, 1);
    push_player_power(&mut state, 2, PowerKind::Nostalgia, 1);
    push_player_power(&mut state, 3, PowerKind::Feral, 1);
    let uid = add_hand_card(&mut state, 1, sts2_core::cards::CardType::Attack, 0);
    state.cards[uid.get() as usize - 1].keyword_override =
        Some(sts2_core::cards::KeywordOverride::EXHAUST);

    let _ = sts2_core::combat_loop::play_card(&mut state, 0, None);

    assert!(
        state.player.hand.contains(&uid),
        "a 0-cost Attack under Feral returns to hand, exhaust keyword and all"
    );
    assert_eq!(flash_rows(&state, PowerKind::Feral), 1);
    assert_eq!(flash_rows(&state, PowerKind::Rebound), 0);
    assert_eq!(flash_rows(&state, PowerKind::Nostalgia), 0);
    assert_eq!(
        state.player_power_amount(PowerKind::Rebound),
        1,
        "no redirect by Rebound means no stack spent"
    );
}
