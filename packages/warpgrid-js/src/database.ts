/**
 * WarpGrid database connection module.
 *
 * Wraps the low-level `warpgrid:shim/database-proxy` WIT bindings
 * in a developer-friendly API. The WIT bindings are injected via
 * constructor for testability — in production, they come from the
 * ComponentizeJS WIT import; in tests, they're mocked.
 */

import { WarpGridError } from "./errors.js";
import type {
  ConnectConfig,
  Connection,
  DatabaseProxyBindings,
} from "./types.js";

const MIN_PORT = 1;
const MAX_PORT = 65535;

function validateConfig(config: ConnectConfig): void {
  if (!config.host) {
    throw new WarpGridError(
      "Invalid connect config: 'host' is required and must be non-empty",
    );
  }
  if (
    !Number.isInteger(config.port) ||
    config.port < MIN_PORT ||
    config.port > MAX_PORT
  ) {
    throw new WarpGridError(
      `Invalid connect config: 'port' must be an integer between ${MIN_PORT} and ${MAX_PORT}, got ${config.port}`,
    );
  }
  if (!config.database) {
    throw new WarpGridError(
      "Invalid connect config: 'database' is required and must be non-empty",
    );
  }
  if (!config.username) {
    throw new WarpGridError(
      "Invalid connect config: 'username' is required and must be non-empty",
    );
  }
}

function wrapWitError(operation: string, err: unknown): never {
  const message =
    typeof err === "string"
      ? err
      : err instanceof Error
        ? err.message
        : String(err);

  throw new WarpGridError(`database ${operation} failed: ${message}`, {
    cause: err,
  });
}

/**
 * WarpGrid database module providing `connect()` for establishing
 * proxied database connections through the host's connection pool.
 */
export class WarpGridDatabase {
  private readonly bindings: DatabaseProxyBindings;

  constructor(bindings: DatabaseProxyBindings) {
    this.bindings = bindings;
  }

  /**
   * Establish a proxied database connection.
   *
   * Under the hood, calls `warpgrid:shim/database-proxy.connect()` to
   * obtain a connection from the host's pool. The returned connection
   * object provides `send()` and `recv()` for raw wire protocol I/O.
   *
   * @throws {WarpGridError} if config is invalid or the connection fails
   */
  connect(config: ConnectConfig): Connection {
    validateConfig(config);

    let handle: bigint;
    try {
      handle = this.bindings.connect({
        host: config.host,
        port: config.port,
        database: config.database,
        user: config.username,
        password: config.password,
      });
    } catch (err) {
      wrapWitError("connect", err);
    }

    const bindings = this.bindings;
    let closed = false;

    function assertOpen(): void {
      if (closed) {
        throw new WarpGridError("connection is closed");
      }
    }

    return {
      send(data: Uint8Array): void {
        assertOpen();
        try {
          bindings.send(handle, data);
        } catch (err) {
          wrapWitError("send", err);
        }
      },

      recv(maxBytes: number): Uint8Array {
        assertOpen();
        try {
          return bindings.recv(handle, maxBytes);
        } catch (err) {
          wrapWitError("recv", err);
        }
      },

      close(): void {
        assertOpen();
        closed = true;
        try {
          bindings.close(handle);
        } catch (err) {
          wrapWitError("close", err);
        }
      },
    };
  }
}
