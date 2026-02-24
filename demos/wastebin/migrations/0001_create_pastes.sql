-- Paste storage for wastebin demo.
-- Each WarpGrid instance gets tenant isolation via instance_id.

CREATE TABLE IF NOT EXISTS pastes (
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
);

CREATE INDEX IF NOT EXISTS idx_pastes_instance_created
    ON pastes (instance_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_pastes_expires
    ON pastes (expires_at)
    WHERE expires_at IS NOT NULL;
