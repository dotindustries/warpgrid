# Async TypeScript Handler

A WarpGrid async handler written in TypeScript.

## Prerequisites

- Node.js
- `warp` CLI

## Getting Started

```bash
# Build the Wasm component
warp pack

# The output .wasm component will be in the dist/ directory
```

## Project Structure

- `src/handler.ts` — Handler implementation
- `package.json` — Node.js package metadata
- `warp.toml` — WarpGrid build configuration
- `wit/` — WIT interface definitions (WASI + WarpGrid shims)

## How It Works

The handler uses a service-worker-style `fetch` event listener pattern.
`addEventListener("fetch", ...)` registers the handler which receives
`Request` objects and returns `Response` objects.

WarpGrid shim globals (DNS, database, filesystem) are available via
`globalThis.warpgrid` — these are auto-injected by `warp pack` during
componentization.

The handler demonstrates:
- Health check endpoint (`/health`)
- Request echo (returns method and URI as JSON)
