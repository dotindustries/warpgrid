# Implementation Plan: US-704 — Go HTTP + Postgres Integration Test (T3)

## Overview

Issue #88 requires a complete Go HTTP + Postgres integration test. All scaffolding is in place:
- `test-apps/t3-go-http-postgres/` — Go app with net/http + pgx, 9 unit tests
- `tests/fixtures/t3-go-http-guest/` — Rust guest Wasm component (10 exported test functions)
- `crates/warpgrid-host/tests/integration_t3_go_http_postgres.rs` — 10 Rust integration tests

## Task List

- [x] **Task 1: Run Go unit tests and verify all pass** — 9/9 pass
- [x] **Task 2: Build t3-go-http-guest Rust fixture and verify Wasm compilation** — compiles to 42KB component
- [x] **Task 3: Run Rust integration tests and fix any failures** — 10/10 pass
- [x] **Task 4: Run end-to-end test script and standalone build** — 8/8 e2e pass, standalone build clean
- [x] **Task 5: Validate acceptance criteria and create PR referencing issue #88**

## Acceptance Criteria Mapping

| Criterion | Verification |
|---|---|
| `test-apps/t3-go-http-postgres/` with net/http + pgx | Directory exists with `main.go` using `pgx/v5` |
| GET /users returns 200 with seed users | `TestGetUsersReturnsSeedData` + `test_t3_http_get_users_returns_200_json` |
| POST /users returns 201; GET reflects new user | `TestPostThenGetIncludesNewUser` + `test_t3_http_post_user_returns_201_json` |
| net.Dial routed through database proxy | `test_t3_db_proxy_tracks_pool_usage` (pool stats prove proxy routing) |
| Invalid db host returns 503 | `test_t3_http_db_unavailable_returns_503` |
| go test pass | 9/9 Go unit tests pass |
| Tests written before implementation (TDD) | Tests exist in both Go and Rust layers |
