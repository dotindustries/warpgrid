import { describe, it, expect } from "bun:test";
import {
  createBunEnvProxy,
  createBunFile,
  bunSleep,
  bunServe,
  installPolyfills,
  type WasiEnvProvider,
  type WasiFilesystemProvider,
  type WasiClockProvider,
} from "./index.ts";

// ── Bun.env tests ────────────────────────────────────────────────────

describe("Bun.env polyfill", () => {
  it("reads environment variables from the WASI provider", () => {
    const provider: WasiEnvProvider = {
      get(key: string): string | undefined {
        const vars: Record<string, string> = {
          FOO: "bar",
          DB_HOST: "localhost",
        };
        return vars[key];
      },
      getAll(): Record<string, string> {
        return { FOO: "bar", DB_HOST: "localhost" };
      },
    };

    const env = createBunEnvProxy(provider);
    expect(env.FOO).toBe("bar");
    expect(env.DB_HOST).toBe("localhost");
  });

  it("returns undefined for unset variables", () => {
    const provider: WasiEnvProvider = {
      get(): string | undefined {
        return undefined;
      },
      getAll(): Record<string, string> {
        return {};
      },
    };

    const env = createBunEnvProxy(provider);
    expect(env.NONEXISTENT).toBeUndefined();
  });

  it("supports iteration via Object.keys when provider supports getAll", () => {
    const provider: WasiEnvProvider = {
      get(key: string): string | undefined {
        const vars: Record<string, string> = { A: "1", B: "2" };
        return vars[key];
      },
      getAll(): Record<string, string> {
        return { A: "1", B: "2" };
      },
    };

    const env = createBunEnvProxy(provider);
    const keys = Object.keys(env);
    expect(keys).toContain("A");
    expect(keys).toContain("B");
  });

  it("supports 'in' operator", () => {
    const provider: WasiEnvProvider = {
      get(key: string): string | undefined {
        return key === "EXISTS" ? "yes" : undefined;
      },
      getAll(): Record<string, string> {
        return { EXISTS: "yes" };
      },
    };

    const env = createBunEnvProxy(provider);
    expect("EXISTS" in env).toBe(true);
    expect("MISSING" in env).toBe(false);
  });
});

// ── Bun.sleep tests ──────────────────────────────────────────────────

describe("Bun.sleep polyfill", () => {
  it("resolves after approximately the requested duration", async () => {
    const sleepCalls: number[] = [];
    const provider: WasiClockProvider = {
      sleep(ms: number): Promise<void> {
        sleepCalls.push(ms);
        return new Promise((resolve) => setTimeout(resolve, ms));
      },
    };

    const start = performance.now();
    await bunSleep(100, provider);
    const elapsed = performance.now() - start;

    expect(elapsed).toBeGreaterThanOrEqual(50);
    expect(elapsed).toBeLessThan(250);
    expect(sleepCalls).toEqual([100]);
  });

  it("resolves immediately for 0ms sleep", async () => {
    const provider: WasiClockProvider = {
      sleep(ms: number): Promise<void> {
        return new Promise((resolve) => setTimeout(resolve, ms));
      },
    };

    const start = performance.now();
    await bunSleep(0, provider);
    const elapsed = performance.now() - start;

    expect(elapsed).toBeLessThan(50);
  });

  it("delegates to the WASI clock provider", async () => {
    let calledWith: number | undefined;
    const provider: WasiClockProvider = {
      sleep(ms: number): Promise<void> {
        calledWith = ms;
        return Promise.resolve();
      },
    };

    await bunSleep(42, provider);
    expect(calledWith).toBe(42);
  });
});

// ── Bun.file tests ───────────────────────────────────────────────────

describe("Bun.file polyfill", () => {
  it("returns a BunFile-compatible object with text()", async () => {
    const provider: WasiFilesystemProvider = {
      readFile(path: string): Uint8Array {
        if (path === "/etc/config.txt") {
          return new TextEncoder().encode("hello config");
        }
        throw new Error(`ENOENT: ${path}`);
      },
      stat(path: string): { size: number } | null {
        if (path === "/etc/config.txt") return { size: 12 };
        return null;
      },
    };

    const file = createBunFile("/etc/config.txt", provider);
    const text = await file.text();
    expect(text).toBe("hello config");
  });

  it("returns a BunFile-compatible object with arrayBuffer()", async () => {
    const content = new Uint8Array([0x00, 0x01, 0x02, 0x03]);
    const provider: WasiFilesystemProvider = {
      readFile(path: string): Uint8Array {
        if (path === "/data/binary.bin") return content;
        throw new Error(`ENOENT: ${path}`);
      },
      stat(path: string): { size: number } | null {
        if (path === "/data/binary.bin") return { size: 4 };
        return null;
      },
    };

    const file = createBunFile("/data/binary.bin", provider);
    const buf = await file.arrayBuffer();
    expect(new Uint8Array(buf)).toEqual(content);
  });

  it("exposes the file size via .size", () => {
    const provider: WasiFilesystemProvider = {
      readFile(): Uint8Array {
        return new Uint8Array(256);
      },
      stat(path: string): { size: number } | null {
        if (path === "/data/large.dat") return { size: 256 };
        return null;
      },
    };

    const file = createBunFile("/data/large.dat", provider);
    expect(file.size).toBe(256);
  });

  it("exposes the file name", () => {
    const provider: WasiFilesystemProvider = {
      readFile(): Uint8Array {
        return new Uint8Array(0);
      },
      stat(): { size: number } | null {
        return { size: 0 };
      },
    };

    const file = createBunFile("/etc/hosts", provider);
    expect(file.name).toBe("/etc/hosts");
  });

  it("returns size 0 when stat returns null (file not found)", () => {
    const provider: WasiFilesystemProvider = {
      readFile(): Uint8Array {
        throw new Error("ENOENT");
      },
      stat(): { size: number } | null {
        return null;
      },
    };

    const file = createBunFile("/missing", provider);
    expect(file.size).toBe(0);
  });

  it("propagates readFile errors through text()", async () => {
    const provider: WasiFilesystemProvider = {
      readFile(): Uint8Array {
        throw new Error("ENOENT: /nope");
      },
      stat(): { size: number } | null {
        return null;
      },
    };

    const file = createBunFile("/nope", provider);
    expect(file.text()).rejects.toThrow("ENOENT");
  });
});

// ── Bun.serve tests ──────────────────────────────────────────────────

describe("Bun.serve polyfill", () => {
  it("throws a descriptive error explaining WarpGridHandler", () => {
    expect(() => bunServe({})).toThrow();
  });

  it("error message mentions WarpGridHandler", () => {
    try {
      bunServe({ port: 3000, fetch: () => new Response("ok") });
      // Should not reach here
      expect(true).toBe(false);
    } catch (e: unknown) {
      const error = e as Error;
      expect(error.message).toContain("WarpGridHandler");
      expect(error.message).toContain("WarpGrid");
    }
  });
});

// ── installPolyfills tests ───────────────────────────────────────────
// Note: In native Bun, `globalThis.Bun` is non-configurable and non-writable,
// so we can't delete or reassign it during tests. Instead, we use the `target`
// option to install polyfills on a fresh object and verify the result.

describe("installPolyfills", () => {
  it("installs Bun global with env, file, sleep, serve on target", () => {
    const target: Record<string, unknown> = {};

    const envProvider: WasiEnvProvider = {
      get(key: string): string | undefined {
        return key === "TEST_VAR" ? "test_value" : undefined;
      },
      getAll(): Record<string, string> {
        return { TEST_VAR: "test_value" };
      },
    };

    const fsProvider: WasiFilesystemProvider = {
      readFile(): Uint8Array {
        return new TextEncoder().encode("mock");
      },
      stat(): { size: number } | null {
        return { size: 4 };
      },
    };

    const clockProvider: WasiClockProvider = {
      sleep(ms: number): Promise<void> {
        return new Promise((resolve) => setTimeout(resolve, ms));
      },
    };

    installPolyfills({
      env: envProvider,
      fs: fsProvider,
      clock: clockProvider,
      target,
    });

    const bunGlobal = target.Bun as Record<string, unknown>;
    expect(bunGlobal).toBeDefined();
    expect(bunGlobal.env).toBeDefined();
    expect(typeof bunGlobal.file).toBe("function");
    expect(typeof bunGlobal.sleep).toBe("function");
    expect(typeof bunGlobal.serve).toBe("function");
  });

  it("Bun.env reads from the injected provider after install", () => {
    const target: Record<string, unknown> = {};

    const envProvider: WasiEnvProvider = {
      get(key: string): string | undefined {
        return key === "MY_VAR" ? "hello" : undefined;
      },
      getAll(): Record<string, string> {
        return { MY_VAR: "hello" };
      },
    };

    installPolyfills({ env: envProvider, target });

    const bunGlobal = target.Bun as Record<string, unknown>;
    const env = bunGlobal.env as Record<string, string | undefined>;
    expect(env.MY_VAR).toBe("hello");
    expect(env.UNDEFINED_VAR).toBeUndefined();
  });

  it("sets __WARPGRID_WASM__ marker on target", () => {
    const target: Record<string, unknown> = {};

    installPolyfills({ target });

    expect(target.__WARPGRID_WASM__).toBe(true);
  });

  it("Bun.file returns a BunFile from the installed polyfill", async () => {
    const target: Record<string, unknown> = {};

    const fsProvider: WasiFilesystemProvider = {
      readFile(path: string): Uint8Array {
        if (path === "/etc/test") return new TextEncoder().encode("content");
        throw new Error("ENOENT");
      },
      stat(path: string): { size: number } | null {
        if (path === "/etc/test") return { size: 7 };
        return null;
      },
    };

    installPolyfills({ fs: fsProvider, target });

    const bunGlobal = target.Bun as Record<string, unknown>;
    const fileFn = bunGlobal.file as (path: string) => { text(): Promise<string>; size: number };
    const file = fileFn("/etc/test");
    expect(file.size).toBe(7);
    expect(await file.text()).toBe("content");
  });

  it("Bun.serve throws descriptive error from installed polyfill", () => {
    const target: Record<string, unknown> = {};
    installPolyfills({ target });

    const bunGlobal = target.Bun as Record<string, unknown>;
    const serveFn = bunGlobal.serve as (opts: unknown) => never;
    expect(() => serveFn({})).toThrow("WarpGridHandler");
  });
});
