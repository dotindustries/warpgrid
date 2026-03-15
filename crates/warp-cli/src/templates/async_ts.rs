use super::TemplateFile;

pub fn files() -> Vec<TemplateFile> {
    let mut files = vec![
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
        // Top-level WIT world definition
        TemplateFile {
            path: "wit/handler.wit",
            content: WIT_HANDLER,
        },
    ];

    // WASI WIT deps (required by ComponentizeJS)
    files.extend(wit_dep_files());

    files
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

  // Echo request info as JSON
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

const WIT_HANDLER: &str = r#"/// TypeScript handler world with WarpGrid shim imports.
///
/// This world defines a standard WASI HTTP handler that also imports
/// WarpGrid shim interfaces for database, DNS, and filesystem access.
package warpgrid:ts-handler;

world handler {
  // Standard WASI HTTP
  import wasi:http/types@0.2.3;
  export wasi:http/incoming-handler@0.2.3;

  // WarpGrid shim imports (injected by prelude)
  import warpgrid:shim/database-proxy@0.1.0;
  import warpgrid:shim/dns@0.1.0;
  import warpgrid:shim/filesystem@0.1.0;
}
"#;

/// Returns all WASI + WarpGrid shim WIT dep files needed by ComponentizeJS.
fn wit_dep_files() -> Vec<TemplateFile> {
    vec![
        // CLI
        TemplateFile { path: "wit/deps/cli/command.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/cli/command.wit") },
        TemplateFile { path: "wit/deps/cli/environment.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/cli/environment.wit") },
        TemplateFile { path: "wit/deps/cli/exit.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/cli/exit.wit") },
        TemplateFile { path: "wit/deps/cli/imports.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/cli/imports.wit") },
        TemplateFile { path: "wit/deps/cli/run.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/cli/run.wit") },
        TemplateFile { path: "wit/deps/cli/stdio.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/cli/stdio.wit") },
        TemplateFile { path: "wit/deps/cli/terminal.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/cli/terminal.wit") },
        // Clocks
        TemplateFile { path: "wit/deps/clocks/monotonic-clock.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/clocks/monotonic-clock.wit") },
        TemplateFile { path: "wit/deps/clocks/timezone.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/clocks/timezone.wit") },
        TemplateFile { path: "wit/deps/clocks/wall-clock.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/clocks/wall-clock.wit") },
        TemplateFile { path: "wit/deps/clocks/world.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/clocks/world.wit") },
        // Filesystem
        TemplateFile { path: "wit/deps/filesystem/preopens.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/filesystem/preopens.wit") },
        TemplateFile { path: "wit/deps/filesystem/types.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/filesystem/types.wit") },
        TemplateFile { path: "wit/deps/filesystem/world.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/filesystem/world.wit") },
        // HTTP
        TemplateFile { path: "wit/deps/http/handler.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/http/handler.wit") },
        TemplateFile { path: "wit/deps/http/proxy.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/http/proxy.wit") },
        TemplateFile { path: "wit/deps/http/types.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/http/types.wit") },
        // IO
        TemplateFile { path: "wit/deps/io/error.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/io/error.wit") },
        TemplateFile { path: "wit/deps/io/poll.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/io/poll.wit") },
        TemplateFile { path: "wit/deps/io/streams.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/io/streams.wit") },
        TemplateFile { path: "wit/deps/io/world.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/io/world.wit") },
        // Random
        TemplateFile { path: "wit/deps/random/insecure-seed.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/random/insecure-seed.wit") },
        TemplateFile { path: "wit/deps/random/insecure.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/random/insecure.wit") },
        TemplateFile { path: "wit/deps/random/random.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/random/random.wit") },
        TemplateFile { path: "wit/deps/random/world.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/random/world.wit") },
        // WarpGrid shims
        TemplateFile { path: "wit/deps/shim/database-proxy.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/shim/database-proxy.wit") },
        TemplateFile { path: "wit/deps/shim/dns.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/shim/dns.wit") },
        TemplateFile { path: "wit/deps/shim/filesystem.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/shim/filesystem.wit") },
        // Sockets
        TemplateFile { path: "wit/deps/sockets/instance-network.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/sockets/instance-network.wit") },
        TemplateFile { path: "wit/deps/sockets/ip-name-lookup.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/sockets/ip-name-lookup.wit") },
        TemplateFile { path: "wit/deps/sockets/network.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/sockets/network.wit") },
        TemplateFile { path: "wit/deps/sockets/tcp-create-socket.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/sockets/tcp-create-socket.wit") },
        TemplateFile { path: "wit/deps/sockets/tcp.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/sockets/tcp.wit") },
        TemplateFile { path: "wit/deps/sockets/udp-create-socket.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/sockets/udp-create-socket.wit") },
        TemplateFile { path: "wit/deps/sockets/udp.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/sockets/udp.wit") },
        TemplateFile { path: "wit/deps/sockets/world.wit", content: include_str!("../../../../tests/fixtures/js-warpgrid-handler/wit/deps/sockets/world.wit") },
    ]
}
