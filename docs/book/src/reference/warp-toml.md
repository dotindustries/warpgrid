# warp.toml Specification

Every WarpGrid project has a `warp.toml` file at its root. This file declares
how to build, run, and scale your Wasm component.

## Minimal Example

```toml
[package]
name = "my-api"
version = "0.1.0"
```

Only `[package].name` and `[package].version` are required. All other sections
are optional.

## Complete Example

```toml
[package]
name = "my-api"
version = "0.1.0"
description = "A production REST API"

[build]
lang = "bun"
entry = "src/handler.ts"
target = "wasip2"
flags = ["--minify"]

[runtime]
trigger = "http"
min_instances = 2
max_instances = 20

[runtime.resources]
memory_limit = "128MB"
cpu_weight = 200

[runtime.scaling]
metric = "rps"
target_value = 500
scale_up_window = "30s"
scale_down_window = "120s"

[health]
endpoint = "/health"
interval = "5s"
timeout = "2s"
unhealthy_threshold = 3

[shims]
timezone = false
dev_urandom = true
dns = true
threading = "cooperative"
signals = false
database_proxy = false

[env]
APP_NAME = "my-api"
APP_ENV = "production"

[capabilities]
```

---

## `[package]`

Project metadata.

| Field         | Type   | Required | Description                    |
|---------------|--------|----------|--------------------------------|
| `name`        | string | yes      | Deployment name. Must be unique within your namespace. Used as the subdomain in `<name>.<ns>.edge.warpgrid.dev`. |
| `version`     | string | yes      | Semantic version (e.g., `"0.1.0"`). |
| `description` | string | no       | Human-readable description.    |

---

## `[build]`

Build configuration. If omitted, `warp pack` auto-detects the language from
project marker files.

| Field    | Type     | Required | Description                              |
|----------|----------|----------|------------------------------------------|
| `lang`   | string   | yes*     | Build language: `rust`, `go`, `bun`, `js`, `typescript`. Auto-detected if not set. |
| `entry`  | string   | yes*     | Entry point file relative to project root (e.g., `src/handler.ts`, `main.go`, `src/lib.rs`). |
| `target` | string   | no       | Compilation target. Default: `wasip2`.   |
| `flags`  | string[] | no       | Additional compiler flags passed to the toolchain. |

*Required when `[build]` is present.

---

## `[runtime]`

Runtime and deployment configuration.

| Field           | Type   | Required | Description                            |
|-----------------|--------|----------|----------------------------------------|
| `trigger`       | string | no       | Trigger type. Currently only `"http"` is supported. |
| `min_instances` | u32    | no       | Minimum instance count. Set to `0` for scale-to-zero. Default: `1`. |
| `max_instances` | u32    | no       | Maximum instance count. Default: `10`. |

### `[runtime.resources]`

Resource constraints for each instance.

| Field          | Type   | Required | Description                             |
|----------------|--------|----------|-----------------------------------------|
| `memory_limit` | string | no       | Maximum memory (e.g., `"64MB"`, `"256MB"`). |
| `cpu_weight`   | u32    | no       | Relative CPU priority. Higher values get more CPU time under contention. Default: `100`. |

### `[runtime.scaling]`

Autoscaling policy. Requires `min_instances` and `max_instances` to be set.

| Field               | Type   | Required | Description                       |
|---------------------|--------|----------|-----------------------------------|
| `metric`            | string | no       | Metric to scale on: `rps`, `latency_p99`, `error_rate`, `memory`. |
| `target_value`      | u32    | no       | Target value for the metric (e.g., 200 RPS, 500 ms latency). |
| `scale_up_window`   | string | no       | Cooldown between scale-up events (e.g., `"30s"`). |
| `scale_down_window` | string | no       | Cooldown between scale-down events (e.g., `"120s"`). |

---

## `[health]`

Health check configuration. WarpGrid periodically polls the specified endpoint
to determine instance health.

| Field                 | Type   | Required | Description                        |
|-----------------------|--------|----------|------------------------------------|
| `endpoint`            | string | no       | HTTP path to poll (e.g., `"/health"`, `"/healthz"`). |
| `interval`            | string | no       | Time between checks (e.g., `"5s"`). |
| `timeout`             | string | no       | Maximum response time (e.g., `"2s"`). |
| `unhealthy_threshold` | u32    | no       | Consecutive failures before marking instance unhealthy. |

---

## `[shims]`

WASI shim configuration. Each shim enables a host-side capability that bridges
the gap between WASI and POSIX.

| Field            | Type        | Required | Description                          |
|------------------|-------------|----------|--------------------------------------|
| `timezone`       | bool        | no       | Enable timezone database access (`/etc/localtime`). |
| `dev_urandom`    | bool        | no       | Enable `/dev/urandom` for cryptographic randomness. |
| `dns`            | bool        | no       | Enable DNS resolution.               |
| `threading`      | string      | no       | Threading model: `"cooperative"` (async scheduling). |
| `signals`        | bool        | no       | Enable POSIX signal delivery (SIGTERM, SIGINT). |
| `database_proxy` | bool        | no       | Enable transparent database connection proxy. |

All shims default to `false` (disabled). Only enable what your application needs
to keep the sandbox surface minimal.

---

## `[env]`

Environment variables passed to the Wasm component at runtime.

```toml
[env]
APP_NAME = "my-api"
DATABASE_URL = "libsql://mydb.turso.io"
LOG_LEVEL = "info"
```

Keys must be strings. Values must be strings. These are available via standard
environment variable APIs in your language (`process.env`, `os.Getenv`,
`std::env::var`).

---

## `[capabilities]`

Reserved for future WASI capability declarations. Currently accepts arbitrary
key-value pairs that are stored but not enforced.

---

## Language-Specific Notes

### Bun / TypeScript

- `lang = "bun"` uses the WarpGrid Bun polyfill layer (`warpgrid-bun-polyfills`).
- The entry file should export or register a `fetch` event listener.
- `target = "wasip2"` is the only supported target.

### Go

- `lang = "go"` compiles via TinyGo targeting `wasm32-wasip2`.
- Use standard `net/http` patterns in the entry file.
- Pure-Go libraries generally work. cgo libraries are not supported.

### Rust

- `lang = "rust"` compiles with `cargo` targeting `wasm32-wasip2`.
- The entry file should implement the `warpgrid-handler` WIT world.
- Using `#![no_std]` with `dlmalloc` produces the smallest binaries.
