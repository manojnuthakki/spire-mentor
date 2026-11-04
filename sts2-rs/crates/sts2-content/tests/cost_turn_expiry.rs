//! F6 step32, the cost and turn-expiry batch (fresh DLL authority, sha 08faee0c...):
//! `CardEnergyCost` keeps a base and an ordered list of local modifiers, each
//! expiring at `EndOfTurn`, `WhenPlayed` or `EndOfCombat`.
//!
//! * `PlayerCombatState.EndOfTurnCleanup` drops the `EndOfTurn` ones when the
//!   turn ENDS (`FlushPlayerHand`: hand to discard, `Hook.AfterFlush`, then the
//!   cleanup), not at the next player-turn start (`this-turn-cost-turn-end`);
//!   Bookmark, the only `AfterFlush` override, reads the costs before it.
//! * `AfterCardPlayedCleanup` drops the `WhenPlayed` ones at the end of
//!   `OnPlayWrapper`, after `Hook.AfterCardPlayed` (`juggling-clone-cost`).
//! * `SetToFreeThisTurn` is `SetThisTurnOrUntilPlayed(0)`, not a new base
//!   (`vexing-puzzlebox-free-this-turn`).
//! * Enlightenment caps every hand card with `SetThisTurnOrUntilPlayed(1,
//!   reduceOnly)`, upgraded `SetThisCombat(1, reduceOnly)`
//!   (`enlightenment-until-played`).
//! * Tezcatara's Ember sets the base to 0 (`UpgradeBy(-cost)`)
//!   (`ember-enchanted-cost`).
use std::num::NonZeroU32;
use sts2_content::cards::spec_index::card_pod_from_id;
use sts2_core::cards::{CardId, EnchantmentId, VarKey};
use sts2_core::combat_loop::{end_turn_phase, play_card};
use sts2_core::{
    CardUid, CombatPhase, CombatState, MonsterKind, PowerKind, Relic, RelicKind, Side,
};

fn add(s: &mut CombatState, id: CardId, upgraded: bool) -> CardUid {
    let mut c = card_pod_from_id(id, upgraded).unwrap();
    c.uid = CardUid::new(s.cards.len() as u32 + 1).unwrap();
    s.cards.push(c);
    c.uid
}

fn card(s: &CombatState, uid: CardUid) -> &sts2_core::cards::Card {
    &s.cards[uid.get() as usize - 1]
}

fn cost(s: &CombatState, uid: CardUid) -> i8 {
    card(s, uid).cost
}

fn relic(s: &mut CombatState, kind: RelicKind) {
    let id = NonZeroU32::new(900 + s.relics.len() as u32).unwrap();
    s.relics.push(Relic::new(id, kind));
}

fn board(energy: i8) -> CombatState {
    sts2_content::cards::dispatch::install_default_registry();
    sts2_content::relics::dispatch::ensure_installed();
    let mut s = CombatState::default();
    s.player.current_hp = 80;
    s.player.max_hp = 80;
    s.player.energy = energy;
    s.init_player_creature();
    let e = s.spawn_enemy(MonsterKind::Nibbit);
    let enemy = s.creatures.iter_mut().find(|c| c.id == e.0).unwrap();
    enemy.current_hp = 500;
    enemy.max_hp = 500;
    s.phase = CombatPhase::PlayerPlayPhase;
    s
}

fn hand_index(s: &CombatState, uid: CardUid) -> u8 {
    s.player.hand.iter().position(|&u| u == uid).unwrap() as u8
}

fn play(s: &mut CombatState, uid: CardUid) {
    let target = match card(s, uid).target_type {
        sts2_core::cards::TargetType::Enemy => Some(0),
        _ => None,
    };
    let _ = play_card(s, hand_index(s, uid), target);
    assert!(s.pending_choice.is_none(), "no choice expected");
}

/// The player's turn end, through the hand flush and its cleanup, stopping
/// before the enemy side starts (where Knowledge Demon's TO_OBTAIN is asked).
fn end_player_turn(s: &mut CombatState) {
    for phase in 0..2 {
        let _ = end_turn_phase(s, phase);
    }
}

fn strikes(s: &mut CombatState, n: usize) -> Vec<CardUid> {
    (0..n)
        .map(|_| {
            let u = add(s, CardId::StrikeIronclad, false);
            s.player.hand.push(u);
            u
        })
        .collect()
}

// ---------------------------------------------------------------- this turn --

#[test]
fn stomp_shave_expires_at_the_players_turn_end_not_the_next_turn_start() {
    let mut s = board(3);
    let stomp = add(&mut s, CardId::Stomp, false);
    s.player.draw.push(stomp);
    for u in strikes(&mut s, 3) {
        play(&mut s, u);
    }
    assert_eq!(
        cost(&s, stomp),
        0,
        "three Attacks shave Stomp 3 -> 0 this turn"
    );
    end_player_turn(&mut s);
    assert_eq!(
        cost(&s, stomp),
        3,
        "the enemy turn sees Stomp's base cost (EndOfTurnCleanup at the flush)"
    );
    assert_eq!(
        card(&s, stomp).combat_var(VarKey::StompCostReductionThisTurn),
        0
    );
}

#[test]
fn bookmark_reads_a_retained_zero_stomp_before_the_cleanup() {
    // The real Headless counterexample of the rejected step20 candidate: a
    // retained Stomp shaved to 0 is not Bookmark-eligible (local cost 0) and
    // comes back at 3 after the cleanup.
    let mut s = board(3);
    relic(&mut s, RelicKind::Bookmark);
    let stomp = add(&mut s, CardId::Stomp, false);
    s.player.hand.push(stomp);
    for u in strikes(&mut s, 3) {
        play(&mut s, u);
    }
    s.player.retain_this_turn.push(stomp);
    end_player_turn(&mut s);
    assert!(s.player.hand.contains(&stomp));
    assert_eq!(cost(&s, stomp), 3);
}

#[test]
fn bookmark_discount_on_a_retained_shaved_stomp_survives_the_cleanup() {
    // Stomp shaved 3 -> 2 and retained alone: Bookmark takes 1 off before the
    // cleanup gives the shave back, so the enemy turn already sees 2.
    let mut s = board(3);
    relic(&mut s, RelicKind::Bookmark);
    let stomp = add(&mut s, CardId::Stomp, false);
    s.player.hand.push(stomp);
    for u in strikes(&mut s, 1) {
        play(&mut s, u);
    }
    assert_eq!(cost(&s, stomp), 2);
    s.player.retain_this_turn.push(stomp);
    end_player_turn(&mut s);
    assert_eq!(cost(&s, stomp), 2);
}

// ------------------------------------------------------------ free this turn --

#[test]
fn vexing_puzzlebox_card_is_free_this_turn_over_its_base_cost() {
    let mut s = board(3);
    relic(&mut s, RelicKind::VexingPuzzlebox);
    s.player.character = Some(sts2_core::potions::CharacterId::Ironclad);
    s.round = 1;
    let before = s.cards.len();
    let _ = sts2_core::relic_effects::dispatch_after_side_turn_start(&mut s, Side::Player);
    assert_eq!(s.cards.len(), before + 1, "one generated card");
    let uid = s.cards[before].uid;
    assert!(s.player.hand.contains(&uid));
    let base = card_pod_from_id(card(&s, uid).card_id, false).unwrap().cost;
    assert!(base > 0, "the seed's card has a cost to waive");
    assert_eq!(cost(&s, uid), base, "the base cost is untouched");
    assert_eq!(
        card(&s, uid).combat_var(VarKey::FreeThisTurn),
        1,
        "free this turn"
    );
    end_player_turn(&mut s);
    assert_eq!(cost(&s, uid), base);
    assert_eq!(
        card(&s, uid).combat_var(VarKey::FreeThisTurn),
        0,
        "the turn's end ends it"
    );
}

#[test]
fn juggling_clone_keeps_the_played_cards_free_this_turn() {
    let mut s = board(3);
    let p = s.player.creature_uid.unwrap();
    sts2_core::card_effects::apply_power_to_creature(&mut s, p, PowerKind::Juggling, 1);
    let st = strikes(&mut s, 3);
    s.cards[st[2].get() as usize - 1].set_combat_var(VarKey::FreeThisTurn, 1);
    for u in &st {
        play(&mut s, *u);
    }
    let clone = *s
        .player
        .hand
        .last()
        .expect("Juggling's clone of the third Attack");
    assert_ne!(clone, st[2]);
    assert_eq!(card(&s, clone).card_id, CardId::StrikeIronclad);
    assert_eq!(
        card(&s, clone).combat_var(VarKey::FreeThisTurn),
        1,
        "CreateClone copies the WhenPlayed modifier before AfterCardPlayedCleanup"
    );
    assert_eq!(
        card(&s, st[2]).combat_var(VarKey::FreeThisTurn),
        0,
        "the played card's is gone"
    );
    assert!(s.player.discard.contains(&st[2]));
}

// ---------------------------------------------------------------- Enlightenment --

#[test]
fn enlightenment_cap_ends_when_the_card_is_played_or_the_turn_ends() {
    let mut s = board(3);
    let bludgeon = add(&mut s, CardId::Bludgeon, false);
    let perfected = add(&mut s, CardId::PerfectedStrike, false);
    let enl = add(&mut s, CardId::Enlightenment, false);
    s.player.hand.extend([bludgeon, perfected, enl]);
    play(&mut s, enl);
    assert_eq!((cost(&s, bludgeon), cost(&s, perfected)), (1, 1));
    play(&mut s, perfected);
    assert!(s.player.discard.contains(&perfected));
    assert_eq!(cost(&s, perfected), 2, "played: the WhenPlayed cap is gone");
    end_player_turn(&mut s);
    assert_eq!(cost(&s, bludgeon), 3, "held: the EndOfTurn cap is gone");
}

#[test]
fn upgraded_enlightenment_cap_survives_an_upgrade_that_lowers_the_base() {
    let mut s = board(3);
    let barricade = add(&mut s, CardId::BarricadeCard, false);
    let enl = add(&mut s, CardId::Enlightenment, true);
    s.player.hand.extend([barricade, enl]);
    play(&mut s, enl);
    assert_eq!(cost(&s, barricade), 1);
    assert!(s.cards[barricade.get() as usize - 1].upgrade());
    assert_eq!(
        cost(&s, barricade),
        1,
        "UpgradeBy clamps the absolute cap, 1 stays 1"
    );
    end_player_turn(&mut s);
    assert_eq!(cost(&s, barricade), 1, "the upgraded cap lasts the combat");
}

#[test]
fn enlightenment_cap_then_upgrade_expires_to_the_new_base() {
    let mut s = board(3);
    let barricade = add(&mut s, CardId::BarricadeCard, false);
    let enl = add(&mut s, CardId::Enlightenment, false);
    s.player.hand.extend([barricade, enl]);
    play(&mut s, enl);
    assert!(s.cards[barricade.get() as usize - 1].upgrade());
    assert_eq!(cost(&s, barricade), 1);
    end_player_turn(&mut s);
    let upgraded_base = card_pod_from_id(CardId::BarricadeCard, true).unwrap().cost;
    assert_eq!(cost(&s, barricade), upgraded_base);
}

#[test]
fn enlightenment_and_stomp_shave_compose_in_order() {
    // C# folds the local modifiers in order and clamps only the result: base
    // 3, cap 1, the Strike's -1 (shown 0), then Stomp's own BeforeCardPlayed
    // -1 lands beneath the cap although the shown cost is already 0. Played,
    // the cap goes: 3 - 1 - 1 = 1 in the discard (Headless: 1; review r1 of
    // step32, which this test first asserted as 2). The turn's end restores 3.
    let mut s = board(3);
    let stomp = add(&mut s, CardId::Stomp, false);
    let enl = add(&mut s, CardId::Enlightenment, false);
    s.player.hand.extend([stomp, enl]);
    play(&mut s, enl);
    assert_eq!(cost(&s, stomp), 1);
    let st = strikes(&mut s, 1);
    play(&mut s, st[0]);
    assert_eq!(cost(&s, stomp), 0);
    play(&mut s, stomp);
    assert!(s.player.discard.contains(&stomp));
    assert_eq!(cost(&s, stomp), 1, "both shaves survive the cap's expiry");
    end_player_turn(&mut s);
    assert_eq!(cost(&s, stomp), 3);
}

#[test]
fn stomp_shaves_beneath_an_enlightenment_cap_survive_its_turn_end_expiry() {
    // Held, not played: cap 1, then two Attacks (-1 shown, -1 beneath the
    // cap). A third Attack's shave is beneath too; playing Stomp now adds its
    // own: 3 - 4 = -1, shown 0 after the cap goes. The turn's end restores 3.
    let mut s = board(9);
    let stomp = add(&mut s, CardId::Stomp, false);
    let held = add(&mut s, CardId::Stomp, false);
    let enl = add(&mut s, CardId::Enlightenment, false);
    s.player.hand.extend([stomp, held, enl]);
    play(&mut s, enl);
    for u in strikes(&mut s, 2) {
        play(&mut s, u);
    }
    assert_eq!((cost(&s, stomp), cost(&s, held)), (0, 0));
    play(&mut s, stomp);
    assert_eq!(cost(&s, stomp), 0, "3 - 3 shaves, the cap gone");
    assert_eq!(cost(&s, held), 0, "still capped");
    end_player_turn(&mut s);
    assert_eq!((cost(&s, stomp), cost(&s, held)), (3, 3));
}

#[test]
fn bookmark_skips_a_turn_whose_orb_passive_already_won_the_combat() {
    // C# checks the win after phase one (DoTurnEnd's orb passive included);
    // phase two, FlushPlayerHand with its AfterFlush (Bookmark), only runs
    // while the combat is in progress: no cost change, no combat_card_selection
    // draw (review r1 of step32: Cracked Core's Lightning killing the last
    // enemy on a Bookmark turn).
    let mut s = board(3);
    relic(&mut s, RelicKind::Bookmark);
    let e = s.enemy_uids[0];
    let enemy = s.creatures.iter_mut().find(|c| c.id == e.0).unwrap();
    enemy.current_hp = 3;
    s.player.orbs = sts2_core::orbs::OrbSlots::with_default_capacity();
    s.player.orbs.channel(sts2_core::orbs::Orb::Lightning);
    let st = strikes(&mut s, 1)[0];
    s.player.retain_this_turn.push(st);
    let before = s.run_rng.combat_card_selection.counter();
    end_player_turn(&mut s);
    assert!(s.is_over && s.player_won, "the zap won the combat");
    assert_eq!(cost(&s, st), 1, "Bookmark did not run");
    assert_eq!(s.run_rng.combat_card_selection.counter(), before, "no draw");
}

#[test]
fn bookmark_runs_when_the_orb_passive_leaves_an_enemy_alive() {
    // The control: the same turn with the enemy surviving the zap.
    let mut s = board(3);
    relic(&mut s, RelicKind::Bookmark);
    s.player.orbs = sts2_core::orbs::OrbSlots::with_default_capacity();
    s.player.orbs.channel(sts2_core::orbs::Orb::Lightning);
    let st = strikes(&mut s, 1)[0];
    s.player.retain_this_turn.push(st);
    let before = s.run_rng.combat_card_selection.counter();
    end_player_turn(&mut s);
    assert!(!s.is_over);
    assert_eq!(cost(&s, st), 0);
    assert_eq!(s.run_rng.combat_card_selection.counter(), before + 1);
}

// ---------------------------------------------------------------- Ember --

#[test]
fn nutritious_soup_sets_the_enchanted_strikes_base_cost_to_zero() {
    use sts2_core::runstate::RunState;
    use sts2_core::RelicId;
    sts2_content::relics::dispatch::ensure_installed();
    let mut rs = RunState::new();
    for id in [CardId::StrikeIronclad, CardId::DefendIronclad] {
        rs.master_deck.push(card_pod_from_id(id, false).unwrap());
    }
    rs.relics.push(RelicId::NutritiousSoup);
    rs.last_obtained_relic = Some(RelicId::NutritiousSoup);
    let _ = sts2_core::run_effects::dispatch_after_obtained(&mut rs);
    let strike = &rs.master_deck[0];
    assert_eq!(strike.enchantment(EnchantmentId::TezcatarasEmber), 1);
    assert_eq!(
        strike.cost, 0,
        "TezcatarasEmber.OnEnchant: UpgradeBy(-cost)"
    );
    assert_eq!(rs.master_deck[1].cost, 1, "the Defend is untouched");
}
