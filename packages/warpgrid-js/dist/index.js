/**
 * @warpgrid/js — WarpGrid JavaScript SDK for ComponentizeJS runtime.
 *
 * Provides the `warpgrid` global object with database connectivity
 * through the host's WIT shim interfaces.
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
 * ```
 */
export { WarpGridDatabase } from "./database.js";
export { WarpGridError } from "./errors.js";
//# sourceMappingURL=index.js.map