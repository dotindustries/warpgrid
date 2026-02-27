#!/usr/bin/env bun
/**
 * CLI entry point for `warp dev --lang bun`.
 *
 * Usage:
 *   bun run dev-cli.ts [--port 3000] [--native] [--entry src/index.ts] <project-path>
 *
 * This script is spawned by the Rust `warp dev` command.
 * It creates and starts a DevServer, then keeps the process alive
 * until interrupted (Ctrl+C).
 */

import { parseArgs } from "node:util";
import { resolve } from "node:path";
import { createDevServer, findEntryPoint, type DevConfig } from "./dev.ts";

const { values, positionals } = parseArgs({
  args: Bun.argv.slice(2),
  options: {
    port: { type: "string", short: "p", default: "3000" },
    native: { type: "boolean", default: false },
    entry: { type: "string", short: "e" },
  },
  allowPositionals: true,
  strict: false,
});

const projectPath = resolve(positionals[0] ?? ".");
const port = parseInt(String(values.port ?? "3000"), 10);
const native = Boolean(values.native);

// Resolve entry point
let entry: string;
if (typeof values.entry === "string") {
  entry = values.entry;
} else {
  try {
    entry = await findEntryPoint(projectPath);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    process.stderr.write(`Error: ${message}\n`);
    process.exit(1);
  }
}

const config: DevConfig = {
  projectPath,
  port,
  native,
  entry,
};

const mode = native ? "native Bun" : "Wasm (jco serve)";
process.stderr.write(`[warp dev] Starting ${mode} dev server...\n`);
process.stderr.write(`[warp dev] Project: ${projectPath}\n`);
process.stderr.write(`[warp dev] Entry:   ${entry}\n`);

try {
  const server = await createDevServer(config);
  process.stderr.write(
    `[warp dev] Listening on http://localhost:${server.port} (${mode})\n`
  );
  process.stderr.write(`[warp dev] Watching for file changes...\n`);

  // Keep process alive until interrupted
  const shutdown = async () => {
    process.stderr.write(`\n[warp dev] Shutting down...\n`);
    await server.stop();
    process.exit(0);
  };

  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
} catch (err) {
  const message = err instanceof Error ? err.message : String(err);
  process.stderr.write(`[warp dev] Failed to start: ${message}\n`);
  process.exit(1);
}
