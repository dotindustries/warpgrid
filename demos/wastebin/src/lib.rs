//! wastebin demo — PostgreSQL-backed pastebin for WarpGrid.
//!
//! Entry point for both standalone and WASI HTTP handler modes.

pub mod paste;
pub mod router;
pub mod storage;
pub mod templates;

use router::Response;
use storage::Storage;

/// Handle an incoming HTTP request.
///
/// This is the main entry point called by both the standalone binary
/// and the WASI HTTP handler export.
pub fn handle_request(
    method: &str,
    path: &str,
    body: &[u8],
    conninfo: &str,
    instance_id: &str,
) -> Response {
    let mut storage = match Storage::connect(conninfo, instance_id) {
        Ok(s) => s,
        Err(e) => {
            return Response {
                status: 503,
                headers: vec![("content-type".into(), "text/plain".into())],
                body: format!("Database connection failed: {e}").into_bytes(),
            };
        }
    };

    if let Err(e) = storage.migrate() {
        return Response {
            status: 503,
            headers: vec![("content-type".into(), "text/plain".into())],
            body: format!("Migration failed: {e}").into_bytes(),
        };
    }

    router::route(method, path, body, &mut storage)
}
