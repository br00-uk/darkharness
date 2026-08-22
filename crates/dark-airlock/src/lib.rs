//! The airlock: the only crate that may construct an HTTP client.
//!
//! The primary requirement of darkharness is that a user disconnects the
//! network after `dark setup` and keeps working. This crate is the
//! component that makes that promise true rather than aspirational.
//!
//! Rule 13 restricts the whole workspace to one HTTP client: this one.
//! `cargo xtask check-deps` fails the build if another crate depends on
//! `reqwest`, `hyper`, or `ureq` directly, and `cargo deny check bans`
//! catches one that arrives transitively. Nothing here exports a raw
//! [`reqwest::Client`] or another way to construct one; [`Client`] is the
//! only door.
//!
//! # Dark mode blocks before it looks anything up
//!
//! In dark mode, [`Client`] refuses a request whose host is not loopback,
//! and it refuses before any resolution of that host. A DNS query is
//! itself an act of network egress: it hands a hostname to a server outside
//! the machine, whether or not the connection that follows ever succeeds.
//! Checking the host *after* resolving it would already have leaked the
//! hostname, so the check in this crate runs against the syntax of the
//! parsed URL only — a loopback IP literal (`127.0.0.0/8` or `::1`) or the
//! name `localhost` — and never calls a resolver for a request it goes on
//! to refuse. See [`Client::get`] for where the check runs.
//!
//! A hostname that merely looks like a loopback address does not pass:
//! `127.0.0.1.evil.com` is a domain name, not the IPv4 literal `127.0.0.1`,
//! and it is not the literal string `localhost`, so dark mode blocks it.
//!
//! # What else this crate guards
//!
//! - [`check_git_remote`] refuses a git operation that would reach a
//!   non-loopback remote in dark mode, and names the remote in the error.
//! - [`child_env`] and [`apply_to_command`] give a process-spawning tool
//!   (task unit `C3`) the environment variable to set on a child process in
//!   dark mode. Read their docs before relying on them: the block they
//!   describe is advisory on every platform, not just macOS and Windows.
//!
//! # Errors
//!
//! Every refusal in this crate returns [`dark_contract::Error`] with
//! [`dark_contract::ErrCode::PolicyDark`], which carries the documented
//! remedy: run `/golight` to allow the network.

mod child;
mod client;
mod git;
mod guard;

pub use child::{DARK_OFFLINE_ENV, apply_to_command, child_env};
pub use client::Client;
pub use git::check_git_remote;
