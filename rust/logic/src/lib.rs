//! Farol's logic, shared by the board (`platform`) and the simulator backend
//! (`host`). GSP renders the scene and runs every motion; this crate owns the
//! state and decides what changes.
//!
//! - [`App`]: the state machine. [`App::handle`] takes [`sys::Event`]s,
//!   [`App::poll`] does time-driven work, [`App::next_deadline_ms`] says when
//!   `poll` is next due. All three run on the one thread that owns the `App`.
//! - [`paseo::Session`]: the Paseo client session (hello with the daemon
//!   password, plugin RPC long-poll, actions), a pure state machine.
//! - [`model`]: farol/1, the data the Farol plugin sends.
//! - [`ui::Ui`] and [`sys::Sys`]: the scene setters and platform effects the
//!   logic drives; the platforms implement them, the tests record them.
//!
//! The crate is `no_std` with `alloc`; the platforms supply time.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

/// Firmware version, as the board reports it to the plugin.
pub const FIRMWARE: &str = env!("CARGO_PKG_VERSION");

mod app;
mod demo;
pub mod model;
pub mod paseo;
pub mod qr;
mod scene;
pub mod sys;
pub mod text;
pub mod ui;
mod view;

pub use app::{App, Overlay};
