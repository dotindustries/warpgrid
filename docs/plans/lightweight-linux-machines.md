# Plan: Lightweight Linux Machines for On-Premise Claude Code

## Problem Statement

WarpGrid currently orchestrates **WebAssembly components** — sandboxed, microsecond-cold-start units ideal for stateless HTTP handlers. But Claude Code requires a **full Linux userspace**: a shell, filesystem, Node.js runtime, git, network access, and persistent workspace state. Wasm components cannot provide this.

We need a second execution primitive — **lightweight Linux machines** ("sprites") — managed by the same WarpGrid control plane, optimized for interactive AI coding sessions on on-premise bare metal.

## Design Principles (Adapted from Fly.io Sprites)

Fly.io's Sprites blog post identifies three architectural decisions that make disposable Linux machines practical at scale. We adapt each for on-premise:

| Fly.io Sprites Decision | WarpGrid Adaptation |
|---|---|
| **No container images** — standard base, pre-pooled empties | **Golden image** — single read-only rootfs with Claude Code pre-installed, pool of warm VMs |
| **Object storage for disks** — JuiceFS + S3 + NVMe cache | **Local object storage** — MinIO/SeaweedFS + JuiceFS + NVMe cache (no cloud dependency) |
| **Inside-out orchestration** — services in VM root namespace, user code in inner container | **Same** — init supervisor in root namespace, Claude Code session in inner namespace |

## Architecture Overview

```
                    ┌──────────────────────────────────┐
                    │     WarpGrid Control Plane        │
                    │  (existing: Raft + REST API)      │
                    │                                   │
                    │  ┌─────────────┐ ┌─────────────┐  │
                    │  │ Wasm        │ │ Sprite      │  │
                    │  │ Scheduler   │ │ Scheduler   │  │
                    │  └─────────────┘ └──────┬──────┘  │
                    └─────────────────────────┼─────────┘
                                              │ gRPC
                    ┌─────────────────────────┼─────────┐
                    │     Agent Node (bare metal)       │
                    │                                   │
                    │  ┌─────────────┐ ┌─────────────┐  │
                    │  │ Wasmtime    │ │ Sprite      │  │
                    │  │ Runtime     │ │ Manager     │  │
                    │  │ (existing)  │ │ (new)       │  │
                    │  └─────────────┘ └──────┬──────┘  │
                    │                         │         │
                    │  ┌──────────────────────┼───────┐ │
                    │  │ Warm Pool            │       │ │
                    │  │  ┌────┐ ┌────┐ ┌────┐│       │ │
                    │  │  │ VM │ │ VM │ │ VM ││       │ │
                    │  │  └────┘ └────┘ └────┘│       │ │
                    │  └──────────────────────────────┘ │
                    │                                   │
                    │  ┌──────────────────────────────┐ │
                    │  │ Object Storage (MinIO)       │ │
                    │  │ + NVMe Cache                 │ │
                    │  └──────────────────────────────┘ │
                    └───────────────────────────────────┘

Inside each Sprite VM:
┌─────────────────────────────────────────────┐
│ Root Namespace (init supervisor)            │
│  ├── sprite-init (PID 1, Rust)              │
│  ├── storage driver (JuiceFS/virtiofs)      │
│  ├── checkpoint/restore agent               │
│  ├── log forwarder                          │
│  ├── service manager (port detection)       │
│  └── metrics reporter                       │
│                                             │
│  ┌───────────────────────────────────────┐  │
│  │ User Namespace (inner container)      │  │
│  │  ├── Claude Code (node/claude)        │  │
│  │  ├── shell (bash/zsh)                 │  │
│  │  ├── git, build tools                 │  │
│  │  └── user project files (/workspace)  │  │
│  └───────────────────────────────────────┘  │
└─────────────────────────────────────────────┘
```

## Implementation Plan

### Phase 1: VM Runtime Foundation

**New crate: `warpgrid-sprite`**

Responsible for creating and managing lightweight Linux VMs on the host. This is the core execution engine, analogous to `warp-runtime` but for Linux VMs instead of Wasm components.

#### 1.1 Hypervisor Backend — Cloud Hypervisor (or Firecracker)

Choose **Cloud Hypervisor** as the primary backend:
- Rust-native, actively maintained, Apache 2.0
- Supports virtio-fs (critical for our storage model)
- API-driven (REST over Unix socket)
- Faster boot than QEMU, more flexible than Firecracker (virtio-fs, hotplug)
- Firecracker as alternative backend behind a trait (it lacks virtio-fs but is simpler)

```rust
// crates/warpgrid-sprite/src/hypervisor.rs

/// Trait abstracting the hypervisor, allowing Cloud Hypervisor
/// or Firecracker backends.
#[async_trait]
pub trait Hypervisor: Send + Sync {
    async fn create_vm(&self, config: VmConfig) -> Result<VmHandle>;
    async fn start_vm(&self, handle: &VmHandle) -> Result<()>;
    async fn stop_vm(&self, handle: &VmHandle) -> Result<()>;
    async fn pause_vm(&self, handle: &VmHandle) -> Result<()>;
    async fn resume_vm(&self, handle: &VmHandle) -> Result<()>;
    async fn destroy_vm(&self, handle: &VmHandle) -> Result<()>;
    async fn vm_status(&self, handle: &VmHandle) -> Result<VmStatus>;
}

pub struct VmConfig {
    pub vcpus: u32,           // default: 2
    pub memory_mb: u32,       // default: 4096
    pub kernel: PathBuf,      // shared vmlinux
    pub rootfs: PathBuf,      // golden image (read-only)
    pub overlay: PathBuf,     // per-VM writable overlay
    pub vsock_cid: u32,       // for host<->guest communication
    pub virtio_fs: Option<VirtioFsMount>,
}
```

#### 1.2 Golden Image

A single read-only ext4/squashfs root filesystem containing:

| Component | Purpose |
|---|---|
| Alpine/Debian minimal base | ~50MB compressed |
| Node.js 22 LTS | Claude Code runtime |
| Claude Code (npm global) | AI coding agent |
| git, openssh-client | Version control |
| build-essential, python3 | Common build deps |
| sprite-init (our binary) | PID 1 supervisor |

Build pipeline: Dockerfile → ext4 image → content-addressed in object store.

The golden image is **immutable and versioned**. Updates ship as new versions; running sprites keep their current version until checkpoint/restore.

#### 1.3 VM Warm Pool

Inspired by Fly.io's pre-pooled empties:

```rust
// crates/warpgrid-sprite/src/pool.rs

pub struct SpritePool {
    warm: VecDeque<WarmSprite>,   // booted, idle, no user state
    config: PoolConfig,
    hypervisor: Arc<dyn Hypervisor>,
}

pub struct PoolConfig {
    pub min_warm: usize,      // keep N warm sprites ready (default: 3)
    pub max_total: usize,     // hard cap per node
    pub boot_timeout: Duration,
    pub idle_sleep_after: Duration,  // auto-sleep after inactivity (default: 10min)
}
```

A background task maintains `min_warm` booted-but-unassigned VMs. When a create request comes in, we pop from the warm pool (instant) rather than boot from scratch (~1-2s).

### Phase 2: Storage Layer

#### 2.1 On-Premise Object Storage

For on-premise deployments (no AWS S3), deploy **MinIO** or **SeaweedFS** alongside WarpGrid:

- MinIO: S3-compatible, single-binary, widely adopted, Apache 2.0
- SeaweedFS: lighter weight, better for many small files

Configured per-cluster:

```toml
# warpgrid.toml — new section
[sprite.storage]
backend = "s3"
endpoint = "http://minio.internal:9000"
bucket = "warpgrid-sprites"
access_key_env = "MINIO_ACCESS_KEY"
secret_key_env = "MINIO_SECRET_KEY"

[sprite.storage.cache]
path = "/dev/nvme0n1p2"       # NVMe partition for read-through cache
max_size_gb = 200
```

#### 2.2 Filesystem Stack (JuiceFS Model)

Following Fly.io's approach exactly:

```
┌──────────────────────────────────────┐
│ User sees: normal ext4-like FS       │
├──────────────────────────────────────┤
│ FUSE / virtio-fs mount               │
├──────────────────────────────────────┤
│ JuiceFS (or custom chunk driver)     │
│  ├── Metadata: SQLite + Litestream   │
│  └── Data chunks: MinIO (S3 API)     │
├──────────────────────────────────────┤
│ NVMe read-through cache              │
│  (sparse, immutable chunks cached)   │
└──────────────────────────────────────┘
```

- **Data plane**: Files are split into fixed-size chunks (default 4MB), stored as objects in MinIO. Chunks are content-addressed (immutable).
- **Metadata plane**: SQLite database mapping paths → chunk lists. Made durable via **Litestream** continuous replication to MinIO.
- **NVMe cache**: Sparse local cache on attached NVMe. Read-through: miss → fetch from MinIO → cache locally. Since chunks are immutable, cache invalidation is trivial (never needed).

This gives us:
- **Durability**: Data lives in object storage, survives host failure
- **Portability**: A sprite's state is a metadata DB + chunk references — migratable to any host
- **Fast checkpoint**: Snapshot metadata DB, flush dirty chunks → done
- **Fast restore**: Download metadata DB, start VM, chunks load on-demand through cache

#### 2.3 Checkpoint/Restore

```rust
// crates/warpgrid-sprite/src/checkpoint.rs

pub struct CheckpointManager {
    storage: Arc<dyn ObjectStore>,
    metadata_db: PathBuf,
}

impl CheckpointManager {
    /// Checkpoint: flush dirty chunks + snapshot metadata.
    /// Returns a checkpoint ID (content-addressed).
    pub async fn checkpoint(&self, sprite_id: &SpriteId) -> Result<CheckpointId>;

    /// Restore: download metadata snapshot, attach to VM,
    /// chunks load on-demand via cache.
    pub async fn restore(&self, checkpoint_id: &CheckpointId) -> Result<SpriteId>;

    /// List available checkpoints for a sprite.
    pub async fn list_checkpoints(&self, sprite_id: &SpriteId) -> Result<Vec<CheckpointInfo>>;
}
```

### Phase 3: Inside-Out Orchestration (sprite-init)

#### 3.1 sprite-init (PID 1)

A small Rust binary that runs as PID 1 inside the VM's root namespace. This is the "inside-out" part — it handles orchestration tasks that would traditionally run on the host.

```rust
// sprite-init/src/main.rs (separate binary, compiled for guest)

/// Responsibilities:
/// 1. Mount filesystems (JuiceFS workspace, proc, sys, dev)
/// 2. Set up inner namespace (user container)
/// 3. Start Claude Code session inside inner namespace
/// 4. Expose vsock API for host communication
/// 5. Monitor activity for auto-sleep
/// 6. Forward logs to host
/// 7. Detect bound ports and register with host proxy
/// 8. Handle checkpoint signals
```

Communication between sprite-init and the host uses **vsock** (VM sockets) — no network configuration needed:

```rust
// Host side: crates/warpgrid-sprite/src/vsock.rs

pub enum SpriteMessage {
    // Host → Guest
    Checkpoint,
    Sleep,
    Wake,
    Exec { command: String, env: Vec<(String, String)> },

    // Guest → Host
    Ready,
    ActivityPing,
    PortBound { port: u16, proto: Protocol },
    LogLine { stream: Stream, line: String },
    MetricsSnapshot { cpu_pct: f32, mem_bytes: u64 },
}
```

#### 3.2 Inner Namespace (User Container)

The user's Claude Code session runs in an inner Linux namespace with:
- Own PID namespace (can't see sprite-init processes)
- Own mount namespace (sees /workspace, /home, /tmp)
- Own network namespace (shares host networking via veth or slirp)
- Root inside the namespace (can install packages, etc.)
- `/workspace` mounted from JuiceFS (persistent across checkpoint/restore)

```rust
// sprite-init/src/container.rs

pub struct InnerContainer {
    pub rootfs: PathBuf,           // bind-mount from golden image + overlay
    pub workspace: PathBuf,        // JuiceFS mount for /workspace
    pub uid_map: UidMapping,       // root-in-namespace maps to unprivileged on host
    pub env: HashMap<String, String>,
    pub entrypoint: Vec<String>,   // ["claude", "--dangerously-skip-permissions"]
}
```

#### 3.3 Auto-Sleep/Wake

Sprites auto-sleep after configurable inactivity (default 10 minutes):

1. **sprite-init** tracks last activity (tty input, file write, network traffic)
2. After idle timeout, sends `ActivityTimeout` to host via vsock
3. Host pauses VM (ACPI S3 or hypervisor pause) — costs near-zero resources
4. On incoming connection (SSH/HTTP) or API wake call, host resumes VM
5. Resume takes <500ms (memory is preserved, no re-boot)

For **deep sleep** (longer inactivity, default 1 hour):
1. Full checkpoint to object storage
2. VM destroyed, resources freed completely
3. Wake requires restore from checkpoint (~2-5s)

### Phase 4: WarpGrid Integration

#### 4.1 New Data Models

Extend `warpgrid-state` with sprite-specific models:

```rust
// crates/warpgrid-state/src/models.rs (additions)

pub struct SpriteSpec {
    pub id: SpriteId,
    pub owner: String,               // user/team identifier
    pub name: String,                 // human-friendly name
    pub image_version: String,        // golden image version
    pub resources: SpriteResources,
    pub storage_url: String,          // object store path for this sprite's data
    pub checkpoint_id: Option<CheckpointId>,
    pub status: SpriteStatus,
    pub node_id: Option<NodeId>,      // which agent node it's on (None if sleeping)
    pub created_at: u64,
    pub last_active_at: u64,
}

pub enum SpriteStatus {
    Creating,
    Running,
    Paused,        // light sleep — VM memory preserved
    Sleeping,      // deep sleep — checkpointed to object store, VM destroyed
    Restoring,
    Failed,
}

pub struct SpriteResources {
    pub vcpus: u32,          // default 2
    pub memory_mb: u32,      // default 4096
    pub disk_gb: u32,        // default 100 (virtual, backed by object store)
}
```

#### 4.2 API Endpoints

Add to `warpgrid-api`:

| Method | Path | Description |
|---|---|---|
| POST | `/api/v1/sprites` | Create a new sprite |
| GET | `/api/v1/sprites` | List sprites |
| GET | `/api/v1/sprites/:id` | Get sprite details |
| DELETE | `/api/v1/sprites/:id` | Destroy a sprite |
| POST | `/api/v1/sprites/:id/wake` | Wake a sleeping sprite |
| POST | `/api/v1/sprites/:id/sleep` | Force sleep |
| POST | `/api/v1/sprites/:id/checkpoint` | Create checkpoint |
| POST | `/api/v1/sprites/:id/restore` | Restore from checkpoint |
| GET | `/api/v1/sprites/:id/checkpoints` | List checkpoints |
| POST | `/api/v1/sprites/:id/exec` | Execute command in sprite |
| GET | `/api/v1/sprites/:id/logs` | Stream logs |

#### 4.3 Placement & Scheduling

Extend `warpgrid-placement` to handle sprite placement:

- Sprites require more resources than Wasm instances (GBs vs MBs)
- Placement must account for NVMe cache locality (prefer placing a sprite on a node that has its chunks cached)
- Bin-packing treats sprites and Wasm instances as fungible resource consumers on the same nodes
- On wake-from-deep-sleep, prefer the node that last ran this sprite (warm cache)

#### 4.4 Dashboard

Add a sprites panel to `warpgrid-dashboard`:
- List of sprites with status (running/paused/sleeping)
- Resource usage per sprite (CPU, memory, disk)
- Quick actions: wake, sleep, checkpoint, destroy
- Terminal access (WebSocket → vsock → sprite shell)

### Phase 5: Claude Code Integration

#### 5.1 Pre-configured Claude Code Environment

The golden image includes:
- Claude Code installed globally via npm
- Pre-authenticated API key injection via environment variable (set at sprite create time)
- `--dangerously-skip-permissions` mode (isolated VM, no risk)
- Aggressive checkpoint hooks: Claude Code's `PostToolUse` hook triggers workspace checkpoint

#### 5.2 Session Management

```toml
# Sprite-level config, passed at create time
[claude]
api_key_env = "ANTHROPIC_API_KEY"    # injected into inner namespace
model = "claude-sonnet-4-20250514"
permissions = "dangerously-skip"
auto_checkpoint = true                # checkpoint after significant changes
checkpoint_interval = "5m"            # periodic checkpoint
```

#### 5.3 Multi-Sprite Workflow

For teams, WarpGrid manages a fleet of sprites:
- Each developer gets their own sprite (or multiple)
- Sprites can be templated: "create me a sprite with repo X cloned and deps installed"
- Checkpoint sharing: snapshot a configured sprite, restore copies for the team
- Cost tracking: per-sprite resource accounting via `warpgrid-metrics`

## New Crates Summary

| Crate | Responsibility |
|---|---|
| `warpgrid-sprite` | VM lifecycle, hypervisor abstraction, warm pool, vsock communication |
| `warpgrid-sprite-storage` | JuiceFS/chunk driver, object store client, NVMe cache, checkpoint/restore |
| `sprite-init` | Guest-side PID 1 binary: namespace setup, activity tracking, log forwarding |

## Implementation Order

| Step | What | Depends On | Estimated Complexity |
|---|---|---|---|
| 1 | `warpgrid-sprite` crate: hypervisor trait + Cloud Hypervisor backend | — | High |
| 2 | Golden image build pipeline (Dockerfile → ext4) | — | Medium |
| 3 | `sprite-init` guest binary: basic PID 1 + namespace setup | Step 2 | High |
| 4 | vsock host↔guest communication protocol | Steps 1, 3 | Medium |
| 5 | `warpgrid-sprite-storage`: MinIO client + JuiceFS integration | — | High |
| 6 | NVMe read-through cache layer | Step 5 | Medium |
| 7 | Checkpoint/restore via metadata snapshots | Steps 5, 6 | Medium |
| 8 | Warm pool manager | Steps 1, 2 | Medium |
| 9 | Auto-sleep/wake (pause + deep sleep) | Steps 4, 7, 8 | Medium |
| 10 | State models + API endpoints in warpgrid-api | Steps 1-9 | Medium |
| 11 | Placement engine extension for sprites | Step 10 | Low |
| 12 | Dashboard panel | Step 10 | Low |
| 13 | Claude Code golden image + session hooks | Steps 2, 3 | Low |

## Key Technical Decisions

### Why Cloud Hypervisor over Firecracker?
Firecracker lacks virtio-fs, which is critical for our JuiceFS mount strategy. Cloud Hypervisor supports virtio-fs natively, is also Rust-native, and has a similar API-driven model. We define a `Hypervisor` trait so Firecracker can be added later for environments that prefer it (using 9p or block-device overlay instead of virtio-fs).

### Why MinIO for on-premise?
On-premise means no AWS. MinIO is the de-facto S3-compatible object store for self-hosted infrastructure. Single binary, battle-tested, trivial to operate. SeaweedFS is a lighter alternative if MinIO's resource footprint is too high.

### Why JuiceFS over a custom chunk driver?
JuiceFS already implements the exact model Fly.io describes (and says they use). It supports S3 backends, SQLite metadata, and FUSE mounts. Starting with JuiceFS (with potential custom modifications) is faster than building from scratch. We can replace the internals later if needed.

### Why vsock over virtio-serial or network?
vsock provides socket-like communication without requiring network configuration. It's faster than virtio-serial, supports multiple connections, and works even when guest networking is down. It's what Firecracker and Cloud Hypervisor both use for host↔guest communication.

### Why not containers (Docker/Podman) instead of VMs?
Claude Code with `--dangerously-skip-permissions` has root-level access and runs arbitrary commands. On-premise customers need **hard isolation** — a compromised Claude Code session must not be able to affect other sessions or the host. VMs provide this via hardware virtualization. Containers share the host kernel and are escapable.

## Security Considerations

1. **VM isolation**: Each sprite is a separate KVM VM — hardware-enforced boundary
2. **Inner namespace**: User code runs in a further-restricted namespace inside the VM
3. **No ambient network access**: Sprites get network via explicit proxy rules (egress allowlists configurable per-sprite)
4. **API key injection**: Anthropic API keys injected via vsock at create time, never written to disk in plaintext
5. **Checkpoint encryption**: Checkpoint data in object storage encrypted at rest (MinIO server-side encryption)
6. **Audit logging**: All sprite lifecycle events logged to control plane

## Resource Estimates (per node)

| Resource | Per Sprite (running) | Per Sprite (paused) | Per Sprite (sleeping) |
|---|---|---|---|
| CPU | 2 vCPUs | 0 | 0 |
| Memory | 4 GB | 4 GB (preserved) | 0 |
| NVMe cache | ~1-10 GB | ~1-10 GB | 0 (evictable) |
| Object storage | N/A (on shared MinIO) | N/A | Actual data size |

A 64-core, 256GB RAM bare-metal node can run ~50 active sprites or ~60 paused sprites concurrently, with hundreds sleeping in object storage.
