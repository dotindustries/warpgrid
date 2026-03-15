# Bun Handler

A WarpGrid async handler written in TypeScript for Bun.

## Prerequisites

- [Bun](https://bun.sh) v1.0+
- `warp` CLI

## Getting Started

```bash
# Install dependencies
bun install

# Run tests
bun test

# Type check
bun run typecheck

# Build the Wasm component
warp pack
```

## Project Structure

- `src/index.ts` — Handler implementation
- `src/index.test.ts` — Unit tests
- `warp.toml` — WarpGrid build configuration
- `bunfig.toml` — Bun configuration

## How It Works

The handler exports a `WarpGridHandler` object with a `fetch()` method that
receives standard `Request` objects and returns `Response` objects. This is
the same pattern used by Bun's built-in HTTP server.

WarpGrid capabilities are available via SDK imports:

- `@warpgrid/bun-sdk/postgres` — PostgreSQL database access
- `@warpgrid/bun-sdk/dns` — DNS resolution
- `@warpgrid/bun-sdk/fs` — Virtual filesystem
