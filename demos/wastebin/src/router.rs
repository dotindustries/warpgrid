//! HTTP request routing for wastebin.

use crate::storage::Storage;
use crate::templates;

/// HTTP response with status, headers, and body.
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    fn html(status: u16, body: String) -> Self {
        Self {
            status,
            headers: vec![("content-type".into(), "text/html; charset=utf-8".into())],
            body: body.into_bytes(),
        }
    }

    fn json(status: u16, body: String) -> Self {
        Self {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.into_bytes(),
        }
    }

    fn text(status: u16, body: String) -> Self {
        Self {
            status,
            headers: vec![("content-type".into(), "text/plain; charset=utf-8".into())],
            body: body.into_bytes(),
        }
    }

    fn redirect(location: &str) -> Self {
        Self {
            status: 303,
            headers: vec![("location".into(), location.into())],
            body: Vec::new(),
        }
    }

    fn not_found() -> Self {
        Self::html(404, templates::error_page(404, "Paste not found"))
    }

    fn internal_error(msg: &str) -> Self {
        Self::html(500, templates::error_page(500, msg))
    }
}

/// Route an incoming request to the appropriate handler.
pub fn route(method: &str, path: &str, body: &[u8], storage: &mut Storage) -> Response {
    match (method, path) {
        ("GET", "/") => handle_index(storage),
        ("GET", "/health") => Response::json(200, r#"{"status":"ok"}"#.into()),
        ("GET", "/api/stats") => handle_api_stats(storage),
        ("POST", "/") => handle_create_form(body, storage),
        ("POST", "/api/paste") => handle_create_json(body, storage),
        ("GET", p) if p.starts_with("/raw/") => {
            let id = &p[5..];
            handle_raw(id, storage)
        }
        ("GET", p) if p.starts_with('/') && p.len() > 1 && !p.contains('/') => {
            let id = &p[1..];
            handle_view(id, storage)
        }
        ("DELETE", p) if p.starts_with('/') && p.len() > 1 => {
            let id = &p[1..];
            handle_delete(id, storage)
        }
        ("POST", p) if p.starts_with('/') && p.len() > 1 => {
            // HTML forms use POST with _method=DELETE
            let body_str = std::str::from_utf8(body).unwrap_or("");
            if body_str.contains("_method=DELETE") {
                let id = &p[1..];
                handle_delete(id, storage)
            } else {
                Response::html(405, templates::error_page(405, "Method not allowed"))
            }
        }
        _ => Response::not_found(),
    }
}

fn handle_index(storage: &mut Storage) -> Response {
    match storage.list_pastes(50) {
        Ok(pastes) => {
            let items: Vec<(String, Option<String>, Option<String>, u64)> = pastes
                .iter()
                .map(|p| {
                    (
                        p.id.clone(),
                        p.title.clone(),
                        p.language.clone(),
                        p.created_at,
                    )
                })
                .collect();
            Response::html(200, templates::index_page(&items))
        }
        Err(e) => Response::internal_error(&format!("Failed to list pastes: {e}")),
    }
}

fn handle_api_stats(storage: &mut Storage) -> Response {
    match storage.paste_count() {
        Ok(count) => Response::json(
            200,
            format!(r#"{{"paste_count":{count},"status":"ok"}}"#),
        ),
        Err(e) => Response::json(
            500,
            format!(r#"{{"error":"{}"}}"#, e),
        ),
    }
}

fn handle_create_form(body: &[u8], storage: &mut Storage) -> Response {
    let body_str = std::str::from_utf8(body).unwrap_or("");
    let form = parse_form_data(body_str);

    let content = match form.get("content") {
        Some(c) if !c.is_empty() => c.clone(),
        _ => return Response::html(400, templates::error_page(400, "Content is required")),
    };

    let req = crate::paste::CreatePasteRequest {
        title: form.get("title").filter(|s| !s.is_empty()).cloned(),
        content,
        language: form.get("language").filter(|s| !s.is_empty()).cloned(),
        burn_after: form.get("burn_after").map(|v| v == "true"),
        expires_in_seconds: None,
    };

    match storage.create_paste(&req) {
        Ok(paste) => Response::redirect(&format!("/{}", paste.id)),
        Err(e) => Response::internal_error(&format!("Failed to create paste: {e}")),
    }
}

fn handle_create_json(body: &[u8], storage: &mut Storage) -> Response {
    let req: crate::paste::CreatePasteRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return Response::json(
                400,
                format!(r#"{{"error":"Invalid JSON: {}"}}"#, e),
            )
        }
    };

    match storage.create_paste(&req) {
        Ok(paste) => Response::json(
            201,
            serde_json::to_string(&serde_json::json!({
                "id": paste.id,
                "url": format!("/{}", paste.id),
            }))
            .unwrap_or_default(),
        ),
        Err(e) => Response::json(
            500,
            format!(r#"{{"error":"{}"}}"#, e),
        ),
    }
}

fn handle_view(id: &str, storage: &mut Storage) -> Response {
    match storage.get_paste(id) {
        Ok(Some(paste)) => {
            let content = String::from_utf8_lossy(&paste.content);
            Response::html(
                200,
                templates::paste_page(
                    &paste.id,
                    paste.title.as_deref(),
                    &content,
                    paste.language.as_deref(),
                    paste.created_at,
                ),
            )
        }
        Ok(None) => Response::not_found(),
        Err(e) => Response::internal_error(&format!("Failed to retrieve paste: {e}")),
    }
}

fn handle_raw(id: &str, storage: &mut Storage) -> Response {
    match storage.get_paste(id) {
        Ok(Some(paste)) => {
            let content = String::from_utf8_lossy(&paste.content);
            Response::text(200, content.into_owned())
        }
        Ok(None) => Response::text(404, "Not found".into()),
        Err(e) => Response::text(500, format!("Error: {e}")),
    }
}

fn handle_delete(id: &str, storage: &mut Storage) -> Response {
    match storage.delete_paste(id) {
        Ok(true) => Response::redirect("/"),
        Ok(false) => Response::not_found(),
        Err(e) => Response::internal_error(&format!("Failed to delete paste: {e}")),
    }
}

/// Parse URL-encoded form data into key-value pairs.
fn parse_form_data(body: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for pair in body.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            map.insert(
                url_decode(key),
                url_decode(value),
            );
        }
    }
    map
}

/// Decode a URL-encoded string, handling %XX sequences and '+' as space.
fn url_decode(s: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        match b {
            b'+' => bytes.push(b' '),
            b'%' => {
                let hi = chars.next().unwrap_or(b'0');
                let lo = chars.next().unwrap_or(b'0');
                let byte = hex_val(hi) * 16 + hex_val(lo);
                bytes.push(byte);
            }
            _ => bytes.push(b),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn hex_val(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}
