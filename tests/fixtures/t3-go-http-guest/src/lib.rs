#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

wit_bindgen::generate!({
    path: "wit",
    world: "t3-go-http-test",
    generate_all,
});

struct Component;

/// Build a Postgres v3.0 startup message.
fn build_startup_message(database: &str, user: &str) -> Vec<u8> {
    let params = format!("user\0{user}\0database\0{database}\0\0");
    let params_bytes = params.as_bytes();
    let total_len = (4 + 4 + params_bytes.len()) as i32;

    let mut buf = Vec::with_capacity(total_len as usize);
    buf.extend_from_slice(&total_len.to_be_bytes());
    buf.extend_from_slice(&196608_i32.to_be_bytes()); // protocol 3.0
    buf.extend_from_slice(params_bytes);
    buf
}

/// Build a Postgres simple query message ('Q').
fn build_query_message(sql: &str) -> Vec<u8> {
    let sql_bytes = sql.as_bytes();
    let msg_len = (4 + sql_bytes.len() + 1) as i32;

    let mut buf = Vec::with_capacity(1 + msg_len as usize);
    buf.push(b'Q');
    buf.extend_from_slice(&msg_len.to_be_bytes());
    buf.extend_from_slice(sql_bytes);
    buf.push(0); // null terminator
    buf
}

/// Receive data from the database proxy, accumulating until we see a ReadyForQuery ('Z') marker.
fn recv_until_ready(handle: u64) -> Result<Vec<u8>, String> {
    let mut all_data: Vec<u8> = Vec::new();
    let max_iterations = 100;

    for _ in 0..max_iterations {
        let chunk = warpgrid::shim::database_proxy::recv(handle, 65536)?;
        if chunk.is_empty() {
            break;
        }
        all_data.extend_from_slice(&chunk);

        // Check for ReadyForQuery marker ('Z' = 0x5A)
        if all_data.len() >= 6 {
            for i in 0..all_data.len().saturating_sub(5) {
                if all_data[i] == b'Z' {
                    if i + 5 < all_data.len() {
                        let len = i32::from_be_bytes([
                            all_data[i + 1],
                            all_data[i + 2],
                            all_data[i + 3],
                            all_data[i + 4],
                        ]);
                        if len == 5 {
                            return Ok(all_data);
                        }
                    }
                }
            }
        }
    }

    Ok(all_data)
}

/// Connect and perform the startup handshake.
fn connect_and_handshake(
    host: &str,
    port: u16,
    database: &str,
    user: &str,
) -> Result<u64, String> {
    let config = warpgrid::shim::database_proxy::ConnectConfig {
        host: String::from(host),
        port,
        database: String::from(database),
        user: String::from(user),
        password: None,
    };

    let handle = warpgrid::shim::database_proxy::connect(&config)?;

    let startup = build_startup_message(database, user);
    warpgrid::shim::database_proxy::send(handle, &startup)?;

    let _response = recv_until_ready(handle)?;

    Ok(handle)
}

/// Parse DataRow messages from a Postgres response, extracting text field values.
/// Returns a Vec of rows, where each row is a Vec of string field values.
fn parse_data_rows(data: &[u8]) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut pos = 0;

    while pos < data.len() {
        let msg_type = data[pos];
        if pos + 5 > data.len() {
            break;
        }
        let msg_len = i32::from_be_bytes([
            data[pos + 1],
            data[pos + 2],
            data[pos + 3],
            data[pos + 4],
        ]) as usize;
        let msg_end = pos + 1 + msg_len;
        if msg_end > data.len() {
            break;
        }

        if msg_type == b'D' {
            // DataRow: Int16 field_count, then for each field: Int32 len + bytes
            let field_count =
                i16::from_be_bytes([data[pos + 5], data[pos + 6]]) as usize;
            let mut field_pos = pos + 7;
            let mut fields = Vec::with_capacity(field_count);

            for _ in 0..field_count {
                if field_pos + 4 > data.len() {
                    break;
                }
                let field_len = i32::from_be_bytes([
                    data[field_pos],
                    data[field_pos + 1],
                    data[field_pos + 2],
                    data[field_pos + 3],
                ]) as i32;
                field_pos += 4;

                if field_len < 0 {
                    // NULL
                    fields.push(String::from(""));
                } else {
                    let end = field_pos + field_len as usize;
                    if end <= data.len() {
                        let val = core::str::from_utf8(&data[field_pos..end])
                            .unwrap_or("");
                        fields.push(String::from(val));
                    }
                    field_pos = field_pos + field_len as usize;
                }
            }
            rows.push(fields);
        }

        pos = msg_end;
    }

    rows
}

/// Format rows as a JSON array of user objects: [{"id":1,"name":"...","email":"..."}, ...]
fn format_users_json(rows: &[Vec<String>]) -> String {
    let mut json = String::from("[");
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        let id = if row.len() > 0 { &row[0] } else { "0" };
        let name = if row.len() > 1 { &row[1] } else { "" };
        let email = if row.len() > 2 { &row[2] } else { "" };
        json.push_str(&format!(
            "{{\"id\":{},\"name\":\"{}\",\"email\":\"{}\"}}",
            id, name, email
        ));
    }
    json.push(']');
    json
}

/// Minimal JSON string field extractor: get value for "key" from {"key":"value",...}
fn json_get_str<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let pattern = format!("\"{}\":\"", key);
    let start = json.find(&pattern)? + pattern.len();
    let end = json[start..].find('"')? + start;
    Some(&json[start..end])
}

impl Guest for Component {
    fn test_db_connect(
        host: String,
        port: u16,
        database: String,
        user: String,
    ) -> Result<String, String> {
        let handle = connect_and_handshake(&host, port, &database, &user)?;
        Ok(format!("{handle}"))
    }

    fn test_db_query(handle_str: String, sql: String) -> Result<Vec<u8>, String> {
        let handle: u64 = handle_str
            .parse()
            .map_err(|e| format!("invalid handle: {e}"))?;

        let query_msg = build_query_message(&sql);
        warpgrid::shim::database_proxy::send(handle, &query_msg)?;

        recv_until_ready(handle)
    }

    fn test_db_insert(handle_str: String, name: String, email: String) -> Result<Vec<u8>, String> {
        let handle: u64 = handle_str
            .parse()
            .map_err(|e| format!("invalid handle: {e}"))?;

        let sql = format!(
            "INSERT INTO test_users (name, email) VALUES ('{}', '{}') RETURNING id, name, email",
            name, email
        );
        let query_msg = build_query_message(&sql);
        warpgrid::shim::database_proxy::send(handle, &query_msg)?;

        recv_until_ready(handle)
    }

    fn test_db_close(handle_str: String) -> Result<String, String> {
        let handle: u64 = handle_str
            .parse()
            .map_err(|e| format!("invalid handle: {e}"))?;

        warpgrid::shim::database_proxy::close(handle)?;
        Ok(String::from("closed"))
    }

    fn test_full_lifecycle(
        host: String,
        port: u16,
        database: String,
        user: String,
    ) -> Result<Vec<u8>, String> {
        let handle = connect_and_handshake(&host, port, &database, &user)?;

        let query = build_query_message("SELECT id, name, email FROM test_users ORDER BY id");
        warpgrid::shim::database_proxy::send(handle, &query)?;
        let query_response = recv_until_ready(handle)?;

        warpgrid::shim::database_proxy::close(handle)?;

        Ok(query_response)
    }

    fn test_insert_lifecycle(
        host: String,
        port: u16,
        database: String,
        user: String,
        name: String,
        email: String,
    ) -> Result<Vec<u8>, String> {
        let handle = connect_and_handshake(&host, port, &database, &user)?;

        let sql = format!(
            "INSERT INTO test_users (name, email) VALUES ('{}', '{}') RETURNING id, name, email",
            name, email
        );
        let query_msg = build_query_message(&sql);
        warpgrid::shim::database_proxy::send(handle, &query_msg)?;
        let insert_response = recv_until_ready(handle)?;

        warpgrid::shim::database_proxy::close(handle)?;

        Ok(insert_response)
    }

    fn test_http_get_users(
        host: String,
        port: u16,
        database: String,
        user: String,
    ) -> Result<HttpResponse, String> {
        let handle = connect_and_handshake(&host, port, &database, &user)?;

        let query = build_query_message("SELECT id, name, email FROM test_users ORDER BY id");
        warpgrid::shim::database_proxy::send(handle, &query)?;
        let response = recv_until_ready(handle)?;
        warpgrid::shim::database_proxy::close(handle)?;

        let rows = parse_data_rows(&response);
        let body = format_users_json(&rows);

        Ok(HttpResponse {
            status: 200,
            content_type: String::from("application/json"),
            body,
        })
    }

    fn test_http_post_user(
        host: String,
        port: u16,
        database: String,
        user: String,
        request_body: String,
    ) -> Result<HttpResponse, String> {
        // Parse the request body for name and email
        let name = json_get_str(&request_body, "name")
            .ok_or_else(|| String::from("missing name field"))?;
        let email = json_get_str(&request_body, "email")
            .ok_or_else(|| String::from("missing email field"))?;

        if name.is_empty() || email.is_empty() {
            return Ok(HttpResponse {
                status: 400,
                content_type: String::from("application/json"),
                body: String::from("{\"error\":\"Missing required fields: name and email\"}"),
            });
        }

        let handle = connect_and_handshake(&host, port, &database, &user)?;

        let sql = format!(
            "INSERT INTO test_users (name, email) VALUES ('{}', '{}') RETURNING id, name, email",
            name, email
        );
        let query_msg = build_query_message(&sql);
        warpgrid::shim::database_proxy::send(handle, &query_msg)?;
        let response = recv_until_ready(handle)?;
        warpgrid::shim::database_proxy::close(handle)?;

        let rows = parse_data_rows(&response);
        if rows.is_empty() {
            return Err(String::from("INSERT returned no rows"));
        }
        let row = &rows[0];
        let id = if row.len() > 0 { &row[0] } else { "0" };
        let rname = if row.len() > 1 { &row[1] } else { "" };
        let remail = if row.len() > 2 { &row[2] } else { "" };
        let body = format!(
            "{{\"id\":{},\"name\":\"{}\",\"email\":\"{}\"}}",
            id, rname, remail
        );

        Ok(HttpResponse {
            status: 201,
            content_type: String::from("application/json"),
            body,
        })
    }

    fn test_http_post_invalid_json(request_body: String) -> Result<HttpResponse, String> {
        // Try to parse — if it doesn't contain valid JSON structure, return 400
        if !request_body.contains('{') || json_get_str(&request_body, "name").is_none() {
            return Ok(HttpResponse {
                status: 400,
                content_type: String::from("application/json"),
                body: String::from("{\"error\":\"Invalid JSON\"}"),
            });
        }

        // Check for required fields
        if json_get_str(&request_body, "email").is_none() {
            return Ok(HttpResponse {
                status: 400,
                content_type: String::from("application/json"),
                body: String::from("{\"error\":\"Missing required fields: name and email\"}"),
            });
        }

        // Valid — shouldn't reach here in the test
        Ok(HttpResponse {
            status: 200,
            content_type: String::from("application/json"),
            body: String::from("{\"status\":\"ok\"}"),
        })
    }

    fn test_http_db_unavailable(host: String, port: u16) -> Result<HttpResponse, String> {
        let config = warpgrid::shim::database_proxy::ConnectConfig {
            host,
            port,
            database: String::from("testdb"),
            user: String::from("testuser"),
            password: None,
        };

        match warpgrid::shim::database_proxy::connect(&config) {
            Err(_) => Ok(HttpResponse {
                status: 503,
                content_type: String::from("application/json"),
                body: String::from("{\"error\":\"Service Unavailable\"}"),
            }),
            Ok(handle) => {
                // Connection succeeded unexpectedly — try to send startup and see if it fails
                let startup = build_startup_message("testdb", "testuser");
                if let Err(_) = warpgrid::shim::database_proxy::send(handle, &startup) {
                    let _ = warpgrid::shim::database_proxy::close(handle);
                    return Ok(HttpResponse {
                        status: 503,
                        content_type: String::from("application/json"),
                        body: String::from("{\"error\":\"Service Unavailable\"}"),
                    });
                }
                match recv_until_ready(handle) {
                    Err(_) => {
                        let _ = warpgrid::shim::database_proxy::close(handle);
                        Ok(HttpResponse {
                            status: 503,
                            content_type: String::from("application/json"),
                            body: String::from("{\"error\":\"Service Unavailable\"}"),
                        })
                    }
                    Ok(_) => {
                        let _ = warpgrid::shim::database_proxy::close(handle);
                        Err(String::from("expected connection to fail but it succeeded"))
                    }
                }
            }
        }
    }
}

export!(Component);
