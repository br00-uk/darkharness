//! Loopback classification for dark mode.
//!
//! This module answers one question: does a host name loopback? It answers
//! the question by inspecting the syntax of the host only. It never opens a
//! socket and never calls a resolver. A caller that checks a host with this
//! module before it resolves that host gets the ordering that Rule 13 of the
//! product requirement demands: the refusal happens before the lookup that
//! would otherwise leak the hostname to a DNS server.

use dark_contract::{ErrCode, Error, Result};
use url::Host;

/// Returns `true` when `host` is a loopback address or the name `localhost`.
///
/// This check is a pure function over the parsed host. It performs no I/O,
/// so calling it can never itself cause a name lookup.
///
/// A domain name is loopback only when it equals `localhost`, compared
/// without case. A domain that merely starts with a loopback-looking prefix,
/// for example `127.0.0.1.evil.com`, does not match: it is a `Host::Domain`,
/// not a `Host::Ipv4`, and its text is not `localhost`.
pub(crate) fn is_loopback<S: AsRef<str>>(host: &Host<S>) -> bool {
    match host {
        Host::Domain(name) => name.as_ref().eq_ignore_ascii_case("localhost"),
        Host::Ipv4(addr) => addr.is_loopback(),
        Host::Ipv6(addr) => {
            addr.is_loopback() || addr.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

/// Checks `host` against dark mode.
///
/// Returns `Ok(())` when `dark` is `false`, or when `host` is loopback.
/// Returns `E_POLICY_DARK` otherwise.
///
/// # Errors
///
/// Returns `E_POLICY_DARK` when `dark` is `true` and `host` is not loopback.
pub(crate) fn check_host(dark: bool, host: &Host<&str>) -> Result<()> {
    if !dark || is_loopback(host) {
        return Ok(());
    }
    Err(Error::new(
        ErrCode::PolicyDark,
        format!("dark mode blocks '{host}' because it is not a loopback address"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn domain(name: &str) -> Host<&str> {
        Host::Domain(name)
    }

    #[test]
    fn localhost_is_loopback_case_insensitive() {
        assert!(is_loopback(&domain("localhost")));
        assert!(is_loopback(&domain("LOCALHOST")));
        assert!(is_loopback(&domain("LocalHost")));
    }

    #[test]
    fn a_domain_that_merely_looks_like_loopback_is_not() {
        // The requirement calls this out explicitly: a prefix match is not
        // enough. The host must equal `localhost` exactly.
        assert!(!is_loopback(&domain("127.0.0.1.evil.com")));
        assert!(!is_loopback(&domain("localhost.evil.com")));
        assert!(!is_loopback(&domain("notlocalhost")));
    }

    #[test]
    fn an_ordinary_domain_is_not_loopback() {
        assert!(!is_loopback(&domain("example.com")));
    }

    #[test]
    fn the_full_ipv4_loopback_block_is_loopback() {
        let host: Host<&str> = Host::Ipv4(Ipv4Addr::LOCALHOST);
        assert!(is_loopback(&host));
        let host: Host<&str> = Host::Ipv4(Ipv4Addr::new(127, 255, 0, 9));
        assert!(is_loopback(&host));
    }

    #[test]
    fn a_private_ipv4_address_is_not_loopback() {
        let host: Host<&str> = Host::Ipv4(Ipv4Addr::new(10, 0, 0, 1));
        assert!(!is_loopback(&host));
        let host: Host<&str> = Host::Ipv4(Ipv4Addr::new(192, 168, 1, 1));
        assert!(!is_loopback(&host));
    }

    #[test]
    fn ipv6_loopback_is_loopback() {
        let host: Host<&str> = Host::Ipv6(Ipv6Addr::LOCALHOST);
        assert!(is_loopback(&host));
    }

    #[test]
    fn an_ipv4_mapped_ipv6_loopback_is_loopback() {
        let mapped = Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 0x0001);
        let host: Host<&str> = Host::Ipv6(mapped);
        assert!(is_loopback(&host));
    }

    #[test]
    fn an_ordinary_ipv6_address_is_not_loopback() {
        let host: Host<&str> = Host::Ipv6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1));
        assert!(!is_loopback(&host));
    }

    #[test]
    fn light_mode_allows_every_host() {
        assert!(check_host(false, &domain("example.com")).is_ok());
    }

    #[test]
    fn dark_mode_allows_loopback() {
        assert!(check_host(true, &domain("localhost")).is_ok());
        let host: Host<&str> = Host::Ipv4(Ipv4Addr::LOCALHOST);
        assert!(check_host(true, &host).is_ok());
    }

    #[test]
    fn dark_mode_rejects_a_non_loopback_domain_with_policy_dark() {
        let err = check_host(true, &domain("example.com")).unwrap_err();
        assert_eq!(err.code, ErrCode::PolicyDark);
        assert_eq!(err.code.as_str(), "E_POLICY_DARK");
    }
}
