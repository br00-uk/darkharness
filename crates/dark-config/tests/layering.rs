//! Each of the five sources must override the one before it, and
//! `explain` must name the source that won — this is the "done when" for
//! task unit `J2`.

use std::collections::BTreeMap;
use std::path::Path;

use dark_config::{Source, Sources, resolve};

/// Builds the source set for one layer.
///
/// `Sources` borrows `env` and `flags`, so a value built once cannot
/// outlive a later mutation of either. Each layer builds its own.
fn sources<'a>(
    defaults: &'a str,
    dark_home: &'a Path,
    repo_root: &'a Path,
    env: &'a std::collections::BTreeMap<String, String>,
    flags: &'a [(String, String)],
) -> Sources<'a> {
    Sources {
        defaults,
        dark_home,
        repo_root,
        env,
        flags,
    }
}

fn write(dir: &Path, rel: &str, contents: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

#[test]
fn each_layer_overrides_the_previous_one_and_reports_its_source() {
    let dark_home = tempfile::tempdir().unwrap();
    let repo_root = tempfile::tempdir().unwrap();

    let defaults = "[policy]\nwrite = \"confirm\"\n";
    let mut env = BTreeMap::new();
    let mut flags: Vec<(String, String)> = Vec::new();

    // 1. Only the built-in default is present.
    let config = resolve(&sources(
        defaults,
        dark_home.path(),
        repo_root.path(),
        &env,
        &flags,
    ))
    .unwrap();
    let resolved = config.explain("policy.write").unwrap();
    assert_eq!(resolved.value.as_str(), Some("confirm"));
    assert_eq!(resolved.source, Source::Default);

    // 2. $DARK_HOME/config.toml overrides the default.
    write(
        dark_home.path(),
        "config.toml",
        "[policy]\nwrite = \"allow\"\n",
    );
    let config = resolve(&sources(
        defaults,
        dark_home.path(),
        repo_root.path(),
        &env,
        &flags,
    ))
    .unwrap();
    let resolved = config.explain("policy.write").unwrap();
    assert_eq!(resolved.value.as_str(), Some("allow"));
    assert_eq!(
        resolved.source,
        Source::DarkHomeFile(dark_home.path().join("config.toml"))
    );

    // 3. <repo>/.dark/config.toml overrides the dark-home file.
    write(
        repo_root.path(),
        ".dark/config.toml",
        "[policy]\nwrite = \"deny\"\n",
    );
    let config = resolve(&sources(
        defaults,
        dark_home.path(),
        repo_root.path(),
        &env,
        &flags,
    ))
    .unwrap();
    let resolved = config.explain("policy.write").unwrap();
    assert_eq!(resolved.value.as_str(), Some("deny"));
    assert_eq!(
        resolved.source,
        Source::ProjectFile(repo_root.path().join(".dark").join("config.toml"))
    );

    // 4. An environment variable overrides the project file.
    env.insert("DARK_POLICY_WRITE".to_string(), "confirm".to_string());
    let config = resolve(&sources(
        defaults,
        dark_home.path(),
        repo_root.path(),
        &env,
        &flags,
    ))
    .unwrap();
    let resolved = config.explain("policy.write").unwrap();
    assert_eq!(resolved.value.as_str(), Some("confirm"));
    assert_eq!(
        resolved.source,
        Source::EnvVar("DARK_POLICY_WRITE".to_string())
    );

    // 5. A command-line flag overrides the environment variable.
    flags.push(("policy.write".to_string(), "allow".to_string()));
    let config = resolve(&sources(
        defaults,
        dark_home.path(),
        repo_root.path(),
        &env,
        &flags,
    ))
    .unwrap();
    let resolved = config.explain("policy.write").unwrap();
    assert_eq!(resolved.value.as_str(), Some("allow"));
    assert_eq!(resolved.source, Source::Flag("policy.write".to_string()));
}

#[test]
fn a_later_source_overrides_only_the_key_it_sets() {
    // Setting policy.write in the project file must not disturb
    // policy.read, which still comes from the default.
    let dark_home = tempfile::tempdir().unwrap();
    let repo_root = tempfile::tempdir().unwrap();
    write(
        repo_root.path(),
        ".dark/config.toml",
        "[policy]\nwrite = \"deny\"\n",
    );

    let defaults = "[policy]\nwrite = \"confirm\"\nread = \"allow\"\n";
    let env = BTreeMap::new();
    let flags: Vec<(String, String)> = Vec::new();
    let sources = Sources {
        defaults,
        dark_home: dark_home.path(),
        repo_root: repo_root.path(),
        env: &env,
        flags: &flags,
    };

    let config = resolve(&sources).unwrap();
    assert_eq!(
        config.explain("policy.write").unwrap().value.as_str(),
        Some("deny")
    );
    assert_eq!(
        config.explain("policy.read").unwrap().value.as_str(),
        Some("allow")
    );
    assert_eq!(
        config.explain("policy.read").unwrap().source,
        Source::Default
    );
}

#[test]
fn an_unresolved_key_explains_to_nothing() {
    let dark_home = tempfile::tempdir().unwrap();
    let repo_root = tempfile::tempdir().unwrap();
    let env = BTreeMap::new();
    let flags: Vec<(String, String)> = Vec::new();
    let sources = Sources {
        defaults: "",
        dark_home: dark_home.path(),
        repo_root: repo_root.path(),
        env: &env,
        flags: &flags,
    };
    let config = resolve(&sources).unwrap();
    assert!(config.explain("nothing.here").is_none());
}

#[test]
fn an_env_var_without_the_dark_prefix_is_ignored() {
    let dark_home = tempfile::tempdir().unwrap();
    let repo_root = tempfile::tempdir().unwrap();
    let mut env = BTreeMap::new();
    env.insert("POLICY_WRITE".to_string(), "deny".to_string());
    env.insert("PATH".to_string(), "/usr/bin".to_string());
    let flags: Vec<(String, String)> = Vec::new();
    let sources = Sources {
        defaults: "[policy]\nwrite = \"confirm\"\n",
        dark_home: dark_home.path(),
        repo_root: repo_root.path(),
        env: &env,
        flags: &flags,
    };
    let config = resolve(&sources).unwrap();
    let resolved = config.explain("policy.write").unwrap();
    assert_eq!(resolved.value.as_str(), Some("confirm"));
    assert_eq!(resolved.source, Source::Default);
}
