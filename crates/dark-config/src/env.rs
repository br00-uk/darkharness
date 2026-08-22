//! Environment variables as a configuration source.

use std::collections::BTreeMap;

/// A snapshot of environment variables, keyed by name.
///
/// [`crate::resolve`] takes this instead of reading [`std::env`] itself, so
/// tests stay hermetic. `cargo nextest` runs tests in parallel inside one
/// process, and mutating the real process environment (`std::env::set_var`)
/// races across those threads; passing a snapshot avoids that entirely. The
/// caller (typically `dark-cli`, at startup) builds this once from
/// `std::env::vars()`.
pub type EnvMap = BTreeMap<String, String>;

/// The prefix that marks an environment variable as harness configuration.
pub const ENV_PREFIX: &str = "DARK_";

/// Returns the environment variable name that would set `dotted_key`.
///
/// This is the canonical form: upper-case the key and turn every `.` into
/// `_`. [`crate::resolve`] uses it to match a `DARK_`-prefixed variable
/// against every key that a lower layer (a default or a file) already
/// resolved, so a multi-word field name such as `memory_budget_gb` is not
/// misread as two nested sections.
pub(crate) fn canonical_env_name(dotted_key: &str) -> String {
    format!(
        "{ENV_PREFIX}{}",
        dotted_key.to_uppercase().replace('.', "_")
    )
}

/// Falls back to a naive `_` to `.` conversion when `name` matches no
/// already-known key.
///
/// This lets an environment variable introduce a key that no default or
/// file layer declared first, at the cost of being unable to tell a
/// section boundary from an underscore inside one field's own name.
/// Returns `None` when `name` does not carry the `DARK_` prefix, or when
/// nothing follows it.
pub(crate) fn fallback_dotted_key(name: &str) -> Option<String> {
    let rest = name.strip_prefix(ENV_PREFIX)?;
    if rest.is_empty() {
        return None;
    }
    Some(rest.to_lowercase().replace('_', "."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_env_name_upper_cases_and_joins_with_underscores() {
        assert_eq!(canonical_env_name("policy.write"), "DARK_POLICY_WRITE");
        assert_eq!(
            canonical_env_name("hardware.memory_budget_gb"),
            "DARK_HARDWARE_MEMORY_BUDGET_GB"
        );
    }

    #[test]
    fn fallback_dotted_key_treats_every_underscore_as_a_dot() {
        assert_eq!(
            fallback_dotted_key("DARK_VERBOSE"),
            Some("verbose".to_string())
        );
        assert_eq!(fallback_dotted_key("DARK_A_B_C"), Some("a.b.c".to_string()));
        assert_eq!(fallback_dotted_key("NOT_DARK_PREFIXED"), None);
        assert_eq!(fallback_dotted_key("DARK_"), None);
    }
}
