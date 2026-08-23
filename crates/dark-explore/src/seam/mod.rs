//! Metrics and seams: the places where a change has a bounded effect.
//!
//! See task unit `F3`.
//!
//! # Determinism
//!
//! Every stage here runs without a model and must produce identical bytes
//! for the same commit and configuration (Rules 29 to 32). Each algorithm
//! that depends on visit order visits nodes in node-index order, which
//! `graph::build` fixed to sorted path order using F1's byte comparator.

pub mod assemble;
pub mod betweenness;
pub mod cochange;
pub mod community;
pub mod metrics;
pub mod score;
pub mod structure;

pub use assemble::{
    SeamAnalysis, SymbolBlast, analyse, blast_for_symbol, is_test_path, symbol_scores,
};
pub use betweenness::Betweenness;
pub use cochange::{CoChange, Window};
pub use community::Communities;
pub use metrics::NodeMetrics;
pub use score::{BlastRadius, ScoredSeam, Terms, Weights, blast_radius, rank};
pub use structure::{Bridge, Structure};
