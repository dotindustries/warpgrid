/**
 * @warpgrid/js — WarpGrid JavaScript SDK for ComponentizeJS runtime.
 *
 * Provides the `warpgrid` global object with database connectivity,
 * DNS resolution, and virtual filesystem access through the host's
 * WIT shim interfaces.
 *
 * @example
 * ```typescript
 * // In a ComponentizeJS handler:
 * const conn = warpgrid.database.connect({
 *   host: "db.internal",
 *   port: 5432,
 *   database: "mydb",
 *   username: "app",
 * });
 * conn.send(queryBytes);
 * const result = conn.recv(4096);
 * conn.close();
 *
 * const addrs = warpgrid.dns.resolve("service.local");
 * const config = warpgrid.fs.readFile("/etc/resolv.conf", "utf-8");
 * ```
 */

export { WarpGridDatabase } from "./database.js";
export { WarpGridDns } from "./dns.js";
export { WarpGridFs } from "./fs.js";
export { WarpGridError, WarpGridDNSError, WarpGridFSError } from "./errors.js";
export type {
  ConnectConfig,
  Connection,
  DatabaseProxyBindings,
  DnsBindings,
  DnsRecord,
  FilesystemBindings,
  WitConnectConfig,
} from "./types.js";
