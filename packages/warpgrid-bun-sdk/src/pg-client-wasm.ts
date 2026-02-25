/**
 * WasmPgClient — Single-connection Postgres client for WASI/Wasm mode.
 *
 * Uses the `warpgrid:shim/database-proxy@0.1.0` WIT interface to
 * communicate with the host-side connection pool manager via raw
 * Postgres wire protocol bytes.
 *
 * Unlike WasmPool (which manages multiple pooled connections), this
 * provides a single-connection pg.Client-compatible interface with
 * explicit connect()/end() lifecycle management.
 */

import type {
  QueryResult,
  FieldInfo,
  DatabaseProxyShim,
} from "./postgres.ts";
import { WarpGridDatabaseError, PostgresError } from "./errors.ts";
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
import type { PgClientConfig } from "./pg.ts";

/** Maximum bytes to read per recv call. */
const MAX_RECV_BYTES = 65536;

/** Maximum recv attempts before giving up. */
const MAX_RECV_ATTEMPTS = 100;

/** Resolved configuration with defaults applied. */
interface ResolvedConfig {
  readonly host: string;
  readonly port: number;
  readonly database: string;
  readonly user: string;
  readonly password?: string;
}

export class WasmPgClient {
  private readonly config: ResolvedConfig;
  private readonly shim: DatabaseProxyShim;
  private handle: number | null = null;
  private connected = false;
  private ended = false;

  constructor(config: PgClientConfig, shim: DatabaseProxyShim) {
    this.config = {
      host: config.host ?? "localhost",
      port: config.port ?? 5432,
      database: config.database ?? "postgres",
      user: config.user ?? "postgres",
      password: config.password,
    };
    this.shim = shim;
  }

  async connect(): Promise<void> {
    if (this.connected) {
      throw new WarpGridDatabaseError("Already connected");
    }
    if (this.ended) {
      throw new WarpGridDatabaseError("Client has ended");
    }

    try {
      this.handle = this.shim.connect({
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

    try {
      await this.performStartupHandshake();
      this.connected = true;
    } catch (err) {
      // Clean up handle on handshake failure
      if (this.handle !== null) {
        try {
          this.shim.close(this.handle);
        } catch {
          // Ignore close errors during cleanup
        }
        this.handle = null;
      }
      if (err instanceof WarpGridDatabaseError) throw err;
      throw new WarpGridDatabaseError("Startup handshake failed", {
        cause: err,
      });
    }
  }

  async query(sql: string, params?: unknown[]): Promise<QueryResult> {
    if (!this.connected || this.ended) {
      throw new WarpGridDatabaseError("Not connected");
    }

    const msg =
      params && params.length > 0
        ? buildExtendedQuery(sql, params)
        : buildSimpleQuery(sql);

    this.shimSend(msg);

    const data = this.recvUntilReady();
    return this.parseQueryResponse(data);
  }

  async end(): Promise<void> {
    if (this.ended) return;
    this.ended = true;
    this.connected = false;

    if (this.handle !== null) {
      try {
        this.shim.send(this.handle, buildTerminateMessage());
      } catch {
        // Ignore send errors during shutdown
      }
      try {
        this.shim.close(this.handle);
      } catch {
        // Ignore close errors during shutdown
      }
      this.handle = null;
    }
  }

  // ── Startup Handshake ──────────────────────────────────────────

  private async performStartupHandshake(): Promise<void> {
    const startup = buildStartupMessage(
      this.config.user,
      this.config.database,
    );
    this.shimSend(startup);

    // Initial recv is lenient (strict=false): during password auth the
    // server sends AuthCleartext/AuthMD5 without ReadyForQuery — it's
    // waiting for the client to send credentials first.
    const data = this.recvUntilReady(false);
    const messages = parseMessages(data);

    for (const msg of messages) {
      if (msg.type === MSG.AUTH) {
        const needsSecondRead = await this.handleAuth(msg.payload);
        if (needsSecondRead) {
          // Server sends AuthOk + ParameterStatus + BackendKeyData +
          // ReadyForQuery after validating credentials — strict mode.
          const postAuthData = this.recvUntilReady();
          const postAuthMessages = parseMessages(postAuthData);
          for (const authMsg of postAuthMessages) {
            if (authMsg.type === MSG.ERROR_RESPONSE) {
              const pgErr = parseErrorResponse(authMsg.payload);
              throw new WarpGridDatabaseError(
                `Authentication failed: ${pgErr.message}`,
                {
                  cause: new Error(
                    `SQLSTATE ${pgErr.code}: ${pgErr.message}`,
                  ),
                },
              );
            }
          }
        }
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

  /**
   * Handle an authentication challenge from the server.
   * Returns true if credentials were sent and a second recv is needed
   * to read the server's AuthOk/Error response.
   */
  private async handleAuth(payload: Uint8Array): Promise<boolean> {
    const auth = parseAuthResponse(payload);

    if (auth.type === AUTH_OK) {
      return false;
    }

    if (auth.type === AUTH_CLEARTEXT) {
      if (!this.config.password) {
        throw new WarpGridDatabaseError(
          "Server requires password but none provided",
        );
      }
      this.shimSend(buildCleartextPasswordMessage(this.config.password));
      return true;
    }

    if (auth.type === AUTH_MD5) {
      if (!this.config.password || !auth.salt) {
        throw new WarpGridDatabaseError(
          "Server requires MD5 password but password or salt missing",
        );
      }
      this.shimSend(
        await buildMD5PasswordMessage(
          this.config.user,
          this.config.password,
          auth.salt,
        ),
      );
      return true;
    }

    throw new WarpGridDatabaseError(
      `Unsupported authentication method: ${auth.type}`,
    );
  }

  // ── Query Response Parsing ─────────────────────────────────────

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
          throw new PostgresError({
            severity: pgErr.severity,
            code: pgErr.code,
            message: pgErr.message,
            detail: pgErr.detail,
          });
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

  // ── Shim I/O Helpers ──────────────────────────────────────────

  private shimSend(data: Uint8Array): void {
    try {
      this.shim.send(this.handle!, data);
    } catch (err) {
      throw new WarpGridDatabaseError("Failed to send data to database", {
        cause: err,
      });
    }
  }

  /**
   * Receive data from the shim until we see a ReadyForQuery ('Z') message.
   * Concatenates all received chunks into a single buffer.
   *
   * Only empty recv calls count toward the attempt limit — successful
   * reads reset the counter so large multi-chunk responses are not truncated.
   *
   * @param strict  When true (default), throws if data was received but no
   *                ReadyForQuery was found. Set to false for the initial
   *                auth probe where the server may send an auth challenge
   *                without ReadyForQuery.
   */
  private recvUntilReady(strict = true): Uint8Array {
    let buffer = new Uint8Array(0);
    let emptyAttempts = 0;

    while (emptyAttempts < MAX_RECV_ATTEMPTS) {
      let chunk: Uint8Array;
      try {
        chunk = this.shim.recv(this.handle!, MAX_RECV_BYTES);
      } catch (err) {
        throw new WarpGridDatabaseError(
          "Failed to receive data from database",
          { cause: err },
        );
      }

      if (chunk.length === 0) {
        if (buffer.length > 0) break;
        emptyAttempts++;
        continue;
      }

      // Reset empty counter on successful read
      emptyAttempts = 0;

      const combined = new Uint8Array(buffer.length + chunk.length);
      combined.set(buffer);
      combined.set(chunk, buffer.length);
      buffer = combined;

      if (containsReadyForQuery(buffer)) break;
    }

    // Fail loudly if we never received ReadyForQuery (strict mode only)
    if (strict && buffer.length > 0 && !containsReadyForQuery(buffer)) {
      throw new WarpGridDatabaseError(
        "Timed out waiting for server response (no ReadyForQuery received)",
      );
    }

    return buffer;
  }
}

/**
 * Check if the buffer contains a complete ReadyForQuery message.
 * ReadyForQuery = 'Z' (0x5A) + Int32(5) + Byte(status)
 */
function containsReadyForQuery(data: Uint8Array): boolean {
  for (let i = data.length - 6; i >= 0; i--) {
    if (data[i] === 0x5a) {
      const view = new DataView(data.buffer, data.byteOffset + i + 1, 4);
      if (view.getInt32(0) === 5) return true;
    }
  }
  return false;
}
