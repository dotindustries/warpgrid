# Introduction

WarpGrid is a Wasm-native cluster orchestrator for bare metal. It treats WebAssembly
components as the first-class unit of deployment -- no containers, no Docker, no
Kubernetes. One static binary per node, capability-based security by default.

## Why WarpGrid?

Traditional container orchestrators carry enormous complexity: container images,
overlay networks, storage drivers, and a sprawling control plane. WarpGrid replaces
all of that with a single daemon (`warpd`) that runs WebAssembly components directly
on the host.

**Key advantages:**

- **Instant cold starts.** Wasm modules initialize in microseconds, not seconds.
  There are no layers to unpack and no kernel to boot.
- **Tiny footprint.** A compiled Wasm component is typically 1-10 MB. Compare that
  to multi-hundred-megabyte container images.
- **Capability-based security.** Components only get the host APIs they declare in
  their manifest. No ambient authority, no privilege escalation surface.
- **Polyglot by design.** Write your service in Rust, Go, or TypeScript/Bun. WarpGrid
  compiles each to the same `wasm32-wasip2` target and deploys identically.
- **Edge-native.** Deploy to regions worldwide. WarpGrid's distributed placement
  engine schedules workloads close to users.

## Supported Languages

| Language   | Template       | Runtime       |
|------------|----------------|---------------|
| Bun (TS)   | `bun`          | Bun polyfills |
| Go         | `async-go`     | TinyGo WASI   |
| Rust       | `async-rust`   | `no_std` WASI |
| TypeScript | `async-ts`     | JS WASI       |

## What You Will Need

- The `warp` CLI (ships as a single binary, built from `crates/warp-cli`)
- A supported language toolchain (Bun, Go, or Rust)
- An account on WarpGrid Cloud, or a self-hosted `warpd` instance

## Project Layout

A typical WarpGrid project looks like this:

```
my-api/
  warp.toml        # Project manifest
  src/
    handler.ts     # Your application code
  package.json     # Language-specific deps (Bun example)
```

The `warp.toml` file tells WarpGrid how to build, run, and scale your service.
See the [warp.toml specification](../reference/warp-toml.md) for every available field.

## Next Steps

- Follow the [Quickstart](quickstart.md) to deploy your first service in five minutes.
- Read [How It Works](how-it-works.md) to understand the Wasm component model and
  WASI shims that power WarpGrid under the hood.
