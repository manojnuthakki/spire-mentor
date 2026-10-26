//! A search's value of the potion belt at an evaluation (sts2-agent fork).
//!
//! The deployed evaluations price held potions at a flat, process-wide
//! `λ` per potion ([`crate::victory_potion_weight`], `STS2_RUNAI_POTION_WEIGHT`).
//! A [`BeltValue`] carried on a search's root state ([`crate::CombatState::belt_value`],
//! inherited by every state the search derives from it) replaces that price in
//! every evaluation the search makes: the win terminal ([`crate::evaluate_victory`])
//! and both depth-cap leaves (narrow [`crate::evaluate_in_progress_raw`], wide
//! [`crate::evaluate_in_progress_wide`]). So a continuation pays for a drink
//! wherever in the line it drinks, not only at the root.
//!
//! The table maps each belt that can remain of the belt the search started
//! with (every sub-multiset of `base`, keyed by its sorted kinds) to
//! HP-equivalents. A potion the belt holds beyond `base` (made during the
//! fight) adds its kind's `fallback` value (else `default`). Values are
//! set-aware: the table need not be additive.

use crate::potions::PotionKind;

/// HP-equivalents of each belt that can remain of `base`, with a per-kind
/// additive fallback for potions beyond it.
#[derive(Clone, Debug)]
pub struct BeltValue {
    base: Vec<PotionKind>,
    /// Sorted keys (each a sorted sub-multiset of `base`) and their values.
    table: Vec<(Vec<PotionKind>, f32)>,
    fallback: Vec<(PotionKind, f32)>,
    default: f32,
}

impl PartialEq for BeltValue {
    fn eq(&self, other: &Self) -> bool {
        let bits = |t: &[(Vec<PotionKind>, f32)]| {
            t.iter()
                .map(|(k, v)| (k.clone(), v.to_bits()))
                .collect::<Vec<_>>()
        };
        let fb =
            |t: &[(PotionKind, f32)]| t.iter().map(|(k, v)| (*k, v.to_bits())).collect::<Vec<_>>();
        self.base == other.base
            && bits(&self.table) == bits(&other.table)
            && fb(&self.fallback) == fb(&other.fallback)
            && self.default.to_bits() == other.default.to_bits()
    }
}

// Every value is finite (checked by `new`), so bitwise equality is an equivalence.
impl Eq for BeltValue {}

fn sorted(mut kinds: Vec<PotionKind>) -> Vec<PotionKind> {
    kinds.sort();
    kinds
}

/// Every distinct sub-multiset of a sorted multiset, each sorted.
fn sub_multisets(base: &[PotionKind]) -> Vec<Vec<PotionKind>> {
    let mut out: Vec<Vec<PotionKind>> = vec![Vec::new()];
    let mut i = 0;
    while i < base.len() {
        let kind = base[i];
        let n = base[i..].iter().take_while(|k| **k == kind).count();
        let mut next = Vec::with_capacity(out.len() * (n + 1));
        for s in &out {
            for c in 0..=n {
                let mut t = s.clone();
                t.extend(std::iter::repeat(kind).take(c));
                next.push(t);
            }
        }
        out = next;
        i += n;
    }
    out
}

/// Remove `part` (a sorted sub-multiset) from sorted `whole`; `None` if it isn't one.
fn without(whole: &[PotionKind], part: &[PotionKind]) -> Option<Vec<PotionKind>> {
    let mut rest = whole.to_vec();
    for k in part {
        let i = rest.iter().position(|x| x == k)?;
        rest.remove(i);
    }
    Some(rest)
}

impl BeltValue {
    /// A belt value over `base` (the belt the search starts with). `table` must
    /// give every distinct sub-multiset of `base` exactly once (any order of
    /// kinds within a key); every value must be finite.
    ///
    /// # Errors
    /// A missing, extra or repeated key, or a value that isn't finite.
    pub fn new(
        base: Vec<PotionKind>,
        table: Vec<(Vec<PotionKind>, f32)>,
        fallback: Vec<(PotionKind, f32)>,
        default: f32,
    ) -> Result<Self, String> {
        let base = sorted(base);
        let mut table: Vec<(Vec<PotionKind>, f32)> =
            table.into_iter().map(|(k, v)| (sorted(k), v)).collect();
        table.sort_by(|a, b| a.0.cmp(&b.0)); // keys are Vecs: compared, not copied
        let mut wanted = sub_multisets(&base);
        wanted.sort();
        let keys: Vec<Vec<PotionKind>> = table.iter().map(|(k, _)| k.clone()).collect();
        if keys != wanted {
            return Err(format!(
                "belt value: the table must give every sub-multiset of the base belt {base:?} exactly once \
                 ({} keys wanted, {} given)",
                wanted.len(),
                keys.len()
            ));
        }
        let values = table
            .iter()
            .map(|(_, v)| *v)
            .chain(fallback.iter().map(|(_, v)| *v))
            .chain([default]);
        if values.into_iter().any(|v| !v.is_finite()) {
            return Err("belt value: every value must be finite".to_string());
        }
        let mut fallback = fallback;
        fallback.sort_by_key(|f| f.0);
        fallback.dedup_by(|a, b| a.0 == b.0);
        Ok(Self {
            base,
            table,
            fallback,
            default,
        })
    }

    /// The belt the table is over.
    #[must_use]
    pub fn base(&self) -> &[PotionKind] {
        &self.base
    }

    /// HP-equivalents of holding `belt`: the table's value of what remains of
    /// the base, plus each potion beyond it at its kind's fallback.
    #[must_use]
    pub fn hp_equiv(&self, belt: &[PotionKind]) -> f32 {
        let belt = sorted(belt.to_vec());
        // What remains of the base: the multiset intersection.
        let mut kept = Vec::with_capacity(belt.len());
        let mut extra = Vec::new();
        let mut pool = self.base.clone();
        for k in belt {
            if let Some(i) = pool.iter().position(|x| *x == k) {
                pool.remove(i);
                kept.push(k);
            } else {
                extra.push(k);
            }
        }
        debug_assert!(without(&self.base, &kept).is_some());
        let value = self
            .table
            .binary_search_by(|(k, _)| k.as_slice().cmp(kept.as_slice()))
            .map(|i| self.table[i].1)
            .expect("the table holds every sub-multiset of the base");
        value + extra.iter().map(|k| self.fallback_of(*k)).sum::<f32>()
    }

    fn fallback_of(&self, kind: PotionKind) -> f32 {
        self.fallback
            .binary_search_by(|(k, _)| k.cmp(&kind))
            .map_or(self.default, |i| self.fallback[i].1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PotionKind::{BlockPotion as B, FirePotion as F};

    fn additive(base: &[PotionKind], each: f32) -> BeltValue {
        let table = sub_multisets(&sorted(base.to_vec()))
            .into_iter()
            .map(|k| {
                #[allow(clippy::cast_precision_loss)]
                let v = k.len() as f32 * each;
                (k, v)
            })
            .collect();
        BeltValue::new(base.to_vec(), table, vec![], each).unwrap()
    }

    #[test]
    fn every_sub_multiset_once() {
        let mut s = sub_multisets(&sorted(vec![F, F, B]));
        s.sort();
        assert_eq!(
            s,
            vec![
                vec![],
                vec![F],
                vec![F, F],
                vec![B],
                vec![F, B],
                vec![F, F, B]
            ]
            .into_iter()
            .map(sorted)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
        );
    }

    #[test]
    #[allow(clippy::float_cmp)] // exact-by-construction: sums of small integers.
    fn a_belt_is_valued_by_what_remains_of_the_base_plus_the_fallback() {
        let v = BeltValue::new(
            vec![F, B],
            vec![
                (vec![], 0.0),
                (vec![F], 5.0),
                (vec![B], 3.0),
                (vec![B, F], 6.0),
            ],
            vec![(F, 7.0)],
            2.0,
        )
        .unwrap();
        assert_eq!(v.hp_equiv(&[]), 0.0);
        assert_eq!(v.hp_equiv(&[B, F]), 6.0); // not additive: 6, not 8
        assert_eq!(v.hp_equiv(&[F, F]), 5.0 + 7.0); // the second Fire made in the fight
        assert_eq!(v.hp_equiv(&[B, PotionKind::SwiftPotion]), 3.0 + 2.0); // a kind without a fallback
        assert_eq!(additive(&[F, F, B], 8.0).hp_equiv(&[F, B]), 16.0);
    }

    #[test]
    fn an_incomplete_or_infinite_table_is_refused() {
        assert!(BeltValue::new(vec![F], vec![(vec![F], 1.0)], vec![], 0.0).is_err());
        assert!(BeltValue::new(
            vec![F],
            vec![(vec![], 0.0), (vec![F], f32::NAN)],
            vec![],
            0.0
        )
        .is_err());
        assert!(BeltValue::new(
            vec![F],
            vec![(vec![], 0.0), (vec![F], 1.0), (vec![B], 1.0)],
            vec![],
            0.0
        )
        .is_err());
        assert!(BeltValue::new(
            vec![F],
            vec![(vec![], 0.0), (vec![F], 1.0)],
            vec![],
            f32::INFINITY
        )
        .is_err());
    }
}
