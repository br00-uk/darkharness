//! JSON schema validation and value-level repair for tool arguments.
//!
//! Validates a call's `arguments` object against its [`ToolSchema::parameters`].
//! On a type mismatch, tries the two value-level repairs the build
//! specification allows — a string-to-boolean or string-to-number
//! coercion, and filling an omitted optional field with its schema
//! default — before giving up and naming the field. A required field that
//! is missing is never filled in: see [`FieldProblem::Missing`] and task
//! unit `I3`, step 7.

use dark_contract::ToolSchema;

use super::repair::TextRepair;

/// One repair applied to an argument value, logged in the order it fired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueRepair {
    /// Converted a string value to the type its schema entry names.
    CoercedType {
        /// The argument name.
        field: String,
        /// The JSON type the value arrived as.
        from: &'static str,
        /// The JSON type the schema names.
        to: String,
    },
    /// Filled an omitted, non-required field with its schema default.
    FilledDefault {
        /// The argument name.
        field: String,
    },
}

/// Why validation could not accept a call, once repair has been tried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldProblem {
    /// A required field from the schema is absent from the arguments.
    ///
    /// The harness never invents a value for a required field. See task
    /// unit `I3`, step 7.
    Missing {
        /// The argument name.
        field: String,
        /// The JSON type the schema names.
        expected: String,
    },
    /// A field is present but its value does not match its schema type,
    /// and no repair closed the gap.
    WrongType {
        /// The argument name.
        field: String,
        /// The JSON type the schema names.
        expected: String,
        /// The JSON type the value actually has.
        found: &'static str,
    },
    /// The `arguments` value is not a JSON object at all.
    NotAnObject,
}

impl FieldProblem {
    /// Builds the message a `Role::Tool` reply sends back to the model.
    ///
    /// Names the field and states the expected type, so a small model can
    /// recover on its next attempt. See task unit `I3`, step 5.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Missing { field, expected } => format!(
                "The tool call is missing the required argument `{field}`. Add `{field}` as a {expected}."
            ),
            Self::WrongType {
                field,
                expected,
                found,
            } => format!(
                "The argument `{field}` must be a {expected}. The call sent a {found} instead."
            ),
            Self::NotAnObject => {
                "The tool call's `arguments` field must be a JSON object.".to_owned()
            }
        }
    }
}

/// Returns the JSON type name that `dark_contract`'s JSON schema convention
/// uses, for a parsed value.
fn value_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Checks whether `value` already matches the JSON Schema `type` keyword.
///
/// `integer` accepts any whole number; JSON Schema's `number` accepts both.
fn type_matches(value: &serde_json::Value, expected: &str) -> bool {
    match expected {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        // An unrecognised or absent type keyword constrains nothing.
        _ => true,
    }
}

/// Tries to coerce a string value into the type the schema names.
///
/// This is the repair from task unit `I3`, step 6, item 3: `"true"` becomes
/// `true`, and a numeric string becomes a number, for a field whose schema
/// says it is numeric.
fn coerce(value: &serde_json::Value, expected: &str) -> Option<serde_json::Value> {
    let text = value.as_str()?;
    match expected {
        "boolean" => match text {
            "true" => Some(serde_json::Value::Bool(true)),
            "false" => Some(serde_json::Value::Bool(false)),
            _ => None,
        },
        "number" => text
            .parse::<f64>()
            .ok()
            .and_then(|n| serde_json::Number::from_f64(n).map(serde_json::Value::Number)),
        "integer" => text.parse::<i64>().ok().map(serde_json::Value::from),
        _ => None,
    }
}

/// Validates `arguments` against `schema.parameters`, repairing what task
/// unit `I3` allows repaired and reporting the first field it cannot.
///
/// A schema with no `properties` keyword constrains nothing, and the
/// arguments pass unchanged.
#[must_use]
pub(crate) fn validate_and_repair(
    arguments: serde_json::Value,
    schema: &ToolSchema,
) -> (serde_json::Value, Vec<ValueRepair>, Option<FieldProblem>) {
    let Some(properties) = schema
        .parameters
        .get("properties")
        .and_then(|v| v.as_object())
    else {
        return (arguments, Vec::new(), None);
    };

    let required: Vec<&str> = schema
        .parameters
        .get("required")
        .and_then(|v| v.as_array())
        .map(|items| items.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_default();

    let serde_json::Value::Object(mut object) = arguments else {
        return (
            arguments_placeholder(),
            Vec::new(),
            Some(FieldProblem::NotAnObject),
        );
    };

    let mut repairs = Vec::new();

    for (field, field_schema) in properties {
        let expected = field_schema.get("type").and_then(|v| v.as_str());

        match object.get(field).cloned() {
            Some(value) => {
                let Some(expected) = expected else { continue };
                if type_matches(&value, expected) {
                    continue;
                }
                match coerce(&value, expected) {
                    Some(coerced) => {
                        repairs.push(ValueRepair::CoercedType {
                            field: field.clone(),
                            from: value_type_name(&value),
                            to: expected.to_owned(),
                        });
                        object.insert(field.clone(), coerced);
                    }
                    None => {
                        return (
                            serde_json::Value::Object(object),
                            repairs,
                            Some(FieldProblem::WrongType {
                                field: field.clone(),
                                expected: expected.to_owned(),
                                found: value_type_name(&value),
                            }),
                        );
                    }
                }
            }
            None if required.contains(&field.as_str()) => {
                // Never invent a required field. Report it instead.
                return (
                    serde_json::Value::Object(object),
                    repairs,
                    Some(FieldProblem::Missing {
                        field: field.clone(),
                        expected: expected.unwrap_or("value").to_owned(),
                    }),
                );
            }
            None => {
                if let Some(default) = field_schema.get("default") {
                    object.insert(field.clone(), default.clone());
                    repairs.push(ValueRepair::FilledDefault {
                        field: field.clone(),
                    });
                }
            }
        }
    }

    (serde_json::Value::Object(object), repairs, None)
}

/// A stable placeholder returned alongside [`FieldProblem::NotAnObject`],
/// so every return path yields a JSON value.
fn arguments_placeholder() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// Describes one text-level repair in one line, for a log or a repair list.
#[must_use]
pub(crate) fn describe_text_repair(repair: &TextRepair) -> String {
    match repair {
        TextRepair::StrippedCodeFence => "stripped a Markdown code fence".to_owned(),
        TextRepair::UnescapedDoubleEncoding => "unescaped a double-encoded body".to_owned(),
    }
}

/// Describes one value-level repair in one line, for a log or a repair list.
#[must_use]
pub(crate) fn describe_value_repair(repair: &ValueRepair) -> String {
    match repair {
        ValueRepair::CoercedType { field, from, to } => {
            format!("coerced `{field}` from {from} to {to}")
        }
        ValueRepair::FilledDefault { field } => format!("filled default for `{field}`"),
    }
}

/// Merges the text-level repairs from [`super::repair::strip_and_parse`]
/// and the value-level repairs from [`validate_and_repair`] into one log
/// line's worth of detail, in application order.
#[must_use]
pub fn describe_repairs(text_repairs: &[TextRepair], value_repairs: &[ValueRepair]) -> String {
    text_repairs
        .iter()
        .map(describe_text_repair)
        .chain(value_repairs.iter().map(describe_value_repair))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> ToolSchema {
        ToolSchema {
            name: "read_file".to_owned(),
            description: "Reads a file.".to_owned(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "limit": {"type": "integer"},
                    "verbose": {"type": "boolean"},
                    "encoding": {"type": "string", "default": "utf-8"}
                },
                "required": ["path"]
            }),
            tier: 1,
            mutating: false,
        }
    }

    #[test]
    fn valid_arguments_pass_unchanged() {
        // Every optional field is present, encoding included, so nothing
        // needs a default filled and the arguments come back untouched.
        let args = serde_json::json!({
            "path": "a.rs",
            "limit": 10,
            "verbose": true,
            "encoding": "utf-8"
        });
        let (value, repairs, problem) = validate_and_repair(args.clone(), &schema());
        assert_eq!(value, args);
        assert!(repairs.is_empty());
        assert!(problem.is_none());
    }

    #[test]
    fn a_missing_required_field_is_named_with_its_type_and_never_invented() {
        let args = serde_json::json!({"limit": 10});
        let (_, _, problem) = validate_and_repair(args, &schema());
        let problem = problem.expect("path is required");
        assert_eq!(
            problem,
            FieldProblem::Missing {
                field: "path".to_owned(),
                expected: "string".to_owned()
            }
        );
        assert!(problem.message().contains("path"));
        assert!(problem.message().contains("string"));
    }

    #[test]
    fn a_missing_optional_field_with_a_default_is_filled() {
        let args = serde_json::json!({"path": "a.rs"});
        let (value, repairs, problem) = validate_and_repair(args, &schema());
        assert!(problem.is_none());
        assert_eq!(value["encoding"], "utf-8");
        assert_eq!(
            repairs,
            vec![ValueRepair::FilledDefault {
                field: "encoding".to_owned()
            }]
        );
    }

    #[test]
    fn a_missing_optional_field_with_no_default_is_left_absent() {
        let args = serde_json::json!({"path": "a.rs", "verbose": true});
        let (value, repairs, problem) = validate_and_repair(args, &schema());
        assert!(problem.is_none());
        assert!(value.get("limit").is_none());
        assert!(!repairs.iter().any(|r| matches!(
            r,
            ValueRepair::FilledDefault { field } if field == "limit"
        )));
    }

    #[test]
    fn a_stringly_typed_boolean_is_coerced() {
        // encoding is present too, so the only repair in play is the
        // coercion this test is about.
        let args = serde_json::json!({"path": "a.rs", "verbose": "true", "encoding": "utf-8"});
        let (value, repairs, problem) = validate_and_repair(args, &schema());
        assert!(problem.is_none());
        assert_eq!(value["verbose"], serde_json::json!(true));
        assert_eq!(
            repairs,
            vec![ValueRepair::CoercedType {
                field: "verbose".to_owned(),
                from: "string",
                to: "boolean".to_owned()
            }]
        );
    }

    #[test]
    fn a_numeric_string_is_coerced_for_a_numeric_field() {
        let args = serde_json::json!({"path": "a.rs", "limit": "42"});
        let (value, _, problem) = validate_and_repair(args, &schema());
        assert!(problem.is_none());
        assert_eq!(value["limit"], serde_json::json!(42));
    }

    #[test]
    fn an_uncoercible_type_mismatch_names_the_field_and_the_expected_type() {
        let args = serde_json::json!({"path": "a.rs", "limit": ["not", "a", "number"]});
        let (_, _, problem) = validate_and_repair(args, &schema());
        let problem = problem.expect("limit is the wrong type");
        assert_eq!(
            problem,
            FieldProblem::WrongType {
                field: "limit".to_owned(),
                expected: "integer".to_owned(),
                found: "array"
            }
        );
        let message = problem.message();
        assert!(message.contains("limit"), "{message}");
        assert!(message.contains("integer"), "{message}");
    }

    #[test]
    fn a_non_object_arguments_value_is_reported_not_panicked() {
        let (_, _, problem) = validate_and_repair(serde_json::json!("not an object"), &schema());
        assert_eq!(problem, Some(FieldProblem::NotAnObject));
    }

    #[test]
    fn describe_repairs_joins_both_kinds_in_order() {
        let text = vec![TextRepair::StrippedCodeFence];
        let value = vec![ValueRepair::FilledDefault {
            field: "encoding".to_owned(),
        }];
        let line = describe_repairs(&text, &value);
        assert_eq!(
            line,
            "stripped a Markdown code fence; filled default for `encoding`"
        );
    }
}
