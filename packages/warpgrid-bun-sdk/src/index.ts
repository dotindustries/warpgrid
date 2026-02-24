/**
 * @warpgrid/bun-sdk — WarpGrid SDK for Bun.
 *
 * Provides dual-mode modules that work identically in native Bun
 * (development) and WASI/Wasm (deployed) environments.
 */

export { WarpGridError, WarpGridDatabaseError, PostgresError } from "./errors.ts";
export {
  createPool,
  detectMode,
  type Pool,
  type PoolConfig,
  type QueryResult,
  type FieldInfo,
  type DatabaseProxyShim,
} from "./postgres.ts";
export { Client, type PgClientConfig } from "./pg.ts";
