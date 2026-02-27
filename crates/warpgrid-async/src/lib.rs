//! WarpGrid async handler types with streaming body support.
//!
//! Provides [`Request`] and [`Response`] types that support both buffered
//! and streaming body access. The streaming path is designed for
//! bounded-memory processing of large payloads.
//!
//! # Streaming Model
//!
//! Request bodies arrive as byte buffers from the WIT boundary. The
//! [`Request::body_stream()`] method yields the buffer in fixed-size
//! chunks via zero-copy `Bytes::slice()`, enabling incremental
//! processing without copying the entire body.
//!
//! Response bodies can be constructed from a `Stream<Item = Bytes>`,
//! allowing handlers to produce output incrementally. The response
//! materializes chunks on demand — no pre-buffering.
//!
//! # Memory Guarantee
//!
//! The streaming API is pull-based (via `futures_core::Stream`). At any
//! point during stream processing, at most one chunk is being yielded
//! by the producer. During a transform (map) operation, both the input
//! chunk and output chunk may exist simultaneously, giving a peak
//! overhead of **2× the chunk size**. This bound holds regardless of
//! total body size.

pub(crate) mod body;
mod error;
mod header;
mod request;
mod response;

pub use body::{ByteStream, InfallibleByteStream, DEFAULT_CHUNK_SIZE};
pub use error::Error;
pub use header::{Header, HeaderMap};
pub use request::Request;
pub use response::Response;
