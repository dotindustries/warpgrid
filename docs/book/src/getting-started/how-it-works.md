# How It Works

WarpGrid replaces the traditional container stack with WebAssembly components.
This page explains the key layers.

## Wasm Components

Every WarpGrid deployment is a **WebAssembly component** compiled to the
`wasm32-wasip2` target. A component is a single `.wasm` file that:

- Contains your application code and all its dependencies
- Declares the host capabilities it needs via WIT (Wasm Interface Types)
- Runs inside a sandboxed Wasmtime instance with zero ambient authority

The component model means your Rust `no_std` handler, your Go `net/http` server,
and your Bun `fetch` listener all compile to the same portable binary format.
WarpGrid does not care which language you used.

## No Containers

Traditional platforms build an OCI image (FROM, COPY, RUN), push it to a registry,
pull it on the target node, and unpack filesystem layers before starting a process.

WarpGrid skips all of that:

| Container workflow              | WarpGrid workflow           |
|---------------------------------|-----------------------------|
| Write Dockerfile                | Write `warp.toml`           |
| `docker build` (minutes)        | `warp pack` (seconds)       |
| Push image to registry (100 MB) | Upload `.wasm` binary (2 MB)|
| Pull + unpack layers            | Load module into Wasmtime   |
| Start Linux process             | Instantiate Wasm component  |

Cold-start time drops from seconds to microseconds. Image size drops from hundreds
of megabytes to single-digit megabytes.

## WASI Shims

WASI (WebAssembly System Interface) provides a portable syscall layer. However,
most real-world applications depend on POSIX behaviors that WASI does not yet
cover. WarpGrid bridges this gap with **shims** -- lightweight host-side
implementations of common system APIs.

Available shims (configured in `warp.toml` under `[shims]`):

| Shim             | What it provides                                       |
|------------------|--------------------------------------------------------|
| `timezone`       | `/etc/localtime` and timezone database access          |
| `dev_urandom`    | `/dev/urandom` for cryptographic randomness            |
| `dns`            | DNS resolution via `getaddrinfo`-compatible interface   |
| `threading`      | Thread-like concurrency via async cooperative scheduling|
| `signals`        | POSIX signal delivery (SIGTERM, SIGINT)                |
| `database_proxy` | Transparent proxy for database connections (PostgreSQL) |

Shims are opt-in. If your handler does not need DNS or a database, those shims
stay disabled and the sandbox surface stays minimal.

## The Build Pipeline

When you run `warp deploy`, the CLI executes these steps:

1. **Read `warp.toml`** -- determines the language, entry point, and build target.
2. **Compile to Wasm** -- invokes the appropriate toolchain:
   - **Bun/TS**: Bundles with Bun, applies WarpGrid polyfills, compiles to
     `wasm32-wasip2` via the `warpgrid-bun` toolchain crate.
   - **Go**: Compiles with TinyGo targeting `wasm32-wasip2`.
   - **Rust**: Compiles with `cargo` targeting `wasm32-wasip2` (typically `no_std`).
3. **Package** -- the resulting `.wasm` binary is hashed (SHA-256) and packaged
   for upload.
4. **Upload** -- the binary is sent to the control plane's deploy endpoint.
5. **Schedule** -- the placement engine selects an edge node, loads the component
   into Wasmtime, and begins serving traffic.

## Edge Deployment

WarpGrid's cloud runs edge agents in multiple regions. When you deploy with
`--region iad`, the placement engine targets nodes in that region. The proxy layer
terminates TLS and routes requests to the closest healthy instance.

Available regions are configured at the platform level (e.g., `iad`, `ams`, `sin`).
Each region runs one or more agent nodes that heartbeat to the control plane and
report capacity.

## Capability-Based Security

A Wasm component starts with **no** host access. It cannot read files, open
sockets, or call system APIs unless the host explicitly grants those capabilities.

WarpGrid uses the WASI component model's capability system:

- The `[shims]` section in `warp.toml` declares which host APIs the component needs.
- At instantiation time, the runtime only links the shims the deployment declared.
- There is no root, no privilege escalation, and no escape from the sandbox.

This is a fundamental improvement over containers, which share the host kernel and
require cgroups and namespaces for isolation.

## The Compatibility Analyzer

Not every application can run as a Wasm component today. The `warp convert analyze`
command scans your project's dependencies and produces a compatibility report:

```bash
warp convert analyze --path .
```

Each dependency gets a verdict:

- **Compatible** -- works natively with WASI P2.
- **Shim Compatible** -- works via WarpGrid's shim layer, no code changes needed.
- **Incompatible** -- requires code changes or an alternative library.
- **Blocked** -- fundamentally incompatible (e.g., raw `fork()` or mmap).

Use the report to plan your migration before writing any code.

## Runtime Architecture

At the node level, `warpd agent` runs:

- **Wasmtime engine** -- executes Wasm components with component-model-async support.
- **Local scheduler** -- manages instance pools per deployment.
- **Health checker** -- polls `/healthz` (or a configured endpoint) at regular intervals.
- **Metrics collector** -- tracks RPS, latency percentiles, memory, and CPU weight.
- **Autoscaler** -- compares metrics against target values and adjusts instance counts.

The control plane (`warpd control-plane`) adds Raft consensus, a REST API, the
placement engine, and the web dashboard.

See the [Architecture](../concepts/architecture.md) page for the full system diagram.
