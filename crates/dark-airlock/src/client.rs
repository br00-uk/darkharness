//! The single HTTP client for the darkharness workspace.
//!
//! [`Client`] is the only way any crate constructs an HTTP client. Rule 13
//! forbids every other crate from depending on `reqwest`, `hyper`, or
//! `ureq` directly, and `cargo xtask check-deps` plus `cargo deny` both
//! enforce that at build time. This module is the one seam where a socket
//! can open.

use dark_contract::{ErrCode, Error, Result};
use url::Url;

use crate::guard;

/// The workspace's HTTP client.
///
/// A client built with `dark: true` refuses every request whose host is not
/// loopback. It refuses at the parsed URL, before anything asks the host to
/// resolve: [`Client::get`] parses the URL and classifies the host
/// syntactically first, and only hands the request to the transport after
/// that check passes. A blocked request never reaches a resolver, so it
/// never leaks a hostname to a DNS server.
///
/// A client built with `dark: false` applies no restriction. Construct one
/// per process, from the setting that `dark setup` and `/golight` control;
/// do not build a second client to route around dark mode.
#[derive(Debug, Clone)]
pub struct Client {
    dark: bool,
    inner: reqwest::Client,
}

impl Client {
    /// Creates a client.
    ///
    /// Set `dark` to `true` to enforce dark mode: every request must target
    /// a loopback address. Set it to `false` to allow every request.
    #[must_use]
    pub fn new(dark: bool) -> Self {
        Self {
            dark,
            inner: reqwest::Client::new(),
        }
    }

    /// Returns `true` when this client enforces dark mode.
    #[must_use]
    pub fn is_dark(&self) -> bool {
        self.dark
    }

    /// Sends an HTTP GET request to `url`.
    ///
    /// In dark mode, this method refuses a `url` whose host is not
    /// loopback, before it does anything that could resolve that host.
    ///
    /// # Errors
    ///
    /// Returns `E_POLICY_DARK` when dark mode blocks `url`. Returns
    /// `E_TOOL_FAILED` when `url` does not parse, or when the request fails
    /// after this client allows it through.
    pub async fn get(&self, url: &str) -> Result<reqwest::Response> {
        let parsed = parse_url(url)?;
        self.check(&parsed)?;
        self.inner.get(parsed).send().await.map_err(|source| {
            Error::new(ErrCode::ToolFailed, format!("GET {url} failed: {source}"))
        })
    }

    /// Checks `url` against dark mode without sending a request.
    ///
    /// A caller that wants to fail early, for example before it builds a
    /// request body or writes a log line naming the request, can call this
    /// first.
    ///
    /// # Errors
    ///
    /// Returns `E_POLICY_DARK` when dark mode blocks `url`. Returns
    /// `E_TOOL_FAILED` when `url` does not parse.
    pub fn check_url(&self, url: &str) -> Result<()> {
        let parsed = parse_url(url)?;
        self.check(&parsed)
    }

    /// Runs the dark-mode host check against an already-parsed URL.
    ///
    /// This is the one place the guard runs. It never resolves `url`: it
    /// reads the host straight out of the parsed structure.
    fn check(&self, url: &Url) -> Result<()> {
        if !self.dark {
            return Ok(());
        }
        match url.host() {
            Some(host) => guard::check_host(true, &host),
            None => Err(Error::new(
                ErrCode::PolicyDark,
                format!("dark mode blocks '{url}' because it names no host"),
            )),
        }
    }
}

/// Parses `url`, reporting a bad URL as `E_TOOL_FAILED`.
///
/// A parse failure is an input problem, not a policy decision, so it keeps
/// its own code even when the caller is a dark-mode client.
fn parse_url(url: &str) -> Result<Url> {
    Url::parse(url).map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("'{url}' is not a valid URL: {source}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::net::TcpListener;

    /// Binds a loopback listener, answers exactly one connection with a
    /// minimal valid HTTP response, then stops. Returns the port.
    ///
    /// This uses `TcpStream::try_write`, not `AsyncWriteExt`, because this
    /// crate enables tokio's `net` feature and not `io-util`.
    fn serve_one_ok_response(listener: TcpListener) -> u16 {
        let port = listener.local_addr().expect("local_addr").port();
        tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let response = b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
            let mut written = 0usize;
            while written < response.len() {
                if stream.writable().await.is_err() {
                    return;
                }
                match stream.try_write(&response[written..]) {
                    Ok(n) => written += n,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => return,
                }
            }
        });
        port
    }

    #[tokio::test]
    async fn dark_mode_rejects_a_non_loopback_url() {
        let client = Client::new(true);
        let err = client.get("https://example.com").await.unwrap_err();
        assert_eq!(err.code, ErrCode::PolicyDark);
    }

    #[tokio::test]
    async fn dark_mode_rejects_a_domain_that_only_looks_like_loopback() {
        let client = Client::new(true);
        let err = client.get("http://127.0.0.1.evil.com/").await.unwrap_err();
        assert_eq!(err.code, ErrCode::PolicyDark);
    }

    #[tokio::test]
    async fn dark_mode_rejects_before_any_name_lookup() {
        // `.invalid` is reserved by RFC 2606: it never resolves. Bounding the
        // call with a short timeout proves the rejection did not wait on a
        // resolver — a resolver attempt in a network-denied environment
        // would either error slowly or hang, not return instantly.
        let client = Client::new(true);
        let outcome = tokio::time::timeout(
            Duration::from_secs(2),
            client.get("http://this-host-does-not-exist.invalid/"),
        )
        .await
        .expect("the guard must reject before attempting resolution, not time out");
        assert_eq!(outcome.unwrap_err().code, ErrCode::PolicyDark);
    }

    #[tokio::test]
    async fn dark_mode_allows_an_ipv4_loopback_literal() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = serve_one_ok_response(listener);
        let client = Client::new(true);
        let response = client
            .get(&format!("http://127.0.0.1:{port}/"))
            .await
            .expect("loopback must be allowed in dark mode");
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn dark_mode_allows_localhost_by_name() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = serve_one_ok_response(listener);
        let client = Client::new(true);
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            client.get(&format!("http://localhost:{port}/")),
        )
        .await
        .expect("resolving localhost must not hang")
        .expect("localhost must be allowed in dark mode");
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn dark_mode_allows_the_ipv6_loopback_literal() {
        let Ok(listener) = TcpListener::bind("[::1]:0").await else {
            eprintln!("skipping: this environment has no IPv6 loopback");
            return;
        };
        let port = serve_one_ok_response(listener);
        let client = Client::new(true);
        let response = client
            .get(&format!("http://[::1]:{port}/"))
            .await
            .expect("[::1] must be allowed in dark mode");
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn light_mode_allows_a_loopback_request_normally() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = serve_one_ok_response(listener);
        let client = Client::new(false);
        let response = client
            .get(&format!("http://127.0.0.1:{port}/"))
            .await
            .expect("light mode must not add its own restriction");
        assert_eq!(response.status(), 200);
    }

    #[test]
    fn light_mode_does_not_reject_a_non_loopback_url() {
        // No real network call: `check_url` runs only the guard, never
        // `reqwest::Client::send`.
        let client = Client::new(false);
        assert!(client.check_url("https://example.com").is_ok());
        assert!(client.check_url("https://198.51.100.7/").is_ok());
    }

    #[tokio::test]
    async fn an_unparsable_url_fails_with_tool_failed_not_policy_dark() {
        let client = Client::new(true);
        let err = client.get("not a url").await.unwrap_err();
        assert_eq!(err.code, ErrCode::ToolFailed);
    }
}
