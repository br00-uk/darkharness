//! The `ingest` stage: task unit `G2`.
//!
//! Every adapter in this module converts one source-specific format into
//! [`Document`] values, the one shape the rest of the pipeline (licence
//! gating, then `crate::chunk`) works with.
//!
//! | Adapter | Source | Module |
//! | --- | --- | --- |
//! | `llms-txt` | An `llms.txt` or `llms-full.txt` file. Preferred. | [`llms_txt`] |
//! | `docsrs` | `cargo doc --output-format json`. | [`docsrs`] |
//! | `sitemap` | A sitemap and its HTML pages. | [`sitemap`] |
//! | `git` | A repository at a tag. | [`git`] |
//! | `localdir` | A local directory. | [`localdir`] |
//! | `openapi` | An `OpenAPI` document, one document per operation. | [`openapi`] |
//! | `manpage` | Manual pages. | [`manpage`] |
//!
//! ## Rule 13 against Rule 16, and how `fetch` resolves it
//!
//! G2 tells the `sitemap` and `llms-txt` adapters to "fetch through
//! `dark-airlock`". Rule 13 says only `dark-airlock` may construct an HTTP
//! client. Rule 16 says `dark-lexicon` depends on `dark-contract` and its
//! own storage crates only, so it cannot add `dark-airlock` as a
//! dependency to reach that client — `cargo xtask check-deps` and
//! `cargo deny` both fail the build if it tries. The module docs of
//! [`fetch`] work through the resolution: a small [`fetch::Fetcher`] trait
//! that `dark-lexicon` defines and a caller elsewhere in the workspace
//! implements over `dark_airlock::Client`.
//!
//! ## Licence gate
//!
//! Rule 26: "`dark pack add` refuses a source with no licence." See
//! [`licence`]. `licence_gate.rs` (a crate-level integration test) proves
//! the refusal.
//!
//! ## Untrusted content
//!
//! Rule 36 treats fetched HTML as untrusted. [`html`] never executes
//! anything in a fetched page: `<script>` and `<style>` element content is
//! dropped, not evaluated, and [`fetch::fetch_capped`] enforces a size cap
//! so a hostile or broken server cannot exhaust memory. Every network
//! adapter goes through [`fetch::fetch_capped`], never a raw
//! [`fetch::Fetcher::fetch`] call, for that reason.

pub mod docsrs;
pub mod document;
pub mod fetch;
pub mod git;
pub mod html;
pub mod licence;
pub mod llms_txt;
pub mod localdir;
pub mod manpage;
pub mod markdown;
pub mod openapi;
pub mod robots;
pub mod sitemap;

pub use document::{Document, Heading};
pub use fetch::{Fetcher, RateLimiter};
pub use licence::Licence;
pub use robots::RobotsPolicy;
