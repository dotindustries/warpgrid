/**
 * WarpGrid global object setup for ComponentizeJS.
 *
 * Exports `setupWarpGridGlobal()` which takes WIT database-proxy bindings
 * and installs `globalThis.warpgrid` with the database connection API.
 *
 * This is called by the `warp pack --lang js` pipeline, which:
 * 1. Imports the WIT bindings from `warpgrid:shim/database-proxy`
 * 2. Calls `setupWarpGridGlobal({ connect, send, recv, close })`
 * 3. Bundles the result into the componentized handler
 *
 * @example Injected by warp pack:
 * ```js
 * import { connect, send, recv, close } from 'warpgrid:shim/database-proxy';
 * import { setupWarpGridGlobal } from '@warpgrid/js/global';
 * setupWarpGridGlobal({ connect, send, recv, close });
 * ```
 *
 * @example Handler code:
 * ```js
 * const conn = warpgrid.database.connect({
 *   host: "db.internal", port: 5432,
 *   database: "mydb", username: "app",
 * });
 * conn.send(queryBytes);
 * const result = conn.recv(4096);
 * conn.close();
 * ```
 */
import { WarpGridDatabase } from "./database.js";
import type { DatabaseProxyBindings } from "./types.js";
declare global {
    var warpgrid: {
        database: {
            connect: WarpGridDatabase["connect"];
        };
    };
}
/**
 * Initialize the `warpgrid` global object with WIT database-proxy bindings.
 *
 * Must be called before any handler code accesses `warpgrid.database`.
 * Typically wired by the `warp pack` build pipeline.
 */
export declare function setupWarpGridGlobal(bindings: DatabaseProxyBindings): void;
//# sourceMappingURL=global.d.ts.map