//! `dark agents explain`: shows the resolved `AGENTS.md` instruction chain.
//!
//! [`dark_agentsmd::resolve`] reads `$DARK_HOME/.darkharness/AGENTS.md`,
//! `<repo_root>/AGENTS.md`, and every directory between the repository root
//! and the working set, then [`explain::render`] turns the result into text
//! for a person to read. Both calls are filesystem reads with no network and
//! no loaded model.
//!
//! This command counts tokens by an approximate word count, not
//! [`dark_contract::Engine::tokenize`]: that needs a loaded model, which is
//! task unit `B2`. See [`approximate_token_count`].

use dark_agentsmd::{AgentsMdConfig, WorkingSet, explain, resolve};

/// Approximates a token count by splitting `text` on whitespace.
///
/// Mirrors `doctor::approximate_token_count`: no model is loaded yet
/// ([`dark_contract::Engine::tokenize`] needs task unit `B2`), and a word
/// count runs consistently over the reported budget for English prose,
/// which is what an `AGENTS.md` file is.
fn approximate_token_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Runs `dark agents explain`.
///
/// Resolves the instruction chain against `$DARK_HOME` and the repository
/// root, reads the repository's own `README.md` when one exists, and prints
/// [`explain::render`]'s report.
///
/// # Errors
///
/// Returns an error when the current directory cannot be read, or when the
/// instruction chain cannot be resolved (for example, a file a directory
/// scan found cannot be read).
pub(crate) fn run_command() -> anyhow::Result<()> {
    let dark_home = crate::dark_home();
    let repo_root = crate::repo_root()?;

    let config = AgentsMdConfig::default();
    let counter: &dyn Fn(&str) -> usize = &approximate_token_count;
    let chain = resolve(&dark_home, &repo_root, &WorkingSet::new(), &config, counter)
        .map_err(crate::contract_error)?;

    let readme = std::fs::read_to_string(repo_root.join("README.md")).ok();
    print!("{}", explain::render(&chain, &repo_root, readme.as_deref()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approximate_token_count_splits_on_whitespace() {
        assert_eq!(approximate_token_count("be terse. use active voice."), 5);
    }

    #[test]
    fn approximate_token_count_is_zero_for_empty_text() {
        assert_eq!(approximate_token_count(""), 0);
    }
}
