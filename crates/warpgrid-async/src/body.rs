//! Streaming body primitives.
//!
//! Provides the [`ChunkedBytesStream`] adapter that yields a `Bytes` buffer
//! in fixed-size chunks without copying (via `Bytes::slice()`), and
//! minimal stream helpers to avoid pulling in `futures-util` as a
//! runtime dependency.

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_core::Stream;

use crate::Error;

/// Default chunk size for breaking buffered bodies into stream chunks (64 KB).
pub const DEFAULT_CHUNK_SIZE: usize = 64 * 1024;

/// A type-erased, fallible async stream of byte chunks.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, Error>> + Send>>;

/// A type-erased, infallible async stream of byte chunks.
pub type InfallibleByteStream = Pin<Box<dyn Stream<Item = Bytes> + Send>>;

/// Yields a `Bytes` buffer in fixed-size chunks without copying.
///
/// Uses `Bytes::slice()` for zero-copy sub-slicing backed by the same
/// reference-counted allocation. At any point, only the current chunk
/// is yielded — no additional buffering beyond the original data.
///
/// # Memory Guarantee
///
/// Each `poll_next` returns a single `Bytes::slice()` and advances the
/// offset. The slice shares the original allocation's refcount, so no
/// per-chunk allocation occurs. The streaming consumer holds at most
/// one chunk reference at a time.
pub(crate) struct ChunkedBytesStream {
    buf: Bytes,
    chunk_size: usize,
    offset: usize,
}

impl ChunkedBytesStream {
    pub fn new(buf: Bytes, chunk_size: usize) -> Self {
        assert!(chunk_size > 0, "chunk_size must be > 0");
        Self {
            buf,
            chunk_size,
            offset: 0,
        }
    }
}

impl Stream for ChunkedBytesStream {
    type Item = Result<Bytes, Error>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.offset >= this.buf.len() {
            return Poll::Ready(None);
        }
        let end = std::cmp::min(this.offset + this.chunk_size, this.buf.len());
        let chunk = this.buf.slice(this.offset..end);
        this.offset = end;
        Poll::Ready(Some(Ok(chunk)))
    }
}

// ── Minimal stream helpers ──────────────────────────────────────────

/// An empty fallible stream that immediately returns `None`.
pub(crate) struct EmptyFallibleStream;

impl Stream for EmptyFallibleStream {
    type Item = Result<Bytes, Error>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(None)
    }
}

/// An empty infallible stream that immediately returns `None`.
pub(crate) struct EmptyInfallibleStream;

impl Stream for EmptyInfallibleStream {
    type Item = Bytes;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(None)
    }
}

/// An infallible stream that yields a single `Bytes` value then ends.
pub(crate) struct OnceStream(pub Option<Bytes>);

impl Stream for OnceStream {
    type Item = Bytes;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.get_mut().0.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_sync(stream: &mut ChunkedBytesStream) -> Vec<Bytes> {
        let mut chunks = Vec::new();
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);
        loop {
            match Pin::new(&mut *stream).poll_next(&mut cx) {
                Poll::Ready(Some(Ok(chunk))) => chunks.push(chunk),
                Poll::Ready(Some(Err(e))) => panic!("unexpected error: {e}"),
                Poll::Ready(None) => break,
                Poll::Pending => panic!("ChunkedBytesStream should never pend"),
            }
        }
        chunks
    }

    #[test]
    fn chunked_stream_exact_division() {
        let data = Bytes::from(vec![0xAA; 4096]);
        let mut stream = ChunkedBytesStream::new(data, 1024);
        let chunks = collect_sync(&mut stream);

        assert_eq!(chunks.len(), 4);
        for chunk in &chunks {
            assert_eq!(chunk.len(), 1024);
            assert!(chunk.iter().all(|&b| b == 0xAA));
        }
    }

    #[test]
    fn chunked_stream_remainder() {
        let data = Bytes::from(vec![0xBB; 3000]);
        let mut stream = ChunkedBytesStream::new(data, 1024);
        let chunks = collect_sync(&mut stream);

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 1024);
        assert_eq!(chunks[1].len(), 1024);
        assert_eq!(chunks[2].len(), 952); // remainder
    }

    #[test]
    fn chunked_stream_smaller_than_chunk() {
        let data = Bytes::from(vec![0xCC; 100]);
        let mut stream = ChunkedBytesStream::new(data, 1024);
        let chunks = collect_sync(&mut stream);

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 100);
    }

    #[test]
    fn chunked_stream_empty_body() {
        let data = Bytes::new();
        let mut stream = ChunkedBytesStream::new(data, 1024);
        let chunks = collect_sync(&mut stream);

        assert_eq!(chunks.len(), 0);
    }

    #[test]
    fn chunked_stream_single_byte_chunks() {
        let data = Bytes::from(vec![1, 2, 3, 4, 5]);
        let mut stream = ChunkedBytesStream::new(data, 1);
        let chunks = collect_sync(&mut stream);

        assert_eq!(chunks.len(), 5);
        assert_eq!(chunks[0][0], 1);
        assert_eq!(chunks[4][0], 5);
    }

    #[test]
    fn chunked_stream_is_zero_copy() {
        let original = Bytes::from(vec![0xFF; 8192]);
        let ptr_before = original.as_ptr();

        let mut stream = ChunkedBytesStream::new(original, 4096);
        let chunks = collect_sync(&mut stream);

        // Bytes::slice() shares the same allocation — first chunk
        // should point into the same backing memory.
        assert_eq!(chunks[0].as_ptr(), ptr_before);
        assert_eq!(
            chunks[1].as_ptr(),
            unsafe { ptr_before.add(4096) }
        );
    }

    #[test]
    #[should_panic(expected = "chunk_size must be > 0")]
    fn chunked_stream_zero_chunk_size_panics() {
        let _ = ChunkedBytesStream::new(Bytes::new(), 0);
    }

    #[test]
    fn empty_fallible_stream_returns_none() {
        let mut stream = EmptyFallibleStream;
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);
        assert!(Pin::new(&mut stream).poll_next(&mut cx).is_ready());
    }

    #[test]
    fn once_stream_yields_then_ends() {
        let mut stream = OnceStream(Some(Bytes::from("hello")));
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);

        match Pin::new(&mut stream).poll_next(&mut cx) {
            Poll::Ready(Some(b)) => assert_eq!(b, "hello"),
            other => panic!("expected Some, got {other:?}"),
        }

        assert!(matches!(
            Pin::new(&mut stream).poll_next(&mut cx),
            Poll::Ready(None)
        ));
    }
}
