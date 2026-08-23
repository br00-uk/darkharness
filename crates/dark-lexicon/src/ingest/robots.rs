//! A minimal `robots.txt` parser.
//!
//! G2 asks the network adapters to "obey `robots.txt`". This implements
//! enough of the de facto standard to make that promise good: `User-agent`
//! groups, `Disallow`, and `Allow`, matched by the longest matching prefix
//! rule that every major crawler uses when two rules conflict. It does not
//! implement `Sitemap` lines, wildcards, or `Crawl-delay`; the harness sets
//! its own rate limit (see [`crate::ingest::fetch::RateLimiter`]) rather
//! than trust a site to declare one.

use crate::ingest::fetch::{Fetcher, host_of};
use dark_contract::Result;

/// The user agent that darkharness identifies itself as.
///
/// A `robots.txt` group for this literal name takes priority over the
/// wildcard `*` group, matching how every major crawler resolves group
/// selection.
pub const USER_AGENT: &str = "darkharness";

/// One `Allow` or `Disallow` rule.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    prefix: String,
    allow: bool,
}

/// A parsed `robots.txt` policy for one host.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RobotsPolicy {
    rules: Vec<Rule>,
}

impl RobotsPolicy {
    /// Parses `robots.txt` text.
    ///
    /// Group selection: this collects rules from the first group whose
    /// `User-agent` line matches [`USER_AGENT`] case-insensitively; if no
    /// such group exists, it falls back to the first `User-agent: *`
    /// group. A missing or empty file parses as a policy with no rules,
    /// which allows every path.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let groups = split_into_groups(text);
        let chosen = groups
            .iter()
            .find(|g| g.agents.iter().any(|a| a.eq_ignore_ascii_case(USER_AGENT)))
            .or_else(|| groups.iter().find(|g| g.agents.iter().any(|a| a == "*")));
        match chosen {
            Some(group) => Self {
                rules: group.rules.clone(),
            },
            None => Self::default(),
        }
    }

    /// Returns `true` when `path` may be fetched.
    ///
    /// When more than one rule matches, the longest matching prefix wins,
    /// per the de facto standard. A tie between an `Allow` and a
    /// `Disallow` of the same length favours `Allow`, matching the
    /// standard's own tie-break.
    #[must_use]
    pub fn is_allowed(&self, path: &str) -> bool {
        let mut best: Option<&Rule> = None;
        for rule in &self.rules {
            if !path.starts_with(rule.prefix.as_str()) {
                continue;
            }
            best = match best {
                Some(current) if current.prefix.len() > rule.prefix.len() => Some(current),
                Some(current) if current.prefix.len() == rule.prefix.len() && !rule.allow => {
                    Some(current)
                }
                _ => Some(rule),
            };
        }
        best.is_none_or(|rule| rule.allow)
    }

    /// Fetches and parses `robots.txt` for the host that `url` names.
    ///
    /// A fetch failure — the site has no `robots.txt`, or the request
    /// fails — parses as an empty, all-allowing policy: the absence of a
    /// `robots.txt` file means the site has no exclusions to honour.
    ///
    /// # Errors
    ///
    /// Returns `E_TOOL_FAILED` when `url` names no host.
    pub fn fetch(fetcher: &dyn Fetcher, url: &str) -> Result<Self> {
        let host = host_of(url)?;
        let robots_url = format!("https://{host}/robots.txt");
        let policy = fetcher
            .fetch(&robots_url)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map_or_else(Self::default, |text| Self::parse(&text));
        Ok(policy)
    }
}

struct Group {
    agents: Vec<String>,
    rules: Vec<Rule>,
}

/// Splits `robots.txt` text into `User-agent` groups.
///
/// A group is one or more consecutive `User-agent` lines followed by the
/// `Allow`/`Disallow` lines that apply to all of them, per the standard's
/// grouping rule.
fn split_into_groups(text: &str) -> Vec<Group> {
    let mut groups = Vec::new();
    let mut current_agents: Vec<String> = Vec::new();
    let mut current_rules: Vec<Rule> = Vec::new();
    let mut in_agent_run = false;

    let flush = |groups: &mut Vec<Group>, agents: &mut Vec<String>, rules: &mut Vec<Rule>| {
        if agents.is_empty() {
            rules.clear();
        } else {
            groups.push(Group {
                agents: std::mem::take(agents),
                rules: std::mem::take(rules),
            });
        }
    };

    for raw_line in text.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();

        match key.as_str() {
            "user-agent" => {
                if !in_agent_run {
                    flush(&mut groups, &mut current_agents, &mut current_rules);
                }
                current_agents.push(value.to_owned());
                in_agent_run = true;
            }
            "disallow" if !value.is_empty() => {
                in_agent_run = false;
                current_rules.push(Rule {
                    prefix: value.to_owned(),
                    allow: false,
                });
            }
            "disallow" => {
                // An empty Disallow value means "disallow nothing".
                in_agent_run = false;
            }
            "allow" => {
                in_agent_run = false;
                current_rules.push(Rule {
                    prefix: value.to_owned(),
                    allow: true,
                });
            }
            _ => {}
        }
    }
    flush(&mut groups, &mut current_agents, &mut current_rules);
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_allows_everything() {
        let policy = RobotsPolicy::parse("");
        assert!(policy.is_allowed("/anything"));
    }

    #[test]
    fn a_wildcard_disallow_blocks_the_matching_prefix() {
        let policy = RobotsPolicy::parse("User-agent: *\nDisallow: /private\n");
        assert!(!policy.is_allowed("/private/data"));
        assert!(policy.is_allowed("/public/data"));
    }

    #[test]
    fn the_longest_matching_rule_wins() {
        let policy = RobotsPolicy::parse("User-agent: *\nDisallow: /docs\nAllow: /docs/public\n");
        assert!(!policy.is_allowed("/docs/private"));
        assert!(policy.is_allowed("/docs/public/page"));
    }

    #[test]
    fn a_named_group_for_darkharness_takes_priority_over_the_wildcard() {
        let policy = RobotsPolicy::parse(
            "User-agent: *\nDisallow: /\n\nUser-agent: darkharness\nDisallow:\n",
        );
        assert!(policy.is_allowed("/anything"));
    }

    #[test]
    fn a_named_group_for_darkharness_matches_case_insensitively() {
        let policy = RobotsPolicy::parse("User-agent: DarkHarness\nAllow: /\n");
        assert!(policy.is_allowed("/anything"));
    }

    #[test]
    fn two_user_agent_lines_share_the_following_rules() {
        let policy = RobotsPolicy::parse(
            "User-agent: someoneelse\nUser-agent: darkharness\nDisallow: /blocked\n",
        );
        assert!(!policy.is_allowed("/blocked/page"));
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let policy = RobotsPolicy::parse(
            "# comment\nUser-agent: *\n\n# another comment\nDisallow: /x # inline\n",
        );
        assert!(!policy.is_allowed("/x/y"));
        assert!(policy.is_allowed("/z"));
    }

    struct FailingFetcher;
    impl Fetcher for FailingFetcher {
        fn fetch(&self, _url: &str) -> Result<Vec<u8>> {
            Err(dark_contract::Error::new(
                dark_contract::ErrCode::ToolFailed,
                "no robots.txt",
            ))
        }
    }

    #[test]
    fn a_fetch_failure_parses_as_an_all_allowing_policy() {
        let policy = RobotsPolicy::fetch(&FailingFetcher, "https://example.com/page").unwrap();
        assert!(policy.is_allowed("/anything"));
    }
}
