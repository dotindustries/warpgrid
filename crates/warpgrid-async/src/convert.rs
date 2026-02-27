//! Internal conversion functions between WarpGrid-owned types and WIT-generated types.
//!
//! These functions are `pub(crate)` — they are not part of the public API. They
//! exist to ensure round-trip fidelity between the stable WarpGrid types and the
//! upstream WIT records that may change as the WASI 0.3 spec evolves.
//!
//! When the prototyping branch WIT definitions change, only this module needs
//! updating — all downstream code continues using the stable WarpGrid types.

use crate::{Header, Request, Response};

/// WIT-generated types from inline WIT mirroring the `warpgrid:shim/http-types`
/// interface. Using inline WIT keeps this crate self-contained and avoids
/// circular dependencies with `warpgrid-host`.
mod wit {
    wasmtime::component::bindgen!({
        inline: "
            package warpgrid:async-internal@0.1.0;

            interface http-types {
                record http-header {
                    name: string,
                    value: string,
                }

                record http-request {
                    method: string,
                    uri: string,
                    headers: list<http-header>,
                    body: list<u8>,
                }

                record http-response {
                    status: u16,
                    headers: list<http-header>,
                    body: list<u8>,
                }
            }

            world http-types-only {
                import http-types;
            }
        ",
        world: "http-types-only",
    });
}

pub(crate) use wit::warpgrid::async_internal::http_types::{
    HttpHeader as WitHeader, HttpRequest as WitRequest, HttpResponse as WitResponse,
};

/// Convert a WIT `HttpRequest` to a WarpGrid [`Request`].
pub(crate) fn request_from_wit(wit: WitRequest) -> Request {
    Request {
        method: wit.method,
        uri: wit.uri,
        headers: wit.headers.into_iter().map(header_from_wit).collect(),
        body: wit.body,
    }
}

/// Convert a WarpGrid [`Request`] to a WIT `HttpRequest`.
pub(crate) fn request_to_wit(req: Request) -> WitRequest {
    WitRequest {
        method: req.method,
        uri: req.uri,
        headers: req.headers.into_iter().map(header_to_wit).collect(),
        body: req.body,
    }
}

/// Convert a WIT `HttpResponse` to a WarpGrid [`Response`].
pub(crate) fn response_from_wit(wit: WitResponse) -> Response {
    Response {
        status: wit.status,
        headers: wit.headers.into_iter().map(header_from_wit).collect(),
        body: wit.body,
    }
}

/// Convert a WarpGrid [`Response`] to a WIT `HttpResponse`.
pub(crate) fn response_to_wit(resp: Response) -> WitResponse {
    WitResponse {
        status: resp.status,
        headers: resp.headers.into_iter().map(header_to_wit).collect(),
        body: resp.body,
    }
}

fn header_from_wit(wit: WitHeader) -> Header {
    Header {
        name: wit.name,
        value: wit.value,
    }
}

fn header_to_wit(header: Header) -> WitHeader {
    WitHeader {
        name: header.name,
        value: header.value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Request round-trip tests ────────────────────────────────────

    #[test]
    fn request_round_trip_minimal() {
        let original = Request {
            method: "GET".into(),
            uri: "/".into(),
            headers: vec![],
            body: vec![],
        };
        let round_tripped = request_from_wit(request_to_wit(original.clone()));
        assert_eq!(original, round_tripped);
    }

    #[test]
    fn request_round_trip_with_headers_and_body() {
        let original = Request {
            method: "POST".into(),
            uri: "/users?page=1".into(),
            headers: vec![
                Header {
                    name: "content-type".into(),
                    value: "application/json".into(),
                },
                Header {
                    name: "authorization".into(),
                    value: "Bearer token123".into(),
                },
            ],
            body: br#"{"name":"test"}"#.to_vec(),
        };
        let round_tripped = request_from_wit(request_to_wit(original.clone()));
        assert_eq!(original, round_tripped);
    }

    #[test]
    fn request_round_trip_preserves_all_http_methods() {
        for method in ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"] {
            let original = Request {
                method: method.into(),
                uri: "/test".into(),
                headers: vec![],
                body: vec![],
            };
            let round_tripped = request_from_wit(request_to_wit(original.clone()));
            assert_eq!(
                original.method, round_tripped.method,
                "HTTP method {method} not preserved"
            );
        }
    }

    #[test]
    fn request_round_trip_preserves_binary_body() {
        let body: Vec<u8> = (0u8..=255).collect();
        let original = Request {
            method: "PUT".into(),
            uri: "/upload".into(),
            headers: vec![],
            body,
        };
        let round_tripped = request_from_wit(request_to_wit(original.clone()));
        assert_eq!(original.body, round_tripped.body);
    }

    // ── Response round-trip tests ───────────────────────────────────

    #[test]
    fn response_round_trip_minimal() {
        let original = Response {
            status: 200,
            headers: vec![],
            body: vec![],
        };
        let round_tripped = response_from_wit(response_to_wit(original.clone()));
        assert_eq!(original, round_tripped);
    }

    #[test]
    fn response_round_trip_with_headers_and_body() {
        let original = Response {
            status: 201,
            headers: vec![
                Header {
                    name: "content-type".into(),
                    value: "application/json".into(),
                },
                Header {
                    name: "x-request-id".into(),
                    value: "abc-123".into(),
                },
            ],
            body: br#"{"id":1,"name":"test"}"#.to_vec(),
        };
        let round_tripped = response_from_wit(response_to_wit(original.clone()));
        assert_eq!(original, round_tripped);
    }

    #[test]
    fn response_round_trip_preserves_all_status_codes() {
        for status in [100, 200, 201, 204, 301, 400, 401, 403, 404, 500, 502, 503] {
            let original = Response {
                status,
                headers: vec![],
                body: vec![],
            };
            let round_tripped = response_from_wit(response_to_wit(original.clone()));
            assert_eq!(
                original.status, round_tripped.status,
                "status code {status} not preserved"
            );
        }
    }

    // ── Header edge cases ───────────────────────────────────────────

    #[test]
    fn header_round_trip_preserves_empty_values() {
        let original = Request {
            method: "GET".into(),
            uri: "/".into(),
            headers: vec![Header {
                name: "x-empty".into(),
                value: "".into(),
            }],
            body: vec![],
        };
        let round_tripped = request_from_wit(request_to_wit(original.clone()));
        assert_eq!(original.headers[0].value, "");
        assert_eq!(original, round_tripped);
    }

    #[test]
    fn header_order_preserved() {
        let headers = vec![
            Header {
                name: "z-last".into(),
                value: "3".into(),
            },
            Header {
                name: "a-first".into(),
                value: "1".into(),
            },
            Header {
                name: "m-middle".into(),
                value: "2".into(),
            },
        ];
        let original = Response {
            status: 200,
            headers,
            body: vec![],
        };
        let round_tripped = response_from_wit(response_to_wit(original.clone()));
        assert_eq!(original.headers, round_tripped.headers);
    }

    // ── Streaming body placeholder ──────────────────────────────────

    #[test]
    fn response_round_trip_large_body() {
        // US-505 will add streaming body support. For now, verify that large
        // bodies (simulating a stream buffered in memory) round-trip correctly.
        let body = vec![0xAB_u8; 1024 * 1024]; // 1 MB
        let original = Response {
            status: 200,
            headers: vec![],
            body,
        };
        let round_tripped = response_from_wit(response_to_wit(original.clone()));
        assert_eq!(original.body.len(), round_tripped.body.len());
        assert_eq!(original, round_tripped);
    }
}
