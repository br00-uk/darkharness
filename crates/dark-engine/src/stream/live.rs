//! Runs a real streaming generation against a loaded `mistralrs::Model`
//! (task unit `B4`, steps 1 and 4).
//!
//! This is the seam that actually calls mistral.rs's streaming API. Like
//! [`crate::load::materialize`] and [`crate::embed::embed_via_model`], it
//! cannot run in this sandbox — no model is loaded here — so it is
//! compile-true against the real API but not exercised by a test in this
//! crate.
//!
//! # Why this spawns a task
//!
//! [`mistralrs::Model::stream_chat_request`] returns a `Stream<'_>`
//! borrowing the `&Model` that produced it, but
//! [`dark_contract::ChunkStream`] is `BoxStream<'static, _>` — every other
//! crate holds the engine as `dyn Engine` with no lifetime to tie a
//! borrowed stream to. [`run`] resolves this the ordinary way, not an
//! `unsafe` one (`unsafe_code` is `forbid` workspace-wide): it spawns a
//! task that owns its own clone of `Arc<mistralrs::Model>`, opens the
//! borrowed `Stream` and drains it entirely inside that task's own stack
//! frame, and forwards each mapped [`dark_contract::Chunk`] out through a
//! fresh channel this function owns. The borrow never has to outlive the
//! task that created it.
//!
//! # Cancellation releases the sequence and its key-value block
//!
//! [`mistralrs::Model`]'s public API has no explicit "cancel this
//! sequence" call; what it does have is a channel: [`mistralrs::Stream`]
//! wraps a `tokio::sync::mpsc::Receiver`, and mistral.rs's engine holds
//! the matching sender. Dropping the receiver closes the channel from the
//! consumer's side, and the engine's next attempt to send on it fails,
//! which is how mistral.rs's own scheduler learns a sequence's consumer
//! has gone away and reclaims its key-value block. The spawned task drops
//! its `Stream` the moment `select!` picks the cancellation branch over
//! the next `Response`, which is what releases it. Confirming that
//! mistral.rs's scheduler reclaims the block promptly, rather than merely
//! stops producing more output, needs a live model and a memory
//! measurement this sandbox cannot make; see `docs/adr/0006`.
//!
//! The turn lease and the concurrency permit [`run`] is handed are
//! released the same way regardless of how the task above ends: normally,
//! on an error, or on cancellation. [`Guard`]'s `Drop` is the one place
//! either is released, and it runs when the outer stream this function
//! returns is dropped, which happens once the spawned task's sender side
//! closes. [`crate::resident::ResidentSet::outstanding_leases`] and
//! [`super::concurrency::Limiter::available`] are what the `cancel_leak`
//! test (`crates/dark-engine/tests/cancel_leak.rs`) checks return to
//! baseline after 1000 cancelled turns.

use std::sync::{Arc, Mutex};

use dark_contract::{Caps, Chunk, ChunkStream, ErrCode, Error, FinishReason, Request, Result};
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::resident::{ModelKey, ResidentSet, TurnId};

use super::concurrency::{Limiter, Permit};
use super::{request, response};

/// How many mapped chunks the forwarding channel buffers before the
/// spawned task's send blocks. Generous enough that one `Response`'s
/// worth of mapped chunks (rarely more than two or three) never has to
/// wait on a slow consumer mid-batch.
const CHANNEL_CAPACITY: usize = 32;

/// Releases a turn lease and a concurrency permit together, on drop.
///
/// Holding both in one guard is what makes their release unconditional:
/// there is exactly one place either is released, and it runs no matter
/// how the task that owns this guard ends.
struct Guard {
    resident: Arc<Mutex<ResidentSet>>,
    turn: TurnId,
    _permit: Permit,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(mut resident) = self.resident.lock() {
            resident.release_turn(&self.turn);
        }
    }
}

/// Starts a streaming generation for `req` against `model`, which must
/// already serve `key` in `resident` (see
/// [`crate::resident::ResidentSet::key_for_class`]).
///
/// Acquires a turn lease on `key` (Rule 3: the resident set manager never
/// evicts a leased model) and a concurrency permit from `limiter` before
/// sending anything to mistral.rs, and releases both when the spawned task
/// this starts ends — reaching the stream's end, erroring, or being
/// cancelled all end it eventually, so all three paths release the same
/// way.
///
/// # Errors
///
/// Returns [`ErrCode::EngineWontFit`] when `limiter` has no free permit, or
/// when `key` has no `Loaded` slot in `resident`. Returns
/// [`ErrCode::ToolInvalidArgs`] when `req.tool_choice` names an unknown
/// tool (see [`request::build`]).
// Eight arguments and no await yet: this is the seam the real-hardware
// work reshapes when live streaming lands (see the deferral note above),
// and bundling the arguments now would only be unbundled then. The
// signature stays async because the Engine trait calls it as async.
#[allow(clippy::too_many_arguments, clippy::unused_async)]
pub async fn run(
    model: Arc<mistralrs::Model>,
    key: &ModelKey,
    resident: Arc<Mutex<ResidentSet>>,
    limiter: &Limiter,
    req: &Request,
    caps: &Caps,
    turn: TurnId,
    cancel: CancellationToken,
) -> Result<ChunkStream> {
    let permit = limiter.try_acquire()?;
    {
        let mut guard = resident.lock().map_err(|_| {
            Error::new(ErrCode::EngineGenerate, "the resident set lock is poisoned")
        })?;
        guard.acquire_turn_lease(turn.clone(), key.clone())?;
    }
    let release = Guard {
        resident,
        turn,
        _permit: permit,
    };

    let builder = request::build(req, caps)?;
    let (tx, rx) = tokio::sync::mpsc::channel(CHANNEL_CAPACITY);
    let task_cancel = cancel.clone();

    tokio::spawn(async move {
        let cancel = task_cancel;
        // `release` moves in here: it, and the lease and permit it holds,
        // live exactly as long as this task does.
        let _release = release;

        let mut inner = match model.stream_chat_request(builder).await {
            Ok(stream) => stream,
            Err(source) => {
                let _ = tx
                    .send(Err(Error::new(
                        ErrCode::EngineGenerate,
                        format!("could not start the request: {source}"),
                    )))
                    .await;
                return;
            }
        };

        loop {
            let next = tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    let _ = tx.send(Ok(Chunk::Done(FinishReason::Cancelled))).await;
                    return;
                }
                next = inner.next() => next,
            };

            let Some(item) = next else {
                let _ = tx.send(Ok(Chunk::Done(FinishReason::Stop))).await;
                return;
            };

            let mapped = response::map(&item);
            let saw_done = mapped.iter().any(|chunk| matches!(chunk, Chunk::Done(_)));
            for chunk in mapped {
                if tx.send(Ok(chunk)).await.is_err() {
                    // The receiver dropped: the caller stopped listening.
                    // Nothing left to forward; the task ends and the
                    // `Guard` above releases the lease and the permit.
                    return;
                }
            }
            if saw_done {
                return;
            }
        }
    });

    let receiver_stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    Ok(Box::pin(CancelOnDrop {
        inner: receiver_stream.boxed(),
        cancel,
    }))
}

/// Wraps a [`ChunkStream`], cancelling `cancel` when this wrapper drops.
///
/// [`dark_contract::Engine::stream`] documents two ways to cancel a
/// request: the token, and dropping the stream. The spawned task in
/// [`run`] already reacts to the token; this struct is what makes
/// dropping the stream equivalent to cancelling it, by cancelling the
/// token on the caller's behalf the moment the stream goes away — whether
/// that is because generation finished, the caller lost interest, or the
/// stream was never polled to completion at all.
struct CancelOnDrop {
    inner: ChunkStream,
    cancel: CancellationToken,
}

impl futures_core::Stream for CancelOnDrop {
    type Item = Result<Chunk>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        // `Self` is `Unpin` (both fields are: `ChunkStream` is already a
        // `Pin<Box<_>>`, and `CancellationToken` is a plain `Arc` handle),
        // so projecting out `inner` needs no `unsafe`.
        self.get_mut().inner.as_mut().poll_next(cx)
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
