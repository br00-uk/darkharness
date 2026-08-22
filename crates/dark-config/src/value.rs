//! Conversion between a nested TOML table and dotted-key leaf values.
//!
//! [`crate::resolve`] works one leaf key at a time (`policy.write`, not the
//! whole `[policy]` table) so that a later source overrides one field
//! without disturbing its siblings. These functions do the flattening and
//! the reverse.

use std::collections::BTreeMap;

/// Flattens a TOML table into dotted-key leaf values.
///
/// Recurses into nested tables, joining keys with `.`. An array or a
/// scalar becomes one leaf; the function does not descend into array
/// elements. `prefix` is the dotted path built so far; call with `""` at
/// the top level.
pub(crate) fn flatten(table: &toml::Table, prefix: &str, out: &mut BTreeMap<String, toml::Value>) {
    for (key, value) in table {
        let dotted = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            toml::Value::Table(nested) => flatten(nested, &dotted, out),
            other => {
                out.insert(dotted, other.clone());
            }
        }
    }
}

/// Rebuilds a nested TOML table from dotted-key leaf values, keeping only
/// the keys under `prefix`.
///
/// Strips `"{prefix}."` from each matching key before inserting it, so
/// `unflatten_section(values, "policy")` turns `policy.write` into
/// `write` inside the returned table. A key that does not start with the
/// prefix is skipped.
pub(crate) fn unflatten_section(
    values: &BTreeMap<String, toml::Value>,
    prefix: &str,
) -> toml::Table {
    let mut root = toml::Table::new();
    let head = format!("{prefix}.");
    for (key, value) in values {
        let Some(rest) = key.strip_prefix(&head) else {
            continue;
        };
        insert_path(&mut root, rest, value.clone());
    }
    root
}

/// Inserts `value` at the dotted `path` inside `table`, creating
/// intermediate tables as needed.
fn insert_path(table: &mut toml::Table, path: &str, value: toml::Value) {
    match path.split_once('.') {
        None => {
            table.insert(path.to_string(), value);
        }
        Some((head, rest)) => {
            let entry = table
                .entry(head.to_string())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if let toml::Value::Table(nested) = entry {
                insert_path(nested, rest, value);
            }
            // A leaf value already occupies `head`. The flattened map came
            // from `flatten`, which never produces that shape, so this
            // branch only matters for a hand-built map; keep the existing
            // leaf rather than panic.
        }
    }
}

/// Parses a raw string, from an environment variable or a command-line
/// flag, into a TOML value.
///
/// Tries an integer, then a float, then the TOML booleans `true` and
/// `false` (lower case only, matching the TOML spec), and falls back to a
/// plain string. This is a heuristic, not a TOML parse: an environment
/// variable or a flag carries a raw string with no quoting to mark it as
/// text, unlike a TOML file.
pub(crate) fn parse_scalar(raw: &str) -> toml::Value {
    if let Ok(int) = raw.parse::<i64>() {
        return toml::Value::Integer(int);
    }
    if let Ok(float) = raw.parse::<f64>() {
        return toml::Value::Float(float);
    }
    match raw {
        "true" => toml::Value::Boolean(true),
        "false" => toml::Value::Boolean(false),
        _ => toml::Value::String(raw.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_joins_nested_tables_with_dots() {
        let table: toml::Table = toml::from_str(
            "[policy]\nwrite = \"confirm\"\nread = \"allow\"\n[hardware]\nmemory_budget_gb = 8.0\n",
        )
        .unwrap();
        let mut out = BTreeMap::new();
        flatten(&table, "", &mut out);
        assert_eq!(
            out.get("policy.write").and_then(toml::Value::as_str),
            Some("confirm")
        );
        assert_eq!(
            out.get("policy.read").and_then(toml::Value::as_str),
            Some("allow")
        );
        assert_eq!(
            out.get("hardware.memory_budget_gb")
                .and_then(toml::Value::as_float),
            Some(8.0)
        );
    }

    #[test]
    fn flatten_treats_an_array_as_one_leaf() {
        let table: toml::Table = toml::from_str("tags = [\"a\", \"b\"]\n").unwrap();
        let mut out = BTreeMap::new();
        flatten(&table, "", &mut out);
        assert!(out.get("tags").unwrap().is_array());
        assert!(!out.contains_key("tags.0"));
    }

    #[test]
    fn unflatten_section_strips_the_prefix_and_ignores_other_keys() {
        let mut values = BTreeMap::new();
        values.insert(
            "policy.write".to_string(),
            toml::Value::String("confirm".into()),
        );
        values.insert(
            "policy.read".to_string(),
            toml::Value::String("allow".into()),
        );
        values.insert("unrelated.value".to_string(), toml::Value::Integer(1));

        let table = unflatten_section(&values, "policy");
        assert_eq!(
            table.get("write").and_then(toml::Value::as_str),
            Some("confirm")
        );
        assert_eq!(
            table.get("read").and_then(toml::Value::as_str),
            Some("allow")
        );
        assert!(!table.contains_key("unrelated"));
    }

    #[test]
    fn parse_scalar_reads_bool_int_float_and_string() {
        assert_eq!(parse_scalar("true"), toml::Value::Boolean(true));
        assert_eq!(parse_scalar("false"), toml::Value::Boolean(false));
        assert_eq!(parse_scalar("42"), toml::Value::Integer(42));
        assert_eq!(parse_scalar("3.5"), toml::Value::Float(3.5));
        assert_eq!(
            parse_scalar("confirm"),
            toml::Value::String("confirm".to_string())
        );
    }
}
