## gstack

Use the `/browse` skill from gstack for **all web browsing**. Never use `mcp__claude-in-chrome__*` tools.

Available skills: `/plan-ceo-review`, `/plan-eng-review`, `/review`, `/ship`, `/browse`, `/qa`, `/setup-browser-cookies`, `/retro`

If gstack skills aren't working, run `cd .claude/skills/gstack && ./setup` to build the binary and register skills.

## BYOC Agent Architecture (for tervezo integration)

### What was built

BYOC = Bring Your Own Compute. Customers run `warpd agent` on their infra, connecting to WarpGrid cloud control plane on Fly.io. No self-hosted control plane needed.

### Data flow

```
Customer infra                          WarpGrid Cloud (Fly.io)
─────────────────                       ───────────────────────
warpd agent                             warpd cloud
  --auth-token wg_agent_*   ──gRPC──►   ClusterServer.Join()
  --control-plane host:port              validates token via TokenValidator trait
  --region iad                           injects namespace label into node
                                         returns node_id + namespace binding
  heartbeat loop (5s)        ──gRPC──►   MembershipManager.heartbeat()
  DeploymentWatcher          ◄─Turso──   cloud_deployments (read replica)
  RuntimeSync (30s)          ──Turso──►  cloud_instances, cloud_nodes, cloud_metrics
```

### Key files and what they do

**Agent token system (issue/validate/revoke):**
- `crates/warpd/src/cloud/agent_tokens.rs` — `AgentTokenStore` with memory + libSQL backends. Token format: `wg_agent_<32hex>`. SHA-256 hashed before storage. Methods: `issue()`, `validate()`, `list()`, `revoke()`.
- `crates/warpd/src/cloud/db.rs` — `cloud_agent_tokens` table (id, namespace, token_hash, name, revoked, created_at, last_used_at).

**gRPC auth on cluster Join:**
- `crates/warpgrid-cluster/proto/cluster.proto` — `JoinRequest.auth_token` (field 6), `JoinResponse.namespace` (field 4).
- `crates/warpgrid-cluster/src/server.rs` — `TokenValidator` trait with `validate_agent_token(&str) -> Option<(token_id, namespace)>`. `ClusterServer::with_auth()` constructor for cloud mode (requires token). `ClusterServer::new()` for self-hosted (NoopTokenValidator, no auth). On valid token: namespace injected into node labels as `labels["namespace"] = ns`.
- `crates/warpgrid-cluster/src/agent.rs` — `AgentConfig.auth_token: Option<String>`. `NodeAgent.namespace: Option<String>` set after join.

**REST API for token management:**
- `crates/warpd/src/cloud/routes.rs` — Three endpoints added to `cloud_router()`:
  - `POST /api/v1/cloud/agent-tokens` — `create_agent_token()`, requires Bearer auth, returns raw token (shown once).
  - `GET /api/v1/cloud/agent-tokens` — `list_agent_tokens()`, namespace-scoped.
  - `POST /api/v1/cloud/agent-tokens/{id}/revoke` — `revoke_agent_token()`, namespace-scoped.
- `CloudState.agent_tokens: AgentTokenStore` field added.

**CLI:**
- `crates/warpd/src/main.rs` — `Command::Agent` has `--auth-token` / `WARPGRID_AGENT_TOKEN` env var.
- `crates/warpd/src/agent_mode.rs` — `run_agent()` accepts `auth_token: Option<String>`, passes to `AgentConfig`.

**Wiring in cloud mode:**
- `crates/warpd/src/cloud_mode.rs` — Creates `AgentTokenStore::with_libsql()`, adds to `CloudState`.

**Deploy artifacts:**
- `deploy/agent/install.sh` — curl|bash installer. Validates KVM, downloads warpd + cloud-hypervisor + kernel + golden image, creates systemd unit.
- `deploy/helm/warpgrid-agent/` — Helm chart. DaemonSet on nodes labeled `warpgrid.dev/kvm=true`. Secret for agent token. Mounts `/dev/kvm`.

### What's NOT wired yet (needed for tervezo)

1. **Cloud mode does not run a gRPC ClusterServer** — `cloud_mode.rs` only starts HTTP. Need to add a gRPC listener (e.g. port 50051) with `ClusterServer::with_auth(membership, agent_token_store_as_validator)`. The `AgentTokenStore` needs to implement `TokenValidator` trait (trivial adapter).
2. **Placement engine doesn't filter by namespace** — `warpgrid-placement` picks nodes by resource availability only. Need to add `namespace` label filter so sprites for tenant X only land on tenant X's nodes.
3. **Sprite API doesn't exist yet on cloud** — Cloud has Wasm deployment API but no sprite-specific endpoints. Tervezo needs: `POST /api/v1/sprites` (create sprite VM on customer's BYOC node), `GET /api/v1/sprites/{id}` (status), `DELETE /api/v1/sprites/{id}` (terminate).
4. **No TokenValidator impl for AgentTokenStore** — Need `impl TokenValidator for AgentTokenStore` in warpd (can't be in warpgrid-cluster crate due to dependency direction). Add a newtype wrapper or implement directly since both are in warpd's scope.

### Test coverage

- 8 unit tests in `cloud::agent_tokens::tests` (memory + libSQL backends, revoke, namespace isolation).
- 15 unit tests in `warpgrid-cluster` (membership, heartbeat, labels, dead node detection).
- 133 total tests in warpd binary pass.
