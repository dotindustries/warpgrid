# Quickstart

Deploy your first WarpGrid service in five minutes.

## Prerequisites

- The `warp` CLI installed and available on your `PATH`
- A WarpGrid Cloud account (or a self-hosted `warpd` instance)

## Step 1: Log In

Register a new account or authenticate with an existing API key:

```bash
# Register a new account
warp login --email you@example.com

# Or log in with an existing API key
warp login --api-key wgk_xxxxxxxxxxxxxxxxxxxx
```

On success the CLI prints your namespace and stores credentials in
`~/.warpgrid/config.toml`.

## Step 2: Scaffold a Project

Use `warp init` to generate a project from a template:

```bash
warp init --template bun my-api
```

This creates a `my-api/` directory containing:

```
my-api/
  warp.toml          # Build + runtime config
  src/handler.ts     # HTTP handler
  package.json       # Bun dependencies
```

Available templates: `bun`, `async-rust`, `async-go`, `async-ts`.

## Step 3: Explore the Code

Open `my-api/src/handler.ts`. The default Bun template serves a JSON greeting:

```typescript
addEventListener("fetch", (event: FetchEvent) => {
  const url = new URL(event.request.url);

  if (url.pathname === "/" && event.request.method === "GET") {
    event.respondWith(
      Promise.resolve(
        new Response(JSON.stringify({
          message: "Hello from WarpGrid!",
          runtime: "bun",
          timestamp: new Date().toISOString(),
        }), {
          headers: { "Content-Type": "application/json" },
        })
      )
    );
  }
});
```

Edit the handler however you like. WarpGrid deploys standard HTTP handlers --
there is no proprietary SDK to learn.

## Step 4: Deploy

```bash
cd my-api
warp deploy --region iad
```

The CLI compiles your project to a Wasm component, uploads it to WarpGrid Cloud,
and prints the live URL:

```
Compiling project...
  Compiled: target/my-api.wasm (2.4 MB, sha256: a1b2c3d4e5f6)
Deploying 'my-api' to iad (2457600 bytes)...
Deployed successfully!
  Name:      my-api
  URL:       https://my-api.you.edge.warpgrid.dev
  Wasm hash: a1b2c3d4e5f6
```

## Step 5: Verify

```bash
curl https://my-api.you.edge.warpgrid.dev
```

Expected response:

```json
{
  "message": "Hello from WarpGrid!",
  "runtime": "bun",
  "timestamp": "2026-03-15T12:00:00.000Z"
}
```

## Next Commands

Check your deployment status:

```bash
warp status
```

View live logs:

```bash
warp logs my-api --follow
```

Scale the service:

```bash
warp scale my-api --min 2 --max 10
```

Tear it down when you are done:

```bash
warp destroy my-api
```

## What Just Happened?

1. `warp init` scaffolded a project with a `warp.toml` manifest.
2. `warp deploy` compiled your TypeScript to a `wasm32-wasip2` component using the
   Bun toolchain, then uploaded the binary to WarpGrid Cloud.
3. WarpGrid's scheduler placed the component on an edge node in the `iad` region,
   started an instance, and routed traffic to it via the proxy layer.
4. Your service is now running as a sandboxed Wasm component -- no container image,
   no Dockerfile, no Kubernetes YAML.

Continue to [How It Works](how-it-works.md) for a deeper look at the internals.
