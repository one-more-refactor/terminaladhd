//! A tiny xorshift64* generator — the games need "where does the apple
//! go", not cryptography, and `save` ships zero extra dependencies.

pub struct Rng(u64);

impl Rng {
    pub fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64 ^ d.as_secs())
            .unwrap_or(0x9e3779b97f4a7c15);
        Self::from_seed(nanos ^ ((std::process::id() as u64) << 32) | 1)
    }

    pub fn from_seed(seed: u64) -> Self {
        Self(if seed == 0 { 0xdeadbeefcafe } else { seed })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545f4914f6cdd1d)
    }

    /// Uniform-ish value in `0..n` (n = 0 returns 0).
    pub fn range(&mut self, n: u32) -> u32 {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as u32
        }
    }
}

impl Default for Rng {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_from_seed() {
        let mut a = Rng::from_seed(42);
        let mut b = Rng::from_seed(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn range_stays_in_bounds() {
        let mut rng = Rng::from_seed(7);
        for _ in 0..1000 {
            assert!(rng.range(10) < 10);
        }
        assert_eq!(rng.range(0), 0);
    }

    #[test]
    fn zero_seed_still_generates() {
        let mut rng = Rng::from_seed(0);
        assert_ne!(rng.next_u64(), rng.next_u64());
    }
}
