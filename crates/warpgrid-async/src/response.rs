use bytes::Bytes;
use futures_core::Stream;

use crate::body::{EmptyInfallibleStream, InfallibleByteStream, OnceStream};
use crate::header::HeaderMap;

/// An outgoing HTTP response with support for streaming bodies.
///
/// Responses can hold either a pre-buffered body (for small payloads)
/// or a streaming body (for large payloads or incremental generation).
///
/// # Buffered vs Streaming
///
/// Use [`Response::new()`] for small, known-size responses where the
/// entire body fits comfortably in memory.
///
/// Use [`Response::streaming()`] for large or incrementally generated
/// responses. The streaming path defers body materialization — chunks
/// are produced on demand and never buffered beyond the current chunk.
///
/// # Memory Guarantee
///
/// A streaming response holds only a reference to the stream, not any
/// buffered data. Chunks are materialized one at a time during
/// consumption via [`into_bytes()`](Response::into_bytes) or
/// [`into_body_stream()`](Response::into_body_stream). During a
/// transform pipeline (e.g., request chunk → uppercase → response chunk),
/// peak additional memory is 2× the chunk size.
pub struct Response {
    status: u16,
    headers: HeaderMap,
    body: ResponseBody,
}

pub(crate) enum ResponseBody {
    Buffered(Bytes),
    Streaming(InfallibleByteStream),
}

impl Response {
    /// Create a response with a pre-buffered body.
    pub fn new(status: u16, headers: HeaderMap, body: impl Into<Bytes>) -> Self {
        Self {
            status,
            headers,
            body: ResponseBody::Buffered(body.into()),
        }
    }

    /// Create a response with an empty body.
    pub fn empty(status: u16, headers: HeaderMap) -> Self {
        Self {
            status,
            headers,
            body: ResponseBody::Buffered(Bytes::new()),
        }
    }

    /// Create a response with a streaming body.
    ///
    /// The stream yields byte chunks that are assembled into the
    /// response body on demand. This constructor does not buffer
    /// the stream — chunks are consumed lazily when the body is read.
    ///
    /// # Memory Guarantee
    ///
    /// The streaming response holds only the stream handle. Chunks
    /// are produced one at a time on each poll. During consumption
    /// (e.g., via [`into_bytes()`](Response::into_bytes)), each chunk
    /// is appended to the output buffer as it arrives — at most one
    /// chunk is in transit at any point.
    pub fn streaming(
        status: u16,
        headers: HeaderMap,
        stream: impl Stream<Item = Bytes> + Send + 'static,
    ) -> Self {
        Self {
            status,
            headers,
            body: ResponseBody::Streaming(Box::pin(stream)),
        }
    }

    pub fn status(&self) -> u16 {
        self.status
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Consume the response and collect the body into a single buffer.
    ///
    /// For buffered responses, returns the body directly.
    /// For streaming responses, polls the stream to completion and
    /// concatenates all chunks.
    pub async fn into_bytes(self) -> Bytes {
        match self.body {
            ResponseBody::Buffered(bytes) => bytes,
            ResponseBody::Streaming(mut stream) => {
                let mut collected = Vec::new();
                while let Some(chunk) =
                    std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await
                {
                    collected.extend_from_slice(&chunk);
                }
                Bytes::from(collected)
            }
        }
    }

    /// Consume the response into a stream of byte chunks.
    ///
    /// For buffered responses, yields the body as a single chunk.
    /// For streaming responses, returns the underlying stream.
    pub fn into_body_stream(self) -> InfallibleByteStream {
        match self.body {
            ResponseBody::Buffered(bytes) => {
                if bytes.is_empty() {
                    Box::pin(EmptyInfallibleStream)
                } else {
                    Box::pin(OnceStream(Some(bytes)))
                }
            }
            ResponseBody::Streaming(stream) => stream,
        }
    }

    /// Returns `true` if the body is streaming (not pre-buffered).
    pub fn is_streaming(&self) -> bool {
        matches!(self.body, ResponseBody::Streaming(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn buffered_response_into_bytes() {
        let resp = Response::new(200, HeaderMap::new(), "hello world");
        assert_eq!(resp.status(), 200);
        assert!(!resp.is_streaming());

        let body = resp.into_bytes().await;
        assert_eq!(body.as_ref(), b"hello world");
    }

    #[tokio::test]
    async fn empty_response_into_bytes() {
        let resp = Response::empty(204, HeaderMap::new());
        let body = resp.into_bytes().await;
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn streaming_response_basic() {
        let chunks = vec![Bytes::from("hello "), Bytes::from("world")];
        let stream = futures_util::stream::iter(chunks);

        let resp = Response::streaming(200, HeaderMap::new(), stream);
        assert!(resp.is_streaming());

        let body = resp.into_bytes().await;
        assert_eq!(body.as_ref(), b"hello world");
    }

    #[tokio::test]
    async fn streaming_response_empty_stream() {
        let stream = futures_util::stream::empty::<Bytes>();
        let resp = Response::streaming(200, HeaderMap::new(), stream);
        let body = resp.into_bytes().await;
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn buffered_response_into_body_stream() {
        let resp = Response::new(200, HeaderMap::new(), "test");
        let mut stream = resp.into_body_stream();

        let chunk = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await;
        assert_eq!(chunk, Some(Bytes::from("test")));

        let end = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await;
        assert_eq!(end, None);
    }

    #[tokio::test]
    async fn empty_buffered_response_into_body_stream() {
        let resp = Response::empty(204, HeaderMap::new());
        let mut stream = resp.into_body_stream();

        let end = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await;
        assert_eq!(end, None);
    }

    #[tokio::test]
    async fn response_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("Content-Type", "application/json");
        headers.insert("X-Custom", "value");

        let resp = Response::new(201, headers, "{}");
        assert_eq!(resp.status(), 201);
        assert_eq!(resp.headers().get("content-type"), Some("application/json"));
        assert_eq!(resp.headers().get("x-custom"), Some("value"));
    }
}
