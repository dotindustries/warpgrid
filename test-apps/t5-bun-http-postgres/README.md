# T5: Bun HTTP + Postgres Integration Test

Cross-domain integration test validating that a Bun handler compiled
via `warp pack --lang bun` can serve HTTP requests and query PostgreSQL
through the WarpGrid database proxy shim — with behavioral parity to T4.

## Architecture

```
                  bun build + jco componentize
handler.js  ──────────────────────────────────►  handler.wasm
                                                    │
                                                    ▼
                                           WarpGridEngine (Rust)
                                           ├─ wasi:http host
                                           ├─ warpgrid:shim/database-proxy
                                           └─ warpgrid:shim/filesystem
                                                    │
                                                    ▼
                                           Mock Postgres Server
```

## Handlers

| File | Description | Status |
|------|-------------|--------|
| `src/handler.js` | Full handler with warpgrid:shim DB access + Bun polyfills | Blocked on US-606 |
| `src/handler-standalone.js` | Standalone with in-memory data + Bun polyfill stubs | Working |

## Routes

| Method | Path | Description |
|--------|------|-------------|
| GET | `/users` | List all users from test_users |
| POST | `/users` | Insert new user, return 201 |
| GET | `/health` | Health check |

## Behavioral Parity with T4

T5 produces byte-identical HTTP responses to T4 for the same requests:
- Same JSON keys and ordering
- Same response headers (`Content-Type`, `X-App-Name`)
- Same status codes (200, 201, 400, 404)
- Same seed data (5 users: Alice, Bob, Carol, Dave, Eve)

## Bun Polyfills

T5 validates that Bun-specific APIs work in both modes:
- `Bun.env` → WASI environment variables (falls back to `process.env`)
- `Bun.sleep()` → WASI clocks (falls back to `setTimeout`)

## Dependencies

| Story | Description | Status |
|-------|-------------|--------|
| US-701 | Docker Compose test stack | Not started |
| US-609 | Bun native + Wasm parity test | Blocked |
| US-614 | Register --lang bun in warp pack | Blocked |
| US-606 | @warpgrid/bun-sdk/postgres | Blocked |

## Running

```bash
# Build standalone handler
./build.sh --standalone

# Run integration tests
./test.sh

# Build-only verification
./test.sh --build-only
```
