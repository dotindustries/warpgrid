# CLI Commands

The `warp` CLI is the primary interface for building, deploying, and managing
WarpGrid services. It ships as a single binary built from the `crates/warp-cli`
crate.

## Global Behavior

- Credentials are stored in `~/.warpgrid/config.toml`.
- The default cloud API URL is `http://localhost:8443` (override with `--api-url`
  on `login` or `ping`, or set `api_url` in the config file).
- Logging is controlled via the `RUST_LOG` environment variable (e.g.,
  `RUST_LOG=warp=debug`).

---

## `warp login`

Authenticate with WarpGrid Cloud.

```bash
# Register a new account
warp login --email you@example.com

# Login with an existing API key
warp login --api-key wgk_xxxxxxxxxxxxxxxxxxxx

# Specify a custom API URL
warp login --email you@example.com --api-url https://api.warpgrid.dev
```

**Flags:**

| Flag         | Description                                     |
|--------------|-------------------------------------------------|
| `--api-key`  | API key for an existing account                 |
| `--email`    | Email address to register a new account         |
| `--api-url`  | Cloud API URL (default: `http://localhost:8443`) |

On registration, the CLI prints the generated API key, user ID, and namespace.
The key is shown only once -- save it.

---

## `warp init`

Scaffold a new project from a template.

```bash
warp init --template bun my-api
warp init --template async-rust --path ./services/my-handler
```

**Flags:**

| Flag         | Description                                          |
|--------------|------------------------------------------------------|
| `--template` | Template name: `bun`, `async-rust`, `async-go`, `async-ts` |
| `--path`     | Target directory (default: `./<template-name>`)      |

---

## `warp pack`

Compile a project to a Wasm component without deploying.

```bash
warp pack
warp pack --path ./my-api
warp pack --path ./my-api --lang rust
```

**Flags:**

| Flag     | Description                                              |
|----------|----------------------------------------------------------|
| `--path` | Project directory (default: `.`)                         |
| `--lang` | Override build language (`rust`, `go`, `bun`, `js`, `typescript`) |

The language is read from `[build].lang` in `warp.toml`, or auto-detected from
project marker files (`Cargo.toml` -> rust, `go.mod` -> go, `bunfig.toml` -> bun,
`package.json` -> typescript/js).

---

## `warp deploy`

Compile and deploy a Wasm component to WarpGrid Cloud.

```bash
warp deploy
warp deploy --region ams
warp deploy --path ./my-api --region sin --lang bun
```

**Flags:**

| Flag       | Description                              |
|------------|------------------------------------------|
| `--path`   | Project directory (default: `.`)         |
| `--region` | Target region (default: `iad`)           |
| `--lang`   | Override build language                  |

The deployment name is read from `[package].name` in `warp.toml`.

---

## `warp status`

List all deployments in your namespace.

```bash
warp status
```

Output:

```
DEPLOYMENT                     STATUS     REGION     INST     NS
my-api                         running    iad        2        default
go-service                     running    ams        1        default

2 deployment(s)
```

---

## `warp logs`

Fetch deployment logs.

```bash
warp logs my-api
warp logs my-api --follow
```

**Arguments:**

| Argument        | Description                |
|-----------------|----------------------------|
| `deployment_id` | The deployment to query    |

**Flags:**

| Flag       | Description                              |
|------------|------------------------------------------|
| `--follow` | Poll for new logs every 2 seconds        |

---

## `warp scale`

Set the instance count range for a deployment.

```bash
warp scale my-api --min 2 --max 10
warp scale my-api --min 0 --max 5    # enable scale-to-zero
```

**Arguments:**

| Argument        | Description                |
|-----------------|----------------------------|
| `deployment_id` | The deployment to scale    |

**Flags:**

| Flag    | Description                  |
|---------|------------------------------|
| `--min` | Minimum instance count       |
| `--max` | Maximum instance count       |

`--min` must be less than or equal to `--max`.

---

## `warp destroy`

Delete a deployment.

```bash
warp destroy my-api
```

**Arguments:**

| Argument        | Description                |
|-----------------|----------------------------|
| `deployment_id` | The deployment to delete   |

This stops all instances and removes the deployment from the cluster.

---

## `warp ping`

Check WarpGrid Cloud platform status.

```bash
warp ping
warp ping --api-url https://api.warpgrid.dev
```

**Flags:**

| Flag        | Description                              |
|-------------|------------------------------------------|
| `--api-url` | Override the cloud API URL               |

Output:

```
WarpGrid Cloud
  Status:  ok
  Version: 0.1.0
  Mode:    cluster
  API:     https://api.warpgrid.dev
  Namespace: default
```

---

## `warp domains verify`

Verify DNS configuration for a custom domain.

```bash
warp domains verify api.example.com
```

**Arguments:**

| Argument | Description                              |
|----------|------------------------------------------|
| `domain` | The domain to verify (e.g., `api.example.com`) |

See the [Custom Domains guide](../guides/custom-domains.md) for the full workflow.

---

## `warp convert analyze`

Analyze a project for Wasm compatibility.

```bash
warp convert analyze
warp convert analyze --path ./my-project --format json
warp convert analyze --lang go
```

**Flags:**

| Flag       | Description                                          |
|------------|------------------------------------------------------|
| `--path`   | Project directory or Dockerfile path (default: `.`)  |
| `--format` | Output format: `text` or `json` (default: `text`)   |
| `--lang`   | Override language detection                          |

The analyzer scans dependencies and reports compatibility verdicts:
Compatible, Shim Compatible, Incompatible, or Blocked.

---

## `warp convert init`

Generate a `warp.toml` scaffold from project analysis.

```bash
warp convert init
warp convert init --path ./my-project
```

**Flags:**

| Flag     | Description                       |
|----------|-----------------------------------|
| `--path` | Project directory (default: `.`)  |
