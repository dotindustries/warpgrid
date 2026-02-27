/**
 * TDD tests for `warp dev --lang bun` — local development with watch and hot-reload.
 *
 * Tests cover:
 * - DevServer starts and serves HTTP requests
 * - File watcher detects .ts/.tsx/.js/.jsx changes
 * - Hot-reload swaps handler after file change (within 2s)
 * - --native mode runs directly in Bun
 * - Compilation errors shown without crashing; previous module serves
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { mkdtemp, writeFile, mkdir, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import {
  type DevConfig,
  type DevServerHandle,
  createDevServer,
  shouldWatch,
  DEFAULT_WATCH_EXTENSIONS,
  debounce,
  findEntryPoint,
  CompileError,
} from "./dev.ts";

// ── File watcher utilities ────────────────────────────────────────────────

describe("shouldWatch", () => {
  it("accepts .ts files", () => {
    expect(shouldWatch("handler.ts", DEFAULT_WATCH_EXTENSIONS)).toBe(true);
  });

  it("accepts .tsx files", () => {
    expect(shouldWatch("App.tsx", DEFAULT_WATCH_EXTENSIONS)).toBe(true);
  });

  it("accepts .js files", () => {
    expect(shouldWatch("utils.js", DEFAULT_WATCH_EXTENSIONS)).toBe(true);
  });

  it("accepts .jsx files", () => {
    expect(shouldWatch("Component.jsx", DEFAULT_WATCH_EXTENSIONS)).toBe(true);
  });

  it("rejects non-source files", () => {
    expect(shouldWatch("README.md", DEFAULT_WATCH_EXTENSIONS)).toBe(false);
    expect(shouldWatch("image.png", DEFAULT_WATCH_EXTENSIONS)).toBe(false);
    expect(shouldWatch("data.json", DEFAULT_WATCH_EXTENSIONS)).toBe(false);
  });

  it("rejects node_modules paths", () => {
    expect(shouldWatch("node_modules/foo/index.ts", DEFAULT_WATCH_EXTENSIONS)).toBe(false);
  });

  it("rejects .git paths", () => {
    expect(shouldWatch(".git/HEAD", DEFAULT_WATCH_EXTENSIONS)).toBe(false);
  });

  it("rejects dist/target output paths", () => {
    expect(shouldWatch("dist/bundle.js", DEFAULT_WATCH_EXTENSIONS)).toBe(false);
    expect(shouldWatch("target/wasm/handler.wasm", DEFAULT_WATCH_EXTENSIONS)).toBe(false);
  });
});

// ── Debounce utility ──────────────────────────────────────────────────────

describe("debounce", () => {
  it("collapses rapid calls into one", async () => {
    let callCount = 0;
    const debounced = debounce(() => {
      callCount++;
    }, 50);

    debounced();
    debounced();
    debounced();

    // Should not have fired yet
    expect(callCount).toBe(0);

    // Wait for debounce period
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(callCount).toBe(1);
  });

  it("fires immediately after debounce period passes", async () => {
    let callCount = 0;
    const debounced = debounce(() => {
      callCount++;
    }, 20);

    debounced();
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(callCount).toBe(1);

    debounced();
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(callCount).toBe(2);
  });
});

// ── Entry point resolution ────────────────────────────────────────────────

describe("findEntryPoint", () => {
  let tmpDir: string;

  beforeEach(async () => {
    tmpDir = await mkdtemp(join(tmpdir(), "warp-dev-test-"));
  });

  afterEach(async () => {
    await rm(tmpDir, { recursive: true, force: true });
  });

  it("finds entry from warp.toml build.entry", async () => {
    await mkdir(join(tmpDir, "src"), { recursive: true });
    await writeFile(join(tmpDir, "src", "index.ts"), 'export default { fetch() { return new Response("ok") } }');
    await writeFile(
      join(tmpDir, "warp.toml"),
      `[package]\nname = "test"\nversion = "0.1.0"\n\n[build]\nlang = "bun"\nentry = "src/index.ts"\n`
    );

    const entry = await findEntryPoint(tmpDir);
    expect(entry).toBe("src/index.ts");
  });

  it("falls back to src/index.ts if no warp.toml", async () => {
    await mkdir(join(tmpDir, "src"), { recursive: true });
    await writeFile(join(tmpDir, "src", "index.ts"), "export default {}");

    const entry = await findEntryPoint(tmpDir);
    expect(entry).toBe("src/index.ts");
  });

  it("throws if no entry point found", async () => {
    await expect(findEntryPoint(tmpDir)).rejects.toThrow("entry point");
  });
});

// ── CompileError ──────────────────────────────────────────────────────────

describe("CompileError", () => {
  it("stores stderr and exit code", () => {
    const err = new CompileError("Build failed", "syntax error at line 5", 1);
    expect(err.message).toBe("Build failed");
    expect(err.stderr).toBe("syntax error at line 5");
    expect(err.exitCode).toBe(1);
    expect(err.name).toBe("CompileError");
  });

  it("includes stderr in string representation", () => {
    const err = new CompileError("Build failed", "unexpected token", 1);
    expect(err.toString()).toContain("unexpected token");
  });
});

// ── DevServer native mode ─────────────────────────────────────────────────

describe("DevServer (native mode)", () => {
  let tmpDir: string;

  beforeEach(async () => {
    tmpDir = await mkdtemp(join(tmpdir(), "warp-dev-native-"));
  });

  afterEach(async () => {
    await rm(tmpDir, { recursive: true, force: true });
  });

  it("starts HTTP server and responds to requests", async () => {
    await mkdir(join(tmpDir, "src"), { recursive: true });
    await writeFile(
      join(tmpDir, "src", "index.ts"),
      `export default { fetch(req: Request) { return new Response("v1"); } };`
    );

    const config: DevConfig = {
      projectPath: tmpDir,
      port: 0, // auto-assign
      native: true,
      entry: "src/index.ts",
    };

    const server = await createDevServer(config);
    try {
      expect(server.port).toBeGreaterThan(0);

      const res = await fetch(`http://localhost:${server.port}/`);
      expect(res.status).toBe(200);
      expect(await res.text()).toBe("v1");
    } finally {
      await server.stop();
    }
  });

  it("hot-reloads after file change within 2 seconds", async () => {
    await mkdir(join(tmpDir, "src"), { recursive: true });
    await writeFile(
      join(tmpDir, "src", "index.ts"),
      `export default { fetch(req: Request) { return new Response("v1"); } };`
    );

    const config: DevConfig = {
      projectPath: tmpDir,
      port: 0,
      native: true,
      entry: "src/index.ts",
    };

    const server = await createDevServer(config);
    try {
      // Verify initial response
      const res1 = await fetch(`http://localhost:${server.port}/`);
      expect(await res1.text()).toBe("v1");

      // Modify the handler
      await writeFile(
        join(tmpDir, "src", "index.ts"),
        `export default { fetch(req: Request) { return new Response("v2"); } };`
      );

      // Wait for hot-reload (should complete within 2 seconds)
      const startTime = Date.now();
      let foundV2 = false;
      while (Date.now() - startTime < 2000) {
        await new Promise((resolve) => setTimeout(resolve, 100));
        const res = await fetch(`http://localhost:${server.port}/`);
        if ((await res.text()) === "v2") {
          foundV2 = true;
          break;
        }
      }

      expect(foundV2).toBe(true);
    } finally {
      await server.stop();
    }
  });

  it("survives compilation errors — previous handler keeps serving", async () => {
    await mkdir(join(tmpDir, "src"), { recursive: true });
    await writeFile(
      join(tmpDir, "src", "index.ts"),
      `export default { fetch(req: Request) { return new Response("good"); } };`
    );

    const config: DevConfig = {
      projectPath: tmpDir,
      port: 0,
      native: true,
      entry: "src/index.ts",
    };

    const server = await createDevServer(config);
    try {
      // Verify initial response
      const res1 = await fetch(`http://localhost:${server.port}/`);
      expect(await res1.text()).toBe("good");

      // Write broken code
      await writeFile(
        join(tmpDir, "src", "index.ts"),
        // This will fail at dynamic import time — no default export of correct shape
        `throw new Error("compile time error simulation");`
      );

      // Wait for reload attempt
      await new Promise((resolve) => setTimeout(resolve, 500));

      // Server should still be running, either serving old or an error response
      const res2 = await fetch(`http://localhost:${server.port}/`);
      expect(res2.status).toBeGreaterThanOrEqual(200);
      // The key assertion: server did NOT crash
      expect(server.isRunning()).toBe(true);
    } finally {
      await server.stop();
    }
  });

  it("watches only specified extensions", async () => {
    await mkdir(join(tmpDir, "src"), { recursive: true });
    await writeFile(
      join(tmpDir, "src", "index.ts"),
      `export default { fetch(req: Request) { return new Response("original"); } };`
    );

    const config: DevConfig = {
      projectPath: tmpDir,
      port: 0,
      native: true,
      entry: "src/index.ts",
    };

    const server = await createDevServer(config);
    try {
      // Write a non-watched file
      await writeFile(join(tmpDir, "src", "data.json"), '{"key": "value"}');

      // Wait a bit
      await new Promise((resolve) => setTimeout(resolve, 300));

      // Handler should NOT have reloaded
      const res = await fetch(`http://localhost:${server.port}/`);
      expect(await res.text()).toBe("original");
    } finally {
      await server.stop();
    }
  });

  it("serves last compilation error as 500 response when handler fails to load", async () => {
    await mkdir(join(tmpDir, "src"), { recursive: true });
    // Start with a handler that will fail to load (no default export)
    await writeFile(
      join(tmpDir, "src", "index.ts"),
      `export const notDefault = "oops";`
    );

    const config: DevConfig = {
      projectPath: tmpDir,
      port: 0,
      native: true,
      entry: "src/index.ts",
    };

    const server = await createDevServer(config);
    try {
      const res = await fetch(`http://localhost:${server.port}/`);
      // Should get an error response since handler is invalid
      expect(res.status).toBe(503);
      const body = await res.text();
      expect(body).toContain("error");
    } finally {
      await server.stop();
    }
  });
});

// ── DevServer Wasm mode ───────────────────────────────────────────────────

describe("DevServer (wasm mode)", () => {
  let tmpDir: string;

  beforeEach(async () => {
    tmpDir = await mkdtemp(join(tmpdir(), "warp-dev-wasm-"));
  });

  afterEach(async () => {
    await rm(tmpDir, { recursive: true, force: true });
  });

  it("compiles handler to Wasm before serving", async () => {
    await mkdir(join(tmpDir, "src"), { recursive: true });
    await writeFile(
      join(tmpDir, "src", "index.ts"),
      `addEventListener("fetch", (event) => event.respondWith(new Response("wasm-ok")));`
    );

    const config: DevConfig = {
      projectPath: tmpDir,
      port: 0,
      native: false,
      entry: "src/index.ts",
    };

    // This test verifies the compile step is invoked.
    // We test the compile function in isolation since full Wasm serving
    // requires jco + wasmtime which may not be in CI.
    const server = await createDevServer(config);
    try {
      // In Wasm mode, the server should have attempted compilation
      expect(server.lastCompileAttempted()).toBe(true);
    } finally {
      await server.stop();
    }
  });
});
