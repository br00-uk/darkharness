//! Parses and validates the arguments that a model supplies to `run_command`.

use dark_contract::{ErrCode, Error, Result};
use serde::Deserialize;
use serde_json::Value;

/// The timeout that applies when the caller gives none.
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// The validated arguments for one `run_command` call.
#[derive(Debug, Clone)]
pub(crate) struct ExecArgs {
    /// The command line, for example `"cargo test"`.
    pub command: String,
    /// The working directory, relative to the repository root.
    pub cwd: Option<String>,
    /// The timeout in seconds. Always at least 1.
    pub timeout_secs: u64,
    /// Whether the command runs through a shell.
    pub shell: bool,
}

/// The shape of the raw JSON arguments, before validation.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawArgs {
    /// The command line.
    command: String,
    /// The working directory.
    #[serde(default)]
    cwd: Option<String>,
    /// The timeout in seconds.
    #[serde(default)]
    timeout_secs: Option<u64>,
    /// Whether to use a shell.
    #[serde(default)]
    shell: bool,
}

impl ExecArgs {
    /// Parses and validates `value` against the `run_command` schema.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::ToolInvalidArgs`] when the JSON does not match the
    /// schema, when the command is empty or whitespace only, or when
    /// `timeout_secs` is zero.
    pub(crate) fn parse(value: Value) -> Result<Self> {
        let raw: RawArgs = serde_json::from_value(value).map_err(|e| {
            Error::new(
                ErrCode::ToolInvalidArgs,
                format!("run_command arguments do not match the schema: {e}"),
            )
        })?;

        let command = raw.command.trim();
        if command.is_empty() {
            return Err(Error::new(
                ErrCode::ToolInvalidArgs,
                "the command field must not be empty",
            ));
        }

        let timeout_secs = raw.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        if timeout_secs == 0 {
            return Err(Error::new(
                ErrCode::ToolInvalidArgs,
                "timeout_secs must be at least 1",
            ));
        }

        Ok(Self {
            command: command.to_owned(),
            cwd: raw.cwd,
            timeout_secs,
            shell: raw.shell,
        })
    }
}

/// Returns the JSON schema for the `run_command` arguments.
pub(crate) fn schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "The command line to run, for example \"cargo test\". The tool splits this into a program and its arguments. It does not use a shell unless shell is true."
            },
            "cwd": {
                "type": "string",
                "description": "The working directory. It is relative to the repository root. The default is the repository root."
            },
            "timeout_secs": {
                "type": "integer",
                "minimum": 1,
                "description": "The timeout in seconds. The default is 120."
            },
            "shell": {
                "type": "boolean",
                "description": "Runs the command through a shell. This needs a person present to confirm. The default is false."
            }
        },
        "required": ["command"],
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_TIMEOUT_SECS, ExecArgs};
    use dark_contract::ErrCode;
    use serde_json::json;

    #[test]
    fn a_minimal_call_fills_in_the_defaults() {
        let args = ExecArgs::parse(json!({"command": "cargo test"})).unwrap();
        assert_eq!(args.command, "cargo test");
        assert_eq!(args.cwd, None);
        assert_eq!(args.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert!(!args.shell);
    }

    #[test]
    fn every_field_is_honoured_when_present() {
        let args = ExecArgs::parse(json!({
            "command": "ls -la",
            "cwd": "crates/dark-tools",
            "timeout_secs": 5,
            "shell": true,
        }))
        .unwrap();
        assert_eq!(args.command, "ls -la");
        assert_eq!(args.cwd.as_deref(), Some("crates/dark-tools"));
        assert_eq!(args.timeout_secs, 5);
        assert!(args.shell);
    }

    #[test]
    fn a_missing_command_is_rejected() {
        let err = ExecArgs::parse(json!({})).unwrap_err();
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }

    #[test]
    fn a_blank_command_is_rejected() {
        let err = ExecArgs::parse(json!({"command": "   "})).unwrap_err();
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }

    #[test]
    fn a_command_is_trimmed() {
        let args = ExecArgs::parse(json!({"command": "  ls  "})).unwrap();
        assert_eq!(args.command, "ls");
    }

    #[test]
    fn a_zero_timeout_is_rejected() {
        let err = ExecArgs::parse(json!({"command": "ls", "timeout_secs": 0})).unwrap_err();
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }

    #[test]
    fn an_unknown_field_is_rejected() {
        let err = ExecArgs::parse(json!({"command": "ls", "surprise": 1})).unwrap_err();
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }

    #[test]
    fn a_wrong_type_is_rejected() {
        let err = ExecArgs::parse(json!({"command": 5})).unwrap_err();
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }

    #[test]
    fn the_schema_requires_only_the_command_field() {
        let schema = super::schema();
        assert_eq!(schema["required"], json!(["command"]));
        assert_eq!(schema["additionalProperties"], json!(false));
    }
}
