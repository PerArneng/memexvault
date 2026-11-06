/// Randomness as an injected edge, so ids and samples can be fixed in tests.
pub trait Seed {
    fn next_u64(&self) -> u64;
}
