//! Raw FFI bindings to libpq (PostgreSQL client library).
//!
//! These bindings target the wasm32-wasip2 cross-compiled libpq.a.
//! On native targets, the module is empty (no FFI available).

#![allow(non_camel_case_types)]
#![allow(dead_code)]

#[cfg(target_arch = "wasm32")]
use std::os::raw::{c_char, c_int, c_void};

/// Opaque connection handle.
#[repr(C)]
pub struct PGconn {
    _private: [u8; 0],
}

/// Opaque result handle.
#[repr(C)]
pub struct PGresult {
    _private: [u8; 0],
}

/// PostgreSQL OID type.
pub type Oid = u32;

/// Connection status codes (matches ConnStatusType in libpq-fe.h).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnStatusType {
    ConnectionOk = 0,
    ConnectionBad = 1,
}

/// Query execution result status codes (matches ExecStatusType in libpq-fe.h).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecStatusType {
    EmptyQuery = 0,
    CommandOk = 1,
    TuplesOk = 2,
    CopyOut = 3,
    CopyIn = 4,
    BadResponse = 5,
    NonfatalError = 6,
    FatalError = 7,
    CopyBoth = 8,
    SingleTuple = 9,
    PipelineSync = 10,
    PipelineAborted = 11,
}

/// Transaction status codes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PGTransactionStatusType {
    Idle = 0,
    Active = 1,
    InTrans = 2,
    InError = 3,
    Unknown = 4,
}

#[cfg(target_arch = "wasm32")]
unsafe extern "C" {
    // ── Connection ──────────────────────────────────────────────
    pub fn PQconnectdb(conninfo: *const c_char) -> *mut PGconn;
    pub fn PQfinish(conn: *mut PGconn);
    pub fn PQstatus(conn: *const PGconn) -> ConnStatusType;
    pub fn PQerrorMessage(conn: *const PGconn) -> *const c_char;
    pub fn PQdb(conn: *const PGconn) -> *const c_char;
    pub fn PQuser(conn: *const PGconn) -> *const c_char;
    pub fn PQhost(conn: *const PGconn) -> *const c_char;
    pub fn PQport(conn: *const PGconn) -> *const c_char;
    pub fn PQserverVersion(conn: *const PGconn) -> c_int;
    pub fn PQprotocolVersion(conn: *const PGconn) -> c_int;
    pub fn PQparameterStatus(conn: *const PGconn, param: *const c_char) -> *const c_char;
    pub fn PQtransactionStatus(conn: *const PGconn) -> PGTransactionStatusType;
    pub fn PQbackendPID(conn: *const PGconn) -> c_int;
    pub fn PQreset(conn: *mut PGconn);

    // ── Query execution ─────────────────────────────────────────
    pub fn PQexec(conn: *mut PGconn, query: *const c_char) -> *mut PGresult;
    pub fn PQexecParams(
        conn: *mut PGconn,
        command: *const c_char,
        n_params: c_int,
        param_types: *const Oid,
        param_values: *const *const c_char,
        param_lengths: *const c_int,
        param_formats: *const c_int,
        result_format: c_int,
    ) -> *mut PGresult;
    pub fn PQprepare(
        conn: *mut PGconn,
        stmt_name: *const c_char,
        query: *const c_char,
        n_params: c_int,
        param_types: *const Oid,
    ) -> *mut PGresult;
    pub fn PQexecPrepared(
        conn: *mut PGconn,
        stmt_name: *const c_char,
        n_params: c_int,
        param_values: *const *const c_char,
        param_lengths: *const c_int,
        param_formats: *const c_int,
        result_format: c_int,
    ) -> *mut PGresult;

    // ── Result accessors ────────────────────────────────────────
    pub fn PQresultStatus(res: *const PGresult) -> ExecStatusType;
    pub fn PQresultErrorMessage(res: *const PGresult) -> *const c_char;
    pub fn PQntuples(res: *const PGresult) -> c_int;
    pub fn PQnfields(res: *const PGresult) -> c_int;
    pub fn PQfname(res: *const PGresult, field_num: c_int) -> *const c_char;
    pub fn PQfnumber(res: *const PGresult, field_name: *const c_char) -> c_int;
    pub fn PQgetvalue(res: *const PGresult, tup_num: c_int, field_num: c_int) -> *const c_char;
    pub fn PQgetlength(res: *const PGresult, tup_num: c_int, field_num: c_int) -> c_int;
    pub fn PQgetisnull(res: *const PGresult, tup_num: c_int, field_num: c_int) -> c_int;
    pub fn PQcmdStatus(res: *mut PGresult) -> *const c_char;
    pub fn PQcmdTuples(res: *mut PGresult) -> *const c_char;
    pub fn PQclear(res: *mut PGresult);

    // ── String escaping ─────────────────────────────────────────
    pub fn PQescapeLiteral(
        conn: *mut PGconn,
        str: *const c_char,
        len: usize,
    ) -> *mut c_char;
    pub fn PQescapeIdentifier(
        conn: *mut PGconn,
        str: *const c_char,
        len: usize,
    ) -> *mut c_char;
    pub fn PQfreemem(ptr: *mut c_void);

    // ── Misc ────────────────────────────────────────────────────
    pub fn PQlibVersion() -> c_int;
}
