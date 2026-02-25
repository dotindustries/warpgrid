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
import { WarpGridError } from "./errors.js";
import type { DatabaseProxyBindings } from "./types.js";

// Declare the warpgrid global type
declare global {
  // eslint-disable-next-line no-var
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
export function setupWarpGridGlobal(bindings: DatabaseProxyBindings): void {
  if (globalThis.warpgrid !== undefined) {
    throw new WarpGridError(
      "warpgrid global is already initialized; setupWarpGridGlobal() must only be called once",
    );
  }

  const db = new WarpGridDatabase(bindings);

  globalThis.warpgrid = {
    database: {
      connect: db.connect.bind(db),
    },
  };
}
