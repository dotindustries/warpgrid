use super::TemplateFile;

pub fn files() -> Vec<TemplateFile> {
    vec![
        TemplateFile {
            path: "package.json",
            content: PACKAGE_JSON,
        },
        TemplateFile {
            path: "src/handler.ts",
            content: HANDLER_TS,
        },
        TemplateFile {
            path: "warp.toml",
            content: WARP_TOML,
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
    "build": "warp pack"
  }
}
"#;

const WARP_TOML: &str = r#"[package]
name = "my-async-handler"
version = "0.1.0"

[build]
lang = "typescript"
entry = "src/handler.ts"
"#;

const README: &str = r#"# Async TypeScript Handler

A WarpGrid async handler written in TypeScript.

## Prerequisites

- Node.js
- `warp` CLI

## Getting Started

```bash
# Build the Wasm component
warp pack

# The output .wasm component will be in the project directory
```

## Project Structure

- `src/handler.ts` — Handler implementation
- `package.json` — Node.js package metadata
- `warp.toml` — WarpGrid build configuration

## How It Works

The handler uses a service-worker-style `fetch` event listener pattern.
`addEventListener("fetch", ...)` registers the handler which receives
`Request` objects and returns `Response` objects. WarpGrid shim globals
(DNS, database, filesystem) are available via `globalThis.warpgrid`.
"#;

const HANDLER_TS: &str = r#"addEventListener("fetch", (event: any) => {
  event.respondWith(handleRequest(event.request));
});

async function handleRequest(request: Request): Promise<Response> {
  const url = new URL(request.url, "http://localhost");

  if (url.pathname === "/health") {
    return new Response(JSON.stringify({ status: "ok" }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  }

  // Echo request info
  const body = {
    method: request.method,
    uri: url.pathname,
  };

  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}
"#;
