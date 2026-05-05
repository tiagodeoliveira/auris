//! Meeting Companion server library. See `docs/ARCHITECTURE.md` for
//! the system overview.

pub mod audio;
pub mod contract;
pub mod db;
pub mod extraction;
pub mod llm;
pub mod mnemo;
pub mod state;
pub mod stt;
pub mod summarizer;
pub mod ws;

pub use ws::run_server;
