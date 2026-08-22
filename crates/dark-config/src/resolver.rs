//! Layered resolution: five sources merged one key at a time, with
//! provenance kept for every key.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::env::{EnvMap, canonical_env_name, fallback_dotted_key};
use crate::error::{Error, Result};
use crate::source::Source;
use crate::value::{flatten, parse_scalar, unflatten_section};

/// One resolved key: the value that won, and the source that set it.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedValue {
    /// The value after all five layers.
    pub value: toml::Value,
    /// The source that set the winning value.
    pub source: Source,
}

/// The five inputs to layered resolution.
///
/// Build one of these and pass it to [`resolve`]. Every field is
/// injectable: tests build `dark_home` and `repo_root` with `tempfile`, and
/// pass their own `env` snapshot, instead of touching the real home
/// directory or the real process environment.
#[derive(Debug, Clone, Copy)]
pub struct Sources<'a> {
    /// Built-in defaults, as TOML text.
    ///
    /// This crate carries no built-in knowledge of any section's keys —
    /// task units add sections later (`[policy]` from `A4`, `[hardware]`
    /// from `B6`, and so on) without touching this crate. Each section
    /// owner exposes its own default TOML text as a constant; the caller
    /// that assembles the full configuration concatenates every section's
    /// defaults (each under its own top-level table, so concatenation
    /// stays valid TOML) before calling [`resolve`]. Pass `""` for no
    /// defaults at all.
    pub defaults: &'a str,
    /// `$DARK_HOME`. [`resolve`] reads `dark_home.join("config.toml")` if
    /// that file exists; a missing file is not an error.
    pub dark_home: &'a Path,
    /// The repository root. [`resolve`] reads
    /// `repo_root.join(".dark").join("config.toml")` if that file exists; a
    /// missing file is not an error.
    pub repo_root: &'a Path,
    /// A snapshot of environment variables. Only names with the `DARK_`
    /// prefix are read; see [`crate::env`].
    pub env: &'a EnvMap,
    /// Command-line overrides, as `(dotted key, raw value)` pairs.
    ///
    /// The command-line parser (owned elsewhere, for example `dark-cli`)
    /// already knows which flag maps to which dotted key; this crate does
    /// not parse flags itself.
    pub flags: &'a [(String, String)],
}

/// The result of layered configuration resolution.
///
/// Holds every resolved key as a dotted path (`policy.write`), alongside
/// the value and the source that set it. Look up one key with
/// [`Config::explain`] — this is what `dark config explain <key>` reports —
/// or reconstruct a whole section with [`Config::section`].
#[derive(Debug, Clone, Default)]
pub struct Config {
    values: BTreeMap<String, ResolvedValue>,
}

impl Config {
    /// Returns the resolved value at `key`, without its source.
    pub fn get(&self, key: &str) -> Option<&toml::Value> {
        self.values.get(key).map(|resolved| &resolved.value)
    }

    /// Returns the resolved value at `key` and the source that set it.
    pub fn explain(&self, key: &str) -> Option<&ResolvedValue> {
        self.values.get(key)
    }

    /// Iterates every resolved key, in sorted order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(String::as_str)
    }

    /// Deserializes every key under `prefix` into `T`.
    ///
    /// A task unit that owns a configuration section calls this with its
    /// section name (for example `"policy"`) to get a typed value, instead
    /// of reaching into the resolved map by hand. A key outside `prefix` is
    /// not visible to `T`, so an unrelated section cannot leak in.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Section`] when the values under `prefix` do not
    /// match the shape of `T`.
    pub fn section<T: serde::de::DeserializeOwned>(&self, prefix: &str) -> Result<T> {
        let flat: BTreeMap<String, toml::Value> = self
            .values
            .iter()
            .map(|(key, resolved)| (key.clone(), resolved.value.clone()))
            .collect();
        let table = unflatten_section(&flat, prefix);
        toml::Value::Table(table)
            .try_into()
            .map_err(|source| Error::Section {
                prefix: prefix.to_string(),
                source,
            })
    }
}

/// Resolves configuration from the five layered sources in [`Sources`].
///
/// A later source overrides an earlier one at the level of one dotted key,
/// not one whole table: setting `policy.write` in the project file does
/// not disturb `policy.read` inherited from `$DARK_HOME`. The order is:
///
/// 1. `sources.defaults`.
/// 2. `$DARK_HOME/config.toml`.
/// 3. `<repo>/.dark/config.toml`.
/// 4. `DARK_`-prefixed environment variables in `sources.env`.
/// 5. `sources.flags`.
///
/// # Errors
///
/// Returns [`Error::Io`] when a configuration file exists but cannot be
/// read, [`Error::Parse`] when a file (or the defaults text) is not valid
/// TOML, and [`Error::SecretInFile`] when a file sets a key that looks like
/// a secret — see [`crate::token`] for where a secret belongs instead.
pub fn resolve(sources: &Sources<'_>) -> Result<Config> {
    let mut values: BTreeMap<String, ResolvedValue> = BTreeMap::new();

    load_defaults(sources.defaults, &mut values)?;
    load_file_layer(
        &sources.dark_home.join("config.toml"),
        Source::DarkHomeFile,
        &mut values,
    )?;
    load_file_layer(
        &sources.repo_root.join(".dark").join("config.toml"),
        Source::ProjectFile,
        &mut values,
    )?;
    apply_env(sources.env, &mut values);
    apply_flags(sources.flags, &mut values);

    Ok(Config { values })
}

/// Parses the built-in defaults text and inserts every leaf with
/// [`Source::Default`].
fn load_defaults(text: &str, values: &mut BTreeMap<String, ResolvedValue>) -> Result<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    let table: toml::Table = text.parse().map_err(|source| Error::Parse {
        path: PathBuf::from("<built-in defaults>"),
        source,
    })?;
    let mut flat = BTreeMap::new();
    flatten(&table, "", &mut flat);
    for (key, value) in flat {
        values.insert(
            key,
            ResolvedValue {
                value,
                source: Source::Default,
            },
        );
    }
    Ok(())
}

/// Loads one file layer if it exists, checks it for a secret key, and
/// overlays it onto `values`.
fn load_file_layer(
    path: &Path,
    make_source: impl Fn(PathBuf) -> Source,
    values: &mut BTreeMap<String, ResolvedValue>,
) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let table: toml::Table = text.parse().map_err(|source| Error::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    let mut flat = BTreeMap::new();
    flatten(&table, "", &mut flat);
    for (key, value) in flat {
        if is_secret_key(&key) {
            return Err(Error::SecretInFile {
                key,
                path: path.to_path_buf(),
            });
        }
        values.insert(
            key,
            ResolvedValue {
                value,
                source: make_source(path.to_path_buf()),
            },
        );
    }
    Ok(())
}

/// Reports whether `key`'s leaf segment names a secret.
///
/// A configuration file must not hold a Hugging Face token (see the
/// `Do not` rule in task unit `J2`). This is a conservative, generic guard
/// — any key literally named `token` — rather than one wired to a specific
/// section, since this crate does not own the `[huggingface]` section.
fn is_secret_key(key: &str) -> bool {
    let leaf = key.rsplit('.').next().unwrap_or(key);
    leaf.eq_ignore_ascii_case("token")
}

/// Overlays `DARK_`-prefixed environment variables onto `values`.
///
/// Only variables present in `env` are considered; this never reads the
/// real process environment. See [`crate::env`] for how a variable name
/// becomes a dotted key.
fn apply_env(env: &EnvMap, values: &mut BTreeMap<String, ResolvedValue>) {
    // Snapshot the keys known before this layer runs, so one `DARK_`
    // variable cannot match a key that another `DARK_` variable in the
    // same layer just introduced.
    let known: Vec<String> = values.keys().cloned().collect();
    for (name, raw) in env {
        if !name.starts_with(crate::env::ENV_PREFIX) {
            continue;
        }
        let key = known
            .iter()
            .find(|candidate| canonical_env_name(candidate) == *name)
            .cloned()
            .or_else(|| fallback_dotted_key(name));
        let Some(key) = key else { continue };
        values.insert(
            key,
            ResolvedValue {
                value: parse_scalar(raw),
                source: Source::EnvVar(name.clone()),
            },
        );
    }
}

/// Overlays command-line flags onto `values`. Each flag already names its
/// exact dotted key, so no name matching is needed.
fn apply_flags(flags: &[(String, String)], values: &mut BTreeMap<String, ResolvedValue>) {
    for (key, raw) in flags {
        values.insert(
            key.clone(),
            ResolvedValue {
                value: parse_scalar(raw),
                source: Source::Flag(key.clone()),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_files_are_not_an_error() {
        let dark_home = tempfile::tempdir().unwrap();
        let repo_root = tempfile::tempdir().unwrap();
        let env = EnvMap::new();
        let flags: Vec<(String, String)> = Vec::new();
        let sources = Sources {
            defaults: "[policy]\nwrite = \"confirm\"\n",
            dark_home: dark_home.path(),
            repo_root: repo_root.path(),
            env: &env,
            flags: &flags,
        };
        let config = resolve(&sources).unwrap();
        assert_eq!(
            config.get("policy.write").and_then(toml::Value::as_str),
            Some("confirm")
        );
    }

    #[test]
    fn invalid_toml_in_a_file_layer_is_reported() {
        let dark_home = tempfile::tempdir().unwrap();
        let repo_root = tempfile::tempdir().unwrap();
        std::fs::write(dark_home.path().join("config.toml"), "not = valid = toml").unwrap();
        let env = EnvMap::new();
        let flags: Vec<(String, String)> = Vec::new();
        let sources = Sources {
            defaults: "",
            dark_home: dark_home.path(),
            repo_root: repo_root.path(),
            env: &env,
            flags: &flags,
        };
        let err = resolve(&sources).unwrap_err();
        assert!(matches!(err, Error::Parse { .. }));
    }
}
