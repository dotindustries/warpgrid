# OpenClaw "Marketeer" K8s Deployment Plan

## Context & Research Summary

### The Tweet (Ernesto Lopez / @ErnestoSOFTWARE)

The specific tweet (status/2033917717762191659) could not be fetched directly due to X's 403 restrictions, but extensive research into Ernesto Lopez's OpenClaw content reveals a consistent playbook centered around his agent **"Eddie"**:

- **Revenue claim**: $73k/mo across 11 B2C apps, with OpenClaw automating content that previously required a $30k/mo agency
- **Core approach**: Automated faceless content pages (still images + text overlays) across TikTok, X, YouTube, Instagram
- **Key skill**: Uses the "Larry" skill by @oliverhenry (8M+ views via automated content)
- **Capabilities deployed**: Content generation, influencer outreach (Instagram DMs/emails), customer support triage, KPI reporting (Singular integration)
- **Architecture**: Single OpenClaw agent ("Eddie") managing 4+ social accounts with specialized skills

### OpenClaw Architecture

- **Runtime**: Node 24+ (or 22.16+), WebSocket Gateway on port 18789
- **Identity**: Defined via `SOUL.md` (persistent personality/instructions)
- **Skills**: Installable from ClawHub marketplace (700+ skills)
- **Browser automation**: Chromium sidecar via CDP (port 9222)
- **Channels**: 20+ messaging platforms (Telegram, WhatsApp, Slack, Discord, etc.)
- **Model providers**: Anthropic API key (OAuth deprecated Jan 2026), plus OpenAI/Gemini/Ollama
- **Stateful**: Memory lives on disk, single-instance only (no horizontal scaling per agent)

### Available K8s Tooling

- **Helm chart**: `serhanekicii/openclaw-helm` (v1.5.5, app version 2026.3.13-1)
  - Based on bjw-s app-template
  - Includes Chromium sidecar, init-skills, init-config containers
  - Config modes: `merge` (preserves runtime changes) or `overwrite` (strict GitOps)
- **K8s Operator**: `openclaw-rocks/k8s-operator` (OCI Helm chart, validating webhook)
- **Official docs**: `docs.openclaw.ai/install/kubernetes` (Kustomize-based manifests)

---

## Deployment Plan

### Phase 1: Directory Structure & Base Configuration

Create the following structure under `/home/user/warpgrid/infra/openclaw/`:

```
infra/openclaw/
├── README.md                    # Deployment documentation
├── namespace.yaml               # Dedicated namespace
├── secrets.yaml.example         # Template for secrets (never committed with real values)
├── kustomization.yaml           # Kustomize base
├── helm/
│   └── values.yaml              # Helm values for serhanekicii/openclaw-helm
├── agents/
│   └── marketeer/
│       ├── SOUL.md              # Eddie-style marketing agent personality
│       ├── AGENTS.md            # Operating rules & guardrails
│       └── HEARTBEAT.md         # Wake-up checklist (daily KPI reporting, content queue)
└── skills/
    └── install-list.txt         # ClawHub skills to install at init
```

### Phase 2: Namespace & Secrets

**Namespace**: `openclaw` — isolates the marketing agent from WarpGrid workloads.

**Secrets** (created manually or via external secrets operator):
- `ANTHROPIC_API_KEY` — Anthropic API key for Claude models
- `OPENCLAW_GATEWAY_TOKEN` — Gateway authentication token
- `TELEGRAM_BOT_TOKEN` — (optional) For Telegram channel integration
- Platform-specific credentials (Instagram, X API keys, email SMTP) as needed

### Phase 3: Helm Deployment

Using `serhanekicii/openclaw-helm` chart with custom values:

```yaml
app-template:
  configMode: merge  # Preserve runtime config changes (agent memory, paired devices)

  controllers:
    main:
      replicas: 1  # OpenClaw is single-instance
      strategy: Recreate

      containers:
        main:
          image:
            repository: ghcr.io/openclaw/openclaw
            tag: "2026.3.13-1"
          resources:
            requests:
              cpu: 200m
              memory: 512Mi
            limits:
              cpu: 2000m
              memory: 2Gi
          envFrom:
            - secretRef:
                name: openclaw-env-secret

        chromium:
          enabled: true  # Required for browser automation (content posting, scraping)
          resources:
            requests:
              cpu: 100m
              memory: 256Mi
            limits:
              cpu: 1000m
              memory: 1Gi

      initContainers:
        init-skills:
          enabled: true  # Auto-install marketing skills from ClawHub

  persistence:
    data:
      size: 10Gi  # Agent memory, sessions, content cache
      accessMode: ReadWriteOnce

  service:
    main:
      ports:
        http:
          port: 18789

  networkPolicy:
    enabled: true  # Lock down network access
```

### Phase 4: Marketing Agent Configuration (SOUL.md)

Create an "Eddie"-style marketing agent based on research. The SOUL.md will define:

1. **Identity**: Marketing automation agent focused on B2C app content
2. **Content strategy**: Faceless content pages — still images + text overlays, hook-first format
3. **Platform rules**: Platform-specific formatting (X: hook + 1 concept + CTA, TikTok: short scripts, Instagram: visual-first)
4. **Outreach parameters**: Influencer discovery criteria (follower range, engagement thresholds, niche alignment)
5. **Reporting**: Daily KPI reporting to designated Telegram/Slack channel
6. **Guardrails**: No API key exposure, rate limits on outreach (1000 emails/day, 100 DMs/day caps), brand voice consistency

### Phase 5: Skills Installation

Skills to install from ClawHub (configured in init-skills):

| Skill | Purpose |
|-------|---------|
| `social-content` | Multi-platform content generation |
| `copywriting` | Marketing copy & hooks |
| `web-scrape` | Competitor monitoring, trend research |
| `email-send` | Outreach email automation |
| `image-gen` | Still image/overlay generation |
| `calendar` | Content scheduling |
| `analytics` | KPI tracking & reporting |

### Phase 6: Security Hardening

1. **Pod security**: Non-root (UID 1000), read-only root FS, all capabilities dropped
2. **Network policies**:
   - Ingress: Only from gateway-system namespace on port 18789
   - Egress: DNS + public internet (block RFC1918 to prevent lateral movement)
3. **Secret management**: Use Kubernetes secrets (or external-secrets-operator for Vault/AWS SM integration)
4. **DM policy**: `pairing` mode — require pairing codes for inbound messages
5. **Resource limits**: Hard caps to prevent runaway browser automation

### Phase 7: Observability

1. **Health checks**: Liveness/readiness probes on `/health` endpoint (port 18789)
2. **Monitoring**: Prometheus scraping of OpenClaw metrics
3. **Alerting**: UptimeRobot or Prometheus AlertManager for gateway downtime
4. **Logging**: Stdout/stderr → cluster log aggregation (Loki, ELK, CloudWatch)

---

## Deployment Commands

```bash
# 1. Add Helm repo
helm repo add openclaw https://serhanekicii.github.io/openclaw-helm
helm repo update

# 2. Create namespace
kubectl create namespace openclaw

# 3. Create secrets
kubectl create secret generic openclaw-env-secret -n openclaw \
  --from-literal=ANTHROPIC_API_KEY=sk-ant-xxx \
  --from-literal=OPENCLAW_GATEWAY_TOKEN=your-token

# 4. Install chart
helm install openclaw openclaw/openclaw -n openclaw \
  -f infra/openclaw/helm/values.yaml

# 5. Port-forward for initial setup
kubectl port-forward -n openclaw svc/openclaw 18789:18789

# 6. Approve device pairing
kubectl exec -n openclaw deployment/openclaw -c main -- \
  node dist/index.js devices list
kubectl exec -n openclaw deployment/openclaw -c main -- \
  node dist/index.js devices approve <REQUEST_ID>
```

---

## Files to Create

1. `infra/openclaw/helm/values.yaml` — Full Helm values
2. `infra/openclaw/namespace.yaml` — Namespace manifest
3. `infra/openclaw/secrets.yaml.example` — Secret template
4. `infra/openclaw/agents/marketeer/SOUL.md` — Agent personality
5. `infra/openclaw/agents/marketeer/AGENTS.md` — Operating rules
6. `infra/openclaw/agents/marketeer/HEARTBEAT.md` — Daily wake-up tasks
7. `infra/openclaw/kustomization.yaml` — Kustomize overlay
8. `infra/openclaw/network-policy.yaml` — Network isolation rules

---

## Key Considerations

1. **Single-instance constraint**: OpenClaw cannot scale horizontally. For multiple marketing accounts, deploy separate OpenClaw instances (separate Helm releases) rather than scaling replicas.

2. **Storage**: Agent memory is disk-based. Use a reliable StorageClass with backups. The 10Gi PVC covers sessions, content cache, and agent memory.

3. **Cost**: Anthropic API usage is pay-as-you-go. Monitor token consumption via `/status` command and built-in usage tracking.

4. **Content moderation**: Add explicit guardrails in SOUL.md to prevent off-brand content or policy violations on social platforms.

5. **Rate limiting**: Social platform APIs have rate limits. Configure the agent's outreach caps conservatively to avoid account bans.

6. **Backup**: Regularly back up the PVC containing agent memory and configuration. Runtime changes (paired devices, learned preferences) live on disk.
