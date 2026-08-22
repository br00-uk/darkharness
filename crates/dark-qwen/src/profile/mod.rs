//! Model profiles for the Qwen family.
//!
//! A profile configures the harness for one model family and size. The
//! built-in table gives every supported Qwen identifier a profile.
//! Configuration can add profiles ahead of the built-in table, so a
//! configured match always wins over a built-in one. See task unit `I1`.
//!
//! A profile never stores a context length. The context length comes from
//! the loaded model's [`Caps`], read fresh on every turn. See
//! [`ProfileTable::resolve`] and PRD Section 4.1.

use dark_contract::{Caps, ErrCode, Error, Result, RoleClass, ThinkMode};
use serde::{Deserialize, Serialize};

/// The sampling and thinking settings for one micro-role.
///
/// A micro-role configures one resident model for one narrow purpose. It
/// never changes which model is loaded. See the glossary in PRD Section 2.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MicroRoleConfig {
    /// Whether this micro-role thinks before it answers.
    pub think: ThinkMode,
    /// The sampling temperature.
    pub temperature: f32,
    /// The nucleus sampling threshold.
    pub top_p: f32,
    /// Whether this micro-role constrains its output with a grammar.
    pub grammar: bool,
    /// The generation limit, when this micro-role sets one.
    pub max_tokens: Option<usize>,
}

/// The four micro-roles that every profile carries.
///
/// The build specification lists these values in task unit `I1`, step 2.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MicroRoles {
    /// Reasons about a plan. Thinking is on. Grammar is off.
    pub deliberate: MicroRoleConfig,
    /// Pulls one fact out of text. Thinking is off. Grammar is on.
    pub extract: MicroRoleConfig,
    /// Chooses one label from a fixed set. Thinking is off. Grammar is on.
    pub classify: MicroRoleConfig,
    /// Narrates a result in prose. Thinking is off. Grammar is off.
    pub narrate: MicroRoleConfig,
}

impl Default for MicroRoles {
    fn default() -> Self {
        Self {
            deliberate: MicroRoleConfig {
                think: ThinkMode::On,
                temperature: 0.6,
                top_p: 0.95,
                grammar: false,
                max_tokens: None,
            },
            extract: MicroRoleConfig {
                think: ThinkMode::Off,
                temperature: 0.2,
                top_p: 0.8,
                grammar: true,
                max_tokens: Some(1200),
            },
            classify: MicroRoleConfig {
                think: ThinkMode::Off,
                temperature: 0.0,
                top_p: 0.8,
                grammar: true,
                max_tokens: Some(64),
            },
            narrate: MicroRoleConfig {
                think: ThinkMode::Off,
                temperature: 0.4,
                top_p: 0.8,
                grammar: false,
                max_tokens: Some(200),
            },
        }
    }
}

/// The configuration for one model family and size.
///
/// [`ProfileTable::match_profile`] chooses a profile by matching
/// [`Profile::match_pattern`] against the loaded model's identifier. The
/// pattern never encodes a context length; read that from [`Caps`] instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// A pipe-separated list of alternatives. Each alternative is a literal
    /// substring of the model identifier, or a literal prefix followed by
    /// `*`. The profile matches when any alternative matches.
    #[serde(rename = "match")]
    pub match_pattern: String,
    /// The role class that a model with this profile serves.
    pub role: RoleClass,
    /// 1 essential, 2 standard, 3 advanced. See `dark_contract::tool::tier`.
    pub tool_tier: u8,
    /// The largest number of tools to show the model in one request.
    ///
    /// `None` means the harness sets no extra limit beyond the tier.
    #[serde(default)]
    pub max_tools: Option<usize>,
    /// The model may call at most one tool in a turn.
    #[serde(default)]
    pub one_tool_per_turn: bool,
    /// The thinking mode to use when the caller asks for [`ThinkMode::Auto`]
    /// and gives no turn purpose. Task unit `I2` refines this per turn.
    pub think_default: ThinkMode,
    /// Grammar-constrained decoding is mandatory for this profile.
    #[serde(default)]
    pub force_grammar: bool,
    /// The token budget for a map digest in the context prefix.
    ///
    /// `None` means this profile sets no digest at all.
    #[serde(default)]
    pub digest_budget: Option<usize>,
    /// The model may chart a map. See [`Profile::authorize_charting`].
    pub allow_charting: bool,
    /// The four micro-roles that this profile configures.
    #[serde(default)]
    pub roles: MicroRoles,
}

impl Profile {
    /// Checks whether this profile allows the model to chart a map.
    ///
    /// A small model produces a shallow, low-quality map. Refusing here
    /// keeps that failure out of the map store. See task unit `I1`, step 3.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineUnsupported`] when [`Profile::allow_charting`]
    /// is false.
    pub fn authorize_charting(&self, model_id: &str) -> Result<()> {
        if self.allow_charting {
            return Ok(());
        }
        Err(Error::new(
            ErrCode::EngineUnsupported,
            format!("{model_id} cannot chart a map; its profile sets allow_charting to false"),
        )
        .with_remedy("Use a 14B or larger worker model, or an architect model, to chart a map."))
    }
}

/// One alternative inside a [`Profile::match_pattern`].
enum Alternative<'a> {
    /// The alternative must appear as a literal substring.
    Literal(&'a str),
    /// The alternative must appear as a literal prefix, followed by anything.
    Prefix(&'a str),
}

impl<'a> Alternative<'a> {
    fn parse(text: &'a str) -> Self {
        let text = text.trim();
        match text.strip_suffix('*') {
            Some(prefix) => Self::Prefix(prefix),
            None => Self::Literal(text),
        }
    }

    fn matches(&self, model_id: &str) -> bool {
        match self {
            Self::Literal(pattern) => model_id.contains(pattern),
            Self::Prefix(prefix) => model_id.contains(prefix),
        }
    }
}

/// Checks whether `model_id` matches any alternative in `pattern`.
fn pattern_matches(pattern: &str, model_id: &str) -> bool {
    pattern
        .split('|')
        .map(Alternative::parse)
        .any(|alt| alt.matches(model_id))
}

/// A profile bound to the capabilities of the model that is actually loaded.
///
/// The context limits come from `caps`, never from the profile table. See
/// PRD Section 4.1, Rule 4.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedProfile {
    /// The model identifier that this resolution matched.
    pub model_id: String,
    /// The matched profile.
    pub profile: Profile,
    /// The context length the resident set manager grants now.
    ///
    /// Budget against this field, never [`ResolvedProfile::max_context`].
    pub granted_context: usize,
    /// The context length the model supports at full grant.
    pub max_context: usize,
}

impl ResolvedProfile {
    /// Checks whether the resolved model may chart a map.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineUnsupported`] when the profile refuses.
    pub fn authorize_charting(&self) -> Result<()> {
        self.profile.authorize_charting(&self.model_id)
    }
}

/// The ordered set of profiles that [`ProfileTable::match_profile`] searches.
///
/// The first matching profile wins, so an override placed ahead of the
/// built-in table always takes precedence over it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileTable {
    profiles: Vec<Profile>,
}

/// The raw form of one `[[qwen.profile]]` table in configuration.
///
/// This mirrors [`Profile`] but carries no micro-roles: the four micro-roles
/// come from the separate `[plan.roles.*]` tables and are the same across
/// every profile, built-in or configured.
#[derive(Debug, Clone, Deserialize)]
struct ProfileSpec {
    #[serde(rename = "match")]
    match_pattern: String,
    role: RoleClass,
    tool_tier: u8,
    #[serde(default)]
    max_tools: Option<usize>,
    #[serde(default)]
    one_tool_per_turn: bool,
    think_default: ThinkMode,
    #[serde(default)]
    force_grammar: bool,
    #[serde(default)]
    digest_budget: Option<usize>,
    allow_charting: bool,
}

impl ProfileSpec {
    fn into_profile(self, roles: MicroRoles) -> Profile {
        Profile {
            match_pattern: self.match_pattern,
            role: self.role,
            tool_tier: self.tool_tier,
            max_tools: self.max_tools,
            one_tool_per_turn: self.one_tool_per_turn,
            think_default: self.think_default,
            force_grammar: self.force_grammar,
            digest_budget: self.digest_budget,
            allow_charting: self.allow_charting,
            roles,
        }
    }
}

/// The `[plan.roles.*]` section of configuration.
///
/// Every field is optional. A missing micro-role keeps the built-in default
/// for that micro-role.
#[derive(Debug, Clone, Default, Deserialize)]
struct MicroRolesSpec {
    #[serde(default)]
    deliberate: Option<MicroRoleConfig>,
    #[serde(default)]
    extract: Option<MicroRoleConfig>,
    #[serde(default)]
    classify: Option<MicroRoleConfig>,
    #[serde(default)]
    narrate: Option<MicroRoleConfig>,
}

impl MicroRolesSpec {
    fn apply(self, base: MicroRoles) -> MicroRoles {
        MicroRoles {
            deliberate: self.deliberate.unwrap_or(base.deliberate),
            extract: self.extract.unwrap_or(base.extract),
            classify: self.classify.unwrap_or(base.classify),
            narrate: self.narrate.unwrap_or(base.narrate),
        }
    }
}

/// The top level of a `qwen.profile` / `plan.roles` configuration document.
#[derive(Debug, Clone, Default, Deserialize)]
struct ConfigDocument {
    #[serde(default)]
    qwen: QwenSection,
    #[serde(default)]
    plan: PlanSection,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct QwenSection {
    #[serde(default)]
    profile: Vec<ProfileSpec>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PlanSection {
    #[serde(default)]
    roles: MicroRolesSpec,
}

impl ProfileTable {
    /// Builds the table from the four built-in profiles.
    ///
    /// The values match task unit `I1`, step 1. Configuration may override
    /// or extend this table; see [`ProfileTable::from_toml`].
    #[must_use]
    pub fn builtin() -> Self {
        let roles = MicroRoles::default();
        Self {
            profiles: vec![
                Profile {
                    match_pattern: "Qwen3-0.6B|Qwen3-1.7B".to_owned(),
                    role: RoleClass::Scout,
                    tool_tier: 1,
                    max_tools: Some(5),
                    one_tool_per_turn: true,
                    think_default: ThinkMode::Off,
                    force_grammar: true,
                    digest_budget: Some(600),
                    allow_charting: false,
                    roles,
                },
                Profile {
                    match_pattern: "Qwen3-4B|Qwen3-8B".to_owned(),
                    role: RoleClass::Worker,
                    tool_tier: 1,
                    max_tools: Some(8),
                    one_tool_per_turn: false,
                    think_default: ThinkMode::Auto,
                    force_grammar: true,
                    digest_budget: None,
                    allow_charting: false,
                    roles,
                },
                Profile {
                    match_pattern: "Qwen3-14B|Qwen3-32B|Qwen3-Coder-30B-A3B".to_owned(),
                    role: RoleClass::Worker,
                    tool_tier: 2,
                    max_tools: None,
                    one_tool_per_turn: false,
                    think_default: ThinkMode::Auto,
                    force_grammar: false,
                    digest_budget: None,
                    allow_charting: true,
                    roles,
                },
                Profile {
                    match_pattern: "Qwen3.5-*".to_owned(),
                    role: RoleClass::Architect,
                    tool_tier: 3,
                    max_tools: None,
                    one_tool_per_turn: false,
                    think_default: ThinkMode::On,
                    force_grammar: false,
                    digest_budget: None,
                    allow_charting: true,
                    roles,
                },
            ],
        }
    }

    /// Builds the table from configuration text, layered over the built-in
    /// profiles.
    ///
    /// Configured profiles come first, so they take precedence over the
    /// built-in table in [`ProfileTable::match_profile`]. A `[plan.roles.*]`
    /// table overrides the matching micro-role for every profile, built-in
    /// profiles included.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineLoad`] when `text` is not valid TOML or does
    /// not match the configuration schema.
    pub fn from_toml(text: &str) -> Result<Self> {
        let document: ConfigDocument = toml::from_str(text).map_err(|err| {
            Error::new(
                ErrCode::EngineLoad,
                format!("invalid Qwen profile configuration: {err}"),
            )
            .with_remedy("Check the [[qwen.profile]] and [plan.roles] tables.")
        })?;

        let roles = document.plan.roles.apply(MicroRoles::default());
        let mut table = Self::builtin();
        for profile in &mut table.profiles {
            profile.roles = roles;
        }

        let overrides: Vec<Profile> = document
            .qwen
            .profile
            .into_iter()
            .map(|spec| spec.into_profile(roles))
            .collect();

        table.profiles.splice(0..0, overrides);
        Ok(table)
    }

    /// Returns the profile whose `match` pattern matches `model_id`.
    ///
    /// The first matching profile wins. See [`ProfileTable`].
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineLoad`] when no profile in the table matches.
    pub fn match_profile(&self, model_id: &str) -> Result<&Profile> {
        self.profiles
            .iter()
            .find(|profile| pattern_matches(&profile.match_pattern, model_id))
            .ok_or_else(|| {
                Error::new(
                    ErrCode::EngineLoad,
                    format!("no Qwen profile matches model identifier {model_id:?}"),
                )
                .with_remedy("Add a [[qwen.profile]] entry whose match pattern covers this model.")
            })
    }

    /// Matches `caps.model_id` and binds the result to `caps`'s live
    /// context limits.
    ///
    /// The returned [`ResolvedProfile`] reads its context limits from `caps`
    /// on every call, so a profile can never hard-code a context length.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineLoad`] when no profile matches `caps.model_id`.
    pub fn resolve(&self, caps: &Caps) -> Result<ResolvedProfile> {
        let profile = self.match_profile(&caps.model_id)?.clone();
        Ok(ResolvedProfile {
            model_id: caps.model_id.clone(),
            profile,
            granted_context: caps.granted_context,
            max_context: caps.max_context,
        })
    }

    /// Returns the profiles in match order, configured overrides first.
    #[must_use]
    pub fn profiles(&self) -> &[Profile] {
        &self.profiles
    }
}

impl Default for ProfileTable {
    fn default() -> Self {
        Self::builtin()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]
    use super::*;

    fn caps_for(model_id: &str, granted_context: usize) -> Caps {
        Caps {
            model_id: model_id.to_owned(),
            max_context: granted_context,
            granted_context,
            native_tools: false,
            thinking: true,
            grammar: true,
            vision: false,
            logprobs: false,
            params_b: 4.0,
            quant: "q4k".to_owned(),
            device: dark_contract::Device::Cpu,
            measured_tok_s: None,
        }
    }

    #[test]
    fn every_documented_identifier_resolves_to_exactly_one_profile() {
        let table = ProfileTable::builtin();
        let cases = [
            ("Qwen/Qwen3-0.6B", RoleClass::Scout),
            ("Qwen/Qwen3-1.7B", RoleClass::Scout),
            ("Qwen/Qwen3-4B", RoleClass::Worker),
            ("Qwen/Qwen3-8B", RoleClass::Worker),
            ("Qwen/Qwen3-14B", RoleClass::Worker),
            ("Qwen/Qwen3-32B", RoleClass::Worker),
            ("Qwen/Qwen3-Coder-30B-A3B", RoleClass::Worker),
            ("Qwen/Qwen3.5-7B", RoleClass::Architect),
            ("Qwen/Qwen3.5-72B", RoleClass::Architect),
        ];

        for (model_id, expected_role) in cases {
            let matches: Vec<&Profile> = table
                .profiles()
                .iter()
                .filter(|profile| pattern_matches(&profile.match_pattern, model_id))
                .collect();
            assert_eq!(
                matches.len(),
                1,
                "{model_id} matched {} profiles, want exactly one",
                matches.len()
            );
            assert_eq!(matches[0].role, expected_role, "wrong role for {model_id}");

            let resolved = table
                .match_profile(model_id)
                .unwrap_or_else(|_| panic!("{model_id} should resolve"));
            assert_eq!(resolved.role, expected_role);
        }
    }

    #[test]
    fn the_qwen35_wildcard_does_not_match_a_qwen3_point_release() {
        // Qwen3.5 must never fall through to a Qwen3-14B-style pattern: the
        // literal alternatives are substrings, and "Qwen3.5-7B" does not
        // contain "Qwen3-14B", "Qwen3-32B", or "Qwen3-Coder-30B-A3B".
        assert!(!pattern_matches(
            "Qwen3-14B|Qwen3-32B|Qwen3-Coder-30B-A3B",
            "Qwen3.5-7B"
        ));
        assert!(pattern_matches("Qwen3.5-*", "Qwen3.5-7B"));
        assert!(pattern_matches("Qwen3.5-*", "Qwen/Qwen3.5-72B-Instruct"));
    }

    #[test]
    fn match_precedence_favours_the_first_entry_in_the_table() {
        // Two profiles whose patterns both match the same identifier: the
        // table takes the first one, regardless of specificity.
        let roles = MicroRoles::default();
        let first = Profile {
            match_pattern: "Qwen3-4B".to_owned(),
            role: RoleClass::Scout,
            tool_tier: 1,
            max_tools: None,
            one_tool_per_turn: false,
            think_default: ThinkMode::Off,
            force_grammar: false,
            digest_budget: None,
            allow_charting: false,
            roles,
        };
        let mut second = first.clone();
        second.role = RoleClass::Worker;
        let table = ProfileTable {
            profiles: vec![first, second],
        };

        let matched = table.match_profile("Qwen/Qwen3-4B").expect("matches");
        assert_eq!(matched.role, RoleClass::Scout);
    }

    #[test]
    fn a_configured_profile_takes_precedence_over_the_builtin_table() {
        let table = ProfileTable::from_toml(
            r#"
            [[qwen.profile]]
            match = "Qwen3-4B"
            role = "architect"
            tool_tier = 3
            think_default = "on"
            allow_charting = true
            "#,
        )
        .expect("valid configuration");

        let matched = table.match_profile("Qwen/Qwen3-4B").expect("matches");
        assert_eq!(matched.role, RoleClass::Architect);
        assert!(matched.allow_charting);
    }

    #[test]
    fn plan_roles_override_every_profile_built_in_and_configured() {
        let table = ProfileTable::from_toml(
            r#"
            [plan.roles.classify]
            think = "off"
            temperature = 0.1
            top_p = 0.8
            grammar = true
            max_tokens = 32
            "#,
        )
        .expect("valid configuration");

        for profile in table.profiles() {
            assert_eq!(profile.roles.classify.temperature, 0.1);
            assert_eq!(profile.roles.classify.max_tokens, Some(32));
            // Unrelated micro-roles keep the built-in default.
            assert_eq!(profile.roles.narrate.temperature, 0.4);
        }
    }

    #[test]
    fn an_unknown_identifier_is_refused_with_a_remedy() {
        let table = ProfileTable::builtin();
        let err = table
            .match_profile("Llama-3-8B")
            .expect_err("must not resolve");
        assert_eq!(err.code, ErrCode::EngineLoad);
        assert!(err.remedy.is_some());
    }

    #[test]
    fn a_small_model_refuses_to_chart() {
        let table = ProfileTable::builtin();
        let profile = table.match_profile("Qwen/Qwen3-4B").expect("matches");
        let err = profile
            .authorize_charting("Qwen/Qwen3-4B")
            .expect_err("a 4B model must not chart a map");
        assert_eq!(err.code, ErrCode::EngineUnsupported);
        assert!(err.message.contains("Qwen/Qwen3-4B"));
    }

    #[test]
    fn a_large_model_may_chart() {
        let table = ProfileTable::builtin();
        let profile = table.match_profile("Qwen/Qwen3-32B").expect("matches");
        profile
            .authorize_charting("Qwen/Qwen3-32B")
            .expect("a 32B model may chart a map");
    }

    #[test]
    fn resolve_reads_context_limits_from_caps_not_the_profile() {
        // The same profile, bound to two different Caps, must report two
        // different context limits: proof that Profile stores none of its
        // own and ResolvedProfile is not caching a stale value.
        let table = ProfileTable::builtin();

        let small = table
            .resolve(&caps_for("Qwen/Qwen3-14B", 8_192))
            .expect("resolves");
        let large = table
            .resolve(&caps_for("Qwen/Qwen3-14B", 131_072))
            .expect("resolves");

        assert_eq!(small.granted_context, 8_192);
        assert_eq!(large.granted_context, 131_072);
        assert_ne!(small.granted_context, large.granted_context);
        // Same profile identity, only the live grant differs.
        assert_eq!(small.profile.match_pattern, large.profile.match_pattern);
    }

    #[test]
    fn resolve_propagates_the_charting_refusal() {
        let table = ProfileTable::builtin();
        let resolved = table
            .resolve(&caps_for("Qwen/Qwen3-0.6B", 32_768))
            .expect("resolves");
        assert!(resolved.authorize_charting().is_err());
    }

    #[test]
    fn micro_role_defaults_match_the_build_specification() {
        let roles = MicroRoles::default();
        assert_eq!(roles.deliberate.think, ThinkMode::On);
        assert_eq!(roles.deliberate.temperature, 0.6);
        assert!(!roles.deliberate.grammar);

        assert_eq!(roles.extract.think, ThinkMode::Off);
        assert_eq!(roles.extract.max_tokens, Some(1200));
        assert!(roles.extract.grammar);

        assert_eq!(roles.classify.temperature, 0.0);
        assert_eq!(roles.classify.max_tokens, Some(64));

        assert_eq!(roles.narrate.max_tokens, Some(200));
        assert!(!roles.narrate.grammar);
    }
}
