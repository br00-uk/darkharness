//! `G2`'s own verify command: `cargo nextest run -p dark-lexicon --test
//! licence_gate`.
//!
//! "Done when": a source with no licence is refused. Rule 26: "`dark pack
//! add` refuses a source with no licence." This proves the refusal for
//! both ways a source reaches `dark-lexicon`: a local directory
//! (`ingest::licence::discover_in_dir`, which `localdir` and `git` use)
//! and a fetched site (`ingest::licence::discover_via_fetcher`, which
//! `sitemap` uses), and proves the explicit override that
//! `--i-accept-responsibility` needs still exists for a source a person
//! has already checked by hand.

use dark_contract::{ErrCode, Error, Result};
use dark_lexicon::ingest::Fetcher;
use dark_lexicon::ingest::licence;

#[test]
fn a_local_source_with_no_licence_file_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("README.md"), "# Hello\nno licence here\n").unwrap();

    let discovered = licence::discover_in_dir(dir.path()).expect("discovery does not fail");
    assert!(
        discovered.is_none(),
        "the fixture must genuinely carry no licence"
    );

    let err = licence::gate(discovered.as_ref(), false).unwrap_err();
    assert_eq!(err.code, ErrCode::PackNoLicence);
}

#[test]
fn a_local_source_with_a_license_file_passes_the_gate() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("LICENSE"),
        "MIT License\n\nPermission is hereby granted...",
    )
    .unwrap();

    let discovered = licence::discover_in_dir(dir.path()).expect("discovery does not fail");
    assert!(discovered.is_some());
    licence::gate(discovered.as_ref(), false).expect("a discovered licence must pass the gate");
}

#[test]
fn a_local_source_with_only_a_cargo_toml_license_key_passes_the_gate() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"examplelib\"\nlicense = \"Apache-2.0\"\n",
    )
    .unwrap();

    let discovered = licence::discover_in_dir(dir.path()).expect("discovery does not fail");
    licence::gate(discovered.as_ref(), false).expect("a Cargo.toml license key must pass the gate");
    assert_eq!(discovered.unwrap().spdx.as_deref(), Some("Apache-2.0"));
}

/// A [`Fetcher`] whose fixed map of URLs to bodies stands in for a remote
/// site, so the `sitemap` adapter's licence discovery can be tested
/// without a network dependency `dark-lexicon` is not allowed to add. See
/// `crate::ingest::fetch`'s module docs for why this trait exists.
struct FixtureSite(std::collections::HashMap<&'static str, &'static str>);

impl Fetcher for FixtureSite {
    fn fetch(&self, url: &str) -> Result<Vec<u8>> {
        self.0
            .get(url)
            .map(|body| body.as_bytes().to_vec())
            .ok_or_else(|| Error::new(ErrCode::ToolFailed, format!("404: {url}")))
    }
}

#[test]
fn a_remote_source_with_no_licence_at_any_conventional_path_is_refused() {
    let site = FixtureSite(std::collections::HashMap::new());
    let discovered = licence::discover_via_fetcher(&site, "https://docs.example.com/guide")
        .expect("discovery does not fail");
    assert!(discovered.is_none());

    let err = licence::gate(discovered.as_ref(), false).unwrap_err();
    assert_eq!(err.code, ErrCode::PackNoLicence);
}

#[test]
fn a_remote_source_with_a_license_at_the_conventional_path_passes_the_gate() {
    let mut map = std::collections::HashMap::new();
    map.insert(
        "https://docs.example.com/LICENSE",
        "SPDX-License-Identifier: MIT\n",
    );
    let site = FixtureSite(map);
    let discovered = licence::discover_via_fetcher(&site, "https://docs.example.com/guide")
        .expect("discovery does not fail");
    licence::gate(discovered.as_ref(), false)
        .expect("a discovered remote licence must pass the gate");
    assert_eq!(discovered.unwrap().spdx.as_deref(), Some("MIT"));
}

#[test]
fn the_i_accept_responsibility_override_bypasses_the_gate() {
    // The CLI flag itself belongs to a later task unit's `cli.rs`, but the
    // bypass it needs lives in `licence::gate` so the escape hatch is
    // auditable in the one place that enforces Rule 26.
    licence::gate(None, true).expect("an explicit override must pass regardless of discovery");
}

#[test]
fn refusal_carries_the_pack_domain_error_code_and_a_remedy() {
    let err = licence::gate(None, false).unwrap_err();
    assert_eq!(err.code, ErrCode::PackNoLicence);
    assert_eq!(err.domain(), dark_contract::ErrDomain::Pack);
    assert!(
        err.remedy.is_some(),
        "a refusal must carry a remedy a person can act on"
    );
}
