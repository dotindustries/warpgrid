/**
 * NativePgClient — Single-connection Postgres client for native Bun mode.
 *
 * Delegates to the `pg` npm package's `Client` class, which uses real
 * TCP connections. Provides the same interface as WasmPgClient for
 * seamless dual-mode operation.
 */

import type { QueryResult, FieldInfo } from "./postgres.ts";
import { WarpGridDatabaseError, PostgresError } from "./errors.ts";
import type { PgClientConfig } from "./pg.ts";

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

export class NativePgClient {
  private pgClient: InstanceType<typeof import("pg").Client> | null = null;
  private readonly clientConfig: PgClientConfig;
  private connected = false;
  private ended = false;

  constructor(config?: PgClientConfig) {
    this.clientConfig = config ?? {};
  }

  async connect(): Promise<void> {
    if (this.connected) {
      throw new WarpGridDatabaseError("Already connected");
    }
    if (this.ended) {
      throw new WarpGridDatabaseError("Client has ended");
    }

    const pg = await getPg();
    this.pgClient = new pg.Client({
      host: this.clientConfig.host ?? "localhost",
      port: this.clientConfig.port ?? 5432,
      database: this.clientConfig.database ?? "postgres",
      user: this.clientConfig.user ?? "postgres",
      password: this.clientConfig.password,
    });

    try {
      await this.pgClient.connect();
      this.connected = true;
    } catch (err) {
      this.pgClient = null;
      throw new WarpGridDatabaseError("Failed to connect to database", {
        cause: err,
      });
    }
  }

  async query(sql: string, params?: unknown[]): Promise<QueryResult> {
    if (!this.connected || !this.pgClient) {
      throw new WarpGridDatabaseError("Not connected");
    }

    try {
      const result = await this.pgClient.query(sql, params);
      const fields: FieldInfo[] = (result.fields ?? []).map((f) => ({
        name: f.name,
        dataTypeID: f.dataTypeID,
      }));
      return {
        rows: result.rows as Record<string, unknown>[],
        rowCount: result.rowCount ?? result.rows.length,
        fields,
      };
    } catch (err: unknown) {
      // pg package errors carry .code, .severity, .detail — surface as PostgresError
      const pgErr = err as Record<string, unknown>;
      if (
        typeof pgErr.code === "string" &&
        typeof pgErr.severity === "string"
      ) {
        throw new PostgresError({
          severity: pgErr.severity as string,
          code: pgErr.code as string,
          message:
            typeof pgErr.message === "string"
              ? pgErr.message
              : String(err),
          detail: typeof pgErr.detail === "string" ? pgErr.detail : undefined,
        });
      }
      throw new WarpGridDatabaseError(
        `Query failed: ${err instanceof Error ? err.message : String(err)}`,
        { cause: err },
      );
    }
  }

  async end(): Promise<void> {
    if (this.ended) return;
    this.ended = true;
    this.connected = false;

    if (this.pgClient) {
      try {
        await this.pgClient.end();
      } catch (err) {
        throw new WarpGridDatabaseError("Failed to close connection", {
          cause: err,
        });
      } finally {
        this.pgClient = null;
      }
    }
  }
}
