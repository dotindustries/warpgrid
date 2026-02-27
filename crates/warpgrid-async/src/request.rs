use bytes::Bytes;

use crate::body::{ByteStream, ChunkedBytesStream, EmptyFallibleStream, DEFAULT_CHUNK_SIZE};
use crate::header::HeaderMap;

/// An incoming HTTP request with support for streaming body access.
///
/// Request bodies can be accessed either as a complete buffer via
/// [`body_bytes()`](Request::body_bytes) or as a stream of chunks via
/// [`body_stream()`](Request::body_stream). The streaming path is
/// designed for bounded-memory processing of large payloads.
///
/// # Memory Guarantee
///
/// `body_stream()` yields chunks via `Bytes::slice()`, which shares the
/// underlying allocation without copying. The pull-based `Stream` trait
/// ensures at most one chunk is in flight at a time. During a transform
/// (map) operation where both input and output chunks exist, peak
/// overhead is 2× the chunk size — regardless of total body size.
pub struct Request {
    method: String,
    uri: String,
    headers: HeaderMap,
    body: Bytes,
}

impl Request {
    /// Create a request with a buffered body.
    pub fn new(
        method: impl Into<String>,
        uri: impl Into<String>,
        headers: HeaderMap,
        body: impl Into<Bytes>,
    ) -> Self {
        Self {
            method: method.into(),
            uri: uri.into(),
            headers,
            body: body.into(),
        }
    }

    /// Create a request with an empty body.
    pub fn empty(method: impl Into<String>, uri: impl Into<String>, headers: HeaderMap) -> Self {
        Self {
            method: method.into(),
            uri: uri.into(),
            headers,
            body: Bytes::new(),
        }
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Direct access to the body buffer.
    pub fn body_bytes(&self) -> &Bytes {
        &self.body
    }

    /// Consume the request body as a stream of byte chunks.
    ///
    /// The buffer is yielded in chunks of [`DEFAULT_CHUNK_SIZE`] (64 KB)
    /// using zero-copy `Bytes::slice()`. Each call creates an independent
    /// stream from the same underlying buffer (cheap — just an `Arc`
    /// refcount increment).
    ///
    /// # Memory Guarantee
    ///
    /// The returned stream yields one chunk at a time. Combined with the
    /// pull-based `Stream` trait, at most one chunk is in flight per poll —
    /// enabling bounded-memory processing of arbitrarily large bodies.
    pub fn body_stream(&self) -> ByteStream {
        self.body_stream_chunked(DEFAULT_CHUNK_SIZE)
    }

    /// Like [`body_stream()`](Request::body_stream) but with a custom chunk size.
    pub fn body_stream_chunked(&self, chunk_size: usize) -> ByteStream {
        if self.body.is_empty() {
            return Box::pin(EmptyFallibleStream);
        }
        Box::pin(ChunkedBytesStream::new(self.body.clone(), chunk_size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::{Context, Poll};

    fn poll_all(mut stream: ByteStream) -> Vec<Bytes> {
        let mut chunks = Vec::new();
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);
        loop {
            match stream.as_mut().poll_next(&mut cx) {
                Poll::Ready(Some(Ok(chunk))) => chunks.push(chunk),
                Poll::Ready(Some(Err(e))) => panic!("unexpected error: {e}"),
                Poll::Ready(None) => break,
                Poll::Pending => panic!("should not pend"),
            }
        }
        chunks
    }

    #[test]
    fn request_accessors() {
        let mut headers = HeaderMap::new();
        headers.insert("Host", "example.com");

        let req = Request::new("GET", "/users?page=1", headers, "hello");
        assert_eq!(req.method(), "GET");
        assert_eq!(req.uri(), "/users?page=1");
        assert_eq!(req.headers().get("host"), Some("example.com"));
        assert_eq!(req.body_bytes().as_ref(), b"hello");
    }

    #[test]
    fn request_empty() {
        let req = Request::empty("HEAD", "/", HeaderMap::new());
        assert!(req.body_bytes().is_empty());
    }

    #[test]
    fn body_stream_empty_body() {
        let req = Request::empty("GET", "/", HeaderMap::new());
        let chunks = poll_all(req.body_stream());
        assert!(chunks.is_empty());
    }

    #[test]
    fn body_stream_small_body_single_chunk() {
        let req = Request::new("POST", "/", HeaderMap::new(), vec![1u8; 100]);
        let chunks = poll_all(req.body_stream()); // default 64 KB chunk
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 100);
    }

    #[test]
    fn body_stream_chunked_yields_correct_sizes() {
        let req = Request::new("POST", "/", HeaderMap::new(), vec![0xAB; 4096]);
        let chunks = poll_all(req.body_stream_chunked(1024));

        assert_eq!(chunks.len(), 4);
        for chunk in &chunks {
            assert_eq!(chunk.len(), 1024);
        }
    }

    #[test]
    fn body_stream_preserves_content() {
        let data: Vec<u8> = (0..=255).cycle().take(10_000).collect();
        let req = Request::new("POST", "/", HeaderMap::new(), data.clone());

        let chunks = poll_all(req.body_stream_chunked(1024));
        let reassembled: Vec<u8> = chunks.iter().flat_map(|c| c.iter().copied()).collect();
        assert_eq!(reassembled, data);
    }

    #[test]
    fn body_stream_callable_multiple_times() {
        let req = Request::new("POST", "/", HeaderMap::new(), vec![1u8; 100]);

        let chunks1 = poll_all(req.body_stream());
        let chunks2 = poll_all(req.body_stream());

        assert_eq!(chunks1.len(), 1);
        assert_eq!(chunks2.len(), 1);
        assert_eq!(chunks1[0], chunks2[0]);
    }
}
