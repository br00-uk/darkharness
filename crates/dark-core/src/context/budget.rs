//! The token budget from Appendix B.
//!
//! Appendix B of the build specification names nine parts of the context
//! window and gives each part a token target at a 32,000-token grant. This
//! module turns that table into a type a caller can compare real token
//! counts against.

/// One accounted-for part of the context window, in build order.
///
/// The first five parts make up the prefix (`A3` `Do` step 1). The engine
/// caches the prefix, so its content must not change during a turn. The
/// remaining four parts make up the tail (`A3` `Do` step 2), which changes
/// as a turn runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    /// Prefix section 1: the system prompt.
    SystemPrompt,
    /// Prefix section 2: the `AGENTS.md` chain.
    AgentsChain,
    /// Prefix section 3: the environment block. Date only, no time.
    Environment,
    /// Prefix section 4: the map digest, present only when a map is loaded.
    MapDigest,
    /// Prefix section 5: the claimed ticket body.
    Ticket,
    /// Tail section 6: the tool schemas.
    ToolSchemas,
    /// Tail section 7: the Lexicon chunks for this turn.
    Lexicon,
    /// Tail sections 8 to 10: message history, the input message, and tool
    /// results, all counted against one budget.
    WorkingSpace,
    /// Tail section 11: the reserve for generation.
    GenerationReserve,
}

/// Every [`Part`], in build order.
pub const ALL_PARTS: [Part; 9] = [
    Part::SystemPrompt,
    Part::AgentsChain,
    Part::Environment,
    Part::MapDigest,
    Part::Ticket,
    Part::ToolSchemas,
    Part::Lexicon,
    Part::WorkingSpace,
    Part::GenerationReserve,
];

/// The sum of every [`Part::tokens_at_32k`] value.
///
/// Appendix B totals the table at 32,000 tokens. This constant is that
/// total, kept in one place so a test can check the table stays consistent.
pub const TOTAL_AT_32K: usize = 32_000;

impl Part {
    /// Returns the name that a notice or a log line shows for this part.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SystemPrompt => "system prompt",
            Self::AgentsChain => "AGENTS.md chain",
            Self::Environment => "environment block",
            Self::MapDigest => "map digest",
            Self::Ticket => "claimed ticket body",
            Self::ToolSchemas => "tool schemas",
            Self::Lexicon => "Lexicon chunks",
            Self::WorkingSpace => "working space",
            Self::GenerationReserve => "generation reserve",
        }
    }

    /// Returns the token target from Appendix B, at a 32,000-token grant.
    ///
    /// The build specification gives this table only at a 32k grant, with no
    /// formula for another grant size. Treat the value as a fixed reference
    /// target for the budget checks in this module, not as a value that
    /// scales with [`dark_contract::Caps::granted_context`]. Only the
    /// compaction threshold ([`super::compact::compaction_threshold`]) scales
    /// with the real grant.
    // The Appendix B table gives the map digest and the tool schemas the
    // same figure by coincidence, not because the two parts should merge:
    // each row names a distinct part of the context window.
    #[allow(clippy::match_same_arms)]
    pub fn tokens_at_32k(self) -> usize {
        match self {
            Self::SystemPrompt => 800,
            Self::AgentsChain => 1500,
            Self::Environment => 100,
            Self::MapDigest => 1200,
            Self::Ticket => 400,
            Self::ToolSchemas => 1200,
            Self::Lexicon => 4000,
            Self::WorkingSpace => 18_000,
            Self::GenerationReserve => 4800,
        }
    }

    /// Returns `true` for the five parts that make up the prefix.
    ///
    /// A `true` part sits in the engine's key-value cache and must not
    /// change during a turn (Rule 5). A `false` part is in the tail and may
    /// change as the turn runs (Rule 8).
    pub fn is_prefix(self) -> bool {
        matches!(
            self,
            Self::SystemPrompt
                | Self::AgentsChain
                | Self::Environment
                | Self::MapDigest
                | Self::Ticket
        )
    }
}

impl std::fmt::Display for Part {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One part's real token count, measured against its Appendix B target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetCheck {
    /// The part this check covers.
    pub part: Part,
    /// The target from [`Part::tokens_at_32k`].
    pub target: usize,
    /// The token count that [`dark_contract::Engine::tokenize`] measured.
    pub actual: usize,
}

impl BudgetCheck {
    /// Measures `actual` tokens against `part`'s Appendix B target.
    pub fn new(part: Part, actual: usize) -> Self {
        Self {
            part,
            target: part.tokens_at_32k(),
            actual,
        }
    }

    /// Returns `true` when `actual` exceeds `target`.
    ///
    /// Appendix B calls out the map digest by name: each extra 100 tokens
    /// there removes 100 tokens from the working space on every turn.
    /// "Defend the limit" applies to every part, not only the digest.
    pub fn is_over_budget(self) -> bool {
        self.actual > self.target
    }

    /// Returns how many tokens `actual` sits over `target`.
    ///
    /// Returns zero when `actual` is at or under `target`.
    pub fn overage(self) -> usize {
        self.actual.saturating_sub(self.target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_totals_the_appendix_b_figure() {
        let total: usize = ALL_PARTS.iter().map(|part| part.tokens_at_32k()).sum();
        assert_eq!(total, TOTAL_AT_32K);
    }

    #[test]
    fn exactly_five_parts_are_in_the_prefix() {
        let prefix_parts = ALL_PARTS.iter().filter(|part| part.is_prefix()).count();
        assert_eq!(prefix_parts, 5);
    }

    #[test]
    fn a_check_under_budget_is_not_over_budget() {
        let check = BudgetCheck::new(Part::SystemPrompt, 799);
        assert!(!check.is_over_budget());
        assert_eq!(check.overage(), 0);
    }

    #[test]
    fn a_check_over_budget_reports_the_overage() {
        let check = BudgetCheck::new(Part::MapDigest, 1300);
        assert!(check.is_over_budget());
        assert_eq!(check.overage(), 100);
    }

    #[test]
    fn every_part_name_is_unique() {
        let mut seen = std::collections::HashSet::new();
        for part in ALL_PARTS {
            assert!(seen.insert(part.as_str()), "duplicate name {part}");
        }
    }
}
