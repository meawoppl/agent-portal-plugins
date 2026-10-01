//! CPython-compatible `random` module (MT19937), so seeded router passes
//! (net-order shuffles, Monte Carlo trials, evolutionary mutation,
//! deterministic UUIDs) draw the same sequence as upstream for a given seed.
//!
//! Upstream uses the process-global `random` module; [`with_global`] gives
//! the same shared-state semantics per thread.

use std::cell::RefCell;

const N: usize = 624;
const M: usize = 397;

/// Mersenne Twister with CPython's seeding and derived distributions.
#[derive(Clone)]
pub struct PyRandom {
    mt: [u32; N],
    index: usize,
    gauss_next: Option<f64>,
}

impl std::fmt::Debug for PyRandom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PyRandom")
            .field("index", &self.index)
            .finish()
    }
}

impl Default for PyRandom {
    fn default() -> Self {
        Self::from_entropy()
    }
}

impl PyRandom {
    /// `random.Random(seed)` for an integer seed.
    pub fn new(seed: u64) -> Self {
        let mut r = Self {
            mt: [0; N],
            index: N,
            gauss_next: None,
        };
        r.seed(seed as i128);
        r
    }

    /// Unseeded generator (OS entropy).
    pub fn from_entropy() -> Self {
        let v = uuid::Uuid::new_v4().as_u128();
        let mut r = Self {
            mt: [0; N],
            index: N,
            gauss_next: None,
        };
        r.seed_u128(v);
        r
    }

    /// `random.seed(int)`: CPython uses `abs(seed)` split into 32-bit words.
    pub fn seed(&mut self, seed: i128) {
        self.seed_u128(seed.unsigned_abs());
    }

    fn seed_u128(&mut self, mut v: u128) {
        let mut key: Vec<u32> = Vec::new();
        if v == 0 {
            key.push(0);
        }
        while v > 0 {
            key.push((v & 0xffff_ffff) as u32);
            v >>= 32;
        }
        self.init_by_array(&key);
        self.gauss_next = None;
    }

    fn init_genrand(&mut self, s: u32) {
        self.mt[0] = s;
        for i in 1..N {
            self.mt[i] = 1_812_433_253u32
                .wrapping_mul(self.mt[i - 1] ^ (self.mt[i - 1] >> 30))
                .wrapping_add(i as u32);
        }
        self.index = N;
    }

    fn init_by_array(&mut self, key: &[u32]) {
        self.init_genrand(19_650_218);
        let mut i = 1usize;
        let mut j = 0usize;
        let klen = key.len();
        let mut k = N.max(klen);
        while k > 0 {
            self.mt[i] = (self.mt[i]
                ^ (self.mt[i - 1] ^ (self.mt[i - 1] >> 30)).wrapping_mul(1_664_525))
            .wrapping_add(key[j])
            .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= N {
                self.mt[0] = self.mt[N - 1];
                i = 1;
            }
            if j >= klen {
                j = 0;
            }
            k -= 1;
        }
        k = N - 1;
        while k > 0 {
            self.mt[i] = (self.mt[i]
                ^ (self.mt[i - 1] ^ (self.mt[i - 1] >> 30)).wrapping_mul(1_566_083_941))
            .wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                self.mt[0] = self.mt[N - 1];
                i = 1;
            }
            k -= 1;
        }
        self.mt[0] = 0x8000_0000;
        self.index = N;
    }

    fn generate(&mut self) {
        const UPPER: u32 = 0x8000_0000;
        const LOWER: u32 = 0x7fff_ffff;
        const MATRIX_A: u32 = 0x9908_b0df;
        for kk in 0..N {
            let y = (self.mt[kk] & UPPER) | (self.mt[(kk + 1) % N] & LOWER);
            let mut v = self.mt[(kk + M) % N] ^ (y >> 1);
            if y & 1 != 0 {
                v ^= MATRIX_A;
            }
            self.mt[kk] = v;
        }
        self.index = 0;
    }

    /// One tempered 32-bit output (`genrand_uint32`).
    pub fn next_u32(&mut self) -> u32 {
        if self.index >= N {
            self.generate();
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }

    /// `random.random()`: 53-bit float in [0, 1).
    pub fn random(&mut self) -> f64 {
        let a = (self.next_u32() >> 5) as f64;
        let b = (self.next_u32() >> 6) as f64;
        (a * 67_108_864.0 + b) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// `random.getrandbits(k)` for k <= 128.
    pub fn getrandbits(&mut self, k: u32) -> u128 {
        if k == 0 {
            return 0;
        }
        if k <= 32 {
            return (self.next_u32() >> (32 - k)) as u128;
        }
        let mut result: u128 = 0;
        let words = k.div_ceil(32);
        let mut remaining = k;
        for i in 0..words {
            let mut r = self.next_u32();
            if remaining < 32 {
                r >>= 32 - remaining;
            }
            result |= (r as u128) << (32 * i);
            remaining = remaining.saturating_sub(32);
        }
        result
    }

    /// `_randbelow_with_getrandbits(n)`.
    pub fn randbelow(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        let k = 64 - n.leading_zeros();
        loop {
            let r = self.getrandbits(k) as u64;
            if r < n {
                return r;
            }
        }
    }

    /// `random.randint(a, b)` (inclusive).
    pub fn randint(&mut self, a: i64, b: i64) -> i64 {
        a + self.randbelow((b - a + 1) as u64) as i64
    }

    /// `random.randrange(n)`.
    pub fn randrange(&mut self, n: usize) -> usize {
        self.randbelow(n as u64) as usize
    }

    /// `random.uniform(a, b)`.
    pub fn uniform(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.random()
    }

    /// `random.shuffle(x)`.
    pub fn shuffle<T>(&mut self, x: &mut [T]) {
        for i in (1..x.len()).rev() {
            let j = self.randbelow((i + 1) as u64) as usize;
            x.swap(i, j);
        }
    }

    /// `random.choice(seq)`.
    pub fn choice<'a, T>(&mut self, seq: &'a [T]) -> Option<&'a T> {
        if seq.is_empty() {
            return None;
        }
        let i = self.randbelow(seq.len() as u64) as usize;
        seq.get(i)
    }

    /// `random.sample(population, k)` (CPython's set/pool selection).
    pub fn sample<T: Clone>(&mut self, population: &[T], k: usize) -> Vec<T> {
        let n = population.len();
        let k = k.min(n);
        let mut result: Vec<T> = Vec::with_capacity(k);
        let mut setsize = 21usize;
        if k > 5 {
            let exp = ((k * 3) as f64).log(4.0).ceil() as u32;
            setsize += 4usize.pow(exp);
        }
        if n <= setsize {
            let mut pool: Vec<T> = population.to_vec();
            for i in 0..k {
                let j = self.randbelow((n - i) as u64) as usize;
                result.push(pool[j].clone());
                pool[j] = pool[n - i - 1].clone();
            }
        } else {
            let mut selected = std::collections::HashSet::new();
            for _ in 0..k {
                let mut j = self.randbelow(n as u64) as usize;
                while selected.contains(&j) {
                    j = self.randbelow(n as u64) as usize;
                }
                selected.insert(j);
                result.push(population[j].clone());
            }
        }
        result
    }

    /// `random.gauss(mu, sigma)`.
    pub fn gauss(&mut self, mu: f64, sigma: f64) -> f64 {
        let z = match self.gauss_next.take() {
            Some(z) => z,
            None => {
                let x2pi = self.random() * std::f64::consts::TAU;
                let g2rad = (-2.0 * (1.0 - self.random()).ln()).sqrt();
                self.gauss_next = Some(x2pi.sin() * g2rad);
                x2pi.cos() * g2rad
            }
        };
        mu + z * sigma
    }

    /// `random.normalvariate(mu, sigma)` (Kinderman-Monahan).
    pub fn normalvariate(&mut self, mu: f64, sigma: f64) -> f64 {
        let nv_magicconst = 4.0 * (-0.5f64).exp() / 2.0f64.sqrt();
        loop {
            let u1 = self.random();
            let u2 = 1.0 - self.random();
            let z = nv_magicconst * (u1 - 0.5) / u2;
            let zz = z * z / 4.0;
            if zz <= -u2.ln() {
                return mu + z * sigma;
            }
        }
    }
}

thread_local! {
    static GLOBAL: RefCell<PyRandom> = RefCell::new(PyRandom::from_entropy());
}

/// Run `f` against this thread's global generator (Python's module-level
/// `random` functions).
pub fn with_global<R>(f: impl FnOnce(&mut PyRandom) -> R) -> R {
    GLOBAL.with(|g| f(&mut g.borrow_mut()))
}

/// `random.seed(seed)` on the global generator.
pub fn seed_global(seed: i128) {
    with_global(|r| r.seed(seed));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_cpython_seed_42() {
        // python3 -c "import random; random.seed(42); print(random.random(), random.getrandbits(32), random.randint(1,100))"
        let mut r = PyRandom::new(42);
        assert_eq!(r.random(), 0.6394267984578837);
        assert_eq!(r.getrandbits(32), 107_420_369);
    }

    #[test]
    fn shuffle_matches_cpython() {
        // python3 -c "import random; random.seed(1); x=list(range(10)); random.shuffle(x); print(x)"
        let mut r = PyRandom::new(1);
        let mut x: Vec<i32> = (0..10).collect();
        r.shuffle(&mut x);
        assert_eq!(x, vec![6, 8, 9, 7, 5, 3, 0, 4, 1, 2]);
    }

    #[test]
    fn sample_randint_gauss_match_cpython() {
        let mut r = PyRandom::new(3);
        let pop: Vec<i32> = (0..100).collect();
        assert_eq!(r.sample(&pop, 5), vec![30, 75, 69, 16, 47]);
        assert_eq!(r.randint(1, 6), 5);
        assert_eq!(r.gauss(0.0, 1.0), -1.3012496599855783);
    }

    #[test]
    fn uuid_from_bits_matches_cpython() {
        let mut r = PyRandom::new(7);
        let bits = r.getrandbits(128);
        assert_eq!(
            crate::router::primitives::uuid_v4_from_int(bits),
            "6513270e-269e-4d37-b2a7-4de452e6b438"
        );
    }
}
