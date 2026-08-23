//! The pack commands: task unit `G5`.
//!
//! ```text
//! dark pack add tokio --source docsrs --version 1.47.0
//! dark pack add ./internal-docs --name acme-platform --version 2026.8
//! dark pack list
//! dark pack refresh --all
//! dark pack export tokio@1.47.0 -o tokio.darkpack
//! dark pack import tokio.darkpack
//! dark pack reindex --all
//! ```
//!
//! `dark-cli` is not this task unit's to touch (Rule: change only the files
//! your task unit owns): "expose what a later change wires up" is the
//! brief. Every function below is a plain, synchronous entry point that a
//! later `dark-cli` change calls once it can supply the dependencies this
//! crate cannot construct itself — an `&dyn Engine`
//! (`crate::chunk::chunk_document` already takes one, per Rule 17 and
//! `crate::chunk`'s module docs), a `crate::index::Embedder`, and, for a
//! network source, a `crate::ingest::Fetcher`.
//!
//! [`SourceInput`] carries whatever raw material an ingest adapter needs —
//! Markdown text, `rustdoc` JSON, a rendered man page. Obtaining that
//! material (running `cargo doc`, running `man`, downloading a page) is
//! not this crate's job: Rule 13 gives `dark-airlock` the network client
//! and the subprocess seam, and `crate::ingest::manpage`'s own module docs
//! say the same about running `man`. [`add`] gates on a licence
//! ([`crate::ingest::licence::gate`], Rule 26), chunks every document
//! ([`crate::chunk::chunk_document`]), builds both indexes
//! ([`crate::index::Bm25Index`], [`crate::index::DenseIndex`]), and writes
//! the whole pack directory G1 defines, hash included.

use std::path::{Path, PathBuf};

use dark_contract::{EmbedPurpose, Engine, ErrCode, Error, Result, RoleClass};

use crate::chunk::{self, Chunk, TokenCounter};
use crate::index::{Bm25Index, DenseIndex, Embedder};
use crate::ingest::licence::{self, Licence};
use crate::ingest::{
    Document, Fetcher, docsrs, git, llms_txt, localdir, manpage, openapi, sitemap,
};
use crate::pack::{self, EmbedBlock, License, PackManifest};
use crate::tools::get::read_chunks;
use crate::tools::staleness;

/// The adapter that produced (or will produce) a pack's documents.
///
/// This names `--source`'s value: `docsrs`, `sitemap`, `git`, `localdir`,
/// `openapi`, `manpage`, or `llms-txt`. It carries no data of its own —
/// [`SourceInput`] carries the data once a caller has obtained it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// An `llms.txt` or `llms-full.txt` file.
    LlmsTxt,
    /// `cargo doc --output-format json`.
    Docsrs,
    /// A sitemap and its HTML pages.
    Sitemap,
    /// A repository at a tag.
    Git,
    /// A local directory.
    Localdir,
    /// An `OpenAPI` document.
    Openapi,
    /// A rendered manual page.
    Manpage,
}

impl SourceKind {
    /// Parses `--source`'s value.
    ///
    /// # Errors
    ///
    /// Returns `E_TOOL_INVALID_ARGS` when `text` names none of the seven
    /// adapters.
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "llms-txt" | "llms_txt" => Ok(Self::LlmsTxt),
            "docsrs" => Ok(Self::Docsrs),
            "sitemap" => Ok(Self::Sitemap),
            "git" => Ok(Self::Git),
            "localdir" => Ok(Self::Localdir),
            "openapi" => Ok(Self::Openapi),
            "manpage" => Ok(Self::Manpage),
            other => Err(Error::new(
                ErrCode::ToolInvalidArgs,
                format!(
                    "'{other}' is not a known pack source; use llms-txt, docsrs, sitemap, git, localdir, openapi, or manpage"
                ),
            )),
        }
    }

    /// The name this kind parses back from.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LlmsTxt => "llms-txt",
            Self::Docsrs => "docsrs",
            Self::Sitemap => "sitemap",
            Self::Git => "git",
            Self::Localdir => "localdir",
            Self::Openapi => "openapi",
            Self::Manpage => "manpage",
        }
    }

    /// Guesses a source kind with no `--source` flag at all: `source` names
    /// an existing local directory (the PRD's `dark pack add
    /// ./internal-docs` shape). Every other case needs an explicit
    /// `--source`, since a bare name like `tokio` gives no way to tell
    /// `docsrs` from `sitemap` from `git` apart.
    #[must_use]
    pub fn detect(source: &str) -> Option<Self> {
        Path::new(source).is_dir().then_some(Self::Localdir)
    }
}

/// The raw material one ingest adapter needs, already obtained by the
/// caller. See the module docs for why obtaining it is not this crate's
/// job.
pub enum SourceInput {
    /// A local directory of Markdown or plain-text files.
    Localdir {
        /// The directory to walk.
        root: PathBuf,
    },
    /// A repository checked out at the tag to ingest.
    Git {
        /// The worktree root.
        worktree_root: PathBuf,
        /// A URL template for building a source link per document, when
        /// the repository is hosted somewhere that supports one.
        url_template: Option<String>,
    },
    /// Already-produced `cargo doc --output-format json` output.
    Docsrs {
        /// The JSON text.
        json_text: String,
        /// The documentation's base URL, when it has one.
        base_url: Option<String>,
    },
    /// An already-fetched `OpenAPI` document.
    Openapi {
        /// The JSON text.
        json_text: String,
        /// The API's base URL, when it has one.
        base_url: Option<String>,
    },
    /// An already-rendered manual page (`man <page> | col -bx`).
    Manpage {
        /// The page name.
        name: String,
        /// The rendered text.
        rendered_text: String,
    },
    /// An already-read `llms.txt` or `llms-full.txt` file.
    LlmsTxt {
        /// A stable path for the one document this produces.
        path: String,
        /// The source URL, when it has one.
        url: Option<String>,
        /// The file's text.
        text: String,
    },
    /// A sitemap to fetch and walk. The only variant that itself performs
    /// network access, through the caller-supplied [`Fetcher`] — see
    /// `crate::ingest::fetch`'s module docs for why that seam exists.
    Sitemap {
        /// The sitemap URL.
        sitemap_url: String,
    },
}

impl SourceInput {
    /// This input's [`SourceKind`].
    #[must_use]
    pub fn kind(&self) -> SourceKind {
        match self {
            Self::Localdir { .. } => SourceKind::Localdir,
            Self::Git { .. } => SourceKind::Git,
            Self::Docsrs { .. } => SourceKind::Docsrs,
            Self::Openapi { .. } => SourceKind::Openapi,
            Self::Manpage { .. } => SourceKind::Manpage,
            Self::LlmsTxt { .. } => SourceKind::LlmsTxt,
            Self::Sitemap { .. } => SourceKind::Sitemap,
        }
    }

    /// The location text to record in the manifest's `[source] url` field.
    #[must_use]
    pub fn location(&self) -> String {
        match self {
            Self::Localdir { root } => root.display().to_string(),
            Self::Git { worktree_root, .. } => worktree_root.display().to_string(),
            Self::Docsrs { base_url, .. } | Self::Openapi { base_url, .. } => {
                base_url.clone().unwrap_or_default()
            }
            Self::Manpage { name, .. } => name.clone(),
            Self::LlmsTxt { path, url, .. } => url.clone().unwrap_or_else(|| path.clone()),
            Self::Sitemap { sitemap_url } => sitemap_url.clone(),
        }
    }
}

/// Produces the documents `input` describes.
///
/// `fetcher` is needed only for [`SourceInput::Sitemap`]; every other
/// variant already carries its own raw material.
///
/// # Errors
///
/// Returns whatever the underlying adapter returns. Returns
/// `E_TOOL_FAILED` when `input` is [`SourceInput::Sitemap`] and `fetcher`
/// is `None`.
pub fn ingest(input: &SourceInput, fetcher: Option<&dyn Fetcher>) -> Result<Vec<Document>> {
    match input {
        SourceInput::Localdir { root } => localdir::ingest(root),
        SourceInput::Git {
            worktree_root,
            url_template,
        } => git::ingest(worktree_root, url_template.as_deref()),
        SourceInput::Docsrs {
            json_text,
            base_url,
        } => docsrs::parse(json_text, base_url.as_deref()),
        SourceInput::Openapi {
            json_text,
            base_url,
        } => openapi::parse(json_text, base_url.as_deref()),
        SourceInput::Manpage {
            name,
            rendered_text,
        } => Ok(vec![manpage::parse(name, rendered_text)]),
        SourceInput::LlmsTxt { path, url, text } => {
            Ok(vec![llms_txt::parse(path, url.as_deref(), text)])
        }
        SourceInput::Sitemap { sitemap_url } => {
            let fetcher = fetcher.ok_or_else(|| {
                Error::new(
                    ErrCode::ToolFailed,
                    "a sitemap source needs a Fetcher; none was supplied",
                )
                .with_remedy("Pass a Fetcher backed by dark-airlock.")
            })?;
            let mut limiter = crate::ingest::RateLimiter::per_task_unit_g2();
            sitemap::ingest(fetcher, &mut limiter, sitemap_url)
        }
    }
}

/// Discovers a licence for `input`. `Localdir` and `Git` search their own
/// directory; `Sitemap` searches conventional paths through `fetcher`
/// (`None` skips this and reports no licence found, same as any other
/// source with nowhere to look). `Docsrs`, `Openapi`, `Manpage`, and
/// `LlmsTxt` have no directory or host of their own to search — a caller
/// that already found a licence for one of these (from `cargo metadata`,
/// for example) passes it as [`AddRequest::explicit_licence`] instead.
///
/// # Errors
///
/// Returns whatever the underlying discovery call returns.
pub fn discover_licence(
    input: &SourceInput,
    fetcher: Option<&dyn Fetcher>,
) -> Result<Option<Licence>> {
    match input {
        SourceInput::Localdir { root } => licence::discover_in_dir(root),
        SourceInput::Git { worktree_root, .. } => licence::discover_in_dir(worktree_root),
        SourceInput::Sitemap { sitemap_url } => match fetcher {
            Some(fetcher) => licence::discover_via_fetcher(fetcher, sitemap_url),
            None => Ok(None),
        },
        SourceInput::Docsrs { .. }
        | SourceInput::Openapi { .. }
        | SourceInput::Manpage { .. }
        | SourceInput::LlmsTxt { .. } => Ok(None),
    }
}

/// What `dark pack add` needs beyond an ingest source.
pub struct AddRequest<'a> {
    /// The already-obtained raw material to ingest.
    pub input: SourceInput,
    /// The pack name, for example `tokio`.
    pub name: &'a str,
    /// The version to record, for example `1.47.0`.
    pub version: &'a str,
    /// The ecosystem, for example `crates.io`.
    pub ecosystem: &'a str,
    /// Other names that a lookup should also match.
    pub aliases: Vec<String>,
    /// The staleness policy, for example `90d`.
    pub staleness_policy: &'a str,
    /// The embedding model this pack's dense vectors come from.
    pub embed: EmbedBlock,
    /// Bypasses Rule 26's licence gate. This is `--i-accept-responsibility`
    /// at `dark pack add`'s CLI surface; the flag itself belongs to
    /// `dark-cli`, the gate it bypasses lives in `crate::ingest::licence`.
    pub override_responsibility: bool,
    /// A licence the caller already found by some other means (for
    /// example `cargo metadata` for a `docsrs` source), checked before
    /// this function tries to discover one itself.
    pub explicit_licence: Option<Licence>,
}

/// What [`add`] and [`reindex`] need to run a model.
pub struct AddDeps<'a> {
    /// Counts tokens while chunking (`crate::chunk::chunk_document`).
    pub engine: &'a dyn Engine,
    /// Produces the dense index's vectors.
    pub embedder: &'a dyn Embedder,
    /// Fetches over the network. Needed only for
    /// [`SourceInput::Sitemap`].
    pub fetcher: Option<&'a dyn Fetcher>,
}

/// Builds today's `[ingest] at` value.
fn today_datetime() -> Result<toml::value::Datetime> {
    let epoch_day = staleness::today_epoch_day()?;
    let (year, month, day) = staleness::civil_from_days(epoch_day);
    let year = u16::try_from(year).map_err(|_| {
        Error::new(
            ErrCode::ToolFailed,
            "the system clock's year does not fit a TOML date",
        )
    })?;
    Ok(toml::value::Datetime {
        date: Some(toml::value::Date {
            year,
            month: u8::try_from(month).unwrap_or(1),
            day: u8::try_from(day).unwrap_or(1),
        }),
        time: None,
        offset: None,
    })
}

/// Writes `chunks.jsonl`: one JSON object per line, in chunk order.
fn write_chunks_jsonl(dir: &Path, chunks: &[Chunk]) -> Result<()> {
    let mut jsonl = String::new();
    for chunk in chunks {
        let line = serde_json::to_string(chunk).map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("a chunk will not serialize: {source}"),
            )
        })?;
        jsonl.push_str(&line);
        jsonl.push('\n');
    }
    let path = dir.join(pack::CHUNKS_FILE_NAME);
    std::fs::write(&path, jsonl).map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot write {}: {source}", path.display()),
        )
    })
}

/// Builds and writes both indexes over `chunks`.
fn build_and_write_indexes(dir: &Path, chunks: &[Chunk], embedder: &dyn Embedder) -> Result<()> {
    let bm25 = Bm25Index::build(chunks);
    let bm25_path = dir.join(pack::BM25_INDEX_FILE_NAME);
    std::fs::write(&bm25_path, bm25.to_bytes()).map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot write {}: {source}", bm25_path.display()),
        )
    })?;

    let texts: Vec<String> = chunks.iter().map(|c| c.embed_text.clone()).collect();
    let vectors = embedder.embed(&texts, EmbedPurpose::Document)?;
    let dense = DenseIndex::build(&vectors)?;
    let dense_path = dir.join(pack::DENSE_VECTORS_FILE_NAME);
    std::fs::write(&dense_path, dense.to_bytes()).map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot write {}: {source}", dense_path.display()),
        )
    })?;
    Ok(())
}

/// Ingests, chunks, indexes, and writes one pack, overwriting whatever
/// already sits at `<packs_root>/<name>@<version>`.
///
/// Counts tokens through `deps.engine` — literally `&dyn Engine`, per Rule
/// 17 and the shape `crate::chunk`'s module docs describe. [`add_with_counter`]
/// is what this wraps after building an `EngineCounter`; it is `pub(crate)`
/// so this module's own tests exercise the ingest-through-write pipeline
/// with a trivial [`TokenCounter`] fixture, without needing a concrete
/// `Engine` — this crate cannot build one at all (see `crate::chunk`'s
/// module docs for the wall).
///
/// # Errors
///
/// Returns `E_PACK_NO_LICENCE` when `req` gates on Rule 26 and no licence
/// was found or supplied. Returns `E_TOOL_FAILED` for an ingest, chunk,
/// embed, or write failure.
pub fn add(packs_root: &Path, req: &AddRequest<'_>, deps: &AddDeps<'_>) -> Result<PackManifest> {
    let counter = chunk::EngineCounter::new(deps.engine, RoleClass::Embed);
    add_with_counter(packs_root, req, &counter, deps.embedder, deps.fetcher)
}

/// The counter-driven pipeline [`add`] calls after wrapping `&dyn Engine`
/// in an [`crate::chunk::EngineCounter`]. See [`add`]'s docs.
///
/// # Errors
///
/// Same as [`add`].
pub(crate) fn add_with_counter(
    packs_root: &Path,
    req: &AddRequest<'_>,
    counter: &dyn TokenCounter,
    embedder: &dyn Embedder,
    fetcher: Option<&dyn Fetcher>,
) -> Result<PackManifest> {
    let documents = ingest(&req.input, fetcher)?;
    if documents.is_empty() {
        return Err(Error::new(
            ErrCode::ToolFailed,
            "the source produced no documents",
        ));
    }

    let licence = match &req.explicit_licence {
        Some(licence) => Some(licence.clone()),
        None => discover_licence(&req.input, fetcher)?,
    };
    licence::gate(licence.as_ref(), req.override_responsibility)?;

    let pack_id = format!("{}@{}", req.name, req.version);

    let mut chunks = Vec::new();
    for document in &documents {
        chunks.extend(chunk::chunk_with_counter(counter, &pack_id, document)?);
    }

    let dir = packs_root.join(&pack_id);
    std::fs::create_dir_all(&dir).map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot create {}: {source}", dir.display()),
        )
    })?;

    write_chunks_jsonl(&dir, &chunks)?;
    build_and_write_indexes(&dir, &chunks, embedder)?;

    let graph_path = dir.join(pack::GRAPH_FILE_NAME);
    std::fs::write(&graph_path, b"{}").map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot write {}: {source}", graph_path.display()),
        )
    })?;

    let license_path = dir.join(pack::LICENSE_FILE_NAME);
    let license_text = licence
        .as_ref()
        .map_or_else(String::new, |l| l.text.clone());
    std::fs::write(&license_path, license_text).map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot write {}: {source}", license_path.display()),
        )
    })?;

    let manifest = PackManifest {
        pack: pack::PackId {
            name: req.name.to_owned(),
            version: req.version.to_owned(),
            ecosystem: req.ecosystem.to_owned(),
            aliases: req.aliases.clone(),
        },
        source: pack::Source {
            kind: req.input.kind().as_str().to_owned(),
            url: req.input.location(),
            etag: String::new(),
            commit: String::new(),
        },
        ingest: pack::Ingest {
            at: today_datetime()?,
            tool_version: env!("CARGO_PKG_VERSION").to_owned(),
            chunker: chunk::ALGORITHM.to_owned(),
            chunks: chunks.len() as u64,
        },
        embed: req.embed.clone(),
        staleness: pack::Staleness {
            policy: req.staleness_policy.to_owned(),
        },
        license: License {
            spdx: licence.and_then(|l| l.spdx).unwrap_or_default(),
            notice_required: !req.override_responsibility || documents_have_licence(&dir),
        },
    };
    manifest.write_to_dir(&dir)?;
    pack::hash::write(&dir)?;

    Ok(manifest)
}

/// Whether a licence file was actually written for this pack (non-empty),
/// so `notice_required` never claims a notice exists when
/// `--i-accept-responsibility` shipped a pack with none.
fn documents_have_licence(dir: &Path) -> bool {
    std::fs::metadata(dir.join(pack::LICENSE_FILE_NAME)).is_ok_and(|m| m.len() > 0)
}

/// Refreshes a pack: re-ingests, re-chunks, and re-indexes it from the
/// same source, overwriting what is on disk now.
///
/// This is [`add`] run again over the same request. `chunks.jsonl`,
/// `bm25.idx`, and `dense.vec` must stay consistent with each other, so a
/// refresh rebuilds the whole pack rather than patching one file — the
/// same reasoning `crate::pack::embed`'s module docs give for treating a
/// model change as an all-or-nothing event.
///
/// # Errors
///
/// Returns whatever [`add`] returns.
pub fn refresh(
    packs_root: &Path,
    req: &AddRequest<'_>,
    deps: &AddDeps<'_>,
) -> Result<PackManifest> {
    add(packs_root, req, deps)
}

/// Rebuilds a pack's indexes from its existing `chunks.jsonl`, with no
/// re-ingest and no re-chunk.
///
/// This is the recovery path `crate::pack::embed::EmbedStatus::describe`
/// names: "the pack's model is '…'; the harness runs '…'. Serve lexical
/// results only. Run dark pack reindex." — rebuilding `dense.vec` (and,
/// for good measure, `bm25.idx`) against the model that is resident now.
///
/// # Errors
///
/// Returns `E_PACK_NOT_FOUND` when the pack or its chunk store is absent.
/// Returns `E_TOOL_FAILED` for an embed or write failure.
pub fn reindex(
    packs_root: &Path,
    pack_id: &str,
    embed: &EmbedBlock,
    embedder: &dyn Embedder,
) -> Result<PackManifest> {
    let dir = packs_root.join(pack_id);
    let chunks = read_chunks(&dir)?;

    build_and_write_indexes(&dir, &chunks, embedder)?;

    let mut manifest = PackManifest::read_from_dir(&dir)?;
    manifest.embed = embed.clone();
    manifest.write_to_dir(&dir)?;
    pack::hash::write(&dir)?;
    Ok(manifest)
}

/// Lists every pack under `packs_root`, sorted by pack identifier.
///
/// A directory that exists but carries no valid `pack.toml` is skipped
/// rather than failing the whole listing — one damaged pack should not
/// hide every other one.
///
/// # Errors
///
/// Returns `E_TOOL_FAILED` when `packs_root` exists but cannot be listed.
pub fn list(packs_root: &Path) -> Result<Vec<PackManifest>> {
    if !packs_root.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(packs_root).map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot list {}: {source}", packs_root.display()),
        )
    })?;

    let mut manifests = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!(
                    "cannot read a directory entry under {}: {source}",
                    packs_root.display()
                ),
            )
        })?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Ok(manifest) = PackManifest::read_from_dir(&path) {
            manifests.push(manifest);
        }
    }
    manifests.sort_by(|a, b| a.pack.pack_id().as_bytes().cmp(b.pack.pack_id().as_bytes()));
    Ok(manifests)
}

/// Removes a pack entirely.
///
/// # Errors
///
/// Returns `E_PACK_NOT_FOUND` when no pack directory exists at
/// `<packs_root>/<pack_id>`. Returns `E_TOOL_FAILED` when it cannot be
/// removed.
pub fn rm(packs_root: &Path, pack_id: &str) -> Result<()> {
    let dir = packs_root.join(pack_id);
    if !dir.is_dir() {
        return Err(Error::new(
            ErrCode::PackNotFound,
            format!("no pack at {}", dir.display()),
        ));
    }
    std::fs::remove_dir_all(&dir).map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot remove {}: {source}", dir.display()),
        )
    })
}

/// Writes a pack to one `.darkpack` file. Delegates to
/// [`pack::export_darkpack`], which writes the pack hash first.
///
/// # Errors
///
/// Returns whatever [`pack::export_darkpack`] returns.
pub fn export(packs_root: &Path, pack_id: &str, out_path: &Path) -> Result<()> {
    pack::export_darkpack(&packs_root.join(pack_id), out_path)
}

/// Reads a pack from one `.darkpack` file into `packs_root`, named by the
/// archive's own manifest.
///
/// Imports into a staging directory first, since the final directory name
/// (`<name>@<version>`) is not known until the manifest inside the
/// archive is read; a stale staging directory left by an interrupted
/// import is cleared before this one starts, since this crate has no
/// concurrent-import protocol to coordinate two at once.
///
/// # Errors
///
/// Returns whatever [`pack::import_darkpack`] returns. Returns
/// `E_TOOL_FAILED` when the staging directory cannot be prepared or the
/// final directory cannot be created.
pub fn import(packs_root: &Path, darkpack_path: &Path) -> Result<PackManifest> {
    let staging = packs_root.join(".importing");
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot clear {}: {source}", staging.display()),
            )
        })?;
    }
    let manifest = pack::import_darkpack(darkpack_path, &staging)?;

    let final_dir = packs_root.join(manifest.pack.pack_id());
    if final_dir.exists() {
        std::fs::remove_dir_all(&final_dir).map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot replace {}: {source}", final_dir.display()),
            )
        })?;
    }
    std::fs::rename(&staging, &final_dir).map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot move the imported pack into place: {source}"),
        )
    })?;

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `add`'s public signature takes `&dyn Engine`, per Rule 17 — but this
    /// crate cannot build one at all (see `crate::chunk`'s module docs for
    /// the wall: naming `Engine::stream`'s `tokio_util::sync::CancellationToken`
    /// needs `tokio-util` as a direct dependency, which `dark-lexicon` does
    /// not have). These tests exercise [`add_with_counter`] instead — the
    /// same seam `crate::chunk::chunk_with_counter` uses under
    /// `chunk_document` — with a whitespace-counting [`TokenCounter`]
    /// fixture that needs nothing beyond this crate's existing
    /// dependencies.
    struct WordCounter;
    impl TokenCounter for WordCounter {
        fn count(&self, text: &str) -> Result<usize> {
            Ok(text.split_whitespace().count())
        }
    }

    struct FixedEmbedder {
        dim: usize,
    }
    impl Embedder for FixedEmbedder {
        fn embed(&self, texts: &[String], _purpose: EmbedPurpose) -> Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![1.0; self.dim]).collect())
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

    fn deps() -> (WordCounter, FixedEmbedder) {
        (WordCounter, FixedEmbedder { dim: 4 })
    }

    fn write_source_dir(root: &Path) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(
            root.join("LICENSE"),
            "MIT License\n\nPermission is hereby granted...",
        )
        .unwrap();
        std::fs::write(
            root.join("intro.md"),
            "# Introduction\nThis library does useful things with async tasks.\n",
        )
        .unwrap();
    }

    #[test]
    fn source_kind_parses_every_documented_name() {
        for (text, kind) in [
            ("llms-txt", SourceKind::LlmsTxt),
            ("docsrs", SourceKind::Docsrs),
            ("sitemap", SourceKind::Sitemap),
            ("git", SourceKind::Git),
            ("localdir", SourceKind::Localdir),
            ("openapi", SourceKind::Openapi),
            ("manpage", SourceKind::Manpage),
        ] {
            assert_eq!(SourceKind::parse(text).unwrap(), kind);
            assert_eq!(SourceKind::parse(text).unwrap().as_str(), text);
        }
    }

    #[test]
    fn source_kind_parse_rejects_an_unknown_name() {
        let err = SourceKind::parse("ftp").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }

    #[test]
    fn source_kind_detects_localdir_from_an_existing_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            SourceKind::detect(dir.path().to_str().unwrap()),
            Some(SourceKind::Localdir)
        );
    }

    #[test]
    fn source_kind_detects_nothing_for_a_bare_name() {
        assert_eq!(SourceKind::detect("tokio"), None);
    }

    #[test]
    fn add_builds_a_complete_pack_directory() {
        let src = tempfile::tempdir().unwrap();
        write_source_dir(src.path());
        let packs_root = tempfile::tempdir().unwrap();
        let (counter, embedder) = deps();

        let manifest = add_with_counter(
            packs_root.path(),
            &AddRequest {
                input: SourceInput::Localdir {
                    root: src.path().to_path_buf(),
                },
                name: "examplelib",
                version: "1.0.0",
                ecosystem: "crates.io",
                aliases: vec![],
                staleness_policy: "90d",
                embed: embed_block(),
                override_responsibility: false,
                explicit_licence: None,
            },
            &counter,
            &embedder,
            None,
        )
        .unwrap();

        assert_eq!(manifest.pack.pack_id(), "examplelib@1.0.0");
        let dir = packs_root.path().join("examplelib@1.0.0");
        for name in [
            pack::MANIFEST_FILE_NAME,
            pack::CHUNKS_FILE_NAME,
            pack::BM25_INDEX_FILE_NAME,
            pack::DENSE_VECTORS_FILE_NAME,
            pack::GRAPH_FILE_NAME,
            pack::LICENSE_FILE_NAME,
            pack::HASH_FILE_NAME,
        ] {
            assert!(dir.join(name).is_file(), "{name} was not written");
        }
        pack::hash::verify(&dir).expect("the pack hash verifies");
    }

    #[test]
    fn add_refuses_a_source_with_no_licence() {
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("intro.md"), "# Hello\nno licence here\n").unwrap();
        let packs_root = tempfile::tempdir().unwrap();
        let (counter, embedder) = deps();

        let err = add_with_counter(
            packs_root.path(),
            &AddRequest {
                input: SourceInput::Localdir {
                    root: src.path().to_path_buf(),
                },
                name: "unlicensed",
                version: "1.0.0",
                ecosystem: "crates.io",
                aliases: vec![],
                staleness_policy: "90d",
                embed: embed_block(),
                override_responsibility: false,
                explicit_licence: None,
            },
            &counter,
            &embedder,
            None,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrCode::PackNoLicence);
    }

    #[test]
    fn add_honours_the_override_responsibility_escape_hatch() {
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("intro.md"), "# Hello\nno licence here\n").unwrap();
        let packs_root = tempfile::tempdir().unwrap();
        let (counter, embedder) = deps();

        let manifest = add_with_counter(
            packs_root.path(),
            &AddRequest {
                input: SourceInput::Localdir {
                    root: src.path().to_path_buf(),
                },
                name: "unlicensed",
                version: "1.0.0",
                ecosystem: "crates.io",
                aliases: vec![],
                staleness_policy: "90d",
                embed: embed_block(),
                override_responsibility: true,
                explicit_licence: None,
            },
            &counter,
            &embedder,
            None,
        )
        .unwrap();
        assert_eq!(manifest.pack.pack_id(), "unlicensed@1.0.0");
    }

    #[test]
    fn list_returns_every_pack_sorted_by_identifier() {
        let src = tempfile::tempdir().unwrap();
        write_source_dir(src.path());
        let packs_root = tempfile::tempdir().unwrap();
        let (counter, embedder) = deps();

        for (name, version) in [("zeta", "1.0.0"), ("alpha", "1.0.0")] {
            add_with_counter(
                packs_root.path(),
                &AddRequest {
                    input: SourceInput::Localdir {
                        root: src.path().to_path_buf(),
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
                &counter,
                &embedder,
                None,
            )
            .unwrap();
        }

        let listed = list(packs_root.path()).unwrap();
        let ids: Vec<String> = listed.iter().map(dark_contract_pack_id).collect();
        assert_eq!(ids, vec!["alpha@1.0.0".to_owned(), "zeta@1.0.0".to_owned()]);
    }

    fn dark_contract_pack_id(m: &PackManifest) -> String {
        m.pack.pack_id()
    }

    #[test]
    fn list_over_an_empty_directory_returns_nothing() {
        let packs_root = tempfile::tempdir().unwrap();
        assert!(list(packs_root.path()).unwrap().is_empty());
    }

    #[test]
    fn list_over_a_missing_directory_returns_nothing() {
        let packs_root = tempfile::tempdir().unwrap();
        let missing = packs_root.path().join("does-not-exist");
        assert!(list(&missing).unwrap().is_empty());
    }

    #[test]
    fn reindex_rebuilds_the_indexes_without_touching_chunks() {
        let src = tempfile::tempdir().unwrap();
        write_source_dir(src.path());
        let packs_root = tempfile::tempdir().unwrap();
        let (counter, embedder) = deps();

        add_with_counter(
            packs_root.path(),
            &AddRequest {
                input: SourceInput::Localdir {
                    root: src.path().to_path_buf(),
                },
                name: "examplelib",
                version: "1.0.0",
                ecosystem: "crates.io",
                aliases: vec![],
                staleness_policy: "90d",
                embed: embed_block(),
                override_responsibility: false,
                explicit_licence: None,
            },
            &counter,
            &embedder,
            None,
        )
        .unwrap();

        let dir = packs_root.path().join("examplelib@1.0.0");
        let chunks_before = std::fs::read(dir.join(pack::CHUNKS_FILE_NAME)).unwrap();

        let mut new_embed = embed_block();
        new_embed.model = "a-newer-model".to_owned();
        let manifest =
            reindex(packs_root.path(), "examplelib@1.0.0", &new_embed, &embedder).unwrap();

        assert_eq!(manifest.embed.model, "a-newer-model");
        let chunks_after = std::fs::read(dir.join(pack::CHUNKS_FILE_NAME)).unwrap();
        assert_eq!(
            chunks_before, chunks_after,
            "reindex must not touch chunks.jsonl"
        );
        pack::hash::verify(&dir).expect("the pack hash verifies after reindex");
    }

    #[test]
    fn rm_removes_a_pack_directory() {
        let src = tempfile::tempdir().unwrap();
        write_source_dir(src.path());
        let packs_root = tempfile::tempdir().unwrap();
        let (counter, embedder) = deps();
        add_with_counter(
            packs_root.path(),
            &AddRequest {
                input: SourceInput::Localdir {
                    root: src.path().to_path_buf(),
                },
                name: "examplelib",
                version: "1.0.0",
                ecosystem: "crates.io",
                aliases: vec![],
                staleness_policy: "90d",
                embed: embed_block(),
                override_responsibility: false,
                explicit_licence: None,
            },
            &counter,
            &embedder,
            None,
        )
        .unwrap();

        rm(packs_root.path(), "examplelib@1.0.0").unwrap();
        assert!(!packs_root.path().join("examplelib@1.0.0").exists());
    }

    #[test]
    fn rm_reports_pack_not_found_for_a_missing_pack() {
        let packs_root = tempfile::tempdir().unwrap();
        let err = rm(packs_root.path(), "nonexistent@0.0.0").unwrap_err();
        assert_eq!(err.code, ErrCode::PackNotFound);
    }

    #[test]
    fn export_then_import_round_trips_a_pack_built_by_add() {
        let src = tempfile::tempdir().unwrap();
        write_source_dir(src.path());
        let packs_root = tempfile::tempdir().unwrap();
        let (counter, embedder) = deps();
        add_with_counter(
            packs_root.path(),
            &AddRequest {
                input: SourceInput::Localdir {
                    root: src.path().to_path_buf(),
                },
                name: "examplelib",
                version: "1.0.0",
                ecosystem: "crates.io",
                aliases: vec![],
                staleness_policy: "90d",
                embed: embed_block(),
                override_responsibility: false,
                explicit_licence: None,
            },
            &counter,
            &embedder,
            None,
        )
        .unwrap();

        let archive_dir = tempfile::tempdir().unwrap();
        let archive_path = archive_dir.path().join("examplelib.darkpack");
        export(packs_root.path(), "examplelib@1.0.0", &archive_path).unwrap();

        let other_root = tempfile::tempdir().unwrap();
        let manifest = import(other_root.path(), &archive_path).unwrap();
        assert_eq!(manifest.pack.pack_id(), "examplelib@1.0.0");
        assert!(
            other_root
                .path()
                .join("examplelib@1.0.0")
                .join(pack::MANIFEST_FILE_NAME)
                .is_file()
        );
    }

    #[test]
    fn import_replaces_an_existing_pack_of_the_same_identifier() {
        let src = tempfile::tempdir().unwrap();
        write_source_dir(src.path());
        let packs_root = tempfile::tempdir().unwrap();
        let (counter, embedder) = deps();
        add_with_counter(
            packs_root.path(),
            &AddRequest {
                input: SourceInput::Localdir {
                    root: src.path().to_path_buf(),
                },
                name: "examplelib",
                version: "1.0.0",
                ecosystem: "crates.io",
                aliases: vec![],
                staleness_policy: "90d",
                embed: embed_block(),
                override_responsibility: false,
                explicit_licence: None,
            },
            &counter,
            &embedder,
            None,
        )
        .unwrap();

        let archive_dir = tempfile::tempdir().unwrap();
        let archive_path = archive_dir.path().join("examplelib.darkpack");
        export(packs_root.path(), "examplelib@1.0.0", &archive_path).unwrap();

        // Import into the same root that already has this pack.
        import(packs_root.path(), &archive_path).unwrap();
        assert!(
            packs_root
                .path()
                .join("examplelib@1.0.0")
                .join(pack::MANIFEST_FILE_NAME)
                .is_file()
        );
    }
}
