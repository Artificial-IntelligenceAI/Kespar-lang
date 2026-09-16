//! A small, fast, seedable generator (SplitMix64 + xoshiro-style mixing).
//! No dependency: a fuzzer's numbers should be reproducible forever.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_add(0x9E3779B97F4A7C15))
    }
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
    pub fn chance(&mut self, p: f64) -> bool {
        ((self.next() >> 11) as f64 / (1u64 << 53) as f64) < p
    }
}
