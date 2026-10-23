//! Property tests for [`sts2_core::inline_kv::InlineKv`]'s derived
//! "may be nonzero" mask (J02).
//!
//! The per-turn card sweeps and `Card::combat_var` skip work when a key's
//! mask bit is clear, which is exact only while the bit is a SUPERSET of the
//! keys holding a nonzero value. These cases drive random sequences of every
//! value writer (`push`, `set`, `add`, `zero`, `clear`, `collect`) on the two
//! real `Card` maps, against a plain `Vec` model with the old find-first /
//! push semantics, and check after every step that:
//!
//! * the live entries equal the model (the writers kept the old semantics);
//! * a clear bit means every live entry for that key is 0;
//! * `get` answers the model's first-match value (0 when absent);
//! * the mask is invisible: a fresh `collect` of the same entries (a
//!   different write history) is `==`, hashes alike and prints alike.
//!
//! `VarKey` has more variants than the mask has bits, so the aliased keys
//! (`WireCostStable`/`RampageDamage`, `Generic`/`ClawBuff`) are covered too.

use core::hash::{Hash, Hasher};

use proptest::prelude::*;
use sts2_core::cards::{CardEnchantments, CombatVars, EnchantmentId, VarKey};
use sts2_core::inline_kv::{InlineKv, InlineKvKey};

const VAR_KEYS: [VarKey; 27] = [
    VarKey::RampageDamage,
    VarKey::ClawBuff,
    VarKey::GeneticAlgorithmBlock,
    VarKey::RitualDaggerDamage,
    VarKey::FreeThisTurn,
    VarKey::EtherealThisCombat,
    VarKey::StarCostFreeThisCombat,
    VarKey::MakeItSoInterval,
    VarKey::StompCostReductionThisTurn,
    VarKey::PinpointCostReductionThisTurn,
    VarKey::SneckoCostDeltaThisTurn,
    VarKey::ReturnIfPlayedRound,
    VarKey::DampenDowngradedMarker,
    VarKey::IsDupe,
    VarKey::GeneratedInCombat,
    VarKey::CostAbsolutelySet,
    VarKey::ThrashFoldRemNum,
    VarKey::ThrashFoldRemDen,
    VarKey::FreeUntilPlayed,
    VarKey::WitherFakeUpgradeLevel,
    VarKey::PendingBatchAutoPlay,
    VarKey::AutoPlayStarX,
    VarKey::WireTransientCostThisTurn,
    VarKey::WireCostExpiringOnPlay,
    VarKey::WireCostStable,
    VarKey::Generic,
    VarKey::EnchantmentUsed,
];

const ENCHANTMENT_IDS: [EnchantmentId; 23] = [
    EnchantmentId::RoyallyApproved,
    EnchantmentId::Steady,
    EnchantmentId::TezcatarasEmber,
    EnchantmentId::SoulsPower,
    EnchantmentId::Instinct,
    EnchantmentId::Goopy,
    EnchantmentId::Sharp,
    EnchantmentId::Momentum,
    EnchantmentId::Corrupted,
    EnchantmentId::Favored,
    EnchantmentId::Vigorous,
    EnchantmentId::Nimble,
    EnchantmentId::Spiral,
    EnchantmentId::Glam,
    EnchantmentId::Adroit,
    EnchantmentId::Swift,
    EnchantmentId::Sown,
    EnchantmentId::Imbued,
    EnchantmentId::Slither,
    EnchantmentId::SlumberingEssence,
    EnchantmentId::PerfectFit,
    EnchantmentId::Clone,
    EnchantmentId::Inky,
];

/// One writer call. Keys are indices into the key table under test.
#[derive(Clone, Debug)]
enum Op {
    Push(usize, i32),
    Set(usize, i32),
    Add(usize, i32),
    Zero(usize),
    Clear,
    Recollect,
}

fn value() -> impl Strategy<Value = i32> {
    prop_oneof![
        4 => Just(0),
        6 => -3i32..=3,
        1 => Just(i32::MAX),
        1 => Just(i32::MIN),
    ]
}

fn ops(n_keys: usize) -> impl Strategy<Value = Vec<Op>> {
    let k = 0..n_keys;
    let op = prop_oneof![
        3 => (k.clone(), value()).prop_map(|(k, v)| Op::Push(k, v)),
        6 => (k.clone(), value()).prop_map(|(k, v)| Op::Set(k, v)),
        3 => (k.clone(), value()).prop_map(|(k, v)| Op::Add(k, v)),
        3 => k.prop_map(Op::Zero),
        1 => Just(Op::Clear),
        1 => Just(Op::Recollect),
    ];
    prop::collection::vec(op, 0..80)
}

fn hash_of(x: &impl Hash) -> u64 {
    let mut s = std::collections::hash_map::DefaultHasher::new();
    x.hash(&mut s);
    s.finish()
}

/// Apply `ops` to a map and to the `Vec` model, checking the contract after
/// every step.
fn run<K: InlineKvKey + Hash, const N: usize>(keys: &[K], ops: &[Op]) -> Result<(), TestCaseError> {
    let mut map = InlineKv::<K, N>::new();
    let mut model: Vec<(K, i32)> = Vec::new();
    for op in ops {
        let full = model.len() == N;
        let first = |model: &mut Vec<(K, i32)>, key: K| model.iter().position(|(k, _)| *k == key);
        match *op {
            Op::Push(k, v) => {
                if full {
                    continue;
                }
                map.push((keys[k], v));
                model.push((keys[k], v));
            }
            Op::Set(k, v) => match first(&mut model, keys[k]) {
                Some(i) => {
                    map.set(keys[k], v);
                    model[i].1 = v;
                }
                None if !full => {
                    map.set(keys[k], v);
                    model.push((keys[k], v));
                }
                None => continue,
            },
            Op::Add(k, d) => match first(&mut model, keys[k]) {
                Some(i) => {
                    map.add(keys[k], d);
                    model[i].1 = model[i].1.saturating_add(d);
                }
                None if !full => {
                    map.add(keys[k], d);
                    model.push((keys[k], d));
                }
                None => continue,
            },
            Op::Zero(k) => {
                map.zero(keys[k]);
                if let Some(i) = first(&mut model, keys[k]) {
                    model[i].1 = 0;
                }
            }
            Op::Clear => {
                map.clear();
                model.clear();
            }
            Op::Recollect => {
                map = map.iter().copied().collect();
            }
        }

        prop_assert_eq!(map.as_slice(), model.as_slice(), "after {:?}", op);
        for &key in keys {
            let want = model
                .iter()
                .find_map(|(k, v)| (*k == key).then_some(*v))
                .unwrap_or(0);
            if !map.maybe_nonzero(key) {
                prop_assert!(
                    model.iter().all(|(k, v)| *k != key || *v == 0),
                    "mask clear for {:?} but entries are {:?}",
                    key,
                    model
                );
            }
            prop_assert_eq!(map.get(key), want, "get({:?}) after {:?}", key, op);
        }
        let fresh: InlineKv<K, N> = model.iter().copied().collect();
        prop_assert!(map == fresh, "mask leaked into ==");
        prop_assert_eq!(hash_of(&map), hash_of(&fresh), "mask leaked into Hash");
        prop_assert_eq!(
            format!("{map:?}"),
            format!("{fresh:?}"),
            "mask leaked into Debug"
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// `Card::combat_vars` (16 slots, 26 keys over a 24-bit mask).
    #[test]
    fn combat_vars_mask_is_a_superset_of_nonzero_keys(ops in ops(VAR_KEYS.len())) {
        // The alias the mask relies on: the type is the real `CombatVars`.
        let _: fn() -> CombatVars = InlineKv::<VarKey, 16>::new;
        run::<VarKey, 16>(&VAR_KEYS, &ops)?;
    }

    /// `Card::enchantments` (6 slots, 23 keys).
    #[test]
    fn enchantments_mask_is_a_superset_of_nonzero_keys(ops in ops(ENCHANTMENT_IDS.len())) {
        let _: fn() -> CardEnchantments = InlineKv::<EnchantmentId, 6>::new;
        run::<EnchantmentId, 6>(&ENCHANTMENT_IDS, &ops)?;
    }
}

/// The sweeps gate on these keys; none of them may share a mask bit with
/// another key (an alias would only cost speed, but the gate notes in
/// `lib.rs` / `combat_loop.rs` claim there is none).
#[test]
fn gate_keys_do_not_alias() {
    for gate in [
        VarKey::FreeThisTurn,
        VarKey::FreeUntilPlayed,
        VarKey::SneckoCostDeltaThisTurn,
        VarKey::WireTransientCostThisTurn,
        VarKey::StompCostReductionThisTurn,
        VarKey::PinpointCostReductionThisTurn,
    ] {
        let sharing: Vec<VarKey> = VAR_KEYS
            .iter()
            .copied()
            .filter(|k| *k != gate && k.nz_index() == gate.nz_index())
            .collect();
        assert!(
            sharing.is_empty(),
            "{gate:?} shares its mask bit with {sharing:?}"
        );
    }
}
