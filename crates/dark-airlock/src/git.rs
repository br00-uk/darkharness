//! Dark-mode enforcement for git remotes.
//!
//! `dark-airlock` does not run git. [`check_git_remote`] is a pure check
//! that another crate calls before it spawns a git subprocess that would
//! reach a remote. It classifies the remote's URL and, in dark mode,
//! refuses anything that is not local or loopback.
//!
//! A git remote can be an `https://` or `ssh://` URL, the scp-like
//! `user@host:path` shorthand, or a plain filesystem path. This module
//! recognises all four. It does not recognise a git remote helper
//! (`ext::…`, or a third-party `git-remote-<name>`): those can open a
//! connection of their own choosing, and no URL check catches that. The
//! `DARK_OFFLINE` signal in [`crate::child`] is the only guard that reaches
//! that case, and it is advisory. See the module docs on `child` for what
//! that does and does not guarantee.
//!
//! When this module cannot prove a remote is local, it refuses the remote.
//! A guard that fails open on an unrecognised form is not a guard.

use dark_contract::{ErrCode, Error, Result};
use url::{Host, Url};

use crate::guard::is_loopback;

/// Checks whether a git operation against `remote_name` may proceed.
///
/// Call this before spawning a git subprocess that would reach
/// `remote_url`, for example `git fetch <remote_name>` or `git push
/// <remote_name>`. In light mode, or when `remote_url` names a local path
/// or a loopback host, this returns `Ok(())`. Otherwise it refuses, and
/// names `remote_name` in the message.
///
/// # Errors
///
/// Returns `E_POLICY_DARK` when `dark` is `true` and `remote_url` reaches a
/// host that is not loopback, or cannot be shown to be local.
pub fn check_git_remote(dark: bool, remote_name: &str, remote_url: &str) -> Result<()> {
    if !dark {
        return Ok(());
    }
    match classify(remote_url) {
        Target::Local => Ok(()),
        Target::Host(host) if is_loopback(&host) => Ok(()),
        Target::Host(host) => Err(blocked(
            remote_name,
            remote_url,
            &format!("it reaches '{host}', which is not loopback"),
        )),
        Target::Unclassifiable => Err(blocked(
            remote_name,
            remote_url,
            "dark mode cannot show that it is local",
        )),
    }
}

/// Builds the `E_POLICY_DARK` error, naming both the remote and its URL.
fn blocked(remote_name: &str, remote_url: &str, reason: &str) -> Error {
    Error::new(
        ErrCode::PolicyDark,
        format!("dark mode blocks git remote '{remote_name}' ({remote_url}): {reason}"),
    )
}

/// What a remote URL reaches.
enum Target {
    /// A filesystem path. No network egress reaches this remote.
    Local,
    /// A network host, resolved syntactically and never looked up.
    Host(Host<String>),
    /// Neither of the above: this module cannot prove the remote is local.
    Unclassifiable,
}

/// Classifies a git remote URL.
///
/// Tries, in order: a URL with an explicit scheme (`https://`, `ssh://`,
/// `git://`, `file://`, …); git's scp-like shorthand (`user@host:path` or
/// `host:path`); and finally a plain filesystem path, which is the default
/// when neither of the first two forms matches.
fn classify(remote_url: &str) -> Target {
    if let Ok(url) = Url::parse(remote_url) {
        return match url.host() {
            Some(host) => Target::Host(normalise(&host)),
            // No host: a `file://` URL, or a scheme this module does not
            // know reaches a network. Treat it as local.
            None => Target::Local,
        };
    }

    if let Some(host_str) = scp_like_host(remote_url) {
        return match Host::parse(host_str) {
            Ok(host) => Target::Host(host),
            Err(_) => Target::Unclassifiable,
        };
    }

    Target::Local
}

/// Promotes a host written as a literal IP address to its typed form.
///
/// The `url` crate parses a host as an IP address only for a special
/// scheme: `http`, `https`, `ws`, `wss`, `ftp`, and `file`. A git remote is
/// most often `ssh://`, which is not special, so `ssh://127.0.0.1/repo.git`
/// arrives as `Host::Domain("127.0.0.1")` and would fail a loopback check
/// that expects `Host::Ipv4`. Re-parsing the text restores the address
/// form.
///
/// This cannot widen what counts as loopback. A name that merely looks like
/// an address, for example `127.0.0.1.evil.com`, is not a valid address, so
/// it stays a domain and still fails the check.
fn normalise(host: &Host<&str>) -> Host<String> {
    match host {
        Host::Domain(name) => {
            Host::parse(name).unwrap_or_else(|_| Host::Domain((*name).to_owned()))
        }
        Host::Ipv4(addr) => Host::Ipv4(*addr),
        Host::Ipv6(addr) => Host::Ipv6(*addr),
    }
}

/// Extracts the host from git's scp-like syntax, `[user@]host:path`.
///
/// Returns `None` when `remote` is not scp-like syntax: when it has no
/// colon, when a `/` appears before the first colon (an ordinary path that
/// happens to contain a colon later on), or when the text before the colon
/// is a single character (a Windows drive letter, for example `C:\repo`).
fn scp_like_host(remote: &str) -> Option<&str> {
    let colon = remote.find(':')?;
    if let Some(slash) = remote.find('/') {
        if slash < colon {
            return None;
        }
    }
    let before_colon = &remote[..colon];
    let host_part = before_colon.rsplit('@').next().unwrap_or(before_colon);
    if host_part.len() <= 1 {
        return None;
    }
    Some(host_part)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_mode_allows_any_remote() {
        assert!(check_git_remote(false, "origin", "https://github.com/org/repo.git").is_ok());
    }

    #[test]
    fn dark_mode_blocks_a_remote_url_and_names_the_remote() {
        let err = check_git_remote(true, "origin", "https://github.com/org/repo.git").unwrap_err();
        assert_eq!(err.code, ErrCode::PolicyDark);
        assert!(
            err.message.contains("origin"),
            "message must name the remote: {}",
            err.message
        );
        assert!(
            err.message.contains("github.com"),
            "message must name the host: {}",
            err.message
        );
    }

    #[test]
    fn dark_mode_blocks_scp_like_syntax_and_names_the_remote() {
        let err = check_git_remote(true, "upstream", "git@github.com:org/repo.git").unwrap_err();
        assert_eq!(err.code, ErrCode::PolicyDark);
        assert!(err.message.contains("upstream"));
        assert!(err.message.contains("github.com"));
    }

    #[test]
    fn dark_mode_allows_an_ssh_loopback_remote() {
        assert!(check_git_remote(true, "origin", "ssh://127.0.0.1:2222/repo.git").is_ok());
    }

    #[test]
    fn a_host_that_only_looks_like_loopback_is_still_blocked() {
        // Promoting a textual address to a typed one must not widen what
        // counts as loopback. These are domains, not addresses.
        for remote in [
            "ssh://127.0.0.1.evil.com/repo.git",
            "https://127.0.0.1.evil.com/repo.git",
            "ssh://localhost.evil.com/repo.git",
        ] {
            let err = check_git_remote(true, "origin", remote)
                .expect_err("a lookalike host must be blocked");
            assert_eq!(err.code, ErrCode::PolicyDark);
            assert!(
                err.message.contains("origin"),
                "the remote must be named: {}",
                err.message
            );
        }
    }

    #[test]
    fn dark_mode_allows_an_http_loopback_remote() {
        assert!(check_git_remote(true, "origin", "http://localhost:9418/repo.git").is_ok());
    }

    #[test]
    fn dark_mode_allows_an_absolute_local_path() {
        assert!(check_git_remote(true, "origin", "/home/user/other-repo").is_ok());
    }

    #[test]
    fn dark_mode_allows_a_relative_local_path() {
        assert!(check_git_remote(true, "origin", "../sibling-repo").is_ok());
        assert!(check_git_remote(true, "origin", "./sibling-repo").is_ok());
    }

    #[test]
    fn dark_mode_allows_a_file_url() {
        assert!(check_git_remote(true, "origin", "file:///home/user/other-repo").is_ok());
    }

    #[test]
    fn dark_mode_does_not_mistake_a_windows_drive_for_a_host() {
        assert!(check_git_remote(true, "origin", "C:\\repos\\thing").is_ok());
    }

    #[test]
    fn dark_mode_fails_closed_on_an_unclassifiable_remote() {
        // Scp-like shape (a colon before any slash), but the text before the
        // colon is not a legal host: this must refuse, not guess "local".
        let err = check_git_remote(true, "origin", "user@bad host:path/to/repo").unwrap_err();
        assert_eq!(err.code, ErrCode::PolicyDark);
        assert!(err.message.contains("origin"));
    }

    #[test]
    fn scp_like_host_parses_the_common_forms() {
        assert_eq!(
            scp_like_host("git@github.com:org/repo.git"),
            Some("github.com")
        );
        assert_eq!(scp_like_host("github.com:org/repo.git"), Some("github.com"));
        assert_eq!(scp_like_host("../relative:with-colon/path"), None);
        assert_eq!(scp_like_host("C:\\repo"), None);
        assert_eq!(scp_like_host("plain-relative-path"), None);
    }
}
