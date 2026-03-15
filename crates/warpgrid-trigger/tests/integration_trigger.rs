//! Integration tests for warpgrid-trigger.
//!
//! Validates the HTTP trigger server, echo handler, and conversion
//! utilities. Uses ephemeral ports and real TCP connections.

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use http_body_util::{BodyExt, Empty};
use hyper_util::rt::TokioExecutor;

use warpgrid_trigger::convert;
use warpgrid_trigger::handler::{HttpTrigger, RequestHandler, echo_handler};

// ── Helpers ────────────────────────────────────────────────────────

/// Start the trigger on an ephemeral port and return the bound address
/// along with the shutdown sender.
async fn start_echo_server() -> (SocketAddr, tokio::sync::watch::Sender<bool>) {
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

    let (tx, rx) = tokio::sync::watch::channel(false);

    // Bind to get an ephemeral port, then start the trigger on it.
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let bound = listener.local_addr().unwrap();
    drop(listener);

    let trigger = HttpTrigger::new(bound, echo_handler());
    tokio::spawn(async move {
        let _ = trigger.serve(rx).await;
    });

    // Give the server a moment to bind.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    (bound, tx)
}

// ── 1. Trigger serves echo handler on ephemeral port ──────────────

#[tokio::test]
async fn echo_handler_serves_on_ephemeral_port() {
    let (addr, shutdown_tx) = start_echo_server().await;

    let client = hyper_util::client::legacy::Client::builder(TokioExecutor::new())
        .build_http::<Empty<Bytes>>();

    let uri: hyper::Uri = format!("http://{addr}/hello").parse().unwrap();
    let resp = client.get(uri).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(text, "GET /hello");

    shutdown_tx.send(true).unwrap();
}

// ── 2. Concurrent request handling ─────────────────────────────────

#[tokio::test]
async fn handles_concurrent_requests() {
    let (addr, shutdown_tx) = start_echo_server().await;

    let mut handles = Vec::new();
    for i in 0..10 {
        handles.push(tokio::spawn(async move {
            let client = hyper_util::client::legacy::Client::builder(TokioExecutor::new())
                .build_http::<Empty<Bytes>>();
            let uri: hyper::Uri = format!("http://{addr}/path/{i}").parse().unwrap();
            let resp = client.get(uri).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);

            let body = resp.into_body().collect().await.unwrap().to_bytes();
            let text = String::from_utf8(body.to_vec()).unwrap();
            assert_eq!(text, format!("GET /path/{i}"));
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    shutdown_tx.send(true).unwrap();
}

// ── 3. Handler error returns 500 ───────────────────────────────────

#[tokio::test]
async fn handler_error_returns_500() {
    let failing_handler: RequestHandler =
        Arc::new(|_req| Box::pin(async { Err(anyhow::anyhow!("simulated failure")) }));

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let bound = listener.local_addr().unwrap();
    drop(listener);

    let trigger = HttpTrigger::new(bound, failing_handler);
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let _ = trigger.serve(rx).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = hyper_util::client::legacy::Client::builder(TokioExecutor::new())
        .build_http::<Empty<Bytes>>();

    let uri: hyper::Uri = format!("http://{bound}/fail").parse().unwrap();
    let resp = client.get(uri).await.unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(text, "Internal Server Error");

    tx.send(true).unwrap();
}

// ── 4. Header conversion roundtrip ─────────────────────────────────

#[test]
fn header_roundtrip_preserves_values() {
    let mut original = HeaderMap::new();
    original.insert("content-type", "application/json".parse().unwrap());
    original.insert("x-request-id", "abc-123".parse().unwrap());
    original.insert("authorization", "Bearer tok".parse().unwrap());

    let tuples = convert::headers_to_tuples(&original);
    assert_eq!(tuples.len(), 3);

    let restored = convert::headers_from_tuples(tuples);
    assert_eq!(restored.get("content-type").unwrap(), "application/json");
    assert_eq!(restored.get("x-request-id").unwrap(), "abc-123");
    assert_eq!(restored.get("authorization").unwrap(), "Bearer tok");
}

#[test]
fn header_roundtrip_with_multiple_values() {
    let mut original = HeaderMap::new();
    original.append("set-cookie", "a=1".parse().unwrap());
    original.append("set-cookie", "b=2".parse().unwrap());

    let tuples = convert::headers_to_tuples(&original);
    assert_eq!(tuples.len(), 2);

    let restored = convert::headers_from_tuples(tuples);
    let cookies: Vec<&HeaderValue> = restored.get_all("set-cookie").iter().collect();
    assert_eq!(cookies.len(), 2);
}

// ── 5. URI edge cases ──────────────────────────────────────────────

#[test]
fn uri_root_path() {
    let uri: Uri = "/".parse().unwrap();
    assert_eq!(convert::uri_path_and_query(&uri), "/");
}

#[test]
fn uri_with_query_only() {
    let uri: Uri = "/?key=val".parse().unwrap();
    assert_eq!(convert::uri_path_and_query(&uri), "/?key=val");
}

#[test]
fn uri_with_path_and_query() {
    let uri: Uri = "/api/v1/data?page=2&limit=10".parse().unwrap();
    assert_eq!(
        convert::uri_path_and_query(&uri),
        "/api/v1/data?page=2&limit=10"
    );
}

#[test]
fn uri_deeply_nested_path() {
    let uri: Uri = "/a/b/c/d/e/f".parse().unwrap();
    assert_eq!(convert::uri_path_and_query(&uri), "/a/b/c/d/e/f");
}

#[test]
fn uri_full_with_host() {
    let uri: Uri = "http://localhost:8080/api?q=1".parse().unwrap();
    assert_eq!(convert::uri_path_and_query(&uri), "/api?q=1");
}

// ── 6. Status code boundary values ─────────────────────────────────

#[test]
fn status_from_valid_codes() {
    assert_eq!(convert::status_from_u16(200), StatusCode::OK);
    assert_eq!(convert::status_from_u16(201), StatusCode::CREATED);
    assert_eq!(convert::status_from_u16(204), StatusCode::NO_CONTENT);
    assert_eq!(convert::status_from_u16(301), StatusCode::MOVED_PERMANENTLY);
    assert_eq!(convert::status_from_u16(400), StatusCode::BAD_REQUEST);
    assert_eq!(convert::status_from_u16(404), StatusCode::NOT_FOUND);
    assert_eq!(
        convert::status_from_u16(500),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        convert::status_from_u16(503),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[test]
fn status_from_boundary_valid_codes() {
    // 100 is the lowest valid status code
    assert_eq!(convert::status_from_u16(100), StatusCode::CONTINUE);
    // 999 is the highest valid u16 that http::StatusCode accepts
    assert_eq!(
        convert::status_from_u16(999),
        StatusCode::from_u16(999).unwrap()
    );
}

#[test]
fn status_from_invalid_codes_fallback_to_500() {
    assert_eq!(
        convert::status_from_u16(0),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        convert::status_from_u16(99),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        convert::status_from_u16(1000),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        convert::status_from_u16(u16::MAX),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[test]
fn method_to_string_all_standard_methods() {
    assert_eq!(convert::method_to_string(&Method::GET), "GET");
    assert_eq!(convert::method_to_string(&Method::POST), "POST");
    assert_eq!(convert::method_to_string(&Method::PUT), "PUT");
    assert_eq!(convert::method_to_string(&Method::DELETE), "DELETE");
    assert_eq!(convert::method_to_string(&Method::PATCH), "PATCH");
    assert_eq!(convert::method_to_string(&Method::HEAD), "HEAD");
    assert_eq!(convert::method_to_string(&Method::OPTIONS), "OPTIONS");
}
