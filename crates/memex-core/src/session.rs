//! Browser session rules that need no IO: how long a session lives, and the limit on failed sign-ins.
use std::collections::HashMap;

/// A session ends after this long, however much it's used. Revoking its token ends it sooner.
pub const SESSION_TTL_MS: i64 = 12 * 60 * 60 * 1000;
/// Failed sign-ins a client gets per window. After that it waits, even with the right token.
pub const LOGIN_FAILURE_LIMIT: u32 = 10;
pub const LOGIN_WINDOW_MS: i64 = 60_000;
/// Clients tracked one by one. Past this they share one count, so a flood of addresses can't grow memory.
const TRACKED_CLIENTS: usize = 10_000;
const SHARED: &str = "*";

#[derive(Debug, Clone, Copy)]
struct Window {
    failures: u32,
    started_ms: i64,
}

#[derive(Debug, Default)]
pub struct LoginLimiter {
    clients: HashMap<String, Window>,
}

impl LoginLimiter {
    fn key<'a>(&self, client: &'a str) -> &'a str {
        if self.clients.contains_key(client) || self.clients.len() < TRACKED_CLIENTS {
            client
        } else {
            SHARED
        }
    }

    /// `Err` holds the seconds to wait.
    pub fn check(&self, client: &str, now_ms: i64) -> Result<(), u64> {
        match self.clients.get(self.key(client)) {
            Some(w) if w.failures >= LOGIN_FAILURE_LIMIT && now_ms - w.started_ms < LOGIN_WINDOW_MS => {
                Err(((LOGIN_WINDOW_MS - (now_ms - w.started_ms)) as u64).div_ceil(1000))
            }
            _ => Ok(()),
        }
    }

    pub fn failed(&mut self, client: &str, now_ms: i64) {
        if self.clients.len() >= TRACKED_CLIENTS {
            self.clients.retain(|_, w| now_ms - w.started_ms < LOGIN_WINDOW_MS);
        }
        let key = self.key(client).to_string();
        let window = self.clients.entry(key).or_insert(Window {
            failures: 0,
            started_ms: now_ms,
        });
        if now_ms - window.started_ms >= LOGIN_WINDOW_MS {
            *window = Window {
                failures: 0,
                started_ms: now_ms,
            };
        }
        window.failures += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fail(l: &mut LoginLimiter, client: &str, n: u32, at: i64) {
        for _ in 0..n {
            l.failed(client, at);
        }
    }

    #[test]
    fn a_client_is_refused_after_the_limit_and_told_how_long_to_wait() {
        let mut l = LoginLimiter::default();
        fail(&mut l, "a", LOGIN_FAILURE_LIMIT - 1, 1_000);
        assert_eq!(l.check("a", 1_000), Ok(()));
        l.failed("a", 1_000);
        assert_eq!(l.check("a", 1_000), Err(60));
        assert_eq!(l.check("a", 1_000 + 59_500), Err(1));
    }

    #[test]
    fn the_window_ends_and_the_count_starts_over() {
        let mut l = LoginLimiter::default();
        fail(&mut l, "a", LOGIN_FAILURE_LIMIT, 0);
        assert_eq!(l.check("a", LOGIN_WINDOW_MS), Ok(()));
        l.failed("a", LOGIN_WINDOW_MS);
        assert_eq!(l.check("a", LOGIN_WINDOW_MS), Ok(()));
    }

    #[test]
    fn one_client_does_not_lock_out_another() {
        let mut l = LoginLimiter::default();
        fail(&mut l, "a", LOGIN_FAILURE_LIMIT, 0);
        assert_eq!(l.check("b", 0), Ok(()));
    }

    #[test]
    fn a_flood_of_clients_stays_bounded_and_shares_one_count() {
        let mut l = LoginLimiter::default();
        for i in 0..TRACKED_CLIENTS + 50 {
            l.failed(&format!("c{i}"), 0);
        }
        assert_eq!(l.clients.len(), TRACKED_CLIENTS + 1);
        assert_eq!(l.check("someone-new", 0), Err(60));
        // Old entries leave once their window is over.
        l.failed("later", LOGIN_WINDOW_MS);
        assert_eq!(l.clients.len(), 1);
    }
}
