use std::time::Duration;

/// Time as an injected edge, so no policy reads the system clock.
pub trait Clock {
    /// Wall time as milliseconds since the Unix epoch, for recording.
    fn now_ms(&self) -> i64;
    /// Monotonic time since an arbitrary start, for measuring.
    fn monotonic(&self) -> Duration;
}
