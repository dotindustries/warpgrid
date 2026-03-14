# Implementation Plan: US-405 — End-to-end TypeScript HTTP handler with Postgres

**Issue:** #54 | **Beads ID:** warpgrid-agm.52 | **Milestone:** M4.2

## Task List

- [x] Verify existing unit tests pass (`npm run typecheck` and `npm test` — 44/44 pass)
- [x] Write E2E integration test (`test/e2e.test.ts`) — 17 tests covering full handler→client→wire pipeline with MockTransport
- [ ] Audit handler.js routing against handler-logic.ts — ensure both handle same routes/status codes/error formats
- [ ] Add missing error handling to handler.js if needed (400 for missing body, invalid JSON, empty name; 500 for DB errors)
- [ ] Verify TypeScript type checking passes after changes
- [ ] Extend Rust E2E integration test (`integration_t4_ts_http_postgres.rs`) — HTTP-level tests using componentized TypeScript handler
- [ ] Final validation — all acceptance criteria met
- [ ] Create PR referencing issue #54

## Acceptance Criteria
1. Sample handler imports warpgrid/pg, queries Postgres, returns JSON
2. Compiles and componentizes via WarpGrid build toolchain
3. Test harness provisions Postgres, deploys Wasm, validates JSON response
4. Correct Content-Type header and JSON body
5. Error cases: missing param returns 400, db error returns 500

## Notes
- Existing tests: handler.test.ts (14 tests), pg-client.test.ts (12 tests), pg-wire.test.ts (18 tests) — all pass
- handler.js is the componentizable entry point (inline wire protocol for jco single-file requirement)
- handler-logic.ts is the testable mirror used by unit tests
