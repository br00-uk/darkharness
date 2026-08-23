//! The `docsrs` adapter: `cargo doc --output-format json`.
//!
//! G2 asks this adapter to use rustdoc's JSON output "for Rust crates. It
//! gives structured items and needs no HTML parsing." This adapter does
//! not run `cargo doc` itself — a caller with a subprocess seam runs the
//! command and hands this adapter the JSON text. This module reads the
//! JSON dynamically with `serde_json::Value` rather than binding rustdoc's
//! full schema: that schema is large, versioned, and not part of this
//! crate's contract with the rest of the harness, and a dynamic read stays
//! correct across the schema's minor revisions as long as the handful of
//! fields this adapter uses keep their names.

use std::collections::BTreeMap;

use dark_contract::{ErrCode, Error, Result};
use serde_json::Value;

use crate::ingest::document::{Document, Heading};

/// Parses `cargo doc --output-format json` output into one [`Document`] for
/// each local item that carries documentation text.
///
/// `base_url`, when given, becomes the prefix of each document's `url`;
/// pass the crate's docs.rs root, for example
/// `https://docs.rs/tokio/1.47.0/tokio/`.
///
/// Only items whose `crate_id` is the crate being documented are included:
/// a re-exported item from another crate belongs to that crate's own pack,
/// not this one. Only items with non-empty `docs` text produce a document;
/// an item with no doc comment has nothing for the chunker to split.
///
/// The result is sorted by dotted path with a byte comparator (Rule 30),
/// so the same input produces the same document order on every platform,
/// independent of the JSON object's own key order.
///
/// # Errors
///
/// Returns `E_TOOL_FAILED` when `json_text` is not valid JSON, or does not
/// have rustdoc JSON's top-level `index` and `paths` objects.
pub fn parse(json_text: &str, base_url: Option<&str>) -> Result<Vec<Document>> {
    let root: Value = serde_json::from_str(json_text)
        .map_err(|source| Error::new(ErrCode::ToolFailed, format!("not valid JSON: {source}")))?;

    let index = root
        .get("index")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::new(ErrCode::ToolFailed, "rustdoc JSON has no 'index' object"))?;
    let paths = root
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::new(ErrCode::ToolFailed, "rustdoc JSON has no 'paths' object"))?;

    let mut by_path: BTreeMap<String, Document> = BTreeMap::new();

    for (id, path_entry) in paths {
        let is_local = path_entry
            .get("crate_id")
            .and_then(Value::as_u64)
            .is_some_and(|crate_id| crate_id == 0);
        if !is_local {
            continue;
        }
        let Some(segments) = path_entry.get("path").and_then(Value::as_array) else {
            continue;
        };
        let dotted: Vec<&str> = segments.iter().filter_map(Value::as_str).collect();
        if dotted.is_empty() {
            continue;
        }
        let dotted_path = dotted.join("::");

        let docs = index
            .get(id)
            .and_then(|item| item.get("docs"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if docs.trim().is_empty() {
            continue;
        }

        let title = (*dotted.last().unwrap_or(&dotted_path.as_str())).to_owned();
        let path = format!("{}.md", dotted.join("/"));
        let mut document =
            Document::new(path, title, docs).with_headings(vec![Heading::new(1, &dotted_path)]);
        if let Some(base) = base_url {
            let anchor_kind = path_entry
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("item");
            let last = dotted.last().copied().unwrap_or_default();
            document = document.with_url(format!("{base}{anchor_kind}.{last}.html"));
        }
        by_path.insert(dotted_path, document);
    }

    Ok(by_path.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/docsrs/rustdoc.json");

    #[test]
    fn parses_local_documented_items_from_the_fixture() {
        let docs = parse(
            FIXTURE,
            Some("https://docs.rs/examplelib/1.0.0/examplelib/"),
        )
        .unwrap();
        let titles: Vec<&str> = docs.iter().map(|d| d.title.as_str()).collect();
        assert_eq!(titles, vec!["Builder", "spawn"]);
        assert!(docs[0].body.contains("Configures the runtime"));
        assert!(
            docs[0]
                .url
                .as_deref()
                .unwrap()
                .starts_with("https://docs.rs/")
        );
    }

    #[test]
    fn skips_external_and_undocumented_items() {
        let docs = parse(FIXTURE, None).unwrap();
        assert!(!docs.iter().any(|d| d.title == "ExternalThing"));
        assert!(!docs.iter().any(|d| d.title == "undocumented_fn"));
    }

    #[test]
    fn rejects_json_with_no_index() {
        let err = parse(r#"{"paths":{}}"#, None).unwrap_err();
        assert_eq!(err.code, ErrCode::ToolFailed);
    }

    #[test]
    fn output_order_is_sorted_by_dotted_path_not_by_json_key_order() {
        // "0:2" (item "b") appears before "0:1" (item "a") in the JSON
        // object's own key order, but the adapter must still emit "a"
        // first: it sorts by dotted path, not by JSON map order.
        let json = r#"{
            "index": {
                "0:2": {"docs": "second item", "crate_id": 0},
                "0:1": {"docs": "first item", "crate_id": 0}
            },
            "paths": {
                "0:2": {"crate_id": 0, "path": ["examplelib", "b"], "kind": "function"},
                "0:1": {"crate_id": 0, "path": ["examplelib", "a"], "kind": "function"}
            }
        }"#;
        let docs = parse(json, None).unwrap();
        let titles: Vec<&str> = docs.iter().map(|d| d.title.as_str()).collect();
        assert_eq!(titles, vec!["a", "b"]);
    }
}
