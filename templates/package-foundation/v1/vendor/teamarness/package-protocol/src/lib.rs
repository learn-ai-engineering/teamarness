//! Rust adapter for the language-neutral Package Process Protocol.
//!
//! The normative contract lives in `contracts/package-process/v1`. This crate
//! supplies framing, strict JSON decoding, version selection, and state checks
//! for Rust hosts and Package Backends without adding host policy.

#![forbid(unsafe_code)]

mod codec;
mod state;
mod version;

pub use codec::{
    Frame, FrameDecoder, FrameKind, ProtocolError, decode_frame, decode_payload, encode_frame,
};
pub use serde_json::Value as JsonValue;
pub use state::{ContractState, Direction};
pub use version::{ProtocolRange, ProtocolVersion, negotiate_version};

pub const MAX_FRAME_BYTES: usize = 1_048_576;
pub const MAX_IN_FLIGHT_EACH_DIRECTION: usize = 32;
pub const MAX_WRITER_FRAMES: usize = 64;
pub const MAX_WRITER_BYTES: usize = 8_388_608;
pub const INITIAL_STREAM_CREDIT: usize = 32;
