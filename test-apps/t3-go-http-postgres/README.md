# T3: Go HTTP + Postgres Integration Test

Cross-domain integration test validating that a Go HTTP handler
compiled via TinyGo wasip2 can serve HTTP requests and query
PostgreSQL through the WarpGrid database proxy shim.

## Architecture

```
                  warp pack --lang go (TinyGo wasip2)
main.go  ──────────────────────────────►  handler.wasm
                                             │
                                             ▼
                                    WarpGridEngine (Rust)
                                    ├─ wasi:http host
                                    ├─ warpgrid:shim/database-proxy
                                    └─ warpgrid:shim/dns
                                             │
                                             ▼
                                    Mock Postgres Server
```

## Handlers

| File | Description | Status |
|------|-------------|--------|
| `main.go` | Standalone handler with in-memory data | Working |
| `main.go` (full) | Handler with pgx + database proxy | Blocked on US-305/307 |

## Routes

| Method | Path | Description |
|--------|------|-------------|
| GET | `/users` | List all users from test_users |
| POST | `/users` | Insert new user, return 201 |
| GET | `/health` | Health check |

## Error Handling

| Scenario | Status Code | Response |
|----------|-------------|----------|
| Malformed JSON body | 400 | `{"error":"Invalid JSON"}` |
| Missing name/email | 400 | `{"error":"name and email are required"}` |
| Database unreachable | 503 | `{"error":"database unavailable"}` |
| Unknown route | 404 | `{"error":"Not Found"}` |

## Dependencies

| Story | Description | Status |
|-------|-------------|--------|
| US-305 | pgx Postgres driver over patched net.Dial | Not started |
| US-307 | warpgrid/net/http overlay — request/response round-trip | Not started |
| US-310 | warp pack --lang go integration | Not started |
| US-701 | Docker Compose test stack | Not started |

## Running

```bash
# Run Go unit tests (standalone)
./build.sh --standalone

# Run full integration test suite (standalone server + curl tests)
./test.sh

# Build-only verification
./test.sh --build-only

# Compile to Wasm (requires TinyGo)
./build.sh --wasm
```
