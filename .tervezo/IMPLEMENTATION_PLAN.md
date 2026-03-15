# Implementation Plan: US-611 — Implement `bun run --warpgrid` native dev mode

## Task List

- [x] **1.1** Create `packages/warpgrid-bun-sdk/tests/preload.test.ts` with TDD tests
- [x] **2.1** Create `packages/warpgrid-bun-sdk/src/preload.ts` — preload script
- [x] **3.1** Update `packages/warpgrid-bun-sdk/package.json` — add `"./preload"` export
- [x] **4.1** Integration test: handler with mock native pool after preload
- [x] **5.1** Add module-load test simulating `bun run --preload`
- [x] **5.2** Document bunfig.toml config in preload.ts comment
- [x] **6.1** Run `bun test` — 195 tests pass (9 new)
- [x] **6.2** Run typecheck — clean
- [x] **6.3** No regressions in existing tests
