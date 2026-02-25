# T4: TypeScript HTTP + Postgres Integration Test

Cross-domain integration test validating that a TypeScript HTTP handler
componentized via ComponentizeJS can serve HTTP requests and query
PostgreSQL through the WarpGrid database proxy shim.

## Architecture

```
                  jco componentize
handler.js  ─────────────────────►  handler.wasm
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
| `src/handler.js` | Full handler with warpgrid:shim DB access | Blocked on US-403/404 |
| `src/handler-standalone.js` | Standalone with in-memory data | Working |

## Routes

| Method | Path | Description |
|--------|------|-------------|
| GET | `/users` | List all users from test_users |
| POST | `/users` | Insert new user, return 201 |
| GET | `/health` | Health check |

## Dependencies

| Story | Description | Status |
|-------|-------------|--------|
| US-701 | Docker Compose test stack | Not started |
| US-403 | warpgrid.database.connect() | Not started |
| US-404 | warpgrid/pg package | Not started |
| US-410 | warp pack --lang js | Not started |

## Running

```bash
# Build standalone handler
./build.sh --standalone

# Run integration tests
./test.sh

# Build-only verification
./test.sh --build-only
```
