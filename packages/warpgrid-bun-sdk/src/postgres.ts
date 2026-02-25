/**
 * @warpgrid/bun-sdk/postgres — Dual-mode Postgres connection pool.
 *
 * Provides a `createPool()` function that returns a Pool with `query()`,
 * `end()`, `getPoolSize()`, and `getIdleCount()` methods.
 *
 * - In **native mode** (Bun development), delegates to the `pg` npm package.
 * - In **Wasm mode** (deployed as WASI component), delegates to the
 *   Domain 1 database proxy shim via Postgres wire protocol.
 *
 * Mode is auto-detected but can be overridden via `config.mode`.
 */

export { WarpGridDatabaseError } from "./errors.ts";

// ── Public Types ──────────────────────────────────────────────────────

/** Configuration for creating a Postgres connection pool. */
export interface PoolConfig {
  /** Database hostname. Default: "localhost". */
  host?: string;
  /** Database port. Default: 5432. */
  port?: number;
  /** Database name. Default: "postgres". */
  database?: string;
  /** Authentication username. Default: "postgres". */
  user?: string;
  /** Authentication password. */
  password?: string;
  /** Maximum number of connections in the pool. Default: 10. */
  maxConnections?: number;
  /** Idle connection timeout in milliseconds. Default: 30000. */
  idleTimeout?: number;
  /**
   * Execution mode override.
   * - `"native"`: Use the `pg` npm package (requires Bun runtime).
   * - `"wasm"`: Use the WarpGrid database proxy shim.
   * - `"auto"`: Auto-detect based on runtime environment (default).
   */
  mode?: "native" | "wasm" | "auto";
  /**
   * Injected database proxy shim functions (Wasm mode).
   * Auto-detected from globals if not provided.
   * Useful for testing with mock shims.
   */
  shim?: DatabaseProxyShim;
}

/** Result of a SQL query. */
export interface QueryResult {
  /** Array of row objects keyed by column name. */
  rows: Record<string, unknown>[];
  /** Number of rows affected (for INSERT/UPDATE/DELETE) or returned. */
  rowCount: number;
  /** Column metadata. */
  fields: FieldInfo[];
}

/** Column metadata from RowDescription. */
export interface FieldInfo {
  /** Column name. */
  name: string;
  /** Postgres data type OID. */
  dataTypeID: number;
}

/** Postgres connection pool interface. */
export interface Pool {
  /** Execute a SQL query with optional parameters ($1, $2, ...). */
  query(sql: string, params?: unknown[]): Promise<QueryResult>;
  /** Close all connections and shut down the pool. */
  end(): Promise<void>;
  /** Total number of connections (active + idle). */
  getPoolSize(): number;
  /** Number of idle (available) connections. */
  getIdleCount(): number;
}

/**
 * Low-level database proxy shim interface.
 * Mirrors the `warpgrid:shim/database-proxy@0.1.0` WIT interface.
 */
export interface DatabaseProxyShim {
  connect(config: {
    host: string;
    port: number;
    database: string;
    user: string;
    password?: string;
  }): number;
  send(handle: number, data: Uint8Array): number;
  recv(handle: number, maxBytes: number): Uint8Array;
  close(handle: number): void;
}

// ── Internals ─────────────────────────────────────────────────────────

import { NativePool } from "./postgres-native.ts";
import { WasmPool } from "./postgres-wasm.ts";

export { WasmPool } from "./postgres-wasm.ts";
export { NativePool } from "./postgres-native.ts";

/** Detect whether we're running in native Bun or WASI/Wasm mode. */
export function detectMode(): "native" | "wasm" {
  if (typeof (globalThis as Record<string, unknown>).Bun !== "undefined") {
    if (
      (globalThis as Record<string, unknown>).__WARPGRID_WASM__ !== undefined
    ) {
      return "wasm";
    }
    return "native";
  }
  return "wasm";
}

/**
 * Resolve the database proxy shim from config or globals.
 * Returns undefined if not available (caller should throw).
 */
function resolveShim(config?: PoolConfig): DatabaseProxyShim | undefined {
  if (config?.shim) return config.shim;
  const g = globalThis as Record<string, unknown>;
  const wg = g.warpgrid as Record<string, unknown> | undefined;
  return wg?.database as DatabaseProxyShim | undefined;
}

// ── Factory ───────────────────────────────────────────────────────────

/**
 * Create a Postgres connection pool.
 *
 * Auto-detects native (Bun) vs Wasm mode, or override with `config.mode`.
 */
export function createPool(config?: PoolConfig): Pool {
  const mode =
    config?.mode === "auto" || !config?.mode ? detectMode() : config.mode;

  if (mode === "native") {
    return new NativePool(config);
  }

  const shim = resolveShim(config);
  if (!shim) {
    throw new Error(
      "Wasm mode requires a DatabaseProxyShim. " +
        "Provide config.shim or ensure globalThis.warpgrid.database is set.",
    );
  }
  return new WasmPool(config, shim);
}
