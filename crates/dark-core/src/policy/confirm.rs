//! The seam that lets [`super::Policy::decide`] block for an answer without
//! hardcoding a global event bus.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use dark_contract::{Allow, ConfirmPrompt, Event, EventTx};
use tokio::sync::{Mutex, oneshot};

/// Presents a [`ConfirmPrompt`] to a person and returns their answer.
///
/// This trait is the seam that keeps [`super::Policy::decide`] testable
/// without a real session: a test supplies a canned [`Confirmer`], and the
/// turn loop supplies [`ChannelConfirmer`], which emits
/// [`dark_contract::Event::ConfirmReq`] and waits for the matching
/// [`dark_contract::Intent::Confirm`].
#[async_trait]
pub trait Confirmer: Send + Sync {
    /// Presents `prompt` and waits for the answer.
    async fn confirm(&self, prompt: ConfirmPrompt) -> Allow;
}

/// A [`Confirmer`] that emits [`Event::ConfirmReq`] on an [`EventTx`] and
/// waits for the turn loop to feed the matching answer back through
/// [`ChannelConfirmer::resolve`].
///
/// This is the mechanism Do step 2 of task unit `A4` describes: emit the
/// request, then block until the answer arrives. The blocking happens
/// entirely behind the [`Confirmer`] trait, so [`super::Policy`] itself never
/// touches an event bus or a session.
#[derive(Debug)]
pub struct ChannelConfirmer {
    tx: EventTx,
    next_id: AtomicU64,
    pending: Mutex<HashMap<String, oneshot::Sender<Allow>>>,
}

impl ChannelConfirmer {
    /// Creates a confirmer that sends [`Event::ConfirmReq`] on `tx`.
    #[must_use]
    pub fn new(tx: EventTx) -> Self {
        Self {
            tx,
            next_id: AtomicU64::new(0),
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Feeds an [`dark_contract::Intent::Confirm`] answer back to the
    /// matching pending [`Confirmer::confirm`] call.
    ///
    /// Returns `true` when `id` matched a pending request. A caller that
    /// receives `false` saw an answer with no matching request, for example
    /// a duplicate or a stale intent, and should ignore it.
    pub async fn resolve(&self, id: &str, allow: Allow) -> bool {
        let sender = self.pending.lock().await.remove(id);
        match sender {
            Some(sender) => sender.send(allow).is_ok(),
            None => false,
        }
    }

    /// Returns the number of requests waiting for an answer.
    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.len()
    }
}

#[async_trait]
impl Confirmer for ChannelConfirmer {
    async fn confirm(&self, prompt: ConfirmPrompt) -> Allow {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), sender);
        self.tx.send(Event::ConfirmReq { id, prompt });

        // The sender side only drops without sending when the harness shuts
        // down mid-confirmation. Fail closed: treat that as a denial rather
        // than as an allow.
        receiver.await.unwrap_or(Allow::Deny)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use dark_contract::{EventBus, Received};

    use super::*;

    /// A confirmer that always answers the same way, for tests that do not
    /// care about the emitted event or the id round-trip.
    struct FixedConfirmer(Allow);

    #[async_trait]
    impl Confirmer for FixedConfirmer {
        async fn confirm(&self, _prompt: ConfirmPrompt) -> Allow {
            self.0
        }
    }

    #[tokio::test]
    async fn fixed_confirmer_returns_its_configured_answer() {
        let confirmer = FixedConfirmer(Allow::Once);
        let prompt = ConfirmPrompt::Exec {
            command: "ls".into(),
            cwd: PathBuf::from("."),
            shell: false,
        };
        assert_eq!(confirmer.confirm(prompt).await, Allow::Once);
    }

    #[tokio::test]
    async fn channel_confirmer_emits_the_exact_prompt_on_the_event_bus() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let confirmer = ChannelConfirmer::new(bus.tx());

        let diff = "@@ -1 +1 @@\n-a\n+b\n";
        let prompt = ConfirmPrompt::Write {
            path: PathBuf::from("src/lib.rs"),
            diff: diff.to_owned(),
        };

        let confirm_task = tokio::spawn(async move { confirmer.confirm(prompt).await });

        // The task blocks on the answer, so the request must already be on
        // the bus.
        let received = rx.recv().await.expect("bus is open");
        let Received::Event(Event::ConfirmReq { id, prompt }) = received else {
            panic!("expected a ConfirmReq, got {received:?}");
        };
        match prompt {
            ConfirmPrompt::Write { path, diff: shown } => {
                assert_eq!(path, PathBuf::from("src/lib.rs"));
                assert_eq!(shown, diff, "the exact diff must reach the event, not a summary");
            }
            other => panic!("unexpected prompt: {other:?}"),
        }
        assert_eq!(id, "0");

        // confirm_task is still pending here: nothing has resolved it yet.
        assert!(!confirm_task.is_finished());
        confirm_task.abort();
    }

    #[tokio::test]
    async fn channel_confirmer_blocks_until_resolve_is_called() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let confirmer = Arc::new(ChannelConfirmer::new(bus.tx()));

        let prompt = ConfirmPrompt::Exec {
            command: "cargo test".into(),
            cwd: PathBuf::from("/repo"),
            shell: false,
        };

        let waiter = Arc::clone(&confirmer);
        let handle = tokio::spawn(async move { waiter.confirm(prompt).await });

        let Received::Event(Event::ConfirmReq { id, .. }) =
            rx.recv().await.expect("bus is open")
        else {
            panic!("expected a ConfirmReq");
        };

        // Give the spawned task every opportunity to finish early; it must
        // not, because nothing has answered yet.
        tokio::task::yield_now().await;
        assert!(!handle.is_finished());

        assert!(confirmer.resolve(&id, Allow::Always).await);
        let answer = tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("confirm() must return once resolve() is called")
            .unwrap();
        assert_eq!(answer, Allow::Always);
    }

    #[tokio::test]
    async fn resolve_with_an_unknown_id_reports_no_match() {
        let bus = EventBus::new();
        let confirmer = ChannelConfirmer::new(bus.tx());
        assert!(!confirmer.resolve("no-such-id", Allow::Deny).await);
    }

    #[tokio::test]
    async fn dropping_the_confirmer_fails_closed_as_deny() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let confirmer = ChannelConfirmer::new(bus.tx());

        let prompt = ConfirmPrompt::Other {
            summary: "test".into(),
            detail: "test".into(),
        };
        let handle = tokio::spawn(async move { confirmer.confirm(prompt).await });
        // Drain the request so the task can proceed to awaiting the answer,
        // then drop the confirmer (and with it, every pending sender).
        let _ = rx.recv().await;

        let answer = handle.await.unwrap();
        assert_eq!(answer, Allow::Deny);
    }
}
