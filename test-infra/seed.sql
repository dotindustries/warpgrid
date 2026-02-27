-- seed.sql — Test data for WarpGrid integration tests.
--
-- Creates:
--   - test_users table with 5 seed rows
--   - test_analytics table for T6 write tests
--
-- This file is mounted into Postgres via docker-entrypoint-initdb.d/
-- and runs automatically on first container start.

-- ─── test_users ───────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS test_users (
    id         SERIAL PRIMARY KEY,
    name       TEXT NOT NULL,
    email      TEXT NOT NULL UNIQUE,
    role       TEXT NOT NULL DEFAULT 'user',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO test_users (name, email, role) VALUES
    ('Alice Johnson',   'alice@example.com',   'admin'),
    ('Bob Smith',       'bob@example.com',     'user'),
    ('Charlie Brown',   'charlie@example.com', 'user'),
    ('Diana Prince',    'diana@example.com',   'moderator'),
    ('Eve Williams',    'eve@example.com',     'user');

-- ─── test_analytics ───────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS test_analytics (
    id           SERIAL PRIMARY KEY,
    event_type   TEXT NOT NULL,
    event_data   JSONB NOT NULL DEFAULT '{}',
    source_service TEXT,
    user_id      INTEGER REFERENCES test_users(id),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Index for typical query patterns in integration tests
CREATE INDEX IF NOT EXISTS idx_analytics_event_type ON test_analytics(event_type);
CREATE INDEX IF NOT EXISTS idx_analytics_created_at ON test_analytics(created_at);
