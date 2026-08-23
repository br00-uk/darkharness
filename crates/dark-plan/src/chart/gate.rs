//! The charting gate: refuses to chart a map on a model too small to do it
//! well.
//!
//! `dark-qwen`'s profile table carries `allow_charting`
//! (`dark_qwen::profile::Profile::allow_charting`), and a 4B model sets it to
//! `false`. `dark-plan` cannot depend on `dark-qwen` to read that flag
//! directly (Rule 17), so the caller reads it from the resolved profile and
//! passes the plain `bool` in here.

use dark_contract::{ErrCode, Error, Result};

/// Refuses to chart when the caller's profile does not allow it.
///
/// # Errors
///
/// Returns [`ErrCode::EngineUnsupported`] when `allow_charting` is `false`.
/// The message names `model_id`, and the remedy names the fix, matching the
/// refusal `dark_qwen::profile::Profile::authorize_charting` gives for the
/// same condition — the two must read the same to a person switching
/// between the profile check and this one.
pub fn authorize_charting(model_id: &str, allow_charting: bool) -> Result<()> {
    if allow_charting {
        return Ok(());
    }
    Err(Error::new(
        ErrCode::EngineUnsupported,
        format!("{model_id} cannot chart a map; its profile sets allow_charting to false"),
    )
    .with_remedy("Use a 14B or larger worker model, or an architect model, to chart a map."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_with_charting_allowed_passes() {
        authorize_charting("Qwen/Qwen3-32B", true).expect("a large model may chart");
    }

    #[test]
    fn a_model_with_charting_disallowed_is_refused_with_a_remedy() {
        let err = authorize_charting("Qwen/Qwen3-4B", false)
            .expect_err("a 4B model must not chart a map");
        assert_eq!(err.code, ErrCode::EngineUnsupported);
        assert!(err.message.contains("Qwen/Qwen3-4B"));
        assert!(err.remedy.is_some());
    }
}
