//! Limits concurrent sequences from resident-set headroom (task unit `B4`,
//! step 5).
//!
//! Parallel sub-sessions consume memory: each running sequence holds its
//! own key-value cache block. [`max_concurrent_sequences`] is the pure
//! arithmetic behind the limit; [`Limiter`] is the semaphore a caller
//! acquires a permit from before starting a sequence, and releases (via
//! [`Permit`]'s `Drop`) when the sequence ends — cancelled or not, since
//! `Drop` runs either way. This is what makes the `cancel_leak` property
//! true structurally rather than by convention: there is no code path that
//! starts a sequence without eventually dropping its `Permit`.

use std::sync::Arc;

use tokio::sync::{Semaphore, TryAcquireError};

use dark_contract::{ErrCode, Error, Result};

/// Returns how many concurrent sequences `free_bytes` of resident-set
/// headroom can afford, at `per_sequence_bytes` each.
///
/// Always returns at least `1`: a caller with zero measured headroom still
/// gets one sequence, because the alternative — refusing every turn until
/// `dark tune` or a load frees something — would make the harness refuse
/// to answer at all on a machine that is merely tight on memory rather
/// than genuinely out of it. [`super::live`]'s load path is what actually
/// enforces Rule 4's "refuse a load that does not fit"; this limit is
/// about sequences sharing an already-loaded model's batch, not about
/// whether the model itself fits.
#[must_use]
pub fn max_concurrent_sequences(free_bytes: u64, per_sequence_bytes: u64) -> usize {
    if per_sequence_bytes == 0 {
        return 1;
    }
    usize::try_from(free_bytes / per_sequence_bytes)
        .unwrap_or(usize::MAX)
        .max(1)
}

/// A semaphore sized from resident-set headroom (see
/// [`max_concurrent_sequences`]).
#[derive(Debug, Clone)]
pub struct Limiter {
    semaphore: Arc<Semaphore>,
}

/// A held concurrency slot. Dropping it — on a normal return or on
/// cancellation — returns the slot, so 1000 cancelled turns leave the
/// limiter exactly where they found it (the `cancel_leak` test asserts
/// this).
#[derive(Debug)]
pub struct Permit {
    _inner: tokio::sync::OwnedSemaphorePermit,
}

impl Limiter {
    /// Creates a limiter that allows `max_concurrent` sequences at once.
    #[must_use]
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent.max(1))),
        }
    }

    /// Creates a limiter sized from resident-set headroom (see
    /// [`max_concurrent_sequences`]).
    #[must_use]
    pub fn from_headroom(free_bytes: u64, per_sequence_bytes: u64) -> Self {
        Self::new(max_concurrent_sequences(free_bytes, per_sequence_bytes))
    }

    /// Returns how many permits are free right now.
    #[must_use]
    pub fn available(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// Tries to acquire one permit without waiting.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineWontFit`] when every permit is already in
    /// use.
    pub fn try_acquire(&self) -> Result<Permit> {
        self.semaphore
            .clone()
            .try_acquire_owned()
            .map(|inner| Permit { _inner: inner })
            .map_err(|err| match err {
                TryAcquireError::NoPermits => Error::new(
                    ErrCode::EngineWontFit,
                    "every concurrent sequence slot is in use",
                )
                .with_remedy("Wait for a running turn to finish, or reduce concurrent turns."),
                TryAcquireError::Closed => {
                    Error::new(ErrCode::EngineWontFit, "the concurrency limiter is closed")
                }
            })
    }

    /// Waits for one permit.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineWontFit`] when the limiter is closed, which
    /// this crate never does — this is here only because
    /// [`tokio::sync::Semaphore::acquire_owned`] returns a `Result`.
    pub async fn acquire(&self) -> Result<Permit> {
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .map(|inner| Permit { _inner: inner })
            .map_err(|_closed| {
                Error::new(ErrCode::EngineWontFit, "the concurrency limiter is closed")
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_concurrent_sequences_divides_headroom_by_the_per_sequence_cost() {
        assert_eq!(max_concurrent_sequences(8 * 1024, 1024), 8);
    }

    #[test]
    fn max_concurrent_sequences_is_never_less_than_one() {
        assert_eq!(max_concurrent_sequences(0, 1024), 1);
        assert_eq!(max_concurrent_sequences(100, 1024), 1);
    }

    #[test]
    fn a_zero_per_sequence_cost_still_allows_one() {
        assert_eq!(max_concurrent_sequences(1024, 0), 1);
    }

    #[tokio::test]
    async fn a_permit_is_returned_when_dropped() {
        let limiter = Limiter::new(1);
        assert_eq!(limiter.available(), 1);
        {
            let _permit = limiter.try_acquire().unwrap();
            assert_eq!(limiter.available(), 0);
        }
        assert_eq!(limiter.available(), 1, "dropping the permit frees the slot");
    }

    #[tokio::test]
    async fn try_acquire_fails_with_engine_wont_fit_when_exhausted() {
        let limiter = Limiter::new(1);
        let _held = limiter.try_acquire().unwrap();
        let err = limiter.try_acquire().unwrap_err();
        assert_eq!(err.code, ErrCode::EngineWontFit);
    }

    #[tokio::test]
    async fn one_thousand_acquire_and_drop_cycles_return_to_baseline() {
        // The cancel_leak property, exercised directly against the
        // limiter: every acquired permit is dropped (simulating a
        // cancelled turn releasing its slot), so availability never
        // drifts from where it started.
        let limiter = Limiter::new(4);
        let baseline = limiter.available();
        for _ in 0..1000 {
            let permit = limiter.acquire().await.unwrap();
            drop(permit);
        }
        assert_eq!(limiter.available(), baseline);
    }
}
