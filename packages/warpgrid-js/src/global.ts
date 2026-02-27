/**
 * WarpGrid global object setup for ComponentizeJS.
 *
 * Exports `setupWarpGridGlobal()` which takes WIT bindings for
 * database-proxy, DNS, and filesystem, and installs them on
 * `globalThis.warpgrid` with typed APIs.
 *
 * This is called by the `warp pack --lang js` pipeline, which:
 * 1. Imports the WIT bindings from each shim interface
 * 2. Calls `setupWarpGridGlobal({ database, dns, fs })`
 * 3. Bundles the result into the componentized handler
 *
 * @example Injected by warp pack:
 * ```js
 * import { connect, send, recv, close } from 'warpgrid:shim/database-proxy';
 * import { resolveAddress } from 'warpgrid:shim/dns';
 * import { openVirtual, readVirtual, closeVirtual } from 'warpgrid:shim/filesystem';
 * import { setupWarpGridGlobal } from '@warpgrid/js/global';
 * setupWarpGridGlobal({
 *   database: { connect, send, recv, close },
 *   dns: { resolveAddress },
 *   fs: { openVirtual, readVirtual, closeVirtual },
 * });
 * ```
 *
 * @example Handler code:
 * ```js
 * const conn = warpgrid.database.connect({ host: "db.internal", ... });
 * const addrs = warpgrid.dns.resolve("service.local");
 * const data = warpgrid.fs.readFile("/etc/resolv.conf", "utf-8");
 * ```
 */

import { WarpGridDatabase } from "./database.js";
import { WarpGridDns } from "./dns.js";
import { WarpGridFs } from "./fs.js";
import { WarpGridError } from "./errors.js";
import type { DatabaseProxyBindings, DnsBindings, FilesystemBindings } from "./types.js";

/**
 * Shape of the `globalThis.warpgrid` object.
 * Each shim property is optional — only initialized shims are present.
 */
interface WarpGridGlobal {
  database?: {
    connect: WarpGridDatabase["connect"];
  };
  dns?: {
    resolve: WarpGridDns["resolve"];
  };
  fs?: {
    readFile: WarpGridFs["readFile"];
  };
}

/**
 * Configuration for `setupWarpGridGlobal()`.
 *
 * Each binding set is optional — only provided shims are installed.
 */
export interface WarpGridGlobalConfig {
  database?: DatabaseProxyBindings;
  dns?: DnsBindings;
  fs?: FilesystemBindings;
}

// Declare the warpgrid global type
declare global {
  // eslint-disable-next-line no-var
  var warpgrid: WarpGridGlobal;
}

/**
 * Initialize the `warpgrid` global object with WIT bindings.
 *
 * Must be called before any handler code accesses `warpgrid.*`.
 * Typically wired by the `warp pack` build pipeline.
 *
 * Accepts either:
 * - A `WarpGridGlobalConfig` object with optional `database`, `dns`, and `fs` bindings
 * - A `DatabaseProxyBindings` object (legacy form, sets up only database)
 *
 * Calling this function twice throws a `WarpGridError`.
 */
export function setupWarpGridGlobal(
  configOrBindings: WarpGridGlobalConfig | DatabaseProxyBindings,
): void {
  if (globalThis.warpgrid !== undefined) {
    throw new WarpGridError(
      "warpgrid global is already initialized; setupWarpGridGlobal() must only be called once",
    );
  }

  // Detect legacy (DatabaseProxyBindings) vs new (WarpGridGlobalConfig) form
  const config = isLegacyBindings(configOrBindings)
    ? { database: configOrBindings }
    : configOrBindings;

  const global: WarpGridGlobal = {};

  if (config.database) {
    const db = new WarpGridDatabase(config.database);
    global.database = {
      connect: db.connect.bind(db),
    };
  }

  if (config.dns) {
    const dns = new WarpGridDns(config.dns);
    global.dns = {
      resolve: dns.resolve.bind(dns),
    };
  }

  if (config.fs) {
    const fs = new WarpGridFs(config.fs);
    global.fs = {
      readFile: fs.readFile.bind(fs),
    };
  }

  globalThis.warpgrid = global;
}

function isLegacyBindings(
  val: WarpGridGlobalConfig | DatabaseProxyBindings,
): val is DatabaseProxyBindings {
  return "connect" in val && "send" in val && "recv" in val && "close" in val;
}
