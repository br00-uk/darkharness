//! The error taxonomy.
//!
//! Every error carries a code, a message, and an optional remedy. The code
//! names the domain with a prefix, for example `E_ENGINE_`. A caller matches
//! on the code. A person reads the message and the remedy.

use serde::{Deserialize, Serialize};

/// The domain that an error came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrDomain {
    /// The engine failed to load a model or to generate.
    Engine,
    /// A tool failed.
    Tool,
    /// The permission policy or dark mode blocked an action.
    Policy,
    /// A map operation failed.
    Map,
    /// A documentation pack operation failed.
    Pack,
    /// Repository analysis failed.
    Explore,
    /// An action needs a person who is not present.
    Hitl,
    /// A session rule blocked an action.
    Session,
}

impl ErrDomain {
    /// Returns the code prefix for this domain.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Engine => "E_ENGINE_",
            Self::Tool => "E_TOOL_",
            Self::Policy => "E_POLICY_",
            Self::Map => "E_MAP_",
            Self::Pack => "E_PACK_",
            Self::Explore => "E_EXPLORE_",
            Self::Hitl => "E_HITL_",
            Self::Session => "E_SESSION_",
        }
    }
}

/// A stable error code.
///
/// The string form appears in logs, in the transcript, and in the terminal
/// application. Do not change a string form once it ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ErrCode {
    /// The model does not fit in the memory budget.
    EngineWontFit,
    /// The engine lacks a capability that the request needs.
    EngineUnsupported,
    /// The engine failed to load a model.
    EngineLoad,
    /// Generation failed.
    EngineGenerate,
    /// The caller cancelled the request.
    EngineCancelled,

    /// The path does not exist.
    ToolNotFound,
    /// The file changed on disk since the session read it.
    ToolStale,
    /// The edit string matched more than once.
    ToolAmbiguous,
    /// The tool arguments failed schema validation.
    ToolInvalidArgs,
    /// The tool exceeded its timeout.
    ToolTimeout,
    /// The tool tried to leave the repository root.
    ToolOutsideRoot,
    /// The tool failed for a reason that no other code covers.
    ToolFailed,

    /// Dark mode blocked the action.
    PolicyDark,
    /// The policy denied the action.
    PolicyDenied,
    /// The action needs a confirmation that no one gave.
    PolicyConfirmRequired,

    /// The edge creates a blocking cycle.
    MapCycle,
    /// No ticket is takeable.
    MapEmptyFrontier,
    /// The map or the ticket does not exist.
    MapNotFound,

    /// The source has no discoverable licence.
    PackNoLicence,
    /// The pack vectors do not match the embedding model.
    PackDimMismatch,
    /// The pack does not exist.
    PackNotFound,

    /// The working tree changed during analysis.
    ExploreDirty,
    /// A source file failed to parse.
    ExploreParse,

    /// The ticket needs a person who is not present.
    HitlRequiresHuman,

    /// This session already resolved a ticket.
    SessionResolutionLimit,
    /// The session does not exist.
    SessionNotFound,
}

impl ErrCode {
    /// Returns the stable string form, for example `E_ENGINE_WONT_FIT`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EngineWontFit => "E_ENGINE_WONT_FIT",
            Self::EngineUnsupported => "E_ENGINE_UNSUPPORTED",
            Self::EngineLoad => "E_ENGINE_LOAD",
            Self::EngineGenerate => "E_ENGINE_GENERATE",
            Self::EngineCancelled => "E_ENGINE_CANCELLED",
            Self::ToolNotFound => "E_TOOL_NOT_FOUND",
            Self::ToolStale => "E_TOOL_STALE",
            Self::ToolAmbiguous => "E_TOOL_AMBIGUOUS",
            Self::ToolInvalidArgs => "E_TOOL_INVALID_ARGS",
            Self::ToolTimeout => "E_TOOL_TIMEOUT",
            Self::ToolOutsideRoot => "E_TOOL_OUTSIDE_ROOT",
            Self::ToolFailed => "E_TOOL_FAILED",
            Self::PolicyDark => "E_POLICY_DARK",
            Self::PolicyDenied => "E_POLICY_DENIED",
            Self::PolicyConfirmRequired => "E_POLICY_CONFIRM_REQUIRED",
            Self::MapCycle => "E_MAP_CYCLE",
            Self::MapEmptyFrontier => "E_MAP_EMPTY_FRONTIER",
            Self::MapNotFound => "E_MAP_NOT_FOUND",
            Self::PackNoLicence => "E_PACK_NO_LICENCE",
            Self::PackDimMismatch => "E_PACK_DIM_MISMATCH",
            Self::PackNotFound => "E_PACK_NOT_FOUND",
            Self::ExploreDirty => "E_EXPLORE_DIRTY",
            Self::ExploreParse => "E_EXPLORE_PARSE",
            Self::HitlRequiresHuman => "E_HITL_REQUIRES_HUMAN",
            Self::SessionResolutionLimit => "E_SESSION_RESOLUTION_LIMIT",
            Self::SessionNotFound => "E_SESSION_NOT_FOUND",
        }
    }

    /// Returns the domain that this code belongs to.
    pub fn domain(self) -> ErrDomain {
        match self {
            Self::EngineWontFit
            | Self::EngineUnsupported
            | Self::EngineLoad
            | Self::EngineGenerate
            | Self::EngineCancelled => ErrDomain::Engine,
            Self::ToolNotFound
            | Self::ToolStale
            | Self::ToolAmbiguous
            | Self::ToolInvalidArgs
            | Self::ToolTimeout
            | Self::ToolOutsideRoot
            | Self::ToolFailed => ErrDomain::Tool,
            Self::PolicyDark | Self::PolicyDenied | Self::PolicyConfirmRequired => {
                ErrDomain::Policy
            }
            Self::MapCycle | Self::MapEmptyFrontier | Self::MapNotFound => ErrDomain::Map,
            Self::PackNoLicence | Self::PackDimMismatch | Self::PackNotFound => ErrDomain::Pack,
            Self::ExploreDirty | Self::ExploreParse => ErrDomain::Explore,
            Self::HitlRequiresHuman => ErrDomain::Hitl,
            Self::SessionResolutionLimit | Self::SessionNotFound => ErrDomain::Session,
        }
    }

    /// Returns the remedy to show when the caller supplies none.
    ///
    /// These strings come from Appendix A of the build specification.
    pub fn default_remedy(self) -> Option<&'static str> {
        let remedy = match self {
            Self::EngineWontFit => {
                "Reduce the context. Use a smaller quantisation. Share a model between role classes."
            }
            Self::EngineUnsupported => "Disable the feature. Use a different model.",
            Self::PolicyDark => "Run /golight to allow the network.",
            Self::PolicyDenied => "Change the policy setting.",
            Self::ToolNotFound => "List the directory first.",
            Self::ToolStale => "Read the file again.",
            Self::ToolAmbiguous => "Add more context to the string.",
            Self::ToolOutsideRoot => "Use a path inside the repository root.",
            Self::MapCycle => "Remove one edge on the reported path.",
            Self::MapEmptyFrontier => "Check the blocking edges.",
            Self::HitlRequiresHuman => "Open the terminal application. Confirm in the modal.",
            Self::SessionResolutionLimit => "Start a new session.",
            Self::PackNoLicence => "Add a licence file. Use --i-accept-responsibility.",
            Self::PackDimMismatch => "Run dark pack reindex.",
            Self::ExploreDirty => "Commit or stash. Run the analysis again.",
            _ => return None,
        };
        Some(remedy)
    }
}

impl std::fmt::Display for ErrCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An error from any part of the harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct Error {
    /// The stable code.
    pub code: ErrCode,
    /// The message for a person to read.
    pub message: String,
    /// The action that clears the error.
    pub remedy: Option<String>,
}

impl Error {
    /// Creates an error and fills the remedy from the code.
    pub fn new(code: ErrCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            remedy: code.default_remedy().map(ToOwned::to_owned),
        }
    }

    /// Replaces the remedy.
    #[must_use]
    pub fn with_remedy(mut self, remedy: impl Into<String>) -> Self {
        self.remedy = Some(remedy.into());
        self
    }

    /// Returns the domain of this error.
    pub fn domain(&self) -> ErrDomain {
        self.code.domain()
    }
}

/// The result type that every fallible function in the workspace returns.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_string_starts_with_its_domain_prefix() {
        // The taxonomy is only useful when a caller can match on the prefix.
        for code in ALL_CODES {
            assert!(
                code.as_str().starts_with(code.domain().prefix()),
                "{} does not start with {}",
                code.as_str(),
                code.domain().prefix()
            );
        }
    }

    #[test]
    fn code_strings_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for code in ALL_CODES {
            assert!(
                seen.insert(code.as_str()),
                "duplicate code {}",
                code.as_str()
            );
        }
    }

    #[test]
    fn new_fills_the_documented_remedy() {
        let err = Error::new(ErrCode::HitlRequiresHuman, "ticket T-018 needs a person");
        assert_eq!(
            err.remedy.as_deref(),
            Some("Open the terminal application. Confirm in the modal.")
        );
    }

    #[test]
    fn with_remedy_replaces_the_default() {
        let err = Error::new(ErrCode::MapCycle, "cycle").with_remedy("drop T-004 → T-007");
        assert_eq!(err.remedy.as_deref(), Some("drop T-004 → T-007"));
    }

    #[test]
    fn display_shows_the_code_and_the_message() {
        let err = Error::new(ErrCode::ToolStale, "pack.rs changed on disk");
        assert_eq!(err.to_string(), "E_TOOL_STALE: pack.rs changed on disk");
    }

    const ALL_CODES: [ErrCode; 26] = [
        ErrCode::EngineWontFit,
        ErrCode::EngineUnsupported,
        ErrCode::EngineLoad,
        ErrCode::EngineGenerate,
        ErrCode::EngineCancelled,
        ErrCode::ToolNotFound,
        ErrCode::ToolStale,
        ErrCode::ToolAmbiguous,
        ErrCode::ToolInvalidArgs,
        ErrCode::ToolTimeout,
        ErrCode::ToolOutsideRoot,
        ErrCode::ToolFailed,
        ErrCode::PolicyDark,
        ErrCode::PolicyDenied,
        ErrCode::PolicyConfirmRequired,
        ErrCode::MapCycle,
        ErrCode::MapEmptyFrontier,
        ErrCode::MapNotFound,
        ErrCode::PackNoLicence,
        ErrCode::PackDimMismatch,
        ErrCode::PackNotFound,
        ErrCode::ExploreDirty,
        ErrCode::ExploreParse,
        ErrCode::HitlRequiresHuman,
        ErrCode::SessionResolutionLimit,
        ErrCode::SessionNotFound,
    ];
}
