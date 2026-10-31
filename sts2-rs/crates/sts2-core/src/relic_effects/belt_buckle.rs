//! Belt Buckle's potion-count side (sts2-agent fork, belt-buckle). C#
//! `BeltBuckle.AfterPotionProcured` removes the Dexterity it gave once the
//! belt holds a potion again, and `AfterPotionDiscarded` gives it when a
//! discard empties the belt; both act only while a combat is in progress and
//! share `DexterityApplied` with `BeforeCombatStart` and `AfterPotionUsed`
//! (the content side's listeners). The latch is the relic's `counter`
//! (0 not applied, 1 applied). Called at the engine's in-combat potion
//! procure and discard sites.

use crate::card_effects::{
    apply_power_to_creature, apply_signed_attr_power, try_consume_artifact_for_signed_debuff,
};
use crate::{CombatState, PowerKind, RelicKind};

/// `BeltBuckle`'s `PowerVar<DexterityPower>(2m)`.
pub const BELT_BUCKLE_DEXTERITY: i32 = 2;

fn buckle(state: &CombatState) -> Option<usize> {
    state
        .relics
        .iter()
        .position(|r| !r.is_melted && r.kind == RelicKind::BeltBuckle)
}

/// C# `PowerCmd.Apply<DexterityPower>(owner, amount)`: a positive amount
/// as any power; a negative one is a debuff (Artifact takes it) that may
/// take Dexterity below zero.
fn apply_dexterity(state: &mut CombatState, amount: i32) {
    let Some(uid) = state.player.creature_uid else {
        return;
    };
    if amount > 0 {
        apply_power_to_creature(state, uid, PowerKind::Dexterity, amount);
    } else if !try_consume_artifact_for_signed_debuff(state, uid, amount) {
        apply_signed_attr_power(state, uid, PowerKind::Dexterity, amount);
    }
}

/// C# `AfterPotionProcured`: with a potion in the belt, `RemoveDexterity`
/// (`if (DexterityApplied) { DexterityApplied = false; Apply(-2); }`).
pub fn after_potion_procured(state: &mut CombatState) {
    let Some(idx) = buckle(state) else {
        return;
    };
    if state.player.potions.is_empty() || state.relics[idx].counter == 0 {
        return;
    }
    state.relics[idx].counter = 0;
    apply_dexterity(state, -BELT_BUCKLE_DEXTERITY);
}

/// C# `AfterPotionDiscarded`: with the belt empty, `ApplyDexterity`
/// (`if (!DexterityApplied) { DexterityApplied = true; Apply(+2); }`).
pub fn after_potion_discarded(state: &mut CombatState) {
    let Some(idx) = buckle(state) else {
        return;
    };
    if !state.player.potions.is_empty() || state.relics[idx].counter != 0 {
        return;
    }
    state.relics[idx].counter = 1;
    apply_dexterity(state, BELT_BUCKLE_DEXTERITY);
}
