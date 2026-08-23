//! Fetching over the network, without `dark-lexicon` ever building a socket.
//!
//! ## Rule 13 against Rule 16
//!
//! Task unit `G2` says the `sitemap` and `llms-txt` adapters "fetch through
//! `dark-airlock`". But Rule 13 says only `dark-airlock` may construct an
//! HTTP client, and Rule 16 says `dark-lexicon` depends on `dark-contract`
//! and its own storage crates only — it may not add `dark-airlock` as a
//! dependency to reach that client. `cargo xtask check-deps` and
//! `cargo deny` both enforce this at build time, so this is not a stylistic
//! preference to work around; a `dark-lexicon` that depended on
//! `dark-airlock` would not build.
//!
//! [`Fetcher`] is the seam that resolves the conflict. `dark-lexicon`
//! defines a minimal trait for "fetch these bytes from this URL" and never
//! implements it itself. A caller that already depends on `dark-airlock`
//! — `dark-core` or `dark-cli` — implements `Fetcher` over
//! `dark_airlock::Client` and passes the implementation in. `dark-lexicon`
//! then depends only on the trait, which lives in this module and needs no
//! HTTP library: no `reqwest`, no `hyper`, no `ureq`, nothing that
//! `cargo deny` would catch.
//!
//! The trait is synchronous by design. `dark_airlock::Client::get` is
//! `async`, but making [`Fetcher::fetch`] `async` here would need either
//! the `async-trait` crate (not a `dark-lexicon` dependency, and this task
//! unit may not add one) or native `async fn` in a `dyn`-safe trait, which
//! is not stable for this shape. A synchronous trait pushes the async-to-
//! sync boundary onto the implementor, which already runs inside a tokio
//! runtime and can block on the async call (for example with
//! `tokio::runtime::Handle::block_on`, run from a blocking-safe context).
//! [`RateLimiter`] paces calls to a `Fetcher` in wall-clock time for the
//! same reason: it uses `std::thread::sleep`, not a tokio timer, because
//! this crate has no `tokio` dependency either.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use dark_contract::{ErrCode, Error, Result};

/// Fetches bytes from a URL.
///
/// A caller that depends on `dark-airlock` implements this trait over
/// `dark_airlock::Client`; see the module docs for why `dark-lexicon`
/// cannot do that itself. A test implements it over a fixture.
pub trait Fetcher: Send + Sync {
    /// Fetches `url` and returns the response body.
    ///
    /// # Errors
    ///
    /// Returns `E_TOOL_FAILED` when the request fails, times out, or the
    /// response exceeds the caller's size cap. The airlock's own dark-mode
    /// and host-allow checks, when the implementation wraps
    /// `dark_airlock::Client`, surface here as the same code: this trait
    /// carries no error taxonomy of its own.
    fn fetch(&self, url: &str) -> Result<Vec<u8>>;
}

/// The maximum response size that [`fetch_capped`] accepts.
///
/// G2 asks adapters to "apply size caps" to fetched HTML. Ten mebibytes
/// comfortably holds a documentation page; a response larger than that is
/// not a documentation page the harness should hold in memory.
pub const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

/// Fetches `url` through `fetcher`, refusing a response over
/// [`MAX_RESPONSE_BYTES`].
///
/// # Errors
///
/// Returns whatever `fetcher.fetch` returns. Returns `E_TOOL_FAILED` when
/// the response exceeds the size cap.
pub fn fetch_capped(fetcher: &dyn Fetcher, url: &str) -> Result<Vec<u8>> {
    let bytes = fetcher.fetch(url)?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(Error::new(
            ErrCode::ToolFailed,
            format!(
                "{url} returned {} bytes, over the {MAX_RESPONSE_BYTES}-byte cap",
                bytes.len()
            ),
        ));
    }
    Ok(bytes)
}

/// Returns the host component of `url`, lowercased, for grouping rate
/// limits and robots.txt policies per host.
///
/// This does a minimal parse — split on `://`, then on the first `/`,
/// `?`, or `#` — rather than pulling in a URL-parsing dependency that
/// Rule 16 would not allow.
///
/// # Errors
///
/// Returns `E_TOOL_FAILED` when `url` has no `scheme://host` shape.
pub fn host_of(url: &str) -> Result<String> {
    let (_scheme, after_scheme) = url
        .split_once("://")
        .ok_or_else(|| Error::new(ErrCode::ToolFailed, format!("'{url}' names no scheme")))?;
    let host_and_port = after_scheme
        .split(['/', '?', '#'])
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::new(ErrCode::ToolFailed, format!("'{url}' names no host")))?;
    // Strip userinfo (`user:pass@host`), keep host:port as-is.
    let host = host_and_port
        .rsplit_once('@')
        .map_or(host_and_port, |(_, h)| h);
    Ok(host.to_ascii_lowercase())
}

/// Paces requests to at most `max_per_second` for any one host.
///
/// G2 sets this at 2 requests each second for one host. The limiter tracks
/// the last request time per host and sleeps out the remainder of the
/// interval before the next request to that host, so a caller that always
/// calls [`Self::wait`] before fetching never exceeds the rate.
pub struct RateLimiter {
    min_interval: Duration,
    last_request: HashMap<String, Instant>,
}

impl RateLimiter {
    /// Creates a limiter that allows `max_per_second` requests to any one
    /// host.
    ///
    /// # Panics
    ///
    /// Panics when `max_per_second` is zero; a zero rate has no interval to
    /// compute.
    #[must_use]
    pub fn new(max_per_second: u32) -> Self {
        assert!(max_per_second > 0, "max_per_second must be positive");
        Self {
            min_interval: Duration::from_secs_f64(1.0 / f64::from(max_per_second)),
            last_request: HashMap::new(),
        }
    }

    /// Creates the limiter that G2 requires: 2 requests each second for one
    /// host.
    #[must_use]
    pub fn per_task_unit_g2() -> Self {
        Self::new(2)
    }

    /// Blocks until a request to `host` is allowed, then records the
    /// request time.
    pub fn wait(&mut self, host: &str) {
        let now = Instant::now();
        if let Some(&last) = self.last_request.get(host) {
            let elapsed = now.duration_since(last);
            if elapsed < self.min_interval {
                std::thread::sleep(self.min_interval.saturating_sub(elapsed));
            }
        }
        self.last_request.insert(host.to_owned(), Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FixedFetcher {
        body: Vec<u8>,
    }

    impl Fetcher for FixedFetcher {
        fn fetch(&self, _url: &str) -> Result<Vec<u8>> {
            Ok(self.body.clone())
        }
    }

    #[test]
    fn fetch_capped_passes_through_a_small_response() {
        let fetcher = FixedFetcher {
            body: b"hello".to_vec(),
        };
        let bytes = fetch_capped(&fetcher, "https://example.com/").unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn fetch_capped_refuses_a_response_over_the_cap() {
        let fetcher = FixedFetcher {
            body: vec![0u8; MAX_RESPONSE_BYTES + 1],
        };
        let err = fetch_capped(&fetcher, "https://example.com/").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolFailed);
    }

    #[test]
    fn host_of_extracts_the_host_from_a_url() {
        assert_eq!(host_of("https://docs.rs/tokio/1.47.0/").unwrap(), "docs.rs");
        assert_eq!(
            host_of("http://Example.COM:8080/a/b").unwrap(),
            "example.com:8080"
        );
        assert_eq!(
            host_of("https://user:pass@example.com/").unwrap(),
            "example.com"
        );
    }

    #[test]
    fn host_of_rejects_a_url_with_no_host() {
        assert!(host_of("not a url").is_err());
    }

    #[test]
    fn rate_limiter_allows_an_immediate_first_request() {
        let mut limiter = RateLimiter::per_task_unit_g2();
        let start = Instant::now();
        limiter.wait("example.com");
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn rate_limiter_spaces_two_requests_to_the_same_host() {
        let mut limiter = RateLimiter::new(2);
        let start = Instant::now();
        limiter.wait("example.com");
        limiter.wait("example.com");
        // 2 requests per second means at least 500ms between requests.
        assert!(start.elapsed() >= Duration::from_millis(450));
    }

    #[test]
    fn rate_limiter_does_not_space_requests_to_different_hosts() {
        let mut limiter = RateLimiter::new(2);
        let start = Instant::now();
        limiter.wait("a.example.com");
        limiter.wait("b.example.com");
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn fetcher_trait_is_object_safe() {
        fn assert_object_safe(_: Option<&dyn Fetcher>) {}
        assert_object_safe(None);
    }

    #[test]
    fn fixed_fetcher_can_be_shared_across_threads_via_mutex() {
        let fetcher = Mutex::new(FixedFetcher {
            body: b"x".to_vec(),
        });
        let guard = fetcher.lock().unwrap();
        assert_eq!(guard.fetch("u").unwrap(), b"x");
    }
}
