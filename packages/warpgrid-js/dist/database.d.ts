/**
 * WarpGrid database connection module.
 *
 * Wraps the low-level `warpgrid:shim/database-proxy` WIT bindings
 * in a developer-friendly API. The WIT bindings are injected via
 * constructor for testability — in production, they come from the
 * ComponentizeJS WIT import; in tests, they're mocked.
 */
import type { ConnectConfig, Connection, DatabaseProxyBindings } from "./types.js";
/**
 * WarpGrid database module providing `connect()` for establishing
 * proxied database connections through the host's connection pool.
 */
export declare class WarpGridDatabase {
    private readonly bindings;
    constructor(bindings: DatabaseProxyBindings);
    /**
     * Establish a proxied database connection.
     *
     * Under the hood, calls `warpgrid:shim/database-proxy.connect()` to
     * obtain a connection from the host's pool. The returned connection
     * object provides `send()` and `recv()` for raw wire protocol I/O.
     *
     * @throws {WarpGridError} if config is invalid or the connection fails
     */
    connect(config: ConnectConfig): Connection;
}
//# sourceMappingURL=database.d.ts.map