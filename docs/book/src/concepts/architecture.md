# Architecture

WarpGrid is built as a set of composable Rust crates that assemble into a single
binary (`warpd`). This page describes the system architecture, from the control
plane to edge agents.

## System Overview

```
                        +---------------------------+
                        |      warp CLI             |
                        |  (login, deploy, status)  |
                        +------------+--------------+
                                     |
                                     | HTTPS / REST
                                     v
                        +---------------------------+
                        |     Control Plane         |
                        |  warpd control-plane      |
                        |                           |
                        |  +---------------------+  |
                        |  |   REST API (axum)    |  |
                        |  +---------------------+  |
                        |  |   Raft Consensus     |  |
                        |  |   (openraft + redb)  |  |
                        |  +---------------------+  |
                        |  |   Placement Engine   |  |
                        |  +---------------------+  |
                        |  |   Rollout Controller |  |
                        |  +---------------------+  |
                        |  |   Dashboard (HTML)   |  |
                        |  +---------------------+  |
                        +---+---+---+---+-----------+
                            |   |   |   |
                   gRPC heartbeat + placement commands
                            |   |   |   |
              +-------------+   |   |   +-----------+
              v                 v   v               v
    +----------------+  +----------------+  +----------------+
    |  Agent Node 1  |  |  Agent Node 2  |  |  Agent Node N  |
    |  (region: iad) |  |  (region: ams) |  |  (region: sin) |
    |                |  |                |  |                |
    |  +-----------+ |  |  +-----------+ |  |  +-----------+ |
    |  | Wasmtime  | |  |  | Wasmtime  | |  |  | Wasmtime  | |
    |  | Runtime   | |  |  | Runtime   | |  |  | Runtime   | |
    |  +-----------+ |  |  +-----------+ |  |  +-----------+ |
    |  | Scheduler | |  |  | Scheduler | |  |  | Scheduler | |
    |  +-----------+ |  |  +-----------+ |  |  +-----------+ |
    |  | Health    | |  |  | Health    | |  |  | Health    | |
    |  +-----------+ |  |  +-----------+ |  |  +-----------+ |
    |  | Metrics   | |  |  | Metrics   | |  |  | Metrics   | |
    |  +-----------+ |  |  +-----------+ |  |  +-----------+ |
    |  | Autoscale | |  |  | Autoscale | |  |  | Autoscale | |
    |  +-----------+ |  |  +-----------+ |  |  +-----------+ |
    |  | Proxy     | |  |  | Proxy     | |  |  | Proxy     | |
    |  +-----------+ |  |  +-----------+ |  |  +-----------+ |
    +----------------+  +----------------+  +----------------+
```

## Daemon Modes

`warpd` runs in one of three modes:

### Standalone

```bash
warpd standalone --port 8443 --data-dir /tmp/warpgrid
```

Single-process mode that combines the API, scheduler, and runtime. No Raft
consensus, no clustering. Suitable for development and small-scale deployments.

### Control Plane

```bash
warpd control-plane \
  --api-port 8443 \
  --grpc-port 50051 \
  --data-dir /tmp/warpgrid-cp
```

Runs the cluster brain: REST API, Raft consensus, placement engine, rollout
controller, and web dashboard. Does not execute Wasm components directly.

### Agent

```bash
warpd agent \
  --control-plane 127.0.0.1:50051 \
  --address 127.0.0.1 \
  --port 9000 \
  --data-dir /tmp/warpgrid-agent
```

Joins the cluster, runs the Wasm runtime, and executes deployments. Heartbeats
to the control plane over gRPC and reports node capacity and instance health.

## Crate Map

The project is organized into focused crates with clear responsibilities:

```
crates/
  warpd                 Daemon binary (standalone / control-plane / agent)
  warp-core             Shared types, warp.toml config, source resolution
  warp-analyzer         Compatibility analyzer (warp convert)
  warp-pack             Packaging CLI (warp pack)
  warp-compat           POSIX shim layer for WASI components
  warp-runtime          Wasmtime sandbox and instance lifecycle
  warp-cli              CLI entry point (warp login, deploy, etc.)
  warpgrid-state        State store (redb) -- deployments, instances, nodes
  warpgrid-scheduler    Local instance scheduling and pool management
  warpgrid-health       Health checking and monitoring
  warpgrid-metrics      Metrics collection and Prometheus exposition
  warpgrid-autoscale    Autoscaler (CPU, memory, RPS, latency policies)
  warpgrid-api          REST API (axum) and rollout handlers
  warpgrid-dashboard    Server-rendered HTML dashboard
  warpgrid-cluster      Cluster membership, gRPC heartbeat, mTLS
  warpgrid-raft         Raft consensus (openraft + redb log/state machine)
  warpgrid-placement    Distributed placement engine (bin-packing, affinity)
  warpgrid-proxy        Service mesh: HTTP router, DNS, TLS termination
  warpgrid-rollout      Rolling / canary / blue-green deployment controller
  warpgrid-host         Wasm host configuration and Wasmtime engine setup
  warpgrid-bun          Bun-to-Wasm compilation toolchain
  warpgrid-async        Async handler runtime for WASI 0.3 components
```

## Data Plane: Turso

WarpGrid Cloud uses [Turso](https://turso.tech) (libSQL) as the data plane for
storing deployment metadata, user accounts, and logs. Turso provides:

- Edge-replicated SQLite databases with low-latency reads worldwide.
- Embedded replicas on agent nodes for offline resilience.
- A single connection string per database (`libsql://...turso.io`).

The control plane writes deployment and user state to Turso. Agent nodes read
their assigned deployments from the nearest Turso edge replica.

## Placement Engine

When a deployment is created or scaled, the placement engine decides which
agent nodes should host its instances. The engine uses:

- **Bin-packing** -- fits instances onto nodes with available capacity, minimizing
  wasted resources.
- **Affinity rules** -- co-locates or separates instances based on deployment
  labels and region preferences.
- **Balance scoring** -- spreads instances across nodes to avoid hotspots.
- **Preemption** -- evicts lower-priority instances when the cluster is at
  capacity and a higher-priority deployment needs resources.

The placement plan is computed on the control plane and sent to agent nodes
via gRPC.

## Raft Consensus

In multi-node mode, the control plane uses Raft (via the `openraft` crate) for
leader election and replicated state. The Raft log is persisted in `redb` (an
embedded key-value store). This ensures:

- Only one control plane node accepts writes at a time.
- State survives node failures and restarts.
- New nodes can join the cluster and catch up from the log.

## Proxy Layer

Each agent node runs a proxy (`warpgrid-proxy`) that handles:

- **HTTP routing** -- maps incoming requests to the correct deployment instance
  based on hostname or path.
- **TLS termination** -- handles certificates for default and custom domains.
- **DNS resolution** -- resolves service names within the mesh for inter-service
  calls.
- **Load balancing** -- distributes requests across healthy instances of a
  deployment.

## Security Model

WarpGrid's security is layered:

1. **Wasm sandbox** -- each component runs in an isolated Wasmtime instance with
   no ambient capabilities. Only explicitly granted shims are linked.
2. **Capability declarations** -- the `[shims]` section in `warp.toml` declares
   what the component can access. The runtime enforces these at instantiation.
3. **mTLS** -- cluster communication between control plane and agents uses
   mutual TLS for authentication and encryption.
4. **API authentication** -- cloud API endpoints require Bearer token
   authentication.
