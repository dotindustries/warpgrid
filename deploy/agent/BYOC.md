# BYOC Agent — Bring Your Own Compute

Run WarpGrid sprites on your infrastructure while using the hosted cloud control plane for management. No need to deploy your own control plane.

```
┌─────────────────────────────┐         ┌──────────────────────────────┐
│  WarpGrid Cloud (Fly.io)    │  gRPC   │  Your Infrastructure         │
│  cloud.warpgrid.dev         │◄────────│  warpd agent + KVM           │
│  - Sprite scheduling        │         │  - Cloud Hypervisor          │
│  - API + Console            │         │  - Sprite VMs                │
│  - State + Metrics          │         │  - NVMe local cache          │
└─────────────────────────────┘         └──────────────────────────────┘
```

## Prerequisites

- Linux x86_64 host with KVM support (`/dev/kvm`)
- Network access to `cloud.warpgrid.dev:50051` (gRPC)
- A WarpGrid cloud account

## Quick Start

### 1. Get an agent token

From the console, or via API:

```bash
curl -X POST https://cloud.warpgrid.dev/api/v1/cloud/agent-tokens \
  -H 'Authorization: Bearer wg_live_...' \
  -H 'Content-Type: application/json' \
  -d '{"name": "prod-node-1"}'
```

Response (token shown once):

```json
{
  "success": true,
  "data": {
    "id": "agt_a1b2c3d4e5f6g7h8",
    "name": "prod-node-1",
    "namespace": "acme",
    "token": "wg_agent_a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6",
    "revoked": false,
    "created_at": 1710000000
  }
}
```

Save the `token` value — it's only shown once.

### 2. Install the agent

#### Option A: Bare metal / VM (systemd)

```bash
curl -fsSL https://get.warpgrid.dev/agent | bash -s -- \
  --token wg_agent_... \
  --control-plane cloud.warpgrid.dev:50051 \
  --region iad
```

This installs `warpd`, Cloud Hypervisor, the sprite kernel and base image, then creates and starts a `warpgrid-agent` systemd service.

#### Option B: Kubernetes (Helm)

For K8s clusters with KVM-capable nodes (bare-metal or nested virt):

```bash
# Label your KVM-capable nodes
kubectl label node <node-name> warpgrid.dev/kvm=true

# Install the agent DaemonSet
helm install warpgrid-agent deploy/helm/warpgrid-agent \
  --set agentToken=wg_agent_... \
  --set controlPlane=cloud.warpgrid.dev:50051 \
  --set region=iad
```

Or with an existing secret:

```bash
kubectl create secret generic warpgrid-agent-creds \
  --from-literal=token=wg_agent_...

helm install warpgrid-agent deploy/helm/warpgrid-agent \
  --set existingSecret=warpgrid-agent-creds \
  --set controlPlane=cloud.warpgrid.dev:50051
```

#### Option C: Manual

```bash
warpd agent \
  --control-plane cloud.warpgrid.dev:50051 \
  --auth-token wg_agent_... \
  --region iad \
  --address $(hostname -I | awk '{print $1}')
```

### 3. Verify

The agent logs its cluster join:

```
INFO warpd: BYOC mode: agent will authenticate with cloud control plane
INFO warpgrid_cluster: joined cluster node_id="node-a1b2c3d4" namespace="acme"
```

Your node now appears in the cloud console and is available for sprite placement.

## Token Management

### List tokens

```bash
curl https://cloud.warpgrid.dev/api/v1/cloud/agent-tokens \
  -H 'Authorization: Bearer wg_live_...'
```

### Revoke a token

Revoked tokens are rejected immediately on the next agent Join or reconnect.

```bash
curl -X POST https://cloud.warpgrid.dev/api/v1/cloud/agent-tokens/agt_abc123/revoke \
  -H 'Authorization: Bearer wg_live_...'
```

## How It Works

1. **Agent authenticates** — sends `wg_agent_*` token on gRPC `Join`
2. **Cloud validates** — checks token hash against `cloud_agent_tokens` table, rejects if revoked
3. **Namespace binding** — cloud injects `namespace=acme` label into the node's metadata
4. **Placement scoping** — sprites for namespace `acme` are only placed on nodes labeled `namespace=acme`
5. **State sync** — agent reports metrics and instance state to cloud via Turso every 30s

## Systemd Management

```bash
systemctl status warpgrid-agent    # Check status
journalctl -u warpgrid-agent -f    # Tail logs
systemctl restart warpgrid-agent   # Restart
systemctl stop warpgrid-agent      # Stop
```

## Uninstall

```bash
systemctl stop warpgrid-agent
systemctl disable warpgrid-agent
rm /etc/systemd/system/warpgrid-agent.service
rm -rf /opt/warpgrid /var/lib/warpgrid
systemctl daemon-reload
```
