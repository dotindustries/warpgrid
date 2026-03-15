# REST API

WarpGrid exposes a REST API on the control plane (default port 8443). All
endpoints return JSON. The API is served by `warpd standalone` or
`warpd control-plane`.

## Base URL

- Local development: `http://localhost:8443`
- Cloud: `https://api.warpgrid.dev`

## Authentication

Cloud API endpoints under `/api/v1/cloud/` require a Bearer token:

```
Authorization: Bearer wgk_xxxxxxxxxxxxxxxxxxxx
```

Self-hosted cluster endpoints under `/api/v1/` do not require authentication
by default.

---

## Deployments

### List Deployments

```
GET /api/v1/deployments
```

Returns all deployments in the cluster.

**Response:**

```json
[
  {
    "id": "default/my-api",
    "namespace": "default",
    "name": "my-api",
    "source": "file://my-api.wasm",
    "trigger": { "type": "http", "port": 8080 },
    "instances": { "min": 1, "max": 10 },
    "resources": { "memory_bytes": 67108864, "cpu_weight": 100 },
    "shims": {
      "timezone": false,
      "dev_urandom": false,
      "dns": false,
      "signals": false,
      "database_proxy": false
    },
    "env": {},
    "created_at": 1710500000,
    "updated_at": 1710500000
  }
]
```

### Create Deployment

```
POST /api/v1/deployments
Content-Type: application/json
```

**Request body:**

```json
{
  "id": "default/my-api",
  "namespace": "default",
  "name": "my-api",
  "source": "file://my-api.wasm",
  "trigger": { "type": "http", "port": 8080 },
  "instances": { "min": 1, "max": 5 },
  "resources": { "memory_bytes": 67108864, "cpu_weight": 100 },
  "shims": {
    "timezone": false,
    "dev_urandom": false,
    "dns": false,
    "signals": false,
    "database_proxy": false
  },
  "env": {}
}
```

**Response:** the created deployment object (same schema as list).

### Get Deployment

```
GET /api/v1/deployments/:id
```

Returns a single deployment by ID (e.g., `default/my-api`).

### Delete Deployment

```
DELETE /api/v1/deployments/:id
```

Stops all instances and removes the deployment.

**Response:** `204 No Content` on success.

---

## Scaling

### Scale a Deployment

```
POST /api/v1/deployments/:id/scale
Content-Type: application/json
```

**Request body:**

```json
{
  "min": 2,
  "max": 10
}
```

Updates the instance range. The scheduler adjusts instance counts to match.

---

## Instances

### List Instances

```
GET /api/v1/deployments/:id/instances
```

Returns the running instances for a deployment.

**Response:**

```json
[
  {
    "instance_id": "inst-abc123",
    "deployment_id": "default/my-api",
    "node_id": "node-1",
    "status": "running",
    "started_at": 1710500100
  }
]
```

---

## Metrics

### Get Deployment Metrics

```
GET /api/v1/deployments/:id/metrics
```

Returns current metrics for a deployment.

**Response:**

```json
{
  "deployment_id": "default/my-api",
  "rps": 150.5,
  "latency_p50_ms": 12,
  "latency_p99_ms": 89,
  "error_rate": 0.02,
  "memory_used_bytes": 45000000,
  "instance_count": 3
}
```

### Prometheus Metrics

```
GET /metrics
```

Returns all cluster metrics in Prometheus exposition format. Suitable for
scraping by Prometheus, Grafana Agent, or any OpenMetrics-compatible tool.

---

## Rollouts

### Start a Rollout

```
POST /api/v1/deployments/:id/rollout
Content-Type: application/json
```

**Rolling update:**

```json
{
  "strategy": "rolling",
  "batch_size": 2,
  "health_check_interval": "5s"
}
```

**Canary deployment:**

```json
{
  "strategy": "canary",
  "canary_percent": 10,
  "observation_window": "300s"
}
```

**Blue-green switch:**

```json
{
  "strategy": "blue_green"
}
```

### List Active Rollouts

```
GET /api/v1/rollouts
```

### Get Rollout Status

```
GET /api/v1/rollouts/:id
```

**Response:**

```json
{
  "id": "rollout-xyz",
  "deployment_id": "default/my-api",
  "strategy": "rolling",
  "phase": "in_progress",
  "batches_completed": 2,
  "batches_total": 5,
  "started_at": 1710500200
}
```

### Pause Rollout

```
POST /api/v1/rollouts/:id/pause
```

### Resume Rollout

```
POST /api/v1/rollouts/:id/resume
```

---

## Nodes

### List Nodes

```
GET /api/v1/nodes
```

Returns all nodes in the cluster with their status and capacity.

**Response:**

```json
[
  {
    "node_id": "node-1",
    "address": "10.0.1.5",
    "port": 9000,
    "status": "healthy",
    "capacity": {
      "memory_bytes": 8589934592,
      "cpu_weight": 1000
    },
    "instances_running": 12,
    "last_heartbeat": 1710500300
  }
]
```

---

## Cloud API Endpoints

These endpoints are used by the `warp` CLI when communicating with WarpGrid Cloud.

### Register

```
POST /api/v1/auth/register
Content-Type: application/json
```

```json
{ "email": "you@example.com" }
```

**Response:**

```json
{
  "success": true,
  "data": {
    "api_key": "wgk_xxxxxxxxxxxxxxxxxxxx",
    "user_id": "usr_abc123",
    "namespace": "you"
  }
}
```

### Deploy (Upload)

```
POST /api/v1/cloud/deploy/upload
Authorization: Bearer <api_key>
X-WarpGrid-Name: my-api
X-WarpGrid-Region: iad
Content-Type: application/octet-stream

<wasm binary>
```

### List Cloud Deployments

```
GET /api/v1/cloud/deployments
Authorization: Bearer <api_key>
```

### Destroy Cloud Deployment

```
DELETE /api/v1/cloud/deploy/:deployment_id
Authorization: Bearer <api_key>
```

### Cloud Platform Status

```
GET /api/v1/cloud/status
```

### Fetch Logs

```
GET /api/v1/cloud/logs/:deployment_id
Authorization: Bearer <api_key>
```

### Scale Cloud Deployment

```
PUT /api/v1/cloud/deploy/:deployment_id/scale
Authorization: Bearer <api_key>
Content-Type: application/json
```

```json
{ "min": 2, "max": 10 }
```

### Verify Domain

```
POST /api/v1/cloud/domains/:domain/verify
Authorization: Bearer <api_key>
```
