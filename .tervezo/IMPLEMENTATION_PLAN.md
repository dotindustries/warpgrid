# Implementation Plan: End-to-End Regression Test Suite

## Task List

### Phase 1: PR Preview Environment Infrastructure

- [x] **1.1** Create `infra/fly-pr-preview.toml` — Fly.io app config template for ephemeral PR environments
- [ ] **1.2** Create `.github/workflows/pr-preview.yml` — GitHub Actions workflow (deploy-preview, e2e-tests, cleanup-preview)
- [ ] **1.3** Add PR preview URL comment via `gh pr comment` in deploy-preview job

### Phase 2: Playwright Test Infrastructure

- [ ] **2.1** Initialize `e2e/` project — `package.json`, `tsconfig.json`, npm scripts
- [ ] **2.2** Create `e2e/playwright.config.ts` — Playwright config with baseURL from env
- [ ] **2.3** Create `e2e/helpers/api-client.ts` — Typed wrapper for WarpGrid REST API
- [ ] **2.4** Create `e2e/helpers/fixtures.ts` — Shared test data factory + cleanup utilities

### Phase 3: E2E Test Suites

- [ ] **3.1** Smoke test (`e2e/tests/smoke.spec.ts`) — health, dashboard, API quick checks
- [ ] **3.2** API deployment CRUD tests (`e2e/tests/api/deployments.spec.ts`)
- [ ] **3.3** API scaling tests (`e2e/tests/api/scaling.spec.ts`)
- [ ] **3.4** API rollout tests (`e2e/tests/api/rollouts.spec.ts`)
- [ ] **3.5** API node + metrics tests (`e2e/tests/api/nodes.spec.ts`, `metrics.spec.ts`)
- [ ] **3.6** Dashboard overview tests (`e2e/tests/dashboard/overview.spec.ts`)
- [ ] **3.7** Dashboard deployments page tests (`e2e/tests/dashboard/deployments.spec.ts`)
- [ ] **3.8** Dashboard nodes page tests (`e2e/tests/dashboard/nodes.spec.ts`)
- [ ] **3.9** Dashboard rollouts page tests (`e2e/tests/dashboard/rollouts.spec.ts`)
- [ ] **3.10** Dashboard density demo tests (`e2e/tests/dashboard/density-demo.spec.ts`)

### Phase 4: CI Integration & Reporting

- [ ] **4.1** Upload Playwright report as artifact in e2e-tests job
- [ ] **4.2** Post test summary on PR via GitHub reporter + comment
- [ ] **4.3** Update `.gitignore` for e2e artifacts
- [ ] **4.4** Document E2E status check as required for branch protection

### Phase 5: Documentation & Cleanup

- [ ] **5.1** Add `e2e/README.md` — local run instructions, PR preview docs, adding tests guide
- [ ] **5.2** Verify full workflow end-to-end

## Notes

- Existing infra: `infra/fly-cloud.toml` (shared-cpu-2x, 1GB, region iad, port 8443 HTTPS)
- No existing e2e/ directory, no Playwright/Cypress, no PR preview workflow
- `.gitignore` already has `/test-results` but not e2e-specific entries
- OpenSpec docs at `docs/warpgrid-openspec-v0.2.0.docx`
