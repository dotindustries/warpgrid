//! HTTP type conversions between hyper and wasi-http.
//!
//! Converts between the external HTTP types (hyper/http) and the
//! wasmtime-wasi-http internal types used by the component model.

use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};

/// Convert an http::Method to a string representation used by wasi-http.
pub fn method_to_string(method: &Method) -> String {
    method.as_str().to_string()
}

/// Convert a status code from u16.
pub fn status_from_u16(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

/// Convert headers from a list of (name, value) tuples.
pub fn headers_from_tuples(tuples: Vec<(String, Vec<u8>)>) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in tuples {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_bytes(&value),
        ) {
            map.append(name, value);
        }
    }
    map
}

/// Convert headers to a list of (name, value) tuples.
pub fn headers_to_tuples(headers: &HeaderMap) -> Vec<(String, Vec<u8>)> {
    headers
        .iter()
        .map(|(name, value)| (name.as_str().to_string(), value.as_bytes().to_vec()))
        .collect()
}

/// Extract the path and query from a URI.
pub fn uri_path_and_query(uri: &Uri) -> String {
    uri.path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_to_string_get() {
        assert_eq!(method_to_string(&Method::GET), "GET");
    }

    #[test]
    fn method_to_string_post() {
        assert_eq!(method_to_string(&Method::POST), "POST");
    }

    #[test]
    fn status_from_valid_code() {
        assert_eq!(status_from_u16(200), StatusCode::OK);
        assert_eq!(status_from_u16(404), StatusCode::NOT_FOUND);
    }

    #[test]
    fn status_from_invalid_code() {
        // Codes 100-999 are valid in http::StatusCode; only out-of-range codes
        // fall back to 500.
        assert_eq!(status_from_u16(9999), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn headers_roundtrip() {
        let mut original = HeaderMap::new();
        original.insert("content-type", "application/json".parse().unwrap());
        original.insert("x-custom", "hello".parse().unwrap());

        let tuples = headers_to_tuples(&original);
        let restored = headers_from_tuples(tuples);

        assert_eq!(
            restored.get("content-type").unwrap(),
            "application/json"
        );
        assert_eq!(restored.get("x-custom").unwrap(), "hello");
    }

    #[test]
    fn uri_path_and_query_full() {
        let uri: Uri = "http://localhost:8080/api/v1?foo=bar".parse().unwrap();
        assert_eq!(uri_path_and_query(&uri), "/api/v1?foo=bar");
    }

    #[test]
    fn uri_path_and_query_root() {
        let uri: Uri = "/".parse().unwrap();
        assert_eq!(uri_path_and_query(&uri), "/");
    }
}
