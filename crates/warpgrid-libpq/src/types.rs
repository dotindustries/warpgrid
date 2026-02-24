//! Safe wrapper types for libpq results.

#[cfg(target_arch = "wasm32")]
use std::ffi::CStr;
#[cfg(target_arch = "wasm32")]
use std::os::raw::c_int;

#[cfg(target_arch = "wasm32")]
use crate::ffi;

/// Errors from PostgreSQL operations.
#[derive(Debug, thiserror::Error)]
pub enum PgError {
    #[error("connection failed: {0}")]
    ConnectionFailed(String),

    #[error("query failed: {0}")]
    QueryFailed(String),

    #[error("null result from server")]
    NullResult,

    #[error("not available on this platform")]
    NotAvailable,
}

/// Connection status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnStatus {
    Ok,
    Bad,
}

/// Query execution result status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecStatus {
    EmptyQuery,
    CommandOk,
    TuplesOk,
    CopyOut,
    CopyIn,
    BadResponse,
    NonfatalError,
    FatalError,
    CopyBoth,
    SingleTuple,
    PipelineSync,
    PipelineAborted,
}

impl ExecStatus {
    /// Returns true if the status indicates success.
    pub fn is_ok(self) -> bool {
        matches!(self, Self::CommandOk | Self::TuplesOk | Self::SingleTuple)
    }
}

/// Owned query result. Calls `PQclear` on drop.
pub struct PgResult {
    #[cfg(target_arch = "wasm32")]
    ptr: *mut ffi::PGresult,
    #[cfg(not(target_arch = "wasm32"))]
    _phantom: std::marker::PhantomData<()>,
}

// PgResult is safe to Send on wasm32 (single-threaded).
// On native targets, the struct is a phantom with no pointer.
unsafe impl Send for PgResult {}

impl PgResult {
    /// Create a PgResult from a raw pointer. Returns `Err` if null.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn from_raw(ptr: *mut ffi::PGresult) -> Result<Self, PgError> {
        if ptr.is_null() {
            return Err(PgError::NullResult);
        }
        Ok(Self { ptr })
    }

    /// The execution status of this result.
    #[cfg(target_arch = "wasm32")]
    pub fn status(&self) -> ExecStatus {
        let raw = unsafe { ffi::PQresultStatus(self.ptr) };
        match raw {
            ffi::ExecStatusType::EmptyQuery => ExecStatus::EmptyQuery,
            ffi::ExecStatusType::CommandOk => ExecStatus::CommandOk,
            ffi::ExecStatusType::TuplesOk => ExecStatus::TuplesOk,
            ffi::ExecStatusType::CopyOut => ExecStatus::CopyOut,
            ffi::ExecStatusType::CopyIn => ExecStatus::CopyIn,
            ffi::ExecStatusType::BadResponse => ExecStatus::BadResponse,
            ffi::ExecStatusType::NonfatalError => ExecStatus::NonfatalError,
            ffi::ExecStatusType::FatalError => ExecStatus::FatalError,
            ffi::ExecStatusType::CopyBoth => ExecStatus::CopyBoth,
            ffi::ExecStatusType::SingleTuple => ExecStatus::SingleTuple,
            ffi::ExecStatusType::PipelineSync => ExecStatus::PipelineSync,
            ffi::ExecStatusType::PipelineAborted => ExecStatus::PipelineAborted,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn status(&self) -> ExecStatus {
        ExecStatus::FatalError
    }

    /// Error message from the result (empty string if none).
    #[cfg(target_arch = "wasm32")]
    pub fn error_message(&self) -> String {
        let ptr = unsafe { ffi::PQresultErrorMessage(self.ptr) };
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

    /// Number of rows in the result.
    #[cfg(target_arch = "wasm32")]
    pub fn num_rows(&self) -> usize {
        let n = unsafe { ffi::PQntuples(self.ptr) };
        n.max(0) as usize
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn num_rows(&self) -> usize {
        0
    }

    /// Number of columns in the result.
    #[cfg(target_arch = "wasm32")]
    pub fn num_cols(&self) -> usize {
        let n = unsafe { ffi::PQnfields(self.ptr) };
        n.max(0) as usize
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn num_cols(&self) -> usize {
        0
    }

    /// Get a column name by index.
    #[cfg(target_arch = "wasm32")]
    pub fn column_name(&self, col: usize) -> Option<&str> {
        if col >= self.num_cols() {
            return None;
        }
        let ptr = unsafe { ffi::PQfname(self.ptr, col as c_int) };
        if ptr.is_null() {
            return None;
        }
        unsafe { CStr::from_ptr(ptr) }.to_str().ok()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn column_name(&self, _col: usize) -> Option<&str> {
        None
    }

    /// Get a row by index, borrowing from this result.
    pub fn row(&self, row: usize) -> Option<PgRow<'_>> {
        if row >= self.num_rows() {
            return None;
        }
        Some(PgRow { result: self, row })
    }

    /// Iterate over all rows.
    pub fn rows(&self) -> impl Iterator<Item = PgRow<'_>> {
        (0..self.num_rows()).map(move |i| PgRow {
            result: self,
            row: i,
        })
    }

    /// The command status tag (e.g., "SELECT 5", "INSERT 0 1").
    #[cfg(target_arch = "wasm32")]
    pub fn cmd_status(&self) -> String {
        let ptr = unsafe { ffi::PQcmdStatus(self.ptr as *mut _) };
        if ptr.is_null() {
            return String::new();
        }
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn cmd_status(&self) -> String {
        String::new()
    }

    /// Number of rows affected by the command.
    #[cfg(target_arch = "wasm32")]
    pub fn cmd_tuples(&self) -> u64 {
        let ptr = unsafe { ffi::PQcmdTuples(self.ptr as *mut _) };
        if ptr.is_null() {
            return 0;
        }
        let s = unsafe { CStr::from_ptr(ptr) }.to_string_lossy();
        s.parse().unwrap_or(0)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn cmd_tuples(&self) -> u64 {
        0
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for PgResult {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { ffi::PQclear(self.ptr) };
        }
    }
}

/// A borrowed row within a PgResult.
#[derive(Clone, Copy)]
pub struct PgRow<'a> {
    result: &'a PgResult,
    #[allow(dead_code)] // used only on wasm32 target
    row: usize,
}

impl<'a> PgRow<'a> {
    /// Get a column value as a string. Returns `None` if the column is NULL.
    #[cfg(target_arch = "wasm32")]
    pub fn get(&self, col: usize) -> Option<&'a str> {
        if col >= self.result.num_cols() {
            return None;
        }
        let is_null = unsafe { ffi::PQgetisnull(self.result.ptr, self.row as c_int, col as c_int) };
        if is_null != 0 {
            return None;
        }
        let ptr = unsafe { ffi::PQgetvalue(self.result.ptr, self.row as c_int, col as c_int) };
        if ptr.is_null() {
            return None;
        }
        unsafe { CStr::from_ptr(ptr) }.to_str().ok()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn get(&self, _col: usize) -> Option<&'a str> {
        None
    }

    /// Check if a column is NULL.
    #[cfg(target_arch = "wasm32")]
    pub fn is_null(&self, col: usize) -> bool {
        if col >= self.result.num_cols() {
            return true;
        }
        unsafe { ffi::PQgetisnull(self.result.ptr, self.row as c_int, col as c_int) != 0 }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn is_null(&self, _col: usize) -> bool {
        true
    }

    /// Length of the value at this column (in bytes).
    #[cfg(target_arch = "wasm32")]
    pub fn len(&self, col: usize) -> usize {
        if col >= self.result.num_cols() {
            return 0;
        }
        let n = unsafe { ffi::PQgetlength(self.result.ptr, self.row as c_int, col as c_int) };
        n.max(0) as usize
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn len(&self, _col: usize) -> usize {
        0
    }

    /// Number of columns in this row.
    pub fn num_cols(&self) -> usize {
        self.result.num_cols()
    }
}
