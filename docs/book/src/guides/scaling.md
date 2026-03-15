# Scaling and Rollouts

WarpGrid provides manual scaling, metrics-driven autoscaling, and multiple
rollout strategies for deploying new versions safely.

## Manual Scaling

Set the instance count range for a deployment:

```bash
warp scale my-api --min 2 --max 10
```

- `--min` is the floor. WarpGrid will always keep at least this many instances
  running (unless you set it to 0 for scale-to-zero).
- `--max` is the ceiling. The autoscaler will never exceed this count.

Check the current instance count:

```bash
warp status
```

```
DEPLOYMENT                     STATUS     REGION     INST     NS
my-api                         running    iad        3        default

1 deployment(s)
```

## Scale to Zero

To enable scale-to-zero (no instances when there is no traffic), set `--min 0`:

```bash
warp scale my-api --min 0 --max 10
```

When the autoscaler detects zero requests per second, it scales the deployment
down to zero instances. The first incoming request triggers a cold start, which
takes microseconds for Wasm components (compared to seconds for containers).

## Autoscaling Configuration

Configure autoscaling in `warp.toml`:

```toml
[runtime]
min_instances = 1
max_instances = 20

[runtime.scaling]
metric = "rps"
target_value = 200
scale_up_window = "30s"
scale_down_window = "120s"
```

### Supported Metrics

| Metric         | Description                        | Example target |
|----------------|------------------------------------|----------------|
| `rps`          | Requests per second per instance   | 200            |
| `latency_p99`  | 99th percentile latency (ms)       | 500            |
| `error_rate`   | Error rate (percentage)            | 5              |
| `memory`       | Memory usage (percentage)          | 80             |

### Scaling Algorithm

The autoscaler evaluates the current metric value against the target:

```
if current_value > target * 1.1:
    desired = ceil(instances * (current / target))
    scale up to min(desired, max_instances)

if current_value < target * 0.5 and instances > min:
    desired = ceil(instances * (current / target))
    scale down to max(desired, min_instances)

if rps == 0 and min_instances == 0:
    scale to 0
```

### Cooldown Windows

- `scale_up_window` -- minimum time between scale-up events. Prevents the
  autoscaler from adding instances too rapidly during traffic spikes.
- `scale_down_window` -- minimum time between scale-down events. Prevents
  thrashing during variable traffic patterns.

Recommended starting values: 30 seconds for scale-up, 2 minutes for scale-down.

## Rollout Strategies

When you redeploy a service (`warp deploy`), WarpGrid performs a rollout to
replace the old version with the new one. Three strategies are available.

### Rolling Update

The default strategy. Instances are replaced in batches with health checks
between each batch.

```bash
curl -X POST http://localhost:8443/api/v1/deployments/my-api/rollout \
  -H "Content-Type: application/json" \
  -d '{
    "strategy": "rolling",
    "batch_size": 2,
    "health_check_interval": "5s"
  }'
```

WarpGrid replaces `batch_size` instances at a time, waits for health checks to
pass, then proceeds to the next batch. If a batch fails health checks, the
rollout pauses automatically.

### Canary Deployment

Route a small percentage of traffic to the new version and observe metrics before
promoting:

```bash
curl -X POST http://localhost:8443/api/v1/deployments/my-api/rollout \
  -H "Content-Type: application/json" \
  -d '{
    "strategy": "canary",
    "canary_percent": 10,
    "observation_window": "300s"
  }'
```

During the observation window, 10% of traffic goes to the new version. If error
rates stay below thresholds, the rollout promotes the new version to 100%.

### Blue-Green

Run both versions simultaneously and switch traffic atomically:

```bash
curl -X POST http://localhost:8443/api/v1/deployments/my-api/rollout \
  -H "Content-Type: application/json" \
  -d '{
    "strategy": "blue_green"
  }'
```

The new version ("green") is deployed alongside the old ("blue"). Once the
green instances pass health checks, traffic switches from blue to green in one
step.

## Managing Rollouts

List active rollouts:

```bash
curl http://localhost:8443/api/v1/rollouts
```

Check rollout status:

```bash
curl http://localhost:8443/api/v1/rollouts/ROLLOUT_ID
```

Pause a rollout (e.g., if you spot an issue):

```bash
curl -X POST http://localhost:8443/api/v1/rollouts/ROLLOUT_ID/pause
```

Resume a paused rollout:

```bash
curl -X POST http://localhost:8443/api/v1/rollouts/ROLLOUT_ID/resume
```

## Resource Limits

Each deployment can declare resource constraints:

```toml
[runtime.resources]
memory_limit = "64MB"
cpu_weight = 100
```

- `memory_limit` -- maximum memory the Wasm instance can allocate.
- `cpu_weight` -- relative CPU priority (higher weight = more CPU time when
  the node is contended).

The placement engine uses these limits when bin-packing instances onto nodes.

## Monitoring

WarpGrid exposes Prometheus metrics at the `/metrics` endpoint on the control
plane:

```bash
curl http://localhost:8443/metrics
```

Per-deployment metrics are available via the REST API:

```bash
curl http://localhost:8443/api/v1/deployments/my-api/metrics
```

This returns RPS, latency percentiles, error rates, and memory usage -- the
same data the autoscaler uses for scaling decisions.
