# Ralph Progress Log

This file tracks progress across iterations. Agents update this file
after each iteration and it's included in prompts for context.

## Codebase Patterns (Study These First)

### WIT Binding Structure
- WIT files live in `crates/warpgrid-host/wit/` with package `warpgrid:shim@0.1.0`
- All interfaces (filesystem, dns, signals, database-proxy, threading) share the same package
- `wasmtime::component::bindgen!` generates types under `bindings::warpgrid::shim::<interface>`
- Each interface produces a `Host` trait with methods matching WIT function signatures
- The world type is `WarpgridShims` (generated from `warpgrid-shims` world name)
- WIT `result<_, string>` maps to Rust `Result<(), String>` in Host traits
- WIT `type connection-handle = u64` maps to plain `u64` in Rust (type alias)
- WIT `option<T>` maps to Rust `Option<T>` (e.g., `password: option<string>` → `Option<String>`)

### Wasmtime Version & Features
- Wasmtime 41 with `component-model`, `async`, `component-model-async` features
- `wasmtime-wasi` v41 with `p3` feature (WASI preview 3 support)
- Rust edition 2024

### VirtualFileMap Architecture
- `VirtualFileMap` uses builder pattern → immutable after construction (no `&mut self` methods)
- Content providers are an enum: `DevNull`, `DevUrandom`, `StaticContent(Arc<[u8]>)`, `PrefixMapped(Arc<HashMap<String, Arc<[u8]>>>)`
- Two lookup strategies: exact path match, then prefix match with sub-path dispatch
- Path canonicalization (`..`, `.`, double slashes) runs before every lookup to prevent traversal bypass
- `getrandom` crate for `/dev/urandom` (platform-agnostic crypto random)
- `VirtualContent` enum distinguishes `DevNull` (absorbs writes), `DevUrandom` (generate random per-read), and `Found(vec![])` (just empty)

### DnsResolver Architecture
- `DnsResolver` holds immutable service registry (`HashMap<String, Vec<IpAddr>>`) and parsed `EtcHosts`
- 3-tier resolution chain: service registry → `/etc/hosts` → `tokio::net::lookup_host`
- Resolution stops at the first chain link with results (no unnecessary downstream lookups)
- All lookups are case-insensitive (lowercased at query time)
- `EtcHosts::parse()` handles standard `/etc/hosts` format (comments, multiple hostnames, inline `#`)
- `DnsHost` bridges sync WIT `Host` trait to async resolver via `tokio::task::block_in_place` + `handle.block_on()`
- Host tests require `#[tokio::test(flavor = "multi_thread")]` because `block_in_place` needs multi-thread runtime

### ConnectionPoolManager Architecture
- `ConnectionPoolManager` uses `tokio::sync::Mutex` for interior mutability (async-compatible)
- Pools keyed by `PoolKey { host, port, database, user }` — each tuple gets an independent bounded pool
- `tokio::sync::Semaphore` bounds total connections per pool key (checked-out + idle ≤ max_size)
- `Semaphore::acquire_owned` + `tokio::time::timeout` implements the "block with timeout" requirement
- `permit.forget()` keeps the semaphore slot consumed while a connection is checked out; `add_permits(1)` returns it on release/reap
- `ConnectionBackend` trait abstracts the transport for testability — US-112/115/116 implement with real TCP/TLS
- `ConnectionFactory` trait for dependency injection — mock factories in tests, real factories in production
- Handle table: `HashMap<u64, PooledConnection>` in `checked_out` mutex, monotonically increasing IDs starting at 1
- Idle connections stored as `Vec<PooledConnection>` per pool key — `pop()` for LIFO reuse (most recently used connection is healthiest)
- `DbProxyHost` bridges sync WIT Host trait to async pool manager via `block_in_place` + `handle.block_on()` (same pattern as `DnsHost`)

### TcpBackend Architecture (Wire Protocol Passthrough)
- `TcpBackend` wraps a `Transport` enum: `Plain(TcpStream)` or `Tls(Box<StreamOwned<ClientConnection, TcpStream>>)`
- Uses `std::net::TcpStream` (blocking I/O), not `tokio::net::TcpStream` — compatible with sync `ConnectionBackend` trait
- `TcpConnectionFactory` implements `ConnectionFactory` — creates plain TCP or TLS connections
- Recv timeout set via `TcpStream::set_read_timeout()` which propagates through TLS layer
- `TCP_NODELAY` enabled for low-latency wire protocol exchange
- `TlsConfig` with `with_system_roots()` (Mozilla CA via `webpki_roots`) and `dangerous_no_verify()` (test only)
- Must use `ClientConfig::builder_with_provider(ring::default_provider().into())` — plain `builder()` panics when both ring and aws-lc-rs are in dep tree
- Ping uses `TcpStream::peek` with short timeout: `Ok(0)` = closed, `Err(TimedOut)` = alive
- Factory ignores `password` param — guest sends auth via wire protocol (pure byte passthrough)

### FilesystemHost Architecture
- `FilesystemHost` implements `bindings::warpgrid::shim::filesystem::Host` trait
- Lives in `filesystem/host.rs` submodule (data layer in `filesystem.rs`, behavior layer in `filesystem/host.rs`)
- Handle table: `HashMap<u64, OpenVirtualFile>` with monotonically increasing handle IDs starting at 1
- `OpenFileKind` enum: `Regular` (buffered + cursor), `DevNull` (always empty), `DevUrandom` (fresh random per-read)
- Non-matching paths return `Err("not a virtual path: {path}")` — signals guest to fall through to WASI FS
- All intercept decisions logged at `tracing::debug` level
- Rust 2018+ submodule pattern: `filesystem.rs` + `filesystem/host.rs` (not `filesystem/mod.rs`)

### wasi-libc Build Pipeline (Domain 2)
- wasi-libc has migrated from `make` to `cmake` as of wasi-sdk-30 — PRD references to `make -j$(nproc) THREAD_MODEL=single` are outdated
- Build requires wasi-sdk (provides Clang with WASM target support) — auto-downloaded by `scripts/build-libc.sh`
- `UPSTREAM_REF` file pins both TAG and COMMIT for reproducibility (full 40-char SHA)
- CMake key variables: `TARGET_TRIPLE` (default: `wasm32-wasip2`), `BUILD_TESTS=OFF`, `BUILD_SHARED=OFF`
- Thread model is inferred from target triple (no explicit `THREAD_MODEL` var in CMake build)
- wasi-sdk-30 provides native arm64-macos builds — no Rosetta needed
- Sysroot output at `${CMAKE_BUILD_DIR}/sysroot/` with `lib/${TARGET_TRIPLE}/libc.a`
- Stock and patched builds use separate source checkouts (`build/src-stock/`, `build/src-patched/`) for clean isolation
- Patches applied via `git am --3way` on the patched source checkout
- macOS lacks GNU `timeout` — use `perl -e "alarm N; exec @ARGV"` as portable fallback

### Weak Symbols in Wasm Static Archives
- wasm-ld treats weak references differently than ELF linkers in static archives
- A weak `extern` reference (`extern __attribute__((weak))`) in one .a member will NOT pull a weak definition from another .a member
- The linker creates an `undefined_weak` trap stub instead — calling it executes `unreachable`
- Fix: make the `extern` declaration strong (normal `extern`), keep only the definition as `__attribute__((weak))`
- Strong reference (`U` in nm) forces wasm-ld to pull the archive member containing the weak definition (`W` in nm)
- The weak definition can still be overridden at link time by a strong definition from another object

### Patch Maintenance Workflow (rebase-libc.sh)
- Patches stored as numbered `git format-patch` files in `libc-patches/*.patch`
- Apply with `git am --3way` for 3-way merge conflict resolution
- `--apply`: reset checkout to UPSTREAM_REF, apply patches sequentially, stop on first failure
- `--export`: `git format-patch <upstream>..HEAD` regenerates patches from branch
- `--update <tag>`: clone new upstream, apply patches, update UPSTREAM_REF on success
- `--validate`: checks numbering order and known dependency constraints (0002→0001, 0005→0004, etc.)
- macOS bash 3.2 compatibility: no `declare -A`, use `case` or string matching
- `git clone --depth 50` (not --depth 1) needed for `git am` 3-way merge to work

---

## 2026-02-22 - warpgrid-agm.2
- Implemented WIT interface definitions for all 5 shim domains
- Created `src/bindings.rs` with `wasmtime::component::bindgen!` macro
- Files changed:
  - NEW: `crates/warpgrid-host/wit/filesystem.wit` — virtual file open/read/stat/close
  - NEW: `crates/warpgrid-host/wit/dns.wit` — resolve-address returning IP records
  - NEW: `crates/warpgrid-host/wit/signals.wit` — on-signal/poll-signal with signal-type enum
  - NEW: `crates/warpgrid-host/wit/database-proxy.wit` — connect/send/recv/close on connection-handle
  - NEW: `crates/warpgrid-host/wit/threading.wit` — declare-threading-model with model enum
  - NEW: `crates/warpgrid-host/wit/world.wit` — warpgrid-shims world importing all interfaces
  - NEW: `crates/warpgrid-host/src/bindings.rs` — bindgen macro + 12 tests
  - MOD: `crates/warpgrid-host/src/lib.rs` — added `pub mod bindings`
- **Learnings:**
  - `component::bindgen!` with `path: "wit"` resolves relative to the crate's Cargo.toml dir
  - WIT `type X = u64` becomes a transparent `u64` in Rust (not a newtype wrapper)
  - Each WIT file in the same directory must repeat the `package` declaration
  - Host traits are sync by default; async requires `async: true` in bindgen options (for US-506+)
  - Generated module path mirrors WIT package: `warpgrid::shim::<interface>::Host`
---

## 2026-02-22 - warpgrid-agm.3
- Implemented `VirtualFileMap` with builder pattern and content provider enum
- 39 new tests covering all virtual paths, canonicalization, and edge cases (51 total in crate)
- Files changed:
  - MOD: `crates/warpgrid-host/Cargo.toml` — added `getrandom = "0.2"` dependency
  - MOD: `crates/warpgrid-host/src/filesystem.rs` — full implementation (~450 lines)
    - `VirtualFileMap` struct with `lookup()` and `contains()` methods
    - `VirtualFileMapBuilder` with fluent builder API
    - `ContentProvider` enum: DevNull, DevUrandom, StaticContent, PrefixMapped
    - `VirtualContent` enum: Found, DevNull, NotFound
    - Path canonicalization preventing `..`/`.` traversal attacks
    - `with_defaults()` constructor for standard WarpGrid paths
- **Learnings:**
  - `Arc<[u8]>` (not `Arc<Vec<u8>>`) is idiomatic for shared immutable byte buffers — avoids double indirection
  - Path canonicalization must handle `..` beyond root (pop on empty vec is a no-op) to prevent escape
  - `PrefixMapped` provider needs careful sub-path extraction: `strip_prefix` gives the remainder after the prefix including nested paths like `US/Eastern`
  - `getrandom` crate works transparently on all platforms including WASI — better than `rand` for this use case
  - `VirtualContent::DevNull` as a distinct variant (not `Found(vec![])`) is important because US-104 needs to know the path accepts writes too
---

## 2026-02-22 - warpgrid-agm.4
- Implemented `FilesystemHost` — host-side filesystem intercept with handle table and cursor management
- Added `DevUrandom` variant to `VirtualContent` enum (parallel to `DevNull`)
- 31 new tests (82 total in crate), all quality gates pass
- Files changed:
  - NEW: `crates/warpgrid-host/src/filesystem/host.rs` — `FilesystemHost` struct + `Host` trait impl + 31 tests
  - MOD: `crates/warpgrid-host/src/filesystem.rs` — added `pub mod host;`, added `VirtualContent::DevUrandom`, updated `read_provider` and urandom tests
- **Learnings:**
  - `/dev/urandom` needs a distinct `VirtualContent` variant because unlike regular files, it generates fresh random bytes per-read (not buffered at open time). `DevNull` set the precedent for this pattern.
  - Rust 2018+ submodule pattern works in edition 2024: `filesystem.rs` coexists with `filesystem/host.rs` directory — no need to rename to `mod.rs`
  - `OpenFileKind` enum dispatch in `read_virtual()` cleanly separates the three behaviors (buffered cursor, always-empty, fresh-random) without needing `if-else` chains on path strings
  - Handle IDs start at 1 (not 0) so 0 can serve as a sentinel "invalid handle" value in guest code
  - Path canonicalization happens transparently via `VirtualFileMap::lookup()` — `FilesystemHost` doesn't need its own canonicalization logic
---

## 2026-02-22 - warpgrid-agm.6
- Implemented `DnsResolver` with 3-tier resolution chain: service registry → `/etc/hosts` → system DNS
- Implemented `EtcHosts` parser for `/etc/hosts` format content
- Implemented `DnsHost` WIT host trait bridging sync `Host` trait to async resolver
- 32 new tests (114 total in crate), all quality gates pass
- Files changed:
  - MOD: `crates/warpgrid-host/src/dns.rs` — full implementation (~190 lines): `EtcHosts` parser, `DnsResolver` struct, 23 tests
  - NEW: `crates/warpgrid-host/src/dns/host.rs` — `DnsHost` struct + `Host` trait impl + 9 tests
- **Learnings:**
  - WIT `Host` trait is synchronous but `tokio::net::lookup_host` is async. The bridge pattern is `tokio::task::block_in_place(|| handle.block_on(future))` — `block_in_place` tells the multi-threaded scheduler to temporarily yield other tasks while blocking
  - `block_in_place` requires `tokio::test(flavor = "multi_thread")` — the default current-thread runtime doesn't support it. This is fine since Wasmtime uses multi-threaded runtime in production
  - `let-chains` syntax (`if let Some(x) = expr && condition { ... }`) is stable in Rust edition 2024 and required by clippy's `collapsible_if` lint — eliminates nested `if let` + `if`
  - Case-insensitive DNS: lowercasing both the query and the registry keys on insertion would be more efficient, but lowercasing at query time keeps the registry immutable and avoids assumptions about caller behavior
  - `/etc/hosts` parsing follows same pattern as real systems: skip `#` comments, split on whitespace, first token is IP, rest are hostnames. Inline comments after hostnames need explicit `#` break
  - The submodule pattern (`dns.rs` + `dns/host.rs`) mirrors `filesystem.rs` + `filesystem/host.rs` perfectly — data layer in parent, behavior layer in child
---

## 2026-02-22 - warpgrid-agm.11
- Implemented `ConnectionPoolManager` — host-side connection pool keyed by `(host, port, database, user)` tuple
- Implemented `DbProxyHost` — WIT `database-proxy` Host trait bridging sync interface to async pool manager
- 46 new tests (160 total in crate), all quality gates pass
- Files changed:
  - MOD: `crates/warpgrid-host/Cargo.toml` — added `sync` feature to tokio
  - MOD: `crates/warpgrid-host/src/db_proxy.rs` — full implementation (~450 lines): `PoolKey`, `PoolConfig`, `PooledConnection`, `ConnectionBackend` trait, `ConnectionFactory` trait, `ConnectionPoolManager`, `PoolStats`, 34 tests
  - NEW: `crates/warpgrid-host/src/db_proxy/host.rs` — `DbProxyHost` struct + `Host` trait impl + 12 tests
- **Learnings:**
  - `tokio::sync::Semaphore` is the right primitive for bounding connection pools — `acquire_owned` returns an `OwnedSemaphorePermit` that can be `forget()`-ed to keep the slot consumed while a connection is checked out, and `add_permits(1)` returns it on release
  - LIFO idle connection reuse (Vec::pop) is better than FIFO for connection pools — the most recently returned connection is most likely still alive, reducing health check failures
  - `tokio::sync::Mutex` (not `std::sync::Mutex`) is required because lock guards must be held across `.await` points in async methods like `checkout()`
  - The `ConnectionBackend` + `ConnectionFactory` trait pair enables full unit testing without real TCP connections — US-112/115/116 will implement these with actual protocol handling
  - `block_in_place` + `handle.block_on()` pattern from `DnsHost` works identically for `DbProxyHost` — this is becoming a standard WarpGrid sync-to-async bridge pattern
  - Pool `total_count` must be tracked separately from `idle.len() + checked_out.count()` because connections can be destroyed during health checks, and the count needs to stay consistent with semaphore permits
---

## 2026-02-22 - warpgrid-agm.12
- Implemented `TcpBackend` — real TCP/TLS connection backend for database proxy wire protocol passthrough
- Implemented `TcpConnectionFactory` — factory creating plain TCP or TLS-wrapped connections
- Added `TlsConfig` with `with_system_roots()` (Mozilla CA store) and `dangerous_no_verify()` (test-only)
- Added `recv_timeout` to `PoolConfig` (default: 30s, configurable)
- 20 new tests (180 total in crate), all quality gates pass
- Files changed:
  - MOD: `crates/warpgrid-host/Cargo.toml` — added `rustls = "0.23"`, `webpki-roots = "0.26"`, `rcgen = "0.13"` (dev)
  - NEW: `crates/warpgrid-host/src/db_proxy/tcp.rs` — full implementation (~400 lines): `TcpBackend`, `Transport` enum, `TlsConfig`, `TcpConnectionFactory`, `danger::NoVerifier`, 20 tests
  - MOD: `crates/warpgrid-host/src/db_proxy.rs` — added `pub mod tcp;`, added `recv_timeout` to `PoolConfig` + Default + test_config
- **Learnings:**
  - `rustls` 0.23 with both `ring` and `aws-lc-rs` in the dep tree requires explicit `CryptoProvider`: use `ClientConfig::builder_with_provider(ring::default_provider().into())` instead of `ClientConfig::builder()` — the latter panics at runtime with "Could not automatically determine the process-level CryptoProvider"
  - `std::net::TcpStream` (blocking) is the right choice for `ConnectionBackend` sync trait — avoids mixing async/sync and `set_read_timeout` naturally provides the recv timeout
  - `TCP_NODELAY` (Nagle off) is critical for wire protocol latency — database protocols are request-response, and Nagle would delay small writes by up to 40ms
  - `TcpStream::peek` with a short read timeout is a reliable health check: `Ok(0)` = closed, `Err(TimedOut/WouldBlock)` = alive, `Ok(n)` = data pending
  - `"localhost".to_socket_addrs()` on macOS resolves to `::1` (IPv6) first — tests with listeners on `127.0.0.1` must use `"127.0.0.1"` in PoolKey to avoid connection refused
  - `rcgen::KeyPair::generate()` + `CertificateParams::self_signed()` is the cleanest way to generate test TLS certs — no hardcoded PEM needed
  - `rustls::StreamOwned<ClientConnection, TcpStream>` wraps the sync TcpStream transparently — read timeout on the underlying TcpStream propagates through the TLS layer
  - The password parameter in `ConnectionFactory::connect()` is intentionally ignored for TCP passthrough — the guest sends auth via the wire protocol (e.g., Postgres StartupMessage)
---

## 2026-02-22 - warpgrid-agm.23
- Implemented US-201: Fork wasi-libc and establish patched build pipeline
- Pinned wasi-libc to `wasi-sdk-30` tag (commit `ec0effd769df5f05b647216578bcf5d3b5449725`)
- Both stock and patched sysroots build successfully and produce `libc.a`
- Minimal C test program compiles and links against both sysroots, runs in Wasmtime
- Files changed:
  - NEW: `libc-patches/UPSTREAM_REF` — pinned upstream commit hash and tag
  - NEW: `libc-patches/tests/hello.c` — minimal C test program validating sysroot linkability
  - NEW: `scripts/build-libc.sh` — builds stock/patched wasi-libc sysroots (~190 lines)
  - NEW: `scripts/test-libc.sh` — test harness compiling and running C tests against sysroots (~180 lines)
  - MOD: `.github/workflows/ci.yml` — added `wasi-libc` CI job with caching
  - MOD: `.gitignore` — added `/build` directory
- **Learnings:**
  - wasi-libc migrated from Make to CMake between wasi-sdk-27 and wasi-sdk-30 — the PRD's `make -j$(nproc) THREAD_MODEL=single` is outdated. Use `cmake` with `-DTARGET_TRIPLE=wasm32-wasip2` instead
  - wasi-sdk-30 ships native arm64-macos binaries (previously required Rosetta for M-series Macs)
  - CMake build produces sysroot at `${CMAKE_BUILD_DIR}/sysroot/` — copy to `build/sysroot-{stock,patched}/` for our layout
  - macOS `sed` has incompatible `-n` address range syntax with GNU sed — avoid sed-based help text extraction in scripts
  - macOS lacks GNU `timeout` command — portable fallback: `perl -e "alarm N; exec @ARGV" -- command args`
  - Wasmtime 20 (user's local version) supports both wasip1 and wasip2 component model — `--wasm component-model=y` flag needed for wasip2 targets
  - Build idempotency: cmake only rebuilds changed targets on re-runs, but source checkout is skipped entirely if commit matches UPSTREAM_REF
  - wasi-libc CMake requires BUILD_SHARED=OFF for our use case (we only need static libc.a for Wasm linking)
---

## 2026-02-22 - warpgrid-agm.24
- Implemented US-202: Create patch maintenance and rebase tooling
- Created `scripts/rebase-libc.sh` with four modes: `--apply`, `--export`, `--update`, `--validate`
- Validated full apply/export roundtrip with a test patch (created, exported, re-applied, verified)
- Confirmed `test-libc.sh` (from US-201) already satisfies the test harness requirements
- Files changed:
  - NEW: `scripts/rebase-libc.sh` — patch maintenance script (~350 lines) with:
    - `--apply`: applies `libc-patches/*.patch` onto pinned upstream via `git am --3way`, reports conflicting file names on failure
    - `--export`: regenerates patch files from commits on top of upstream base via `git format-patch`
    - `--update <tag>`: clones new upstream tag, applies patches, updates UPSTREAM_REF on success
    - `--validate`: checks patch numbering, ordering, and dependency constraints
    - `--help`: comprehensive usage documentation
    - `--src <path>`: override default source checkout path
- **Learnings:**
  - macOS ships bash 3.2 which lacks `declare -A` (associative arrays) — use portable `case` statements or string matching instead. This affects any script that needs to map keys to values
  - `git format-patch` / `git am` roundtrip: the `From` header (commit hash) differs after apply+export because `git am` creates new commits. Content (diff, subject, body) remains identical. This is expected behavior — compare content excluding first line to verify idempotency
  - `git clone --depth 50` is necessary (not `--depth 1`) for `git am` to work — the 3-way merge needs some history context to resolve patches
  - `git am --abort` must be called after a failed `git am` to clean up the mailbox state before attempting the next patch or before re-applying
  - Patch dependency validation uses a simple numbering convention: 0001-0008 with known dependency chains (socket depends on filesystem, gethostbyname depends on getaddrinfo, etc.)
---

## 2026-02-22 - warpgrid-agm.25
- Implemented US-203: Patch DNS getaddrinfo to route through WarpGrid shim
- Completed and verified getaddrinfo() interception in wasi-libc's bottom-half netdb.c
- Fixed critical wasm-ld weak symbol linking bug discovered during testing
- Files changed:
  - MOD: `build/src-patched/libc-bottom-half/sources/netdb.c` — WarpGrid DNS shim interception in getaddrinfo(), warpgrid_build_addrinfo() helper for packed address conversion
  - NEW: `build/src-patched/libc-bottom-half/sources/warpgrid_dns_shim.c` — weak default stub returning 0 for graceful degradation
  - MOD: `build/src-patched/libc-bottom-half/CMakeLists.txt` — added warpgrid_dns_shim.c to wasip2 build sources
  - NEW: `libc-patches/0001-dns-getaddrinfo-shim.patch` — exported git format-patch (11KB, 225 insertions)
  - MOD: `libc-patches/tests/test_dns_getaddrinfo.c` — updated AI_NUMERICHOST test to tolerate WASI runtime limitations
- **Learnings:**
  - **CRITICAL: wasm-ld weak symbol semantics differ from ELF.** In static archives (.a), a weak reference (`extern __attribute__((weak))`) in one archive member will NOT cause wasm-ld to pull a weak definition from another archive member. The linker creates an `undefined_weak` trap stub instead. Fix: make the extern declaration non-weak (strong reference), keep only the definition as weak.
  - `nm` output key: lowercase `w` = weak undefined (import), uppercase `W` = weak defined (export), `U` = strong undefined (forces archive pull)
  - Wasmtime 20 doesn't provide WASI sockets capabilities to vanilla `wasmtime run` — `ip_name_lookup_resolve_addresses()` returns `EAI_FAIL`. Tests must tolerate this gracefully.
  - The `build-libc.sh --patched` script clones fresh source + applies patches from `libc-patches/*.patch` — editing files directly in `build/src-patched/` without re-exporting the patch gets overwritten on next build
  - Correct workflow: edit source → amend commit in patched checkout → `git format-patch` → copy to `libc-patches/` → clean rebuild
  - Packed address record format (17 bytes: 1 family + 16 address) supports both IPv4 and IPv6 — IPv4 uses only bytes 1-4, remaining 12 bytes zeroed
---

## 2026-02-22 - warpgrid-agm.28
- Implemented US-206: Patch fopen/open to intercept virtual filesystem paths
- Created unified virtual fd mechanism — fopen() works transparently through the same path as open()
- All 10 TDD tests pass, full test suite (3/3) passes with no regressions
- Files changed:
  - NEW: `build/src-patched/libc-bottom-half/sources/warpgrid_fs_shim.c` — weak `__warpgrid_fs_read_virtual()` stub + virtual fd table (32 slots, 8KiB max content per file, base fd 0x70000000)
  - MOD: `build/src-patched/libc-bottom-half/sources/posix.c` — intercept `__wasilibc_open_nomode()` with `__warpgrid_vfd_open()` before `find_relpath()` call
  - MOD: `build/src-patched/libc-bottom-half/cloudlibc/src/libc/unistd/read.c` — intercept `read()` with `__warpgrid_vfd_is_virtual()` + `__warpgrid_vfd_read()`
  - MOD: `build/src-patched/libc-bottom-half/sources/__wasilibc_fd_renumber.c` — intercept `close()` with `__warpgrid_vfd_is_virtual()` + `__warpgrid_vfd_close()`
  - MOD: `build/src-patched/libc-bottom-half/cloudlibc/src/libc/unistd/lseek.c` — intercept `__lseek()` with `__warpgrid_vfd_is_virtual()` + `__warpgrid_vfd_lseek()`
  - MOD: `build/src-patched/libc-bottom-half/CMakeLists.txt` — added `sources/warpgrid_fs_shim.c` to wasip2 build
  - NEW: `libc-patches/0002-fs-virtual-open.patch` — exported patch (312 lines, 206 insertions across 6 files)
  - NEW: `libc-patches/tests/test_fs_virtual_open.c` — 10 comprehensive tests
- **Learnings:**
  - **Unified virtual fd approach eliminates need to patch fopen.c.** The call chain `fopen("r")` → `__wasilibc_open_nomode()` (intercepted) → virtual fd → `__fdopen()` → FILE* with `__stdio_read` callback → `read(virtual_fd)` (intercepted). Since `__fdopen()` for "r" mode never calls `__isatty()` or `fcntl()`, virtual fds work transparently.
  - **wasip2 readv() delegates to read()** — intercepting `read()` automatically covers `readv()` used by `__stdio_read`. No separate readv patch needed.
  - **Virtual fd numbering (0x70000000 base) avoids collision** with WASI descriptor table. WASI wasip2 uses a slab allocator starting at low fd numbers (3+). High base ensures no overlap even with many real fds.
  - **Return value protocol (-2/-1/>=0)** cleanly separates "not virtual" (-2, fall through), "virtual but error" (-1, errno set), and "success" (>=0, virtual fd). This avoids ambiguity between "path not found" and "open error".
  - **build-libc.sh --clean wipes source tree** — manual edits to `build/src-patched/` are destroyed. Must commit changes and export as patch before rebuilding. The `ensure_source()` function compares HEAD to UPSTREAM_COMMIT and re-clones on mismatch.
  - **malloc in wasm32-wasip2 works** — virtual fd content is malloc'd + memcpy'd from the stack buffer. The wasi-libc allocator handles this correctly for content up to WARPGRID_FS_MAX_CONTENT (8KiB).
---

