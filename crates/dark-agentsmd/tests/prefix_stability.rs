//! Acceptance test for task unit K1.
//!
//! "A turn that touches three subtrees produces an identical prefix across
//! its round-trips. The two late subtrees appear in the tail."
//!
//! The turn's working set names only one subtree up front (the claimed
//! ticket's scope). Two more subtrees come into play only as later
//! round-trips touch them — the way a tool call might edit a file the
//! ticket scope never mentioned. Rule 22 says the prefix is fixed at the
//! start of the turn; Rule 23 says a file discovered later goes in the
//! tail, with a notice, and never edits the prefix.

use std::fs;
use std::path::PathBuf;

use dark_agentsmd::{
    AgentsMdConfig, TailTracker, WorkingSet, discover_for_tail, resolve, tail_text,
};
use tempfile::TempDir;

fn count_words(text: &str) -> usize {
    text.split_whitespace().count()
}

struct Repo {
    _tmp: TempDir,
    home: PathBuf,
    root: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let home = tmp.path().join("home");
        let root = tmp.path().join("repo");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&root).unwrap();
        Self {
            _tmp: tmp,
            home,
            root,
        }
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
}

#[test]
fn a_turn_touching_three_subtrees_keeps_an_identical_prefix_and_tails_the_late_two() {
    let repo = Repo::new();
    repo.write("AGENTS.md", "root policy");
    // The subtree the turn starts in: part of the prefix from the start.
    repo.write("crates/alpha/AGENTS.md", "alpha subtree policy");
    // Two subtrees the ticket scope never named. A tool call reaches them
    // only partway through the turn.
    repo.write("crates/beta/AGENTS.md", "beta subtree policy");
    repo.write("crates/gamma/AGENTS.md", "gamma subtree policy");

    let config = AgentsMdConfig::default();

    let mut working_set = WorkingSet::new();
    working_set
        .ticket_scope
        .push(repo.root.join("crates/alpha"));

    // Resolve once, at the start of the turn. This is the only call to
    // `resolve` for the whole turn — everything after this point works
    // off the same `ResolvedChain` value.
    let chain = resolve(&repo.home, &repo.root, &working_set, &config, &count_words).unwrap();

    let prefix_at_start = chain.prefix_text();
    assert!(prefix_at_start.contains("alpha subtree policy"));
    assert!(!prefix_at_start.contains("beta subtree policy"));
    assert!(!prefix_at_start.contains("gamma subtree policy"));

    let mut tracker = TailTracker::new();
    let mut round_trip_prefixes = Vec::new();
    let mut tails = Vec::new();

    // Round-trip 1: a tool call reads a file in `alpha`. Already in the
    // prefix, so it adds nothing to the tail.
    round_trip_prefixes.push(chain.prefix_text());
    tails.push(tail_text(
        &discover_for_tail(
            &chain,
            &mut tracker,
            &repo.root,
            &repo.root.join("crates/alpha"),
            &config,
            &count_words,
        )
        .unwrap(),
    ));

    // Round-trip 2: a tool call touches `beta`, unseen at turn start.
    round_trip_prefixes.push(chain.prefix_text());
    tails.push(tail_text(
        &discover_for_tail(
            &chain,
            &mut tracker,
            &repo.root,
            &repo.root.join("crates/beta"),
            &config,
            &count_words,
        )
        .unwrap(),
    ));

    // Round-trip 3: a tool call touches `gamma`, also unseen at turn
    // start.
    round_trip_prefixes.push(chain.prefix_text());
    tails.push(tail_text(
        &discover_for_tail(
            &chain,
            &mut tracker,
            &repo.root,
            &repo.root.join("crates/gamma"),
            &config,
            &count_words,
        )
        .unwrap(),
    ));

    // The done-when condition: every round-trip in the turn sees the exact
    // same prefix bytes.
    for prefix in &round_trip_prefixes {
        assert_eq!(
            prefix, &prefix_at_start,
            "the prefix must not change during the turn"
        );
    }

    // The two late subtrees never touched the prefix...
    for prefix in &round_trip_prefixes {
        assert!(!prefix.contains("beta subtree policy"));
        assert!(!prefix.contains("gamma subtree policy"));
    }

    // ...they appear in the tail instead, exactly where the harness found
    // them, with a notice each.
    assert!(
        tails[0].is_empty(),
        "alpha was already in the prefix; round-trip 1 adds nothing to the tail"
    );
    assert!(tails[1].contains("beta subtree policy"));
    assert!(
        tails[1].contains("crates"),
        "the notice should name the subtree"
    );
    assert!(tails[2].contains("gamma subtree policy"));

    // A round-trip that revisits a subtree already noticed does not repeat
    // the notice or the content.
    let repeat = tail_text(
        &discover_for_tail(
            &chain,
            &mut tracker,
            &repo.root,
            &repo.root.join("crates/beta"),
            &config,
            &count_words,
        )
        .unwrap(),
    );
    assert!(
        repeat.is_empty(),
        "beta was already noticed in round-trip 2; it must not repeat"
    );
}

#[test]
fn prefix_text_is_byte_identical_across_many_repeated_calls() {
    let repo = Repo::new();
    repo.write("AGENTS.md", "root policy");
    repo.write("crates/alpha/AGENTS.md", "alpha policy");

    let config = AgentsMdConfig::default();
    let mut working_set = WorkingSet::new();
    working_set
        .ticket_scope
        .push(repo.root.join("crates/alpha"));

    let chain = resolve(&repo.home, &repo.root, &working_set, &config, &count_words).unwrap();
    let first = chain.prefix_text();
    for _ in 0..20 {
        assert_eq!(chain.prefix_text(), first);
    }
}
