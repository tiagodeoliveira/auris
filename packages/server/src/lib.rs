//! Meeting Companion server library.
//!
//! See `docs/specs/server.md` for the component specification.

pub mod audio;
pub mod contract;
pub mod extraction;
pub mod llm;
pub mod state;
pub mod stt;
pub mod summarizer;
pub mod ws;

pub use ws::run_server;
