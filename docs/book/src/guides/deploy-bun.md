# Deploy a Bun App

This guide walks through deploying a Bun/TypeScript HTTP service to WarpGrid,
from project setup to a live endpoint.

## Prerequisites

- The `warp` CLI
- [Bun](https://bun.sh) installed locally (for development/testing)
- A WarpGrid Cloud account (`warp login`)

## Create the Project

Scaffold from the built-in Bun template:

```bash
warp init --template bun my-bun-api
cd my-bun-api
```

This generates the following structure:

```
my-bun-api/
  warp.toml
  package.json
  src/
    handler.ts
```

## Understand the Manifest

Open `warp.toml`:

```toml
[package]
name = "my-bun-api"
version = "0.1.0"
description = "Minimal Bun REST API example for WarpGrid"

[build]
lang = "bun"
entry = "src/handler.ts"
target = "wasip2"

[runtime]
trigger = "http"
```

Key fields:

- `lang = "bun"` tells `warp pack` to use the Bun-to-Wasm toolchain.
- `entry` points to your handler file.
- `trigger = "http"` means WarpGrid will route HTTP requests to your component.

## Write the Handler

The handler uses the standard `fetch` event listener pattern. Here is a
complete example with multiple routes (from `examples/bun-json-api`):

```typescript
function jsonResponse(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

addEventListener("fetch", (event: FetchEvent) => {
  const url = new URL(event.request.url);
  const method = event.request.method;

  let responsePromise: Promise<Response>;

  if (url.pathname === "/" && method === "GET") {
    responsePromise = Promise.resolve(
      jsonResponse({
        message: "Hello from WarpGrid!",
        runtime: "bun",
        timestamp: new Date().toISOString(),
      })
    );
  } else if (url.pathname === "/health" && method === "GET") {
    responsePromise = Promise.resolve(jsonResponse({ status: "ok" }));
  } else if (url.pathname === "/echo" && method === "POST") {
    responsePromise = event.request.json().then(
      (body) => jsonResponse({ echo: body }),
      () => jsonResponse({ error: "Invalid JSON" }, 400)
    );
  } else {
    responsePromise = Promise.resolve(
      jsonResponse({ error: "Not Found" }, 404)
    );
  }

  event.respondWith(responsePromise);
});
```

There is no proprietary SDK. You use `addEventListener("fetch", ...)` and return
standard `Response` objects.

## Test Locally

Before deploying, run the handler with Bun on your machine:

```bash
bun run src/handler.ts
```

In a separate terminal:

```bash
curl http://localhost:3000/
curl -X POST http://localhost:3000/echo -d '{"msg":"hi"}'
```

## Deploy

```bash
warp deploy --region iad
```

The CLI compiles your TypeScript to a Wasm component using the WarpGrid Bun
polyfill layer, uploads the binary, and prints the live URL:

```
Deployed successfully!
  Name:      my-bun-api
  URL:       https://my-bun-api.you.edge.warpgrid.dev
  Wasm hash: a1b2c3d4e5f6
```

## Add Environment Variables

Set environment variables in `warp.toml`:

```toml
[env]
APP_NAME = "my-bun-api"
APP_VERSION = "1.0.0"
```

Access them in your handler:

```typescript
const appName = Bun.env.APP_NAME ?? "default";
```

Redeploy after changing `warp.toml`:

```bash
warp deploy --region iad
```

## Configure Health Checks

Add a `[health]` section so WarpGrid can monitor your service:

```toml
[health]
endpoint = "/health"
interval = "5s"
timeout = "2s"
unhealthy_threshold = 3
```

WarpGrid will poll `GET /health` every five seconds. If three consecutive checks
fail, the instance is marked unhealthy and replaced.

## Enable WASI Shims

If your handler needs DNS resolution or randomness, enable shims:

```toml
[shims]
dns = true
dev_urandom = true
```

See the [warp.toml reference](../reference/warp-toml.md) for all available shims.

## Full Example

The complete Bun example is in the repository at
[`examples/bun-json-api/`](https://github.com/dotindustries/warpgrid/tree/main/examples/bun-json-api).
