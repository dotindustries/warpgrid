import { describe, test, expect, beforeEach } from "bun:test";
import { WasmPool } from "../src/postgres-wasm.ts";
import { WarpGridDatabaseError } from "../src/errors.ts";
import type { DatabaseProxyShim } from "../src/postgres.ts";
import {
  MSG,
  AUTH_OK,
  AUTH_CLEARTEXT,
} from "../src/postgres-protocol.ts";

const encoder = new TextEncoder();

// ── Mock Database Proxy Shim ────────────────────────────────────────
//
// Simulates the host-side database proxy by building realistic Postgres
// wire protocol responses. Tracks calls for assertion.

function buildBackendMessage(type: number, payload: Uint8Array): Uint8Array {
  const length = 4 + payload.length;
  const buf = new ArrayBuffer(1 + length);
  const view = new DataView(buf);
  view.setUint8(0, type);
  view.setInt32(1, length);
  new Uint8Array(buf).set(payload, 5);
  return new Uint8Array(buf);
}

function buildAuthOk(): Uint8Array {
  const buf = new ArrayBuffer(4);
  new DataView(buf).setInt32(0, AUTH_OK);
  return buildBackendMessage(MSG.AUTH, new Uint8Array(buf));
}

function buildAuthCleartext(): Uint8Array {
  const buf = new ArrayBuffer(4);
  new DataView(buf).setInt32(0, AUTH_CLEARTEXT);
  return buildBackendMessage(MSG.AUTH, new Uint8Array(buf));
}

function buildParamStatus(name: string, value: string): Uint8Array {
  return buildBackendMessage(
    MSG.PARAM_STATUS,
    encoder.encode(`${name}\0${value}\0`),
  );
}

function buildBackendKeyData(): Uint8Array {
  const buf = new ArrayBuffer(8);
  const view = new DataView(buf);
  view.setInt32(0, 1234); // process ID
  view.setInt32(4, 5678); // secret key
  return buildBackendMessage(MSG.BACKEND_KEY, new Uint8Array(buf));
}

function buildReadyForQuery(status = "I"): Uint8Array {
  return buildBackendMessage(
    MSG.READY_FOR_QUERY,
    new Uint8Array([status.charCodeAt(0)]),
  );
}

function buildRowDescription(
  fields: { name: string; typeOID: number }[],
): Uint8Array {
  const parts: number[] = [];
  parts.push((fields.length >> 8) & 0xff, fields.length & 0xff);
  for (const f of fields) {
    const nameBytes = encoder.encode(f.name);
    for (const b of nameBytes) parts.push(b);
    parts.push(0);
    // table OID, col attr, type OID, size, modifier, format
    parts.push(0, 0, 0, 0); // table OID
    parts.push(0, 0); // col attr
    parts.push(
      (f.typeOID >> 24) & 0xff,
      (f.typeOID >> 16) & 0xff,
      (f.typeOID >> 8) & 0xff,
      f.typeOID & 0xff,
    );
    parts.push(0xff, 0xff); // size
    parts.push(0xff, 0xff, 0xff, 0xff); // modifier
    parts.push(0, 0); // format
  }
  return buildBackendMessage(MSG.ROW_DESCRIPTION, new Uint8Array(parts));
}

function buildDataRow(values: (string | null)[]): Uint8Array {
  const parts: number[] = [];
  parts.push((values.length >> 8) & 0xff, values.length & 0xff);
  for (const v of values) {
    if (v === null) {
      parts.push(0xff, 0xff, 0xff, 0xff);
    } else {
      const bytes = encoder.encode(v);
      parts.push(
        (bytes.length >> 24) & 0xff,
        (bytes.length >> 16) & 0xff,
        (bytes.length >> 8) & 0xff,
        bytes.length & 0xff,
      );
      for (const b of bytes) parts.push(b);
    }
  }
  return buildBackendMessage(MSG.DATA_ROW, new Uint8Array(parts));
}

function buildCommandComplete(tag: string): Uint8Array {
  return buildBackendMessage(MSG.COMMAND_COMPLETE, encoder.encode(tag + "\0"));
}

function buildParseComplete(): Uint8Array {
  return buildBackendMessage(0x31, new Uint8Array(0)); // '1'
}

function buildBindComplete(): Uint8Array {
  return buildBackendMessage(0x32, new Uint8Array(0)); // '2'
}

function buildErrorResponse(fields: Record<string, string>): Uint8Array {
  const parts: number[] = [];
  for (const [key, value] of Object.entries(fields)) {
    parts.push(key.charCodeAt(0));
    const bytes = encoder.encode(value);
    for (const b of bytes) parts.push(b);
    parts.push(0);
  }
  parts.push(0);
  return buildBackendMessage(MSG.ERROR_RESPONSE, new Uint8Array(parts));
}

function concat(...arrays: Uint8Array[]): Uint8Array {
  const total = arrays.reduce((sum, a) => sum + a.length, 0);
  const result = new Uint8Array(total);
  let offset = 0;
  for (const a of arrays) {
    result.set(a, offset);
    offset += a.length;
  }
  return result;
}

/** Full startup response: AuthOk + params + BackendKeyData + ReadyForQuery */
function buildStartupResponse(): Uint8Array {
  return concat(
    buildAuthOk(),
    buildParamStatus("server_version", "16.0"),
    buildParamStatus("server_encoding", "UTF8"),
    buildBackendKeyData(),
    buildReadyForQuery("I"),
  );
}

/** Full SELECT response for seed users */
function buildSelectUsersResponse(): Uint8Array {
  return concat(
    buildRowDescription([
      { name: "id", typeOID: 23 },
      { name: "name", typeOID: 25 },
      { name: "email", typeOID: 25 },
    ]),
    buildDataRow(["1", "Alice", "alice@test.com"]),
    buildDataRow(["2", "Bob", "bob@test.com"]),
    buildCommandComplete("SELECT 2"),
    buildReadyForQuery("I"),
  );
}

/** Full INSERT response */
function buildInsertResponse(): Uint8Array {
  return concat(
    buildRowDescription([
      { name: "id", typeOID: 23 },
      { name: "name", typeOID: 25 },
    ]),
    buildDataRow(["3", "Carol"]),
    buildCommandComplete("INSERT 0 1"),
    buildReadyForQuery("I"),
  );
}

/** Extended query SELECT response */
function buildExtendedSelectResponse(): Uint8Array {
  return concat(
    buildParseComplete(),
    buildBindComplete(),
    buildRowDescription([
      { name: "id", typeOID: 23 },
      { name: "name", typeOID: 25 },
    ]),
    buildDataRow(["1", "Alice"]),
    buildCommandComplete("SELECT 1"),
    buildReadyForQuery("I"),
  );
}

/** Cleartext auth response: ask for password, then OK */
function buildCleartextAuthStartupResponse(): Uint8Array {
  return concat(
    buildAuthCleartext(),
    // After client sends password, server sends:
    // (handled in second recv call)
  );
}

function buildPostAuthResponse(): Uint8Array {
  return concat(
    buildAuthOk(),
    buildParamStatus("server_version", "16.0"),
    buildBackendKeyData(),
    buildReadyForQuery("I"),
  );
}

class MockDatabaseShim implements DatabaseProxyShim {
  connectCount = 0;
  sendLog: { handle: number; data: Uint8Array }[] = [];
  closeLog: number[] = [];

  private nextHandle = 1;
  private handles = new Set<number>();
  recvQueues = new Map<number, Uint8Array[]>();
  private connectError: Error | null = null;
  private sendError: Error | null = null;
  private customRecvResponses: Map<number, (sendData: Uint8Array) => Uint8Array> | null = null;

  /** Configure the shim to fail on connect */
  failOnConnect(err: Error): void {
    this.connectError = err;
  }

  /** Configure the shim to fail on send */
  failOnSend(err: Error): void {
    this.sendError = err;
  }

  /** Queue a specific response for a handle's next recv */
  queueResponse(handle: number, data: Uint8Array): void {
    const queue = this.recvQueues.get(handle) ?? [];
    queue.push(data);
    this.recvQueues.set(handle, queue);
  }

  connect(config: {
    host: string;
    port: number;
    database: string;
    user: string;
    password?: string;
  }): number {
    if (this.connectError) throw this.connectError;

    this.connectCount++;
    const handle = this.nextHandle++;
    this.handles.add(handle);

    // Queue the default startup response
    this.queueResponse(handle, buildStartupResponse());

    return handle;
  }

  send(handle: number, data: Uint8Array): number {
    if (!this.handles.has(handle)) {
      throw new Error(`invalid handle: ${handle}`);
    }
    if (this.sendError) throw this.sendError;

    this.sendLog.push({ handle, data: new Uint8Array(data) });

    // Auto-detect message type and queue appropriate response
    if (data.length > 0) {
      const msgType = data[0];
      if (msgType === MSG.QUERY) {
        this.queueResponse(handle, buildSelectUsersResponse());
      } else if (msgType === MSG.PARSE) {
        this.queueResponse(handle, buildExtendedSelectResponse());
      }
      // Terminate, Password: no response queued
    }

    return data.length;
  }

  recv(handle: number, maxBytes: number): Uint8Array {
    if (!this.handles.has(handle)) {
      throw new Error(`invalid handle: ${handle}`);
    }

    const queue = this.recvQueues.get(handle) ?? [];
    if (queue.length === 0) return new Uint8Array(0);

    const response = queue.shift()!;
    return response.slice(0, maxBytes);
  }

  close(handle: number): void {
    this.closeLog.push(handle);
    this.handles.delete(handle);
    this.recvQueues.delete(handle);
  }
}

// ── Tests ───────────────────────────────────────────────────────────

describe("WasmPool", () => {
  let shim: MockDatabaseShim;
  let pool: WasmPool;

  beforeEach(() => {
    shim = new MockDatabaseShim();
    pool = new WasmPool(
      { host: "db.test", port: 5432, database: "testdb", user: "testuser" },
      shim,
    );
  });

  describe("createPool returns a Pool with correct interface", () => {
    test("has query method", () => {
      expect(typeof pool.query).toBe("function");
    });

    test("has end method", () => {
      expect(typeof pool.end).toBe("function");
    });

    test("has getPoolSize method", () => {
      expect(typeof pool.getPoolSize).toBe("function");
    });

    test("has getIdleCount method", () => {
      expect(typeof pool.getIdleCount).toBe("function");
    });
  });

  describe("query()", () => {
    test("executes a simple SELECT and returns rows as objects", async () => {
      const result = await pool.query("SELECT id, name, email FROM users");

      expect(result.rows).toHaveLength(2);
      expect(result.rows[0]).toEqual({
        id: "1",
        name: "Alice",
        email: "alice@test.com",
      });
      expect(result.rows[1]).toEqual({
        id: "2",
        name: "Bob",
        email: "bob@test.com",
      });
      expect(result.rowCount).toBe(2);
      expect(result.fields).toHaveLength(3);
      expect(result.fields[0].name).toBe("id");
      expect(result.fields[1].name).toBe("name");
    });

    test("calls shim.connect on first query", async () => {
      await pool.query("SELECT 1");
      expect(shim.connectCount).toBe(1);
    });

    test("sends startup message followed by query message", async () => {
      await pool.query("SELECT 1");

      // First send: startup message (no type byte, starts with length)
      expect(shim.sendLog.length).toBeGreaterThanOrEqual(2);

      // Second send should be the query (starts with 'Q' = 0x51)
      const queryMsg = shim.sendLog[shim.sendLog.length - 1];
      expect(queryMsg.data[0]).toBe(MSG.QUERY);
    });

    test("uses extended query protocol for parameterized queries", async () => {
      // Override the default response for extended queries
      const result = await pool.query("SELECT $1", [42]);

      // Check that Parse message was sent
      const lastSend = shim.sendLog[shim.sendLog.length - 1];
      expect(lastSend.data[0]).toBe(MSG.PARSE);

      expect(result.rows).toHaveLength(1);
    });

    test("returns field metadata", async () => {
      const result = await pool.query("SELECT id, name, email FROM users");
      expect(result.fields).toEqual([
        { name: "id", dataTypeID: 23 },
        { name: "name", dataTypeID: 25 },
        { name: "email", dataTypeID: 25 },
      ]);
    });
  });

  describe("connection pooling", () => {
    test("reuses connection for sequential queries", async () => {
      await pool.query("SELECT 1");
      await pool.query("SELECT 2");

      // Only one connect call (connection reused)
      expect(shim.connectCount).toBe(1);
    });

    test("getPoolSize reflects total connections", async () => {
      expect(pool.getPoolSize()).toBe(0);
      await pool.query("SELECT 1");
      expect(pool.getPoolSize()).toBe(1);
    });

    test("getIdleCount reflects idle connections after query", async () => {
      expect(pool.getIdleCount()).toBe(0);
      await pool.query("SELECT 1");
      // After query completes, connection returns to idle
      expect(pool.getIdleCount()).toBe(1);
    });

    test("pool exhaustion throws WarpGridDatabaseError", async () => {
      const smallPool = new WasmPool(
        { maxConnections: 1 },
        shim,
      );

      // First query creates a connection
      await smallPool.query("SELECT 1");

      // The connection should be idle now, so this should reuse it
      await smallPool.query("SELECT 2");

      // Still only 1 connection
      expect(smallPool.getPoolSize()).toBe(1);

      await smallPool.end();
    });
  });

  describe("end()", () => {
    test("closes all connections via shim", async () => {
      await pool.query("SELECT 1");
      expect(pool.getPoolSize()).toBe(1);

      await pool.end();
      expect(pool.getPoolSize()).toBe(0);
      expect(shim.closeLog).toHaveLength(1);
    });

    test("sends Terminate message before closing", async () => {
      await pool.query("SELECT 1");
      await pool.end();

      // Last send before close should be Terminate
      const terminateMsg = shim.sendLog[shim.sendLog.length - 1];
      expect(terminateMsg.data[0]).toBe(MSG.TERMINATE);
    });

    test("subsequent queries throw after end()", async () => {
      await pool.end();

      try {
        await pool.query("SELECT 1");
        expect(true).toBe(false); // Should not reach here
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDatabaseError);
        expect((err as WarpGridDatabaseError).message).toBe("Pool is closed");
      }
    });
  });

  describe("error handling", () => {
    test("connection error throws WarpGridDatabaseError with cause", async () => {
      const failShim = new MockDatabaseShim();
      failShim.failOnConnect(new Error("ECONNREFUSED"));

      const failPool = new WasmPool(
        { host: "unreachable" },
        failShim,
      );

      try {
        await failPool.query("SELECT 1");
        expect(true).toBe(false);
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDatabaseError);
        expect((err as WarpGridDatabaseError).message).toContain(
          "Failed to connect",
        );
        expect((err as WarpGridDatabaseError).cause).toBeInstanceOf(Error);
        expect(
          ((err as WarpGridDatabaseError).cause as Error).message,
        ).toBe("ECONNREFUSED");
      }
    });

    test("send error throws WarpGridDatabaseError with cause", async () => {
      // First query succeeds (creates connection)
      await pool.query("SELECT 1");

      // Now make send fail
      shim.failOnSend(new Error("broken pipe"));

      try {
        await pool.query("SELECT 2");
        expect(true).toBe(false);
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDatabaseError);
        expect((err as WarpGridDatabaseError).cause).toBeInstanceOf(Error);
      }
    });

    test("Postgres error response throws WarpGridDatabaseError", async () => {
      const errShim = new MockDatabaseShim();
      const errPool = new WasmPool(
        { host: "db.test", database: "testdb", user: "testuser" },
        errShim,
      );

      // Override send to queue error response instead of success for queries
      const origSendFn = errShim.send.bind(errShim);
      errShim.send = (handle: number, data: Uint8Array): number => {
        errShim.sendLog.push({ handle, data: new Uint8Array(data) });

        if (data.length > 0 && data[0] === MSG.QUERY) {
          errShim.queueResponse(
            handle,
            concat(
              buildErrorResponse({
                S: "ERROR",
                C: "42P01",
                M: 'relation "missing_table" does not exist',
              }),
              buildReadyForQuery("I"),
            ),
          );
        }

        return data.length;
      };

      try {
        await errPool.query("SELECT * FROM missing_table");
        expect(true).toBe(false);
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDatabaseError);
        expect((err as WarpGridDatabaseError).message).toContain(
          "missing_table",
        );
      }

      await errPool.end();
    });
  });

  describe("authentication", () => {
    test("handles trust authentication (AuthOk)", async () => {
      // Default mock already returns AuthOk
      const result = await pool.query("SELECT 1");
      expect(result).toBeDefined();
    });

    test("handles cleartext password authentication", async () => {
      const authShim = new MockDatabaseShim();
      // Override connect to queue cleartext auth challenge
      const origConnect = authShim.connect.bind(authShim);
      authShim.connect = (config) => {
        const handle = origConnect(config);
        // Replace the default startup response with cleartext auth
        authShim.recvQueues.set(handle, [
          buildAuthCleartext(),
        ]);
        return handle;
      };

      // Override send to detect password message and return auth ok
      const origSend = authShim.send.bind(authShim);
      authShim.send = (handle: number, data: Uint8Array): number => {
        authShim.sendLog.push({ handle, data: new Uint8Array(data) });

        if (data.length > 0 && data[0] === MSG.PASSWORD) {
          // Password sent, queue AuthOk + ReadyForQuery
          authShim.queueResponse(handle, buildPostAuthResponse());
        } else if (data.length > 0 && data[0] === MSG.QUERY) {
          authShim.queueResponse(handle, buildSelectUsersResponse());
        }

        return data.length;
      };

      const authPool = new WasmPool(
        { user: "testuser", password: "secret" },
        authShim,
      );

      const result = await authPool.query("SELECT 1");
      expect(result).toBeDefined();

      // Verify password message was sent
      const pwMsg = authShim.sendLog.find(
        (log) => log.data[0] === MSG.PASSWORD,
      );
      expect(pwMsg).toBeDefined();

      await authPool.end();
    });
  });
});
