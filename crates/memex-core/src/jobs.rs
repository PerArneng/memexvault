//! The job queue's rules, as pure functions on plain data.
use memex_types::{Header, JobAttempt, JobRow, JobState, JobWait, ObjectId, Status};
use serde_json::Value;

/// How long a worker may hold a job before it counts as dead.
pub const LEASE_MS: i64 = 60_000;
/// How long a waiting job sleeps at most, so it picks up soon after whatever it waits for comes back.
pub const WAIT_CAP_MS: i64 = 300_000;

/// Exponential, capped at an hour, with jitter from `jitter`, so failures don't retry in step.
pub fn backoff_ms(attempt: u32, jitter: u64) -> i64 {
    let base = 2_000i64.saturating_mul(1i64 << attempt.saturating_sub(1).min(20));
    let capped = base.min(3_600_000);
    capped + (jitter % (capped as u64 / 4 + 1)) as i64
}

pub fn new_job(id: ObjectId, creator: &str, kind: &str, input: Value, dedupe_key: String, max_attempts: u32, now_ms: i64) -> JobRow {
    JobRow {
        header: Header {
            id,
            creator: creator.into(),
            status: Status::Active,
            version: 1,
            acl_id: None,
        },
        kind: kind.into(),
        state: JobState::Queued,
        input,
        dedupe_key,
        max_attempts: max_attempts.max(1),
        next_run_at_ms: now_ms,
        lease_owner: None,
        lease_until_ms: None,
        attempts: vec![],
        created_at_ms: now_ms,
        finished_at_ms: None,
        wait: None,
    }
}

/// Takes a job for a worker. A running job whose lease expired had its attempt end in a crash, which is recorded. If that used up its attempts, it goes dead instead of running again.
pub fn claimed(mut job: JobRow, owner: &str, now_ms: i64) -> JobRow {
    if job.state == JobState::Running {
        if let Some(last) = job.attempts.last_mut() {
            last.finished_at_ms = Some(now_ms);
            last.error = Some("the worker stopped without finishing".into());
        }
        if job.attempts.len() as u32 >= job.max_attempts {
            job.state = JobState::Dead;
            job.lease_owner = None;
            job.lease_until_ms = None;
            job.finished_at_ms = Some(now_ms);
            job.header.version += 1;
            return job;
        }
    }
    job.state = JobState::Running;
    job.lease_owner = Some(owner.into());
    job.lease_until_ms = Some(now_ms + LEASE_MS);
    job.attempts.push(JobAttempt {
        started_at_ms: now_ms,
        finished_at_ms: None,
        error: None,
    });
    job.header.version += 1;
    job
}

pub fn succeeded(mut job: JobRow, now_ms: i64) -> JobRow {
    if let Some(last) = job.attempts.last_mut() {
        last.finished_at_ms = Some(now_ms);
    }
    job.state = JobState::Done;
    job.wait = None;
    job.lease_owner = None;
    job.lease_until_ms = None;
    job.finished_at_ms = Some(now_ms);
    job.header.version += 1;
    job
}

/// Back to the queue with a delay, or dead when out of attempts. `retryable: false` goes straight to dead, for input that can never work.
pub fn failed(mut job: JobRow, error: &str, retryable: bool, now_ms: i64, jitter: u64) -> JobRow {
    if let Some(last) = job.attempts.last_mut() {
        last.finished_at_ms = Some(now_ms);
        last.error = Some(error.into());
    }
    job.wait = None;
    job.lease_owner = None;
    job.lease_until_ms = None;
    job.header.version += 1;
    let attempts = job.attempts.len() as u32;
    if !retryable || attempts >= job.max_attempts {
        job.state = JobState::Dead;
        job.finished_at_ms = Some(now_ms);
    } else {
        job.state = JobState::Queued;
        job.next_run_at_ms = now_ms + backoff_ms(attempts, jitter);
    }
    job
}

/// Back to the queue because something it needs is away. The attempt is taken back, so an outage of any length can't kill the job or grow its row.
pub fn waiting(mut job: JobRow, error: &str, now_ms: i64, jitter: u64) -> JobRow {
    job.attempts.pop();
    let (since_ms, tries) = job.wait.as_ref().map_or((now_ms, 1), |w| (w.since_ms, w.tries.saturating_add(1)));
    job.wait = Some(JobWait {
        since_ms,
        tries,
        last_error: error.into(),
    });
    job.state = JobState::Queued;
    job.lease_owner = None;
    job.lease_until_ms = None;
    job.next_run_at_ms = now_ms + backoff_ms(tries, jitter).min(WAIT_CAP_MS);
    job.header.version += 1;
    job
}

/// Gives the holder longer, for work that takes longer than the usual lease.
pub fn lease_extended(mut job: JobRow, until_ms: i64) -> JobRow {
    job.lease_until_ms = Some(until_ms);
    job.header.version += 1;
    job
}

/// A person's retry of a dead or cancelled job. Its history stays, and it gets a fresh set of attempts.
pub fn retried(mut job: JobRow, now_ms: i64) -> Option<JobRow> {
    if !matches!(job.state, JobState::Dead | JobState::Cancelled) {
        return None;
    }
    job.state = JobState::Queued;
    job.next_run_at_ms = now_ms;
    job.finished_at_ms = None;
    job.max_attempts = job.attempts.len() as u32 + job.max_attempts.max(1);
    job.header.version += 1;
    Some(job)
}

/// Only work that hasn't finished can be cancelled.
pub fn cancelled(mut job: JobRow, now_ms: i64) -> Option<JobRow> {
    if !matches!(job.state, JobState::Queued | JobState::Running) {
        return None;
    }
    job.state = JobState::Cancelled;
    job.lease_owner = None;
    job.lease_until_ms = None;
    job.finished_at_ms = Some(now_ms);
    job.header.version += 1;
    Some(job)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn job(max: u32) -> JobRow {
        new_job(ObjectId("j".into()), "c", "k", json!({}), "d".into(), max, 100)
    }

    #[test]
    fn backoff_grows_is_capped_and_has_bounded_jitter() {
        assert_eq!(backoff_ms(1, 0), 2_000);
        assert_eq!(backoff_ms(2, 0), 4_000);
        assert_eq!(backoff_ms(50, 0), 3_600_000);
        assert!(backoff_ms(1, u64::MAX) <= 2_500);
        assert!(backoff_ms(0, 0) > 0);
    }

    #[test]
    fn a_queued_job_runs_when_due_and_not_before() {
        let mut j = job(3);
        j.next_run_at_ms = 500;
        assert!(!j.claimable(499));
        assert!(j.claimable(500));
    }

    #[test]
    fn a_claim_takes_a_lease_and_a_live_lease_is_not_claimable() {
        let j = claimed(job(3), "w1", 1000);
        assert_eq!(
            (j.state, j.lease_owner.as_deref(), j.lease_until_ms),
            (JobState::Running, Some("w1"), Some(1000 + LEASE_MS))
        );
        assert!(!j.claimable(1000 + LEASE_MS - 1));
        assert!(j.claimable(1000 + LEASE_MS));
    }

    #[test]
    fn an_expired_lease_records_the_crash_and_gives_another_attempt() {
        let j = claimed(claimed(job(3), "w1", 1000), "w2", 1000 + LEASE_MS);
        assert_eq!(j.attempts.len(), 2);
        assert!(j.attempts[0].error.as_ref().unwrap().contains("stopped"));
        assert_eq!(j.lease_owner.as_deref(), Some("w2"));
    }

    #[test]
    fn a_job_that_keeps_crashing_its_worker_goes_dead() {
        let j = claimed(claimed(job(1), "w1", 1000), "w2", 1000 + LEASE_MS);
        assert_eq!(j.state, JobState::Dead);
        assert_eq!(j.attempts.len(), 1);
    }

    #[test]
    fn failure_retries_with_backoff_until_out_of_attempts() {
        let j = failed(claimed(job(2), "w", 1000), "boom", true, 2000, 0);
        assert_eq!((j.state, j.next_run_at_ms), (JobState::Queued, 4000));
        assert_eq!(j.attempts[0].error.as_deref(), Some("boom"));
        let j = failed(claimed(j, "w", 5000), "boom2", true, 6000, 0);
        assert_eq!(j.state, JobState::Dead);
        assert_eq!(j.attempts.len(), 2);
    }

    #[test]
    fn a_permanent_failure_goes_straight_to_dead() {
        assert_eq!(failed(claimed(job(5), "w", 1), "bad input", false, 2, 0).state, JobState::Dead);
    }

    #[test]
    fn retry_only_applies_to_dead_or_cancelled_and_grants_new_attempts() {
        let dead = failed(claimed(job(1), "w", 1), "x", true, 2, 0);
        let again = retried(dead, 10).unwrap();
        assert_eq!((again.state, again.max_attempts, again.next_run_at_ms), (JobState::Queued, 2, 10));
        assert!(retried(again, 11).is_none());
    }

    #[test]
    fn waiting_uses_no_attempts_and_its_delay_is_capped() {
        let mut j = job(1);
        for i in 0..40 {
            j = waiting(claimed(j, "w", 1000 + i), "down", 2000, 0);
            assert_eq!(j.state, JobState::Queued, "{i}");
            assert!(j.attempts.is_empty());
            assert!(j.next_run_at_ms <= 2000 + WAIT_CAP_MS);
        }
        let w = j.wait.as_ref().unwrap();
        assert_eq!((w.since_ms, w.tries, w.last_error.as_str()), (2000, 40, "down"));
        assert_eq!(j.next_run_at_ms, 2000 + WAIT_CAP_MS);
    }

    #[test]
    fn a_waiting_job_keeps_the_earlier_attempts_and_is_fenced_from_its_old_worker() {
        let failed_once = failed(claimed(job(3), "w", 1), "boom", true, 2, 0);
        let held = claimed(failed_once, "w", 10_000);
        let waited = waiting(held.clone(), "down", 10_001, 0);
        assert_eq!(waited.attempts.len(), 1);
        assert_eq!(waited.attempts[0].error.as_deref(), Some("boom"));
        assert_ne!(waited.state, JobState::Running);
    }

    #[test]
    fn extending_a_lease_moves_only_the_lease() {
        let j = claimed(job(1), "w", 1000);
        let e = lease_extended(j.clone(), 500_000);
        assert_eq!(e.lease_until_ms, Some(500_000));
        assert!(!e.claimable(499_999));
        assert_eq!((e.state, e.lease_owner, e.attempts.len()), (j.state, j.lease_owner, j.attempts.len()));
    }

    #[test]
    fn cancel_only_applies_to_unfinished_work() {
        let c = cancelled(job(1), 5).unwrap();
        assert_eq!(c.state, JobState::Cancelled);
        assert!(cancelled(c, 6).is_none());
        assert!(cancelled(succeeded(claimed(job(1), "w", 1), 2), 3).is_none());
    }
}
