# WarpGrid

**A Wasm-native cluster orchestrator for bare metal.**

WarpGrid treats WebAssembly components as the first-class unit of deployment — no containers, no Docker, no Kubernetes. One static binary per node. Capability-based security by default.

> ⚠️ **Early development.** Phase 1 (Wedge) is in progress: the compatibility analyzer and packaging CLI.

## Quick Start

```bash
# Analyze a Dockerfile for Wasm compatibility
warp convert analyze --dockerfile ./Dockerfile

# Package a Rust service as a Wasm component
warp pack --lang rust --entry src/main.rs

# Package and deploy (Phase 3+)
warp deploy my-api --source ./target/wasm/my-api.wasm
```

## Project Structure

```
crates/
├── warp-core        # Shared types, config (warp.toml), source resolution
├── warp-analyzer    # Compatibility analyzer (warp convert)
├── warp-pack        # Packaging CLI (warp pack)
├── warp-compat      # Shim layer for POSIX compatibility
├── warp-runtime     # Wasmtime runtime sandbox (Phase 3)
└── warp-cli         # Main CLI entry point (warpctl)
compat-db/           # Community compatibility database
docs/                # Specification and guides
```

## Roadmap

| Phase | Focus | Status |
|-------|-------|--------|
| **1: Wedge** | Analyzer + packaging CLI | 🔨 In progress |
| **2: Bridge** | Shim layer + curated registry | Planned |
| **3: Platform** | Single-node orchestrator | Planned |
| **4: Scale** | Multi-node clustering | Planned |

See [docs/SPEC.md](docs/SPEC.md) for the full specification.

## Building

```bash
cargo build --release
```

## License

Apache 2.0 — see [LICENSE](LICENSE).
