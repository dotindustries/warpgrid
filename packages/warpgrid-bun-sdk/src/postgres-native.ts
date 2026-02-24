/**
 * NativePool — Postgres pool for native Bun development mode.
 *
 * Delegates to the `pg` npm package, which uses real TCP connections.
 * Provides the same Pool interface as WasmPool for seamless dual-mode.
 */

import type { Pool, PoolConfig, QueryResult, FieldInfo } from "./postgres.ts";
import { WarpGridDatabaseError } from "./errors.ts";

/**
 * Dynamic pg import. The `pg` package is an optional dependency — only
 * required when running in native mode. This avoids import failures in
 * Wasm environments where pg is not available.
 */
let pgModule: typeof import("pg") | null = null;

async function getPg(): Promise<typeof import("pg")> {
  if (pgModule) return pgModule;
  try {
    pgModule = await import("pg");
    return pgModule;
  } catch (err) {
    throw new WarpGridDatabaseError(
      'Native mode requires the "pg" package. Install it with: bun add pg',
      { cause: err },
    );
  }
}

export class NativePool implements Pool {
  private pgPool: InstanceType<typeof import("pg").Pool> | null = null;
  private readonly poolConfig: PoolConfig;
  private initPromise: Promise<void> | null = null;

  constructor(config?: PoolConfig) {
    this.poolConfig = config ?? {};
  }

  private async ensurePool(): Promise<InstanceType<typeof import("pg").Pool>> {
    if (this.pgPool) return this.pgPool;

    if (!this.initPromise) {
      this.initPromise = this.initPool();
    }
    await this.initPromise;
    return this.pgPool!;
  }

  private async initPool(): Promise<void> {
    const pg = await getPg();
    this.pgPool = new pg.Pool({
      host: this.poolConfig.host ?? "localhost",
      port: this.poolConfig.port ?? 5432,
      database: this.poolConfig.database ?? "postgres",
      user: this.poolConfig.user ?? "postgres",
      password: this.poolConfig.password,
      max: this.poolConfig.maxConnections ?? 10,
      idleTimeoutMillis: this.poolConfig.idleTimeout ?? 30000,
    });
  }

  async query(sql: string, params?: unknown[]): Promise<QueryResult> {
    const pool = await this.ensurePool();
    try {
      const result = await pool.query(sql, params);
      const fields: FieldInfo[] = (result.fields ?? []).map((f) => ({
        name: f.name,
        dataTypeID: f.dataTypeID,
      }));
      return {
        rows: result.rows as Record<string, unknown>[],
        rowCount: result.rowCount ?? result.rows.length,
        fields,
      };
    } catch (err) {
      throw new WarpGridDatabaseError(
        `Query failed: ${err instanceof Error ? err.message : String(err)}`,
        { cause: err },
      );
    }
  }

  async end(): Promise<void> {
    if (this.pgPool) {
      try {
        await this.pgPool.end();
      } catch (err) {
        throw new WarpGridDatabaseError("Failed to close connection pool", {
          cause: err,
        });
      } finally {
        this.pgPool = null;
        this.initPromise = null;
      }
    }
  }

  getPoolSize(): number {
    return this.pgPool?.totalCount ?? 0;
  }

  getIdleCount(): number {
    return this.pgPool?.idleCount ?? 0;
  }
}
