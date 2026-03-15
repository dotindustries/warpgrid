# Implementation Plan: US-603 — `warp pack --lang bun` Compilation Pipeline

## Task List

- [x] Implement `bun_build()` function — bundles handler with `bun build --target browser --format esm`
- [x] Implement `jco_componentize()` function — creates WASI HTTP Wasm component via jco
- [x] Implement `validate_component()` function — validates `wasi:http/incoming-handler` export via wasm-tools
- [x] Implement `pack_bun()` orchestrator — connects all pipeline stages, creates `target/wasm/<name>.wasm`
- [x] Implement `resolve_jco()` — find jco binary (env var → project-local → PATH)
- [x] Implement `resolve_wit_dir()` — find WIT directory (project-local → shared fixture)
- [x] Implement `resolve_polyfills_dir()` — find @warpgrid/bun-polyfills package
- [x] Implement `generate_polyfill_wrapper()` — create wrapper entry with polyfill injection
- [x] Add `"bun"` to `SUPPORTED_LANGUAGES` in `lib.rs`
- [x] Add `bun::pack_bun` dispatch in `pack_with_lang()` match arm
- [x] Implement `detect_language()` — auto-detect bun from `bunfig.toml` (highest priority)
- [x] Add CLI `--lang bun` support in warp-cli
- [x] Write unit tests for entry point validation, missing build section, jco env var
- [x] Write unit tests for WIT dir resolution, polyfill resolution, wrapper generation
- [x] Write integration tests for bun build, full pipeline, and pack dispatcher
- [x] Write error message tests (bun build stderr, jco hint, unsupported language listing)
- [x] Verify `bun run typecheck` passes for warpgrid-bun-polyfills package
- [x] Verify `bun test` passes for warpgrid-bun-polyfills package (20 pass, 0 fail)
- [x] Verify `cargo test -p warp-pack` passes all tests (52 passed, 0 failed, 2 ignored)
- [x] Add `warp.toml` to `tests/fixtures/bun-json-api/` for warp pack integration testing
- [x] Add `warp.toml` to `tests/fixtures/bun-postgres-handler/` for warp pack integration testing
- [x] Create PR referencing GitHub issue #72 — PR #134
