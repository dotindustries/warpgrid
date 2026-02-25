/**
 * Postgres client for WarpGrid.
 *
 * Implements a pg.Client-compatible interface on top of the WarpGrid database
 * proxy shim. The client speaks raw Postgres wire protocol over the Transport
 * interface, which maps to `warpgrid:shim/database-proxy` WIT imports at runtime.
 *
 * Usage:
 *   const client = new Client(config, transport);
 *   client.connect();
 *   const result = client.query("SELECT * FROM users WHERE id = $1", ["42"]);
 *   client.end();
 */

import {
  encodeStartup,
  encodeSimpleQuery,
  encodeExtendedQuery,
  encodeTerminate,
  encodePasswordMessage,
  parseBackendMessages,
  BackendMessageType,
  type BackendMessage,
  type RowDescriptionMessage,
  type DataRowMessage,
  type CommandCompleteMessage,
  type ErrorResponseMessage,
  type AuthenticationMessage,
} from "./pg-wire.js";

// ── Public Interfaces ────────────────────────────────────────────────────────

/**
 * Transport abstraction over the WIT database-proxy shim.
 *
 * In production (componentized), this maps to:
 *   connect → warpgrid:shim/database-proxy.connect
 *   send    → warpgrid:shim/database-proxy.send
 *   recv    → warpgrid:shim/database-proxy.recv
 *   close   → warpgrid:shim/database-proxy.close
 *
 * For testing, use a mock implementation.
 */
export interface Transport {
  connect(config: { host: string; port: number; database: string; user: string; password?: string }): bigint;
  send(handle: bigint, data: Uint8Array): number;
  recv(handle: bigint, maxBytes: number): Uint8Array;
  close(handle: bigint): void;
}

export interface ClientConfig {
  host: string;
  port: number;
  database: string;
  user: string;
  password?: string;
}

export interface QueryResult {
  rows: Record<string, string | null>[];
  rowCount: number;
  fields: Array<{ name: string; typeOid: number }>;
}

// ── Client Implementation ────────────────────────────────────────────────────

const RECV_BUFFER_SIZE = 65536;

export class Client {
  private handle: bigint | null = null;
  private readonly config: ClientConfig;
  private readonly transport: Transport;
  private recvBuffer: Uint8Array = new Uint8Array(0);

  constructor(config: ClientConfig, transport: Transport) {
    this.config = config;
    this.transport = transport;
  }

  /**
   * Connect to the Postgres server and complete the startup handshake.
   *
   * Sends the startup message, handles authentication (cleartext password),
   * and waits for ReadyForQuery.
   */
  connect(): void {
    // Establish the transport connection
    this.handle = this.transport.connect({
      host: this.config.host,
      port: this.config.port,
      database: this.config.database,
      user: this.config.user,
      password: this.config.password,
    });

    // Send startup message
    const startup = encodeStartup(this.config.user, this.config.database);
    this.transportSend(startup);

    // Read and process startup response
    this.processStartupResponse();
  }

  /**
   * Execute a SQL query and return the results.
   *
   * If params are provided, uses the Extended Query protocol (Parse/Bind/Execute/Sync).
   * Otherwise, uses the Simple Query protocol.
   */
  query(sql: string, params?: Array<string | null>): QueryResult {
    this.ensureConnected();

    if (params !== undefined && params.length > 0) {
      return this.executeExtendedQuery(sql, params);
    }
    return this.executeSimpleQuery(sql);
  }

  /**
   * Close the connection gracefully.
   *
   * Sends a Terminate message and closes the transport.
   * Idempotent — safe to call multiple times.
   */
  end(): void {
    if (this.handle === null) return;

    try {
      this.transportSend(encodeTerminate());
    } catch {
      // Ignore send errors during shutdown
    }

    try {
      this.transport.close(this.handle);
    } catch {
      // Ignore close errors
    }

    this.handle = null;
  }

  // ── Private Methods ──────────────────────────────────────────────────────

  private ensureConnected(): void {
    if (this.handle === null) {
      throw new Error("Client is not connected. Call connect() first.");
    }
  }

  private transportSend(data: Uint8Array): void {
    if (this.handle === null) throw new Error("Not connected");
    this.transport.send(this.handle, data);
  }

  /**
   * Receive messages from the transport and parse them.
   *
   * Reads up to RECV_BUFFER_SIZE bytes and returns parsed backend messages.
   * Accumulates partial messages across calls via the internal recvBuffer.
   */
  private receiveMessages(): BackendMessage[] {
    if (this.handle === null) throw new Error("Not connected");

    const data = this.transport.recv(this.handle, RECV_BUFFER_SIZE);
    if (data.byteLength === 0) return [];

    // Concatenate with any leftover buffer
    let combined: Uint8Array;
    if (this.recvBuffer.byteLength > 0) {
      combined = new Uint8Array(this.recvBuffer.byteLength + data.byteLength);
      combined.set(this.recvBuffer, 0);
      combined.set(data, this.recvBuffer.byteLength);
      this.recvBuffer = new Uint8Array(0);
    } else {
      combined = data;
    }

    return parseBackendMessages(combined);
  }

  /**
   * Read messages until we find one matching the given predicate.
   * Returns all received messages up to and including the match.
   */
  private readUntil(predicate: (msg: BackendMessage) => boolean): BackendMessage[] {
    const collected: BackendMessage[] = [];
    const maxAttempts = 100; // safety limit

    for (let attempt = 0; attempt < maxAttempts; attempt++) {
      const messages = this.receiveMessages();
      for (const msg of messages) {
        collected.push(msg);
        if (predicate(msg)) return collected;
      }
    }

    throw new Error("Timed out waiting for expected backend message");
  }

  private processStartupResponse(): void {
    const messages = this.readUntil(
      (msg) => msg.type === BackendMessageType.ReadyForQuery
    );

    for (const msg of messages) {
      if (msg.type === BackendMessageType.Authentication) {
        const auth = msg as AuthenticationMessage;
        if (auth.authType === 3) {
          // Cleartext password required
          if (!this.config.password) {
            throw new Error("Server requires password but none was provided");
          }
          this.transportSend(encodePasswordMessage(this.config.password));
          // Continue reading — AuthOk + ReadyForQuery will follow
        } else if (auth.authType === 0) {
          // AuthenticationOk — continue
        } else {
          throw new Error(`Unsupported authentication type: ${auth.authType}`);
        }
      } else if (msg.type === BackendMessageType.ErrorResponse) {
        const err = msg as ErrorResponseMessage;
        throw new Error(`Startup error [${err.code}]: ${err.message}`);
      }
      // ParameterStatus, BackendKeyData — ignored
    }
  }

  private executeSimpleQuery(sql: string): QueryResult {
    this.transportSend(encodeSimpleQuery(sql));

    const messages = this.readUntil(
      (msg) => msg.type === BackendMessageType.ReadyForQuery
    );

    return this.buildQueryResult(messages);
  }

  private executeExtendedQuery(sql: string, params: Array<string | null>): QueryResult {
    this.transportSend(encodeExtendedQuery(sql, params));

    const messages = this.readUntil(
      (msg) => msg.type === BackendMessageType.ReadyForQuery
    );

    return this.buildQueryResult(messages);
  }

  /**
   * Build a QueryResult from a sequence of backend messages.
   *
   * Extracts RowDescription, DataRow, CommandComplete, and ErrorResponse.
   * Throws on ErrorResponse with the Postgres error code and message.
   */
  private buildQueryResult(messages: BackendMessage[]): QueryResult {
    let fields: Array<{ name: string; typeOid: number }> = [];
    const rows: Record<string, string | null>[] = [];
    let rowCount = 0;

    for (const msg of messages) {
      switch (msg.type) {
        case BackendMessageType.ErrorResponse: {
          const err = msg as ErrorResponseMessage;
          throw new Error(`Query error [${err.code}]: ${err.message}`);
        }
        case BackendMessageType.RowDescription: {
          const rd = msg as RowDescriptionMessage;
          fields = rd.fields.map((f) => ({ name: f.name, typeOid: f.typeOid }));
          break;
        }
        case BackendMessageType.DataRow: {
          const dr = msg as DataRowMessage;
          const row: Record<string, string | null> = {};
          for (let i = 0; i < dr.values.length && i < fields.length; i++) {
            const val = dr.values[i];
            row[fields[i].name] = val !== null ? new TextDecoder().decode(val) : null;
          }
          rows.push(row);
          break;
        }
        case BackendMessageType.CommandComplete: {
          const cc = msg as CommandCompleteMessage;
          rowCount = parseRowCount(cc.tag, rows.length);
          break;
        }
        // ParseComplete, BindComplete, ReadyForQuery — ignored
      }
    }

    return { rows, rowCount, fields };
  }
}

/**
 * Parse the row count from a CommandComplete tag.
 *
 * Tags look like: "SELECT 5", "INSERT 0 1", "UPDATE 3", "DELETE 2"
 */
function parseRowCount(tag: string, dataRowCount: number): number {
  const parts = tag.split(" ");
  const lastPart = parts[parts.length - 1];
  const parsed = parseInt(lastPart, 10);
  return isNaN(parsed) ? dataRowCount : parsed;
}
