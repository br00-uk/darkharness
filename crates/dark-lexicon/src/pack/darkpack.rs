//! The single-file pack form: `.darkpack`.
//!
//! Task unit `G1` asks for "a single-file form" that is "a zstd tarball
//! with the extension `.darkpack`". [`export`] writes the pack hash, then
//! tars the pack directory in a deterministic file order and compresses the
//! tar with zstd. [`import`] reverses that and verifies the pack hash
//! before it hands back the manifest, so a caller never opens a pack whose
//! archive was truncated or edited.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use dark_contract::{ErrCode, Error, Result};

use crate::pack::hash::{self, list_files_relative};
use crate::pack::manifest::PackManifest;

/// The default zstd compression level for a `.darkpack` archive.
///
/// Level 19 favours a small archive over fast compression: a pack is built
/// once and read many times, and it ships over a slow link or sits on a
/// laptop's disk, so the read side matters more than the write side.
const COMPRESSION_LEVEL: i32 = 19;

/// Builds one deterministic tar entry for `relative` under `dir`.
///
/// The header carries no modification time, so the same file content
/// always produces the same tar bytes regardless of when it was written to
/// disk. Rule 31 asks the same of hashed output; an archive that a caller
/// might diff or re-hash deserves the same treatment.
fn append_entry(builder: &mut tar::Builder<impl Write>, dir: &Path, relative: &str) -> Result<()> {
    let bytes = std::fs::read(dir.join(relative)).map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot read {relative}: {source}"),
        )
    })?;
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(0);
    header.set_cksum();
    builder
        .append_data(&mut header, relative, bytes.as_slice())
        .map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot add {relative} to the archive: {source}"),
            )
        })
}

/// Exports the pack directory at `dir` to the single file `out_path`.
///
/// This writes (or rewrites) `<dir>/pack.hash` first, over every other file
/// in `dir`, then archives all of `dir`, the hash file included, into a
/// zstd-compressed tar. Import later verifies that hash, so a caller can
/// tell a corrupted or hand-edited archive from a genuine one.
///
/// # Errors
///
/// Returns `E_TOOL_FAILED` when `dir` cannot be hashed or listed, when a
/// file cannot be read, or when `out_path` cannot be written.
pub fn export(dir: &Path, out_path: &Path) -> Result<()> {
    hash::write(dir)?;

    let mut relative_paths = list_files_relative(dir)?;
    relative_paths.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));

    let out_file = File::create(out_path).map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot create {}: {source}", out_path.display()),
        )
    })?;
    let encoder = zstd::stream::write::Encoder::new(out_file, COMPRESSION_LEVEL)
        .map_err(|source| Error::new(ErrCode::ToolFailed, format!("cannot start zstd: {source}")))?
        .auto_finish();
    let mut builder = tar::Builder::new(encoder);

    for relative in &relative_paths {
        append_entry(&mut builder, dir, relative)?;
    }

    builder.into_inner().map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot close the archive: {source}"),
        )
    })?;
    Ok(())
}

/// Imports the single-file pack at `path` into the directory `out_dir`.
///
/// `out_dir` must be empty or absent; this function creates it if it is
/// absent. After extraction, this verifies the pack hash and refuses to
/// return a manifest for a pack that fails the check.
///
/// # Errors
///
/// Returns `E_TOOL_FAILED` when `path` cannot be read, is not a valid
/// zstd-compressed tar, or when a member path would extract outside
/// `out_dir`. Returns `E_PACK_NOT_FOUND` when the extracted pack carries no
/// hash file. Returns `E_TOOL_FAILED` when the recomputed hash does not
/// match the stored one.
pub fn import(path: &Path, out_dir: &Path) -> Result<PackManifest> {
    std::fs::create_dir_all(out_dir).map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot create {}: {source}", out_dir.display()),
        )
    })?;

    let in_file = File::open(path).map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot open {}: {source}", path.display()),
        )
    })?;
    let decoder = zstd::stream::read::Decoder::new(in_file).map_err(|source| {
        Error::new(ErrCode::ToolFailed, format!("not a zstd stream: {source}"))
    })?;
    let mut archive = tar::Archive::new(decoder);

    let entries = archive
        .entries()
        .map_err(|source| Error::new(ErrCode::ToolFailed, format!("not a tar stream: {source}")))?;
    for entry in entries {
        let mut entry = entry.map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot read a tar entry: {source}"),
            )
        })?;
        let relative = entry
            .path()
            .map_err(|source| Error::new(ErrCode::ToolFailed, format!("bad entry path: {source}")))?
            .into_owned();

        // Rule 34: a write never leaves the intended root. `tar` resolves
        // `..` components itself when unpacking one entry at a time, but
        // this checks the path text too, so a `..` component is refused
        // outright rather than silently normalised away.
        if relative
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(Error::new(
                ErrCode::ToolFailed,
                format!(
                    "archive entry '{}' escapes the pack root",
                    relative.display()
                ),
            ));
        }

        let dest = out_dir.join(&relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|source| {
                Error::new(
                    ErrCode::ToolFailed,
                    format!("cannot create {}: {source}", parent.display()),
                )
            })?;
        }
        let mut out_file = File::create(&dest).map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot create {}: {source}", dest.display()),
            )
        })?;
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot read entry '{}': {source}", relative.display()),
            )
        })?;
        out_file.write_all(&buf).map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot write {}: {source}", dest.display()),
            )
        })?;
    }

    hash::verify(out_dir)?;
    PackManifest::read_from_dir(out_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_source_pack(dir: &Path) {
        std::fs::write(
            dir.join("pack.toml"),
            br#"
[pack]
name = "tokio"
version = "1.47.0"
ecosystem = "crates.io"

[source]
kind = "docsrs"
url = "https://docs.rs/tokio/1.47.0/tokio/"

[ingest]
at = 2026-08-19T11:03:00Z
tool_version = "1.0.0"
chunker = "heading-v1"
chunks = 2

[embed]
model = "Qwen/Qwen3-Embedding-0.6B"
dim = 1024
quant = "int8"

[staleness]
policy = "90d"

[license]
spdx = "MIT"
notice_required = true
"#,
        )
        .expect("write pack.toml");
        std::fs::write(
            dir.join("chunks.jsonl"),
            b"{\"chunk_id\":\"aaa\"}\n{\"chunk_id\":\"bbb\"}\n",
        )
        .expect("write chunks.jsonl");
        std::fs::write(dir.join("bm25.idx"), b"lexical-index-bytes").expect("write bm25.idx");
        std::fs::write(dir.join("dense.vec"), b"dense-vector-bytes").expect("write dense.vec");
        std::fs::write(dir.join("graph.json"), b"{}").expect("write graph.json");
        std::fs::write(dir.join("LICENSE"), b"MIT License text").expect("write LICENSE");
    }

    #[test]
    fn export_then_import_round_trips_every_file_byte_for_byte() {
        let src = tempfile::tempdir().expect("tempdir");
        build_source_pack(src.path());

        let archive_dir = tempfile::tempdir().expect("tempdir");
        let archive_path = archive_dir.path().join("tokio@1.47.0.darkpack");
        export(src.path(), &archive_path).expect("export");

        let dest = tempfile::tempdir().expect("tempdir");
        let out_dir = dest.path().join("tokio@1.47.0");
        let manifest = import(&archive_path, &out_dir).expect("import");
        assert_eq!(manifest.pack.pack_id(), "tokio@1.47.0");

        for name in [
            "chunks.jsonl",
            "bm25.idx",
            "dense.vec",
            "graph.json",
            "LICENSE",
        ] {
            let original = std::fs::read(src.path().join(name)).unwrap();
            let round_tripped = std::fs::read(out_dir.join(name)).unwrap();
            assert_eq!(original, round_tripped, "{name} differs after round trip");
        }
    }

    #[test]
    fn export_then_import_produces_the_same_directory_hash() {
        let src = tempfile::tempdir().expect("tempdir");
        build_source_pack(src.path());
        let source_hash = hash::write(src.path()).expect("hash source");

        let archive_dir = tempfile::tempdir().expect("tempdir");
        let archive_path = archive_dir.path().join("p.darkpack");
        export(src.path(), &archive_path).expect("export");

        let out_dir = tempfile::tempdir().expect("tempdir");
        import(&archive_path, out_dir.path()).expect("import");
        let imported_hash = hash::compute_dir(out_dir.path()).expect("hash imported");

        assert_eq!(source_hash, imported_hash);
    }

    #[test]
    fn import_refuses_a_tampered_archive() {
        let src = tempfile::tempdir().expect("tempdir");
        build_source_pack(src.path());
        let archive_dir = tempfile::tempdir().expect("tempdir");
        let archive_path = archive_dir.path().join("p.darkpack");
        export(src.path(), &archive_path).expect("export");

        // Corrupt one byte near the end of the archive, inside the
        // compressed payload rather than in the zstd frame header.
        let mut bytes = std::fs::read(&archive_path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        std::fs::write(&archive_path, bytes).unwrap();

        let out_dir = tempfile::tempdir().expect("tempdir");
        let result = import(&archive_path, out_dir.path());
        assert!(
            result.is_err(),
            "a tampered archive must not import cleanly"
        );
    }
}
