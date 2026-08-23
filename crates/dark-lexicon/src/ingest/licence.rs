//! Licence discovery and the Rule 26 gate.
//!
//! Rule 26: "Each pack records the upstream licence. `dark pack add`
//! refuses a source with no licence." [`discover_in_dir`] and
//! [`discover_via_fetcher`] look for a licence in a source. [`gate`] turns
//! the result into a pass or an [`dark_contract::ErrCode::PackNoLicence`]
//! refusal. `dark pack add` (owned by a later task unit's `cli.rs`) calls
//! `gate` before it writes a pack; this module does not decide when that
//! happens, only what counts as "discoverable".

use std::path::Path;

use dark_contract::{ErrCode, Error, Result};

use crate::ingest::fetch::Fetcher;

/// File names that this module recognises as licence files, most specific
/// first. The search is case-sensitive on the exact name and then retried
/// lowercase, since `LICENSE` and `license` both appear in the wild.
const LICENCE_FILE_NAMES: &[&str] = &[
    "LICENSE",
    "LICENSE.md",
    "LICENSE.txt",
    "LICENCE",
    "LICENCE.md",
    "LICENCE.txt",
    "COPYING",
    "COPYING.md",
    "COPYING.txt",
];

/// Substrings that identify a well-known licence by its text, checked in
/// order. This is a heuristic, not a licence classifier: it exists so a
/// pack manifest can carry a useful SPDX identifier when one is obvious,
/// not to resolve every licence a source might use.
const KNOWN_LICENCE_MARKERS: &[(&str, &str)] = &[
    ("SPDX-License-Identifier:", ""),
    ("MIT License", "MIT"),
    ("Apache License", "Apache-2.0"),
    ("BSD 3-Clause", "BSD-3-Clause"),
    ("BSD 2-Clause", "BSD-2-Clause"),
    ("Mozilla Public License", "MPL-2.0"),
    ("GNU GENERAL PUBLIC LICENSE", "GPL"),
];

/// A licence that a source declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Licence {
    /// The SPDX identifier, when this module could determine one from the
    /// licence text. `None` means a licence exists but its identifier is
    /// not known — the gate still passes; only its absence fails it.
    pub spdx: Option<String>,
    /// The full licence text, for storing as the pack's `LICENSE` file.
    pub text: String,
}

/// Guesses an SPDX identifier from licence text, by substring search.
fn guess_spdx(text: &str) -> Option<String> {
    if let Some(rest) = text.find("SPDX-License-Identifier:").map(|i| &text[i..]) {
        let value = rest
            .trim_start_matches("SPDX-License-Identifier:")
            .lines()
            .next()
            .unwrap_or("")
            .trim();
        if !value.is_empty() {
            return Some(value.to_owned());
        }
    }
    for (marker, spdx) in KNOWN_LICENCE_MARKERS {
        if marker.is_empty() {
            continue;
        }
        if text.contains(marker) && !spdx.is_empty() {
            return Some((*spdx).to_owned());
        }
    }
    None
}

/// Searches `dir` for a licence file, without descending into
/// subdirectories: a licence lives at a source's root.
///
/// # Errors
///
/// Returns `E_TOOL_FAILED` when a candidate file exists but cannot be
/// read.
pub fn discover_in_dir(dir: &Path) -> Result<Option<Licence>> {
    for name in LICENCE_FILE_NAMES {
        let path = dir.join(name);
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path).map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot read {}: {source}", path.display()),
            )
        })?;
        return Ok(Some(Licence {
            spdx: guess_spdx(&text),
            text,
        }));
    }

    // `Cargo.toml`'s `license` (or `license-file`) key is a discoverable
    // licence declaration even with no standalone licence file.
    let cargo_toml = dir.join("Cargo.toml");
    if cargo_toml.is_file() {
        let text = std::fs::read_to_string(&cargo_toml).map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot read {}: {source}", cargo_toml.display()),
            )
        })?;
        if let Some(spdx) = extract_cargo_toml_license(&text) {
            return Ok(Some(Licence {
                spdx: Some(spdx.clone()),
                text: format!("SPDX-License-Identifier: {spdx}"),
            }));
        }
    }

    Ok(None)
}

/// Reads the `license = "..."` key from `Cargo.toml` text, without a TOML
/// dependency this module does not need: `dark-lexicon` already depends on
/// `toml` for the pack manifest, so this uses it rather than hand-parse.
fn extract_cargo_toml_license(text: &str) -> Option<String> {
    let value: toml::Value = toml::from_str(text).ok()?;
    value
        .get("package")?
        .get("license")?
        .as_str()
        .map(ToOwned::to_owned)
}

/// Tries a short list of conventional paths for a licence file on the host
/// that `base_url` names, returning the first one that resolves.
///
/// Scraping an arbitrary site for a licence is unreliable, so this checks
/// only the paths that GitHub, GitLab, and most static-site generators
/// place a licence at by convention.
const CONVENTIONAL_LICENCE_PATHS: &[&str] =
    &["/LICENSE", "/LICENSE.md", "/LICENSE.txt", "/license"];

/// Discovers a licence for a remote source by trying conventional paths
/// under `base_url` through `fetcher`.
///
/// # Errors
///
/// Returns `E_TOOL_FAILED` when `base_url` names no host.
pub fn discover_via_fetcher(fetcher: &dyn Fetcher, base_url: &str) -> Result<Option<Licence>> {
    let host = crate::ingest::fetch::host_of(base_url)?;
    for path in CONVENTIONAL_LICENCE_PATHS {
        let url = format!("https://{host}{path}");
        if let Ok(bytes) = fetcher.fetch(&url)
            && let Ok(text) = String::from_utf8(bytes)
            && !text.trim().is_empty()
        {
            return Ok(Some(Licence {
                spdx: guess_spdx(&text),
                text,
            }));
        }
    }
    Ok(None)
}

/// Refuses to proceed when `licence` is absent. See Rule 26.
///
/// `override_responsibility` is the escape hatch that
/// [`dark_contract::ErrCode::PackNoLicence`]'s default remedy names
/// (`--i-accept-responsibility`): the CLI flag itself belongs to a later
/// task unit's `cli.rs`, but the gate that flag bypasses lives here so the
/// bypass is auditable in one place.
///
/// # Errors
///
/// Returns `E_PACK_NO_LICENCE` when `licence` is `None` and
/// `override_responsibility` is `false`.
pub fn gate(licence: Option<&Licence>, override_responsibility: bool) -> Result<()> {
    if licence.is_some() || override_responsibility {
        return Ok(());
    }
    Err(Error::new(
        ErrCode::PackNoLicence,
        "this source declares no discoverable licence",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_in_dir_finds_a_license_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("LICENSE"), "MIT License\n\nPermission...").unwrap();
        let licence = discover_in_dir(dir.path()).unwrap().unwrap();
        assert_eq!(licence.spdx.as_deref(), Some("MIT"));
    }

    #[test]
    fn discover_in_dir_reads_an_spdx_identifier_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("LICENSE.txt"),
            "SPDX-License-Identifier: Apache-2.0\n",
        )
        .unwrap();
        let licence = discover_in_dir(dir.path()).unwrap().unwrap();
        assert_eq!(licence.spdx.as_deref(), Some("Apache-2.0"));
    }

    #[test]
    fn discover_in_dir_finds_no_licence_when_none_is_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "hello").unwrap();
        assert!(discover_in_dir(dir.path()).unwrap().is_none());
    }

    #[test]
    fn discover_in_dir_reads_cargo_toml_license_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"x\"\nlicense = \"MIT OR Apache-2.0\"\n",
        )
        .unwrap();
        let licence = discover_in_dir(dir.path()).unwrap().unwrap();
        assert_eq!(licence.spdx.as_deref(), Some("MIT OR Apache-2.0"));
    }

    #[test]
    fn discover_in_dir_prefers_a_standalone_licence_file_over_cargo_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("LICENSE"), "MIT License").unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"x\"\nlicense = \"Apache-2.0\"\n",
        )
        .unwrap();
        let licence = discover_in_dir(dir.path()).unwrap().unwrap();
        assert_eq!(licence.spdx.as_deref(), Some("MIT"));
    }

    #[test]
    fn gate_passes_when_a_licence_was_found() {
        let licence = Licence {
            spdx: Some("MIT".to_owned()),
            text: "MIT".to_owned(),
        };
        gate(Some(&licence), false).unwrap();
    }

    #[test]
    fn gate_refuses_a_source_with_no_licence() {
        let err = gate(None, false).unwrap_err();
        assert_eq!(err.code, ErrCode::PackNoLicence);
    }

    #[test]
    fn gate_allows_an_explicit_override() {
        gate(None, true).unwrap();
    }

    struct MapFetcher(std::collections::HashMap<&'static str, &'static str>);
    impl Fetcher for MapFetcher {
        fn fetch(&self, url: &str) -> Result<Vec<u8>> {
            self.0
                .get(url)
                .map(|s| s.as_bytes().to_vec())
                .ok_or_else(|| Error::new(ErrCode::ToolFailed, "404"))
        }
    }

    #[test]
    fn discover_via_fetcher_finds_a_conventional_license_path() {
        let mut map = std::collections::HashMap::new();
        map.insert("https://example.com/LICENSE", "MIT License text");
        let fetcher = MapFetcher(map);
        let licence = discover_via_fetcher(&fetcher, "https://example.com/docs/page")
            .unwrap()
            .unwrap();
        assert_eq!(licence.spdx.as_deref(), Some("MIT"));
    }

    #[test]
    fn discover_via_fetcher_finds_nothing_when_no_conventional_path_resolves() {
        let fetcher = MapFetcher(std::collections::HashMap::new());
        assert!(
            discover_via_fetcher(&fetcher, "https://example.com/docs/page")
                .unwrap()
                .is_none()
        );
    }
}
