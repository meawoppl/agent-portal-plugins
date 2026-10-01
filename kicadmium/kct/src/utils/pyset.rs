//! CPython `set` iteration order for int-keyed sets.
//!
//! Upstream occasionally iterates a `set` of ints / int tuples (e.g. engaged
//! diff-pair `(net_a, net_b)` keys) and emits findings in that order. Int and
//! tuple-of-int hashes are deterministic (unlike `str`), so the order is
//! reproducible: [`PySetOrder`] replays CPython 3.12's open-addressing table
//! (`setobject.c`: linear probes + perturbation, resize at 60% fill).

const LINEAR_PROBES: usize = 9;
const PERTURB_SHIFT: u32 = 5;
const MINSIZE: usize = 8;
const MODULUS: u64 = (1 << 61) - 1;

/// `hash(int)` for an `i64`.
pub fn hash_int(v: i64) -> i64 {
    let m = (v.unsigned_abs() % MODULUS) as i64;
    let h = if v < 0 { -m } else { m };
    if h == -1 {
        -2
    } else {
        h
    }
}

/// `hash(tuple_of_ints)` (CPython 3.8+ xxHash-style tuple hash).
pub fn hash_int_tuple(items: &[i64]) -> i64 {
    const P1: u64 = 11400714785074694791;
    const P2: u64 = 14029467366897019727;
    const P5: u64 = 2870177450012600261;
    let mut acc: u64 = P5;
    for &it in items {
        let lane = hash_int(it) as u64;
        acc = acc.wrapping_add(lane.wrapping_mul(P2));
        acc = acc.rotate_left(31);
        acc = acc.wrapping_mul(P1);
    }
    acc = acc.wrapping_add((items.len() as u64) ^ (P5 ^ 3527539));
    if acc == u64::MAX {
        return 1546275796;
    }
    acc as i64
}

/// Replays insertions into a CPython set and yields table order.
#[derive(Debug, Clone)]
pub struct PySetOrder<K: PartialEq + Clone> {
    table: Vec<Option<(i64, K)>>,
    used: usize,
    fill: usize,
}

impl<K: PartialEq + Clone> Default for PySetOrder<K> {
    fn default() -> Self {
        PySetOrder {
            table: vec![None; MINSIZE],
            used: 0,
            fill: 0,
        }
    }
}

impl<K: PartialEq + Clone> PySetOrder<K> {
    pub fn new() -> Self {
        Self::default()
    }

    fn mask(&self) -> usize {
        self.table.len() - 1
    }

    fn insert_clean(table: &mut [Option<(i64, K)>], hash: i64, key: K) {
        let mask = table.len() - 1;
        let mut perturb = hash as u64;
        let mut i = (hash as u64 as usize) & mask;
        loop {
            if table[i].is_none() {
                table[i] = Some((hash, key));
                return;
            }
            if i + LINEAR_PROBES <= mask {
                for j in 1..=LINEAR_PROBES {
                    if table[i + j].is_none() {
                        table[i + j] = Some((hash, key));
                        return;
                    }
                }
            }
            perturb >>= PERTURB_SHIFT;
            i = (i
                .wrapping_mul(5)
                .wrapping_add(1)
                .wrapping_add(perturb as usize))
                & mask;
        }
    }

    fn resize(&mut self, minused: usize) {
        let mut newsize = MINSIZE;
        while newsize <= minused {
            newsize <<= 1;
        }
        let old = std::mem::replace(&mut self.table, vec![None; newsize]);
        for (h, k) in old.into_iter().flatten() {
            Self::insert_clean(&mut self.table, h, k);
        }
        self.fill = self.used;
    }

    /// `set.add(key)` with `hash(key) == hash`.
    pub fn add(&mut self, hash: i64, key: K) {
        let mask = self.mask();
        let mut perturb = hash as u64;
        let mut i = (hash as u64 as usize) & mask;
        loop {
            let probes = if i + LINEAR_PROBES <= mask {
                LINEAR_PROBES
            } else {
                0
            };
            for j in 0..=probes {
                match &self.table[i + j] {
                    None => {
                        self.table[i + j] = Some((hash, key));
                        self.fill += 1;
                        self.used += 1;
                        if self.fill * 5 >= mask * 3 {
                            let n = if self.used > 50000 {
                                self.used * 2
                            } else {
                                self.used * 4
                            };
                            self.resize(n);
                        }
                        return;
                    }
                    Some((h, k)) if *h == hash && *k == key => return,
                    _ => {}
                }
            }
            perturb >>= PERTURB_SHIFT;
            i = (i
                .wrapping_mul(5)
                .wrapping_add(1)
                .wrapping_add(perturb as usize))
                & mask;
        }
    }

    pub fn len(&self) -> usize {
        self.used
    }

    pub fn is_empty(&self) -> bool {
        self.used == 0
    }

    pub fn contains(&self, key: &K) -> bool {
        self.table.iter().flatten().any(|(_, k)| k == key)
    }

    /// Iteration order.
    pub fn iter(&self) -> impl Iterator<Item = &K> {
        self.table.iter().flatten().map(|(_, k)| k)
    }
}

/// A set of `(i64, i64)` keys in CPython iteration order.
pub fn int_pair_set(keys: impl IntoIterator<Item = (i64, i64)>) -> PySetOrder<(i64, i64)> {
    let mut s = PySetOrder::new();
    for k in keys {
        s.add(hash_int_tuple(&[k.0, k.1]), k);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tuple_hash_matches_cpython() {
        // python3.12 -c "print(hash((1, 2)), hash((3, 7)), hash(()))"
        assert_eq!(hash_int_tuple(&[1, 2]), -3550055125485641917);
        assert_eq!(hash_int_tuple(&[]), 5740354900026072187);
        let keys = [
            (5, 6),
            (1, 2),
            (30, 31),
            (12, 13),
            (7, 8),
            (100, 3),
            (9, 10),
            (2, 40),
            (11, 1),
        ];
        let order: Vec<(i64, i64)> = int_pair_set(keys).iter().copied().collect();
        assert_eq!(
            order,
            vec![
                (9, 10),
                (11, 1),
                (1, 2),
                (12, 13),
                (100, 3),
                (5, 6),
                (30, 31),
                (2, 40),
                (7, 8)
            ]
        );
    }
}
