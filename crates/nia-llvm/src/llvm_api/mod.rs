// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed wrappers around the LLVM C API.
//!
//! The wrappers keep raw LLVM references inside small newtypes and expose a
//! narrower API to codegen. Most functions remain thin FFI calls, but centralize
//! ownership/disposal and reduce raw-pointer usage in higher-level modules.

mod base;
mod builder;
mod context;
mod debug;
mod module;
mod target;

pub use base::*;
pub use builder::*;
pub use context::*;
pub use debug::*;
pub use module::*;
pub use target::*;
