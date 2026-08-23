//! Polls [`EventRx`] without a `tokio` runtime.
//!
//! `dark-tui` depends on `dark-contract` only (Rule 14 in `CLAUDE.md`), so it
//! cannot take its own dependency on `tokio` merely to drive
//! [`EventRx::recv`]'s async function. That function never waits on a timer
//! or an I/O driver — the two things that genuinely need a running Tokio
//! runtime underneath — so one poll with a waker that does nothing is
//! enough: either an event is already queued, in which case the poll
//! returns it immediately, or none is, in which case the caller tries again
//! on its next pass through the redraw loop. That is exactly the same
//! pattern the redraw loop already uses for a `crossterm` input poll.

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use dark_contract::{EventRx, Received};

/// What one non-blocking poll of an [`EventRx`] found.
#[derive(Debug)]
pub enum PollOutcome {
    /// Neither channel has anything ready. Poll again later.
    Pending,
    /// One event arrived.
    Event(Received),
    /// Both channels closed. No further call will ever return an event.
    Closed,
}

/// Polls `rx` once for a queued event, without blocking.
///
/// This never waits: it checks whatever is already buffered and returns
/// [`PollOutcome::Pending`] the instant neither channel has more to give.
pub fn try_recv(rx: &mut EventRx) -> PollOutcome {
    let future = rx.recv();
    let mut future = pin!(future);
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(Some(received)) => PollOutcome::Event(received),
        Poll::Ready(None) => PollOutcome::Closed,
        Poll::Pending => PollOutcome::Pending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_contract::{Event, EventBus};

    #[test]
    fn an_empty_bus_is_pending() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        assert!(matches!(try_recv(&mut rx), PollOutcome::Pending));
    }

    #[test]
    fn a_queued_notice_is_returned() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.tx().notice("hello");
        match try_recv(&mut rx) {
            PollOutcome::Event(Received::Event(Event::Notice(text))) => assert_eq!(text, "hello"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn a_dropped_bus_reports_closed() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        drop(bus);
        assert!(matches!(try_recv(&mut rx), PollOutcome::Closed));
    }

    #[test]
    fn an_overflowed_lossy_channel_reports_lagged_without_losing_the_reliable_event() {
        let bus = EventBus::with_capacity(2, 64);
        let mut rx = bus.subscribe();
        let tx = bus.tx();

        for i in 0..50 {
            tx.send(Event::TokenDelta {
                turn: "t1".into(),
                text: i.to_string(),
            });
        }
        tx.send(Event::DarkChanged { dark: true });

        let mut saw_lag = false;
        let mut saw_dark_changed = false;
        // Poll well past what is buffered; try_recv never blocks, so this
        // terminates after a bounded number of Pending results.
        for _ in 0..200 {
            match try_recv(&mut rx) {
                PollOutcome::Event(Received::Lagged(_)) => saw_lag = true,
                PollOutcome::Event(Received::Event(Event::DarkChanged { dark })) => {
                    assert!(dark);
                    saw_dark_changed = true;
                }
                PollOutcome::Event(_) | PollOutcome::Pending | PollOutcome::Closed => {}
            }
        }

        assert!(
            saw_lag,
            "the lossy overflow must surface as Received::Lagged"
        );
        assert!(
            saw_dark_changed,
            "the reliable event must survive the flood"
        );
    }
}
