/**
 * WasmPool — Postgres pool for WASI/Wasm mode.
 *
 * Uses the `warpgrid:shim/database-proxy@0.1.0` WIT interface to
 * communicate with the host-side connection pool manager via raw
 * Postgres wire protocol bytes.
 *
 * The host handles TLS, connection pooling, and health checking.
 * This module handles: startup handshake, authentication, query
 * building, and response parsing on the guest side.
 */

import type {
  Pool,
  PoolConfig,
  QueryResult,
  FieldInfo,
  DatabaseProxyShim,
} from "./postgres.ts";
import { WarpGridDatabaseError } from "./errors.ts";
import {
  buildStartupMessage,
  buildCleartextPasswordMessage,
  buildMD5PasswordMessage,
  buildSimpleQuery,
  buildExtendedQuery,
  buildTerminateMessage,
  parseMessages,
  parseAuthResponse,
  parseRowDescription,
  parseDataRow,
  parseErrorResponse,
  parseCommandComplete,
  MSG,
  AUTH_OK,
  AUTH_CLEARTEXT,
  AUTH_MD5,
} from "./postgres-protocol.ts";

/** Maximum bytes to read per recv call. */
const MAX_RECV_BYTES = 65536;

/** Maximum recv attempts before giving up. */
const MAX_RECV_ATTEMPTS = 100;

/** Internal representation of a managed connection. */
interface ManagedConnection {
  handle: number;
  idle: boolean;
}

export class WasmPool implements Pool {
  private readonly config: Required<
    Pick<PoolConfig, "host" | "port" | "database" | "user" | "maxConnections">
  > & { password?: string; idleTimeout?: number };
  private readonly shim: DatabaseProxyShim;
  private readonly connections: ManagedConnection[] = [];
  private closed = false;

  constructor(config: PoolConfig | undefined, shim: DatabaseProxyShim) {
    this.config = {
      host: config?.host ?? "localhost",
      port: config?.port ?? 5432,
      database: config?.database ?? "postgres",
      user: config?.user ?? "postgres",
      password: config?.password,
      maxConnections: config?.maxConnections ?? 10,
      idleTimeout: config?.idleTimeout,
    };
    this.shim = shim;
  }

  async query(sql: string, params?: unknown[]): Promise<QueryResult> {
    if (this.closed) {
      throw new WarpGridDatabaseError("Pool is closed");
    }

    const conn = await this.checkout();
    try {
      return await this.executeQuery(conn, sql, params);
    } catch (err) {
      // On error, remove the connection (it may be in a bad state)
      this.removeConnection(conn);
      throw err;
    } finally {
      // Return to idle if still tracked
      if (this.connections.includes(conn)) {
        conn.idle = true;
      }
    }
  }

  async end(): Promise<void> {
    this.closed = true;
    for (const conn of this.connections) {
      try {
        // Send Terminate message before closing
        this.shim.send(conn.handle, buildTerminateMessage());
      } catch {
        // Ignore send errors during shutdown
      }
      try {
        this.shim.close(conn.handle);
      } catch {
        // Ignore close errors during shutdown
      }
    }
    this.connections.length = 0;
  }

  getPoolSize(): number {
    return this.connections.length;
  }

  getIdleCount(): number {
    return this.connections.filter((c) => c.idle).length;
  }

  // ── Connection Management ─────────────────────────────────────────

  private async checkout(): Promise<ManagedConnection> {
    // Try to find an idle connection
    const idle = this.connections.find((c) => c.idle);
    if (idle) {
      idle.idle = false;
      return idle;
    }

    // Create a new connection if pool not full
    if (this.connections.length < this.config.maxConnections) {
      return this.createConnection();
    }

    // Pool exhausted
    throw new WarpGridDatabaseError(
      `Connection pool exhausted (max: ${this.config.maxConnections})`,
    );
  }

  private async createConnection(): Promise<ManagedConnection> {
    let handle: number;
    try {
      handle = this.shim.connect({
        host: this.config.host,
        port: this.config.port,
        database: this.config.database,
        user: this.config.user,
        password: this.config.password,
      });
    } catch (err) {
      throw new WarpGridDatabaseError("Failed to connect to database", {
        cause: err,
      });
    }

    const conn: ManagedConnection = { handle, idle: false };
    this.connections.push(conn);

    try {
      await this.performStartupHandshake(conn);
    } catch (err) {
      this.removeConnection(conn);
      if (err instanceof WarpGridDatabaseError) throw err;
      throw new WarpGridDatabaseError("Startup handshake failed", {
        cause: err,
      });
    }

    return conn;
  }

  private removeConnection(conn: ManagedConnection): void {
    const idx = this.connections.indexOf(conn);
    if (idx !== -1) {
      this.connections.splice(idx, 1);
      try {
        this.shim.close(conn.handle);
      } catch {
        // Ignore close errors
      }
    }
  }

  // ── Startup Handshake ─────────────────────────────────────────────

  private async performStartupHandshake(
    conn: ManagedConnection,
  ): Promise<void> {
    // Send startup message
    const startup = buildStartupMessage(
      this.config.user,
      this.config.database,
    );
    this.shimSend(conn, startup);

    // Read server response until ReadyForQuery
    const data = this.recvUntilReady(conn);
    const messages = parseMessages(data);

    for (const msg of messages) {
      if (msg.type === MSG.AUTH) {
        await this.handleAuth(conn, msg.payload);
      } else if (msg.type === MSG.ERROR_RESPONSE) {
        const pgErr = parseErrorResponse(msg.payload);
        throw new WarpGridDatabaseError(
          `Authentication failed: ${pgErr.message}`,
          { cause: new Error(`SQLSTATE ${pgErr.code}: ${pgErr.message}`) },
        );
      }
      // Skip ParameterStatus, BackendKeyData, etc.
    }
  }

  private async handleAuth(
    conn: ManagedConnection,
    payload: Uint8Array,
  ): Promise<void> {
    const auth = parseAuthResponse(payload);

    if (auth.type === AUTH_OK) {
      return; // No auth needed
    }

    if (auth.type === AUTH_CLEARTEXT) {
      if (!this.config.password) {
        throw new WarpGridDatabaseError(
          "Server requires password but none provided",
        );
      }
      const msg = buildCleartextPasswordMessage(this.config.password);
      this.shimSend(conn, msg);
      // Server will send AuthOk or ErrorResponse — handled in the message loop
      return;
    }

    if (auth.type === AUTH_MD5) {
      if (!this.config.password || !auth.salt) {
        throw new WarpGridDatabaseError(
          "Server requires MD5 password but password or salt missing",
        );
      }
      const msg = await buildMD5PasswordMessage(
        this.config.user,
        this.config.password,
        auth.salt,
      );
      this.shimSend(conn, msg);
      return;
    }

    throw new WarpGridDatabaseError(
      `Unsupported authentication method: ${auth.type}`,
    );
  }

  // ── Query Execution ───────────────────────────────────────────────

  private async executeQuery(
    conn: ManagedConnection,
    sql: string,
    params?: unknown[],
  ): Promise<QueryResult> {
    const msg =
      params && params.length > 0
        ? buildExtendedQuery(sql, params)
        : buildSimpleQuery(sql);

    this.shimSend(conn, msg);

    const data = this.recvUntilReady(conn);
    return this.parseQueryResponse(data);
  }

  private parseQueryResponse(data: Uint8Array): QueryResult {
    const messages = parseMessages(data);
    let fields: FieldInfo[] = [];
    const rows: Record<string, unknown>[] = [];
    let rowCount = 0;

    for (const msg of messages) {
      switch (msg.type) {
        case MSG.ROW_DESCRIPTION: {
          const desc = parseRowDescription(msg.payload);
          fields = desc.map((f) => ({
            name: f.name,
            dataTypeID: f.dataTypeOID,
          }));
          break;
        }
        case MSG.DATA_ROW: {
          const values = parseDataRow(msg.payload);
          const row: Record<string, unknown> = {};
          for (let i = 0; i < fields.length && i < values.length; i++) {
            row[fields[i].name] = values[i];
          }
          rows.push(row);
          break;
        }
        case MSG.COMMAND_COMPLETE: {
          rowCount = parseCommandComplete(msg.payload);
          break;
        }
        case MSG.ERROR_RESPONSE: {
          const pgErr = parseErrorResponse(msg.payload);
          throw new WarpGridDatabaseError(
            `Query failed: ${pgErr.message}`,
            {
              cause: new Error(
                `SQLSTATE ${pgErr.code}: ${pgErr.message}` +
                  (pgErr.detail ? ` — ${pgErr.detail}` : ""),
              ),
            },
          );
        }
        // ParseComplete, BindComplete, NoData, ReadyForQuery: skip
      }
    }

    // For SELECT, rowCount = rows returned
    if (rows.length > 0 && rowCount === 0) {
      rowCount = rows.length;
    }

    return { rows, rowCount, fields };
  }

  // ── Shim I/O Helpers ──────────────────────────────────────────────

  private shimSend(conn: ManagedConnection, data: Uint8Array): void {
    try {
      this.shim.send(conn.handle, data);
    } catch (err) {
      throw new WarpGridDatabaseError("Failed to send data to database", {
        cause: err,
      });
    }
  }

  /**
   * Receive data from the shim until we see a ReadyForQuery ('Z') message.
   * Concatenates all received chunks into a single buffer.
   */
  private recvUntilReady(conn: ManagedConnection): Uint8Array {
    let buffer = new Uint8Array(0);
    let attempts = 0;

    while (attempts < MAX_RECV_ATTEMPTS) {
      let chunk: Uint8Array;
      try {
        chunk = this.shim.recv(conn.handle, MAX_RECV_BYTES);
      } catch (err) {
        throw new WarpGridDatabaseError(
          "Failed to receive data from database",
          { cause: err },
        );
      }

      if (chunk.length === 0) {
        // Connection closed or no more data
        if (buffer.length > 0) break;
        attempts++;
        continue;
      }

      const combined = new Uint8Array(buffer.length + chunk.length);
      combined.set(buffer);
      combined.set(chunk, buffer.length);
      buffer = combined;

      // Check if we've received a ReadyForQuery message
      if (containsReadyForQuery(buffer)) break;
      attempts++;
    }

    return buffer;
  }
}

/**
 * Check if the buffer contains a complete ReadyForQuery message.
 * ReadyForQuery = 'Z' (0x5A) + Int32(5) + Byte(status)
 */
function containsReadyForQuery(data: Uint8Array): boolean {
  // Scan from the end for efficiency — ReadyForQuery is usually last
  for (let i = data.length - 6; i >= 0; i--) {
    if (data[i] === 0x5a) {
      // Check length field = 5
      const view = new DataView(data.buffer, data.byteOffset + i + 1, 4);
      if (view.getInt32(0) === 5) return true;
    }
  }
  return false;
}
