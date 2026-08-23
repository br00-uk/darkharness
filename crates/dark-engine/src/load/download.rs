//! Downloads weights through `dark-airlock`, reporting progress at 2 Hz or
//! faster (task unit `B2`, steps 4 and 6).
//!
//! [`drain_to_file`] holds the progress-gating logic: it is generic over
//! [`ByteSource`], so a test drives it with a fake, artificially slow
//! source and asserts on the [`dark_contract::Chunk::ModelLoading`] events
//! it produces, with no network and no timing flakiness (the test pauses
//! tokio's clock and advances it by hand). [`download_via_airlock`] is the
//! real path: it repeats the same loop against a live
//! [`dark_airlock::Client`] response.
//!
//! # Why the loop is not shared between the two
//!
//! Rule 13 lets only `dark-airlock` construct an HTTP client, and
//! `cargo xtask check-deps` fails the build if `dark-engine` names
//! `reqwest` as a dependency. [`dark_airlock::Client::get`] returns a
//! `reqwest::Response`, which this crate is free to call methods on (the
//! type flows in through `dark-airlock`'s public API) but may not name in
//! a struct field, a generic bound, or an `impl` target — any of those
//! would need `reqwest::` written out, which needs `reqwest` in this
//! crate's `Cargo.toml`. [`ByteSource`] cannot abstract over
//! `reqwest::Response`, then, so [`download_via_airlock`] reimplements
//! [`drain_to_file`]'s loop inline instead of taking a `ByteSource`. See
//! `docs/adr/0006`.

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use tokio::io::AsyncWriteExt;
use tokio::time::Instant;

use dark_contract::{Chunk, ErrCode, Error, Result};

/// A source of bytes with a known (or unknown) total size.
///
/// [`drain_to_file`] is generic over this trait so a test can supply an
/// artificially slow fake, rather than a real network fetch, to prove the
/// progress-reporting loop meets task unit `B2`'s "2 Hz or faster"
/// requirement.
#[async_trait]
pub trait ByteSource: Send {
    /// The total size, when known ahead of the transfer.
    fn total_bytes(&self) -> Option<u64>;

    /// Returns the next chunk, or `None` once the source is exhausted.
    ///
    /// # Errors
    ///
    /// Returns an error when reading the next chunk fails.
    async fn next_chunk(&mut self) -> Result<Option<Bytes>>;
}

/// What a completed download produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadOutcome {
    /// The total bytes written to disk.
    pub bytes_written: u64,
}

/// The gap between progress emissions while data is flowing. 500 ms is 2 Hz;
/// this crate emits at least that often, per task unit `B2`.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);

/// Computes a progress fraction in `[0.0, 1.0]` from bytes written against
/// a known or unknown total.
#[allow(clippy::cast_precision_loss)]
fn progress_fraction(written: u64, total: Option<u64>) -> f32 {
    match total {
        Some(0) | None => 0.0,
        Some(total) => (written as f32 / total as f32).min(1.0),
    }
}

/// Drains `source` into a file at `dest`, calling `on_chunk` with
/// [`Chunk::ModelLoading`] at the start, at least every
/// [`PROGRESS_INTERVAL`] while bytes arrive, and once more at completion.
///
/// # Errors
///
/// Returns an error when `source` fails, or when writing `dest` fails.
pub async fn drain_to_file<S: ByteSource>(
    mut source: S,
    dest: &Path,
    model_label: &str,
    mut on_chunk: impl FnMut(Chunk),
) -> Result<DownloadOutcome> {
    let total = source.total_bytes();
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|source| io_error(dest, &source))?;

    on_chunk(Chunk::ModelLoading {
        model: model_label.to_owned(),
        progress: 0.0,
    });

    let mut written: u64 = 0;
    let mut last_emit = Instant::now();
    while let Some(chunk) = source.next_chunk().await? {
        file.write_all(&chunk)
            .await
            .map_err(|source| io_error(dest, &source))?;
        written += chunk.len() as u64;

        let now = Instant::now();
        if now.duration_since(last_emit) >= PROGRESS_INTERVAL {
            on_chunk(Chunk::ModelLoading {
                model: model_label.to_owned(),
                progress: progress_fraction(written, total),
            });
            last_emit = now;
        }
    }

    file.flush()
        .await
        .map_err(|source| io_error(dest, &source))?;
    on_chunk(Chunk::ModelLoading {
        model: model_label.to_owned(),
        progress: 1.0,
    });
    Ok(DownloadOutcome {
        bytes_written: written,
    })
}

/// Downloads `url` through `client` to `dest`, reporting progress the same
/// way [`drain_to_file`] does.
///
/// Dark mode blocks this at `client.get`, before any byte moves (Rule 13,
/// `dark_contract::ErrCode::PolicyDark`).
///
/// # Errors
///
/// Returns `E_POLICY_DARK` when dark mode blocks `url`. Returns
/// `E_ENGINE_LOAD` when the request fails, the server reports a non-success
/// status, or writing `dest` fails.
pub async fn download_via_airlock(
    client: &dark_airlock::Client,
    url: &str,
    dest: &Path,
    model_label: &str,
    mut on_chunk: impl FnMut(Chunk),
) -> Result<DownloadOutcome> {
    let mut response = client.get(url).await?;
    if !response.status().is_success() {
        return Err(Error::new(
            ErrCode::EngineLoad,
            format!("GET {url} returned {}", response.status()),
        ));
    }
    let total = response.content_length();
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|source| io_error(dest, &source))?;

    on_chunk(Chunk::ModelLoading {
        model: model_label.to_owned(),
        progress: 0.0,
    });

    let mut written: u64 = 0;
    let mut last_emit = Instant::now();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|source| Error::new(ErrCode::EngineLoad, format!("GET {url} failed: {source}")))?
    {
        file.write_all(&chunk)
            .await
            .map_err(|source| io_error(dest, &source))?;
        written += chunk.len() as u64;

        let now = Instant::now();
        if now.duration_since(last_emit) >= PROGRESS_INTERVAL {
            on_chunk(Chunk::ModelLoading {
                model: model_label.to_owned(),
                progress: progress_fraction(written, total),
            });
            last_emit = now;
        }
    }

    file.flush()
        .await
        .map_err(|source| io_error(dest, &source))?;
    on_chunk(Chunk::ModelLoading {
        model: model_label.to_owned(),
        progress: 1.0,
    });
    Ok(DownloadOutcome {
        bytes_written: written,
    })
}

/// Wraps a filesystem error with the path it happened on.
fn io_error(path: &Path, source: &std::io::Error) -> Error {
    Error::new(ErrCode::EngineLoad, format!("{}: {source}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration as StdDuration;
    use tempfile::TempDir;

    /// A fake source that yields fixed-size chunks with an artificial delay
    /// between each, so a test can prove the progress gate fires at least
    /// every [`PROGRESS_INTERVAL`] without waiting on it in real time.
    struct SlowFakeSource {
        chunk_size: usize,
        chunks_left: usize,
        delay_per_chunk: StdDuration,
        total: u64,
    }

    #[async_trait]
    impl ByteSource for SlowFakeSource {
        fn total_bytes(&self) -> Option<u64> {
            Some(self.total)
        }

        async fn next_chunk(&mut self) -> Result<Option<Bytes>> {
            if self.chunks_left == 0 {
                return Ok(None);
            }
            tokio::time::sleep(self.delay_per_chunk).await;
            self.chunks_left -= 1;
            Ok(Some(Bytes::from(vec![0u8; self.chunk_size])))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn progress_arrives_at_two_hertz_or_faster_while_data_flows() {
        // 20 chunks, 300 ms apart: 6 seconds of virtual time, so a 2 Hz
        // gate (every 500 ms) must fire at least 12 times, not counting the
        // guaranteed start-and-end emissions.
        let source = SlowFakeSource {
            chunk_size: 1024,
            chunks_left: 20,
            delay_per_chunk: StdDuration::from_millis(300),
            total: 20 * 1024,
        };
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("model.bin");

        let mut events = Vec::new();
        let outcome = drain_to_file(source, &dest, "test-model", |chunk| events.push(chunk))
            .await
            .unwrap();

        assert_eq!(outcome.bytes_written, 20 * 1024);
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), 20 * 1024);

        let progress_events: Vec<f32> = events
            .iter()
            .filter_map(|chunk| match chunk {
                Chunk::ModelLoading { progress, .. } => Some(*progress),
                _ => None,
            })
            .collect();

        // The virtual clock advances only while a task is actually
        // awaiting a timer, so 20 sleeps of 300 ms land at 300, 600, ...,
        // 6000 ms. A 500 ms gate checked after each arrival fires at 600,
        // 1200, ..., 6000 ms: 10 times. Add the guaranteed 0.0 emission
        // before the first chunk and the guaranteed 1.0 emission after the
        // last, and the total is 12 — never slower than 2 Hz across the
        // whole 6-second transfer.
        assert_eq!(
            progress_events.len(),
            12,
            "expected exactly 12 progress events (10 mid-stream + start + end)"
        );
        assert_eq!(progress_events.first(), Some(&0.0));
        assert_eq!(progress_events.last(), Some(&1.0));
        // Progress never goes backwards.
        assert!(progress_events.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[tokio::test]
    async fn a_fast_source_still_reports_the_final_progress() {
        let source = SlowFakeSource {
            chunk_size: 64,
            chunks_left: 3,
            delay_per_chunk: StdDuration::ZERO,
            total: 3 * 64,
        };
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("model.bin");

        let mut events = Vec::new();
        drain_to_file(source, &dest, "test-model", |chunk| events.push(chunk))
            .await
            .unwrap();

        let last = events.last().unwrap();
        assert!(matches!(
            last,
            Chunk::ModelLoading { progress, .. } if (*progress - 1.0).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn progress_fraction_never_exceeds_one() {
        assert!((progress_fraction(150, Some(100)) - 1.0).abs() < f32::EPSILON);
    }

    /// Binds a loopback listener and answers exactly one request with a
    /// real HTTP response carrying `body`, then stops. Mirrors
    /// `dark_airlock::client`'s own test helper, at the byte level: no
    /// mocking library, a genuine socket and a genuine (if minimal) HTTP
    /// response.
    fn serve_one_ok_response(listener: tokio::net::TcpListener, body: Vec<u8>) -> u16 {
        let port = listener.local_addr().expect("local_addr").port();
        tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let mut header = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            )
            .into_bytes();
            header.extend_from_slice(&body);
            let mut written = 0usize;
            while written < header.len() {
                if stream.writable().await.is_err() {
                    return;
                }
                match stream.try_write(&header[written..]) {
                    Ok(n) => written += n,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => return,
                }
            }
        });
        port
    }

    #[tokio::test]
    async fn download_via_airlock_fetches_a_real_response_over_loopback() {
        let body = b"the quick brown fox jumps over the lazy dog".to_vec();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = serve_one_ok_response(listener, body.clone());

        let client = dark_airlock::Client::new(false);
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("fetched.bin");
        let mut events = Vec::new();

        let outcome = download_via_airlock(
            &client,
            &format!("http://127.0.0.1:{port}/model.bin"),
            &dest,
            "loopback-model",
            |chunk| events.push(chunk),
        )
        .await
        .unwrap();

        assert_eq!(outcome.bytes_written, body.len() as u64);
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(events.iter().any(|chunk| matches!(
            chunk,
            Chunk::ModelLoading { progress, .. } if (*progress - 1.0).abs() < f32::EPSILON
        )));
    }

    #[tokio::test]
    async fn download_via_airlock_blocks_a_non_loopback_url_in_dark_mode() {
        let client = dark_airlock::Client::new(true);
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("blocked.bin");
        let err = download_via_airlock(
            &client,
            "https://huggingface.co/does/not/matter",
            &dest,
            "blocked-model",
            |_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, ErrCode::PolicyDark);
        assert!(!dest.exists(), "dark mode must block before any byte moves");
    }

    #[test]
    fn progress_fraction_is_zero_for_an_unknown_total() {
        assert!(progress_fraction(50, None).abs() < f32::EPSILON);
    }
}
