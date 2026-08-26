//! `dark config`: reads and writes configuration (task unit `J2`).
//!
//! Configuration comes from five layers, a later one overriding an
//! earlier: built-in defaults, `$DARK_HOME/config.toml`,
//! `<repo>/.dark/config.toml`, `DARK_`-prefixed environment variables,
//! and command-line flags. [`dark_config::resolve`] does the merging;
//! this module supplies the five inputs and prints the result.
//!
//! # Why `explain` is the interesting one
//!
//! `get` answers "what is the value?", which a merged structure can
//! answer on its own. `explain` answers "and which of the five layers
//! set it?", which a plain merge cannot: the moment two layers touch the
//! same field, a merged structure has forgotten where the surviving value
//! came from. `dark-config` keeps a flat map of dotted key to
//! `(value, source)` pairs precisely so this command can answer that.
//!
//! # Where `set` writes
//!
//! Always `$DARK_HOME/config.toml` — the layer a person owns for every
//! repository. Writing to the project file instead would change a file
//! that is usually committed, on a command that reads as a personal
//! preference. Edit `<repo>/.dark/config.toml` directly to set something
//! for one repository.
//!
//! `set` refuses a key that no layer defines, so a typo is reported
//! rather than written into a file where it would sit and do nothing.

use anyhow::{Context as _, Result};
use dark_config::{Config, EnvMap, Sources, resolve};

use crate::ConfigAction;

/// Runs the `dark config` subcommand named by `action`.
pub(crate) fn run_command(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Get { key } => get(&key),
        ConfigAction::Set { key, value } => set(&key, &value),
        ConfigAction::Explain { key } => explain(&key),
    }
}

/// The built-in defaults, as TOML text.
///
/// Each section owner contributes its own text, concatenated under its
/// own top-level table so the result stays valid TOML. `dark-config`
/// carries no knowledge of any section itself (see [`Sources::defaults`]),
/// which is what lets a new section arrive without touching that crate.
fn defaults() -> Result<String> {
    let policy = toml::to_string_pretty(&PolicyDefaults {
        policy: dark_core::policy::PolicyConfig::default(),
    })
    .context("the built-in policy defaults will not serialise")?;
    let acp = toml::to_string_pretty(&AcpDefaults::default())
        .context("the built-in acp defaults will not serialise")?;
    Ok(policy + &acp)
}

/// Wraps [`dark_core::policy::PolicyConfig`] under a `policy` key, so it
/// serialises as a `[policy]` table rather than bare top-level keys.
#[derive(serde::Serialize)]
struct PolicyDefaults {
    /// The `[policy]` section.
    policy: dark_core::policy::PolicyConfig,
}

/// The `[acp]` section: which agent, if any, `dark` uses for a turn when
/// no local model is installed. Owned here rather than by `dark-acp`
/// (Rule 16 keeps that crate reaching for nothing else) — this is the
/// composition root's own preference, the same way `[policy]` is.
#[derive(Default, serde::Serialize)]
struct AcpDefaults {
    /// The `[acp]` table.
    acp: AcpSection,
}

/// One remembered choice: the agent [`crate::command::Action::Acp`]
/// picked last, by the name [`dark_acp::discover`] knows it by. Empty
/// means none was chosen — a key `set` refuses to write must already
/// exist among the defaults, so "unset" has to be a value, not an absent
/// key.
#[derive(Default, serde::Serialize)]
struct AcpSection {
    /// The agent's name, or empty for none.
    default: String,
}

/// Reads the remembered agent choice, if one was made.
///
/// # Errors
///
/// Returns an error when the configuration cannot be resolved — see
/// [`resolved`].
pub(crate) fn configured_agent() -> Result<Option<String>> {
    let config = resolved()?;
    let name = config
        .get("acp.default")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    Ok((!name.is_empty()).then(|| name.to_owned()))
}

/// Remembers `name` as the agent to use when no local model is installed.
/// An empty `name` clears the choice.
///
/// Prints nothing — the caller is inside the terminal application. See
/// [`write_value`].
///
/// # Errors
///
/// Returns an error when `$DARK_HOME/config.toml` cannot be read or
/// written.
pub(crate) fn set_configured_agent(name: &str) -> Result<()> {
    write_value("acp.default", name)?;
    Ok(())
}

/// Resolves the full configuration from the five layers.
fn resolved() -> Result<Config> {
    let dark_home = crate::dark_home();
    // A directory that is not a repository still has configuration: the
    // project layer is simply absent, which `resolve` treats as a missing
    // file rather than an error.
    let repo_root = crate::repo_root()?;
    let defaults = defaults()?;
    let env: EnvMap = std::env::vars().collect();
    let flags: Vec<(String, String)> = Vec::new();

    resolve(&Sources {
        defaults: &defaults,
        dark_home: &dark_home,
        repo_root: &repo_root,
        env: &env,
        flags: &flags,
    })
    .map_err(|err| anyhow::anyhow!("{err}"))
}

/// Renders a TOML value the way a person would type it back in.
fn render(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Runs `dark config get <key>`.
fn get(key: &str) -> Result<()> {
    let config = resolved()?;
    let value = config
        .get(key)
        .with_context(|| unknown_key_message(key, &config))?;
    println!("{}", render(value));
    Ok(())
}

/// Runs `dark config explain <key>`.
fn explain(key: &str) -> Result<()> {
    let config = resolved()?;
    let resolved_value = config
        .explain(key)
        .with_context(|| unknown_key_message(key, &config))?;

    println!("{key} = {}", render(&resolved_value.value));
    println!("set by: {}", resolved_value.source);
    Ok(())
}

/// Builds the message for a key no layer defines, naming the closest
/// keys that do.
///
/// A person who mistypes `policy.writes` is better served by the three
/// nearest real keys than by a list of every key there is.
fn unknown_key_message(key: &str, config: &Config) -> String {
    let mut near: Vec<&str> = config
        .keys()
        .filter(|known| {
            // Same section, or a name that contains what was typed.
            known.split('.').next() == key.split('.').next()
                || known.contains(key)
                || key.contains(*known)
        })
        .collect();
    near.sort_unstable();
    near.truncate(5);

    if near.is_empty() {
        format!("no configuration key is called {key}. Run dark config get to see what exists.")
    } else {
        format!(
            "no configuration key is called {key}. Did you mean: {}?",
            near.join(", ")
        )
    }
}

/// Runs `dark config set <key> <value>`.
fn set(key: &str, value: &str) -> Result<()> {
    let path = write_value(key, value)?;
    println!("{key} = {value}");
    println!("written to {}", path.display());
    Ok(())
}

/// Writes `value` at `key` in `$DARK_HOME/config.toml`, without printing
/// anything — the part [`set`] and [`set_configured_agent`] share.
///
/// A caller running inside the terminal application must not print: its
/// output would land under the alternate screen. See
/// `crate::shell::blocking_command`'s doc comment for the same reasoning
/// about the commands it wraps.
///
/// Returns the path written to.
fn write_value(key: &str, value: &str) -> Result<std::path::PathBuf> {
    let config = resolved()?;
    // A key no layer defines is a typo. Writing it would leave a line in
    // the file that never takes effect and never reports why.
    anyhow::ensure!(
        config.get(key).is_some(),
        "{}",
        unknown_key_message(key, &config)
    );

    let dark_home = crate::dark_home();
    let path = dark_home.join("config.toml");
    std::fs::create_dir_all(&dark_home)
        .with_context(|| format!("could not create {}", dark_home.display()))?;

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut document: toml::Table = toml::from_str(&existing)
        .with_context(|| format!("{} is not valid TOML", path.display()))?;

    write_dotted(&mut document, key, parse_value(value))?;

    let text = toml::to_string_pretty(&document)
        .context("the updated configuration will not serialise")?;
    std::fs::write(&path, &text).with_context(|| format!("could not write {}", path.display()))?;
    Ok(path)
}

/// Parses a command-line value into the narrowest TOML type it fits.
///
/// `write = "deny"` and `default_dark = true` are both typed on the
/// command line as bare words, and a policy value written as the string
/// `"true"` would fail to deserialise where a boolean is wanted.
fn parse_value(raw: &str) -> toml::Value {
    if let Ok(boolean) = raw.parse::<bool>() {
        return toml::Value::Boolean(boolean);
    }
    if let Ok(integer) = raw.parse::<i64>() {
        return toml::Value::Integer(integer);
    }
    if let Ok(float) = raw.parse::<f64>() {
        return toml::Value::Float(float);
    }
    toml::Value::String(raw.to_owned())
}

/// Writes `value` at the dotted `key`, creating the tables it names.
///
/// # Errors
///
/// Returns an error when a segment of `key` names something that is
/// already in the file but is not a table — `policy = 3` with
/// `policy.write` being set, say. Overwriting it would silently discard
/// what the person wrote, so this reports it instead.
fn write_dotted(document: &mut toml::Table, key: &str, value: toml::Value) -> Result<()> {
    let mut parts: Vec<&str> = key.split('.').collect();
    // `split` always yields at least one item, so this cannot be empty.
    let Some(leaf) = parts.pop() else {
        return Ok(());
    };

    let mut table = document;
    for part in parts {
        let entry = table
            .entry(part.to_owned())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        table = entry.as_table_mut().with_context(|| {
            format!("the configuration file already sets {part} to something that is not a table")
        })?;
    }
    table.insert(leaf.to_owned(), value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_word_that_is_not_a_literal_stays_a_string() {
        assert_eq!(parse_value("deny"), toml::Value::String("deny".to_owned()));
        assert_eq!(
            parse_value("Qwen/Qwen3-4B"),
            toml::Value::String("Qwen/Qwen3-4B".to_owned())
        );
    }

    #[test]
    fn a_boolean_is_parsed_as_one() {
        assert_eq!(parse_value("true"), toml::Value::Boolean(true));
        assert_eq!(parse_value("false"), toml::Value::Boolean(false));
    }

    #[test]
    fn a_number_is_parsed_as_one() {
        assert_eq!(parse_value("8192"), toml::Value::Integer(8192));
        assert_eq!(parse_value("0.8"), toml::Value::Float(0.8));
    }

    #[test]
    fn writing_a_dotted_key_creates_the_table_it_names() {
        let mut document = toml::Table::new();
        write_dotted(
            &mut document,
            "policy.write",
            toml::Value::String("deny".to_owned()),
        )
        .unwrap();

        let policy = document
            .get("policy")
            .and_then(toml::Value::as_table)
            .expect("the [policy] table was created");
        assert_eq!(
            policy.get("write").and_then(toml::Value::as_str),
            Some("deny")
        );
    }

    #[test]
    fn writing_a_key_keeps_the_other_keys_in_its_table() {
        let mut document: toml::Table =
            toml::from_str("[policy]\nwrite = \"confirm\"\nexec = \"deny\"\n").unwrap();
        write_dotted(
            &mut document,
            "policy.write",
            toml::Value::String("deny".to_owned()),
        )
        .unwrap();

        let policy = document
            .get("policy")
            .and_then(toml::Value::as_table)
            .unwrap();
        assert_eq!(
            policy.get("exec").and_then(toml::Value::as_str),
            Some("deny"),
            "setting one key must not drop its neighbours"
        );
        assert_eq!(
            policy.get("write").and_then(toml::Value::as_str),
            Some("deny")
        );
    }

    #[test]
    fn writing_a_key_keeps_the_other_tables() {
        let mut document: toml::Table =
            toml::from_str("[policy]\nwrite = \"confirm\"\n\n[hardware]\ndevice = \"cpu\"\n")
                .unwrap();
        write_dotted(
            &mut document,
            "policy.write",
            toml::Value::String("deny".to_owned()),
        )
        .unwrap();

        assert!(
            document.contains_key("hardware"),
            "setting a policy key must not drop the hardware section"
        );
    }

    #[test]
    fn a_parent_that_is_not_a_table_is_reported_not_overwritten() {
        let mut document: toml::Table = toml::from_str("policy = 3\n").unwrap();
        let err = write_dotted(
            &mut document,
            "policy.write",
            toml::Value::String("deny".to_owned()),
        )
        .unwrap_err();

        assert!(err.to_string().contains("policy"), "message: {err}");
        assert_eq!(
            document.get("policy").and_then(toml::Value::as_integer),
            Some(3),
            "the person's own value is left alone"
        );
    }

    #[test]
    fn the_defaults_are_valid_toml_and_carry_the_policy_section() {
        let text = defaults().expect("the defaults serialise");
        let parsed: toml::Table = toml::from_str(&text).expect("the defaults are valid TOML");
        assert!(parsed.contains_key("policy"), "defaults: {text}");
        assert!(parsed.contains_key("acp"), "defaults: {text}");
    }

    #[test]
    fn no_agent_is_chosen_by_default() {
        let dark_home = tempfile::tempdir().unwrap();
        let repo_root = tempfile::tempdir().unwrap();
        let defaults = defaults().unwrap();
        let env = EnvMap::new();
        let flags: Vec<(String, String)> = Vec::new();

        let config = resolve(&Sources {
            defaults: &defaults,
            dark_home: dark_home.path(),
            repo_root: repo_root.path(),
            env: &env,
            flags: &flags,
        })
        .unwrap();

        assert_eq!(
            config.get("acp.default").and_then(toml::Value::as_str),
            Some(""),
            "acp.default must exist among the defaults, or `set` refuses to write it"
        );
    }

    #[test]
    fn the_defaults_resolve_into_readable_keys() {
        let dark_home = tempfile::tempdir().unwrap();
        let repo_root = tempfile::tempdir().unwrap();
        let defaults = defaults().unwrap();
        let env = EnvMap::new();
        let flags: Vec<(String, String)> = Vec::new();

        let config = resolve(&Sources {
            defaults: &defaults,
            dark_home: dark_home.path(),
            repo_root: repo_root.path(),
            env: &env,
            flags: &flags,
        })
        .unwrap();

        assert!(
            config.get("policy.write").is_some(),
            "policy.write resolves from the built-in defaults"
        );
    }

    #[test]
    fn an_unknown_key_message_suggests_keys_in_the_same_section() {
        let dark_home = tempfile::tempdir().unwrap();
        let repo_root = tempfile::tempdir().unwrap();
        let defaults = defaults().unwrap();
        let env = EnvMap::new();
        let flags: Vec<(String, String)> = Vec::new();
        let config = resolve(&Sources {
            defaults: &defaults,
            dark_home: dark_home.path(),
            repo_root: repo_root.path(),
            env: &env,
            flags: &flags,
        })
        .unwrap();

        let message = unknown_key_message("policy.writes", &config);
        assert!(message.contains("policy.write"), "message: {message}");
    }
}
