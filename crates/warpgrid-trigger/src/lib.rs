//! warpgrid-trigger — HTTP trigger handler for WarpGrid.
//!
//! Bridges inbound HTTP requests to Wasm components via the wasi-http
//! `incoming-handler` interface. Each HTTP request is routed to a
//! component instance, which processes it and returns an HTTP response.
//!
//! # Architecture
//!
//! ```text
//! HTTP client
//!   │
//!   ▼
//! hyper server
//!   │
//!   ├── Convert hyper::Request → wasi-http IncomingRequest
//!   ├── Call component's incoming-handler.handle()
//!   ├── Convert wasi-http OutgoingResponse → hyper::Response
//!   │
//!   ▼
//! HTTP response
//! ```
//!
//! The handler uses `wasmtime-wasi-http` for type conversions and
//! the proxy world binding.

pub mod handler;
pub mod convert;

pub use handler::HttpTrigger;
