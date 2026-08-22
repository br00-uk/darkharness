//! The `[policy]` configuration table.
//!
//! [`PolicyConfig`] is the typed form of the configuration that task unit
//! `A4` names:
//!
//! ```toml
//! [policy]
//! read  = "allow"
//! write = "confirm"          # allow | confirm | deny
//! exec  = "confirm"
//! write_outside_root = "deny"
//! default_dark = false
//! ```

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One of the three settings a policy value can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyValue {
    /// Run the action without asking.
    Allow,
    /// Ask a person, or the `--yes` flag in headless mode. See
    /// [`super::Confirmer`].
    Confirm,
    /// Refuse the action.
    Deny,
}

impl PolicyValue {
    /// Returns the strictness rank. `Allow` is loosest, `Deny` is strictest.
    fn rank(self) -> u8 {
        match self {
            Self::Allow => 0,
            Self::Confirm => 1,
            Self::Deny => 2,
        }
    }

    /// Returns `true` when `self` is at least as restrictive as `other`.
    ///
    /// A repository configuration file may only replace a policy value with
    /// one that is at least as strict as the value it replaces. See Rule 35.
    /// This method only exposes the comparison; applying the narrowing rule
    /// to the AGENTS.md machine-readable block is task unit `K2`'s work.
    pub fn is_at_least_as_strict_as(self, other: Self) -> bool {
        self.rank() >= other.rank()
    }
}

/// The permission for a write outside the repository root.
///
/// This type holds exactly one value, [`WriteOutsideRoot::DENIED`]. Its
/// [`Deserialize`] implementation parses the configured string as a
/// [`PolicyValue`] first, so a malformed value such as `"maybe"` is still
/// rejected, and then discards the parsed result. No configuration, not even
/// one that writes `write_outside_root = "allow"`, can produce anything but
/// `DENIED`. There is no code path, in this crate or any other, that can read
/// a non-denied value out of this type. See Rule 34.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOutsideRoot;

impl WriteOutsideRoot {
    /// The only value this type can hold.
    pub const DENIED: Self = Self;
}

impl Default for WriteOutsideRoot {
    fn default() -> Self {
        Self::DENIED
    }
}

impl<'de> Deserialize<'de> for WriteOutsideRoot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Validate the shape of the configured value, then throw it away.
        // Every well-formed input, "allow" included, becomes `DENIED`.
        PolicyValue::deserialize(deserializer)?;
        Ok(Self::DENIED)
    }
}

impl Serialize for WriteOutsideRoot {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        PolicyValue::Deny.serialize(serializer)
    }
}

/// The typed `[policy]` configuration table.
///
/// Deserializing a value with a key missing fills that key from
/// [`PolicyConfig::default`], so a repository can set only the keys it cares
/// about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PolicyConfig {
    /// The permission for a read action. Defaults to [`PolicyValue::Allow`].
    pub read: PolicyValue,
    /// The permission for a write action. Defaults to [`PolicyValue::Confirm`].
    pub write: PolicyValue,
    /// The permission for a command execution. Defaults to [`PolicyValue::Confirm`].
    pub exec: PolicyValue,
    /// Always [`WriteOutsideRoot::DENIED`]. See Rule 34.
    pub write_outside_root: WriteOutsideRoot,
    /// Whether a new session starts in dark mode.
    pub default_dark: bool,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            read: PolicyValue::Allow,
            write: PolicyValue::Confirm,
            exec: PolicyValue::Confirm,
            write_outside_root: WriteOutsideRoot::DENIED,
            default_dark: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_task_unit_configuration() {
        let config = PolicyConfig::default();
        assert_eq!(config.read, PolicyValue::Allow);
        assert_eq!(config.write, PolicyValue::Confirm);
        assert_eq!(config.exec, PolicyValue::Confirm);
        assert_eq!(config.write_outside_root, WriteOutsideRoot::DENIED);
        assert!(!config.default_dark);
    }

    #[test]
    fn missing_keys_fall_back_to_the_default() {
        let config: PolicyConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config, PolicyConfig::default());
    }

    #[test]
    fn a_full_configuration_round_trips() {
        let json = r#"{
            "read": "allow",
            "write": "deny",
            "exec": "confirm",
            "write_outside_root": "deny",
            "default_dark": true
        }"#;
        let config: PolicyConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.read, PolicyValue::Allow);
        assert_eq!(config.write, PolicyValue::Deny);
        assert_eq!(config.exec, PolicyValue::Confirm);
        assert!(config.default_dark);
    }

    #[test]
    fn write_outside_root_cannot_be_set_to_allow() {
        // This is the single most important test in this module: even a
        // config file that explicitly asks to widen this permission cannot.
        let json = r#"{"write_outside_root": "allow"}"#;
        let config: PolicyConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.write_outside_root, WriteOutsideRoot::DENIED);
    }

    #[test]
    fn write_outside_root_cannot_be_set_to_confirm() {
        let json = r#"{"write_outside_root": "confirm"}"#;
        let config: PolicyConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.write_outside_root, WriteOutsideRoot::DENIED);
    }

    #[test]
    fn write_outside_root_still_rejects_a_malformed_value() {
        // The marker type validates shape even though it discards meaning.
        let json = r#"{"write_outside_root": "sometimes"}"#;
        let result: Result<PolicyConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn there_is_no_way_to_construct_a_write_outside_root_other_than_denied() {
        // WriteOutsideRoot has one field-less value. Every possible value of
        // the type, not just every value serde can produce, is DENIED.
        assert_eq!(WriteOutsideRoot, WriteOutsideRoot::DENIED);
    }

    #[test]
    fn policy_value_strictness_orders_deny_over_confirm_over_allow() {
        assert!(PolicyValue::Deny.is_at_least_as_strict_as(PolicyValue::Confirm));
        assert!(PolicyValue::Deny.is_at_least_as_strict_as(PolicyValue::Allow));
        assert!(PolicyValue::Confirm.is_at_least_as_strict_as(PolicyValue::Allow));
        assert!(!PolicyValue::Allow.is_at_least_as_strict_as(PolicyValue::Confirm));
        assert!(!PolicyValue::Confirm.is_at_least_as_strict_as(PolicyValue::Deny));
        // Equal strictness satisfies "at least as strict".
        assert!(PolicyValue::Confirm.is_at_least_as_strict_as(PolicyValue::Confirm));
    }

    #[test]
    fn policy_value_serialises_as_a_lowercase_string() {
        assert_eq!(
            serde_json::to_string(&PolicyValue::Allow).unwrap(),
            "\"allow\""
        );
        assert_eq!(
            serde_json::to_string(&PolicyValue::Confirm).unwrap(),
            "\"confirm\""
        );
        assert_eq!(
            serde_json::to_string(&PolicyValue::Deny).unwrap(),
            "\"deny\""
        );
    }
}
