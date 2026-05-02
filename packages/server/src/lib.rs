//! Meeting Companion server library.
//!
//! See `docs/specs/server.md` for the component specification.

pub mod contract;
pub mod extraction;
pub mod mock;
pub mod state;
pub mod ws;

pub use ws::run_server;
