/**
 * TypeScript type declarations for the WarpGrid database proxy WIT interface.
 *
 * These types match the WIT definition at wit/deps/warpgrid/database-proxy.wit.
 * When componentized by jco, these imports resolve to the WIT host functions.
 * For testing, use the Transport interface in pg-client.ts with mock implementations.
 */
declare module "warpgrid:shim/database-proxy@0.1.0" {
  export interface ConnectConfig {
    host: string;
    port: number;
    database: string;
    user: string;
    password?: string;
  }

  /** Establish a proxied database connection. Returns an opaque connection handle. */
  export function connect(config: ConnectConfig): bigint;

  /** Send raw protocol bytes over a proxied connection. Returns bytes sent. */
  export function send(handle: bigint, data: Uint8Array): number;

  /** Receive up to maxBytes of raw protocol bytes from a proxied connection. */
  export function recv(handle: bigint, maxBytes: number): Uint8Array;

  /** Close a proxied connection, returning it to the host pool. */
  export function close(handle: bigint): void;
}
