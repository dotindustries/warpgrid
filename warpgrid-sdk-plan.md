# WarpGrid SDK — Fork Engineering Plan for Claude Code

## Meta-Instructions for Claude Code

You are building the **WarpGrid SDK** — a curated, tested distribution of WebAssembly toolchain components that enables WarpGrid (a Wasm-native cluster orchestrator) to run real-world backend services. This document is your master plan. Work through it domain by domain, milestone by milestone.

### How to use this document

1. **Read the full document first** before writing any code.
2. **Work one milestone at a time.** Each milestone is designed to be completable in 1-3 sessions.
3. **Store state in persistent memory** after each session: what you completed, what's failing, what's next, branch names, file paths.
4. **Run tests before declaring a milestone complete.** Every milestone has explicit success criteria.
5. **Commit after each milestone** with a conventional commit message: `feat(domain): milestone description`.
6. **If you get stuck**, document the blocker in memory and move to the next independent milestone. Come back when you have more context.

### Persistent Memory Setup: claude-mem

This project spans dozens of sessions. You MUST have persistent memory to track state across them. Install **claude-mem** — it automatically captures tool usage, compresses observations with AI, and injects relevant context into future sessions.

#### Installation (run once before starting any work)

```bash
# From inside Claude Code:
/plugin marketplace add thedotmack/claude-mem
/plugin install claude-mem

# Restart Claude Code after installation.
# The plugin auto-starts a worker service on first session.
# Verify it's running:
# Visit http://localhost:37777 for the web dashboard.
```

#### How claude-mem works with this project

claude-mem uses a **3-layer progressive disclosure** system to stay within token limits:

1. **`search`** — returns a compact index of matching memories (~50-100 tokens/result)
2. **`timeline`** — shows chronological context around a specific observation
3. **`get_observations`** — fetches full details only for filtered IDs (~500-1,000 tokens/result)

This means ~10x token efficiency vs. loading all context upfront.

#### Session lifecycle hooks

claude-mem hooks into three Claude Code lifecycle events:
- **SessionStart** — injects relevant context from previous sessions into CLAUDE.md
- **Stop** (after each response) — captures tool usage observations
- **PreCompact** / **SessionEnd** — compresses and stores session summary

You don't need to do anything manually. Just work normally and claude-mem captures everything.

#### What to store explicitly (beyond auto-capture)

At the end of each session, explicitly tell claude-mem to remember:
- Which milestone you just completed or are mid-way through
- Any upstream commit hashes you're tracking
- Test failures that need investigation next session
- Design decisions made and why

Example:
```
Please remember: Completed M1.2 (filesystem shim). All virtual paths working 
except /proc/self/stat which needs kernel version string. Tracking wasi-libc 
upstream at commit a]b3c4d5. Next session: start M1.3 (DNS shim). Branch: 
feat/filesystem-shim.
```

#### Configuration

Settings live at `~/.claude-mem/settings.json`. Key settings for this project:

```json
{
  "contextObservations": 50,
  "summaryEnabled": true,
  "logLevel": "info"
}
```

Increase `contextObservations` if you find sessions aren't getting enough prior context. Decrease if you're hitting token limits.

#### Alternative: CLAUDE.md manual approach

If claude-mem isn't available (corporate environment, AGPL license restriction), fall back to manual CLAUDE.md management:

```bash
# In project root, create/maintain:
echo "# WarpGrid SDK Context" > CLAUDE.md
```

Manually update CLAUDE.md at end of each session with the Session Memory Template (see end of this document). Claude Code reads CLAUDE.md automatically on session start.

#### Alternative: @modelcontextprotocol/server-memory

For a lighter-weight option, the official Anthropic MCP memory server provides a knowledge graph in a local SQLite database:

```json
{
  "mcpServers": {
    "memory": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-memory"],
      "env": {
        "MEMORY_FILE_PATH": "/path/to/warpgrid-sdk/.claude/memory.json"
      }
    }
  }
}
```

This gives you entity/relation/observation storage, searchable via MCP tools. Less automated than claude-mem but simpler and MIT-licensed.

---

### Repository structure

```
warpgrid-sdk/
├── README.md
├── Cargo.toml                    # Workspace root
├── crates/
│   ├── warpgrid-host/            # Domain 1: Wasmtime host functions
│   ├── warpgrid-libc/            # Domain 2: Forked wasi-libc patches
│   ├── warpgrid-go/              # Domain 3: TinyGo WASI overlay
│   ├── warpgrid-js/              # Domain 4: ComponentizeJS extensions (Node.js)
│   ├── warpgrid-bun/             # Domain 6: Bun WASI runtime overlay
│   ├── warpgrid-async/           # Domain 5: WASI 0.3 async pre-integration
│   └── warpgrid-test-harness/    # Cross-domain integration tests
├── libc-patches/                 # Patch files against upstream wasi-libc
├── tinygo-patches/               # Patch files against upstream TinyGo
├── componentize-patches/         # Patch files against upstream componentize-js
├── bun-patches/                  # Patch files against upstream Bun WASI layer
├── test-apps/                    # Real-world test applications
│   ├── rust-http-server/
│   ├── rust-postgres-client/
│   ├── go-http-server/
│   ├── go-postgres-client/
│   ├── ts-http-handler/          # Node.js / ComponentizeJS
│   ├── ts-redis-client/          # Node.js / ComponentizeJS
│   ├── bun-http-handler/         # Bun runtime
│   └── bun-postgres-client/      # Bun runtime
├── docs/
│   ├── ARCHITECTURE.md
│   ├── FORK-MAINTENANCE.md
│   └── domains/
│       ├── 01-host-functions.md
│       ├── 02-libc-patches.md
│       ├── 03-tinygo-overlay.md
│       ├── 04-componentize-js.md
│       ├── 05-async-integration.md
│       └── 06-bun-runtime.md
└── scripts/
    ├── setup.sh                  # Clone upstreams, apply patches
    ├── rebase.sh                 # Rebase patches on new upstream releases
    └── test-all.sh               # Run full test suite
```

---

## Overview: What We're Building and Why

WarpGrid's core constraint is workload compatibility. ~25-35% of backend services compile to WASI today. The SDK's job is to push that number toward 60-70% through five coordinated efforts:

| Domain | Component | Approach | Impact |
|--------|-----------|----------|--------|
| 1 | Wasmtime host functions | No fork — use public API | Shim layer foundation |
| 2 | wasi-libc | Fork + patch | Socket/filesystem compat |
| 3 | TinyGo WASI layer | Fork + overlay | Go workload support |
| 4 | ComponentizeJS | Fork + extend | TypeScript/Node.js workload support |
| 5 | WASI 0.3 async | Pre-integrate prototyping branch | Async I/O for all languages |
| 6 | Bun WASI runtime | Overlay + shim bridge | Bun/TypeScript workload support |

### Dependency graph

```
Domain 5 (async) ──────────────────────────────────────┐
                                                        ▼
Domain 1 (host functions) ◄── Domain 2 (libc) ──► Domain 3 (TinyGo)
                                                ──► Domain 4 (ComponentizeJS / Node.js)
                                                ──► Domain 6 (Bun)
```

Domain 1 is the foundation. Domains 2-4, 6 depend on it. Domain 5 is orthogonal and can proceed in parallel. Domains 4 and 6 are parallel JS runtime tracks — 4 targets Node.js/ComponentizeJS (StarlingMonkey), 6 targets Bun's native Wasm/WASI support.

### Critical design principle

**Patches must be minimal and rebasing-friendly.** Do NOT rewrite upstream libraries. Patch the specific functions needed. Keep a `patches/` directory with `git format-patch` output so rebasing on upstream releases is mechanical, not archaeological.

---

## Domain 1: Wasmtime Host Functions (`warpgrid-host`)

### Purpose
Implement WarpGrid's shim layer as Wasmtime host functions using the public `wasmtime` crate API. No fork of Wasmtime required.

### Architecture

```
Wasm Module
    │
    ├── WASI P2 standard calls ──► Wasmtime's built-in WASI impl (unchanged)
    │
    └── WarpGrid shim calls ──► warpgrid-host functions (new)
         ├── warpgrid:shim/filesystem
         ├── warpgrid:shim/dns
         ├── warpgrid:shim/signals
         ├── warpgrid:shim/database-proxy
         └── warpgrid:shim/threading
```

Host functions are defined as WIT interfaces, registered with the Wasmtime linker at instantiation time. Modules that don't use shims don't pay for them.

### WIT Interface Definitions

Before writing any Rust, define the WIT interfaces. These are the contracts.

```
// wit/warpgrid-shims.wit

package warpgrid:shim@0.1.0;

interface filesystem {
    /// Read a virtual file. Returns contents or none if not virtual.
    read-virtual: func(path: string) -> option<list<u8>>;

    /// Check if a path is handled by the virtual filesystem.
    is-virtual: func(path: string) -> bool;

    /// List of virtual paths this shim provides.
    list-virtual-paths: func() -> list<string>;
}

interface dns {
    /// Resolve a hostname to addresses via WarpGrid service discovery.
    resolve: func(hostname: string) -> result<list<ip-address>, dns-error>;

    record ip-address {
        octets: list<u8>,  // 4 bytes for IPv4, 16 for IPv6
        is-v6: bool,
    }

    enum dns-error {
        not-found,
        timeout,
        internal,
    }
}

interface signals {
    /// Register a handler for lifecycle signals.
    on-signal: func(signal: signal-type) -> bool;

    enum signal-type {
        terminate,   // Maps to SIGTERM
        hangup,      // Maps to SIGHUP — config reload
        interrupt,   // Maps to SIGINT
    }

    /// Check if a signal has been received (non-blocking poll).
    poll-signal: func() -> option<signal-type>;
}

interface database-proxy {
    /// Open a proxied database connection through the host's connection pool.
    connect: func(config: db-config) -> result<connection, db-error>;

    /// Send bytes on a proxied connection (wire protocol passthrough).
    send: func(conn: connection, data: list<u8>) -> result<u32, db-error>;

    /// Receive bytes from a proxied connection.
    recv: func(conn: connection, max-bytes: u32) -> result<list<u8>, db-error>;

    /// Close a proxied connection (returns it to the pool).
    close: func(conn: connection);

    record db-config {
        protocol: db-protocol,
        host: string,
        port: u16,
        database: option<string>,
        pool-size: u32,
    }

    enum db-protocol {
        postgres,
        mysql,
        redis,
    }

    type connection = u64;  // Opaque handle

    enum db-error {
        connection-refused,
        timeout,
        protocol-error,
        pool-exhausted,
        closed,
    }
}

interface threading {
    /// Hint to the host about the module's threading expectations.
    /// The host may use this to warn about incompatibilities.
    declare-threading-model: func(model: threading-model);

    enum threading-model {
        single-threaded,
        cooperative,       // Goroutines, green threads — sequential OK
        parallel-required, // Needs real CPU parallelism — will warn
    }
}

/// The world that shim-enabled modules target.
world shim-enabled {
    include wasi:cli/imports@0.2.0;

    import filesystem;
    import dns;
    import signals;
    import database-proxy;
    import threading;
}
```

### Milestones

#### M1.1 — Project scaffold and WIT definitions (1 session)
- [ ] Create `crates/warpgrid-host/` with Cargo.toml
- [ ] Dependencies: `wasmtime`, `wasmtime-wasi`, `tokio`, `tracing`, `anyhow`
- [ ] Write WIT files in `crates/warpgrid-host/wit/`
- [ ] Generate Rust bindings from WIT using `wit-bindgen` or `wasmtime::component::bindgen!`
- [ ] Verify: `cargo check` passes

**Success criteria:** Project compiles. WIT bindings generate. No runtime code yet.

#### M1.2 — Filesystem shim (1-2 sessions)
- [ ] Implement `filesystem` host functions
- [ ] Virtual file map:
  - `/dev/null` → empty sink
  - `/dev/urandom` → `getrandom()` on host
  - `/etc/resolv.conf` → generated from WarpGrid DNS config
  - `/etc/hosts` → generated from WarpGrid service registry
  - `/proc/self/` → synthetic process info (pid=1, basic stat)
  - `/usr/share/zoneinfo/**` → embedded timezone database (use `chrono-tz` data)
- [ ] Intercept logic: if path matches virtual, return virtual content; otherwise fall through to real WASI filesystem
- [ ] Unit tests for each virtual path
- [ ] Integration test: compile a Rust program that reads `/etc/resolv.conf` and `/dev/urandom`, verify it gets WarpGrid-generated content

**Success criteria:** A Wasm module reading virtual paths gets correct synthetic content. Non-virtual paths work normally.

#### M1.3 — DNS resolution shim (1-2 sessions)
- [ ] Implement `dns` host functions
- [ ] Resolution chain:
  1. Check WarpGrid service registry: `<name>.<namespace>.warp.local` → internal endpoint
  2. Check `/etc/hosts` overrides
  3. Fall through to host system DNS
- [ ] Service registry is a `HashMap<String, Vec<IpAddr>>` injected at instantiation
- [ ] Round-robin across multiple addresses
- [ ] TTL caching with configurable duration
- [ ] Unit tests: resolve known service names, verify round-robin, verify cache expiry
- [ ] Integration test: Wasm module does DNS lookup for `db.production.warp.local`, gets correct internal IP

**Success criteria:** Service-to-service DNS works. External DNS falls through correctly.

#### M1.4 — Signal handling shim (1 session)
- [ ] Implement `signals` host functions
- [ ] Map WarpGrid lifecycle events to signal types:
  - Instance stop → `terminate`
  - Config reload → `hangup`
  - Ctrl-C (dev mode) → `interrupt`
- [ ] Signal queue (bounded, 16 entries) that modules poll
- [ ] `on-signal` registers interest, `poll-signal` dequeues
- [ ] Unit tests
- [ ] Integration test: host sends terminate, module's signal handler runs cleanup

**Success criteria:** Modules that register signal handlers get notified of lifecycle events.

#### M1.5 — Database proxy shim: connection pool (2-3 sessions)
- [ ] Implement `database-proxy` host functions
- [ ] **Connection pool manager** (host-side, Rust):
  - Pool per `(protocol, host, port, database)` tuple
  - Configurable pool size (default 10)
  - Connection health checking (ping on checkout)
  - Idle timeout (return to pool after 60s unused)
  - TLS support via `rustls` (not native OpenSSL)
- [ ] **Postgres wire protocol passthrough:**
  - Module sends raw Postgres protocol bytes via `send()`
  - Host forwards to pooled connection
  - Host returns response bytes via `recv()`
  - Module thinks it's talking raw TCP
  - Host manages TLS termination transparently
- [ ] Connection handle is opaque `u64` mapping to pool slot
- [ ] `close()` returns connection to pool (does NOT terminate TCP)
- [ ] Unit tests with mock Postgres server
- [ ] Integration test: compile `sqlx` Rust app to Wasm, run queries against real Postgres through the proxy

**Success criteria:** A Rust app using `sqlx` or `tokio-postgres` can query a real Postgres database through the proxy with zero code changes.

#### M1.6 — Database proxy: MySQL and Redis (2 sessions)
- [ ] Add MySQL wire protocol passthrough (same architecture as Postgres)
- [ ] Add Redis RESP protocol passthrough
- [ ] Each protocol: health check (ping), TLS, connection draining
- [ ] Integration tests with real MySQL and Redis

**Success criteria:** Standard MySQL and Redis drivers work through the proxy.

#### M1.7 — Threading model shim (1 session)
- [ ] Implement `threading` host functions
- [ ] `declare-threading-model`: module tells host what it expects
- [ ] If `parallel-required`: host logs warning, sets deployment status flag
- [ ] If `cooperative`: host enables sequential execution mode
- [ ] This is informational in Phase 1 — actual threading waits for upstream `shared-everything-threads`
- [ ] Unit tests

**Success criteria:** Host knows the module's threading expectations and warns appropriately.

#### M1.8 — Shim configuration and instantiation (1-2 sessions)
- [ ] `ShimConfig` struct loaded from deployment spec (`warp.toml` `[shims]` section)
- [ ] Only register host functions for enabled shims (zero cost for disabled)
- [ ] `WarpGridEngine` wrapper:
  ```rust
  let engine = WarpGridEngine::new(config)?;
  let module = engine.load_module("path/to/module.wasm")?;
  let instance = engine.instantiate(module, shim_config)?;
  instance.call_handler(request)?;
  ```
- [ ] Startup logging: list which shims are active, what they intercept
- [ ] Integration test: instantiate module with various shim configs, verify only requested shims are active

**Success criteria:** Clean API for instantiating Wasm modules with configurable shims.

---

## Domain 2: wasi-libc Patches (`warpgrid-libc`)

### Purpose
Patch upstream wasi-libc so that standard C library functions (used by Rust, Go, C programs compiled to WASI) route through WarpGrid's host function shims where appropriate.

### Approach
- Fork `WebAssembly/wasi-libc` to `warpgrid/wasi-libc`
- Maintain patches as `git format-patch` files in `libc-patches/`
- Rebase script: `scripts/rebase-libc.sh` applies patches to latest upstream

### Key files to patch

```
wasi-libc/
├── libc-bottom-half/
│   ├── sources/
│   │   ├── connect.c          # TCP connect → shim DNS + proxy
│   │   ├── gethostbyname.c    # DNS → shim DNS resolution
│   │   └── socket.c           # Socket creation
│   └── headers/
│       └── wasi/               # WASI-specific headers
└── libc-top-half/
    └── musl/
        └── src/
            ├── network/
            │   ├── getaddrinfo.c   # DNS resolution
            │   ├── getnameinfo.c   # Reverse DNS
            │   └── res_query.c     # DNS queries
            ├── stdio/
            │   └── fopen.c         # File open → virtual filesystem check
            └── time/
                └── __tz.c          # Timezone loading
```

### Milestones

#### M2.1 — Fork setup and build verification (1 session)
- [ ] Clone `WebAssembly/wasi-libc` at a known tag (find latest stable)
- [ ] Verify stock build: `make -j$(nproc) THREAD_MODEL=single`
- [ ] Create `warpgrid` branch
- [ ] Set up CI that builds both stock and patched versions
- [ ] Document the upstream commit hash in `libc-patches/UPSTREAM_REF`

**Success criteria:** Stock wasi-libc builds. Patched branch builds. CI green.

#### M2.2 — DNS resolution patches (2-3 sessions)
- [ ] Patch `getaddrinfo.c`:
  - Before normal resolution, call `warpgrid:shim/dns.resolve(hostname)`
  - If shim returns addresses, use them (skip system DNS)
  - If shim returns not-found, fall through to normal resolution
  - Handle IPv4 and IPv6 addresses
- [ ] Patch `gethostbyname.c` to use same logic
- [ ] Patch `getnameinfo.c` for reverse lookups against WarpGrid registry
- [ ] Add weak symbol imports for shim functions (so unpatched builds still link)
- [ ] Test: compile a C program that calls `getaddrinfo("db.production.warp.local")`, verify it resolves via shim
- [ ] Test: compile same program with stock wasi-libc, verify it still builds (shim calls are weak/optional)

**Success criteria:** `getaddrinfo` routes through WarpGrid DNS for `.warp.local` names. Programs compiled against stock wasi-libc still link and work (graceful degradation).

#### M2.3 — Filesystem virtualization patches (1-2 sessions)
- [ ] Patch `fopen.c` / `open.c`:
  - Intercept opens to virtual paths (`/dev/urandom`, `/etc/resolv.conf`, `/proc/self/*`, `/usr/share/zoneinfo/*`)
  - Route to `warpgrid:shim/filesystem.read-virtual(path)`
  - Return file descriptor backed by in-memory buffer
  - Non-virtual paths: unchanged
- [ ] Patch `__tz.c`:
  - Timezone loading uses virtual `/usr/share/zoneinfo/` backed by embedded tz data
- [ ] Test: C program reads `/etc/resolv.conf`, gets WarpGrid-generated content
- [ ] Test: C program calls `localtime()`, gets correct timezone

**Success criteria:** Programs that read system files get WarpGrid-provided content transparently.

#### M2.4 — Socket connect patches for database proxy (2-3 sessions)
- [ ] Patch `connect.c`:
  - When connecting to a host:port that matches a configured database proxy endpoint:
    - Instead of raw TCP connect, call `warpgrid:shim/database-proxy.connect(config)`
    - Return a file descriptor backed by the proxy's `send()`/`recv()`
  - Non-proxied connections: unchanged (use wasi-sockets normally)
- [ ] Patch `send.c` / `recv.c` / `read.c` / `write.c`:
  - If fd is a proxied connection, route through `database-proxy.send()` / `database-proxy.recv()`
  - Otherwise: normal WASI I/O
- [ ] Patch `close.c`:
  - Proxied fd: call `database-proxy.close()` (returns to pool)
  - Normal fd: standard close
- [ ] Test: compile `libpq` (Postgres C client) against patched wasi-libc, connect to Postgres through proxy
- [ ] Test: compile a Go program (via TinyGo) that uses `database/sql` with `pgx`, verify query execution

**Success criteria:** Database drivers that use standard socket syscalls work transparently through the proxy. This is the single most impactful patch in the entire SDK.

#### M2.5 — Patch maintenance tooling (1 session)
- [ ] `scripts/rebase-libc.sh`:
  - Fetches latest upstream wasi-libc tag
  - Attempts to apply patches via `git am`
  - Reports conflicts clearly
  - Generates new patch set on success
- [ ] `scripts/build-libc.sh`:
  - Builds patched wasi-libc
  - Produces sysroot suitable for `--sysroot` flag in clang/rustc
- [ ] `scripts/test-libc.sh`:
  - Compiles test programs against patched sysroot
  - Runs them in Wasmtime with WarpGrid host functions
  - Reports pass/fail
- [ ] Document rebase process in `docs/FORK-MAINTENANCE.md`

**Success criteria:** Rebasing on a new upstream release is a single command. Building is a single command. Testing is a single command.

---

## Domain 3: TinyGo WASI Overlay (`warpgrid-go`)

### Purpose
Patch TinyGo's WASI target and stdlib shims so Go programs compiled to WASI can use networking, database drivers, and standard library functions that currently fail.

### Approach
- Fork `tinygo-org/tinygo`
- Focus patches on `src/runtime/` and `src/syscall/` WASI implementations
- Overlay patched stdlib packages where TinyGo's versions are too limited

### Key gaps in TinyGo's WASI support

1. `net` package: partial. `net.Dial("tcp", ...)` doesn't work reliably.
2. `net/http`: very limited. HTTP client works for simple GETs. Server doesn't work.
3. `database/sql`: depends on `net` working for TCP connections to databases.
4. `crypto/tls`: depends on `net` and is incomplete.
5. `os/exec`: fundamentally impossible (no fork/exec in WASI).
6. `reflect`: partial support — many drivers use it.

### Milestones

#### M3.1 — Fork setup and baseline (1-2 sessions)
- [ ] Clone TinyGo at latest release tag
- [ ] Build TinyGo from source (requires Go 1.22+, LLVM)
- [ ] Verify: `tinygo build -target=wasip2 -o test.wasm` works for a hello-world
- [ ] Document which stdlib packages currently work/fail for wasip2 target
- [ ] Create a test matrix: 20 common Go packages, mark each as pass/fail/partial

**Success criteria:** TinyGo builds from source. Baseline compatibility documented.

#### M3.2 — net.Dial TCP patch (2-3 sessions)
- [ ] Patch `src/syscall/syscall_wasip2.go` (or equivalent):
  - `net.Dial("tcp", "host:port")` → use wasi-sockets if available
  - If host matches WarpGrid proxy config → route through database proxy shim
- [ ] Patch `src/net/` to handle the WASI socket file descriptors correctly
- [ ] Test: Go program does `net.Dial("tcp", "postgres:5432")` and sends/receives bytes
- [ ] Test: Go program uses `pgx` to connect to Postgres

**Success criteria:** `net.Dial("tcp", ...)` works for database connections via WarpGrid's proxy.

#### M3.3 — net/http server stub (2 sessions)
- [ ] For WarpGrid's HTTP trigger model, the module doesn't run an HTTP server — it exports a handler function
- [ ] Create `warpgrid/net/http` overlay package:
  - `http.ListenAndServe()` → registers the handler with WarpGrid's trigger system instead of binding a socket
  - Request/response objects are wasi-http types under the hood
  - Existing Go HTTP handler code (`http.HandleFunc("/path", handler)`) works without changes
- [ ] Test: standard Go HTTP server code compiles and handles requests via WarpGrid trigger

**Success criteria:** Go developers write normal `net/http` handler code, it runs as a WarpGrid HTTP trigger.

#### M3.4 — database/sql driver compatibility (2 sessions)
- [ ] Test `pgx` (Postgres), `go-sql-driver/mysql`, `go-redis/redis` with patched TinyGo
- [ ] For each driver, document:
  - Does it compile? If not, what fails?
  - Does it run? If not, what syscall is missing?
  - Fix or document workaround
- [ ] Create `warpgrid/database/sql` overlay if needed (unlikely if net.Dial works)

**Success criteria:** At least Postgres via `pgx` works end-to-end through TinyGo + WarpGrid.

#### M3.5 — Patch maintenance (1 session)
- [ ] Same tooling pattern as Domain 2: rebase script, build script, test script
- [ ] `scripts/build-tinygo.sh`: builds patched TinyGo, produces `warp-tinygo` binary
- [ ] `warp pack --lang go` calls `warp-tinygo` instead of stock TinyGo

**Success criteria:** Single command to build patched TinyGo and use it via `warp pack`.

---

## Domain 4: ComponentizeJS Extensions (`warpgrid-js`)

### Purpose
Extend ComponentizeJS so TypeScript/JavaScript HTTP handlers can use WarpGrid's shim layer, access databases, and handle a broader set of Node.js API patterns.

### Approach
- Fork `bytecodealliance/ComponentizeJS` (or `nicolo-ribaudo/componentize-js` — check which is canonical)
- Extend the JS runtime (StarlingMonkey) with WarpGrid-specific globals
- Add polyfills for common Node.js patterns

### Key gaps

1. No `net.Socket` — can't connect to databases
2. No `dns.resolve` — can't do service discovery
3. No `fs.readFile` for system paths
4. No `process.env` equivalent (WASI has `wasi-cli` env, but JS layer may not expose it)
5. No `pg` (node-postgres) — depends on `net.Socket`

### Milestones

#### M4.1 — Fork setup and baseline (1-2 sessions)
- [ ] Clone ComponentizeJS at latest tag
- [ ] Build and verify: componentize a simple HTTP handler, run in Wasmtime
- [ ] Document which Node.js APIs are available in the JS runtime
- [ ] Test: can you `import pg from 'pg'`? What breaks?

**Success criteria:** ComponentizeJS builds. Baseline API surface documented.

#### M4.2 — Database connectivity via shim (2-3 sessions)
- [ ] Add `warpgrid.database.connect(config)` global function in the JS runtime
- [ ] Returns an object with `send(data)` and `recv(maxBytes)` methods (backed by database proxy shim)
- [ ] Create a thin wrapper: `warpgrid/pg` package that uses this to implement the `pg.Client` interface
- [ ] Test: TypeScript handler connects to Postgres, runs a query, returns result as HTTP response

**Success criteria:** TypeScript HTTP handlers can query Postgres.

#### M4.3 — DNS and filesystem polyfills (1-2 sessions)
- [ ] `warpgrid.dns.resolve(hostname)` → backed by DNS shim
- [ ] `warpgrid.fs.readFile(path)` → backed by filesystem shim (virtual paths only)
- [ ] `process.env` → backed by WASI environment variables
- [ ] Test: handler reads env vars, resolves service names, reads timezone data

**Success criteria:** Common Node.js patterns work in WarpGrid's JS runtime.

#### M4.4 — npm package compatibility testing (1-2 sessions)
- [ ] Test top 20 npm packages used in HTTP handlers:
  - `express` / `hono` / `itty-router` (routing)
  - `zod` (validation)
  - `jose` (JWT)
  - `nanoid` / `uuid`
  - `date-fns` / `luxon`
  - `lodash` / `ramda`
  - `pg` / `ioredis` (with shim)
- [ ] For each: compile, run, document result
- [ ] Create compatibility table in docs

**Success criteria:** Compatibility matrix for top 20 npm packages. At least 12/20 work.

---

## Domain 5: WASI 0.3 Async Pre-Integration (`warpgrid-async`)

### Purpose
Pre-integrate the WASI 0.3 native async support from the Wasmtime prototyping branch, giving WarpGrid modules async I/O capabilities 6-12 months ahead of stable upstream.

### Approach
- Track `bytecodealliance/wasip3-prototyping` branch
- Integrate into WarpGrid's Wasmtime embedding
- This is NOT a fork of Wasmtime — it's using the prototyping branch as a dependency
- Wrap the async APIs to provide a stable interface even as the upstream API shifts

### Key capabilities unlocked

1. Async HTTP handling (multiple concurrent requests without threads)
2. Async database queries (non-blocking I/O)
3. Async file operations
4. Stream types for efficient data transfer

### Milestones

#### M5.1 — Prototyping branch integration (2-3 sessions)
- [ ] Clone `wasip3-prototyping` repo
- [ ] Build Wasmtime from the prototyping branch
- [ ] Verify: compile and run a WASI 0.3 async HTTP handler example
- [ ] Document the API surface: what's available, what's unstable

**Success criteria:** A Wasm module using async I/O runs in the prototyping Wasmtime.

#### M5.2 — Stable wrapper API (2 sessions)
- [ ] Create `warpgrid-async` crate that wraps the prototyping APIs
- [ ] Provide a stable interface:
  ```rust
  pub trait AsyncHandler {
      async fn handle_request(&self, req: Request) -> Response;
  }
  ```
- [ ] If upstream API changes, only the wrapper internals change — user-facing API is stable
- [ ] Integration with Domain 1's `WarpGridEngine`

**Success criteria:** WarpGrid modules can be compiled against a stable async API.

#### M5.3 — Async database proxy (1-2 sessions)
- [ ] Modify Domain 1's database proxy shim to use async I/O
- [ ] Non-blocking sends and receives
- [ ] Connection pool uses async checkout
- [ ] Benchmark: concurrent query throughput vs. sync version

**Success criteria:** Database queries don't block other requests in the same module.

#### M5.4 — Language support (2-3 sessions)
- [ ] Verify Rust async handlers compile against wasi 0.3 WIT
- [ ] Test with `wit-bindgen` Rust async bindings
- [ ] Document: how to write an async handler in Rust, Go (cooperative), TypeScript (natural async)
- [ ] Create template projects for each language

**Success criteria:** Developers can write async HTTP handlers in Rust and TypeScript.

---

## Domain 6: Bun WASI Runtime (`warpgrid-bun`)

### Purpose
Enable Bun-based TypeScript/JavaScript applications to compile and run as WarpGrid Wasm modules, providing an alternative to the Node.js/ComponentizeJS path (Domain 4) for teams already using Bun.

### Why Bun alongside Node.js/ComponentizeJS?

Bun is rapidly gaining adoption as a Node.js alternative. Many new TypeScript backend projects start with Bun. Offering a Bun path alongside the ComponentizeJS/Node.js path means WarpGrid doesn't force teams to switch runtimes. The approaches differ:

| | Domain 4: ComponentizeJS (Node.js) | Domain 6: Bun |
|---|---|---|
| **JS engine** | StarlingMonkey (SpiderMonkey fork) | JavaScriptCore (Bun's engine) |
| **WASI support** | Full component model via jco | Partial — `node:wasi` module (P1), raw Wasm instantiation |
| **Maturity** | More mature for Wasm components | Earlier stage for WASI, but very fast native runtime |
| **Best for** | Full component model workflows, wasi-http handlers | Teams already on Bun, simpler Wasm embedding, fast iteration |
| **Compilation path** | `componentize-js` → Wasm component | `jco` transpile or direct Wasm instantiation via Bun's native API |

### Current state of Bun + Wasm/WASI

Key facts from research:

- Bun supports `node:wasi` module (WASI Preview 1 level), based on wasmer-js
- Bun has native `.wasm` file loading — `import` or `Bun.file("module.wasm").arrayBuffer()` → `WebAssembly.instantiate()`
- Bun does NOT yet support Wasm Components (WASI P2) natively — there's an open feature request (oven-sh/bun#24867)
- Bun CAN use `jco transpile` to convert Wasm components into JS modules that Bun can run
- Bun's `node:wasi` implementation supports preopened directories, env vars, args, stdin/stdout/stderr

### Architecture: two compilation paths

```
Path A: Bun-native (simpler, WASI P1 only)
  TypeScript source → esbuild/Bun.build → bundle.js
  → jco componentize → component.wasm
  → Run in Wasmtime with WarpGrid host functions

Path B: Bun as development runtime, deploy as Wasm component
  TypeScript source → develop/test with `bun run` locally
  → componentize-js → component.wasm (for deployment)
  → Run in Wasmtime with WarpGrid host functions
  
  This path lets teams use Bun's fast DX for development
  while deploying standard Wasm components to WarpGrid.

Path C: Bun as Wasm host (embed Wasmtime modules IN Bun)
  For local development/testing only.
  Bun process → loads .wasm via node:wasi → runs with mock shims
  Enables `bun test` to run WarpGrid modules locally.
```

**Path B is the recommended default.** Teams write Bun-idiomatic TypeScript, test with `bun run`/`bun test` locally, and `warp pack --lang bun` compiles to a Wasm component for deployment.

### Milestones

#### M6.1 — Bun development environment setup (1-2 sessions)
- [ ] Create `crates/warpgrid-bun/` (Rust tooling + TypeScript scaffolding)
- [ ] Create `warpgrid-bun-sdk/` npm package (TypeScript):
  ```
  warpgrid-bun-sdk/
  ├── package.json
  ├── tsconfig.json
  ├── src/
  │   ├── index.ts           # Main exports
  │   ├── handler.ts         # HTTP handler types
  │   ├── database.ts        # Database proxy bindings
  │   ├── dns.ts             # DNS shim bindings
  │   └── filesystem.ts      # Virtual filesystem bindings
  └── test/
      └── handler.test.ts    # Bun test suite
  ```
- [ ] Define the WarpGrid handler interface for Bun:
  ```typescript
  // src/handler.ts
  export interface WarpGridHandler {
    fetch(request: Request): Promise<Response>;
  }
  
  // Uses Web Standard Request/Response — same as Bun.serve()
  // This means existing Bun HTTP handlers work with minimal changes.
  ```
- [ ] Verify: `bun test` runs, `bun build` produces a bundle

**Success criteria:** SDK package structure exists. TypeScript types compile. Bun tests run.

#### M6.2 — `warp pack --lang bun` compilation path (2-3 sessions)
- [ ] Add Bun support to `warp-pack` crate:
  ```
  warp pack --lang bun --entry src/handler.ts
  ```
  This does:
  1. Run `bun build --target=browser` to bundle TypeScript into a single ESM file
  2. Run `jco componentize` to convert the bundle into a Wasm component
  3. Validate the component exports `wasi:http/incoming-handler`
  4. Output the `.wasm` component file
- [ ] Handle Bun-specific patterns:
  - `Bun.file()` → polyfill with WASI filesystem
  - `Bun.env` → polyfill with WASI environment
  - `Bun.sleep()` → polyfill with WASI clocks
  - `Bun.serve()` → compile-time transform to `wasi:http/incoming-handler` export
- [ ] Create shim package `@warpgrid/bun-polyfills`:
  - Replaces Bun-specific APIs with WASI-compatible equivalents at bundle time
  - Automatic via `warp pack` — developers don't need to change imports
- [ ] Test: simple Bun HTTP handler (`Bun.serve({fetch(req) { ... }})`) compiles to Wasm component
- [ ] Test: compiled component runs in Wasmtime and handles HTTP requests

**Success criteria:** A standard Bun HTTP handler compiles to a Wasm component and runs in Wasmtime.

#### M6.3 — Database and DNS shim bindings for Bun (1-2 sessions)
- [ ] `@warpgrid/bun-sdk` package with Bun-idiomatic APIs:
  ```typescript
  // Bun-idiomatic database access
  import { createPool } from "@warpgrid/bun-sdk/postgres";
  
  const pool = createPool({
    host: "db.production.warp.local",  // Resolved via DNS shim
    database: "myapp",
    poolSize: 10,                       // Managed by host proxy
  });
  
  // Uses WarpGrid database proxy under the hood
  const result = await pool.query("SELECT * FROM users WHERE id = $1", [userId]);
  ```
- [ ] DNS resolution: `@warpgrid/bun-sdk/dns` wraps the DNS shim
- [ ] Filesystem: `@warpgrid/bun-sdk/fs` wraps the filesystem shim for virtual paths
- [ ] These APIs work in TWO modes:
  - **Development (Bun native):** Real TCP connections (for local dev with real DBs)
  - **Deployed (Wasm component):** Routes through WarpGrid host function shims
  - Mode detection is automatic: check if running in Wasm or native
- [ ] Test: Bun handler queries Postgres via SDK, works in both dev mode and Wasm deployment

**Success criteria:** Developers use the same SDK code locally and in WarpGrid. Zero code changes between dev and deploy.

#### M6.4 — Local development experience (1-2 sessions)
- [ ] `warp dev --lang bun` command:
  1. Starts a local Wasmtime instance with WarpGrid shims
  2. Watches TypeScript source files for changes
  3. On change: re-bundles with `bun build`, re-componentizes, hot-reloads in Wasmtime
  4. Proxies HTTP requests from localhost:3000 to the Wasm module
- [ ] Alternative: `bun run --warpgrid` mode:
  - Runs the handler natively in Bun for fastest iteration
  - Uses real network calls (no shims)
  - Quick-test mode before deploying to Wasm
- [ ] Test: modify source → see change reflected in ~1 second (Bun build is fast)

**Success criteria:** Developer edits TypeScript, sees results in under 2 seconds. Two modes: fast-native (Bun) and accurate-Wasm (Wasmtime).

#### M6.5 — npm/Bun package compatibility testing (1-2 sessions)
- [ ] Test top 20 packages commonly used with Bun:
  - **HTTP/routing:** `hono`, `elysia`, `itty-router`
  - **Validation:** `zod`, `typebox`, `valibot`
  - **Auth/crypto:** `jose`, `@noble/hashes`, `nanoid`, `uuid`
  - **Data:** `drizzle-orm`, `kysely` (with WarpGrid Postgres proxy)
  - **Utilities:** `date-fns`, `lodash`, `superjson`
  - **Testing:** `bun:test` (for local dev only, not in Wasm)
- [ ] For each package: compile via `warp pack --lang bun`, run in Wasmtime, document result
- [ ] Special attention to packages with native bindings (these WILL fail):
  - `better-sqlite3` → suggest `sql.js` or WarpGrid Postgres proxy
  - `sharp` → suggest `wasm-vips`
  - `bcrypt` → suggest `bcryptjs` or `@noble/hashes`
  - `canvas` → not supported
- [ ] Create compatibility table in docs
- [ ] Integrate results into warp-analyzer: `warp convert analyze` should know Bun package compat

**Success criteria:** Compatibility matrix for top 20 Bun packages. At least 14/20 work. Analyzer reports accurate verdicts.

#### M6.6 — Bun maintenance and `warp pack` integration (1 session)
- [ ] `warp pack --lang bun` is a first-class path alongside `--lang rust`, `--lang go`, `--lang typescript`
- [ ] `--lang typescript` uses ComponentizeJS (Domain 4), `--lang bun` uses Bun build + jco
- [ ] Document the differences and when to use which in docs
- [ ] `scripts/test-bun.sh`: build + run integration tests for Bun path
- [ ] Bun package compat entries added to `compat-db/bun/` directory

**Success criteria:** `warp pack --lang bun` is documented, tested, and part of CI.

---

## Cross-Domain Integration Tests

### Purpose
Verify that all domains work together end-to-end.

### Test applications (in `test-apps/`)

#### T1: Rust HTTP + Postgres
- Rust HTTP handler using `axum` compiled to WASI
- Queries Postgres through database proxy shim
- Uses DNS shim to resolve `db.production.warp.local`
- Expected: full request cycle works, data returns correctly

#### T2: Rust HTTP + Redis + Postgres
- Same as T1 but also connects to Redis for caching
- Two proxied connections from same module

#### T3: Go HTTP + Postgres
- Go HTTP handler using standard `net/http` patterns
- Compiled with patched TinyGo
- Queries Postgres through database proxy
- Expected: full request cycle works

#### T4: TypeScript (Node.js) HTTP + Postgres
- TypeScript handler using `hono` router
- Compiled with patched ComponentizeJS
- Queries Postgres through database proxy wrapper
- Expected: full request cycle works

#### T5: Bun HTTP + Postgres
- Bun TypeScript handler using `hono` router
- Compiled with `warp pack --lang bun` (Bun build + jco)
- Queries Postgres through `@warpgrid/bun-sdk/postgres`
- Expected: full request cycle works, identical behavior to T4

#### T6: Multi-service
- Four modules: API gateway (Rust), user service (Go), notification service (TypeScript/Node.js), analytics service (Bun)
- API gateway routes to all services via DNS shim
- User service queries Postgres
- Notification service reads from Redis queue
- Analytics service writes to Postgres via Bun SDK
- Expected: full end-to-end multi-service flow works across all four language runtimes

### Integration test infrastructure

- Docker Compose for test dependencies (Postgres, MySQL, Redis)
- `scripts/test-all.sh` orchestrates: start deps → build test apps → run in Wasmtime with shims → assert results → teardown
- CI runs integration tests on every PR

---

## Milestone Ordering and Dependencies

### Phase A: Foundation (Weeks 1-4)
```
M1.1 (scaffold) ──► M1.2 (filesystem) ──► M1.3 (DNS) ──► M1.4 (signals)
M2.1 (libc fork setup) [parallel]
M5.1 (async prototyping) [parallel]
```

### Phase B: Database Proxy (Weeks 4-8)
```
M1.5 (DB proxy - Postgres) ──► M1.6 (MySQL + Redis)
M2.2 (libc DNS patches) [parallel with M1.5]
M2.3 (libc filesystem patches) [parallel]
```

### Phase C: Socket Integration (Weeks 8-12)
```
M2.4 (libc socket patches) ──► depends on M1.5 being done
M1.7 (threading model)
M1.8 (config + instantiation API)
M2.5 (libc maintenance tooling)
```

### Phase D: Language Support (Weeks 10-16)
```
M3.1 (TinyGo setup) ──► M3.2 (net.Dial) ──► M3.3 (HTTP server) ──► M3.4 (database/sql)
M4.1 (ComponentizeJS setup) ──► M4.2 (DB via shim) ──► M4.3 (polyfills) ──► M4.4 (npm compat)
M6.1 (Bun SDK setup) ──► M6.2 (warp pack --lang bun) ──► M6.3 (DB/DNS bindings) [parallel with M4.x]
```

### Phase E: Async + Polish (Weeks 12-20)
```
M5.2 (stable wrapper) ──► M5.3 (async DB proxy) ──► M5.4 (language support)
M6.4 (Bun local dev) ──► M6.5 (package compat testing) ──► M6.6 (maintenance)
M3.5 (TinyGo maintenance)
Cross-domain integration tests (T1-T6)
```

---

## Session Memory Template

After each session, store the following in persistent memory:

```
## WarpGrid SDK Session Log — [DATE]

### Completed
- [milestone ID]: [brief description of what was done]
- Files changed: [list]
- Tests passing: [yes/no, which ones]

### In Progress
- [milestone ID]: [what remains]
- Current branch: [name]
- Blockers: [if any]

### Next Session Priority
1. [specific task]
2. [specific task]

### Known Issues
- [issue description + file + line if applicable]

### Upstream Tracking
- wasi-libc upstream: [commit hash we're based on]
- TinyGo upstream: [commit hash]
- ComponentizeJS upstream: [commit hash]
- Bun upstream: [version tag]
- jco upstream: [version tag]
- wasip3-prototyping: [commit hash]
```

---

## Quality Gates

Before declaring any domain "complete":

1. **All milestones pass their success criteria**
2. **CI is green** — builds and tests automated
3. **Documentation exists** — ARCHITECTURE.md updated, domain doc updated
4. **Rebase tested** — patches apply cleanly to latest upstream
5. **At least one integration test (T1-T6) passes** using that domain
6. **Performance baseline** — no more than 10% overhead vs. stock for non-shimmed operations

---

## Notes for Claude Code

### Environment expectations
- You'll need: Rust nightly (for Wasmtime), Go 1.22+, Node.js 20+, Bun 1.1+, Docker (for test DBs)
- LLVM/Clang for building wasi-libc
- TinyGo build requires LLVM 17+
- Wasmtime builds take ~5-10 minutes. Cache aggressively.
- Bun installs fast (`curl -fsSL https://bun.sh/install | bash`). Use for Domain 6 and as claude-mem dependency.
- `jco` (from `@bytecodealliance/jco`) is needed for both Domain 4 and Domain 6.

### When you get stuck
- If a Wasmtime API is unclear: check `wasmtime/crates/wasi/src/` for how stock WASI host functions are implemented. Your host functions follow the same pattern.
- If a wasi-libc function is unclear: check musl source. wasi-libc is musl with a WASI bottom-half.
- If TinyGo WASI is unclear: check `src/runtime/runtime_wasip2.go` and `src/syscall/syscall_wasip2.go`.
- If ComponentizeJS is unclear: check the StarlingMonkey runtime source.
- If Bun WASI is unclear: check Bun's `node:wasi` implementation (based on wasmer-js). For component model, use `jco` — Bun doesn't have native component model support yet.

### What NOT to do
- Do NOT attempt to implement real threads. Wait for upstream.
- Do NOT fork Wasmtime itself. Use the public embedding API.
- Do NOT rewrite wasi-libc functions. Patch minimally.
- Do NOT try to support Python yet. Phase 2+.
- Do NOT try to make `os/exec` work. It's fundamentally impossible in WASI.
