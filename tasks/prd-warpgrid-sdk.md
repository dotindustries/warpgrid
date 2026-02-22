[PRD]
# PRD: WarpGrid SDK — Fork Engineering Implementation

## Overview

The WarpGrid SDK is a curated, tested distribution of WebAssembly toolchain components that enables WarpGrid (a Wasm-native cluster orchestrator) to run real-world backend services. The SDK pushes WASI workload compatibility from ~25-35% to 60-70% through six coordinated engineering domains:

1. **Wasmtime host functions** — shim layer foundation (no fork)
2. **wasi-libc patches** — socket/filesystem compat (fork + patch)
3. **TinyGo WASI overlay** — Go workload support (fork + overlay)
4. **ComponentizeJS extensions** — TypeScript/Node.js workload support (fork + extend)
5. **WASI 0.3 async pre-integration** — async I/O for all languages (pre-integrate prototyping branch)
6. **Bun WASI runtime** — Bun/TypeScript workload support (overlay + shim bridge)

This PRD breaks down the full SDK plan into ~85 individually actionable user stories with explicit dependency chains, designed for parallel AI agent execution via ralph-tui.

## Goals

- Enable Rust, Go, TypeScript (Node.js), and Bun (TypeScript) backend services to compile and run as WASI modules on WarpGrid
- Provide transparent database connectivity (Postgres, MySQL, Redis) through host-side connection pooling
- Provide transparent DNS resolution for WarpGrid service-to-service communication
- Provide virtual filesystem shims for system paths (`/etc/resolv.conf`, `/dev/urandom`, timezone data, etc.)
- Pre-integrate WASI 0.3 async I/O 6-12 months ahead of stable upstream
- Maintain all forks as minimal, rebasing-friendly patch series
- Validate everything end-to-end with cross-language integration tests

## Quality Gates

These commands must pass for every user story:

**Rust stories (Domains 1, 5, and Rust parts of 2):**
- `cargo check` — compilation
- `cargo test` — unit and integration tests
- `cargo clippy -- -D warnings` — linting

**C/libc stories (Domain 2):**
- `make -j$(nproc) THREAD_MODEL=single` — libc build
- `scripts/test-libc.sh` — custom test harness

**Go stories (Domain 3):**
- `tinygo build -target=wasip2` — compilation
- `go test ./...` — unit tests

**TypeScript/Node.js stories (Domain 4):**
- `npm run typecheck` — type checking
- `npm test` — unit tests

**Bun stories (Domain 6):**
- `bun run typecheck` — type checking
- `bun test` — unit tests

**All stories:**
- Tests written BEFORE implementation (TDD: red-green-refactor)
- All tests pass before story is marked complete

## User Stories

---

## Domain 1: Wasmtime Host Functions (`warpgrid-host`)

### US-101: Scaffold the `warpgrid-host` crate with dependencies
**Milestone:** M1.1
**Depends on:** none
**Description:** As a SDK developer, I want a properly structured `warpgrid-host` crate with all required dependencies so that I have a compilable foundation for implementing host function shims.

**Acceptance Criteria:**
- [ ] `crates/warpgrid-host/` directory exists with `Cargo.toml` and `src/lib.rs`
- [ ] `Cargo.toml` declares dependencies on `wasmtime`, `wasmtime-wasi`, `tokio` (with `rt-multi-thread`, `macros`, `net`, `time` features), `tracing`, and `anyhow`
- [ ] Dev-dependencies include `tokio` (with `test-util`), `tracing-subscriber`, and `wasmtime` (with `component-model` feature)
- [ ] `src/lib.rs` contains a public crate-level module structure (empty modules for `filesystem`, `dns`, `signals`, `db_proxy`, `threading`, `config`, `engine`)
- [ ] `cargo check -p warpgrid-host` passes with zero errors
- [ ] Tests written before implementation (TDD)

### US-102: Define WIT interfaces and generate Rust bindings
**Milestone:** M1.1
**Depends on:** US-101
**Description:** As a SDK developer, I want WIT interface definitions for all shim domains with generated Rust bindings so that guest modules have typed contracts for each host function.

**Acceptance Criteria:**
- [ ] `crates/warpgrid-host/wit/` directory contains WIT files: `filesystem.wit`, `dns.wit`, `signals.wit`, `database-proxy.wit`, `threading.wit`
- [ ] `filesystem.wit` declares functions for opening, reading, and stat-ing virtual paths
- [ ] `dns.wit` declares a `resolve-address` function accepting a hostname and returning a list of IP address records
- [ ] `signals.wit` declares `on-signal` (register interest) and `poll-signal` (dequeue) functions with signal-type enum (`terminate`, `hangup`, `interrupt`)
- [ ] `database-proxy.wit` declares `connect`, `send`, `recv`, and `close` functions operating on an opaque `connection-handle` (u64)
- [ ] `threading.wit` declares `declare-threading-model` accepting an enum (`parallel-required`, `cooperative`)
- [ ] Rust bindings are generated via `wasmtime::component::bindgen!` macro in a dedicated `bindings` module
- [ ] `cargo check -p warpgrid-host` passes after binding generation
- [ ] Tests written before implementation (TDD)

### US-103: Implement the virtual file map
**Milestone:** M1.2
**Depends on:** US-102
**Description:** As a Wasm workload, I want reads to well-known system paths (`/dev/null`, `/dev/urandom`, `/etc/resolv.conf`, `/etc/hosts`, `/proc/self/`, `/usr/share/zoneinfo/**`) to return WarpGrid-controlled content so that my application behaves correctly in a sandboxed environment without access to the real host filesystem.

**Acceptance Criteria:**
- [ ] A `VirtualFileMap` struct holds an immutable map of virtual path prefixes to content-provider functions
- [ ] `/dev/null` returns empty content on read, accepts and discards all writes
- [ ] `/dev/urandom` returns cryptographically random bytes (via `getrandom` or ring) on each read
- [ ] `/etc/resolv.conf` returns a configurable nameserver configuration (default: `nameserver 127.0.0.1`)
- [ ] `/etc/hosts` returns a configurable hosts mapping (populated from service registry)
- [ ] `/proc/self/` paths return synthetic process metadata (e.g., `/proc/self/status` with reasonable defaults)
- [ ] `/usr/share/zoneinfo/**` returns embedded timezone data for common timezones (at minimum UTC, US/Eastern, US/Pacific, Europe/London)
- [ ] `VirtualFileMap` is constructed immutably from a builder, not mutated after creation
- [ ] Tests written before implementation (TDD)

### US-104: Implement filesystem intercept logic as host functions
**Milestone:** M1.2
**Depends on:** US-103
**Description:** As a Wasm workload, I want filesystem operations to be intercepted so that virtual paths return WarpGrid content while all other paths fall through to the real WASI filesystem implementation.

**Acceptance Criteria:**
- [ ] Filesystem host functions are registered with the Wasmtime `Linker` matching the `filesystem.wit` interface
- [ ] On any file-open or file-read call, the path is checked against the `VirtualFileMap` before delegating to `wasmtime-wasi`
- [ ] If the path matches a virtual entry, the virtual content provider is invoked and results returned directly
- [ ] If the path does not match any virtual entry, the call is forwarded to the underlying WASI filesystem implementation unchanged
- [ ] Path matching handles both exact paths and prefix matches (e.g., `/usr/share/zoneinfo/` prefix)
- [ ] Path canonicalization prevents bypass via `..` or symlink traversal (e.g., `/etc/../etc/hosts` still matches)
- [ ] All intercept decisions are logged at `tracing::debug` level
- [ ] Tests written before implementation (TDD)

### US-105: Filesystem shim integration tests
**Milestone:** M1.2
**Depends on:** US-104
**Description:** As a SDK developer, I want integration tests proving that a compiled Wasm module reading virtual paths gets WarpGrid-generated content so that I can be confident the filesystem shim works end-to-end.

**Acceptance Criteria:**
- [ ] A test Rust program is compiled to a Wasm component that reads `/etc/resolv.conf` and asserts it contains `nameserver`
- [ ] A test Rust program is compiled to a Wasm component that reads 32 bytes from `/dev/urandom` and asserts the result is non-zero and 32 bytes long
- [ ] A test Wasm component reads `/dev/null` and verifies it returns empty content
- [ ] A test Wasm component reads a non-virtual path (e.g., a pre-seeded temp file) and verifies it receives the real file content (fall-through behavior)
- [ ] A test Wasm component reads `/etc/hosts` and verifies it contains entries injected from a test service registry
- [ ] All integration tests run against a real Wasmtime engine instance (not mocked)
- [ ] Tests written before implementation (TDD)

### US-106: Implement core DNS resolution chain
**Milestone:** M1.3
**Depends on:** US-102
**Description:** As a Wasm workload, I want hostname resolution to check the WarpGrid service registry first, then `/etc/hosts`, then host system DNS so that intra-cluster services resolve correctly while external domains still work.

**Acceptance Criteria:**
- [ ] A `DnsResolver` struct is constructed with an injected `HashMap<String, Vec<IpAddr>>` representing the service registry
- [ ] The resolution chain is: (1) service registry lookup, (2) virtual `/etc/hosts` lookup, (3) fallback to host system DNS via `tokio::net::lookup_host`
- [ ] Resolution stops at the first chain link that returns results (no unnecessary downstream lookups)
- [ ] The `resolve-address` host function is registered with the Wasmtime `Linker` matching the `dns.wit` interface
- [ ] If no chain link resolves the hostname, a clear `HostNotFound` error is returned to the guest
- [ ] All resolution steps are logged at `tracing::debug` level, including which chain link produced the result
- [ ] Tests written before implementation (TDD)

### US-107: DNS caching with TTL and round-robin selection
**Milestone:** M1.3
**Depends on:** US-106
**Description:** As a Wasm workload, I want DNS results to be cached with a configurable TTL and addresses to be returned in round-robin order so that resolution is fast and load is distributed across service replicas.

**Acceptance Criteria:**
- [ ] A TTL-based cache stores resolved addresses keyed by hostname, with configurable TTL duration (default: 30 seconds)
- [ ] Cache entries are evicted after their TTL expires; subsequent lookups re-resolve through the chain
- [ ] When a hostname resolves to multiple addresses, consecutive calls return addresses in round-robin order
- [ ] Round-robin state is per-hostname and uses an atomic counter (no mutex contention on the hot path)
- [ ] The cache is bounded in size (configurable, default: 1024 entries) and uses LRU eviction when full
- [ ] Cache statistics (hits, misses, evictions) are exposed as `tracing::info` level metrics
- [ ] Tests written before implementation (TDD)

### US-108: DNS shim integration tests
**Milestone:** M1.3
**Depends on:** US-107
**Description:** As a SDK developer, I want integration tests proving the DNS resolution chain works end-to-end inside a Wasm component so that I can be confident the shim resolves both internal services and external hostnames correctly.

**Acceptance Criteria:**
- [ ] A test Wasm component resolves a service name present in the injected registry and receives the expected IP address
- [ ] A test Wasm component resolves a hostname present in the virtual `/etc/hosts` (but not the registry) and receives the expected IP address
- [ ] A test Wasm component resolves a well-known external hostname (e.g., `localhost`) and receives a valid address from host system DNS
- [ ] A test verifies round-robin behavior: resolving a multi-address service name N times produces addresses cycling through the list
- [ ] A test verifies TTL caching: after resolution, mutating the registry and resolving again within TTL returns the cached value; after TTL expiry, the new value is returned
- [ ] All integration tests run against a real Wasmtime engine instance
- [ ] Tests written before implementation (TDD)

### US-109: Implement signal queue and host functions
**Milestone:** M1.4
**Depends on:** US-102
**Description:** As a Wasm workload, I want to register interest in lifecycle signals and poll a bounded signal queue so that I can perform graceful shutdown or reconfiguration when the orchestrator sends events.

**Acceptance Criteria:**
- [ ] A `SignalQueue` struct provides a bounded queue (capacity: 16 entries) of signal types (`terminate`, `hangup`, `interrupt`)
- [ ] `on-signal` host function registers interest in one or more signal types for the calling module instance
- [ ] `poll-signal` host function dequeues the oldest signal from the queue, or returns `None` if the queue is empty
- [ ] If the queue is full when a new signal arrives, the oldest undelivered signal is dropped and a `tracing::warn` is emitted
- [ ] Host-side API `deliver_signal(instance_id, signal_type)` enqueues a signal for a specific module instance
- [ ] Only signals matching registered interest are enqueued; others are silently ignored
- [ ] The `signals` host functions are registered with the Wasmtime `Linker` matching the `signals.wit` interface
- [ ] Tests written before implementation (TDD)

### US-110: Signal handling integration tests
**Milestone:** M1.4
**Depends on:** US-109
**Description:** As a SDK developer, I want integration tests proving that Wasm modules can register for and receive signals so that I can verify the signal delivery pipeline works end-to-end.

**Acceptance Criteria:**
- [ ] A test Wasm component calls `on-signal` for `terminate`, the host delivers a `terminate` signal, and `poll-signal` returns it
- [ ] A test Wasm component calls `poll-signal` on an empty queue and receives `None`
- [ ] A test verifies queue bounding: 20 signals are delivered, and only the 16 most recent are retrievable via `poll-signal`
- [ ] A test verifies signal filtering: a module registers for `hangup` only, a `terminate` signal is delivered, and `poll-signal` returns `None`
- [ ] All integration tests run against a real Wasmtime engine instance
- [ ] Tests written before implementation (TDD)

### US-111: Implement the database connection pool manager
**Milestone:** M1.5
**Depends on:** US-102
**Description:** As a Wasm workload, I want a host-side connection pool that manages database connections per `(host, port, database, user)` tuple so that connection reuse is efficient and I do not need to manage raw sockets from inside the sandbox.

**Acceptance Criteria:**
- [ ] A `ConnectionPoolManager` struct maintains a pool of connections keyed by `(host, port, database, user)` tuple
- [ ] Pool size is configurable per tuple (default: 10 connections)
- [ ] Idle connections are reaped after a configurable timeout (default: 300 seconds)
- [ ] Health checking pings idle connections on a configurable interval (default: 30 seconds) and removes unhealthy ones
- [ ] TLS connections are established via `rustls` with configurable certificate verification (system roots by default)
- [ ] `connect` returns an opaque `u64` connection handle to the guest module
- [ ] `close` returns the connection to the pool for reuse (not destroyed unless unhealthy)
- [ ] If the pool for a tuple is exhausted, `connect` blocks (with configurable timeout, default: 5 seconds) or returns an error
- [ ] Pool statistics (active, idle, total, wait count) are emitted as `tracing::info` metrics
- [ ] Tests written before implementation (TDD)

### US-112: Implement Postgres wire protocol passthrough
**Milestone:** M1.5
**Depends on:** US-111
**Description:** As a Wasm workload, I want to send and receive raw Postgres wire protocol bytes through the database proxy so that existing Postgres client libraries (e.g., sqlx compiled to Wasm) work without modification.

**Acceptance Criteria:**
- [ ] `send` host function accepts a connection handle and a byte buffer, forwards the bytes to the corresponding Postgres TCP connection
- [ ] `recv` host function reads bytes from the Postgres connection and returns them to the guest (with configurable read timeout, default: 30 seconds)
- [ ] The host performs no parsing or transformation of the Postgres wire protocol — pure byte passthrough
- [ ] The host manages TLS transparently: the guest sends/receives plaintext, the host encrypts/decrypts via rustls
- [ ] Invalid connection handles return a clear error to the guest
- [ ] `database-proxy` host functions (`connect`, `send`, `recv`, `close`) are registered with the Wasmtime `Linker` matching the `database-proxy.wit` interface
- [ ] Tests written before implementation (TDD)

### US-113: Database proxy unit tests with mock Postgres server
**Milestone:** M1.5
**Depends on:** US-112
**Description:** As a SDK developer, I want unit tests using a mock Postgres server so that I can verify connection pooling, health checking, idle timeout, and byte passthrough without requiring a real database.

**Acceptance Criteria:**
- [ ] A `MockPostgresServer` test helper listens on a local TCP port and responds to Postgres startup handshake
- [ ] Test: `connect` returns a valid handle, `close` returns the connection to the pool, a subsequent `connect` to the same tuple reuses the pooled connection
- [ ] Test: when pool size is exhausted, `connect` either blocks or returns a timeout error
- [ ] Test: idle connections are reaped after the configured timeout
- [ ] Test: health check removes connections that fail the ping
- [ ] Test: `send` and `recv` pass bytes through to and from the mock server without modification
- [ ] Test: `send` or `recv` on an invalid handle returns an error
- [ ] Tests written before implementation (TDD)

### US-114: Database proxy Postgres integration test
**Milestone:** M1.5
**Depends on:** US-113
**Description:** As a SDK developer, I want an integration test that compiles a Rust sqlx application to Wasm and runs queries through the database proxy so that I can prove real Postgres workloads work end-to-end.

**Acceptance Criteria:**
- [ ] A test Rust program using `sqlx` with Postgres is compiled to a Wasm component
- [ ] The Wasm component connects to a Postgres instance (test container or CI service) via the database proxy
- [ ] The component executes `CREATE TABLE`, `INSERT`, `SELECT`, and `DROP TABLE` statements and verifies results
- [ ] The test verifies that connection reuse occurs (multiple queries on the same handle)
- [ ] The test verifies that `close` followed by a new `connect` reuses the pooled connection
- [ ] The integration test is gated behind a `#[cfg(feature = "integration")]` or `#[ignore]` attribute for CI flexibility
- [ ] Tests written before implementation (TDD)

### US-115: Implement MySQL wire protocol passthrough
**Milestone:** M1.6
**Depends on:** US-112
**Description:** As a Wasm workload, I want to send and receive raw MySQL wire protocol bytes through the database proxy so that MySQL client libraries compiled to Wasm work without modification.

**Acceptance Criteria:**
- [ ] The `ConnectionPoolManager` supports a `protocol` discriminator differentiating Postgres, MySQL, and Redis connections
- [ ] MySQL connections are established using the MySQL wire protocol handshake (capability flags, auth exchange) handled transparently by the host
- [ ] `send` and `recv` pass MySQL protocol bytes through without parsing or transformation
- [ ] TLS for MySQL connections is handled by the host via `rustls` (guest sends/receives plaintext)
- [ ] Health check for MySQL connections uses `COM_PING`
- [ ] Connection draining on shutdown: stop accepting new `connect` calls, allow in-flight queries to complete (configurable drain timeout, default: 30 seconds), then close all connections
- [ ] Tests written before implementation (TDD)

### US-116: Implement Redis RESP protocol passthrough
**Milestone:** M1.6
**Depends on:** US-112
**Description:** As a Wasm workload, I want to send and receive raw Redis RESP protocol bytes through the database proxy so that Redis client libraries compiled to Wasm work without modification.

**Acceptance Criteria:**
- [ ] Redis connections are established as plain TCP (or TLS) connections to the Redis server
- [ ] `send` and `recv` pass RESP protocol bytes through without parsing or transformation
- [ ] TLS for Redis connections is handled by the host via `rustls` when configured
- [ ] Health check for Redis connections uses `PING` command and expects `PONG` response
- [ ] Connection draining on shutdown follows the same pattern as MySQL (stop new connects, drain in-flight, close)
- [ ] Redis AUTH is forwarded transparently as part of the byte stream (host does not intercept credentials)
- [ ] Tests written before implementation (TDD)

### US-117: MySQL and Redis integration tests
**Milestone:** M1.6
**Depends on:** US-115, US-116
**Description:** As a SDK developer, I want integration tests proving that Wasm modules can communicate with real MySQL and Redis instances through the database proxy so that I can verify multi-protocol support works end-to-end.

**Acceptance Criteria:**
- [ ] A test Wasm component connects to a MySQL instance via the database proxy and executes `CREATE TABLE`, `INSERT`, `SELECT`, and `DROP TABLE`
- [ ] A test Wasm component connects to a Redis instance via the database proxy and executes `SET`, `GET`, and `DEL` commands
- [ ] A test verifies connection pooling works for MySQL (connect, close, reconnect reuses pooled connection)
- [ ] A test verifies connection pooling works for Redis (connect, close, reconnect reuses pooled connection)
- [ ] A test verifies that health checks detect and remove dead MySQL and Redis connections
- [ ] Integration tests are gated behind `#[cfg(feature = "integration")]` or `#[ignore]` for CI flexibility
- [ ] Tests written before implementation (TDD)

### US-118: Implement threading model declaration host function
**Milestone:** M1.7
**Depends on:** US-102
**Description:** As a Wasm workload, I want to declare my threading model expectation to the host so that the host can configure the appropriate execution mode and warn if the requested model is unsupported.

**Acceptance Criteria:**
- [ ] `declare-threading-model` host function accepts a threading model enum (`parallel-required`, `cooperative`)
- [ ] If `parallel-required` is declared, the host emits a `tracing::warn` log explaining that parallel threading is not yet supported and the module will run in cooperative mode
- [ ] If `cooperative` is declared, the host enables sequential/cooperative execution mode (no action needed beyond acknowledgment)
- [ ] The declared threading model is stored per module instance and queryable by the host engine
- [ ] Calling `declare-threading-model` more than once per instance returns an error (model is immutable once declared)
- [ ] The `threading` host function is registered with the Wasmtime `Linker` matching the `threading.wit` interface
- [ ] Tests written before implementation (TDD)

### US-119: Threading model unit and integration tests
**Milestone:** M1.7
**Depends on:** US-118
**Description:** As a SDK developer, I want tests proving that threading model declarations are handled correctly so that I can be confident modules receive appropriate execution behavior.

**Acceptance Criteria:**
- [ ] Unit test: declaring `cooperative` succeeds and stores the model on the instance
- [ ] Unit test: declaring `parallel-required` succeeds, stores the model, and emits a warning log (verified via tracing test subscriber)
- [ ] Unit test: calling `declare-threading-model` twice returns an error
- [ ] Integration test: a Wasm component declares `cooperative` and continues executing normally
- [ ] Integration test: a Wasm component declares `parallel-required` and continues executing (degraded to cooperative) with a host warning logged
- [ ] Tests written before implementation (TDD)

### US-120: Implement ShimConfig parsing from deployment spec
**Milestone:** M1.8
**Depends on:** US-101
**Description:** As an operator, I want shim configuration to be parsed from the `[shims]` section of `warp.toml` so that I can enable or disable individual shims per deployment without recompiling.

**Acceptance Criteria:**
- [ ] A `ShimConfig` struct is defined with boolean/optional fields for each shim domain: `filesystem`, `dns`, `signals`, `database_proxy`, `threading`
- [ ] Each shim field supports domain-specific sub-configuration (e.g., `dns.ttl_seconds`, `database_proxy.pool_size`, `filesystem.extra_virtual_paths`)
- [ ] `ShimConfig` implements `Default` with all shims enabled at their default settings
- [ ] A `ShimConfig::from_toml(value: &toml::Value) -> Result<ShimConfig>` constructor parses the `[shims]` table
- [ ] Unknown shim names in the TOML produce a `tracing::warn` (not an error) for forward compatibility
- [ ] Missing `[shims]` section results in the default config (all shims enabled)
- [ ] Tests written before implementation (TDD)

### US-121: Implement WarpGridEngine wrapper API
**Milestone:** M1.8
**Depends on:** US-120, US-104, US-106, US-109, US-112, US-118
**Description:** As a SDK developer, I want a `WarpGridEngine` struct that takes a `ShimConfig` and selectively registers only the enabled host functions with the Wasmtime linker so that instantiation is clean and unused shims incur zero overhead.

**Acceptance Criteria:**
- [ ] A `WarpGridEngine` struct wraps a `wasmtime::Engine` and a `ShimConfig`
- [ ] `WarpGridEngine::new(config: ShimConfig) -> Result<Self>` constructs the engine and validates the config
- [ ] `WarpGridEngine::instantiate(module_bytes: &[u8]) -> Result<Instance>` creates a Wasmtime store, linker, and instance with only the shims enabled in the config
- [ ] If `filesystem` is disabled, no filesystem host functions are registered and virtual file interception is skipped
- [ ] If `dns` is disabled, no DNS host functions are registered
- [ ] If `signals` is disabled, no signal host functions are registered
- [ ] If `database_proxy` is disabled, no database proxy host functions are registered
- [ ] If `threading` is disabled, no threading host functions are registered
- [ ] Startup logging at `tracing::info` level lists which shims are enabled and their key configuration values
- [ ] Tests written before implementation (TDD)

### US-122: ShimConfig and WarpGridEngine integration tests
**Milestone:** M1.8
**Depends on:** US-121
**Description:** As a SDK developer, I want integration tests proving that the engine correctly enables and disables shims based on configuration so that I can verify the composition model works end-to-end.

**Acceptance Criteria:**
- [ ] Test: a config with all shims enabled instantiates a Wasm module that calls functions from every shim domain
- [ ] Test: a config with only `filesystem` enabled instantiates a module that reads `/etc/hosts` successfully but receives a `LinkError` when calling DNS functions
- [ ] Test: a config with only `dns` enabled instantiates a module that resolves hostnames but receives a `LinkError` when calling filesystem shim functions
- [ ] Test: a config with no shims enabled instantiates a module that can only use standard WASI functions
- [ ] Test: a config parsed from a TOML string with `[shims] filesystem = true, dns = false` produces the expected `ShimConfig`
- [ ] Test: startup logs correctly list enabled and disabled shims (verified via tracing test subscriber)
- [ ] Tests written before implementation (TDD)

---

## Domain 2: wasi-libc Patches (`warpgrid-libc`)

### US-201: Fork wasi-libc and establish patched build pipeline
**Milestone:** M2.1
**Depends on:** none
**Description:** As a SDK build engineer, I want a reproducible fork of wasi-libc pinned to a known upstream tag with a CI pipeline that builds both stock and patched variants, so that we have a stable baseline for applying WarpGrid patches and can detect regressions against upstream.

**Acceptance Criteria:**
- [ ] Test: a build script exists that clones wasi-libc at the commit hash recorded in `libc-patches/UPSTREAM_REF` and succeeds with `make -j$(nproc) THREAD_MODEL=single` producing a valid sysroot
- [ ] Test: CI job builds the stock (unpatched) wasi-libc and the warpgrid-patched wasi-libc; both produce a sysroot containing `libc.a`
- [ ] Test: a minimal C program (`int main() { return 0; }`) compiles and links against both sysroots without errors
- [ ] `libc-patches/UPSTREAM_REF` contains the pinned upstream commit hash and tag name
- [ ] A `warpgrid` branch exists in the fork with an empty initial patch series directory (`libc-patches/`)
- [ ] `scripts/build-libc.sh` accepts `--stock` and `--patched` flags and produces the corresponding sysroot under `build/sysroot-stock/` and `build/sysroot-patched/`

### US-202: Create patch maintenance and rebase tooling
**Milestone:** M2.5 (early — needed by all subsequent patch stories)
**Depends on:** US-201
**Description:** As a SDK maintainer, I want scripts that apply, export, and rebase patch files against upstream wasi-libc, so that the patch series remains a reviewable set of `git format-patch` files and upstream updates can be incorporated with conflict reporting.

**Acceptance Criteria:**
- [ ] Test: `scripts/rebase-libc.sh --apply` applies all patches from `libc-patches/*.patch` onto the pinned upstream ref and succeeds when patches are clean
- [ ] Test: `scripts/rebase-libc.sh --apply` reports specific conflicting file names and exits non-zero when a patch conflicts with a simulated upstream change
- [ ] Test: `scripts/rebase-libc.sh --export` regenerates the `libc-patches/*.patch` files from the current warpgrid branch, and the exported patches are byte-identical when no changes were made
- [ ] Test: `scripts/rebase-libc.sh --update` fetches a new upstream ref, attempts rebase, and produces a summary listing applied/conflicting patches
- [ ] `scripts/test-libc.sh` compiles test programs from `libc-patches/tests/`, runs them in Wasmtime with mock shims, and reports pass/fail with exit code
- [ ] Each script includes `--help` output documenting all flags and expected workflow

### US-203: Patch DNS getaddrinfo to route through WarpGrid shim
**Milestone:** M2.2
**Depends on:** US-201, US-202, US-106
**Description:** As a developer compiling a network-aware C program to WASI, I want `getaddrinfo()` to route hostname resolution through `warpgrid:shim/dns.resolve(hostname)` before falling back to default behavior, so that WarpGrid-managed service names (e.g., `db.production.warp.local`) resolve to the correct addresses within the mesh.

**Acceptance Criteria:**
- [ ] Test: a C program calling `getaddrinfo("db.production.warp.local", "5432", ...)` compiled against the patched sysroot resolves to the address returned by the `dns.resolve` shim when the shim is present
- [ ] Test: when the shim returns an empty result (host not managed by WarpGrid), `getaddrinfo` falls through to the original WASI implementation
- [ ] Test: `getaddrinfo` with `AI_NUMERICHOST` flag (raw IP like `"127.0.0.1"`) bypasses the shim entirely and resolves directly
- [ ] The patch uses a weak symbol import (`__attribute__((weak))`) for the shim function so the patched libc links even when the shim is not provided by the host
- [ ] Patch is exported as a `git format-patch` file in `libc-patches/0001-dns-getaddrinfo-shim.patch`

### US-204: Patch gethostbyname and getnameinfo for WarpGrid DNS
**Milestone:** M2.2
**Depends on:** US-203
**Description:** As a developer using legacy DNS APIs in C code compiled to WASI, I want `gethostbyname()` and `getnameinfo()` to also route through the WarpGrid DNS shim, so that all hostname resolution paths are consistent regardless of which POSIX API the application uses.

**Acceptance Criteria:**
- [ ] Test: a C program calling `gethostbyname("cache.staging.warp.local")` returns the address provided by the `dns.resolve` shim
- [ ] Test: a C program calling `getnameinfo()` with an address that maps to a WarpGrid-managed host returns the correct hostname via `dns.reverse-resolve` shim
- [ ] Test: `gethostbyname(NULL)` and `getnameinfo` with non-WarpGrid addresses fall through to default behavior without error
- [ ] Both patches share the same weak-symbol shim import mechanism established in US-203
- [ ] Patches are exported as `libc-patches/0002-dns-gethostbyname-shim.patch` and `libc-patches/0003-dns-getnameinfo-shim.patch`

### US-205: Verify DNS patches with stock build compatibility
**Milestone:** M2.2
**Depends on:** US-203, US-204
**Description:** As a SDK maintainer, I want to verify that all DNS patches maintain full backward compatibility with stock wasi-libc, so that programs compiled against the patched sysroot still function correctly when the WarpGrid host shims are absent.

**Acceptance Criteria:**
- [ ] Test: a C program using `getaddrinfo`, `gethostbyname`, and `getnameinfo` compiles against the patched sysroot and runs in a vanilla Wasmtime (no WarpGrid shims) without crashing or hanging
- [ ] Test: the stock (unpatched) wasi-libc still builds cleanly after the DNS patches exist in the repository (patches do not corrupt the build script or Makefile)
- [ ] Test: binary size of `libc.a` with DNS patches is within 5% of stock `libc.a` size
- [ ] Weak symbol fallback paths are exercised and produce the same results as unpatched wasi-libc for standard hostnames

### US-206: Patch fopen/open to intercept virtual filesystem paths
**Milestone:** M2.3
**Depends on:** US-201, US-202, US-104
**Description:** As a developer compiling a C program to WASI, I want `fopen()` and `open()` to intercept reads to WarpGrid virtual paths (e.g., `/etc/resolv.conf`, `/etc/hosts`) and return content provided by `warpgrid:shim/filesystem.read-virtual(path)`, so that configuration files can be injected by the WarpGrid runtime without a real filesystem.

**Acceptance Criteria:**
- [ ] Test: a C program calling `fopen("/etc/resolv.conf", "r")` and reading its contents receives the string returned by the `filesystem.read-virtual` shim
- [ ] Test: the returned `FILE*` supports `fread`, `fgets`, `fclose`, and `feof` correctly on the in-memory buffer
- [ ] Test: `open("/etc/resolv.conf", O_RDONLY)` returns a valid file descriptor that supports `read()` and `close()` on the in-memory buffer
- [ ] Test: opening a non-virtual path (e.g., a preopened directory file) falls through to the original WASI implementation
- [ ] Test: `fopen` with write mode (`"w"`) on a virtual path returns `NULL` with `errno` set to `EROFS`
- [ ] Shim functions use weak symbol imports for graceful degradation
- [ ] Patch exported as `libc-patches/0004-fs-virtual-open.patch`

### US-207: Patch timezone loading to use virtual filesystem
**Milestone:** M2.3
**Depends on:** US-206
**Description:** As a developer whose WASI program calls `localtime()` or `strftime()`, I want timezone data to be loaded from the virtual path `/usr/share/zoneinfo/` via the WarpGrid filesystem shim, so that time-aware applications get correct timezone behavior without bundling tzdata into the Wasm module.

**Acceptance Criteria:**
- [ ] Test: a C program calling `localtime()` after `setenv("TZ", "America/New_York", 1)` reads `/usr/share/zoneinfo/America/New_York` via the virtual filesystem shim and returns the correct UTC offset
- [ ] Test: when the shim provides tzdata for `Europe/London`, `strftime` with `%Z` outputs the correct timezone abbreviation
- [ ] Test: when the shim returns an empty result for an unknown timezone, `localtime` falls back to UTC without crashing
- [ ] The `__tz.c` patch reuses the virtual file read mechanism from US-206 (no duplicated shim call logic)
- [ ] Patch exported as `libc-patches/0005-fs-timezone-virtual.patch`

### US-208: Verify filesystem patches with stock build and edge cases
**Milestone:** M2.3
**Depends on:** US-206, US-207
**Description:** As a SDK maintainer, I want comprehensive edge-case coverage for the virtual filesystem patches, so that programs behave correctly under boundary conditions and the patched libc remains compatible with stock builds.

**Acceptance Criteria:**
- [ ] Test: opening a virtual path, reading partial content with small buffer sizes (1 byte, 16 bytes), and seeking (`lseek`) produces correct results
- [ ] Test: opening the same virtual path twice concurrently returns independent file descriptors with independent read positions
- [ ] Test: a program compiled against the patched sysroot runs in vanilla Wasmtime (no shims) and `fopen("/etc/resolv.conf", "r")` returns `NULL` with `errno == ENOENT` (graceful degradation)
- [ ] Test: stock wasi-libc builds successfully with filesystem patches present in the repository
- [ ] Test: the in-memory buffer is freed when `fclose`/`close` is called (no file descriptor or memory leak across 1000 open/close cycles)

### US-209: Patch connect() to route database proxy connections
**Milestone:** M2.4
**Depends on:** US-201, US-202, US-112
**Description:** As a developer whose WASI program connects to a database endpoint, I want `connect()` to detect connections to WarpGrid proxy endpoints and route them through `warpgrid:shim/database-proxy.connect()`, returning a file descriptor backed by the proxy, so that database drivers (libpq, pgx) work transparently over the WarpGrid mesh.

**Acceptance Criteria:**
- [ ] Test: a C program calling `connect()` to the WarpGrid proxy address (e.g., `127.0.0.1:54321` or a well-known sentinel address) invokes `database-proxy.connect()` and receives a valid file descriptor
- [ ] Test: `connect()` to a non-proxy address (e.g., `93.184.216.34:80`) falls through to the original WASI socket implementation
- [ ] Test: the returned file descriptor is tracked internally as a "proxied fd" for subsequent send/recv routing
- [ ] Test: `connect()` with the proxy shim absent (weak symbol) falls through to default behavior without error
- [ ] Proxy endpoint detection is configurable via a virtual config file (`/etc/warpgrid/proxy.conf`) read through the filesystem shim from US-206
- [ ] Patch exported as `libc-patches/0006-socket-connect-proxy.patch`

### US-210: Patch send/recv/read/write for proxied file descriptors
**Milestone:** M2.4
**Depends on:** US-209
**Description:** As a developer whose WASI program communicates over a database connection, I want `send()`, `recv()`, `read()`, and `write()` on proxied file descriptors to route data through `warpgrid:shim/database-proxy.send()` and `database-proxy.recv()`, so that the database wire protocol flows transparently through the WarpGrid proxy.

**Acceptance Criteria:**
- [ ] Test: after `connect()` to the proxy, `send(fd, data, len, 0)` delivers data via `database-proxy.send()` and returns the correct byte count
- [ ] Test: `recv(fd, buf, len, 0)` on a proxied fd reads data from `database-proxy.recv()` and returns the correct byte count
- [ ] Test: `read(fd, buf, len)` and `write(fd, buf, len)` on proxied fds also route through the proxy (since libpq uses read/write, not send/recv)
- [ ] Test: `send`/`recv`/`read`/`write` on a non-proxied fd behave identically to unpatched wasi-libc
- [ ] Test: partial reads (proxy returns less data than requested) are handled correctly without data loss
- [ ] Test: `MSG_PEEK` flag on `recv` returns data without consuming it from the proxy buffer
- [ ] Patches exported as `libc-patches/0007-socket-send-recv-proxy.patch`

### US-211: Patch close() for proxied file descriptors and end-to-end database test
**Milestone:** M2.4
**Depends on:** US-210
**Description:** As a developer, I want `close()` on a proxied file descriptor to call `database-proxy.close()` and clean up the proxy tracking state, and I want to validate the full socket lifecycle end-to-end with a real database driver, so that connections are properly terminated and resources are freed.

**Acceptance Criteria:**
- [ ] Test: `close(fd)` on a proxied fd invokes `database-proxy.close()` and removes the fd from the internal proxy tracking table
- [ ] Test: after `close()`, subsequent `read`/`write`/`send`/`recv` on that fd return `-1` with `errno == EBADF`
- [ ] Test: closing a non-proxied fd behaves identically to unpatched wasi-libc
- [ ] Test: 100 sequential connect/send/recv/close cycles complete without fd leaks (fd numbers do not grow unboundedly)
- [ ] Patch exported as `libc-patches/0008-socket-close-proxy.patch`

### US-212: End-to-end database driver compilation and connection test
**Milestone:** M2.4
**Depends on:** US-211, US-203
**Description:** As a developer building a database-backed application for WarpGrid, I want to compile libpq (PostgreSQL C client) and a Go program using `database/sql` + `pgx` against the patched wasi-libc and successfully connect to a PostgreSQL instance through the WarpGrid proxy, so that real-world database drivers are validated end-to-end.

**Acceptance Criteria:**
- [ ] Test: libpq compiles against the patched wasi-libc sysroot without modifications to libpq source (or with minimal, documented `#ifdef __wasi__` changes)
- [ ] Test: a C program using libpq executes `PQconnectdb`, sends `SELECT 1`, receives the result, and disconnects cleanly through the WarpGrid database proxy shim
- [ ] Test: a Go program compiled with TinyGo using `database/sql` and `pgx` driver connects, executes `SELECT version()`, and disconnects cleanly through the proxy shim
- [ ] Test: connection errors (proxy unreachable, auth failure) propagate as standard libpq/pgx error codes, not crashes or hangs
- [ ] DNS resolution for the database hostname (e.g., `db.production.warp.local`) uses the DNS shim from US-203

### US-213: Comprehensive patch maintenance documentation and test harness
**Milestone:** M2.5
**Depends on:** US-202, US-205, US-208, US-211
**Description:** As a SDK maintainer onboarding to the project, I want complete documentation of the fork maintenance workflow and a single-command test harness that validates all patches, so that upstream rebases are predictable and the full patch suite can be verified in CI.

**Acceptance Criteria:**
- [ ] Test: `scripts/test-libc.sh --all` compiles and runs all test programs from `libc-patches/tests/` (DNS, filesystem, socket proxy) against the patched sysroot in Wasmtime with mock shims, and reports a summary with pass/fail counts
- [ ] Test: `scripts/test-libc.sh --stock` runs the same test suite against the stock sysroot and all shim-dependent tests are skipped (not failed) with a "shim not available" message
- [ ] Test: `scripts/test-libc.sh --ci` exits with code 0 only when all tests pass and produces JUnit XML output for CI integration
- [ ] `docs/FORK-MAINTENANCE.md` documents: upstream pin policy, how to update `UPSTREAM_REF`, full rebase workflow with conflict resolution steps, how to add a new patch, and how to run the test harness
- [ ] `docs/FORK-MAINTENANCE.md` includes a troubleshooting section covering the three most common rebase failure modes (context drift, deleted upstream file, conflicting upstream refactor)

### US-214: Patch series ordering and dependency validation
**Milestone:** M2.5
**Depends on:** US-213
**Description:** As a CI system applying the WarpGrid patch series, I want the patches to be numbered and ordered so that each patch applies cleanly in sequence and partial application (e.g., DNS-only or filesystem-only) is supported, so that downstream consumers can opt into subsets of WarpGrid functionality.

**Acceptance Criteria:**
- [ ] Test: applying patches `0001` through `0008` sequentially with `git am` succeeds on a fresh checkout of the pinned upstream ref
- [ ] Test: applying only DNS patches (`0001`-`0003`) produces a sysroot where DNS shims work and filesystem/socket code is unmodified stock wasi-libc
- [ ] Test: applying only filesystem patches (`0004`-`0005`) on top of stock produces a sysroot where virtual filesystem works and DNS/socket code is stock
- [ ] Test: applying socket patches (`0006`-`0008`) without filesystem patches fails with a clear error message (since US-209 depends on virtual config file from US-206)
- [ ] Each patch file includes a header comment documenting its purpose, dependencies on other patches, and the WarpGrid WIT interface it requires
- [ ] `scripts/rebase-libc.sh --validate` checks patch ordering and dependency constraints, reporting errors for misordered or missing dependency patches

---

## Domain 3: TinyGo WASI Overlay (`warpgrid-go`)

### US-301: Fork TinyGo and establish reproducible build pipeline
**Milestone:** M3.1
**Depends on:** none
**Description:** As a SDK maintainer, I want a forked TinyGo repository with an automated build-from-source pipeline, so that we have a stable baseline for applying WASI patches.

**Acceptance Criteria:**
- [ ] TinyGo is cloned at the latest release tag (>= 0.34.x) into `vendor/tinygo/`
- [ ] `scripts/build-tinygo.sh` builds TinyGo from source using Go 1.22+ and the required LLVM version
- [ ] A hello-world Go program compiles successfully with `tinygo build -target=wasip2 -o test.wasm`
- [ ] The resulting `test.wasm` executes correctly in Wasmtime and prints expected output
- [ ] Build script is idempotent: running it twice produces the same artifact
- [ ] CI configuration runs the build script and caches the LLVM toolchain
- [ ] Tests written before implementation (TDD)

### US-302: Audit Go stdlib compatibility for wasip2 target
**Milestone:** M3.1
**Depends on:** US-301
**Description:** As a SDK maintainer, I want a documented compatibility matrix of common Go stdlib packages against the wasip2 target, so that we know exactly which packages need patching and can prioritize work.

**Acceptance Criteria:**
- [ ] A test suite exercises 20 common Go packages: `fmt`, `strings`, `strconv`, `encoding/json`, `encoding/base64`, `crypto/sha256`, `crypto/tls`, `math`, `sort`, `bytes`, `io`, `os`, `net`, `net/http`, `database/sql`, `context`, `sync`, `time`, `regexp`, `log`
- [ ] Each package has a minimal Go program that imports and exercises core functionality
- [ ] Each is compiled with `tinygo build -target=wasip2` and the result is recorded as pass / fail / partial
- [ ] A machine-readable compatibility matrix (`compat-db/tinygo-stdlib.json`) is generated with package name, status, error message (if any), and TinyGo version
- [ ] The matrix is regenerated by a single script (`scripts/audit-tinygo-stdlib.sh`)
- [ ] Tests written before implementation (TDD)

### US-303: Patch net.Dial for TCP via wasi-sockets
**Milestone:** M3.2
**Depends on:** US-301, US-102
**Description:** As a Go developer targeting Wasm, I want `net.Dial("tcp", "host:port")` to work in my TinyGo wasip2 build, so that my Go programs can establish outbound TCP connections from inside a Wasm module.

**Acceptance Criteria:**
- [ ] `src/syscall/syscall_wasip2.go` is patched so that `socket()`, `connect()`, `read()`, `write()`, and `close()` syscalls delegate to wasi-sockets or the WarpGrid socket proxy
- [ ] `src/net/` dial logic is patched to construct a valid `net.Conn` from a WASI socket file descriptor
- [ ] A Go program calling `net.Dial("tcp", "echo-server:7")` compiles with patched TinyGo and successfully sends/receives bytes when run against a TCP echo server in the test harness
- [ ] Error cases return idiomatic Go errors: connection refused, timeout, invalid address
- [ ] Patch is isolated into clearly delimited blocks marked with `// WARPGRID PATCH START` / `// WARPGRID PATCH END`
- [ ] Tests written before implementation (TDD)

### US-304: Patch net.Dial to support DNS resolution via WarpGrid DNS shim
**Milestone:** M3.2
**Depends on:** US-303, US-106
**Description:** As a Go developer targeting Wasm, I want `net.Dial("tcp", "my-service:5432")` to resolve hostnames through the WarpGrid DNS shim, so that I can connect to services by name rather than IP address.

**Acceptance Criteria:**
- [ ] The patched `net.Dial` path calls `warpgrid:dns/resolve` (WIT import) when the host component is not an IP literal
- [ ] Resolved addresses are used to establish the TCP connection via the socket path from US-303
- [ ] A Go program calling `net.Dial("tcp", "postgres:5432")` resolves `postgres` to the correct IP and connects
- [ ] When DNS resolution fails, the error wraps the underlying DNS error and is surfaced as a `*net.OpError`
- [ ] Multiple A records are tried in order until one succeeds (basic failover)
- [ ] Tests written before implementation (TDD)

### US-305: Validate pgx Postgres driver over patched net.Dial
**Milestone:** M3.2
**Depends on:** US-304, US-112
**Description:** As a Go developer, I want the `pgx` Postgres driver to work with patched TinyGo wasip2, so that I can connect to Postgres databases from Go Wasm modules using a real database driver.

**Acceptance Criteria:**
- [ ] A Go program imports `github.com/jackc/pgx/v5` and connects to a Postgres instance via `pgx.Connect(ctx, connString)`
- [ ] The program compiles with patched TinyGo (`tinygo build -target=wasip2`)
- [ ] The compiled module runs in the test harness, executes `SELECT 1`, and returns the result
- [ ] The compiled module runs a `CREATE TABLE`, `INSERT`, `SELECT`, `DROP TABLE` sequence successfully
- [ ] Any pgx features that fail to compile are documented in `compat-db/tinygo-pgx.json` with workaround notes
- [ ] Tests written before implementation (TDD)

### US-306: Implement warpgrid/net/http overlay — ListenAndServe registration
**Milestone:** M3.3
**Depends on:** US-301, US-102
**Description:** As a Go developer, I want `http.ListenAndServe(":8080", handler)` to compile and register my handler with the WarpGrid trigger system, so that I can write standard Go HTTP server code that runs inside a Wasm module.

**Acceptance Criteria:**
- [ ] A `warpgrid/net/http` overlay package is created that shadows `net/http` for wasip2 builds
- [ ] `ListenAndServe(addr, handler)` does not open a socket; instead it registers `handler` with the WarpGrid inbound trigger system via the `warpgrid:http/incoming-handler` WIT export
- [ ] `http.Request` and `http.ResponseWriter` are backed by wasi-http types internally but present the standard Go interface
- [ ] A Go program using `http.HandleFunc("/", fn)` followed by `http.ListenAndServe(":8080", nil)` compiles to wasip2
- [ ] Tests written before implementation (TDD)

### US-307: Implement warpgrid/net/http overlay — request/response round-trip
**Milestone:** M3.3
**Depends on:** US-306
**Description:** As a Go developer, I want my standard Go HTTP handler to receive real HTTP requests and return responses through the WarpGrid trigger system, so that I can handle HTTP traffic end-to-end inside a Wasm module.

**Acceptance Criteria:**
- [ ] An incoming wasi-http request (method, URL, headers, body) is fully converted to a Go `*http.Request` with correct fields
- [ ] The handler's writes to `http.ResponseWriter` (status code, headers, body) are fully converted to a wasi-http outgoing response
- [ ] Streaming request bodies are supported via `io.Reader` on the request
- [ ] The handler can set arbitrary response headers and status codes (200, 201, 400, 404, 500)
- [ ] A test harness sends an HTTP request to the Wasm module and validates the full response
- [ ] Tests written before implementation (TDD)

### US-308: Database driver compatibility — MySQL and Redis
**Milestone:** M3.4
**Depends on:** US-305, US-112
**Description:** As a SDK maintainer, I want to test and document MySQL and Redis driver compatibility with patched TinyGo wasip2, so that Go developers know which database clients work and have workarounds for those that don't.

**Acceptance Criteria:**
- [ ] `go-sql-driver/mysql` is tested: compile, connect, execute `SELECT 1`, run a CRUD cycle
- [ ] `go-redis/redis` is tested: compile, connect, execute `PING`, `SET`/`GET` cycle
- [ ] For each driver, results are recorded in `compat-db/tinygo-drivers.json` with status (pass/fail/partial), error details, and workarounds
- [ ] If a driver fails to compile, the specific unsupported stdlib dependency is identified
- [ ] If a driver compiles but fails at runtime, the specific WASI gap is identified
- [ ] Where feasible, a `warpgrid/database/sql` overlay adapter is provided to bridge gaps
- [ ] Tests written before implementation (TDD)

### US-309: Patch rebase tooling and reproducible build script
**Milestone:** M3.5
**Depends on:** US-301
**Description:** As a SDK maintainer, I want automated scripts to rebase our TinyGo patches onto new upstream releases, so that we can stay current with TinyGo without manual merge conflict resolution every time.

**Acceptance Criteria:**
- [ ] `scripts/rebase-tinygo.sh <upstream-tag>` rebases all WarpGrid patches onto the specified upstream tag
- [ ] Patches are maintained as a `git format-patch` series in `patches/tinygo/` for clean reapplication
- [ ] The rebase script detects conflicts and exits with a clear error message listing conflicting files
- [ ] `scripts/build-tinygo.sh` applies patches from `patches/tinygo/` onto a clean checkout, builds, and outputs the patched `tinygo` binary
- [ ] A CI job runs the full cycle: checkout upstream, apply patches, build, run test suite
- [ ] The build produces a deterministic output given the same inputs (upstream tag + patch set)
- [ ] Tests written before implementation (TDD)

### US-310: Integrate patched TinyGo into `warp pack --lang go`
**Milestone:** M3.5
**Depends on:** US-309
**Description:** As a Go developer using the WarpGrid CLI, I want `warp pack --lang go` to automatically use the patched TinyGo to compile my project into a Wasm component, so that I don't need to manually manage the patched compiler.

**Acceptance Criteria:**
- [ ] `warp pack --lang go` detects Go source files and invokes the patched TinyGo binary (`warp-tinygo`) with `-target=wasip2`
- [ ] The CLI resolves the `warp-tinygo` binary from the SDK installation path or `$WARPGRID_TINYGO_PATH`
- [ ] The output `.wasm` component is placed in the standard `dist/` directory
- [ ] If `warp-tinygo` is not found, the CLI prints an actionable error message with installation instructions
- [ ] A Go project with `net`, `net/http`, and `pgx` imports compiles successfully through this command
- [ ] Tests written before implementation (TDD)

### US-311: End-to-end Go HTTP handler with Postgres integration test
**Milestone:** M3.4
**Depends on:** US-305, US-307, US-310
**Description:** As a SDK maintainer, I want a full end-to-end test that compiles a Go HTTP handler using pgx, deploys it as a Wasm component, and validates request-response with real database interaction, so that we have confidence the entire Domain 3 stack works together.

**Acceptance Criteria:**
- [ ] A sample Go application defines an HTTP handler that accepts a POST with a JSON body, inserts a record into Postgres, and returns the inserted row as JSON
- [ ] The application compiles via `warp pack --lang go` using the patched TinyGo
- [ ] The test harness spins up Postgres, deploys the Wasm component, sends an HTTP request, and validates the full response
- [ ] The test verifies: HTTP status 200, correct JSON response body, record exists in Postgres
- [ ] The test also verifies error handling: malformed JSON returns 400, database down returns 503
- [ ] Tests written before implementation (TDD)

---

## Domain 4: ComponentizeJS Extensions (`warpgrid-js`)

### US-401: Fork ComponentizeJS and establish reproducible build pipeline
**Milestone:** M4.1
**Depends on:** none
**Description:** As a SDK maintainer, I want a forked ComponentizeJS repository with an automated build and verification pipeline, so that we have a stable baseline for extending the JS runtime with WarpGrid shims.

**Acceptance Criteria:**
- [ ] ComponentizeJS is cloned at the latest release tag into `vendor/componentize-js/`
- [ ] `scripts/build-componentize-js.sh` builds the project from source and produces the componentize binary/library
- [ ] A simple HTTP handler (`export default { fetch(req) { return new Response("ok") } }`) is componentized into a Wasm component
- [ ] The resulting component runs in Wasmtime and returns "ok" for an HTTP request
- [ ] Build script is idempotent and CI runs it with caching
- [ ] Tests written before implementation (TDD)

### US-402: Audit Node.js API surface and npm package compatibility baseline
**Milestone:** M4.1
**Depends on:** US-401
**Description:** As a SDK maintainer, I want a documented inventory of which Node.js APIs and npm packages are available in the ComponentizeJS runtime, so that we know exactly what needs shimming and can prioritize.

**Acceptance Criteria:**
- [ ] An audit script tests availability of core Node.js APIs: `Buffer`, `TextEncoder`/`TextDecoder`, `crypto`, `URL`, `console`, `setTimeout`, `process.env`, `fs`, `net`, `dns`, `http`
- [ ] The audit tests import of 20 npm packages: `pg`, `ioredis`, `express`, `hono`, `zod`, `jose`, `uuid`, `lodash-es`, `date-fns`, `nanoid`, `superjson`, `drizzle-orm`, `kysely`, `better-sqlite3`, `node-fetch`, `axios`, `dotenv`, `pino`, `fast-json-stringify`, `ajv`
- [ ] Each package is tested for: successful import, basic function call, runtime execution
- [ ] Results are recorded in `compat-db/componentize-js-baseline.json` with status, error, and missing API details
- [ ] The audit is runnable via `scripts/audit-componentize-js.sh`
- [ ] Tests written before implementation (TDD)

### US-403: Implement warpgrid.database.connect() global function
**Milestone:** M4.2
**Depends on:** US-401, US-112, US-102
**Description:** As a JavaScript developer writing Wasm handlers, I want a `warpgrid.database.connect(config)` function available in the JS runtime, so that I can establish database connections through the WarpGrid database proxy from within a componentized handler.

**Acceptance Criteria:**
- [ ] A `warpgrid` global object is injected into the ComponentizeJS runtime during componentization
- [ ] `warpgrid.database.connect({ host, port, database, username, password })` returns a connection object
- [ ] The connection object exposes `send(data: Uint8Array): void` and `recv(maxBytes: number): Uint8Array` methods
- [ ] Under the hood, `connect()` calls the `warpgrid:database/proxy` WIT import to establish a proxied connection
- [ ] `send()` and `recv()` operate on the raw protocol bytes (Postgres wire protocol, MySQL protocol, etc.)
- [ ] Calling `connect()` with invalid config throws a descriptive `WarpGridError`
- [ ] Tests written before implementation (TDD)

### US-404: Implement warpgrid/pg package with pg.Client-compatible interface
**Milestone:** M4.2
**Depends on:** US-403
**Description:** As a JavaScript developer, I want a `warpgrid/pg` package that implements the `pg.Client` interface on top of `warpgrid.database.connect()`, so that I can use familiar Postgres query patterns without learning a new API.

**Acceptance Criteria:**
- [ ] `warpgrid/pg` exports a `Client` class compatible with the `pg` package interface
- [ ] `new Client(config)` accepts standard `pg` connection config (`host`, `port`, `database`, `user`, `password`)
- [ ] `client.connect()` establishes a connection via `warpgrid.database.connect()` and performs the Postgres startup/auth handshake
- [ ] `client.query(sql, params?)` sends a parameterized query over the Postgres wire protocol and returns `{ rows, rowCount, fields }`
- [ ] `client.end()` cleanly closes the connection
- [ ] The package handles Postgres error responses and surfaces them as JavaScript `Error` objects with `code`, `message`, and `detail`
- [ ] Tests written before implementation (TDD)

### US-405: End-to-end TypeScript HTTP handler with Postgres query
**Milestone:** M4.2
**Depends on:** US-404
**Description:** As a JavaScript developer, I want to write a TypeScript HTTP handler that connects to Postgres, runs a query, and returns the result as an HTTP response, so that I can build real data-driven API endpoints as Wasm components.

**Acceptance Criteria:**
- [ ] A sample TypeScript handler imports `warpgrid/pg`, connects to Postgres in the handler body, runs `SELECT * FROM users WHERE id = $1`, and returns the result as JSON
- [ ] The handler compiles and componentizes via the WarpGrid build toolchain
- [ ] The test harness provisions Postgres with seed data, deploys the Wasm component, sends an HTTP request, and validates the JSON response
- [ ] The response includes correct `Content-Type: application/json` header and proper JSON body
- [ ] Error cases are tested: missing query parameter returns 400, database error returns 500
- [ ] Tests written before implementation (TDD)

### US-406: Implement warpgrid.dns.resolve() shim
**Milestone:** M4.3
**Depends on:** US-401, US-106, US-102
**Description:** As a JavaScript developer writing Wasm handlers, I want a `warpgrid.dns.resolve(hostname)` function in the JS runtime, so that I can resolve service hostnames to IP addresses within my handler.

**Acceptance Criteria:**
- [ ] `warpgrid.dns.resolve(hostname)` is available on the `warpgrid` global object
- [ ] It calls the `warpgrid:dns/resolve` WIT import and returns a `Promise<string[]>` of resolved IP addresses
- [ ] Resolving a valid hostname returns one or more IP address strings
- [ ] Resolving an invalid or unresolvable hostname rejects with a `WarpGridDNSError` including the hostname
- [ ] The function supports both IPv4 and IPv6 addresses in the result
- [ ] Tests written before implementation (TDD)

### US-407: Implement warpgrid.fs.readFile() and process.env polyfills
**Milestone:** M4.3
**Depends on:** US-401, US-104, US-102
**Description:** As a JavaScript developer writing Wasm handlers, I want `warpgrid.fs.readFile(path)` and `process.env` to work in the JS runtime, so that I can read configuration files and environment variables from my handler.

**Acceptance Criteria:**
- [ ] `warpgrid.fs.readFile(path)` reads a file from the WASI virtual filesystem and returns a `Promise<Uint8Array>`
- [ ] An optional `encoding` parameter (`warpgrid.fs.readFile(path, "utf-8")`) returns a `Promise<string>`
- [ ] Reading a nonexistent path rejects with a `WarpGridFSError` including the path
- [ ] Paths are restricted to the WASI preopened directories; attempting to escape returns an error
- [ ] `process.env` returns an object populated from WASI environment variables
- [ ] `process.env.MY_VAR` returns the value of the `MY_VAR` environment variable or `undefined`
- [ ] Tests written before implementation (TDD)

### US-408: Integration test — DNS, env vars, and file read in a single handler
**Milestone:** M4.3
**Depends on:** US-406, US-407
**Description:** As a SDK maintainer, I want a test that exercises DNS resolution, environment variable access, and file reading together in one handler, so that we validate that all polyfills work in combination without conflicts.

**Acceptance Criteria:**
- [ ] A TypeScript handler reads `process.env.SERVICE_HOST`, resolves it via `warpgrid.dns.resolve()`, reads timezone data from `warpgrid.fs.readFile("/usr/share/zoneinfo/UTC")`, and returns a JSON response containing all three results
- [ ] The test harness sets environment variables, configures DNS, mounts the virtual filesystem, and validates the combined response
- [ ] The handler correctly handles partial failures: if DNS fails, it returns the error for that field while still returning env and file results
- [ ] Tests written before implementation (TDD)

### US-409: npm package compatibility testing and documentation
**Milestone:** M4.4
**Depends on:** US-405, US-407
**Description:** As a SDK maintainer, I want a comprehensive compatibility report for the top 20 npm packages against the WarpGrid-extended ComponentizeJS runtime, so that JavaScript developers know which packages work out of the box and which require alternatives.

**Acceptance Criteria:**
- [ ] Each of the 20 packages from US-402 is retested against the WarpGrid-extended runtime (with database, DNS, and FS shims active)
- [ ] Each package is tested at three levels: (1) import succeeds, (2) basic function call works, (3) realistic usage scenario runs
- [ ] Results are recorded in `compat-db/componentize-js-warpgrid.json` with: package name, version, import status, runtime status, blocking issue, workaround
- [ ] Packages that now work due to WarpGrid shims (compared to US-402 baseline) are highlighted
- [ ] A human-readable compatibility table is generated for documentation
- [ ] Tests written before implementation (TDD)

### US-410: Integrate ComponentizeJS extensions into `warp pack --lang js`
**Milestone:** M4.4
**Depends on:** US-401, US-403, US-406, US-407
**Description:** As a JavaScript developer using the WarpGrid CLI, I want `warp pack --lang js` to automatically use the extended ComponentizeJS to compile my TypeScript/JavaScript handler into a Wasm component, so that I get WarpGrid shims injected without manual configuration.

**Acceptance Criteria:**
- [ ] `warp pack --lang js` detects JS/TS source files and invokes the patched ComponentizeJS with WarpGrid shim injection enabled
- [ ] The `warpgrid` global object (database, dns, fs) is automatically available in the componentized output
- [ ] `process.env` polyfill is injected without explicit developer configuration
- [ ] The output `.wasm` component is placed in the standard `dist/` directory
- [ ] If the extended ComponentizeJS is not found, the CLI prints an actionable error with installation instructions
- [ ] A TypeScript handler using `warpgrid/pg`, `warpgrid.dns.resolve`, and `process.env` compiles successfully
- [ ] Tests written before implementation (TDD)

---

## Domain 5: WASI 0.3 Async Pre-Integration (`warpgrid-async`)

### US-501: Clone and build Wasmtime from the wasip3-prototyping branch
**Milestone:** M5.1
**Depends on:** US-102
**Description:** As a platform engineer, I want a reproducible build of Wasmtime from the wasip3-prototyping branch checked into our CI pipeline, so that we have a known-good async-capable runtime to develop against before WASI 0.3 reaches stable upstream.

**Acceptance Criteria:**
- [ ] `scripts/build-wasmtime-async.sh` clones the `wasip3-prototyping` branch at a pinned commit SHA and builds Wasmtime with the `component-model-async` feature enabled
- [ ] A test compiles and instantiates a minimal WASI 0.3 async component (an `incoming-handler` that returns a static 200 response) and asserts the response status and body
- [ ] CI caches the built Wasmtime binary; subsequent runs skip the build if the pinned SHA has not changed
- [ ] A `docs/wasi-03-api-surface.md` file documents every available async interface, every known-unstable export, and the pinned commit SHA with date
- [ ] Tests written before implementation (TDD)

### US-502: Verify async HTTP round-trip on the prototyping runtime
**Milestone:** M5.1
**Depends on:** US-501
**Description:** As a platform engineer, I want an end-to-end test proving a WASI 0.3 async HTTP handler can receive a request and return a response through the prototyping runtime, so that we have confidence the async path works before building our wrapper API on top of it.

**Acceptance Criteria:**
- [ ] A Rust guest component implements `wasi:http/incoming-handler@0.3.0-draft` with a genuinely async body: it reads the request body stream to completion, echoes it back, and sets a `x-async: true` response header
- [ ] A host-side integration test sends a multi-chunk request body, asserts the echoed response matches, and asserts the `x-async` header is present
- [ ] A second test sends 10 concurrent requests to the same component instance and asserts all 10 responses arrive without deadlock within a 5-second timeout
- [ ] Test failures surface the Wasmtime async-tracing output in CI logs for debugging
- [ ] Tests written before implementation (TDD)

### US-503: Create `warpgrid-async` crate with stable `AsyncHandler` trait
**Milestone:** M5.2
**Depends on:** US-501, US-502
**Description:** As a module author, I want a stable Rust trait I can implement for async request handling that shields me from upstream prototyping-branch churn, so that my handler code does not break when the WASI 0.3 draft evolves.

**Acceptance Criteria:**
- [ ] `crates/warpgrid-async/src/lib.rs` exports `pub trait AsyncHandler { async fn handle_request(&self, req: Request) -> Response; }` where `Request` and `Response` are WarpGrid-owned types
- [ ] Conversion functions between WarpGrid types and prototyping-branch types are internal (`pub(crate)`) and covered by unit tests that assert round-trip fidelity for headers, status codes, and streaming bodies
- [ ] A compile-time test (trybuild or similar) confirms that a user implementing `AsyncHandler` does not need to import anything from the prototyping branch directly
- [ ] The crate's `Cargo.toml` pins the prototyping-branch Wasmtime dependency to the same SHA as US-501
- [ ] Tests written before implementation (TDD)

### US-504: Implement `AsyncHandler` host-side adapter for `WarpGridEngine`
**Milestone:** M5.2
**Depends on:** US-503
**Description:** As a platform engineer, I want `WarpGridEngine` to instantiate and invoke modules that export `AsyncHandler`, so that async modules are first-class citizens in the WarpGrid runtime alongside sync modules.

**Acceptance Criteria:**
- [ ] `WarpGridEngine::load_module()` detects whether a component exports the async handler interface and selects the async instantiation path automatically
- [ ] An integration test loads an async echo handler module via `WarpGridEngine`, sends a request through the engine's public API, and asserts the correct response
- [ ] A second test loads a sync module and an async module side-by-side in the same engine instance, sends one request to each, and asserts both respond correctly
- [ ] If a module claims async export but the prototyping runtime is not available, `load_module()` returns a descriptive `Err` rather than panicking
- [ ] Tests written before implementation (TDD)

### US-505: Expose async streaming body support through the wrapper API
**Milestone:** M5.2
**Depends on:** US-503
**Description:** As a module author, I want the `warpgrid-async` wrapper to support streaming request and response bodies, so that I can handle large payloads without buffering the entire body in memory.

**Acceptance Criteria:**
- [ ] `Request` exposes `pub fn body_stream(&self) -> impl Stream<Item = Result<Bytes, Error>>` and `Response` can be constructed with a streaming body via `Response::streaming(status, headers, impl Stream<Item = Bytes>)`
- [ ] A unit test creates a response with a 10 MB streaming body composed of 1 KB chunks, consumes it, and asserts total byte count and chunk ordering
- [ ] An integration test through the prototyping runtime sends a 1 MB chunked request to an async handler that transforms each chunk (e.g., uppercases ASCII) and streams it back, asserting the transformed output matches
- [ ] Memory profiling annotation confirms the streaming path does not buffer more than 2x the chunk size at any point
- [ ] Tests written before implementation (TDD)

### US-506: Convert database proxy shim to async I/O
**Milestone:** M5.3
**Depends on:** US-112, US-504
**Description:** As a platform engineer, I want the database proxy shim from Domain 1 to use non-blocking async I/O through the `warpgrid-async` primitives, so that modules can issue database queries without blocking the async executor and starving other concurrent tasks.

**Acceptance Criteria:**
- [ ] The database proxy's `send_query` and `receive_results` functions are async and use the prototyping runtime's async streams rather than blocking WASI I/O
- [ ] The connection pool's `checkout()` method is async; a test spawns 20 concurrent checkout requests against a pool of size 5 and asserts all 20 complete without deadlock within 10 seconds, with no more than 5 connections open simultaneously
- [ ] An integration test runs an async handler that issues three sequential queries in a single request and asserts all three results are correct
- [ ] Existing sync database proxy tests continue to pass (sync path is preserved as a fallback)
- [ ] Tests written before implementation (TDD)

### US-507: Benchmark async database proxy throughput vs sync baseline
**Milestone:** M5.3
**Depends on:** US-506
**Description:** As a platform engineer, I want a reproducible benchmark comparing concurrent query throughput between the async and sync database proxy paths, so that we can quantify the performance benefit and detect regressions.

**Acceptance Criteria:**
- [ ] `benches/async_db_proxy.rs` uses `criterion` to measure queries-per-second at concurrency levels 1, 10, 50, and 100 for both the async and sync paths against a mock database backend with configurable latency (0 ms, 5 ms, 50 ms)
- [ ] At 50 ms simulated latency and concurrency 50, the async path achieves at least 5x the throughput of the sync path
- [ ] Benchmark results are written to `target/criterion/` and a CI step stores them as artifacts
- [ ] A test validates the benchmark harness itself: running at concurrency 1 with 0 ms latency produces results within 10% between async and sync (sanity check)
- [ ] Tests written before implementation (TDD)

### US-508: Verify Rust async handlers compile against WASI 0.3 WIT with wit-bindgen
**Milestone:** M5.4
**Depends on:** US-102, US-503
**Description:** As a Rust module author, I want confirmation that `wit-bindgen`-generated async bindings produce a valid WASI 0.3 component when compiled against the WarpGrid WIT, so that I can use the standard Rust toolchain to write async handlers without manual glue code.

**Acceptance Criteria:**
- [ ] A test project under `tests/fixtures/rust-async-handler/` uses `wit-bindgen` with the `async` flag to generate bindings from WarpGrid's WIT package and implements a handler that reads a JSON body, queries a mock service, and returns a transformed JSON response
- [ ] `cargo component build` produces a `.wasm` file that passes `wasm-tools component wit` validation against `wasi:http/incoming-handler@0.3.0-draft`
- [ ] An integration test loads the compiled component into `WarpGridEngine` and exercises the full request/response cycle
- [ ] The fixture project's `Cargo.toml` documents the minimum `wit-bindgen` version required
- [ ] Tests written before implementation (TDD)

### US-509: Create async handler template projects for Rust, Go, and TypeScript
**Milestone:** M5.4
**Depends on:** US-508
**Description:** As a module author working in Rust, Go, or TypeScript, I want a `warp init --template async-<lang>` command that scaffolds a working async handler project, so that I can start writing async WarpGrid modules in under five minutes.

**Acceptance Criteria:**
- [ ] `warp init --template async-rust` creates a project that compiles with `warp pack` and produces a valid WASI 0.3 async component; an integration test runs this end-to-end
- [ ] `warp init --template async-go` creates a project using TinyGo with async stubs; a test verifies the project builds and the output component validates against the WIT
- [ ] `warp init --template async-ts` creates a project using `jco componentize` with async handler bindings; a test verifies the project builds and validates
- [ ] Each template includes a README with a "Getting Started" section, a working handler that returns `{"status":"ok"}`, and a test file that the language's native test runner can execute
- [ ] A parameterized test iterates over all three templates, runs `warp init`, `warp pack`, and `warp validate`, asserting success for each
- [ ] Tests written before implementation (TDD)

### US-510: Document async handler authoring guide for Rust, Go, and TypeScript
**Milestone:** M5.4
**Depends on:** US-508, US-509
**Description:** As a module author, I want a comprehensive guide explaining how to write, test, and deploy async handlers in each supported language, so that I can adopt WASI 0.3 async without reading upstream prototyping-branch internals.

**Acceptance Criteria:**
- [ ] `docs/guides/async-handlers.md` covers: conceptual overview of WASI 0.3 async model, language-specific sections for Rust, Go, and TypeScript, streaming body patterns, async database access, error handling, and known limitations
- [ ] Each code example in the guide is extracted from files under `docs/examples/async-*` that are compiled and tested in CI (no stale examples)
- [ ] The guide includes a "Migration from sync handlers" section with a diff-style before/after for each language
- [ ] A link checker test validates all internal cross-references and external URLs in the document
- [ ] Tests written before implementation (TDD)

---

## Domain 6: Bun WASI Runtime (`warpgrid-bun`)

### US-601: Scaffold `crates/warpgrid-bun/` and `warpgrid-bun-sdk` npm package
**Milestone:** M6.1
**Depends on:** US-102
**Description:** As a platform engineer, I want the foundational Rust crate and npm package structure for Bun support established with passing CI, so that subsequent Bun stories have a home for their code and dependencies.

**Acceptance Criteria:**
- [ ] `crates/warpgrid-bun/` exists with a `Cargo.toml` that compiles (`cargo check`), a `src/lib.rs` with a placeholder public function, and a unit test that passes
- [ ] `packages/warpgrid-bun-sdk/` exists with a `package.json` naming the package `@warpgrid/bun-sdk`, a `tsconfig.json`, and a `src/index.ts` that exports `WarpGridHandler` as a TypeScript interface: `{ fetch(request: Request): Promise<Response> }`
- [ ] `bun test` in the SDK package passes with at least one test asserting that a trivial handler conforming to `WarpGridHandler` type-checks and returns a `Response`
- [ ] `bun build ./src/index.ts --outdir ./dist` produces a bundle without errors
- [ ] Both the crate and the npm package are included in the workspace-level CI pipeline
- [ ] Tests written before implementation (TDD)

### US-602: Define and validate the `WarpGridHandler` Bun interface contract
**Milestone:** M6.1
**Depends on:** US-601
**Description:** As a Bun module author, I want a typed `WarpGridHandler` interface with runtime validation, so that I get clear errors when my handler does not conform to the expected contract before attempting Wasm compilation.

**Acceptance Criteria:**
- [ ] `@warpgrid/bun-sdk` exports a `validateHandler(handler: unknown): asserts handler is WarpGridHandler` function that checks the handler has a callable `fetch` property
- [ ] A test passes a valid handler (returning `new Response("ok")`) and asserts no error is thrown
- [ ] A test passes an object without `fetch` and asserts a descriptive `WarpGridHandlerValidationError` is thrown with a message specifying what is missing
- [ ] A test passes an object where `fetch` is not a function and asserts the same error type with an appropriate message
- [ ] The interface supports an optional `init()` lifecycle hook: `init?(): Promise<void>`; a test confirms a handler with `init` is accepted
- [ ] Tests written before implementation (TDD)

### US-603: Implement `warp pack --lang bun` compilation pipeline
**Milestone:** M6.2
**Depends on:** US-601, US-602, US-102
**Description:** As a Bun module author, I want `warp pack --lang bun` to compile my Bun handler into a valid WASI HTTP Wasm component, so that I can deploy Bun-native code to WarpGrid without manual toolchain steps.

**Acceptance Criteria:**
- [ ] `warp pack --lang bun` executes the pipeline: `bun build` (produce single-file bundle) -> `jco componentize` (produce Wasm component) -> `wasm-tools component wit` validates the output exports `wasi:http/incoming-handler`
- [ ] A test provides a minimal Bun handler (`export default { fetch(req) { return new Response("hello") } }`) and asserts the pipeline produces a `.wasm` file that passes WIT validation
- [ ] If `bun build` fails, the error message includes the Bun stderr output and the exit code
- [ ] If `jco componentize` fails, the error message includes the jco stderr output, a hint about unsupported APIs, and a link to the compatibility guide
- [ ] The output `.wasm` file is written to `target/wasm/<module-name>.wasm` consistent with other language targets
- [ ] Tests written before implementation (TDD)

### US-604: Create `@warpgrid/bun-polyfills` shim package for Bun-specific APIs
**Milestone:** M6.2
**Depends on:** US-603
**Description:** As a Bun module author, I want Bun-specific APIs (`Bun.file()`, `Bun.env`, `Bun.sleep()`, `Bun.serve()`) shimmed to WASI equivalents during compilation, so that I can use familiar Bun patterns and have them transparently work in the Wasm runtime.

**Acceptance Criteria:**
- [ ] `packages/warpgrid-bun-polyfills/` exports a `Bun` global shim with `file()`, `env`, `sleep()`, and `serve()` implementations
- [ ] `Bun.env` delegates to WASI environment variable access; a test sets `FOO=bar` in the Wasm environment and asserts `Bun.env.FOO === "bar"`
- [ ] `Bun.sleep(ms)` delegates to WASI clocks; a test asserts that `await Bun.sleep(100)` resolves after approximately 100 ms (within 50 ms tolerance)
- [ ] `Bun.file(path)` delegates to WASI filesystem; returns a `BunFile`-compatible object with `.text()`, `.arrayBuffer()`, and `.size`
- [ ] `Bun.serve()` throws a descriptive error explaining that WarpGrid manages the HTTP listener and the user should export a `WarpGridHandler` instead
- [ ] `warp pack --lang bun` automatically injects the polyfills package into the bundle
- [ ] Tests written before implementation (TDD)

### US-605: Test and validate compilation pipeline with a realistic Bun handler
**Milestone:** M6.2
**Depends on:** US-603, US-604
**Description:** As a platform engineer, I want an end-to-end test that compiles a non-trivial Bun handler through `warp pack --lang bun` and runs it in Wasmtime, so that we have confidence the full pipeline works beyond trivial "hello world" cases.

**Acceptance Criteria:**
- [ ] A test fixture at `tests/fixtures/bun-json-api/` implements a handler that: parses a JSON request body, validates required fields, transforms data, and returns a JSON response with appropriate headers
- [ ] `warp pack --lang bun` compiles the fixture without errors
- [ ] An integration test loads the resulting `.wasm` into `WarpGridEngine`, sends a valid JSON request, and asserts the response body, status, and `content-type` header
- [ ] A second test sends an invalid JSON body and asserts the handler returns a 400 status with an error message
- [ ] Tests written before implementation (TDD)

### US-606: Implement `@warpgrid/bun-sdk/postgres` with `createPool()`
**Milestone:** M6.3
**Depends on:** US-112, US-601
**Description:** As a Bun module author, I want a `createPool()` function in `@warpgrid/bun-sdk/postgres` that works in both native development mode and deployed Wasm mode, so that I can query PostgreSQL using familiar connection pool patterns without changing code between environments.

**Acceptance Criteria:**
- [ ] `@warpgrid/bun-sdk/postgres` exports `createPool(config?: PoolConfig): Pool` where `Pool` has `query(sql: string, params?: unknown[]): Promise<QueryResult>` and `end(): Promise<void>`
- [ ] In development mode (detected via `typeof Bun !== 'undefined'` and absence of WASI marker), `createPool()` delegates to a native Postgres driver; a test connects to a test Postgres instance, inserts a row, queries it, and asserts the result
- [ ] In Wasm mode (detected via WASI marker), `createPool()` delegates to the Domain 1 database proxy shim (US-112); a test using a mock shim backend asserts that `query("SELECT 1")` returns the expected result
- [ ] The `Pool` interface includes `getPoolSize(): number` and `getIdleCount(): number`; tests assert these reflect actual pool state
- [ ] Connection errors throw a `WarpGridDatabaseError` with the original error as `cause`
- [ ] Tests written before implementation (TDD)

### US-607: Implement `@warpgrid/bun-sdk/dns` for DNS resolution
**Milestone:** M6.3
**Depends on:** US-106, US-601
**Description:** As a Bun module author, I want DNS resolution available through `@warpgrid/bun-sdk/dns`, so that my handler can resolve hostnames in both development and Wasm-deployed modes.

**Acceptance Criteria:**
- [ ] `@warpgrid/bun-sdk/dns` exports `resolve(hostname: string, rrtype?: string): Promise<string[]>` supporting at least `A`, `AAAA`, and `CNAME` record types
- [ ] In development mode, `resolve()` delegates to Bun's native DNS; a test resolves `localhost` and asserts at least one result
- [ ] In Wasm mode, `resolve()` delegates to the Domain 1 DNS shim (US-106); a test using a mock shim asserts that `resolve("db.internal", "A")` returns the configured address
- [ ] Resolving a non-existent domain throws a `WarpGridDnsError` with `code: "ENOTFOUND"`
- [ ] A timeout parameter (`resolve(hostname, rrtype, { timeout: 5000 })`) is supported; a test asserts that exceeding the timeout throws with `code: "ETIMEOUT"`
- [ ] Tests written before implementation (TDD)

### US-608: Implement `@warpgrid/bun-sdk/fs` for filesystem access
**Milestone:** M6.3
**Depends on:** US-104, US-601
**Description:** As a Bun module author, I want filesystem operations available through `@warpgrid/bun-sdk/fs`, so that my handler can read and write files in both development and Wasm-deployed modes.

**Acceptance Criteria:**
- [ ] `@warpgrid/bun-sdk/fs` exports `readFile(path: string): Promise<Uint8Array>`, `readTextFile(path: string): Promise<string>`, `writeFile(path: string, data: Uint8Array | string): Promise<void>`, and `exists(path: string): Promise<boolean>`
- [ ] In development mode, these delegate to Bun's native file system APIs; a test writes a temp file, reads it back, asserts content equality, and cleans up
- [ ] In Wasm mode, these delegate to the Domain 1 filesystem shim (US-104); a test using a mock shim asserts a round-trip write/read cycle
- [ ] Path traversal outside the module's sandbox root (`../../etc/passwd`) throws a `WarpGridFsPermissionError`
- [ ] `readFile` on a non-existent path throws a `WarpGridFsNotFoundError` with the attempted path in the message
- [ ] Tests written before implementation (TDD)

### US-609: End-to-end test: Bun handler queries Postgres in both native and Wasm modes
**Milestone:** M6.3
**Depends on:** US-605, US-606
**Description:** As a platform engineer, I want a single test suite that exercises a Bun handler querying Postgres end-to-end in both development and Wasm-deployed modes, so that we can guarantee dual-mode parity for the most critical shim (database access).

**Acceptance Criteria:**
- [ ] A test fixture at `tests/fixtures/bun-postgres-handler/` implements a handler that accepts `POST /users` (inserts a user, returns 201) and `GET /users/:id` (queries by ID, returns 200 or 404)
- [ ] A test suite runs the handler in native Bun mode against a test Postgres instance: creates a user, fetches the user by ID, and asserts correct responses
- [ ] The same test suite compiles the handler with `warp pack --lang bun`, loads the `.wasm` into `WarpGridEngine` with the database proxy configured, and repeats the same create/fetch assertions
- [ ] The test asserts that response bodies are byte-identical between native and Wasm modes for the same input
- [ ] Tests written before implementation (TDD)

### US-610: Implement `warp dev --lang bun` local development with watch and hot-reload
**Milestone:** M6.4
**Depends on:** US-603, US-605
**Description:** As a Bun module author, I want `warp dev --lang bun` to start a local Wasmtime instance with file watching and hot-reload, so that I can see my code changes reflected in under two seconds during development.

**Acceptance Criteria:**
- [ ] `warp dev --lang bun` starts a local HTTP server backed by Wasmtime, compiles the handler on startup, and serves requests
- [ ] A file watcher detects changes to `.ts`, `.tsx`, `.js`, and `.jsx` files in the project; upon change, it re-runs the compilation pipeline and swaps the loaded Wasm module
- [ ] A test modifies a handler's response body from `"v1"` to `"v2"`, waits up to 2 seconds, sends a request, and asserts the response contains `"v2"`
- [ ] `warp dev --lang bun --native` skips Wasm compilation and runs the handler directly in Bun with the same file watcher; a test asserts this mode also reflects changes within 2 seconds
- [ ] Compilation errors are displayed in the terminal without crashing the dev server; the previous working module continues to serve requests
- [ ] Tests written before implementation (TDD)

### US-611: Implement `bun run --warpgrid` native development mode
**Milestone:** M6.4
**Depends on:** US-606, US-607, US-608
**Description:** As a Bun module author, I want `bun run --warpgrid` (via a Bun plugin or preload script) to run my handler locally with WarpGrid shims active, so that I can develop with Bun's native speed and debugger while using WarpGrid APIs.

**Acceptance Criteria:**
- [ ] A `@warpgrid/bun-sdk/preload.ts` script registers WarpGrid shims when loaded via `bun run --preload @warpgrid/bun-sdk/preload.ts`
- [ ] A convenience script entry in `@warpgrid/bun-sdk` package.json allows `bun run --warpgrid` as an alias (via `bunfig.toml` preload configuration)
- [ ] A test starts a handler using the preload script, sends an HTTP request, and asserts a correct response that exercises the database shim (via mock)
- [ ] The preload script sets a `WARPGRID_MODE=development` environment variable; a test asserts its presence
- [ ] Tests written before implementation (TDD)

### US-612: Test top 20 Bun-ecosystem packages for Wasm compatibility
**Milestone:** M6.5
**Depends on:** US-603, US-604
**Description:** As a platform engineer, I want automated compatibility testing of the 20 most popular Bun packages against the `warp pack --lang bun` pipeline, so that we can provide users with a clear compatibility table and catch regressions.

**Acceptance Criteria:**
- [ ] `compat-db/bun/packages.json` lists 20 packages (including at minimum: `hono`, `elysia`, `zod`, `drizzle-orm`, `jose`, `nanoid`, `date-fns`, `lodash-es`, `cheerio`, `superjson`) with versions pinned
- [ ] For each package, a test fixture imports the package in a minimal handler, runs `warp pack --lang bun`, and records the result (pass, fail-build, fail-runtime) to `compat-db/bun/results.json`
- [ ] A CI job runs these tests nightly and opens an issue if any previously passing package regresses
- [ ] Packages that fail due to native bindings are documented in `compat-db/bun/native-bindings.md` with the specific binding and suggested alternatives
- [ ] Tests written before implementation (TDD)

### US-613: Integrate Bun compatibility data into `warp-analyzer`
**Milestone:** M6.5
**Depends on:** US-612
**Description:** As a Bun module author, I want `warp analyze --lang bun` to scan my `package.json` and report which dependencies are Wasm-compatible, so that I can identify issues before attempting compilation.

**Acceptance Criteria:**
- [ ] `warp analyze --lang bun` reads `package.json` (and `bun.lockb` if present), cross-references each dependency against `compat-db/bun/results.json`, and prints a table with columns: package, version, status (compatible/incompatible/untested), and notes
- [ ] A test with a `package.json` containing `hono` (compatible) and a known-incompatible package asserts the output shows one green and one red entry
- [ ] A test with an empty `package.json` asserts the analyzer reports "No dependencies to check" and exits 0
- [ ] If any dependency is incompatible, the command exits with code 1 and prints a summary: "X of Y dependencies are incompatible"
- [ ] The `--json` flag outputs machine-readable results; a test parses the JSON and asserts the schema
- [ ] Tests written before implementation (TDD)

### US-614: Register `--lang bun` as first-class target in `warp pack`
**Milestone:** M6.6
**Depends on:** US-603, US-610
**Description:** As a platform engineer, I want `--lang bun` to be a first-class peer of `--lang rust`, `--lang go`, and `--lang typescript` in the `warp pack` CLI, so that Bun support is fully integrated and discoverable.

**Acceptance Criteria:**
- [ ] `warp pack --help` lists `bun` alongside `rust`, `go`, and `typescript` in the language options
- [ ] `warp pack` with no `--lang` flag and a `bunfig.toml` in the project root auto-detects Bun and uses the Bun pipeline; a test asserts this auto-detection
- [ ] `warp pack --lang bun` and `warp pack --lang typescript` produce different compilation pipelines; a documentation section in `docs/guides/bun-vs-typescript.md` explains the differences
- [ ] `scripts/test-bun.sh` runs the full Bun test suite (unit, integration, compilation, compatibility) and is wired into CI; a test confirms the script exits 0 on a clean checkout
- [ ] `compat-db/bun/` directory is included in the repository tree and referenced from the top-level README's supported-languages table
- [ ] Tests written before implementation (TDD)

### US-615: Create `warp init --template bun` project scaffolding
**Milestone:** M6.6
**Depends on:** US-601, US-602, US-614
**Description:** As a Bun module author, I want `warp init --template bun` to generate a ready-to-run WarpGrid Bun project, so that I can start building a Bun-based Wasm module in under five minutes.

**Acceptance Criteria:**
- [ ] `warp init --template bun` creates a project with `package.json` (depending on `@warpgrid/bun-sdk`), `bunfig.toml`, `tsconfig.json`, `src/index.ts` (implementing `WarpGridHandler` with a sample JSON endpoint), and `src/index.test.ts`
- [ ] `bun install && bun test` passes in the generated project; a CI test asserts this
- [ ] `warp pack --lang bun` in the generated project produces a valid `.wasm` component; a CI test asserts this
- [ ] The generated `src/index.ts` includes commented examples for database access, DNS resolution, and filesystem usage with import paths
- [ ] A `.warpgrid.toml` is generated with `lang = "bun"` so that `warp pack` auto-detects the language without `--lang`
- [ ] Tests written before implementation (TDD)

---

## Cross-Domain Integration Tests

### US-701: Docker Compose test dependency stack
**Milestone:** Integration
**Depends on:** none
**Description:** As a SDK developer, I want a Docker Compose configuration that provisions Postgres, Redis, and a mock service registry, so that all integration test applications have a repeatable, isolated set of backend dependencies.

**Acceptance Criteria:**
- [ ] `test-infra/docker-compose.yml` defines services: `postgres` (PostgreSQL 16, seeded with test schema), `redis` (Redis 7), `mock-registry` (lightweight HTTP service returning WarpGrid service discovery responses)
- [ ] A `test-infra/seed.sql` file creates a `test_users` table with 5 seed rows and a `test_analytics` table for T6 writes
- [ ] A health-check wait script (`test-infra/wait-for-deps.sh`) polls each service and exits 0 only when all are accepting connections, or exits 1 after a 60-second timeout
- [ ] Integration test: bring up the stack, run `wait-for-deps.sh`, execute a `SELECT 1` against Postgres and a `PING` against Redis, tear down
- [ ] All containers use `tmpfs` for data directories so tests start from a clean state every run
- [ ] Tests written before implementation (TDD)

### US-702: Rust HTTP + Postgres integration test (T1)
**Milestone:** Integration
**Depends on:** US-701, US-122, US-112, US-106
**Description:** As a SDK developer, I want an end-to-end test that compiles a Rust axum HTTP handler to WASI, runs it in WarpGrid's engine with DNS and database proxy shims, and verifies a full HTTP-to-Postgres request cycle, so that Domain 1 and Domain 2 are validated together on the Rust compilation path.

**Acceptance Criteria:**
- [ ] `test-apps/t1-rust-http-postgres/` contains a Rust axum application that: accepts `GET /users`, resolves `db.test.warp.local` via DNS shim, queries `SELECT * FROM test_users` through the database proxy, and returns the rows as JSON
- [ ] The application compiles to `wasm32-wasip2` using the patched wasi-libc sysroot
- [ ] A test harness instantiates the compiled module in `WarpGridEngine` with filesystem, DNS, and database-proxy shims enabled
- [ ] Test: `GET /users` returns 200 with all 5 seed users as JSON
- [ ] Test: `POST /users` with JSON body returns 201; subsequent `GET /users` includes the new user
- [ ] Test: DNS shim received a resolve call for `db.test.warp.local`
- [ ] Test: if database proxy is unreachable, the module returns 503 with error message
- [ ] Tests written before implementation (TDD)

### US-703: Rust HTTP + Redis + Postgres integration test (T2)
**Milestone:** Integration
**Depends on:** US-702, US-116
**Description:** As a SDK developer, I want an end-to-end test that extends T1 by adding a Redis caching layer with two simultaneous proxied connections from a single Wasm module, so that multi-protocol database proxy support is validated within a single module instance.

**Acceptance Criteria:**
- [ ] `test-apps/t2-rust-http-redis-postgres/` contains a Rust application that: on `GET /users/:id`, checks Redis cache first, on miss queries Postgres and caches the result with 30s TTL
- [ ] The test harness configures the database proxy shim with entries for both Postgres and Redis protocols
- [ ] Test (cold cache): request returns correct user, Postgres queried, Redis SET called
- [ ] Test (warm cache): same request returns cached value, Postgres NOT queried
- [ ] Test (TTL expiry): after cache flush, Postgres re-queried
- [ ] Test: connection pool metrics show exactly 1 Postgres and 1 Redis connection
- [ ] Tests written before implementation (TDD)

### US-704: Go HTTP + Postgres integration test (T3)
**Milestone:** Integration
**Depends on:** US-701, US-311, US-307
**Description:** As a SDK developer, I want an end-to-end test that compiles a Go net/http handler with patched TinyGo, runs it in WarpGrid's engine, and verifies HTTP-to-Postgres connectivity, so that Domain 3 is validated working with Domains 1 and 2.

**Acceptance Criteria:**
- [ ] `test-apps/t3-go-http-postgres/` contains a Go application using `net/http` with `pgx`
- [ ] The application compiles using `warp-tinygo` targeting `wasip2`
- [ ] Test: `GET /users` returns 200 with all seed users
- [ ] Test: `POST /users` with JSON returns 201; subsequent `GET /users` includes new user
- [ ] Test: `net.Dial("tcp", ...)` was intercepted and routed through database proxy
- [ ] Test: invalid database host returns 503 with meaningful error
- [ ] Tests written before implementation (TDD)

### US-705: TypeScript (Node.js) HTTP + Postgres integration test (T4)
**Milestone:** Integration
**Depends on:** US-701, US-405, US-410
**Description:** As a SDK developer, I want an end-to-end test that componentizes a TypeScript hono handler via patched ComponentizeJS, runs it in WarpGrid's engine, and verifies HTTP-to-Postgres connectivity, so that Domain 4 is validated with Domain 1.

**Acceptance Criteria:**
- [ ] `test-apps/t4-ts-http-postgres/` contains a TypeScript application using `hono` router and `warpgrid/pg`
- [ ] The application compiles via `warp pack --lang js` producing a Wasm component
- [ ] Test: `GET /users` returns 200 with seed users
- [ ] Test: `POST /users` returns 201; `GET /users` reflects the new row
- [ ] Test: `warpgrid.database.connect()` was invoked (not raw TCP)
- [ ] Test: `process.env.APP_NAME` is accessible and included in response headers
- [ ] Tests written before implementation (TDD)

### US-706: Bun HTTP + Postgres integration test (T5)
**Milestone:** Integration
**Depends on:** US-701, US-609, US-614
**Description:** As a SDK developer, I want an end-to-end test that compiles a Bun handler via `warp pack --lang bun`, runs it in WarpGrid's engine, and verifies HTTP-to-Postgres connectivity with behavioral parity to T4, so that Domain 6 is validated with Domain 1.

**Acceptance Criteria:**
- [ ] `test-apps/t5-bun-http-postgres/` contains a Bun TypeScript application using `hono` and `@warpgrid/bun-sdk/postgres`
- [ ] The application compiles via `warp pack --lang bun` producing a Wasm component
- [ ] Test: `GET /users` returns 200 with seed users
- [ ] Test: `POST /users` returns 201; `GET /users` reflects the new row
- [ ] Test: response body is byte-for-byte identical to T4 for the same request (behavioral parity)
- [ ] Test: Bun polyfills work — `Bun.env` resolves WASI env vars, `Bun.sleep()` uses WASI clocks
- [ ] Tests written before implementation (TDD)

### US-707: Multi-service polyglot integration test (T6)
**Milestone:** Integration
**Depends on:** US-702, US-704, US-705, US-706
**Description:** As a SDK developer, I want an end-to-end test running four Wasm modules (Rust gateway, Go user service, TS notification service, Bun analytics service) within a single WarpGrid engine instance with inter-service DNS routing, so that cross-language interoperability is validated as a complete system.

**Acceptance Criteria:**
- [ ] `test-apps/t6-multi-service/` contains four sub-applications: `gateway/` (Rust axum), `user-svc/` (Go), `notification-svc/` (TypeScript), `analytics-svc/` (Bun)
- [ ] Each compiles to a separate Wasm component using its respective `warp pack --lang` pipeline
- [ ] DNS shim routes `user-svc.test.warp.local`, `notification-svc.test.warp.local`, `analytics-svc.test.warp.local` to correct internal endpoints
- [ ] Full flow test: POST /users -> 201, POST /notify -> 202 (enqueues to Redis), POST /analytics/event -> 201 (inserts to Postgres), GET /users -> includes new user
- [ ] Test: `test_analytics` table contains the event row, Redis list consumed the notification
- [ ] Test: if one service errors, gateway returns error with `X-WarpGrid-Source-Service` header identifying which downstream failed
- [ ] Tests written before implementation (TDD)

### US-708: `test-all.sh` orchestration script
**Milestone:** Integration
**Depends on:** US-701, US-702, US-703, US-704, US-705, US-706, US-707
**Description:** As a SDK developer, I want a single shell script that orchestrates the full integration test lifecycle (start deps, build apps, run tests, collect results, tear down), so that the entire cross-domain test suite can be executed with one command.

**Acceptance Criteria:**
- [ ] `scripts/test-all.sh` validates prerequisites, starts Docker Compose, waits for deps, builds T1-T6 (parallelized where independent), runs test suites, prints summary table, tears down
- [ ] Accepts flags: `--only t1,t3` to run a subset, `--keep-deps` to skip teardown, `--verbose` for full output
- [ ] Captures build and test output to `test-results/` directory with timestamped logs per test app
- [ ] If a build fails, dependent tests are skipped (not errored) with `SKIP (build failed)` status
- [ ] `--dry-run` mode prints the execution plan without running anything
- [ ] Tests written before implementation (TDD)

### US-709: CI pipeline integration for cross-domain tests
**Milestone:** Integration
**Depends on:** US-708
**Description:** As a SDK developer, I want the cross-domain integration test suite to run automatically on every pull request and on merges to `main`, so that regressions across domain boundaries are caught before code lands.

**Acceptance Criteria:**
- [ ] `.github/workflows/integration-tests.yml` triggers on PRs targeting `main` and pushes to `main`
- [ ] Each test app (T1-T6) runs as a separate matrix job for parallel execution, plus a `summary` job requiring all test jobs
- [ ] Each job: checks out repo, sets up toolchains (cached), starts Docker services, builds test app, runs test suite, uploads `test-results/` as workflow artifact
- [ ] Test jobs have a 15-minute timeout; build caching uses `actions/cache`
- [ ] A nightly schedule (`cron: '0 3 * * *'`) runs `test-all.sh` on `main` to catch flaky regressions
- [ ] Tests written before implementation (TDD)

### US-710: Cross-domain performance baseline
**Milestone:** Integration
**Depends on:** US-708
**Description:** As a SDK developer, I want each integration test to collect latency and throughput metrics establishing a performance baseline, so that performance regressions introduced by changes in any domain are detectable.

**Acceptance Criteria:**
- [ ] Each test app (T1-T6) records per-request metrics: total latency (ms), DNS resolution time (ms), database proxy round-trip time (ms), response body size (bytes)
- [ ] A `test-infra/bench-harness/` sends 100 sequential and 100 concurrent requests per test app and collects p50, p95, p99 latency
- [ ] Results written to `test-results/performance-baseline.json` with structured per-app metrics
- [ ] Quality gate: shim overhead must not exceed 10% compared to direct (non-shimmed) Postgres connection
- [ ] `scripts/compare-perf.sh` diffs current run against last `main` baseline and prints delta table
- [ ] If any test app's p95 latency regresses by >20%, the script exits with a warning (non-blocking in CI, but visible)
- [ ] Tests written before implementation (TDD)

---

## Functional Requirements

- FR-1: The SDK must provide Wasmtime host functions for filesystem, DNS, signals, database-proxy, and threading shims defined via WIT interfaces
- FR-2: The filesystem shim must intercept reads to `/dev/null`, `/dev/urandom`, `/etc/resolv.conf`, `/etc/hosts`, `/proc/self/`, and `/usr/share/zoneinfo/**` and return WarpGrid-controlled content
- FR-3: The DNS shim must resolve hostnames through a chain: WarpGrid service registry -> `/etc/hosts` -> host system DNS, with TTL caching and round-robin
- FR-4: The database proxy shim must pool connections per `(host, port, database, user)` tuple and support Postgres, MySQL, and Redis wire protocols as byte passthrough
- FR-5: All shims must be individually enableable/disableable via `warp.toml` configuration with zero overhead for disabled shims
- FR-6: wasi-libc patches must maintain backward compatibility: programs compiled against patched sysroot must still work in vanilla Wasmtime (graceful degradation via weak symbols)
- FR-7: Patches must be maintained as numbered `git format-patch` files with rebase scripts for upstream updates
- FR-8: TinyGo patches must enable `net.Dial("tcp", ...)`, `net/http` handler registration, and `database/sql` + `pgx` for the wasip2 target
- FR-9: ComponentizeJS extensions must inject `warpgrid.database.connect()`, `warpgrid.dns.resolve()`, `warpgrid.fs.readFile()`, and `process.env` into the JS runtime
- FR-10: WASI 0.3 async integration must provide a stable `AsyncHandler` trait that insulates module authors from upstream prototyping-branch churn
- FR-11: Bun support must provide dual-mode SDKs (native for dev, Wasm for deploy) with zero code changes between modes
- FR-12: `warp pack` must support `--lang rust`, `--lang go`, `--lang js`, and `--lang bun` as first-class compilation targets
- FR-13: Cross-domain integration tests must validate all four language runtimes working together in a multi-service scenario

## Non-Goals (Out of Scope)

- Python workload support (Phase 2+)
- Real thread support (`os/exec`, fork/exec) — fundamentally impossible in WASI
- Forking Wasmtime itself — use public embedding API only
- Rewriting upstream libraries — patch minimally
- Custom color schemes, themes, or UI for CLI tools
- Performance optimization beyond 10% overhead ceiling for shims
- Production-grade connection pool failover (basic health checking only in Phase 1)

## Technical Considerations

- **Wasmtime builds take 5-10 minutes** — aggressive caching required in CI
- **TinyGo requires LLVM 17+** — LLVM toolchain must be cached in CI
- **wasi-libc uses musl** — patches target musl's WASI bottom-half
- **ComponentizeJS uses StarlingMonkey** (SpiderMonkey fork) — JS runtime extension patches target this engine
- **Bun uses JavaScriptCore** — separate from ComponentizeJS's engine, requiring distinct polyfill approach
- **WASI 0.3 is unstable** — the `warpgrid-async` crate must absorb API churn
- **All forks must be rebasing-friendly** — `git format-patch` files, not large divergent branches
- **`jco`** (`@bytecodealliance/jco`) is required for both Domain 4 and Domain 6
- **Rust nightly** may be needed for some Wasmtime features

## Dependency Graph Summary

```
Phase A (Foundation):
  US-101 → US-102 → US-103 → US-104 → US-105
  US-102 → US-106 → US-107 → US-108
  US-102 → US-109 → US-110
  US-201 → US-202 (parallel with D1)
  US-501 → US-502 (parallel with D1)

Phase B (Database Proxy):
  US-102 → US-111 → US-112 → US-113 → US-114
  US-112 → US-115, US-116 → US-117
  US-201,US-202,US-106 → US-203 → US-204 → US-205
  US-201,US-202,US-104 → US-206 → US-207 → US-208

Phase C (Socket Integration):
  US-201,US-202,US-112 → US-209 → US-210 → US-211 → US-212
  US-102 → US-118 → US-119
  US-101 → US-120 → US-121 → US-122

Phase D (Language Support):
  US-301 → US-302, US-303 → US-304 → US-305
  US-301,US-102 → US-306 → US-307
  US-305,US-307,US-310 → US-311
  US-401 → US-402, US-403 → US-404 → US-405
  US-401,US-106,US-104 → US-406, US-407 → US-408 → US-409
  US-601 → US-602 → US-603 → US-604 → US-605
  US-601,US-112 → US-606, US-607, US-608 → US-609

Phase E (Async + Polish + Integration):
  US-503 → US-504 → US-505 → US-506 → US-507
  US-508 → US-509 → US-510
  US-610, US-611, US-612 → US-613, US-614, US-615
  US-701 → US-702 → US-703
  US-701,US-311,US-307 → US-704
  US-701,US-405,US-410 → US-705
  US-701,US-609,US-614 → US-706
  US-702,US-704,US-705,US-706 → US-707 → US-708 → US-709, US-710
```

## Success Metrics

- All 85 user stories pass their acceptance criteria
- All integration tests (T1-T6) pass end-to-end
- CI is green for all 6 domains plus integration tests
- Rebase scripts apply patches cleanly to latest upstream for all forked components
- Performance baseline: shim overhead < 10% vs direct connections
- Compatibility matrices: TinyGo stdlib 15/20 pass, npm packages 12/20 pass, Bun packages 14/20 pass

## Open Questions

- What is the exact latest stable tag for wasi-libc, TinyGo, ComponentizeJS, and Bun to pin against?
- Should `warp pack` be a standalone binary or a subcommand of a `warp` CLI? (Assumed subcommand in this PRD)
- Should the database proxy support connection-level authentication (user/password forwarding) or only service-level auth configured at the host?
- How should the DNS shim handle mDNS-style `.local` domains that conflict with WarpGrid's `.warp.local` convention?
- Should WASI 0.3 async support be opt-in per module or automatic based on WIT detection? (Assumed automatic in US-504)
[/PRD]
