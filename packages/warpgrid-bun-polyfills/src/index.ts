/**
 * @warpgrid/bun-polyfills — Bun-API-compatible shims for WASI environments.
 *
 * Provides `Bun.env`, `Bun.sleep()`, `Bun.file()`, and `Bun.serve()` that
 * delegate to WASI equivalents when running inside a Wasm component.
 *
 * In native Bun development, these polyfills are NOT loaded — the real
 * `Bun` global is used instead. Polyfills are auto-injected by
 * `warp pack --lang bun` during the bundle step.
 */

// ── Provider Interfaces ──────────────────────────────────────────────
// These abstract the WASI runtime layer, allowing mock injection in tests
// and real WASI bindings at bundle time.

/** Provides access to WASI environment variables. */
export interface WasiEnvProvider {
  /** Get a single environment variable by key. */
  get(key: string): string | undefined;
  /** Get all environment variables as a record. */
  getAll(): Record<string, string>;
}

/** Provides access to the WASI filesystem. */
export interface WasiFilesystemProvider {
  /** Read a file's contents as bytes. Throws on error. */
  readFile(path: string): Uint8Array;
  /** Get file metadata. Returns null if file doesn't exist. */
  stat(path: string): { size: number } | null;
}

/** Provides WASI clock/timer functionality. */
export interface WasiClockProvider {
  /** Sleep for the given number of milliseconds. */
  sleep(ms: number): Promise<void>;
}

/** BunFile-compatible interface returned by Bun.file(). */
export interface BunFileShim {
  /** File path. */
  readonly name: string;
  /** File size in bytes (0 if unknown or not found). */
  readonly size: number;
  /** Read file contents as a string. */
  text(): Promise<string>;
  /** Read file contents as an ArrayBuffer. */
  arrayBuffer(): Promise<ArrayBuffer>;
}

// ── Provider configuration for installPolyfills ──────────────────────

export interface PolyfillProviders {
  env?: WasiEnvProvider;
  fs?: WasiFilesystemProvider;
  clock?: WasiClockProvider;
  /**
   * Target object to install the `Bun` shim on. Defaults to `globalThis`.
   * Useful for testing without modifying the real `globalThis.Bun`.
   */
  target?: Record<string, unknown>;
}

// ── Bun.env ──────────────────────────────────────────────────────────

/**
 * Create a Proxy-based `Bun.env` that reads from a WASI environment provider.
 *
 * Supports property access (`env.FOO`), the `in` operator, and
 * `Object.keys()` for iteration.
 */
export function createBunEnvProxy(
  provider: WasiEnvProvider,
): Record<string, string | undefined> {
  return new Proxy({} as Record<string, string | undefined>, {
    get(_target, prop: string | symbol): string | undefined {
      if (typeof prop === "symbol") return undefined;
      return provider.get(prop);
    },
    has(_target, prop: string | symbol): boolean {
      if (typeof prop === "symbol") return false;
      return provider.get(prop) !== undefined;
    },
    ownKeys(): string[] {
      return Object.keys(provider.getAll());
    },
    getOwnPropertyDescriptor(_target, prop: string | symbol) {
      if (typeof prop === "symbol") return undefined;
      const value = provider.get(prop);
      if (value === undefined) return undefined;
      return {
        value,
        writable: false,
        enumerable: true,
        configurable: true,
      };
    },
  });
}

// ── Bun.sleep ────────────────────────────────────────────────────────

/**
 * Sleep for the given number of milliseconds using the WASI clock provider.
 * Equivalent to `await Bun.sleep(ms)`.
 */
export function bunSleep(
  ms: number,
  provider: WasiClockProvider,
): Promise<void> {
  return provider.sleep(ms);
}

// ── Bun.file ─────────────────────────────────────────────────────────

/**
 * Create a BunFile-compatible object backed by the WASI filesystem provider.
 * Equivalent to `Bun.file(path)`.
 */
export function createBunFile(
  path: string,
  provider: WasiFilesystemProvider,
): BunFileShim {
  const statResult = provider.stat(path);
  const size = statResult?.size ?? 0;

  return {
    name: path,
    size,
    text(): Promise<string> {
      return new Promise((resolve, reject) => {
        try {
          const bytes = provider.readFile(path);
          resolve(new TextDecoder().decode(bytes));
        } catch (err) {
          reject(err);
        }
      });
    },
    arrayBuffer(): Promise<ArrayBuffer> {
      return new Promise((resolve, reject) => {
        try {
          const bytes = provider.readFile(path);
          // Copy into a fresh ArrayBuffer to avoid SharedArrayBuffer type issues
          const buf = new ArrayBuffer(bytes.byteLength);
          new Uint8Array(buf).set(bytes);
          resolve(buf);
        } catch (err) {
          reject(err);
        }
      });
    },
  };
}

// ── Bun.serve ────────────────────────────────────────────────────────

/**
 * Throws a descriptive error explaining that `Bun.serve()` is not available
 * in WarpGrid. Users should export a `WarpGridHandler` instead.
 */
export function bunServe(_options: unknown): never {
  throw new Error(
    "Bun.serve() is not available in WarpGrid WASI modules. " +
      "WarpGrid manages the HTTP listener automatically. " +
      "Export a WarpGridHandler from your module instead:\n\n" +
      "  export default {\n" +
      "    fetch(request: Request): Response {\n" +
      '      return new Response("ok");\n' +
      "    }\n" +
      "  } satisfies WarpGridHandler;\n",
  );
}

// ── installPolyfills ─────────────────────────────────────────────────

/** Default providers that use basic fallbacks when no WASI layer is available. */
const defaultEnvProvider: WasiEnvProvider = {
  get(): string | undefined {
    return undefined;
  },
  getAll(): Record<string, string> {
    return {};
  },
};

const defaultFsProvider: WasiFilesystemProvider = {
  readFile(path: string): Uint8Array {
    throw new Error(`ENOENT: no such file: ${path}`);
  },
  stat(): { size: number } | null {
    return null;
  },
};

const defaultClockProvider: WasiClockProvider = {
  sleep(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms));
  },
};

/**
 * Install the Bun polyfills on a target object (defaults to `globalThis`).
 *
 * Call this at module initialization in Wasm mode. Providers are optional —
 * defaults provide no-op behavior (empty env, ENOENT filesystem, setTimeout clock).
 *
 * Also sets `target.__WARPGRID_WASM__` so the SDK can detect Wasm mode.
 */
export function installPolyfills(providers: PolyfillProviders = {}): void {
  const envProvider = providers.env ?? defaultEnvProvider;
  const fsProvider = providers.fs ?? defaultFsProvider;
  const clockProvider = providers.clock ?? defaultClockProvider;
  const target = providers.target ?? (globalThis as Record<string, unknown>);

  const bunShim = {
    env: createBunEnvProxy(envProvider),
    file(path: string): BunFileShim {
      return createBunFile(path, fsProvider);
    },
    sleep(ms: number): Promise<void> {
      return bunSleep(ms, clockProvider);
    },
    serve(options: unknown): never {
      return bunServe(options);
    },
  };

  target.Bun = bunShim;
  target.__WARPGRID_WASM__ = true;
}
