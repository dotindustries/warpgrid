# WarpGrid — A Wasm-Native Orchestrator for Bare Metal Clusters

## Executive Summary

**WarpGrid** is a proposed Kubernetes-replacement orchestrator purpose-built for scheduling, scaling, health-checking, and managing WebAssembly/WASI workloads across bare-metal cluster nodes. Rather than adapting the container model (as projects like Krustlet attempted), WarpGrid treats Wasm modules as the **first-class unit of deployment**, eliminating the Docker image layer entirely and unlocking microsecond cold-starts, sub-megabyte artifacts, and hardware-level density improvements of 10–100x over containers.

---

## Part 1 — Idea Validation

### 1.1 Why Now: The Convergence

| Signal | Status (mid-2025) |
|---|---|
| **WASI Preview 2** | Stable — component model, wasi-http, wasi-keyvalue, wasi-blobstore shipped |
| **WASI 0.2.x+ networking** | wasi-sockets stabilizing; wasi-http covers 90% of backend use cases |
| **Wasm component model** | Canonical ABI stable — enables polyglot linking at the binary level |
| **Language support** | Rust (tier-1), Go (wasip1/wasip2 via TinyGo + mainline Go 1.24+), Zig (native wasm32-wasi target), TypeScript (via Javy, ComponentizeJS, StarlingMonkey) |
| **Fly.io Sprites** | Validates the "lighter-than-container" execution unit thesis |
| **Fermyon Spin / wasmCloud** | Prove Wasm workloads are production-viable but are opinionated PaaS layers, not general orchestrators |
| **Kubernetes fatigue** | Real and measurable — complexity, YAML sprawl, resource overhead of kubelet+containerd+CRI per node |

### 1.2 Competitive Landscape & Differentiation

| Project | What it is | Gap WarpGrid fills |
|---|---|---|
| **Kubernetes + Krustlet** | Wasm as a sidecar to K8s | Krustlet is archived. K8s was never designed for Wasm granularity. |
| **Fermyon Platform / Spin** | PaaS for Spin-framework apps | Locked to Spin SDK. No bare-metal cluster story. Not a general orchestrator. |
| **wasmCloud** | Actor-model Wasm runtime with lattice | Opinionated actor model. No raw TCP/UDP. Not a scheduling orchestrator. |
| **Cosmonic** | Managed wasmCloud | SaaS, not self-hosted bare-metal. |
| **Nomad** | General orchestrator (HCL-based) | Can run Wasm via plugins but treats it as "just another driver." No Wasm-native scheduling. |

**WarpGrid's position**: A **general-purpose, self-hosted, bare-metal orchestrator** where the scheduling primitive is a Wasm component — not a container, not an actor, not a framework-locked module. Think "Kubernetes but the Pod is a Wasm component and every node is 50MB of Rust instead of 800MB of Go + containerd + runc."

### 1.3 Key Technical Risks

| Risk | Severity | Mitigation |
|---|---|---|
| **WASI networking gaps** | Medium | wasi-http handles most backends. For raw TCP/UDP: provide a host-function extension layer (like wasmCloud's capability providers) as a bridge. |
| **Stateful workloads** | High | Wasm is memory-isolated per instance. Provide wasi-keyvalue and wasi-blobstore bindings to external stores. V1 targets stateless; V2 adds state primitives. |
| **GPU / hardware passthrough** | High | Not in WASI scope. Provide host-function bridges for CUDA/ROCm. Mark as V3. |
| **Language ecosystem maturity** | Medium | Rust and Go are ready. TypeScript via ComponentizeJS works. Zig's wasm32-wasi works but stdlib gaps remain. Python via componentize-py is usable. |
| **Debugging / observability** | Medium | WASI logging is basic. Build first-class OpenTelemetry host-function injection from day 1. |
| **Cold-start at scale** | Low | Wasm cold starts are ~1ms with Wasmtime's pre-compiled modules. This is a *strength*, not a risk. |
| **Adoption** | High | The biggest risk. Mitigate by supporting existing OCI + Wasm artifacts and providing a `warpctl pack` toolchain that feels familiar. |

### 1.4 Validation Verdict

**Strong proceed.** The gap between "Wasm is clearly the future compute primitive" and "there's no Kubernetes-equivalent orchestrator for it" is real, growing, and not being filled by any current project. The timing is right: WASI Preview 2 provides enough surface area, and language toolchains have matured enough that a multi-language packaging story is viable.

---

## Part 2 — Architecture Overview

```
┌─────────────────────────────────────────────────────────┐
│                    CONTROL PLANE                         │
│  ┌──────────┐  ┌──────────┐  ┌──────────────────────┐  │
│  │ API      │  │ Scheduler│  │ Controller Manager    │  │
│  │ Server   │  │          │  │ (scale/heal/update)   │  │
│  │ (gRPC +  │  │ (bin-    │  │                       │  │
│  │  REST)   │  │  packing │  │ ┌───────┐ ┌────────┐ │  │
│  │          │  │  + affin- │  │ │Scaler │ │Updater │ │  │
│  │          │  │  ity)    │  │ └───────┘ └────────┘ │  │
│  └────┬─────┘  └────┬─────┘  └──────────┬──────────┘  │
│       │              │                    │              │
│  ┌────┴──────────────┴────────────────────┴──────────┐  │
│  │              Cluster State Store                   │  │
│  │         (embedded: redb or sled + Raft)            │  │
│  └────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
              │ gRPC (mTLS)          │
   ┌──────────┴──────────┐  ┌───────┴────────────┐
   │     NODE AGENT       │  │     NODE AGENT      │
   │  ┌────────────────┐  │  │  ┌────────────────┐ │
   │  │  Wasm Runtime   │  │  │  │  Wasm Runtime  │ │
   │  │  (Wasmtime)     │  │  │  │  (Wasmtime)    │ │
   │  │  ┌────┐ ┌────┐  │  │  │  │  ┌────┐       │ │
   │  │  │ M1 │ │ M2 │  │  │  │  │  │ M3 │       │ │
   │  │  └────┘ └────┘  │  │  │  │  └────┘       │ │
   │  └────────────────┘  │  │  └────────────────┘ │
   │  ┌────────────────┐  │  │  ┌────────────────┐ │
   │  │ Network Proxy   │  │  │  │ Network Proxy  │ │
   │  │ (L4/L7)        │  │  │  │ (L4/L7)       │ │
   │  └────────────────┘  │  │  └────────────────┘ │
   │  ┌────────────────┐  │  │                      │
   │  │ OTel Collector  │  │  │                      │
   │  └────────────────┘  │  │                      │
   └──────────────────────┘  └──────────────────────┘
```

### Core Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| **Control plane language** | Rust | Memory safety without GC, excellent async (tokio), Wasmtime is Rust-native. Eliminates the "orchestrator itself eats 2GB RAM" problem. |
| **Node agent language** | Rust (same binary) | Single binary for agent + runtime. Target: <30MB static binary per node. |
| **CLI + SDK tooling** | Rust core, with TypeScript and Go client libraries | Rust for `warpctl` CLI. TS/Go for developer ergonomics. |
| **Wasm runtime** | Wasmtime (default), pluggable (Wasmer, WasmEdge) | Wasmtime has best WASI P2 support, Cranelift JIT, and AOT compilation. |
| **State store** | Embedded (redb) + Raft consensus | No etcd dependency. Nodes form a Raft group. Radical simplicity — single binary, zero external deps. |
| **Networking** | Userspace L4/L7 proxy per node (Rust, hyper-based) | Replaces kube-proxy + Envoy. Wasm modules get virtual endpoints; proxy routes. |
| **Packaging format** | OCI-compatible Wasm artifacts | Leverage existing registries (ghcr.io, Docker Hub, etc.) using the OCI wasm artifact spec. |

---

## Part 3 — The Packaging Layer: `warp pack`

### 3.1 Concept

A CLI tool that compiles source code to a WASI component, bundles metadata, and pushes an OCI artifact to any registry.

```
$ warp pack --lang rust --entry src/main.rs
  ✓ Compiled to wasm32-wasip2 (1.2 MB)
  ✓ Bundled as OCI artifact: my-app:v1.0.0

$ warp push registry.example.com/my-app:v1.0.0
  ✓ Pushed (1.2 MB, sha256:abc123...)
```

### 3.2 Language Support Matrix (Priority Order)

| Language | Compiler Target | Component Model | Status | Phase |
|---|---|---|---|---|
| **Rust** | `wasm32-wasip2` (native) | `cargo-component` | Production-ready | Phase 1 |
| **Go** | `GOARCH=wasm GOOS=wasip1` (Go 1.24+) / TinyGo wasip2 | `wasm-tools compose` | Functional, some stdlib gaps | Phase 1 |
| **TypeScript** | ComponentizeJS / StarlingMonkey | Built-in | Works for HTTP handlers | Phase 1 |
| **Zig** | `zig build -Dtarget=wasm32-wasi` | Manual WIT bindings | Works, needs WIT tooling | Phase 2 |
| **Python** | `componentize-py` | Built-in | Works, larger modules (~10MB) | Phase 2 |
| **C/C++** | `wasi-sdk` | `wit-bindgen` | Works | Phase 3 |

### 3.3 Artifact Manifest (`warp.toml`)

```toml
[package]
name = "my-api"
version = "1.0.0"
description = "My HTTP API service"

[build]
lang = "rust"                     # rust | go | typescript | zig | python
entry = "src/main.rs"
target = "wasip2"

[runtime]
trigger = "http"                  # http | schedule | queue | cli
min_instances = 2
max_instances = 100

[runtime.resources]
memory_limit = "128MB"            # Wasm linear memory cap
cpu_weight = 100                  # Relative scheduling weight

[runtime.scaling]
metric = "request_concurrency"    # request_concurrency | cpu | custom
target_value = 50
scale_up_window = "10s"
scale_down_window = "60s"

[capabilities]
http_outbound = ["api.stripe.com", "*.internal.svc"]
keyvalue = "default"
blobstore = "s3-main"

[health]
endpoint = "/healthz"
interval = "5s"
timeout = "2s"
unhealthy_threshold = 3
```

---

## Part 4 — Implementation Phases

### Phase 1: Foundation (Months 1–4)
**Goal**: Single-node orchestrator that can schedule, run, and health-check Wasm modules.

#### Milestone 1.1 — Runtime Sandbox (Weeks 1–4)
- Embed Wasmtime with WASI Preview 2 support
- Implement module loading: from local `.wasm` files and OCI registry pull
- AOT compilation cache (Cranelift pre-compilation on first load)
- Memory limits enforcement via Wasmtime's `StoreLimiter`
- Wasm module lifecycle: instantiate → run → trap handling → cleanup
- **Deliverable**: `warpd` binary that can run a single Wasm HTTP handler and route traffic to it

#### Milestone 1.2 — Packaging CLI (Weeks 3–6)
- `warp pack` for Rust (cargo-component integration)
- `warp pack` for Go (TinyGo wasip2 or Go 1.24+ with post-processing)
- `warp pack` for TypeScript (ComponentizeJS wrapper)
- OCI artifact push/pull to standard registries
- `warp.toml` manifest parser and validator
- **Deliverable**: Developer can `warp pack && warp push` in Rust, Go, and TypeScript

#### Milestone 1.3 — Single-Node Scheduler (Weeks 5–8)
- Local scheduler: accept deployment specs, schedule modules to local runtime
- Instance pool management: maintain min_instances, cap at max_instances
- HTTP trigger: inbound request → pick instance from pool → invoke → return
- Basic round-robin load balancing across instances
- **Deliverable**: Single `warpd` node runs multiple Wasm services, routes HTTP traffic

#### Milestone 1.4 — Health Checking & Self-Healing (Weeks 7–10)
- HTTP health check probes against configured endpoints
- Unhealthy instance replacement (kill + re-instantiate)
- Readiness vs. liveness distinction
- Restart backoff (exponential, capped)
- **Deliverable**: Unhealthy instances are automatically detected and replaced

#### Milestone 1.5 — Observability Baseline (Weeks 8–12)
- Inject WASI-logging host functions → structured log capture
- OpenTelemetry trace context propagation through HTTP proxy
- Per-module metrics: request count, latency p50/p95/p99, error rate, memory usage
- Prometheus-compatible `/metrics` endpoint on the node agent
- **Deliverable**: Full request traces and metrics from Wasm workloads

#### Milestone 1.6 — Basic Autoscaling (Weeks 10–14)
- Metrics-driven scaling: request concurrency → target value → scale up/down
- Scale-up: pre-instantiate (Wasm cold start is ~1ms but pre-warming avoids even that)
- Scale-down: grace period, drain connections, then deallocate
- Scale-to-zero support (with fast wake-on-request)
- **Deliverable**: HTTP load → automatic instance count adjustment, including scale-to-zero

---

### Phase 2: Multi-Node Clustering (Months 4–8)
**Goal**: Multiple bare-metal nodes form a cluster with consensus, distributed scheduling, and service discovery.

#### Milestone 2.1 — Node Agent & Cluster Join (Weeks 14–18)
- Node agent (`warpd --agent`) that registers with control plane
- Node heartbeat, resource reporting (CPU, memory, available Wasm slots)
- mTLS bootstrap: auto-generated CA on control plane init, CSR flow for node join
- Node labeling and tainting (e.g., `gpu=true`, `region=us-east`)
- **Deliverable**: N nodes join a cluster, visible in `warp nodes list`

#### Milestone 2.2 — Raft Consensus State Store (Weeks 16–20)
- Embedded Raft (using `openraft` crate) for control plane HA
- State: deployments, node registry, service endpoints, scaling configs
- `redb` as the on-disk state machine backend
- Snapshotting + log compaction
- 3-node or 5-node control plane quorum
- **Deliverable**: Control plane survives single-node failure; state is consistent

#### Milestone 2.3 — Distributed Scheduler (Weeks 18–22)
- Bin-packing scheduler: place modules on nodes based on available memory + CPU weight
- Affinity / anti-affinity rules (co-locate or spread)
- Constraint-based placement (node labels, taints, tolerations)
- Preemption: lower-priority modules evicted if higher-priority needs space
- Rescheduling on node failure (detected via heartbeat timeout)
- **Deliverable**: Modules are intelligently distributed across the cluster

#### Milestone 2.4 — Service Mesh & Discovery (Weeks 20–26)
- Internal DNS: `<service>.<namespace>.warp.local` resolution
- Per-node L4/L7 proxy (Rust, built on `hyper` + `tokio`)
- Service-to-service routing without sidecar overhead (proxy is part of node agent)
- Inbound ingress: TLS termination, host-based routing, path-based routing
- Connection draining on module updates
- **Deliverable**: Services discover and communicate with each other across nodes

#### Milestone 2.5 — Rolling Updates & Canary (Weeks 24–30)
- Rolling update strategy: configurable batch size, health gate between batches
- Canary deployments: route N% of traffic to new version, promote or rollback
- Blue-green: instant switch with rollback capability
- Automatic rollback on health check failure during rollout
- **Deliverable**: Zero-downtime deployments with automatic rollback

---

### Phase 3: Production Hardening (Months 8–12)
**Goal**: Security, multi-tenancy, advanced scaling, and ecosystem integrations.

#### Milestone 3.1 — Security & Multi-Tenancy (Weeks 30–36)
- Namespace isolation (capability-based, not network-based — Wasm is already sandboxed)
- Per-namespace resource quotas (total memory, total instances)
- RBAC for API access (service accounts, roles, role bindings)
- Capability allowlists: each module declares what host functions it needs; denied by default
- Module signing and verification (Sigstore/cosign integration)
- Audit logging for all control plane operations
- **Deliverable**: Multi-team cluster with strong isolation guarantees

#### Milestone 3.2 — Advanced Autoscaling (Weeks 34–38)
- Custom metrics scaling (push metrics from Wasm → scaling decisions)
- Predictive scaling: time-series forecasting on historical load patterns
- Cluster autoscaling: signal to external provisioner to add/remove bare-metal nodes
- KEDA-style event-driven scaling (queue depth, cron schedules)
- **Deliverable**: Scaling that's smarter than reactive threshold-based

#### Milestone 3.3 — Stateful Workload Support (Weeks 36–42)
- wasi-keyvalue provider: backed by Redis, DragonflyDB, or embedded (fjall/redb)
- wasi-blobstore provider: backed by S3, MinIO, or local disk
- Sticky routing: hash-based affinity for stateful sessions
- Volume mounts: mapped directories exposed via WASI filesystem
- **Deliverable**: Stateful applications can run on WarpGrid

#### Milestone 3.4 — Zig & Python Packaging (Weeks 38–42)
- `warp pack --lang zig` with WIT binding generation
- `warp pack --lang python` via componentize-py
- Cross-language component linking (e.g., Rust library composed with TS handler)
- **Deliverable**: 5-language support in the packaging toolchain

#### Milestone 3.5 — CLI, Dashboard & DX (Weeks 40–46)
- `warpctl` CLI: full cluster management (nodes, deployments, logs, exec)
- `warp dev`: local development mode with hot-reload on source change
- Web dashboard: cluster overview, deployment status, live metrics, log tailing
- Terraform / Pulumi provider for infrastructure-as-code
- GitHub Actions integration for CI/CD
- **Deliverable**: Developer experience competitive with modern PaaS

---

### Phase 4: Ecosystem & Advanced Features (Months 12–18)
**Goal**: Become a viable Kubernetes alternative for production workloads.

#### Milestone 4.1 — GPU & Hardware Passthrough
- Host-function bridge for CUDA / ROCm (Wasm module calls host function → host dispatches to GPU)
- WASI-nn integration for ML inference workloads
- Hardware topology-aware scheduling

#### Milestone 4.2 — Edge & Hybrid Deployment
- Lightweight edge agent (ARM64 support, <10MB binary)
- Hub-and-spoke topology: central control plane, edge node agents
- Offline resilience: edge nodes continue operating during network partition

#### Milestone 4.3 — Migration Tooling
- `warp migrate` — analyze a Kubernetes deployment and generate WarpGrid equivalents
- Docker-to-Wasm conversion helper (for compatible workloads)
- Helm chart → warp.toml transpiler

#### Milestone 4.4 — Plugin & Extension System
- Custom scheduler plugins (written in Wasm themselves)
- Custom capability providers (host function extensions)
- Webhook-based admission control

---

## Part 5 — Technology Stack Summary

| Component | Technology | Notes |
|---|---|---|
| **Core runtime** | Rust + Wasmtime | Single static binary, ~25-30MB |
| **Async I/O** | tokio | Proven, battle-tested |
| **Consensus** | openraft + redb | No etcd. Embedded Raft. |
| **HTTP proxy** | hyper + rustls | L4/L7, TLS termination |
| **gRPC** | tonic | Control plane ↔ node agent |
| **REST API** | axum | External API for CLI/dashboard |
| **CLI** | clap (Rust) | `warpctl` |
| **Packaging** | cargo-component, TinyGo, ComponentizeJS, wasm-tools | Per-language compilation |
| **OCI registry** | oci-distribution crate | Push/pull Wasm artifacts |
| **Observability** | opentelemetry-rust + Prometheus exposition | Traces, metrics, logs |
| **Dashboard** | TypeScript + React (or Leptos/HTMX) | Web UI |
| **Client SDKs** | TypeScript (npm), Go (module) | For programmatic access |

---

## Part 6 — Resource Estimates

### Team Composition (Ideal)

| Role | Count | Focus |
|---|---|---|
| Rust systems engineer | 3 | Runtime, scheduler, networking, consensus |
| Wasm toolchain engineer | 1 | Packaging CLI, language integrations, component model |
| Platform / DX engineer | 1 | CLI, dashboard, docs, SDK |
| SRE / test engineer | 1 | Integration tests, chaos testing, benchmarks |

**Minimum viable team**: 3 strong Rust engineers who can flex across areas.

### Rough Timeline to Usable Product

| Milestone | ETA | What you can demo |
|---|---|---|
| Single-node MVP | Month 3 | Rust/Go/TS app running on one node, autoscaling, health checks |
| Multi-node alpha | Month 7 | 3-node cluster, distributed scheduling, service mesh |
| Production beta | Month 12 | Multi-tenant, secure, observable, 5 languages |
| GA-ready | Month 18 | Migration tooling, edge support, plugin system |

---

## Part 7 — Why This Wins

1. **Radical simplicity**: One static binary per node. No containerd, no runc, no CRI, no CNI, no CSI, no etcd, no kubelet. Just `warpd`.

2. **Density**: A node that runs 50 containers today can run 5,000+ Wasm instances. Each instance uses ~1-10MB vs ~50-500MB for a container.

3. **Speed**: Cold start in 1-5ms (vs 1-10s for containers). Scale-to-zero is actually viable.

4. **Security by default**: Wasm's sandbox is capability-based. A module can't access the filesystem, network, or anything unless explicitly granted. This is a *better security model than containers* without needing seccomp, AppArmor, or SELinux.

5. **Polyglot without pain**: The component model means a Rust library can be composed with a TypeScript handler at the binary level. No FFI, no gRPC between languages — just linked Wasm components.

6. **The migration exists**: Developers don't rewrite — they recompile. Rust, Go, and Zig compile to WASI targets with minimal changes. TypeScript runs via embedded engines.

---

## Appendix A — Open Questions to Resolve

1. **Database drivers**: Most SQL drivers use raw TCP sockets. Until wasi-sockets is fully stable, do we provide host-function database proxies (like PlanetScale's proxy model) or wait?
   - *Recommendation*: Provide a `wasi-sql` host function that proxies to real databases. Ship adapters for Postgres, MySQL, SQLite.

2. **Long-running processes**: WASI is request-oriented by nature. How do we handle background workers, queue consumers, streaming?
   - *Recommendation*: Support multiple trigger types — `http`, `schedule` (cron), `queue` (pull-based), `stream` (long-lived). The runtime manages the lifecycle per type.

3. **Local development story**: Developers need to test locally without a cluster.
   - *Recommendation*: `warp dev` runs a single-node WarpGrid locally with hot reload. Feels like `wrangler dev` or `spin up`.

4. **Compatibility with existing K8s tooling**: Should we support any K8s API compatibility?
   - *Recommendation*: No. Clean break. Provide migration tooling instead. K8s API compatibility would compromise the design.

5. **Licensing model**: Open source core + enterprise features? Fully open?
   - *Recommendation*: Apache 2.0 for the core orchestrator. Enterprise features (SSO, advanced RBAC, audit, multi-region) under a BSL or commercial license.
