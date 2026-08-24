//! Finds the coding agents installed on this machine that speak the
//! Agent Client Protocol.
//!
//! # Why this is a table and not a configuration file
//!
//! The point of this feature is to use an agent a person has already
//! installed, with no further setup. That means the harness has to know,
//! without being told, that `opencode` on the path is launched as
//! `opencode acp` and that Gemini wants `--experimental-acp`. [`KNOWN`]
//! is that knowledge. A person can still name an agent this table does
//! not carry, or correct one it gets wrong — see [`Agent::configured`] —
//! but nothing has to be configured for the common case.
//!
//! # Two ways to launch, and why the difference matters here
//!
//! Most ACP agents publish both a native binary and an npm package. The
//! editor extensions that pioneered this protocol launch them with
//! `npx <package>@latest`, which **downloads the package on every
//! launch**. That is a reasonable default for an editor and the wrong
//! one for this harness: darkharness exists so a person can disconnect
//! the network and keep working, and an agent that cannot start without
//! a download breaks that on the first run.
//!
//! So [`find`] prefers a native binary already on the path, and treats
//! the npx form as a fallback that [`Launch::needs_network_to_start`]
//! marks. Dark mode refuses the fallback; a native binary starts either
//! way. Whether the agent then reaches the network to *think* is its own
//! business and outside this harness's control — see
//! [`Agent::reaches_network`], which records what is known rather than
//! pretending to enforce it.

use std::path::{Path, PathBuf};

/// How an agent is started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// The program to run.
    pub program: String,
    /// Its arguments, in order.
    pub args: Vec<String>,
    /// The program downloads something before it can start.
    ///
    /// True for the `npx <package>@latest` form, which fetches the
    /// package every launch. A caller in dark mode must refuse these.
    pub needs_network_to_start: bool,
}

impl Launch {
    /// A launch of a program already on this machine.
    fn native(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            needs_network_to_start: false,
        }
    }

    /// A launch through `npx`, which fetches the package first.
    fn npx(package: &str, args: &[&str]) -> Self {
        let mut all = vec![package.to_owned()];
        all.extend(args.iter().map(|arg| (*arg).to_owned()));
        Self {
            program: "npx".to_owned(),
            args: all,
            needs_network_to_start: true,
        }
    }

    /// Renders the launch as a person would type it, for a listing.
    #[must_use]
    pub fn command_line(&self) -> String {
        if self.args.is_empty() {
            return self.program.clone();
        }
        format!("{} {}", self.program, self.args.join(" "))
    }
}

/// One agent this harness knows how to launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    /// The name a person types, for example `claude`.
    pub name: String,
    /// How to start it.
    pub launch: Launch,
    /// The agent is known to send the repository's code to a remote
    /// service when it runs.
    ///
    /// Recorded, not enforced: once the subprocess is running, what it
    /// sends is its own affair. A caller shows this so a person chooses
    /// knowingly, and dark mode refuses it (see the module
    /// documentation).
    pub reaches_network: bool,
}

impl Agent {
    /// Builds an agent from a command a person named themselves.
    ///
    /// Used for an agent [`KNOWN`] does not carry, and to correct one it
    /// gets wrong. `reaches_network` is the caller's claim about it:
    /// this function cannot know, and says so rather than guessing.
    #[must_use]
    pub fn configured(
        name: impl Into<String>,
        program: impl Into<String>,
        args: Vec<String>,
        reaches_network: bool,
    ) -> Self {
        Self {
            name: name.into(),
            launch: Launch {
                program: program.into(),
                args,
                needs_network_to_start: false,
            },
            reaches_network,
        }
    }
}

/// One row of the table: what an agent is called, and the two ways it
/// might be startable.
struct Known {
    /// The name a person types.
    name: &'static str,
    /// The binary to look for on the path, and the arguments that put it
    /// into ACP mode. `None` when the agent publishes no native binary
    /// that speaks the protocol itself.
    native: Option<(&'static str, &'static [&'static str])>,
    /// The npm package and arguments, for the `npx` fallback.
    npx: Option<(&'static str, &'static [&'static str])>,
    /// See [`Agent::reaches_network`].
    reaches_network: bool,
}

/// The agents this harness knows how to start, in the order a listing
/// shows them.
///
/// The npx forms are the ones the editor extensions that pioneered this
/// protocol use. The native forms are the same agents' own binaries,
/// which is what a person who installed the agent already has.
///
/// Codex has no native entry on purpose: its ACP support comes from a
/// separate adapter package rather than from the `codex` binary, so
/// finding `codex` on the path would say nothing about whether ACP
/// works. Claiming otherwise would produce an agent that appears
/// available and fails at the first message.
const KNOWN: &[Known] = &[
    Known {
        name: "claude",
        native: None,
        npx: Some(("@agentclientprotocol/claude-agent-acp@latest", &[])),
        reaches_network: true,
    },
    Known {
        name: "opencode",
        native: Some(("opencode", &["acp"])),
        npx: Some(("opencode-ai@latest", &["acp"])),
        reaches_network: true,
    },
    Known {
        name: "gemini",
        native: Some(("gemini", &["--experimental-acp"])),
        npx: Some(("@google/gemini-cli@latest", &["--experimental-acp"])),
        reaches_network: true,
    },
    Known {
        name: "qwen",
        native: Some(("qwen", &["--acp"])),
        npx: Some(("@qwen-code/qwen-code@latest", &["--acp"])),
        reaches_network: true,
    },
    Known {
        name: "codex",
        native: None,
        npx: Some(("@zed-industries/codex-acp@latest", &[])),
        reaches_network: true,
    },
    Known {
        name: "copilot",
        native: None,
        npx: Some(("@github/copilot-language-server@latest", &["--acp"])),
        reaches_network: true,
    },
    Known {
        name: "kiro",
        native: Some(("kiro-cli", &["acp"])),
        npx: None,
        reaches_network: true,
    },
    Known {
        name: "hermes",
        native: Some(("hermes", &["acp"])),
        npx: None,
        reaches_network: true,
    },
    Known {
        name: "openclaw",
        native: Some(("openclaw", &["acp"])),
        npx: Some(("openclaw", &["acp"])),
        reaches_network: true,
    },
];

/// Reports whether `path` is a file this user can execute.
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// Reports whether `path` is a file. Windows has no execute bit.
#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file())
}

/// Finds `program` in `path_var`, a `PATH`-shaped list of directories.
///
/// Taking the variable as a parameter rather than reading the process
/// environment keeps this testable against a fixture directory, and
/// keeps a test from depending on what happens to be installed on the
/// machine running it.
#[must_use]
pub fn find_on_path(program: &str, path_var: &str) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

/// Finds every known agent that this machine can actually start.
///
/// An agent with a native binary on `path_var` is reported with that
/// binary. One with no native binary, or whose binary is absent, falls
/// back to its npx form when it has one, and that form is marked
/// [`Launch::needs_network_to_start`]. An agent with neither is not
/// reported: it is not installed, and saying otherwise would offer a
/// choice that fails.
///
/// The order is [`KNOWN`]'s order, so two runs on one machine list the
/// same agents in the same order.
#[must_use]
pub fn find(path_var: &str) -> Vec<Agent> {
    KNOWN
        .iter()
        .filter_map(|known| {
            let launch = known
                .native
                .filter(|(program, _)| find_on_path(program, path_var).is_some())
                .map(|(program, args)| Launch::native(program, args))
                .or_else(|| {
                    // The npx form needs npx itself to be installed. An
                    // agent offered through a program this machine does
                    // not have is not an agent this machine can start.
                    let (package, args) = known.npx?;
                    find_on_path("npx", path_var).map(|_| Launch::npx(package, args))
                })?;

            Some(Agent {
                name: known.name.to_owned(),
                launch,
                reaches_network: known.reaches_network,
            })
        })
        .collect()
}

/// Finds the agent called `name`, when this machine can start it.
#[must_use]
pub fn find_named(name: &str, path_var: &str) -> Option<Agent> {
    find(path_var).into_iter().find(|agent| agent.name == name)
}

/// The names of every agent in the table, installed or not.
///
/// Used to tell a person who asked for an agent by a name this harness
/// does not know from one it knows but cannot find — two different
/// problems with two different remedies.
#[must_use]
pub fn known_names() -> Vec<&'static str> {
    KNOWN.iter().map(|known| known.name).collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// Creates a directory holding executable files with `names`.
    fn bin_dir(names: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for name in names {
            let path = dir.path().join(name);
            fs::write(&path, "#!/bin/sh\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        dir
    }

    /// The `PATH` value naming `dir`.
    fn path_var(dir: &tempfile::TempDir) -> String {
        dir.path().display().to_string()
    }

    #[test]
    fn an_empty_path_finds_nothing() {
        assert!(find("").is_empty());
    }

    #[test]
    fn a_native_binary_on_the_path_is_found() {
        let dir = bin_dir(&["opencode"]);
        let found = find(&path_var(&dir));

        assert_eq!(found.len(), 1, "found: {found:?}");
        assert_eq!(found[0].name, "opencode");
        assert_eq!(found[0].launch.program, "opencode");
        assert_eq!(found[0].launch.args, vec!["acp".to_owned()]);
        assert!(
            !found[0].launch.needs_network_to_start,
            "a binary already installed needs no download to start"
        );
    }

    #[test]
    fn a_native_binary_is_preferred_over_the_npx_form() {
        // Both routes are available. The native one starts with no
        // download, which is the whole point of preferring it.
        let dir = bin_dir(&["opencode", "npx"]);
        let found = find_named("opencode", &path_var(&dir)).expect("opencode is installed");

        assert_eq!(found.launch.program, "opencode");
        assert!(!found.launch.needs_network_to_start);
    }

    #[test]
    fn npx_is_the_fallback_and_is_marked_as_needing_the_network() {
        let dir = bin_dir(&["npx"]);
        let found = find_named("claude", &path_var(&dir)).expect("claude is reachable through npx");

        assert_eq!(found.launch.program, "npx");
        assert!(
            found.launch.needs_network_to_start,
            "npx <package>@latest downloads on every launch"
        );
        assert!(
            found.launch.args[0].contains("claude-agent-acp"),
            "args: {:?}",
            found.launch.args
        );
    }

    #[test]
    fn an_agent_with_no_native_binary_and_no_npx_is_not_reported() {
        // `claude` has no native ACP binary, so with no npx on the path
        // this machine cannot start it. Reporting it would offer a
        // choice that fails at launch.
        let dir = bin_dir(&["opencode"]);
        let found = find(&path_var(&dir));

        assert!(
            !found.iter().any(|agent| agent.name == "claude"),
            "found: {found:?}"
        );
    }

    #[test]
    fn an_agent_whose_binary_is_absent_is_not_reported() {
        let dir = bin_dir(&["something-else"]);
        assert!(find(&path_var(&dir)).is_empty());
    }

    #[test]
    fn a_binary_without_the_execute_bit_is_not_an_installed_agent() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("opencode"), "not executable").unwrap();

        #[cfg(unix)]
        assert!(
            find(&dir.path().display().to_string()).is_empty(),
            "a file that cannot be run is not an installed agent"
        );
    }

    #[test]
    fn a_directory_named_like_an_agent_is_not_an_agent() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("opencode")).unwrap();

        assert!(
            find(&dir.path().display().to_string()).is_empty(),
            "a directory is not an executable"
        );
    }

    #[test]
    fn several_agents_are_listed_in_the_tables_order() {
        let dir = bin_dir(&["qwen", "opencode", "gemini"]);
        let names: Vec<String> = find(&path_var(&dir))
            .into_iter()
            .map(|agent| agent.name)
            .collect();

        assert_eq!(
            names,
            vec![
                "opencode".to_owned(),
                "gemini".to_owned(),
                "qwen".to_owned()
            ],
            "the order is the table's, so two runs agree"
        );
    }

    #[test]
    fn a_configured_agent_takes_the_command_it_was_given() {
        let agent = Agent::configured("mine", "/opt/my-agent", vec!["--acp".to_owned()], false);

        assert_eq!(agent.launch.command_line(), "/opt/my-agent --acp");
        assert!(
            !agent.launch.needs_network_to_start,
            "a named command is not fetched"
        );
    }

    #[test]
    fn a_command_line_renders_as_a_person_would_type_it() {
        assert_eq!(
            Launch::native("hermes", &["acp"]).command_line(),
            "hermes acp"
        );
        assert_eq!(Launch::native("solo", &[]).command_line(), "solo");
        assert_eq!(
            Launch::npx("pkg@latest", &["--acp"]).command_line(),
            "npx pkg@latest --acp"
        );
    }

    #[test]
    fn every_known_agent_can_be_started_some_way() {
        // A row with neither route is unreachable and would be dead
        // weight in the table.
        for known in KNOWN {
            assert!(
                known.native.is_some() || known.npx.is_some(),
                "{} names no way to start it",
                known.name
            );
        }
    }

    #[test]
    fn every_known_agent_has_a_distinct_name() {
        let mut names = known_names();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(before, names.len(), "two rows share a name");
    }
}
