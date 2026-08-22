//! The terminal application: events in, intents out.
//!
//! This crate depends on `dark-contract` only (Rule 14). It renders [`Event`]
//! values and sends [`Intent`] values; it never reaches into the runtime.
//!
//! [`Event`]: dark_contract::Event
//! [`Intent`]: dark_contract::Intent

pub mod anim;
pub mod app;
pub mod replay;
pub mod theme;
pub mod views;
