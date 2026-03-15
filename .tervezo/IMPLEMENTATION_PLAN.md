# Implementation Plan: US-705 — TypeScript HTTP + Postgres Integration Test (T4)

## Task List

- [x] **Task 1: Install dependencies and verify test infrastructure** — `npm install` succeeds, `tsx` and `typescript` available
- [x] **Task 2: Fix TypeScript type errors** — `npm run typecheck` (tsc --noEmit) passes clean, no errors
- [x] **Task 3: Fix and pass all unit tests** — `npm test` passes: 62 tests, 0 failures across 21 suites (pg-wire, pg-client, handler, e2e)
- [x] **Task 4: Verify process.env.APP_NAME in response headers** — handler-standalone.js sets X-App-Name from `globalThis.process?.env?.APP_NAME` with fallback; test.sh test 6 validates
- [x] **Task 5: Verify warpgrid.database.connect() usage** — handler.js imports from `warpgrid:shim/database-proxy@0.1.0`; Rust test `test_t4_db_proxy_not_raw_tcp` confirms pool stats prove shim usage
- [x] **Task 6: Verify GET /users and POST /users routes** — Both handlers implement GET /users (200), POST /users (201 with RETURNING), GET /users/:id (200/404), all with proper validation
- [x] **Task 7: Run full test suite and fix any remaining issues** — Both `npm run typecheck` and `npm test` pass clean
- [ ] **Task 8: Create PR referencing issue #89** (in progress)
