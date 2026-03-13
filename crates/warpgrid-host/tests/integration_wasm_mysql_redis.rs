//! US-117: MySQL and Redis integration tests.
//!
//! End-to-end tests proving Wasm guest components can communicate with MySQL
//! and Redis through WarpGrid's database proxy. The guest sends raw wire
//! protocol bytes (MySQL wire protocol / Redis RESP) through the same generic
//! `database-proxy` WIT interface. The host proxy is protocol-agnostic
//! (pure byte passthrough via `TcpConnectionFactory`).
//!
//! Also includes pool-level health check tests validating that COM_PING
//! (MySQL) and PING (Redis) detect and remove dead connections.
//!
//! Test stack:
//! ```text
//! Guest Component (Wasm)
//!   ↓ WIT import: dns.resolve-address
//! DnsHost → CachedDnsResolver → DnsResolver (service registry)
//!   ↓ WIT import: database-proxy.connect/send/recv/close
//! DbProxyHost → ConnectionPoolManager → TcpConnectionFactory → TCP
//!   ├── StatefulMockMysqlServer (port A)
//!   └── StatefulMockRedisServer (port B)
//! ```
//!
//! All tests are gated behind `cfg(feature = "integration")`.

#![cfg(feature = "integration")]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use wasmtime::component::Component;
use wasmtime::Store;

use warpgrid_host::config::ShimConfig;
use warpgrid_host::db_proxy::mysql::MysqlConnectionFactory;
use warpgrid_host::db_proxy::redis::RedisConnectionFactory;
use warpgrid_host::db_proxy::tcp::TcpConnectionFactory;
use warpgrid_host::db_proxy::{ConnectionPoolManager, PoolConfig, PoolKey, Protocol};
use warpgrid_host::engine::WarpGridEngine;

// ── MySQL protocol helpers ──────────────────────────────────────────

/// Build a minimal MySQL server greeting packet.
fn mysql_server_greeting() -> Vec<u8> {
    let version = b"5.7.0-warpgrid-mock\0";
    let thread_id: u32 = 1;
    let auth_data_1: [u8; 8] = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
    let filler: u8 = 0x00;
    let cap_low: u16 = 0xffff;
    let charset: u8 = 0x21; // utf8_general_ci
    let status: u16 = 0x0002; // SERVER_STATUS_AUTOCOMMIT
    let cap_high: u16 = 0x00ff;
    let auth_len: u8 = 21;
    let reserved: [u8; 10] = [0; 10];
    let auth_data_2: [u8; 13] = [
        0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x00,
    ];

    let mut payload = Vec::new();
    payload.push(0x0a); // protocol version
    payload.extend_from_slice(version);
    payload.extend_from_slice(&thread_id.to_le_bytes());
    payload.extend_from_slice(&auth_data_1);
    payload.push(filler);
    payload.extend_from_slice(&cap_low.to_le_bytes());
    payload.push(charset);
    payload.extend_from_slice(&status.to_le_bytes());
    payload.extend_from_slice(&cap_high.to_le_bytes());
    payload.push(auth_len);
    payload.extend_from_slice(&reserved);
    payload.extend_from_slice(&auth_data_2);

    let len = payload.len() as u32;
    let mut packet = Vec::with_capacity(4 + payload.len());
    packet.push((len & 0xff) as u8);
    packet.push(((len >> 8) & 0xff) as u8);
    packet.push(((len >> 16) & 0xff) as u8);
    packet.push(0x00); // sequence id = 0
    packet.extend_from_slice(&payload);
    packet
}

/// MySQL OK packet with configurable affected rows.
fn mysql_ok_packet(seq_id: u8, affected_rows: u8) -> Vec<u8> {
    let payload: [u8; 7] = [
        0x00, // OK marker
        affected_rows,
        0x00, // last insert id (lenenc)
        0x02,
        0x00, // status flags (SERVER_STATUS_AUTOCOMMIT)
        0x00,
        0x00, // warnings
    ];
    let len = payload.len() as u32;
    let mut packet = Vec::with_capacity(4 + payload.len());
    packet.push((len & 0xff) as u8);
    packet.push(((len >> 8) & 0xff) as u8);
    packet.push(((len >> 16) & 0xff) as u8);
    packet.push(seq_id);
    packet.extend_from_slice(&payload);
    packet
}

/// MySQL EOF packet (marks end of column definitions or row data).
fn mysql_eof_packet(seq_id: u8) -> Vec<u8> {
    let payload: [u8; 5] = [
        0xFE, // EOF marker
        0x00, 0x00, // warnings
        0x02, 0x00, // status flags
    ];
    let len = payload.len() as u32;
    let mut packet = Vec::with_capacity(4 + payload.len());
    packet.push((len & 0xff) as u8);
    packet.push(((len >> 8) & 0xff) as u8);
    packet.push(((len >> 16) & 0xff) as u8);
    packet.push(seq_id);
    packet.extend_from_slice(&payload);
    packet
}

/// Build a MySQL column count packet.
fn mysql_column_count_packet(seq_id: u8, count: u8) -> Vec<u8> {
    vec![0x01, 0x00, 0x00, seq_id, count]
}

/// Build a MySQL column definition packet.
fn mysql_column_def_packet(seq_id: u8, name: &str) -> Vec<u8> {
    let mut payload = Vec::new();

    // catalog = "def"
    payload.push(3);
    payload.extend_from_slice(b"def");
    // schema (empty)
    payload.push(0);
    // table (empty)
    payload.push(0);
    // org_table (empty)
    payload.push(0);
    // name
    payload.push(name.len() as u8);
    payload.extend_from_slice(name.as_bytes());
    // org_name (empty)
    payload.push(0);
    // filler
    payload.push(0x0c);
    // charset = utf8 (33 = 0x21, 0x00)
    payload.extend_from_slice(&[0x21, 0x00]);
    // column_length
    payload.extend_from_slice(&[0xFF, 0x00, 0x00, 0x00]);
    // column_type = VARCHAR (0xFD)
    payload.push(0xFD);
    // flags
    payload.extend_from_slice(&[0x00, 0x00]);
    // decimals
    payload.push(0x00);
    // filler
    payload.extend_from_slice(&[0x00, 0x00]);

    let len = payload.len() as u32;
    let mut packet = Vec::with_capacity(4 + payload.len());
    packet.push((len & 0xff) as u8);
    packet.push(((len >> 8) & 0xff) as u8);
    packet.push(((len >> 16) & 0xff) as u8);
    packet.push(seq_id);
    packet.extend_from_slice(&payload);
    packet
}

/// Build a MySQL result row packet with length-encoded string columns.
fn mysql_row_packet(seq_id: u8, values: &[&str]) -> Vec<u8> {
    let mut payload = Vec::new();
    for val in values {
        let bytes = val.as_bytes();
        // Length-encoded integer for string length
        payload.push(bytes.len() as u8);
        payload.extend_from_slice(bytes);
    }

    let len = payload.len() as u32;
    let mut packet = Vec::with_capacity(4 + payload.len());
    packet.push((len & 0xff) as u8);
    packet.push(((len >> 8) & 0xff) as u8);
    packet.push(((len >> 16) & 0xff) as u8);
    packet.push(seq_id);
    packet.extend_from_slice(&payload);
    packet
}

/// MySQL COM_PING command byte.
const COM_PING: u8 = 0x0e;
/// MySQL COM_QUERY command byte.
const COM_QUERY: u8 = 0x03;

// ── StatefulMockMysqlServer ─────────────────────────────────────────

/// A mock MySQL server that maintains state for CRUD operations.
/// Tracks created tables and inserted rows to validate the full lifecycle.
struct StatefulMockMysqlServer {
    addr: std::net::SocketAddr,
}

/// State shared across connections for a single server instance.
struct MysqlServerState {
    /// Tables that have been created: table_name -> list of column names.
    tables: HashMap<String, Vec<String>>,
    /// Rows that have been inserted: table_name -> list of rows (each row is a list of values).
    rows: HashMap<String, Vec<Vec<String>>>,
}

impl MysqlServerState {
    fn new() -> Self {
        Self {
            tables: HashMap::new(),
            rows: HashMap::new(),
        }
    }
}

impl StatefulMockMysqlServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind to random port");
        let addr = listener.local_addr().expect("local addr");

        std::thread::spawn(move || {
            let state: Arc<Mutex<MysqlServerState>> =
                Arc::new(Mutex::new(MysqlServerState::new()));

            while let Ok((mut stream, _)) = listener.accept() {
                let state = Arc::clone(&state);
                std::thread::spawn(move || {
                    Self::handle_connection(&mut stream, &state);
                });
            }
        });

        std::thread::sleep(Duration::from_millis(10));
        Self { addr }
    }

    /// Start a server that closes connections after auth (for health check tests).
    fn start_close_after_auth() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind to random port");
        let addr = listener.local_addr().expect("local addr");

        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                std::thread::spawn(move || {
                    // Send greeting.
                    let greeting = mysql_server_greeting();
                    let _ = stream.write_all(&greeting);
                    let _ = stream.flush();

                    // Read client handshake response.
                    let mut buf = [0u8; 4096];
                    let _ = stream.read(&mut buf);

                    // Send OK for auth.
                    let ok = mysql_ok_packet(2, 0);
                    let _ = stream.write_all(&ok);
                    let _ = stream.flush();

                    // Close immediately.
                    drop(stream);
                });
            }
        });

        std::thread::sleep(Duration::from_millis(10));
        Self { addr }
    }

    fn handle_connection(
        stream: &mut std::net::TcpStream,
        state: &Mutex<MysqlServerState>,
    ) {
        // 1. Send server greeting.
        let greeting = mysql_server_greeting();
        if stream.write_all(&greeting).is_err() {
            return;
        }
        if stream.flush().is_err() {
            return;
        }

        // 2. Read client handshake response.
        let mut buf = [0u8; 4096];
        if stream.read(&mut buf).is_err() {
            return;
        }

        // 3. Send OK packet (auth success, seq_id = 2).
        let ok = mysql_ok_packet(2, 0);
        if stream.write_all(&ok).is_err() {
            return;
        }
        if stream.flush().is_err() {
            return;
        }

        // 4. Command loop.
        loop {
            let mut header = [0u8; 4];
            if stream.read_exact(&mut header).is_err() {
                break;
            }

            let payload_len = header[0] as usize
                | ((header[1] as usize) << 8)
                | ((header[2] as usize) << 16);
            let _seq_id = header[3];

            if payload_len == 0 {
                continue;
            }

            let mut payload = vec![0u8; payload_len];
            if stream.read_exact(&mut payload).is_err() {
                break;
            }

            match payload[0] {
                COM_PING => {
                    let ok = mysql_ok_packet(_seq_id.wrapping_add(1), 0);
                    if stream.write_all(&ok).is_err() {
                        break;
                    }
                    if stream.flush().is_err() {
                        break;
                    }
                }
                COM_QUERY => {
                    let sql = String::from_utf8_lossy(&payload[1..]).to_string();
                    let response = Self::dispatch_query(&sql, state);
                    if stream.write_all(&response).is_err() {
                        break;
                    }
                    if stream.flush().is_err() {
                        break;
                    }
                }
                _ => {
                    // Echo unknown commands.
                    let mut echo = Vec::with_capacity(4 + payload.len());
                    echo.extend_from_slice(&header);
                    echo.extend_from_slice(&payload);
                    if stream.write_all(&echo).is_err() {
                        break;
                    }
                    if stream.flush().is_err() {
                        break;
                    }
                }
            }
        }
    }

    /// Dispatch a SQL query and return the appropriate MySQL wire protocol response.
    fn dispatch_query(sql: &str, state: &Mutex<MysqlServerState>) -> Vec<u8> {
        let sql_upper = sql.to_uppercase();

        if sql_upper.starts_with("CREATE TABLE") {
            // Parse table name from "CREATE TABLE table_name (...)"
            let table_name = Self::extract_table_name(sql, "CREATE TABLE");
            let columns = Self::extract_column_names(sql);
            let mut guard = state.lock().unwrap();
            guard.tables.insert(table_name.clone(), columns);
            guard.rows.insert(table_name, Vec::new());
            mysql_ok_packet(1, 0)
        } else if sql_upper.starts_with("INSERT INTO") {
            // Parse "INSERT INTO table_name (...) VALUES (...)"
            let table_name = Self::extract_table_name(sql, "INSERT INTO");
            let values = Self::extract_insert_values(sql);
            let mut guard = state.lock().unwrap();
            if let Some(rows) = guard.rows.get_mut(&table_name) {
                rows.push(values);
            }
            mysql_ok_packet(1, 1) // 1 affected row
        } else if sql_upper.starts_with("SELECT") {
            // Parse "SELECT col1, col2 FROM table_name"
            let table_name = Self::extract_select_table(sql);
            let columns = Self::extract_select_columns(sql);
            let guard = state.lock().unwrap();
            let rows = guard.rows.get(&table_name).cloned().unwrap_or_default();
            Self::build_select_response(&columns, &rows)
        } else if sql_upper.starts_with("DROP TABLE") {
            let table_name = Self::extract_table_name(sql, "DROP TABLE");
            let mut guard = state.lock().unwrap();
            guard.tables.remove(&table_name);
            guard.rows.remove(&table_name);
            mysql_ok_packet(1, 0)
        } else {
            // Unknown query — return OK.
            mysql_ok_packet(1, 0)
        }
    }

    fn extract_table_name(sql: &str, prefix: &str) -> String {
        let after_prefix = &sql[prefix.len()..].trim_start();
        after_prefix
            .split(|c: char| c.is_whitespace() || c == '(')
            .next()
            .unwrap_or("unknown")
            .to_string()
    }

    fn extract_column_names(sql: &str) -> Vec<String> {
        // Simple parse: find content between first ( and matching )
        if let (Some(start), Some(end)) = (sql.find('('), sql.rfind(')')) {
            let cols_str = &sql[start + 1..end];
            return cols_str
                .split(',')
                .map(|c| {
                    c.split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_string()
                })
                .collect();
        }
        Vec::new()
    }

    fn extract_insert_values(sql: &str) -> Vec<String> {
        let upper = sql.to_uppercase();
        if let Some(values_pos) = upper.find("VALUES") {
            let after_values = &sql[values_pos + 6..];
            if let (Some(start), Some(end)) = (after_values.find('('), after_values.rfind(')')) {
                let vals_str = &after_values[start + 1..end];
                return vals_str
                    .split(',')
                    .map(|v| v.trim().trim_matches('\'').to_string())
                    .collect();
            }
        }
        Vec::new()
    }

    fn extract_select_table(sql: &str) -> String {
        let upper = sql.to_uppercase();
        if let Some(from_pos) = upper.find("FROM") {
            let after_from = &sql[from_pos + 4..].trim_start();
            return after_from
                .split(|c: char| c.is_whitespace() || c == ';')
                .next()
                .unwrap_or("unknown")
                .to_string();
        }
        "unknown".to_string()
    }

    fn extract_select_columns(sql: &str) -> Vec<String> {
        let upper = sql.to_uppercase();
        let from_pos = upper.find("FROM").unwrap_or(sql.len());
        // After "SELECT " and before "FROM"
        let select_len = if upper.starts_with("SELECT ") {
            7
        } else {
            return Vec::new();
        };
        let cols_str = &sql[select_len..from_pos];
        cols_str
            .split(',')
            .map(|c| c.trim().to_string())
            .collect()
    }

    /// Build a MySQL result set response: column_count + column_defs + EOF + rows + EOF.
    fn build_select_response(columns: &[String], rows: &[Vec<String>]) -> Vec<u8> {
        let mut response = Vec::new();
        let mut seq: u8 = 1;

        // Column count
        response.extend(mysql_column_count_packet(seq, columns.len() as u8));
        seq += 1;

        // Column definitions
        for col in columns {
            response.extend(mysql_column_def_packet(seq, col));
            seq += 1;
        }

        // EOF after column definitions
        response.extend(mysql_eof_packet(seq));
        seq += 1;

        // Row data
        for row in rows {
            let str_refs: Vec<&str> = row.iter().map(|s| s.as_str()).collect();
            response.extend(mysql_row_packet(seq, &str_refs));
            seq += 1;
        }

        // EOF after row data
        response.extend(mysql_eof_packet(seq));

        response
    }

    fn pool_key(&self) -> PoolKey {
        PoolKey::with_protocol(
            "127.0.0.1",
            self.addr.port(),
            "testdb",
            "testuser",
            Protocol::MySQL,
        )
    }
}

// ── StatefulMockRedisServer ─────────────────────────────────────────

/// A mock Redis server that maintains key-value state for CRUD testing.
/// Supports GET, SET, DEL, and PING commands via RESP protocol.
struct StatefulMockRedisServer {
    addr: std::net::SocketAddr,
}

impl StatefulMockRedisServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind to random port");
        let addr = listener.local_addr().expect("local addr");

        std::thread::spawn(move || {
            let store: Arc<Mutex<HashMap<String, String>>> =
                Arc::new(Mutex::new(HashMap::new()));

            while let Ok((mut stream, _)) = listener.accept() {
                let store = Arc::clone(&store);
                std::thread::spawn(move || {
                    Self::handle_connection(&mut stream, &store);
                });
            }
        });

        std::thread::sleep(Duration::from_millis(10));
        Self { addr }
    }

    /// Start a server that closes connections immediately (for health check tests).
    fn start_close_immediately() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind to random port");
        let addr = listener.local_addr().expect("local addr");

        std::thread::spawn(move || {
            while let Ok((stream, _)) = listener.accept() {
                drop(stream);
            }
        });

        std::thread::sleep(Duration::from_millis(10));
        Self { addr }
    }

    fn handle_connection(
        stream: &mut std::net::TcpStream,
        store: &Mutex<HashMap<String, String>>,
    ) {
        let mut buf = [0u8; 8192];
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let received = &buf[..n];
                    let response = Self::process_command(received, store);
                    if stream.write_all(&response).is_err() {
                        break;
                    }
                    if stream.flush().is_err() {
                        break;
                    }
                }
            }
        }
    }

    fn process_command(data: &[u8], store: &Mutex<HashMap<String, String>>) -> Vec<u8> {
        let parts = Self::parse_resp_array(data);
        if parts.is_empty() {
            if data == b"PING\r\n" {
                return b"+PONG\r\n".to_vec();
            }
            return b"-ERR unknown command\r\n".to_vec();
        }

        let cmd = parts[0].to_uppercase();
        match cmd.as_str() {
            "PING" => b"+PONG\r\n".to_vec(),
            "GET" => {
                if parts.len() < 2 {
                    return b"-ERR wrong number of arguments for 'get'\r\n".to_vec();
                }
                let key = &parts[1];
                let guard = store.lock().unwrap();
                match guard.get(key.as_str()) {
                    Some(val) => Self::bulk_string(val.as_bytes()),
                    None => b"$-1\r\n".to_vec(),
                }
            }
            "SET" => {
                if parts.len() < 3 {
                    return b"-ERR wrong number of arguments for 'set'\r\n".to_vec();
                }
                let key = parts[1].clone();
                let value = parts[2].clone();
                store.lock().unwrap().insert(key, value);
                b"+OK\r\n".to_vec()
            }
            "DEL" => {
                if parts.len() < 2 {
                    return b"-ERR wrong number of arguments for 'del'\r\n".to_vec();
                }
                let mut count = 0u32;
                let mut guard = store.lock().unwrap();
                for key in &parts[1..] {
                    if guard.remove(key.as_str()).is_some() {
                        count += 1;
                    }
                }
                format!(":{count}\r\n").into_bytes()
            }
            _ => b"-ERR unknown command\r\n".to_vec(),
        }
    }

    fn parse_resp_array(data: &[u8]) -> Vec<String> {
        if data.is_empty() || data[0] != b'*' {
            return Vec::new();
        }

        let mut pos = 1;
        let count_end = Self::find_crlf(data, pos);
        if count_end.is_none() {
            return Vec::new();
        }
        let count_end = count_end.unwrap();
        let count_str = std::str::from_utf8(&data[pos..count_end]).unwrap_or("0");
        let count: usize = count_str.parse().unwrap_or(0);
        pos = count_end + 2;

        let mut parts = Vec::with_capacity(count);
        for _ in 0..count {
            if pos >= data.len() || data[pos] != b'$' {
                break;
            }
            pos += 1;
            let len_end = Self::find_crlf(data, pos);
            if len_end.is_none() {
                break;
            }
            let len_end = len_end.unwrap();
            let len_str = std::str::from_utf8(&data[pos..len_end]).unwrap_or("0");
            let len: usize = len_str.parse().unwrap_or(0);
            pos = len_end + 2;

            if pos + len > data.len() {
                break;
            }
            let value = std::str::from_utf8(&data[pos..pos + len])
                .unwrap_or("")
                .to_string();
            parts.push(value);
            pos += len + 2;
        }

        parts
    }

    fn bulk_string(data: &[u8]) -> Vec<u8> {
        let mut resp = format!("${}\r\n", data.len()).into_bytes();
        resp.extend_from_slice(data);
        resp.extend_from_slice(b"\r\n");
        resp
    }

    fn find_crlf(data: &[u8], start: usize) -> Option<usize> {
        (start..data.len().saturating_sub(1))
            .find(|&i| data[i] == b'\r' && data[i + 1] == b'\n')
    }

    fn pool_key(&self) -> PoolKey {
        PoolKey::with_protocol("127.0.0.1", self.addr.port(), "", "", Protocol::Redis)
    }
}

// ── Build helpers ───────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

static COMPONENT_BYTES: OnceLock<Vec<u8>> = OnceLock::new();

fn build_guest_component() -> &'static [u8] {
    COMPONENT_BYTES.get_or_init(|| {
        let root = workspace_root();
        let guest_dir = root.join("tests/fixtures/rust-mysql-redis-integration-guest");

        let status = Command::new("cargo")
            .args([
                "build",
                "--target",
                "wasm32-unknown-unknown",
                "--release",
            ])
            .current_dir(&guest_dir)
            .status()
            .expect("failed to run cargo build for guest fixture");
        assert!(
            status.success(),
            "guest fixture build failed with exit code {:?}",
            status.code()
        );

        let core_wasm_path = guest_dir
            .join("target/wasm32-unknown-unknown/release/rust_mysql_redis_integration_guest.wasm");

        let component_path =
            guest_dir.join("target/rust-mysql-redis-integration-guest.component.wasm");
        let status = Command::new("wasm-tools")
            .args([
                "component",
                "new",
                core_wasm_path.to_str().unwrap(),
                "-o",
                component_path.to_str().unwrap(),
            ])
            .status()
            .expect("failed to run wasm-tools component new");
        assert!(
            status.success(),
            "wasm-tools component new failed with exit code {:?}",
            status.code()
        );

        std::fs::read(&component_path).expect("failed to read compiled component")
    })
}

// ── Test host state builder ─────────────────────────────────────────

fn pool_config() -> PoolConfig {
    PoolConfig {
        max_size: 10,
        idle_timeout: Duration::from_secs(300),
        health_check_interval: Duration::from_secs(30),
        connect_timeout: Duration::from_millis(500),
        recv_timeout: Duration::from_secs(5),
        use_tls: false,
        verify_certificates: false,
        drain_timeout: Duration::from_secs(30),
    }
}

/// Build a ShimConfig with DNS entries for "mysql.test.warp.local" and
/// "redis.test.warp.local" resolving to 127.0.0.1.
fn test_shim_config() -> ShimConfig {
    let mut service_registry: HashMap<String, Vec<IpAddr>> = HashMap::new();
    service_registry.insert(
        "mysql.test.warp.local".to_string(),
        vec!["127.0.0.1".parse().unwrap()],
    );
    service_registry.insert(
        "redis.test.warp.local".to_string(),
        vec!["127.0.0.1".parse().unwrap()],
    );

    ShimConfig {
        filesystem: true,
        dns: true,
        signals: true,
        database_proxy: true,
        threading: true,
        service_registry,
        pool_config: pool_config(),
        ..ShimConfig::default()
    }
}

// ── Helper: create engine + store + instance ────────────────────────

async fn fresh_instance(
    engine: &WarpGridEngine,
    component: &Component,
) -> (
    Store<warpgrid_host::engine::HostState>,
    wasmtime::component::Instance,
) {
    let config = test_shim_config();
    let factory = Arc::new(TcpConnectionFactory::plain(
        config.pool_config.recv_timeout,
        config.pool_config.connect_timeout,
    ));
    let host_state = engine.build_host_state(&config, Some(factory));
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, component)
        .await
        .unwrap();

    (store, instance)
}

// ── Pool-level helpers (for health check tests) ─────────────────────

fn default_pool_config(max_size: usize) -> PoolConfig {
    PoolConfig {
        max_size,
        idle_timeout: Duration::from_secs(300),
        health_check_interval: Duration::from_secs(30),
        connect_timeout: Duration::from_millis(500),
        recv_timeout: Duration::from_secs(5),
        use_tls: false,
        verify_certificates: false,
        drain_timeout: Duration::from_secs(30),
    }
}

fn make_mysql_manager(config: PoolConfig) -> ConnectionPoolManager {
    let factory = Arc::new(MysqlConnectionFactory::plain(
        config.recv_timeout,
        config.connect_timeout,
    ));
    ConnectionPoolManager::new(config, factory)
}

fn make_redis_manager(config: PoolConfig) -> ConnectionPoolManager {
    let factory = Arc::new(RedisConnectionFactory::plain(
        config.recv_timeout,
        config.connect_timeout,
    ));
    ConnectionPoolManager::new(config, factory)
}

/// Perform MySQL handshake on a pool handle.
async fn do_mysql_handshake(mgr: &ConnectionPoolManager, handle: u64) {
    // Read server greeting.
    let mut greeting = Vec::new();
    for _ in 0..20 {
        let chunk = mgr.recv(handle, 4096).await.expect("recv greeting");
        greeting.extend_from_slice(&chunk);
        if !greeting.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(!greeting.is_empty(), "should receive MySQL server greeting");
    assert_eq!(greeting[4], 0x0a, "protocol version should be 0x0a");

    // Send client handshake response.
    let mut payload = Vec::new();
    payload.extend_from_slice(&0x0003ffffu32.to_le_bytes());
    payload.extend_from_slice(&0x01000000u32.to_le_bytes());
    payload.push(0x21);
    payload.extend_from_slice(&[0u8; 23]);
    payload.extend_from_slice(b"testuser\0");
    payload.push(0x00);

    let len = payload.len() as u32;
    let mut packet = Vec::with_capacity(4 + payload.len());
    packet.push((len & 0xff) as u8);
    packet.push(((len >> 8) & 0xff) as u8);
    packet.push(((len >> 16) & 0xff) as u8);
    packet.push(1);
    packet.extend_from_slice(&payload);

    mgr.send(handle, &packet).await.expect("send handshake");

    // Read auth OK.
    let mut auth_ok = Vec::new();
    for _ in 0..20 {
        let chunk = mgr.recv(handle, 1024).await.expect("recv auth OK");
        auth_ok.extend_from_slice(&chunk);
        if auth_ok.len() >= 5 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(auth_ok.len() >= 5, "should receive auth OK packet");
    assert_eq!(auth_ok[4], 0x00, "auth OK should have OK marker");
}

// ── Wasm integration tests ─────────────────────────────────────────

/// AC #1: Wasm component connects to MySQL, executes CREATE TABLE, INSERT, SELECT, DROP TABLE.
#[tokio::test(flavor = "multi_thread")]
async fn test_mysql_crud_via_wasm() {
    let mysql_server = StatefulMockMysqlServer::start();
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let (mut store, instance) = fresh_instance(&engine, &component).await;

    let func = instance
        .get_typed_func::<(u16,), (Result<String, String>,)>(
            &mut store,
            "test-mysql-crud",
        )
        .unwrap();

    let (result,) = func
        .call_async(&mut store, (mysql_server.addr.port(),))
        .await
        .unwrap();

    let row_data = result.expect("MySQL CRUD test should succeed");

    // The guest does CREATE TABLE test_items (id INT, name VARCHAR(255)),
    // INSERT INTO test_items (id, name) VALUES (1, 'widget'),
    // SELECT id, name FROM test_items, then DROP TABLE test_items.
    // row_data should contain "1,widget".
    assert!(
        row_data.contains("1") && row_data.contains("widget"),
        "SELECT result should contain inserted row data '1,widget', got: '{row_data}'"
    );
}

/// AC #2: Wasm component connects to Redis, executes SET, GET, DEL.
#[tokio::test(flavor = "multi_thread")]
async fn test_redis_crud_via_wasm() {
    let redis_server = StatefulMockRedisServer::start();
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let (mut store, instance) = fresh_instance(&engine, &component).await;

    let func = instance
        .get_typed_func::<(u16,), (Result<String, String>,)>(
            &mut store,
            "test-redis-crud",
        )
        .unwrap();

    let (result,) = func
        .call_async(&mut store, (redis_server.addr.port(),))
        .await
        .unwrap();

    let value = result.expect("Redis CRUD test should succeed");

    // The guest does SET test_key hello_redis, GET test_key, DEL test_key.
    assert_eq!(
        value, "hello_redis",
        "GET should return the value that was SET"
    );
}

/// AC #3: MySQL connection pooling works (connect, close, reconnect reuses pool).
#[tokio::test(flavor = "multi_thread")]
async fn test_mysql_pool_reuse_via_wasm() {
    let mysql_server = StatefulMockMysqlServer::start();
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let (mut store, instance) = fresh_instance(&engine, &component).await;

    let func = instance
        .get_typed_func::<(u16,), (Result<String, String>,)>(
            &mut store,
            "test-mysql-pool-reuse",
        )
        .unwrap();

    let (result,) = func
        .call_async(&mut store, (mysql_server.addr.port(),))
        .await
        .unwrap();

    let status = result.expect("MySQL pool reuse test should succeed");
    assert_eq!(
        status, "mysql_pool_reuse:ok",
        "pool reuse should report success"
    );
}

/// AC #4: Redis connection pooling works (connect, close, reconnect reuses pool).
#[tokio::test(flavor = "multi_thread")]
async fn test_redis_pool_reuse_via_wasm() {
    let redis_server = StatefulMockRedisServer::start();
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let (mut store, instance) = fresh_instance(&engine, &component).await;

    let func = instance
        .get_typed_func::<(u16,), (Result<String, String>,)>(
            &mut store,
            "test-redis-pool-reuse",
        )
        .unwrap();

    let (result,) = func
        .call_async(&mut store, (redis_server.addr.port(),))
        .await
        .unwrap();

    let status = result.expect("Redis pool reuse test should succeed");
    assert_eq!(
        status, "redis_pool_reuse:ok",
        "pool reuse should report success"
    );
}

// ── Pool-level health check tests ───────────────────────────────────

/// AC #5: COM_PING health check detects dead MySQL connections.
#[tokio::test]
async fn test_mysql_health_check_removes_dead() {
    let server = StatefulMockMysqlServer::start_close_after_auth();
    let mgr = make_mysql_manager(default_pool_config(5));
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();
    do_mysql_handshake(&mgr, h).await;

    // Release to idle pool.
    mgr.release(h).await.unwrap();
    assert_eq!(mgr.stats(&key).await.idle, 1);

    // Give server time to close.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Health check should detect closed connection via COM_PING failure.
    mgr.health_check_idle().await;

    assert_eq!(
        mgr.stats(&key).await.idle,
        0,
        "dead MySQL connection should be removed by COM_PING health check"
    );
}

/// AC #5: PING health check detects dead Redis connections.
#[tokio::test]
async fn test_redis_health_check_removes_dead() {
    let server = StatefulMockRedisServer::start_close_immediately();
    let mgr = make_redis_manager(default_pool_config(5));
    let key = server.pool_key();

    let h = mgr.checkout(&key, None).await.unwrap();

    // Release to idle pool.
    mgr.release(h).await.unwrap();
    assert_eq!(mgr.stats(&key).await.idle, 1);

    // Give server time to close.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Health check should detect closed connection via PING failure.
    mgr.health_check_idle().await;

    assert_eq!(
        mgr.stats(&key).await.idle,
        0,
        "dead Redis connection should be removed by PING health check"
    );
}
