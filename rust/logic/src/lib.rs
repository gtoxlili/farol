//! Application logic, shared by the board (`platform`) and the simulator
//! backend (`host`). GSP renders the scene and runs every motion; this crate
//! owns the state and decides what changes.
//!
//! - [`App`]: the state machine. [`App::handle`] takes decoded [`Event`]s,
//!   [`App::poll`] does time-driven work, [`App::next_deadline_ms`] says when
//!   `poll` is next due. All three run on the one thread that owns the `App`.
//! - [`ui::Ui`]: the setters the logic uses. [`ui::GspUi`] implements them over
//!   the GSP C API; tests record them instead.
//!
//! The crate is `no_std` and allocation-free; the platforms supply time.
#![cfg_attr(not(test), no_std)]

mod app;
mod text;
pub mod ui;

pub use app::{App, Event};
