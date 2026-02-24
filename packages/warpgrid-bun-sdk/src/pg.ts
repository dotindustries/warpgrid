/**
 * @warpgrid/bun-sdk/pg — pg.Client-compatible Postgres client.
 *
 * Provides a `Client` class with the standard pg interface:
 * `connect()`, `query(sql, params?)`, `end()`.
 *
 * - In **native mode** (Bun development), delegates to the `pg` npm package.
 * - In **Wasm mode** (deployed as WASI component), delegates to the
 *   Domain 1 database proxy shim via Postgres wire protocol.
 *
 * Mode is auto-detected but can be overridden via `config.mode`.
 */

export { PostgresError } from "./errors.ts";

import type { QueryResult, FieldInfo, DatabaseProxyShim } from "./postgres.ts";
import { detectMode } from "./postgres.ts";
import { WarpGridDatabaseError } from "./errors.ts";
import { WasmPgClient } from "./pg-client-wasm.ts";
import { NativePgClient } from "./pg-client-native.ts";

// Re-export types that callers need
export type { QueryResult, FieldInfo, DatabaseProxyShim };

// ── Public Types ──────────────────────────────────────────────────────

/** Configuration for creating a pg.Client-compatible connection. */
export interface PgClientConfig {
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

// ── Internals ─────────────────────────────────────────────────────────

/**
 * Resolve the database proxy shim from config or globals.
 * Returns undefined if not available.
 */
function resolveShim(config?: PgClientConfig): DatabaseProxyShim | undefined {
  if (config?.shim) return config.shim;
  const g = globalThis as Record<string, unknown>;
  const wg = g.warpgrid as Record<string, unknown> | undefined;
  return wg?.database as DatabaseProxyShim | undefined;
}

// ── Client Backend Interface ──────────────────────────────────────────

interface ClientBackend {
  connect(): Promise<void>;
  query(sql: string, params?: unknown[]): Promise<QueryResult>;
  end(): Promise<void>;
}

// ── Client Class ──────────────────────────────────────────────────────

/**
 * pg.Client-compatible Postgres client for WarpGrid.
 *
 * Provides explicit connection lifecycle management:
 * 1. `new Client(config)` — configure
 * 2. `await client.connect()` — establish connection
 * 3. `await client.query(sql, params?)` — execute queries
 * 4. `await client.end()` — close connection
 */
export class Client {
  private readonly config: PgClientConfig;
  private readonly mode: "native" | "wasm";
  private backend: ClientBackend | null = null;
  private ended = false;

  constructor(config?: PgClientConfig) {
    this.config = config ?? {};
    this.mode =
      this.config.mode === "auto" || !this.config.mode
        ? detectMode()
        : this.config.mode;
  }

  /**
   * Establish a connection to the Postgres server.
   * Performs the Postgres startup/authentication handshake.
   */
  async connect(): Promise<void> {
    if (this.ended) {
      throw new WarpGridDatabaseError("Client has ended");
    }
    if (this.backend) {
      throw new WarpGridDatabaseError("Already connected");
    }

    this.backend = this.createBackend();
    try {
      await this.backend.connect();
    } catch (err) {
      this.backend = null;
      throw err;
    }
  }

  /**
   * Execute a SQL query with optional parameters ($1, $2, ...).
   * Returns `{ rows, rowCount, fields }`.
   */
  async query(sql: string, params?: unknown[]): Promise<QueryResult> {
    if (!this.backend) {
      throw new WarpGridDatabaseError("Not connected");
    }
    return this.backend.query(sql, params);
  }

  /**
   * Cleanly close the connection.
   * Sends a Terminate message before closing.
   * Safe to call multiple times.
   */
  async end(): Promise<void> {
    this.ended = true;
    if (!this.backend) return;
    const backend = this.backend;
    this.backend = null;
    await backend.end();
  }

  // ── Private ─────────────────────────────────────────────────────

  private createBackend(): ClientBackend {
    if (this.mode === "native") {
      return new NativePgClient(this.config);
    }

    const shim = resolveShim(this.config);
    if (!shim) {
      throw new WarpGridDatabaseError(
        "Wasm mode requires a DatabaseProxyShim. " +
          "Provide config.shim or ensure globalThis.warpgrid.database is set.",
      );
    }
    return new WasmPgClient(this.config, shim);
  }
}
