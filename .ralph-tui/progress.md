Merge conflict resolved and staged. The resolution keeps both new progress entries:

1. **warpgrid-agm.57** (main): US-410 ComponentizeJS `warp pack --lang js` integration
2. **warpgrid-agm.51** (worker): US-404 `@warpgrid/bun-sdk/pg` with pg.Client interface

`★ Insight ─────────────────────────────────────`
This was a classic "both branches appended to the same file" conflict. The resolution strategy is straightforward — include both additions since they're independent entries (different user stories from parallel work streams). The progress log is append-only, so ordering between the two new entries doesn't affect correctness.
`─────────────────────────────────────────────────`

3. **warpgrid-agm.47** (worker-w0-1): US-311 End-to-end Go HTTP handler with Postgres integration test

   **Files created:**
   - `test-apps/t3-go-http-postgres/` — Go HTTP handler (standalone with in-memory data), 9 unit tests, build.sh, test.sh
   - `tests/fixtures/t3-go-http-guest/` — Rust guest component exercising database-proxy shim with INSERT lifecycle
   - `crates/warpgrid-host/tests/integration_t3_go_http_postgres.rs` — 6 Rust integration tests (connect, SELECT, INSERT, close, pool tracking)
   - `crates/warp-pack/src/go.rs` — Full pack_go implementation: TinyGo wasip2 pipeline with find_tinygo, find_sdk_root, SHA256

   **Files modified:**
   - `crates/warp-pack/src/lib.rs` — Added `mod go;` and routed `"go"` lang to `go::pack_go`

   **Patterns & learnings:**
   - Followed T4 dual-handler pattern: standalone handler works now, full handler awaits US-305/307/310
   - Rust 2024 edition requires `unsafe {}` blocks for `std::env::set_var`/`remove_var` in tests
   - Pre-existing clippy lint (manual_strip) in warp-core/src/source.rs — not introduced by this bead
   - Go modules must be tested from within their directory (`cd test-apps/t3-go-http-postgres && go test ./...`)

## 2026-02-24 - warpgrid-agm.40
- Implemented US-304: Patch net.Dial DNS resolution via WarpGrid shim
- Created `packages/warpgrid-go/` Go module with two packages:
  - `dns/` — DNS resolver with pluggable backend (ResolverBackend interface), IP literal detection, WASI shim integration
  - `net/` — DNS-aware Dialer with hostname resolution, ordered failover across multiple A records, *net.OpError wrapping
- Created `dns/shim_wasi.go` — WASI-specific backend using `//go:wasmimport warpgrid_shim dns_resolve` (build-constrained to wasip1/wasip2)
- Created `patches/tinygo/0001-net-Dial-resolve-hostnames-via-WarpGrid-DNS-shim.patch` — format-patch for TinyGo source integration (awaits US-301 cloning TinyGo)
- 29 tests total (15 dns + 14 net), all passing with -race

Files created:
- `packages/warpgrid-go/go.mod`
- `packages/warpgrid-go/dns/resolve.go` — Resolver, ResolverBackend interface, IsIPLiteral
- `packages/warpgrid-go/dns/shim_wasi.go` — WasiBackend with //go:wasmimport (wasip1/wasip2 only)
- `packages/warpgrid-go/dns/resolve_test.go` — 15 unit tests
- `packages/warpgrid-go/net/dial.go` — Dialer with DNS resolution and failover
- `packages/warpgrid-go/net/dial_test.go` — 14 unit tests (echo server, failover, error wrapping)
- `patches/tinygo/0001-net-Dial-resolve-hostnames-via-WarpGrid-DNS-shim.patch`

**Learnings:**
- ResolverBackend interface pattern decouples platform-specific DNS from business logic — same approach as libc weak symbols but at Go type system level
- `//go:wasmimport` ABI for DNS matches libc-patches/0001: 17 bytes per record (1 family + 16 address), enabling code sharing between C and Go paths
- Must bounds-check host-returned count in WASI shim to prevent buffer overflow from malicious/buggy host
- RFC 5737 TEST-NET addresses (192.0.2.x) are guaranteed-unreachable, good for failover testing
- `net.DNSError.IsNotFound` should be set for DNS resolution failures to enable correct retry behavior in callers
---