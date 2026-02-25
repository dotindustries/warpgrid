/**
 * TypeScript types for the WarpGrid JS SDK.
 *
 * These types mirror the WIT interface definitions in
 * `crates/warpgrid-host/wit/database-proxy.wit` but are
 * expressed as idiomatic TypeScript for the JS developer experience.
 */

/**
 * Low-level WIT binding functions for `warpgrid:shim/database-proxy`.
 *
 * Matches the jco type mapping of the WIT interface:
 * - `u64` → `bigint` (connection handles)
 * - `u32` / `u16` → `number`
 * - `list<u8>` → `Uint8Array`
 * - `option<string>` → `string | undefined`
 * - `result<T, string>` → returns T, throws string on error
 */
export interface DatabaseProxyBindings {
  connect(config: WitConnectConfig): bigint;
  send(handle: bigint, data: Uint8Array): number;
  recv(handle: bigint, maxBytes: number): Uint8Array;
  close(handle: bigint): void;
}

/**
 * WIT-level connect config — matches the `connect-config` record.
 * Uses `user` (WIT field name), not `username` (JS API name).
 */
export interface WitConnectConfig {
  host: string;
  port: number;
  database: string;
  user: string;
  password: string | undefined;
}

/**
 * User-facing connect config for `warpgrid.database.connect()`.
 * Uses `username` for JS convention; mapped to `user` for WIT.
 */
export interface ConnectConfig {
  host: string;
  port: number;
  database: string;
  username: string;
  password?: string;
}

/**
 * A proxied database connection returned by `warpgrid.database.connect()`.
 *
 * Wraps an opaque WIT connection handle and provides typed
 * send/recv operations for raw wire protocol bytes.
 */
export interface Connection {
  /** Send raw protocol bytes to the database. */
  send(data: Uint8Array): void;
  /** Receive up to `maxBytes` of raw protocol bytes from the database. */
  recv(maxBytes: number): Uint8Array;
  /** Close the connection, returning it to the host pool if healthy. */
  close(): void;
}
