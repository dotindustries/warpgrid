//! Safe Rust wrapper around libpq compiled to wasm32-wasip2.
//!
//! On wasm32 targets, this crate links against `build/libpq-wasm/lib/libpq.a`
//! and provides a safe API for connecting to PostgreSQL and executing queries.
//!
//! On native targets, the crate compiles but all operations return
//! `PgError::NotAvailable`. This allows the workspace to build on the
//! developer's machine without the cross-compiled library.

pub mod ffi;
pub mod types;

pub use types::{ConnStatus, ExecStatus, PgError, PgResult, PgRow};

#[cfg(target_arch = "wasm32")]
use std::ffi::{CStr, CString};

/// A connection to a PostgreSQL server.
///
/// Wraps a `PGconn*` and provides safe query methods.
/// Calls `PQfinish` on drop.
pub struct PgConnection {
    #[cfg(target_arch = "wasm32")]
    conn: *mut ffi::PGconn,
    #[cfg(not(target_arch = "wasm32"))]
    _phantom: std::marker::PhantomData<()>,
}

impl std::fmt::Debug for PgConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgConnection").finish_non_exhaustive()
    }
}

// Safety: on wasm32, there is only one thread. On native, the struct
// holds no real pointer.
unsafe impl Send for PgConnection {}

impl PgConnection {
    /// Connect to a PostgreSQL server using a connection string.
    ///
    /// # Example conninfo formats
    /// - `"host=localhost port=5432 dbname=mydb"`
    /// - `"postgresql://user:pass@host:5432/dbname"`
    #[cfg(target_arch = "wasm32")]
    pub fn connect(conninfo: &str) -> Result<Self, PgError> {
        let c_conninfo = CString::new(conninfo)
            .map_err(|_| PgError::ConnectionFailed("invalid conninfo string".into()))?;

        let conn = unsafe { ffi::PQconnectdb(c_conninfo.as_ptr()) };
        if conn.is_null() {
            return Err(PgError::ConnectionFailed(
                "PQconnectdb returned null".into(),
            ));
        }

        let status = unsafe { ffi::PQstatus(conn) };
        if status != ffi::ConnStatusType::ConnectionOk {
            let msg = unsafe {
                let ptr = ffi::PQerrorMessage(conn);
                if ptr.is_null() {
                    "unknown error".to_string()
                } else {
                    CStr::from_ptr(ptr).to_string_lossy().into_owned()
                }
            };
            unsafe { ffi::PQfinish(conn) };
            return Err(PgError::ConnectionFailed(msg));
        }

        Ok(Self { conn })
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn connect(_conninfo: &str) -> Result<Self, PgError> {
        Err(PgError::NotAvailable)
    }

    /// Execute a query and return the result.
    #[cfg(target_arch = "wasm32")]
    pub fn query(&mut self, sql: &str) -> Result<PgResult, PgError> {
        let c_sql = CString::new(sql)
            .map_err(|_| PgError::QueryFailed("invalid SQL string".into()))?;

        let res = unsafe { ffi::PQexec(self.conn, c_sql.as_ptr()) };
        let result = PgResult::from_raw(res)?;

        if !result.status().is_ok() {
            return Err(PgError::QueryFailed(result.error_message()));
        }
        Ok(result)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn query(&mut self, _sql: &str) -> Result<PgResult, PgError> {
        Err(PgError::NotAvailable)
    }

    /// Execute a parameterized query.
    ///
    /// Parameters are passed as text (`$1`, `$2`, etc. in the SQL).
    #[cfg(target_arch = "wasm32")]
    pub fn query_params(&mut self, sql: &str, params: &[&str]) -> Result<PgResult, PgError> {
        let c_sql = CString::new(sql)
            .map_err(|_| PgError::QueryFailed("invalid SQL string".into()))?;

        let c_params: Vec<CString> = params
            .iter()
            .map(|p| CString::new(*p).unwrap_or_default())
            .collect();
        let param_ptrs: Vec<*const std::os::raw::c_char> =
            c_params.iter().map(|p| p.as_ptr()).collect();

        let res = unsafe {
            ffi::PQexecParams(
                self.conn,
                c_sql.as_ptr(),
                params.len() as std::os::raw::c_int,
                std::ptr::null(),        // let server infer types
                param_ptrs.as_ptr(),
                std::ptr::null(),        // text format lengths (ignored for text)
                std::ptr::null(),        // all text format
                0,                       // result in text format
            )
        };
        let result = PgResult::from_raw(res)?;

        if !result.status().is_ok() {
            return Err(PgError::QueryFailed(result.error_message()));
        }
        Ok(result)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn query_params(&mut self, _sql: &str, _params: &[&str]) -> Result<PgResult, PgError> {
        Err(PgError::NotAvailable)
    }

    /// Execute a command that doesn't return rows. Returns the number of
    /// rows affected.
    #[cfg(target_arch = "wasm32")]
    pub fn execute(&mut self, sql: &str) -> Result<u64, PgError> {
        let result = self.query(sql)?;
        Ok(result.cmd_tuples())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn execute(&mut self, _sql: &str) -> Result<u64, PgError> {
        Err(PgError::NotAvailable)
    }

    /// Execute a parameterized command. Returns the number of rows affected.
    #[cfg(target_arch = "wasm32")]
    pub fn execute_params(&mut self, sql: &str, params: &[&str]) -> Result<u64, PgError> {
        let result = self.query_params(sql, params)?;
        Ok(result.cmd_tuples())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn execute_params(&mut self, _sql: &str, _params: &[&str]) -> Result<u64, PgError> {
        Err(PgError::NotAvailable)
    }

    /// Escape a literal string for safe inclusion in SQL.
    #[cfg(target_arch = "wasm32")]
    pub fn escape_literal(&mut self, s: &str) -> Result<String, PgError> {
        let c_str = CString::new(s)
            .map_err(|_| PgError::QueryFailed("invalid string for escaping".into()))?;
        let escaped = unsafe { ffi::PQescapeLiteral(self.conn, c_str.as_ptr(), s.len()) };
        if escaped.is_null() {
            return Err(PgError::QueryFailed(self.error_message()));
        }
        let result = unsafe { CStr::from_ptr(escaped) }
            .to_string_lossy()
            .into_owned();
        unsafe { ffi::PQfreemem(escaped as *mut _) };
        Ok(result)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn escape_literal(&mut self, _s: &str) -> Result<String, PgError> {
        Err(PgError::NotAvailable)
    }

    /// Escape an identifier (table/column name) for safe inclusion in SQL.
    #[cfg(target_arch = "wasm32")]
    pub fn escape_identifier(&mut self, s: &str) -> Result<String, PgError> {
        let c_str = CString::new(s)
            .map_err(|_| PgError::QueryFailed("invalid string for escaping".into()))?;
        let escaped = unsafe { ffi::PQescapeIdentifier(self.conn, c_str.as_ptr(), s.len()) };
        if escaped.is_null() {
            return Err(PgError::QueryFailed(self.error_message()));
        }
        let result = unsafe { CStr::from_ptr(escaped) }
            .to_string_lossy()
            .into_owned();
        unsafe { ffi::PQfreemem(escaped as *mut _) };
        Ok(result)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn escape_identifier(&mut self, _s: &str) -> Result<String, PgError> {
        Err(PgError::NotAvailable)
    }

    /// Get the last error message from the connection.
    #[cfg(target_arch = "wasm32")]
    pub fn error_message(&self) -> String {
        let ptr = unsafe { ffi::PQerrorMessage(self.conn) };
        if ptr.is_null() {
            return String::new();
        }
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn error_message(&self) -> String {
        "not available on this platform".to_string()
    }

    /// Get the server version number.
    #[cfg(target_arch = "wasm32")]
    pub fn server_version(&self) -> i32 {
        unsafe { ffi::PQserverVersion(self.conn) }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn server_version(&self) -> i32 {
        0
    }

    /// Get the libpq library version.
    #[cfg(target_arch = "wasm32")]
    pub fn lib_version() -> i32 {
        unsafe { ffi::PQlibVersion() }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn lib_version() -> i32 {
        0
    }

    /// Get a server parameter value (e.g., "server_version", "server_encoding").
    #[cfg(target_arch = "wasm32")]
    pub fn parameter_status(&self, name: &str) -> Option<String> {
        let c_name = CString::new(name).ok()?;
        let ptr = unsafe { ffi::PQparameterStatus(self.conn, c_name.as_ptr()) };
        if ptr.is_null() {
            return None;
        }
        Some(
            unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned(),
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn parameter_status(&self, _name: &str) -> Option<String> {
        None
    }

    /// Get the database name.
    #[cfg(target_arch = "wasm32")]
    pub fn database(&self) -> String {
        let ptr = unsafe { ffi::PQdb(self.conn) };
        if ptr.is_null() {
            return String::new();
        }
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn database(&self) -> String {
        String::new()
    }

    /// Get the connected user name.
    #[cfg(target_arch = "wasm32")]
    pub fn user(&self) -> String {
        let ptr = unsafe { ffi::PQuser(self.conn) };
        if ptr.is_null() {
            return String::new();
        }
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn user(&self) -> String {
        String::new()
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for PgConnection {
    fn drop(&mut self) {
        if !self.conn.is_null() {
            unsafe { ffi::PQfinish(self.conn) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_returns_not_available_on_native() {
        let result = PgConnection::connect("host=localhost");
        assert!(result.is_err());
        match result.unwrap_err() {
            PgError::NotAvailable => {}
            other => panic!("expected NotAvailable, got: {other}"),
        }
    }

    #[test]
    fn exec_status_is_ok() {
        assert!(ExecStatus::CommandOk.is_ok());
        assert!(ExecStatus::TuplesOk.is_ok());
        assert!(ExecStatus::SingleTuple.is_ok());
        assert!(!ExecStatus::FatalError.is_ok());
        assert!(!ExecStatus::BadResponse.is_ok());
    }

    #[test]
    fn lib_version_is_zero_on_native() {
        assert_eq!(PgConnection::lib_version(), 0);
    }
}
