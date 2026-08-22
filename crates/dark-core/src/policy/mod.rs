//! The permission policy. See task unit `A4`.
//!
//! [`Policy`] gates every mutating action the harness takes. It answers one
//! question — "may this action run?" — as a [`Decision`]: run it, ask a
//! person first, or refuse it. It never runs the action itself. See Rule 33.
//!
//! ```
//! use dark_core::policy::{Action, Decision, Policy, PolicyConfig, RunMode};
//!
//! let policy = Policy::new(PolicyConfig::default(), RunMode::Interactive);
//! let action = Action::Read {
//!     what: "src/lib.rs".to_owned(),
//! };
//! assert_eq!(policy.classify(&action), Decision::Allow);
//! ```
//!
//! # For task unit `A2`
//!
//! Call [`Policy::decide`] with an [`Action`] and a [`Confirmer`]. It
//! returns [`Decision::Allow`] or [`Decision::Denied`]; when the policy
//! value is `confirm` and a person is present, `decide` emits the exact
//! prompt through the confirmer and blocks until the answer arrives before
//! it returns. [`ChannelConfirmer`] is a ready [`Confirmer`] that emits
//! [`dark_contract::Event::ConfirmReq`] on an [`dark_contract::EventTx`];
//! feed it the matching [`dark_contract::Intent::Confirm`] through
//! [`ChannelConfirmer::resolve`] as it arrives from the intent channel.
//!
//! Use [`Policy::classify`] instead when the caller wants to own the wait —
//! for example to show a confirmation modal and drive the timing itself —
//! or in a synchronous context.
//!
//! # Rule 34 and Rule 35
//!
//! [`PolicyConfig::write_outside_root`] cannot hold anything but
//! [`WriteOutsideRoot::DENIED`]: see that type's documentation for how the
//! type system, not a runtime check, makes this true. [`Policy::classify`]
//! also enforces it directly on every [`Action::Write`] with
//! `outside_root: true`, independent of the configured
//! [`PolicyConfig::write`] value.
//!
//! Rule 35 says a repository configuration file cannot widen its own
//! permissions. [`PolicyValue::is_at_least_as_strict_as`] exposes the
//! comparison this rule needs, but applying it to the AGENTS.md
//! machine-readable block is task unit `K2`'s work
//! (`crates/dark-agentsmd/src/config_block.rs`), not this module's: that
//! block accepts a narrower set of keys than the full `[policy]` table and
//! lives in a different crate.

mod action;
mod config;
mod confirm;

pub use action::{Action, ActionKind};
pub use config::{PolicyConfig, PolicyValue, WriteOutsideRoot};
pub use confirm::{ChannelConfirmer, Confirmer};

use dark_contract::{Allow, ConfirmPrompt, ErrCode, Error};

/// Whether a person can see a confirmation prompt and answer it now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// A person is present. A `confirm` policy value shows a prompt and
    /// waits for the answer.
    Interactive,
    /// No person is present, for example a scripted or CI invocation.
    Headless {
        /// The `--yes` flag. `true` turns a `confirm` policy value into
        /// `allow`. `false` turns it into `deny`. See Do step 4 of task
        /// unit `A4`.
        yes: bool,
    },
}

/// What [`Policy::classify`] or [`Policy::decide`] decided about one
/// [`Action`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Run the action.
    Allow,
    /// Show this prompt to a person and wait for the answer.
    ///
    /// Only [`Policy::classify`] returns this variant. [`Policy::decide`]
    /// resolves it into [`Decision::Allow`] or [`Decision::Denied`] before
    /// it returns.
    NeedsConfirmation(ConfirmPrompt),
    /// Refuse the action.
    Denied(Error),
}

/// The permission policy for one session.
///
/// A `Policy` is cheap to copy: it holds a [`PolicyConfig`] and a
/// [`RunMode`], both of which are themselves `Copy`.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    config: PolicyConfig,
    mode: RunMode,
}

impl Policy {
    /// Creates a policy from a configuration and a run mode.
    #[must_use]
    pub fn new(config: PolicyConfig, mode: RunMode) -> Self {
        Self { config, mode }
    }

    /// Returns the configuration this policy enforces.
    pub fn config(&self) -> PolicyConfig {
        self.config
    }

    /// Returns the run mode this policy enforces.
    pub fn mode(&self) -> RunMode {
        self.mode
    }

    /// Classifies `action` without blocking.
    ///
    /// A `confirm` policy value in [`RunMode::Interactive`] returns
    /// [`Decision::NeedsConfirmation`] rather than waiting: the caller owns
    /// the wait. [`Policy::decide`] is the convenience method that performs
    /// the wait through a [`Confirmer`].
    ///
    /// A write with `outside_root: true` is always
    /// [`Decision::Denied`], regardless of [`PolicyConfig::write`]. See
    /// Rule 34.
    #[must_use]
    pub fn classify(&self, action: &Action) -> Decision {
        if let Action::Write {
            outside_root: true, ..
        } = action
        {
            return Decision::Denied(
                Error::new(
                    ErrCode::PolicyDenied,
                    "a write outside the repository root is always denied",
                )
                .with_remedy("Use a path inside the repository root."),
            );
        }

        let value = match action.kind() {
            ActionKind::Read => self.config.read,
            ActionKind::Write => self.config.write,
            ActionKind::Exec => self.config.exec,
        };

        match value {
            PolicyValue::Allow => Decision::Allow,
            PolicyValue::Deny => {
                let kind = action.kind().as_str();
                Decision::Denied(Error::new(
                    ErrCode::PolicyDenied,
                    format!("the policy denies this {kind} action"),
                ))
            }
            PolicyValue::Confirm => match self.mode {
                RunMode::Headless { yes: true } => Decision::Allow,
                RunMode::Headless { yes: false } => Decision::Denied(Error::new(
                    ErrCode::PolicyConfirmRequired,
                    "headless mode cannot show a confirmation prompt; pass --yes to allow it",
                )),
                RunMode::Interactive => Decision::NeedsConfirmation(action.to_prompt()),
            },
        }
    }

    /// Classifies `action`, resolving a `confirm` decision through
    /// `confirmer` before it returns.
    ///
    /// In [`RunMode::Interactive`], a `confirm` policy value emits the
    /// exact prompt on `confirmer` and blocks until the matching answer
    /// arrives. See Do step 2 of task unit `A4`. The result is always
    /// [`Decision::Allow`] or [`Decision::Denied`]: this method never
    /// returns [`Decision::NeedsConfirmation`].
    pub async fn decide(&self, action: &Action, confirmer: &dyn Confirmer) -> Decision {
        match self.classify(action) {
            Decision::NeedsConfirmation(prompt) => match confirmer.confirm(prompt).await {
                Allow::Once | Allow::Always => Decision::Allow,
                Allow::Deny => Decision::Denied(Error::new(
                    ErrCode::PolicyDenied,
                    "the person denied the action",
                )),
            },
            decided => decided,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use async_trait::async_trait;

    use super::*;

    fn read() -> Action {
        Action::Read {
            what: "src/lib.rs".into(),
        }
    }

    fn write(outside_root: bool) -> Action {
        Action::Write {
            path: PathBuf::from("src/lib.rs"),
            diff: "@@ -1 +1 @@\n-a\n+b\n".into(),
            outside_root,
        }
    }

    fn exec() -> Action {
        Action::Exec {
            command: "cargo test".into(),
            cwd: PathBuf::from("."),
            shell: false,
        }
    }

    fn config_with(read: PolicyValue, write: PolicyValue, exec: PolicyValue) -> PolicyConfig {
        PolicyConfig {
            read,
            write,
            exec,
            ..PolicyConfig::default()
        }
    }

    fn assert_denied(decision: Decision) {
        match decision {
            Decision::Denied(_) => {}
            other => panic!("expected Denied, got {other:?}"),
        }
    }

    // --- allow / confirm / deny, for each of read, write, exec ---

    #[test]
    fn read_allow_is_allowed() {
        let policy = Policy::new(
            config_with(PolicyValue::Allow, PolicyValue::Deny, PolicyValue::Deny),
            RunMode::Interactive,
        );
        assert_eq!(policy.classify(&read()), Decision::Allow);
    }

    #[test]
    fn read_deny_is_denied() {
        let policy = Policy::new(
            config_with(PolicyValue::Deny, PolicyValue::Allow, PolicyValue::Allow),
            RunMode::Interactive,
        );
        assert_denied(policy.classify(&read()));
    }

    #[test]
    fn read_confirm_needs_confirmation_when_interactive() {
        let policy = Policy::new(
            config_with(PolicyValue::Confirm, PolicyValue::Allow, PolicyValue::Allow),
            RunMode::Interactive,
        );
        match policy.classify(&read()) {
            Decision::NeedsConfirmation(_) => {}
            other => panic!("expected NeedsConfirmation, got {other:?}"),
        }
    }

    #[test]
    fn write_allow_is_allowed() {
        let policy = Policy::new(
            config_with(PolicyValue::Deny, PolicyValue::Allow, PolicyValue::Deny),
            RunMode::Interactive,
        );
        assert_eq!(policy.classify(&write(false)), Decision::Allow);
    }

    #[test]
    fn write_deny_is_denied() {
        let policy = Policy::new(
            config_with(PolicyValue::Allow, PolicyValue::Deny, PolicyValue::Allow),
            RunMode::Interactive,
        );
        assert_denied(policy.classify(&write(false)));
    }

    #[test]
    fn write_confirm_needs_confirmation_when_interactive() {
        let policy = Policy::new(
            config_with(PolicyValue::Allow, PolicyValue::Confirm, PolicyValue::Allow),
            RunMode::Interactive,
        );
        match policy.classify(&write(false)) {
            Decision::NeedsConfirmation(ConfirmPrompt::Write { diff, .. }) => {
                assert_eq!(diff, "@@ -1 +1 @@\n-a\n+b\n");
            }
            other => panic!("expected NeedsConfirmation(Write), got {other:?}"),
        }
    }

    #[test]
    fn exec_allow_is_allowed() {
        let policy = Policy::new(
            config_with(PolicyValue::Deny, PolicyValue::Deny, PolicyValue::Allow),
            RunMode::Interactive,
        );
        assert_eq!(policy.classify(&exec()), Decision::Allow);
    }

    #[test]
    fn exec_deny_is_denied() {
        let policy = Policy::new(
            config_with(PolicyValue::Allow, PolicyValue::Allow, PolicyValue::Deny),
            RunMode::Interactive,
        );
        assert_denied(policy.classify(&exec()));
    }

    #[test]
    fn exec_confirm_needs_confirmation_when_interactive() {
        let policy = Policy::new(
            config_with(PolicyValue::Allow, PolicyValue::Allow, PolicyValue::Confirm),
            RunMode::Interactive,
        );
        match policy.classify(&exec()) {
            Decision::NeedsConfirmation(ConfirmPrompt::Exec { command, .. }) => {
                assert_eq!(command, "cargo test");
            }
            other => panic!("expected NeedsConfirmation(Exec), got {other:?}"),
        }
    }

    // --- headless mode ---

    #[test]
    fn headless_confirm_without_yes_is_denied() {
        let policy = Policy::new(
            config_with(
                PolicyValue::Confirm,
                PolicyValue::Confirm,
                PolicyValue::Confirm,
            ),
            RunMode::Headless { yes: false },
        );
        assert_denied(policy.classify(&read()));
        assert_denied(policy.classify(&write(false)));
        assert_denied(policy.classify(&exec()));
    }

    #[test]
    fn headless_confirm_without_yes_reports_confirm_required() {
        let policy = Policy::new(
            config_with(PolicyValue::Allow, PolicyValue::Confirm, PolicyValue::Allow),
            RunMode::Headless { yes: false },
        );
        match policy.classify(&write(false)) {
            Decision::Denied(err) => assert_eq!(err.code, ErrCode::PolicyConfirmRequired),
            other => panic!("expected Denied, got {other:?}"),
        }
    }

    #[test]
    fn headless_confirm_with_yes_is_allowed() {
        let policy = Policy::new(
            config_with(
                PolicyValue::Confirm,
                PolicyValue::Confirm,
                PolicyValue::Confirm,
            ),
            RunMode::Headless { yes: true },
        );
        assert_eq!(policy.classify(&read()), Decision::Allow);
        assert_eq!(policy.classify(&write(false)), Decision::Allow);
        assert_eq!(policy.classify(&exec()), Decision::Allow);
    }

    #[test]
    fn headless_mode_does_not_change_allow_or_deny() {
        let policy = Policy::new(
            config_with(PolicyValue::Allow, PolicyValue::Deny, PolicyValue::Allow),
            RunMode::Headless { yes: false },
        );
        assert_eq!(policy.classify(&read()), Decision::Allow);
        assert_denied(policy.classify(&write(false)));
    }

    // --- write_outside_root: Rule 34 ---

    #[test]
    fn write_outside_root_is_denied_even_when_write_is_allow() {
        let policy = Policy::new(
            config_with(PolicyValue::Allow, PolicyValue::Allow, PolicyValue::Allow),
            RunMode::Interactive,
        );
        assert_denied(policy.classify(&write(true)));
    }

    #[test]
    fn write_outside_root_is_denied_in_every_run_mode() {
        let config = config_with(PolicyValue::Allow, PolicyValue::Allow, PolicyValue::Allow);
        assert_denied(Policy::new(config, RunMode::Interactive).classify(&write(true)));
        assert_denied(Policy::new(config, RunMode::Headless { yes: true }).classify(&write(true)));
        assert_denied(Policy::new(config, RunMode::Headless { yes: false }).classify(&write(true)));
    }

    #[test]
    fn write_outside_root_denial_names_the_right_code() {
        let policy = Policy::new(
            config_with(PolicyValue::Allow, PolicyValue::Allow, PolicyValue::Allow),
            RunMode::Interactive,
        );
        match policy.classify(&write(true)) {
            Decision::Denied(err) => assert_eq!(err.code, ErrCode::PolicyDenied),
            other => panic!("expected Denied, got {other:?}"),
        }
    }

    #[test]
    fn write_outside_root_is_also_denied_even_starting_from_the_deserialized_config() {
        // Rule 34's most important property, exercised end to end: parse a
        // config that asks to widen write_outside_root, then prove classify
        // still denies an outside-root write. WriteOutsideRoot cannot carry
        // "allow" (see config.rs), but this confirms the whole path.
        let config: PolicyConfig =
            serde_json::from_str(r#"{"write": "allow", "write_outside_root": "allow"}"#).unwrap();
        let policy = Policy::new(config, RunMode::Interactive);
        assert_denied(policy.classify(&write(true)));
        // The same policy still allows a write inside the root.
        assert_eq!(policy.classify(&write(false)), Decision::Allow);
    }

    // --- decide(): resolves confirmation through a Confirmer ---

    struct FixedConfirmer(Allow);

    #[async_trait]
    impl Confirmer for FixedConfirmer {
        async fn confirm(&self, _prompt: ConfirmPrompt) -> Allow {
            self.0
        }
    }

    #[tokio::test]
    async fn decide_allows_when_the_confirmer_says_once() {
        let policy = Policy::new(
            config_with(PolicyValue::Allow, PolicyValue::Confirm, PolicyValue::Allow),
            RunMode::Interactive,
        );
        let confirmer = FixedConfirmer(Allow::Once);
        assert_eq!(
            policy.decide(&write(false), &confirmer).await,
            Decision::Allow
        );
    }

    #[tokio::test]
    async fn decide_allows_when_the_confirmer_says_always() {
        let policy = Policy::new(
            config_with(PolicyValue::Allow, PolicyValue::Confirm, PolicyValue::Allow),
            RunMode::Interactive,
        );
        let confirmer = FixedConfirmer(Allow::Always);
        assert_eq!(
            policy.decide(&write(false), &confirmer).await,
            Decision::Allow
        );
    }

    #[tokio::test]
    async fn decide_denies_when_the_confirmer_says_deny() {
        let policy = Policy::new(
            config_with(PolicyValue::Allow, PolicyValue::Confirm, PolicyValue::Allow),
            RunMode::Interactive,
        );
        let confirmer = FixedConfirmer(Allow::Deny);
        assert_denied(policy.decide(&write(false), &confirmer).await);
    }

    #[tokio::test]
    async fn decide_never_returns_needs_confirmation() {
        let policy = Policy::new(
            config_with(
                PolicyValue::Confirm,
                PolicyValue::Confirm,
                PolicyValue::Confirm,
            ),
            RunMode::Interactive,
        );
        let confirmer = FixedConfirmer(Allow::Once);
        for action in [read(), write(false), exec()] {
            if let Decision::NeedsConfirmation(_) = policy.decide(&action, &confirmer).await {
                panic!("decide() must resolve confirmation");
            }
        }
    }

    #[tokio::test]
    async fn decide_never_asks_the_confirmer_when_the_value_is_not_confirm() {
        // A confirmer that panics if it is ever called proves decide() only
        // consults it for a `confirm` policy value.
        struct PanicIfCalled;
        #[async_trait]
        impl Confirmer for PanicIfCalled {
            async fn confirm(&self, _prompt: ConfirmPrompt) -> Allow {
                panic!("must not be called for an allow or deny action");
            }
        }

        let policy = Policy::new(
            config_with(PolicyValue::Allow, PolicyValue::Deny, PolicyValue::Allow),
            RunMode::Interactive,
        );
        assert_eq!(
            policy.decide(&read(), &PanicIfCalled).await,
            Decision::Allow
        );
        assert_denied(policy.decide(&write(false), &PanicIfCalled).await);
    }

    #[tokio::test]
    async fn decide_uses_the_channel_confirmer_end_to_end() {
        use std::sync::Arc;

        use dark_contract::{Event, EventBus, Received};

        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let confirmer = Arc::new(ChannelConfirmer::new(bus.tx()));

        let policy = Policy::new(
            config_with(PolicyValue::Allow, PolicyValue::Confirm, PolicyValue::Allow),
            RunMode::Interactive,
        );

        let task_confirmer = Arc::clone(&confirmer);
        let decide_task =
            tokio::spawn(
                async move { policy.decide(&write(false), task_confirmer.as_ref()).await },
            );

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("a ConfirmReq must arrive")
            .expect("bus is open");
        let Received::Event(Event::ConfirmReq { id, prompt }) = received else {
            panic!("expected a ConfirmReq");
        };
        match prompt {
            ConfirmPrompt::Write { diff, .. } => {
                assert_eq!(diff, "@@ -1 +1 @@\n-a\n+b\n");
            }
            other => panic!("unexpected prompt: {other:?}"),
        }

        assert!(confirmer.resolve(&id, Allow::Once).await);

        let decision = tokio::time::timeout(std::time::Duration::from_secs(1), decide_task)
            .await
            .expect("decide() must return once resolve() is called")
            .unwrap();
        assert_eq!(decision, Decision::Allow);
    }
}
