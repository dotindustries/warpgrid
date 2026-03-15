use super::TemplateFile;

pub fn files() -> Vec<TemplateFile> {
    vec![
        TemplateFile {
            path: "package.json",
            content: PACKAGE_JSON,
        },
        TemplateFile {
            path: "bunfig.toml",
            content: BUNFIG_TOML,
        },
        TemplateFile {
            path: "tsconfig.json",
            content: TSCONFIG_JSON,
        },
        TemplateFile {
            path: "warp.toml",
            content: WARP_TOML,
        },
        TemplateFile {
            path: "src/index.ts",
            content: INDEX_TS,
        },
        TemplateFile {
            path: "src/index.test.ts",
            content: INDEX_TEST_TS,
        },
        TemplateFile {
            path: "README.md",
            content: README,
        },
    ]
}

const PACKAGE_JSON: &str = r#"{
  "name": "my-async-handler",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "test": "bun test",
    "typecheck": "bun run --bun tsc --noEmit",
    "build": "warp pack"
  },
  "dependencies": {
    "@warpgrid/bun-sdk": "^0.1.0"
  },
  "devDependencies": {
    "@types/bun": "^1.3.9",
    "typescript": "^5.7.0"
  }
}
"#;

const BUNFIG_TOML: &str = r#"# Bun configuration
# See https://bun.sh/docs/runtime/bunfig for options

[test]
coverage = false
"#;

const TSCONFIG_JSON: &str = r#"{
  "compilerOptions": {
    "target": "ESNext",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "allowImportingTsExtensions": true,
    "noEmit": true,
    "types": ["bun"]
  },
  "include": ["src/**/*.ts"]
}
"#;

const WARP_TOML: &str = r#"[package]
name = "my-async-handler"
version = "0.1.0"

[build]
lang = "bun"
entry = "src/index.ts"
"#;

const INDEX_TS: &str = r#"import type { WarpGridHandler } from "@warpgrid/bun-sdk";

// Database access (PostgreSQL):
// import { createPool } from "@warpgrid/bun-sdk/postgres";
// const pool = createPool({ database: "mydb" });
// const result = await pool.query("SELECT * FROM users WHERE id = $1", [userId]);

// DNS resolution:
// import { resolve } from "@warpgrid/bun-sdk/dns";
// const addresses = await resolve("example.com", "A");

// Virtual filesystem:
// import { readTextFile, writeFile } from "@warpgrid/bun-sdk/fs";
// const content = await readTextFile("/data/config.json");

const handler: WarpGridHandler = {
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);

    if (url.pathname === "/health") {
      return new Response(JSON.stringify({ status: "ok" }), {
        headers: { "Content-Type": "application/json" },
      });
    }

    // Echo endpoint: return request method and path
    return new Response(
      JSON.stringify({
        method: request.method,
        path: url.pathname,
      }),
      {
        headers: { "Content-Type": "application/json" },
      },
    );
  },
};

export default handler;
"#;

const INDEX_TEST_TS: &str = r#"import { describe, expect, it } from "bun:test";
import handler from "./index";

describe("handler", () => {
  it("returns health status", async () => {
    const request = new Request("http://localhost/health");
    const response = await handler.fetch(request);

    expect(response.status).toBe(200);
    expect(response.headers.get("Content-Type")).toBe("application/json");

    const body = await response.json();
    expect(body).toEqual({ status: "ok" });
  });

  it("echoes request method and path", async () => {
    const request = new Request("http://localhost/test-path", {
      method: "POST",
    });
    const response = await handler.fetch(request);

    expect(response.status).toBe(200);

    const body = await response.json();
    expect(body).toEqual({ method: "POST", path: "/test-path" });
  });
});
"#;

const README: &str = r#"# Bun Handler

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
"#;
