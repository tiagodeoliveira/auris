//! Wire protocol types for the Auris server + clients.
//!
//! Generated from `packages/contract/proto/auris/v1/*.proto`
//! at build time via `prost-build`. The generated code lives in `OUT_DIR`
//! and is included verbatim below — there's no committed Rust output to
//! drift from the .proto sources.
//!
//! Wire format is binary protobuf over WebSocket binary frames. The
//! protocol version (today: 1) lives in `Snapshot.protocol_version`;
//! bump it on truly breaking schema changes (see `events.proto` header
//! for the evolution rules).
//!
//! Usage from the server:
//! ```ignore
//! use auris_contract::v1::{Intent, Event};
//! use prost::Message;
//!
//! let bytes = intent.encode_to_vec();      // → wire bytes
//! let parsed = Intent::decode(&bytes[..])?; // ← wire bytes
//! ```

#![allow(clippy::doc_lazy_continuation, clippy::doc_overindented_list_items)]

/// All v1 protocol types. Re-exported under a versioned module so a
/// future `v2` can land alongside without a breaking rename of every
/// import site.
pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/auris.v1.rs"));
}

/// The wire-protocol version this crate's generated types speak.
/// Mirrors `Snapshot.protocol_version` for callers that need the
/// constant outside of an event payload.
pub const PROTOCOL_VERSION: u32 = 1;
