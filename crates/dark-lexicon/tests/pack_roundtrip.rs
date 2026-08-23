//! `G1`'s own verify command: `cargo nextest run -p dark-lexicon --test
//! pack_roundtrip`.
//!
//! "Done when": export and import produce identical chunk identifiers and
//! index hashes. This builds a small pack end to end — manifest, chunks
//! produced by the `heading-v1` chunker, and placeholder index files —
//! exports it to a `.darkpack`, imports it into a fresh directory, and
//! checks that every one of those survives the round trip unchanged: the
//! chunk identifiers computed from the chunker's own output, and the
//! BLAKE3 hash of each index file.

use dark_lexicon::chunk::{self, TokenCounter};
use dark_lexicon::ingest::Document;
use dark_lexicon::pack::manifest::{
    EmbedBlock, Ingest, License, PackId, PackManifest, Source, Staleness,
};
use dark_lexicon::pack::{self, hash as pack_hash};

/// A deterministic, dependency-free token counter for this test: one token
/// per whitespace-separated word. `heading-v1`'s determinism guarantee does
/// not depend on which counter is used, only on the counter being a pure
/// function of the text, so this fixture is enough to prove the
/// round-trip property without a real `Engine`.
struct WordCounter;
impl TokenCounter for WordCounter {
    fn count(&self, text: &str) -> dark_contract::Result<usize> {
        Ok(text.split_whitespace().count())
    }
}

fn sample_manifest() -> PackManifest {
    PackManifest {
        pack: PackId {
            name: "examplelib".to_owned(),
            version: "1.0.0".to_owned(),
            ecosystem: "crates.io".to_owned(),
            aliases: vec![],
        },
        source: Source {
            kind: "localdir".to_owned(),
            url: "file:///fixtures/examplelib".to_owned(),
            etag: String::new(),
            commit: String::new(),
        },
        ingest: Ingest {
            at: "2026-08-19T11:03:00Z".parse().expect("valid datetime"),
            tool_version: "1.0.0".to_owned(),
            chunker: chunk::ALGORITHM.to_owned(),
            chunks: 0, // filled in below, before writing
        },
        embed: EmbedBlock {
            model: "Qwen/Qwen3-Embedding-0.6B".to_owned(),
            dim: 1024,
            quant: "int8".to_owned(),
            query_prefix: String::new(),
            doc_prefix: String::new(),
        },
        staleness: Staleness {
            policy: "90d".to_owned(),
        },
        license: License {
            spdx: "MIT".to_owned(),
            notice_required: true,
        },
    }
}

/// Builds a small pack directory: a manifest, a `chunks.jsonl` produced by
/// the real `heading-v1` chunker, and placeholder `bm25.idx`, `dense.vec`,
/// and `graph.json` files standing in for `G4`'s index stage, which this
/// task unit does not own.
fn build_pack(dir: &std::path::Path) -> Vec<chunk::Chunk> {
    let doc = Document::new(
        "guide.md",
        "ExampleLib",
        "# Getting started\nInstall the crate, then read the quick reference below.\n\n\
         ## Configuration\nSet `worker_threads` to the number of cores you want to use, \
         then call `build()` to construct the runtime with those settings applied.\n",
    )
    .with_url("https://example.com/docs/guide");

    let chunks = chunk::chunk_with_counter(&WordCounter, "examplelib@1.0.0", &doc).expect("chunk");

    let mut manifest = sample_manifest();
    manifest.ingest.chunks = chunks.len() as u64;
    manifest.write_to_dir(dir).expect("write manifest");

    let mut chunks_jsonl = String::new();
    for chunk in &chunks {
        chunks_jsonl.push_str(&serde_json::to_string(chunk).expect("serialize chunk"));
        chunks_jsonl.push('\n');
    }
    std::fs::write(dir.join(pack::CHUNKS_FILE_NAME), chunks_jsonl).expect("write chunks.jsonl");
    std::fs::write(
        dir.join(pack::BM25_INDEX_FILE_NAME),
        b"lexical-index-placeholder",
    )
    .expect("write bm25.idx");
    std::fs::write(
        dir.join(pack::DENSE_VECTORS_FILE_NAME),
        b"dense-vector-placeholder",
    )
    .expect("write dense.vec");
    std::fs::write(dir.join(pack::GRAPH_FILE_NAME), b"{}").expect("write graph.json");
    std::fs::write(
        dir.join(pack::LICENSE_FILE_NAME),
        b"MIT License\n\nPermission is hereby granted...",
    )
    .expect("write LICENSE");

    chunks
}

#[test]
fn export_then_import_preserves_chunk_identifiers_and_index_file_hashes() {
    let source_dir = tempfile::tempdir().expect("tempdir");
    let source_chunks = build_pack(source_dir.path());
    let source_ids: Vec<String> = source_chunks.iter().map(|c| c.chunk_id.clone()).collect();

    let archive_dir = tempfile::tempdir().expect("tempdir");
    let archive_path = archive_dir.path().join("examplelib@1.0.0.darkpack");
    pack::export_darkpack(source_dir.path(), &archive_path).expect("export");

    let dest_dir = tempfile::tempdir().expect("tempdir");
    let imported_root = dest_dir.path().join("examplelib@1.0.0");
    let manifest = pack::import_darkpack(&archive_path, &imported_root).expect("import");

    assert_eq!(manifest.pack.pack_id(), "examplelib@1.0.0");

    // The chunk identifiers in the imported chunks.jsonl are byte-for-byte
    // the same as the ones the chunker produced before export: the file
    // round-tripped unchanged, so re-parsing it recovers the same ids in
    // the same order.
    let imported_jsonl = std::fs::read_to_string(imported_root.join(pack::CHUNKS_FILE_NAME))
        .expect("read chunks.jsonl");
    let imported_ids: Vec<String> = imported_jsonl
        .lines()
        .map(|line| {
            let chunk: chunk::Chunk = serde_json::from_str(line).expect("parse chunk line");
            chunk.chunk_id
        })
        .collect();
    assert_eq!(source_ids, imported_ids);
    assert!(
        !source_ids.is_empty(),
        "the fixture must produce at least one chunk"
    );

    // Every index file G4 will eventually own hashes the same before and
    // after the round trip: nothing in a single-file darkpack alters file
    // content.
    for name in [
        pack::BM25_INDEX_FILE_NAME,
        pack::DENSE_VECTORS_FILE_NAME,
        pack::GRAPH_FILE_NAME,
    ] {
        let before = blake3::hash(&std::fs::read(source_dir.path().join(name)).unwrap());
        let after = blake3::hash(&std::fs::read(imported_root.join(name)).unwrap());
        assert_eq!(before, after, "{name} hash changed across the round trip");
    }

    // Do 4: the imported pack's own hash file verifies.
    pack_hash::verify(&imported_root).expect("verify the imported pack's hash");
}

#[test]
fn importing_a_pack_whose_hash_was_never_written_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    build_pack(dir.path());
    // No `pack::export_darkpack` call, so `pack.hash` was never written:
    // Do 4 says the harness verifies the pack hash before use, so a pack
    // that never carried a hash must not open silently.
    let err = pack_hash::verify(dir.path()).unwrap_err();
    assert_eq!(err.code, dark_contract::ErrCode::PackNotFound);
}

#[test]
fn a_pack_edited_after_export_fails_the_hash_check_on_import() {
    let source_dir = tempfile::tempdir().expect("tempdir");
    build_pack(source_dir.path());

    let archive_dir = tempfile::tempdir().expect("tempdir");
    let archive_path = archive_dir.path().join("examplelib@1.0.0.darkpack");
    pack::export_darkpack(source_dir.path(), &archive_path).expect("export");

    // Tamper with one byte of the compressed archive to simulate a
    // corrupted or hand-edited pack reaching import.
    let mut bytes = std::fs::read(&archive_path).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    std::fs::write(&archive_path, bytes).unwrap();

    let dest_dir = tempfile::tempdir().expect("tempdir");
    let result = pack::import_darkpack(&archive_path, &dest_dir.path().join("out"));
    assert!(result.is_err(), "a tampered archive must not import");
}
