//! Auris server library. See `docs/ARCHITECTURE.md` for
//! the system overview.

pub mod agent;
pub mod api;
pub mod audio;
pub mod auth;
pub mod boot;
pub mod config;
pub mod context;
pub mod llm;
pub mod mnemo;
pub mod observability;
pub mod pdf;
pub mod protocol;
pub mod session;
pub mod storage;
pub mod stt;
pub mod util;
pub mod workers;
pub mod ws;

pub use boot::run_server;
