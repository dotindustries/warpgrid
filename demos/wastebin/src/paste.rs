//! Paste data types.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// A paste stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paste {
    pub id: String,
    pub title: Option<String>,
    pub content: Vec<u8>,
    pub language: Option<String>,
    pub compressed: bool,
    pub burn_after: bool,
    pub created_at: u64,
    pub expires_at: Option<u64>,
}

/// Request to create a new paste.
#[derive(Debug, Deserialize)]
pub struct CreatePasteRequest {
    pub title: Option<String>,
    pub content: String,
    pub language: Option<String>,
    pub burn_after: Option<bool>,
    pub expires_in_seconds: Option<u64>,
}

/// Generate a short, unique paste ID.
///
/// Uses an atomic counter mixed with a simple hash for uniqueness
/// within a single instance. Cross-instance uniqueness is guaranteed
/// by the (instance_id, id) composite key.
pub fn generate_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);

    // Mix bits for less predictable IDs
    let mixed = n
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);

    // Encode as base62 (6 chars)
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut result = String::with_capacity(6);
    let mut val = mixed;
    for _ in 0..6 {
        result.push(ALPHABET[(val % 62) as usize] as char);
        val /= 62;
    }
    result
}
