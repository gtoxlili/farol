//! Raw bindings to the ESP-GSP 1.9.0 facade (`esp_gsp.h`) and the scene
//! constants gspc generates for `components/app_logic/ui/main.json`.
//!
//! Two checked-in files, both written by `cargo xtask gen-gsp` from the same
//! headers: `bindings_riscv32.rs` for the board (ilp32f) and `bindings_host.rs`
//! for 64-bit build hosts, where the simulator backend and the tests run.
//! Regenerate whenever the GSP version or the scene's named objects, binds or
//! callbacks change; the IDF and simulator builds fail while they are stale.
#![no_std]
#![allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code
)]
#![allow(clippy::all, unsafe_op_in_unsafe_fn)]

#[cfg(target_arch = "riscv32")]
include!("bindings_riscv32.rs");

#[cfg(all(not(target_arch = "riscv32"), target_pointer_width = "64"))]
include!("bindings_host.rs");

#[cfg(all(not(target_arch = "riscv32"), not(target_pointer_width = "64")))]
compile_error!("gsp_sys has bindings for riscv32 (board) and 64-bit hosts only");
