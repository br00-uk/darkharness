//! Documentation packs: ingest, chunk, index, and retrieve.
//!
//! A pack holds one library's documentation in a portable, verifiable
//! directory. The lexical index is the fallback and must work on its own,
//! because an embedding model is not always resident. See task units `G1` to
//! `G5`.

pub mod chunk;
pub mod cli;
pub mod index;
pub mod ingest;
pub mod pack;
pub mod retrieve;
pub mod tools;
