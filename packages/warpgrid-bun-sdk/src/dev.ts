/**
 * `warp dev --lang bun` — Local dev server with file watching and hot-reload.
 *
 * Two modes:
 * - **Native mode** (`--native`): Runs handler directly in Bun with hot-reload.
 *   Fastest iteration — no Wasm compilation, full Bun debugger support.
 * - **Wasm mode** (default): Compiles handler via `bun build` + `jco componentize`,
 *   serves via `jco serve`, watches files and recompiles/restarts on change.
 *
 * Architecture:
 * - File watcher uses `fs.watch` (recursive) with debouncing
 * - Native mode: dynamic `import()` with cache-busting query param
 * - Wasm mode: subprocess management (jco serve) with restart on recompile
 * - Compilation errors are displayed but don't crash the server
 */

import { watch, type FSWatcher } from "node:fs";
import { readFile, access, stat } from "node:fs/promises";
import { join, extname, resolve } from "node:path";
import { spawn, type ChildProcess } from "node:child_process";

// ── Public types ──────────────────────────────────────────────────────────

export interface DevConfig {
  /** Absolute path to the project directory. */
  readonly projectPath: string;
  /** HTTP port to listen on. 0 = auto-assign. */
  readonly port: number;
  /** If true, run handler directly in Bun (no Wasm compilation). */
  readonly native: boolean;
  /** Entry point relative to projectPath (e.g., "src/index.ts"). */
  readonly entry: string;
}

export interface DevServerHandle {
  /** Actual port the server is listening on. */
  readonly port: number;
  /** Stop the dev server and file watcher. */
  stop(): Promise<void>;
  /** Whether the server is still running. */
  isRunning(): boolean;
  /** Whether a compile step was attempted (for Wasm mode). */
  lastCompileAttempted(): boolean;
}

// ── Constants ─────────────────────────────────────────────────────────────

export const DEFAULT_WATCH_EXTENSIONS = [".ts", ".tsx", ".js", ".jsx"];

const IGNORED_SEGMENTS = ["node_modules", ".git", "dist", "target"];

// ── Utility: shouldWatch ──────────────────────────────────────────────────

/**
 * Determine if a file path should trigger a rebuild.
 * Checks extension against allowed list and excludes ignored directories.
 */
export function shouldWatch(filePath: string, extensions: readonly string[]): boolean {
  // Reject paths containing ignored directory segments
  for (const segment of IGNORED_SEGMENTS) {
    if (filePath.includes(`${segment}/`) || filePath.includes(`${segment}\\`)) {
      return false;
    }
  }

  const ext = extname(filePath);
  return extensions.includes(ext);
}

// ── Utility: debounce ─────────────────────────────────────────────────────

/**
 * Create a debounced version of a function.
 * Collapses rapid calls into a single invocation after `delayMs`.
 */
export function debounce<T extends (...args: unknown[]) => void>(
  fn: T,
  delayMs: number,
): (...args: Parameters<T>) => void {
  let timer: ReturnType<typeof setTimeout> | null = null;
  return (...args: Parameters<T>) => {
    if (timer !== null) {
      clearTimeout(timer);
    }
    timer = setTimeout(() => {
      timer = null;
      fn(...args);
    }, delayMs);
  };
}

// ── Utility: findEntryPoint ───────────────────────────────────────────────

/**
 * Resolve the handler entry point for a project.
 * Priority: warp.toml [build].entry → src/index.ts fallback.
 */
export async function findEntryPoint(projectPath: string): Promise<string> {
  // Try warp.toml
  const warpTomlPath = join(projectPath, "warp.toml");
  try {
    const content = await readFile(warpTomlPath, "utf-8");
    const entryMatch = content.match(/entry\s*=\s*"([^"]+)"/);
    if (entryMatch) {
      const entry = entryMatch[1];
      await access(join(projectPath, entry));
      return entry;
    }
  } catch {
    // warp.toml not found or unreadable — fall through
  }

  // Fallback: src/index.ts
  const fallback = "src/index.ts";
  try {
    await access(join(projectPath, fallback));
    return fallback;
  } catch {
    // Not found
  }

  throw new Error(
    `No entry point found. Create warp.toml with [build].entry or add src/index.ts.`
  );
}

// ── CompileError ──────────────────────────────────────────────────────────

/**
 * Represents a compilation failure with stderr output and exit code.
 * The dev server displays this but does not crash.
 */
export class CompileError extends Error {
  readonly stderr: string;
  readonly exitCode: number;

  constructor(message: string, stderr: string, exitCode: number) {
    super(message);
    this.name = "CompileError";
    this.stderr = stderr;
    this.exitCode = exitCode;
  }

  override toString(): string {
    return `${this.message}\n--- stderr ---\n${this.stderr}`;
  }
}

// ── Native dev server ─────────────────────────────────────────────────────

interface NativeHandler {
  fetch(request: Request): Response | Promise<Response>;
}

async function loadNativeHandler(
  projectPath: string,
  entry: string,
): Promise<NativeHandler> {
  const fullPath = resolve(projectPath, entry);

  // Cache-bust by appending a timestamp query param
  const cacheBustUrl = `${fullPath}?t=${Date.now()}`;

  // Dynamic import. If the module has a syntax error or throws on load,
  // this will reject — caller must handle gracefully.
  const mod = await import(cacheBustUrl);

  // Support both `export default { fetch }` and `export default handler`
  const handler = mod.default;
  if (!handler || typeof handler.fetch !== "function") {
    throw new Error(
      `Handler at '${entry}' must export a default object with a fetch() method. ` +
      `Got: ${typeof handler}`
    );
  }

  return handler as NativeHandler;
}

class NativeDevServer implements DevServerHandle {
  private server: ReturnType<typeof Bun.serve> | null = null;
  private watcher: FSWatcher | null = null;
  private handler: NativeHandler | null = null;
  private lastError: string | null = null;
  private running = false;
  private compileAttempted = false;
  private readonly config: DevConfig;

  constructor(config: DevConfig) {
    this.config = config;
  }

  get port(): number {
    return this.server?.port ?? 0;
  }

  async start(): Promise<void> {
    // Initial handler load
    await this.reload();

    // Start HTTP server
    this.server = Bun.serve({
      port: this.config.port,
      fetch: async (req: Request) => {
        if (this.handler) {
          try {
            return await this.handler.fetch(req);
          } catch (err) {
            const message = err instanceof Error ? err.message : String(err);
            return new Response(
              JSON.stringify({ error: "Handler error", message }),
              { status: 500, headers: { "content-type": "application/json" } },
            );
          }
        }
        // No handler loaded — return error with last compilation error
        return new Response(
          JSON.stringify({
            error: "Handler not loaded",
            message: this.lastError ?? "Unknown error loading handler",
          }),
          { status: 503, headers: { "content-type": "application/json" } },
        );
      },
    });

    this.running = true;

    // Start file watcher
    this.startWatcher();
  }

  private async reload(): Promise<void> {
    this.compileAttempted = true;
    try {
      this.handler = await loadNativeHandler(
        this.config.projectPath,
        this.config.entry,
      );
      this.lastError = null;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      this.lastError = message;
      // Log error but don't crash — keep previous handler if available
      process.stderr.write(`[warp dev] Reload error: ${message}\n`);
    }
  }

  private startWatcher(): void {
    const srcDir = join(this.config.projectPath, "src");
    const debouncedReload = debounce(async () => {
      process.stderr.write(`[warp dev] Change detected, reloading...\n`);
      await this.reload();
    }, 100);

    try {
      this.watcher = watch(srcDir, { recursive: true }, (eventType, filename) => {
        if (filename && shouldWatch(filename, DEFAULT_WATCH_EXTENSIONS)) {
          debouncedReload();
        }
      });
    } catch {
      // If src/ doesn't exist, watch the project root
      this.watcher = watch(
        this.config.projectPath,
        { recursive: true },
        (eventType, filename) => {
          if (filename && shouldWatch(filename, DEFAULT_WATCH_EXTENSIONS)) {
            debouncedReload();
          }
        },
      );
    }
  }

  async stop(): Promise<void> {
    this.running = false;
    this.watcher?.close();
    this.watcher = null;
    this.server?.stop(true);
    this.server = null;
  }

  isRunning(): boolean {
    return this.running;
  }

  lastCompileAttempted(): boolean {
    return this.compileAttempted;
  }
}

// ── Wasm dev server ───────────────────────────────────────────────────────

class WasmDevServer implements DevServerHandle {
  private watcher: FSWatcher | null = null;
  private serveProcess: ChildProcess | null = null;
  private proxyServer: ReturnType<typeof Bun.serve> | null = null;
  private running = false;
  private compileAttempted = false;
  private lastError: string | null = null;
  private wasmReady = false;
  private jcoPort: number;
  private readonly config: DevConfig;

  constructor(config: DevConfig) {
    this.config = config;
    this.jcoPort = 0;
  }

  get port(): number {
    return this.proxyServer?.port ?? 0;
  }

  async start(): Promise<void> {
    // Initial compilation
    await this.compile();

    // Start proxy server that forwards to jco serve
    this.proxyServer = Bun.serve({
      port: this.config.port,
      fetch: async (req: Request) => {
        if (!this.wasmReady || this.jcoPort === 0) {
          return new Response(
            JSON.stringify({
              error: "Wasm module not ready",
              message: this.lastError ?? "Compiling...",
            }),
            { status: 503, headers: { "content-type": "application/json" } },
          );
        }

        // Proxy to jco serve
        try {
          const url = new URL(req.url);
          url.hostname = "localhost";
          url.port = String(this.jcoPort);
          const proxyReq = new Request(url.toString(), {
            method: req.method,
            headers: req.headers,
            body: req.body,
          });
          return await fetch(proxyReq);
        } catch (err) {
          const message = err instanceof Error ? err.message : String(err);
          return new Response(
            JSON.stringify({ error: "Proxy error", message }),
            { status: 502, headers: { "content-type": "application/json" } },
          );
        }
      },
    });

    this.running = true;
    this.startWatcher();
  }

  private async compile(): Promise<void> {
    this.compileAttempted = true;
    const projectPath = this.config.projectPath;
    const entry = this.config.entry;

    try {
      // Step 1: bun build
      const bundlePath = join(projectPath, "target", "bun-bundle", "handler.js");
      const bundleDir = join(projectPath, "target", "bun-bundle");
      await Bun.write(join(bundleDir, ".gitkeep"), "");

      const bunBuild = Bun.spawn(
        ["bun", "build", join(projectPath, entry), "--outfile", bundlePath, "--target", "browser", "--format", "esm"],
        { stdout: "pipe", stderr: "pipe" },
      );
      const bunResult = await bunBuild.exited;
      if (bunResult !== 0) {
        const stderr = await new Response(bunBuild.stderr).text();
        throw new CompileError("bun build failed", stderr, bunResult);
      }

      // Step 2: jco componentize
      const wasmPath = join(projectPath, "target", "wasm", "handler.wasm");
      const witDir = join(projectPath, "wit");

      // Check if wit dir and jco exist before attempting
      try {
        await access(witDir);
      } catch {
        this.lastError = "WIT directory not found at wit/. Cannot compile to Wasm.";
        process.stderr.write(`[warp dev] ${this.lastError}\n`);
        return;
      }

      const jcoComponentize = Bun.spawn(
        [
          "jco", "componentize", bundlePath,
          "--wit", witDir,
          "--world-name", "handler",
          "--enable", "http",
          "--enable", "fetch-event",
          "-o", wasmPath,
        ],
        { stdout: "pipe", stderr: "pipe" },
      );
      const jcoResult = await jcoComponentize.exited;
      if (jcoResult !== 0) {
        const stderr = await new Response(jcoComponentize.stderr).text();
        throw new CompileError("jco componentize failed", stderr, jcoResult);
      }

      // Step 3: Start/restart jco serve
      await this.startJcoServe(wasmPath);
      this.lastError = null;
    } catch (err) {
      const message = err instanceof Error ? err.toString() : String(err);
      this.lastError = message;
      process.stderr.write(`[warp dev] Compile error: ${message}\n`);
    }
  }

  private async startJcoServe(wasmPath: string): Promise<void> {
    // Kill existing serve process
    if (this.serveProcess) {
      this.serveProcess.kill();
      this.serveProcess = null;
      this.wasmReady = false;
    }

    // Find a free port for jco serve
    const tempServer = Bun.serve({ port: 0, fetch: () => new Response("") });
    this.jcoPort = tempServer.port ?? 0;
    tempServer.stop(true);

    this.serveProcess = spawn(
      "jco",
      ["serve", wasmPath, "--port", String(this.jcoPort)],
      { stdio: ["ignore", "pipe", "pipe"] },
    );

    // Wait for jco serve to be ready (poll with timeout)
    const startTime = Date.now();
    while (Date.now() - startTime < 10000) {
      await new Promise((resolve) => setTimeout(resolve, 200));
      try {
        const res = await fetch(`http://localhost:${this.jcoPort}/`);
        if (res.status > 0) {
          this.wasmReady = true;
          return;
        }
      } catch {
        // Not ready yet
      }
    }

    this.lastError = "jco serve did not become ready within 10 seconds";
    process.stderr.write(`[warp dev] ${this.lastError}\n`);
  }

  private startWatcher(): void {
    const srcDir = join(this.config.projectPath, "src");
    const debouncedRecompile = debounce(async () => {
      process.stderr.write(`[warp dev] Change detected, recompiling...\n`);
      await this.compile();
    }, 200);

    try {
      this.watcher = watch(srcDir, { recursive: true }, (eventType, filename) => {
        if (filename && shouldWatch(filename, DEFAULT_WATCH_EXTENSIONS)) {
          debouncedRecompile();
        }
      });
    } catch {
      this.watcher = watch(
        this.config.projectPath,
        { recursive: true },
        (eventType, filename) => {
          if (filename && shouldWatch(filename, DEFAULT_WATCH_EXTENSIONS)) {
            debouncedRecompile();
          }
        },
      );
    }
  }

  async stop(): Promise<void> {
    this.running = false;
    this.watcher?.close();
    this.watcher = null;
    this.serveProcess?.kill();
    this.serveProcess = null;
    this.proxyServer?.stop(true);
    this.proxyServer = null;
  }

  isRunning(): boolean {
    return this.running;
  }

  lastCompileAttempted(): boolean {
    return this.compileAttempted;
  }
}

// ── Factory ───────────────────────────────────────────────────────────────

/**
 * Create and start a dev server.
 *
 * In native mode, the handler is loaded directly into Bun's runtime.
 * In Wasm mode, the handler is compiled and served via jco serve.
 */
export async function createDevServer(config: DevConfig): Promise<DevServerHandle> {
  const server = config.native
    ? new NativeDevServer(config)
    : new WasmDevServer(config);

  await server.start();
  return server;
}
