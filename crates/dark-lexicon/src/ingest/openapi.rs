//! The `openapi` adapter: one document for each operation.
//!
//! G2: "Produce one document for each operation." This adapter reads a
//! JSON `OpenAPI` document (`dark-lexicon` has no YAML dependency, so a
//! caller with a YAML-to-JSON seam converts a YAML spec before calling
//! this) and, for every `(path, method)` pair under `paths`, builds one
//! [`Document`] from that operation's summary, description, parameters,
//! and request body.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use dark_contract::{ErrCode, Error, Result};
use serde_json::Value;

use crate::ingest::document::{Document, Heading};

/// HTTP methods that `OpenAPI`'s `paths` object recognises as operations.
const OPERATION_METHODS: &[&str] = &[
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// Parses an `OpenAPI` JSON document into one [`Document`] per operation.
///
/// Each document's `path` is `{method}_{url path}`, for example
/// `get_/pets/{petId}`, which is unique within one spec because an HTTP
/// method appears at most once per URL path. `title` is the operation's
/// `operationId` when present, otherwise `{METHOD} {path}`. The body lists
/// the summary, the description, and the parameters as Markdown.
///
/// Operations are emitted sorted by `path` (Rule 30's byte comparator),
/// then by method in the fixed order [`OPERATION_METHODS`] lists, so the
/// result does not depend on the JSON object's own key order.
///
/// # Errors
///
/// Returns `E_TOOL_FAILED` when `json_text` is not valid JSON or has no
/// top-level `paths` object.
pub fn parse(json_text: &str, base_url: Option<&str>) -> Result<Vec<Document>> {
    let root: Value = serde_json::from_str(json_text)
        .map_err(|source| Error::new(ErrCode::ToolFailed, format!("not valid JSON: {source}")))?;
    let paths = root
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            Error::new(
                ErrCode::ToolFailed,
                "OpenAPI document has no 'paths' object",
            )
        })?;

    let mut by_path_and_method: BTreeMap<String, Vec<Document>> = BTreeMap::new();

    for (url_path, item) in paths {
        let Some(item) = item.as_object() else {
            continue;
        };
        let mut per_path = Vec::new();
        for method in OPERATION_METHODS {
            let Some(operation) = item.get(*method) else {
                continue;
            };
            per_path.push(build_document(url_path, method, operation, base_url));
        }
        if !per_path.is_empty() {
            by_path_and_method.insert(url_path.clone(), per_path);
        }
    }

    Ok(by_path_and_method.into_values().flatten().collect())
}

/// Builds one document for a single `(path, method)` operation.
fn build_document(
    url_path: &str,
    method: &str,
    operation: &Value,
    base_url: Option<&str>,
) -> Document {
    let operation_id = operation.get("operationId").and_then(Value::as_str);
    let summary = operation
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("");
    let description = operation
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");

    let title = operation_id.map_or_else(
        || format!("{} {url_path}", method.to_ascii_uppercase()),
        ToOwned::to_owned,
    );

    let mut body = String::new();
    if !summary.is_empty() {
        body.push_str(summary);
        body.push_str("\n\n");
    }
    if !description.is_empty() {
        body.push_str(description);
        body.push_str("\n\n");
    }
    if let Some(parameters) = operation.get("parameters").and_then(Value::as_array)
        && !parameters.is_empty()
    {
        body.push_str("## Parameters\n\n");
        for parameter in parameters {
            let name = parameter.get("name").and_then(Value::as_str).unwrap_or("?");
            let location = parameter.get("in").and_then(Value::as_str).unwrap_or("");
            let required = parameter
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let param_description = parameter
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            let _ = writeln!(
                body,
                "- `{name}` ({location}{}): {param_description}",
                if required { ", required" } else { "" }
            );
        }
        body.push('\n');
    }

    let doc_path = format!("{method}_{url_path}");
    let mut document = Document::new(doc_path, title.clone(), body.trim_end().to_owned())
        .with_headings(vec![Heading::new(1, &title)]);
    if let Some(base) = base_url {
        document = document.with_url(format!("{base}#{method}-{url_path}"));
    }
    document
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/openapi/petstore.json");

    #[test]
    fn produces_one_document_per_operation() {
        let docs = parse(FIXTURE, Some("https://api.example.com/openapi")).unwrap();
        let titles: Vec<&str> = docs.iter().map(|d| d.title.as_str()).collect();
        assert_eq!(titles, vec!["listPets", "createPet", "showPetById"]);
    }

    #[test]
    fn includes_parameters_in_the_body() {
        let docs = parse(FIXTURE, None).unwrap();
        let show_pet = docs.iter().find(|d| d.title == "showPetById").unwrap();
        assert!(show_pet.body.contains("petId"));
        assert!(show_pet.body.contains("required"));
    }

    #[test]
    fn rejects_a_document_with_no_paths() {
        let err = parse(r#"{"openapi": "3.0.0"}"#, None).unwrap_err();
        assert_eq!(err.code, ErrCode::ToolFailed);
    }
}
