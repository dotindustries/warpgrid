//! PostgreSQL storage layer for wastebin.
//!
//! Uses warpgrid-libpq for database access. Each WarpGrid instance
//! gets tenant isolation via `instance_id` in all queries.

use warpgrid_libpq::{PgConnection, PgError};

use crate::paste::{CreatePasteRequest, Paste, generate_id};

/// Storage backed by PostgreSQL through the WarpGrid DB proxy shim.
pub struct Storage {
    conn: PgConnection,
    instance_id: String,
}

impl Storage {
    /// Connect to PostgreSQL and create the storage layer.
    pub fn connect(conninfo: &str, instance_id: &str) -> Result<Self, PgError> {
        let conn = PgConnection::connect(conninfo)?;
        Ok(Self {
            conn,
            instance_id: instance_id.to_string(),
        })
    }

    /// Run the migration to create tables if they don't exist.
    pub fn migrate(&mut self) -> Result<(), PgError> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS pastes (
                id          TEXT NOT NULL,
                instance_id TEXT NOT NULL,
                title       TEXT,
                content     BYTEA NOT NULL,
                language    TEXT,
                compressed  BOOLEAN NOT NULL DEFAULT false,
                burn_after  BOOLEAN NOT NULL DEFAULT false,
                created_at  BIGINT NOT NULL,
                expires_at  BIGINT,
                PRIMARY KEY (instance_id, id)
            )",
        )?;
        Ok(())
    }

    /// Create a new paste and return it.
    pub fn create_paste(&mut self, req: &CreatePasteRequest) -> Result<Paste, PgError> {
        let id = generate_id();
        let now = current_epoch_secs();
        let expires_at = req.expires_in_seconds.map(|s| now + s);
        let burn = req.burn_after.unwrap_or(false);
        let content = req.content.as_bytes().to_vec();
        let content_hex = hex_encode(&content);

        let expires_str = expires_at.map(|e| e.to_string());
        let now_str = now.to_string();

        let params: Vec<&str> = vec![
            &id,
            &self.instance_id,
            req.title.as_deref().unwrap_or(""),
            &content_hex,
            req.language.as_deref().unwrap_or(""),
            if burn { "true" } else { "false" },
            &now_str,
            expires_str.as_deref().unwrap_or(""),
        ];

        // Use plain text params — empty string for NULL-able fields
        let sql = if expires_at.is_some() {
            "INSERT INTO pastes (id, instance_id, title, content, language, compressed, burn_after, created_at, expires_at)
             VALUES ($1, $2, NULLIF($3, ''), decode($4, 'hex'), NULLIF($5, ''), false, $6::boolean, $7::bigint, $8::bigint)"
        } else {
            "INSERT INTO pastes (id, instance_id, title, content, language, compressed, burn_after, created_at)
             VALUES ($1, $2, NULLIF($3, ''), decode($4, 'hex'), NULLIF($5, ''), false, $6::boolean, $7::bigint)"
        };

        let final_params = if expires_at.is_some() {
            &params[..]
        } else {
            &params[..7]
        };

        self.conn.execute_params(sql, final_params)?;

        Ok(Paste {
            id,
            title: req.title.clone(),
            content,
            language: req.language.clone(),
            compressed: false,
            burn_after: burn,
            created_at: now,
            expires_at,
        })
    }

    /// Get a paste by ID. If it's burn-after-reading, deletes it after retrieval.
    pub fn get_paste(&mut self, id: &str) -> Result<Option<Paste>, PgError> {
        let result = self.conn.query_params(
            "SELECT id, title, content, language, compressed, burn_after, created_at, expires_at
             FROM pastes WHERE instance_id = $1 AND id = $2",
            &[&self.instance_id, id],
        )?;

        let row = match result.row(0) {
            Some(r) => r,
            None => return Ok(None),
        };

        let paste_id = row.get(0).unwrap_or("").to_string();
        let title = row.get(1).map(|s| s.to_string());
        let content_str = row.get(2).unwrap_or("");
        let content = content_str.as_bytes().to_vec();
        let language = row.get(3).map(|s| s.to_string());
        let compressed = row.get(4).unwrap_or("f") == "t";
        let burn_after = row.get(5).unwrap_or("f") == "t";
        let created_at: u64 = row.get(6).unwrap_or("0").parse().unwrap_or(0);
        let expires_at: Option<u64> = row.get(7).and_then(|s| s.parse().ok());

        let paste = Paste {
            id: paste_id,
            title,
            content,
            language,
            compressed,
            burn_after,
            created_at,
            expires_at,
        };

        // Burn after reading
        if burn_after {
            let _ = self.conn.execute_params(
                "DELETE FROM pastes WHERE instance_id = $1 AND id = $2",
                &[&self.instance_id, id],
            );
        }

        Ok(Some(paste))
    }

    /// Delete a paste by ID.
    pub fn delete_paste(&mut self, id: &str) -> Result<bool, PgError> {
        let n = self.conn.execute_params(
            "DELETE FROM pastes WHERE instance_id = $1 AND id = $2",
            &[&self.instance_id, id],
        )?;
        Ok(n > 0)
    }

    /// List recent pastes (newest first).
    pub fn list_pastes(&mut self, limit: usize) -> Result<Vec<Paste>, PgError> {
        let limit_str = limit.to_string();
        let result = self.conn.query_params(
            "SELECT id, title, language, created_at
             FROM pastes WHERE instance_id = $1
             ORDER BY created_at DESC LIMIT $2::int",
            &[&self.instance_id, &limit_str],
        )?;

        let mut pastes = Vec::new();
        for row in result.rows() {
            pastes.push(Paste {
                id: row.get(0).unwrap_or("").to_string(),
                title: row.get(1).map(|s| s.to_string()),
                content: Vec::new(), // Not loaded for list view
                language: row.get(2).map(|s| s.to_string()),
                compressed: false,
                burn_after: false,
                created_at: row.get(3).unwrap_or("0").parse().unwrap_or(0),
                expires_at: None,
            });
        }
        Ok(pastes)
    }

    /// Count total pastes for this instance.
    pub fn paste_count(&mut self) -> Result<u64, PgError> {
        let result = self.conn.query_params(
            "SELECT COUNT(*) FROM pastes WHERE instance_id = $1",
            &[&self.instance_id],
        )?;
        match result.row(0) {
            Some(row) => Ok(row.get(0).unwrap_or("0").parse().unwrap_or(0)),
            None => Ok(0),
        }
    }
}

/// Get current epoch time in seconds (simplified — no std::time on wasm32-wasip2).
fn current_epoch_secs() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        // WASI provides clock_time_get
        0 // Placeholder — real impl would use wasi clocks
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

/// Hex-encode bytes for PostgreSQL decode($1, 'hex').
fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
