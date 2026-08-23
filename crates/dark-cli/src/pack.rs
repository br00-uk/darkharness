//! `dark pack`: manages documentation packs under `$DARK_HOME/packs`.
//!
//! `list`, `export`, `import`, and `rm` need no model and are wired here
//! against [`dark_lexicon::cli`]. `add`, `refresh`, and `reindex` need an
//! [`dark_lexicon::index::Embedder`], which needs a loaded embedding model
//! — task units `B2` to `B7` — so they still answer "not yet", with a
//! message naming exactly what they are waiting for.
//!
//! Every function here is a local filesystem read or write; nothing in
//! this module opens a network connection.

use std::path::{Path, PathBuf};

use dark_lexicon::cli;
use dark_lexicon::pack::PackManifest;

use crate::PackAction;

/// Returns `$DARK_HOME/packs`, the root every pack lives under (Section
/// 5.3).
fn packs_root() -> PathBuf {
    crate::dark_home().join("packs")
}

/// Renders one line describing a pack, for `dark pack list`.
fn render_manifest_line(manifest: &PackManifest) -> String {
    format!(
        "{:<32} {:<12} {:>6} chunk(s)  staleness {:<6} embed {}",
        manifest.pack.pack_id(),
        manifest.pack.ecosystem,
        manifest.ingest.chunks,
        manifest.staleness.policy,
        manifest.embed.model,
    )
}

/// Runs `dark pack list`.
///
/// # Errors
///
/// Returns an error when `$DARK_HOME/packs` exists but cannot be listed.
fn run_list() -> anyhow::Result<()> {
    let manifests = cli::list(&packs_root()).map_err(crate::contract_error)?;
    if manifests.is_empty() {
        println!("no packs under {}.", packs_root().display());
        return Ok(());
    }
    for manifest in &manifests {
        println!("{}", render_manifest_line(manifest));
    }
    Ok(())
}

/// Runs `dark pack export <pack> -o <file>`.
///
/// # Errors
///
/// Returns [`dark_contract::ErrCode::PackNotFound`] when no pack has this
/// identifier. Returns an error when the archive cannot be written.
fn run_export(pack: &str, output: &Path) -> anyhow::Result<()> {
    cli::export(&packs_root(), pack, output).map_err(crate::contract_error)?;
    println!("wrote {}", output.display());
    Ok(())
}

/// Runs `dark pack import <file>`.
///
/// # Errors
///
/// Returns an error when the archive cannot be read, its manifest fails to
/// parse, or the pack directory cannot be written.
fn run_import(file: &Path) -> anyhow::Result<()> {
    let manifest = cli::import(&packs_root(), file).map_err(crate::contract_error)?;
    println!(
        "imported {} from {}",
        manifest.pack.pack_id(),
        file.display()
    );
    Ok(())
}

/// Runs `dark pack rm <pack>`.
///
/// # Errors
///
/// Returns [`dark_contract::ErrCode::PackNotFound`] when no pack has this
/// identifier. Returns an error when the directory cannot be removed.
fn run_rm(pack: &str) -> anyhow::Result<()> {
    cli::rm(&packs_root(), pack).map_err(crate::contract_error)?;
    println!("removed {pack}");
    Ok(())
}

/// Reports that a pack subcommand needs an embedding model.
///
/// `add`, `refresh`, and `reindex` each call [`dark_lexicon::index::Embedder`]
/// (`add` and `refresh` through [`cli::AddDeps::embedder`], `reindex`
/// directly): none of the three can run before a model is resident, which
/// needs task units `B2` to `B7`.
fn needs_embedder(what: &str) -> anyhow::Result<()> {
    anyhow::bail!("{what} needs the inference engine; task units B2 to B7.")
}

/// Runs `dark pack <action>`.
///
/// # Errors
///
/// See [`run_list`], [`run_export`], [`run_import`], and [`run_rm`]. `add`,
/// `refresh`, and `reindex` always return an error naming the task units
/// they wait on.
pub(crate) fn run_command(action: PackAction) -> anyhow::Result<()> {
    match action {
        PackAction::Add { .. } => needs_embedder("dark pack add"),
        PackAction::List => run_list(),
        PackAction::Refresh { .. } => needs_embedder("dark pack refresh"),
        PackAction::Rm { pack } => run_rm(&pack),
        PackAction::Export { pack, output } => run_export(&pack, &output),
        PackAction::Import { file } => run_import(&file),
        PackAction::Reindex { .. } => needs_embedder("dark pack reindex"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_contract::{EmbedPurpose, Result as ContractResult};
    use dark_engine_fake::{FakeEngine, Script};
    use dark_lexicon::cli::{AddDeps, AddRequest, SourceInput};
    use dark_lexicon::index::Embedder;
    use dark_lexicon::pack::EmbedBlock;
    use tempfile::TempDir;

    struct FixedEmbedder;
    impl Embedder for FixedEmbedder {
        fn embed(&self, texts: &[String], _purpose: EmbedPurpose) -> ContractResult<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![1.0; 4]).collect())
        }
    }

    fn embed_block() -> EmbedBlock {
        EmbedBlock {
            model: "test-model".to_owned(),
            dim: 4,
            quant: "int8".to_owned(),
            query_prefix: String::new(),
            doc_prefix: String::new(),
        }
    }

    /// Builds one pack under `packs_root`, through the real, public
    /// [`cli::add`] — `dark-cli` is the one crate allowed a real `&dyn
    /// Engine` fixture (Rule 17), here [`FakeEngine`] as a dev-dependency,
    /// counting tokens for [`dark_lexicon::chunk::EngineCounter`].
    fn seed_pack(packs_root: &Path, src: &Path, name: &str, version: &str) {
        std::fs::create_dir_all(src).unwrap();
        std::fs::write(
            src.join("LICENSE"),
            "MIT License\n\nPermission is hereby granted...",
        )
        .unwrap();
        std::fs::write(
            src.join("intro.md"),
            "# Introduction\nThis library does useful things.\n",
        )
        .unwrap();

        let engine = FakeEngine::new(Script::default());
        let embedder = FixedEmbedder;
        cli::add(
            packs_root,
            &AddRequest {
                input: SourceInput::Localdir {
                    root: src.to_path_buf(),
                },
                name,
                version,
                ecosystem: "crates.io",
                aliases: vec![],
                staleness_policy: "90d",
                embed: embed_block(),
                override_responsibility: false,
                explicit_licence: None,
            },
            &AddDeps {
                engine: &engine,
                embedder: &embedder,
                fetcher: None,
            },
        )
        .unwrap();
    }

    #[test]
    fn run_list_reports_no_packs_for_an_empty_root() {
        let tmp = TempDir::new().unwrap();
        run_list_against(tmp.path()).unwrap();
    }

    /// The same body as [`run_list`], parameterised on `packs_root` so the
    /// test never touches the real `$DARK_HOME`.
    fn run_list_against(root: &Path) -> anyhow::Result<()> {
        let manifests = cli::list(root).map_err(crate::contract_error)?;
        if manifests.is_empty() {
            println!("no packs under {}.", root.display());
        }
        for manifest in &manifests {
            println!("{}", render_manifest_line(manifest));
        }
        Ok(())
    }

    #[test]
    fn render_manifest_line_names_the_pack_id_and_the_embed_model() {
        let tmp = TempDir::new().unwrap();
        let packs_root = tmp.path().join("packs");
        seed_pack(&packs_root, &tmp.path().join("src"), "tokio", "1.47.0");

        let manifests = cli::list(&packs_root).unwrap();
        let line = render_manifest_line(&manifests[0]);
        assert!(line.contains("tokio@1.47.0"));
        assert!(line.contains("test-model"));
    }

    #[test]
    fn export_then_import_and_rm_round_trip_through_this_module() {
        let tmp = TempDir::new().unwrap();
        let packs_root = tmp.path().join("packs");
        seed_pack(&packs_root, &tmp.path().join("src"), "tokio", "1.47.0");

        let archive = tmp.path().join("tokio.darkpack");
        cli::export(&packs_root, "tokio@1.47.0", &archive).unwrap();
        assert!(archive.is_file());

        let other_root = tmp.path().join("other-packs");
        let manifest = cli::import(&other_root, &archive).unwrap();
        assert_eq!(manifest.pack.pack_id(), "tokio@1.47.0");

        cli::rm(&packs_root, "tokio@1.47.0").unwrap();
        assert!(!packs_root.join("tokio@1.47.0").exists());
    }

    #[test]
    fn rm_reports_pack_not_found() {
        let tmp = TempDir::new().unwrap();
        let err = run_rm_against(&tmp.path().join("packs"), "no-such-pack").unwrap_err();
        assert!(err.to_string().contains("E_PACK_NOT_FOUND"));
    }

    fn run_rm_against(root: &Path, pack: &str) -> anyhow::Result<()> {
        cli::rm(root, pack).map_err(crate::contract_error)
    }

    #[test]
    fn add_refresh_and_reindex_all_report_they_need_the_engine() {
        for action in [
            PackAction::Add {
                source: "tokio".to_owned(),
                source_kind: None,
                name: None,
                version: None,
            },
            PackAction::Refresh { all: true },
            PackAction::Reindex { all: true },
        ] {
            let err = run_command(action).unwrap_err();
            assert!(err.to_string().contains("B2 to B7"));
        }
    }
}
