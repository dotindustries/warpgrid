import { describe, test, expect, afterEach } from "bun:test";
import { createPool, detectMode } from "../src/postgres.ts";
import { WasmPool } from "../src/postgres-wasm.ts";
import { NativePool } from "../src/postgres-native.ts";
import type { DatabaseProxyShim } from "../src/postgres.ts";
import { WarpGridDatabaseError } from "../src/errors.ts";

// ── Minimal mock shim for factory tests ─────────────────────────────

function createMockShim(): DatabaseProxyShim {
  return {
    connect: () => 1,
    send: () => 0,
    recv: () => new Uint8Array(0),
    close: () => {},
  };
}

// ── Tests ───────────────────────────────────────────────────────────

describe("detectMode", () => {
  test("returns 'native' when Bun global exists", () => {
    // In this test runner (Bun), Bun global exists
    const mode = detectMode();
    expect(mode).toBe("native");
  });

  test("returns 'wasm' when __WARPGRID_WASM__ is set", () => {
    (globalThis as Record<string, unknown>).__WARPGRID_WASM__ = true;
    try {
      expect(detectMode()).toBe("wasm");
    } finally {
      delete (globalThis as Record<string, unknown>).__WARPGRID_WASM__;
    }
  });
});

describe("createPool", () => {
  afterEach(() => {
    delete (globalThis as Record<string, unknown>).__WARPGRID_WASM__;
  });

  test("with mode='wasm' and shim creates WasmPool", () => {
    const pool = createPool({
      mode: "wasm",
      shim: createMockShim(),
    });
    expect(pool).toBeInstanceOf(WasmPool);
  });

  test("with mode='native' creates NativePool", () => {
    const pool = createPool({ mode: "native" });
    expect(pool).toBeInstanceOf(NativePool);
  });

  test("with mode='auto' in Bun creates NativePool", () => {
    const pool = createPool({ mode: "auto" });
    expect(pool).toBeInstanceOf(NativePool);
  });

  test("with no mode defaults to auto-detect", () => {
    // Running in Bun → should create NativePool
    const pool = createPool();
    expect(pool).toBeInstanceOf(NativePool);
  });

  test("wasm mode without shim or globals throws", () => {
    expect(() => createPool({ mode: "wasm" })).toThrow(
      "Wasm mode requires a DatabaseProxyShim",
    );
  });

  test("wasm mode with globalThis.warpgrid.database works", () => {
    const g = globalThis as Record<string, unknown>;
    g.warpgrid = { database: createMockShim() };
    try {
      const pool = createPool({ mode: "wasm" });
      expect(pool).toBeInstanceOf(WasmPool);
    } finally {
      delete g.warpgrid;
    }
  });

  test("createPool passes config to the pool", () => {
    const pool = createPool({
      mode: "wasm",
      shim: createMockShim(),
      host: "custom-host",
      port: 9999,
      database: "mydb",
      user: "myuser",
      maxConnections: 5,
    }) as WasmPool;

    // Pool should start empty
    expect(pool.getPoolSize()).toBe(0);
    expect(pool.getIdleCount()).toBe(0);
  });
});

describe("Pool interface contract", () => {
  test("WasmPool implements Pool interface", () => {
    const pool = new WasmPool({}, createMockShim());
    expect(typeof pool.query).toBe("function");
    expect(typeof pool.end).toBe("function");
    expect(typeof pool.getPoolSize).toBe("function");
    expect(typeof pool.getIdleCount).toBe("function");
  });

  test("NativePool implements Pool interface", () => {
    const pool = new NativePool({});
    expect(typeof pool.query).toBe("function");
    expect(typeof pool.end).toBe("function");
    expect(typeof pool.getPoolSize).toBe("function");
    expect(typeof pool.getIdleCount).toBe("function");
  });
});
