# WarpGrid

**A Wasm-native cluster orchestrator for bare metal.**

WarpGrid treats WebAssembly components as the first-class unit of deployment — no containers, no Docker, no Kubernetes. One static binary per node. Capability-based security by default.

## Quick Start

### Standalone (single-node)

```bash
cargo build --release --package warpd

# Start the daemon
./target/release/warpd standalone --port 8443 --data-dir /tmp/warpgrid
```

Open the dashboard at **http://localhost:8443/dashboard**.

### Create a deployment

```bash
curl -X POST http://localhost:8443/api/v1/deployments \
  -H "Content-Type: application/json" \
  -d '{
    "id": "default/hello",
    "namespace": "default",
    "name": "hello",
    "source": "file://hello.wasm",
    "trigger": {"type": "http", "port": 8080},
    "instances": {"min": 1, "max": 5},
    "resources": {"memory_bytes": 67108864, "cpu_weight": 100},
    "shims": {"timezone": false, "dev_urandom": false, "dns": false, "signals": false, "database_proxy": false},
    "env": {},
    "created_at": 0, "updated_at": 0
  }'
```

### Multi-node cluster

```bash
# Terminal 1: Start the control plane (Raft consensus + cluster gRPC + REST API)
./target/release/warpd control-plane \
  --api-port 8443 \
  --grpc-port 50051 \
  --data-dir /tmp/warpgrid-cp

# Terminal 2: Start an agent node (joins the cluster)
./target/release/warpd agent \
  --control-plane 127.0.0.1:50051 \
  --address 127.0.0.1 \
  --port 9000 \
  --data-dir /tmp/warpgrid-agent
```

### API endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/deployments` | List all deployments |
| POST | `/api/v1/deployments` | Create a deployment |
| GET | `/api/v1/deployments/:id` | Get deployment details |
| DELETE | `/api/v1/deployments/:id` | Delete a deployment |
| POST | `/api/v1/deployments/:id/scale` | Scale a deployment |
| GET | `/api/v1/deployments/:id/instances` | List instances |
| GET | `/api/v1/deployments/:id/metrics` | Get deployment metrics |
| POST | `/api/v1/deployments/:id/rollout` | Start a rollout |
| GET | `/api/v1/rollouts` | List active rollouts |
| GET | `/api/v1/rollouts/:id` | Get rollout status |
| POST | `/api/v1/rollouts/:id/pause` | Pause a rollout |
| POST | `/api/v1/rollouts/:id/resume` | Resume a rollout |
| GET | `/api/v1/nodes` | List cluster nodes |
| GET | `/metrics` | Prometheus metrics |
| GET | `/dashboard` | Web dashboard |

## Architecture

```
warpd
├── standalone     Single process: API + scheduler + runtime (no Raft)
├── control-plane  Raft consensus, cluster gRPC, REST API, background tasks
└── agent          Joins cluster, local scheduler + Wasm runtime, heartbeats
```

## Project Structure

```
crates/
├── warpd               # Daemon binary (standalone / control-plane / agent)
├── warp-core           # Shared types, config (warp.toml), source resolution
├── warp-analyzer       # Compatibility analyzer (warp convert)
├── warp-pack           # Packaging CLI (warp pack)
├── warp-compat         # Shim layer for POSIX compatibility
├── warp-runtime        # Wasmtime runtime sandbox
├── warp-cli            # Main CLI entry point (warpctl)
├── warpgrid-state      # State store (redb) — deployments, instances, nodes
├── warpgrid-scheduler  # Instance scheduling + distributed placement
├── warpgrid-health     # Health checking and monitoring
├── warpgrid-metrics    # Metrics collection + Prometheus exposition
├── warpgrid-autoscale  # Autoscaler (CPU/memory/RPS policies)
├── warpgrid-api        # REST API (axum) + rollout handlers
├── warpgrid-dashboard  # Server-rendered HTML dashboard
├── warpgrid-cluster    # Cluster membership, gRPC heartbeat, mTLS
├── warpgrid-raft       # Raft consensus (openraft + redb)
├── warpgrid-placement  # Multi-node placement engine (bin-packing, affinity)
├── warpgrid-proxy      # Service mesh: router, DNS, TLS termination
├── warpgrid-rollout    # Rolling / canary / blue-green deployments
└── warpgrid-host       # Wasm host configuration and engine
```

## Roadmap

| Phase | Focus | Status |
|-------|-------|--------|
| **1: Wedge** | Analyzer + packaging CLI | Done |
| **2: Bridge** | Shim layer + curated registry | Done |
| **3: Platform** | Single-node orchestrator (API, scheduler, health, metrics, autoscale, dashboard) | Done |
| **4: Scale** | Multi-node clustering (Raft, placement, proxy, rollouts) | Done |

## Building

```bash
cargo build --release
cargo test --workspace    # ~480 tests
```

## License

Apache 2.0 — see [LICENSE](LICENSE).
