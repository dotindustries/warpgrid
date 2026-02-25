import { describe, test, expect, beforeEach, afterEach } from "bun:test";
import { WarpGridDatabaseError, PostgresError } from "../src/errors.ts";
import type { DatabaseProxyShim } from "../src/postgres.ts";
import {
  MSG,
  AUTH_OK,
  AUTH_CLEARTEXT,
  AUTH_MD5,
} from "../src/postgres-protocol.ts";

const encoder = new TextEncoder();

// ── Mock Wire Protocol Helpers ──────────────────────────────────────
//
// Build realistic Postgres backend messages for testing the Client's
// wire protocol handling without a real database server.

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

function buildAuthMD5(salt: Uint8Array): Uint8Array {
  const buf = new ArrayBuffer(8);
  const view = new DataView(buf);
  view.setInt32(0, AUTH_MD5);
  new Uint8Array(buf).set(salt, 4);
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
  view.setInt32(0, 1234);
  view.setInt32(4, 5678);
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
  return buildBackendMessage(0x31, new Uint8Array(0));
}

function buildBindComplete(): Uint8Array {
  return buildBackendMessage(0x32, new Uint8Array(0));
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

function buildStartupResponse(): Uint8Array {
  return concat(
    buildAuthOk(),
    buildParamStatus("server_version", "16.0"),
    buildParamStatus("server_encoding", "UTF8"),
    buildBackendKeyData(),
    buildReadyForQuery("I"),
  );
}

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

function buildPostAuthResponse(): Uint8Array {
  return concat(
    buildAuthOk(),
    buildParamStatus("server_version", "16.0"),
    buildBackendKeyData(),
    buildReadyForQuery("I"),
  );
}

// ── Mock Database Proxy Shim ────────────────────────────────────────

class MockDatabaseShim implements DatabaseProxyShim {
  connectCount = 0;
  sendLog: { handle: number; data: Uint8Array }[] = [];
  closeLog: number[] = [];

  private nextHandle = 1;
  private handles = new Set<number>();
  recvQueues = new Map<number, Uint8Array[]>();
  private connectError: Error | null = null;
  private sendError: Error | null = null;

  failOnConnect(err: Error): void {
    this.connectError = err;
  }

  failOnSend(err: Error): void {
    this.sendError = err;
  }

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

    if (data.length > 0) {
      const msgType = data[0];
      if (msgType === MSG.QUERY) {
        this.queueResponse(handle, buildSelectUsersResponse());
      } else if (msgType === MSG.PARSE) {
        this.queueResponse(handle, buildExtendedSelectResponse());
      }
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

// Lazy import to validate the module structure
let ClientModule: typeof import("../src/pg.ts");

async function loadClient() {
  ClientModule = await import("../src/pg.ts");
  return ClientModule;
}

describe("Client (pg.Client-compatible)", () => {
  let shim: MockDatabaseShim;

  beforeEach(() => {
    shim = new MockDatabaseShim();
  });

  // ── Module exports ──────────────────────────────────────────────

  describe("module exports", () => {
    test("exports Client class", async () => {
      const mod = await loadClient();
      expect(mod.Client).toBeDefined();
      expect(typeof mod.Client).toBe("function");
    });

    test("exports PgClientConfig type (via instance creation)", async () => {
      const { Client } = await loadClient();
      // Config accepted without error
      const client = new Client({
        host: "localhost",
        port: 5432,
        database: "test",
        user: "testuser",
        mode: "wasm",
        shim,
      });
      expect(client).toBeDefined();
    });
  });

  // ── Constructor ─────────────────────────────────────────────────

  describe("constructor", () => {
    test("accepts standard pg connection config", async () => {
      const { Client } = await loadClient();
      const client = new Client({
        host: "db.test",
        port: 5432,
        database: "mydb",
        user: "myuser",
        password: "secret",
        mode: "wasm",
        shim,
      });
      expect(client).toBeDefined();
    });

    test("uses defaults for missing config fields", async () => {
      const { Client } = await loadClient();
      const client = new Client({ mode: "wasm", shim });
      expect(client).toBeDefined();
    });
  });

  // ── connect() ──────────────────────────────────────────────────

  describe("connect()", () => {
    test("establishes connection via shim.connect() and performs startup handshake", async () => {
      const { Client } = await loadClient();
      const client = new Client({
        host: "db.test",
        port: 5432,
        database: "testdb",
        user: "testuser",
        mode: "wasm",
        shim,
      });

      await client.connect();

      expect(shim.connectCount).toBe(1);
      // Startup message was sent (first send in log)
      expect(shim.sendLog.length).toBeGreaterThanOrEqual(1);
    });

    test("handles trust authentication (AuthOk)", async () => {
      const { Client } = await loadClient();
      const client = new Client({
        user: "trustuser",
        mode: "wasm",
        shim,
      });

      // Default mock returns AuthOk — should connect without error
      await client.connect();
      const result = await client.query("SELECT 1");
      expect(result).toBeDefined();
      await client.end();
    });

    test("handles cleartext password authentication", async () => {
      const { Client } = await loadClient();
      const authShim = new MockDatabaseShim();

      // Override connect to queue cleartext auth challenge
      const origConnect = authShim.connect.bind(authShim);
      authShim.connect = (config) => {
        const handle = origConnect(config);
        // Replace default startup response with cleartext auth
        authShim.recvQueues.set(handle, [buildAuthCleartext()]);
        return handle;
      };

      // Override send to detect password message and return auth ok
      authShim.send = (handle: number, data: Uint8Array): number => {
        if (!authShim.recvQueues.has(handle) && handle > 0) {
          // Handle might have been set up by connect
        }
        authShim.sendLog.push({ handle, data: new Uint8Array(data) });

        if (data.length > 0 && data[0] === MSG.PASSWORD) {
          authShim.queueResponse(handle, buildPostAuthResponse());
        } else if (data.length > 0 && data[0] === MSG.QUERY) {
          authShim.queueResponse(handle, buildSelectUsersResponse());
        }

        return data.length;
      };

      const client = new Client({
        user: "testuser",
        password: "secret",
        mode: "wasm",
        shim: authShim,
      });

      await client.connect();

      // Verify password message was sent
      const pwMsg = authShim.sendLog.find(
        (log) => log.data[0] === MSG.PASSWORD,
      );
      expect(pwMsg).toBeDefined();

      await client.end();
    });

    test("handles MD5 password authentication", async () => {
      const { Client } = await loadClient();
      const md5Shim = new MockDatabaseShim();
      const salt = new Uint8Array([0xde, 0xad, 0xbe, 0xef]);

      const origConnect = md5Shim.connect.bind(md5Shim);
      md5Shim.connect = (config) => {
        const handle = origConnect(config);
        // Replace default with MD5 auth challenge
        md5Shim.recvQueues.set(handle, [buildAuthMD5(salt)]);
        return handle;
      };

      md5Shim.send = (handle: number, data: Uint8Array): number => {
        md5Shim.sendLog.push({ handle, data: new Uint8Array(data) });

        if (data.length > 0 && data[0] === MSG.PASSWORD) {
          md5Shim.queueResponse(handle, buildPostAuthResponse());
        } else if (data.length > 0 && data[0] === MSG.QUERY) {
          md5Shim.queueResponse(handle, buildSelectUsersResponse());
        }

        return data.length;
      };

      const client = new Client({
        user: "md5user",
        password: "md5secret",
        mode: "wasm",
        shim: md5Shim,
      });

      await client.connect();

      // Verify password message was sent
      const pwMsg = md5Shim.sendLog.find(
        (log) => log.data[0] === MSG.PASSWORD,
      );
      expect(pwMsg).toBeDefined();

      // Verify we can query after MD5 auth
      const result = await client.query("SELECT 1");
      expect(result.rows).toHaveLength(2);

      await client.end();
    });

    test("throws if called again after end()", async () => {
      const { Client } = await loadClient();
      const client = new Client({ mode: "wasm", shim });

      await client.connect();
      await client.end();

      try {
        await client.connect();
        expect(true).toBe(false);
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDatabaseError);
        expect((err as WarpGridDatabaseError).message).toMatch(
          /ended|closed/i,
        );
      }
    });

    test("throws WarpGridDatabaseError if shim.connect() fails", async () => {
      const { Client } = await loadClient();
      const failShim = new MockDatabaseShim();
      failShim.failOnConnect(new Error("ECONNREFUSED"));

      const client = new Client({
        host: "unreachable",
        mode: "wasm",
        shim: failShim,
      });

      try {
        await client.connect();
        expect(true).toBe(false); // Should not reach
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDatabaseError);
        expect((err as WarpGridDatabaseError).message).toContain(
          "Failed to connect",
        );
        expect((err as WarpGridDatabaseError).cause).toBeInstanceOf(Error);
      }
    });

    test("throws if called when already connected", async () => {
      const { Client } = await loadClient();
      const client = new Client({ mode: "wasm", shim });

      await client.connect();

      try {
        await client.connect();
        expect(true).toBe(false);
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDatabaseError);
        expect((err as WarpGridDatabaseError).message).toContain(
          "Already connected",
        );
      }

      await client.end();
    });
  });

  // ── query() ────────────────────────────────────────────────────

  describe("query()", () => {
    test("executes simple SELECT and returns { rows, rowCount, fields }", async () => {
      const { Client } = await loadClient();
      const client = new Client({
        host: "db.test",
        database: "testdb",
        user: "testuser",
        mode: "wasm",
        shim,
      });

      await client.connect();
      const result = await client.query(
        "SELECT id, name, email FROM test_users",
      );

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

      await client.end();
    });

    test("returns field metadata with name and dataTypeID", async () => {
      const { Client } = await loadClient();
      const client = new Client({ mode: "wasm", shim });

      await client.connect();
      const result = await client.query("SELECT id, name, email FROM users");

      expect(result.fields).toEqual([
        { name: "id", dataTypeID: 23 },
        { name: "name", dataTypeID: 25 },
        { name: "email", dataTypeID: 25 },
      ]);

      await client.end();
    });

    test("uses extended query protocol with parameters", async () => {
      const { Client } = await loadClient();
      const client = new Client({ mode: "wasm", shim });

      await client.connect();
      const result = await client.query("SELECT id, name FROM users WHERE id = $1", [1]);

      // Check Parse message was sent (extended query protocol)
      const parseSend = shim.sendLog.find(
        (log) => log.data[0] === MSG.PARSE,
      );
      expect(parseSend).toBeDefined();

      expect(result.rows).toHaveLength(1);
      expect(result.rows[0]).toEqual({ id: "1", name: "Alice" });

      await client.end();
    });

    test("handles NULL values in result rows", async () => {
      const { Client } = await loadClient();

      // Custom shim that returns a row with NULL
      const nullShim = new MockDatabaseShim();
      const origSend = nullShim.send.bind(nullShim);
      nullShim.send = (handle: number, data: Uint8Array): number => {
        nullShim.sendLog.push({ handle, data: new Uint8Array(data) });

        if (data.length > 0 && data[0] === MSG.QUERY) {
          nullShim.queueResponse(
            handle,
            concat(
              buildRowDescription([
                { name: "id", typeOID: 23 },
                { name: "name", typeOID: 25 },
              ]),
              buildDataRow(["1", null]),
              buildCommandComplete("SELECT 1"),
              buildReadyForQuery("I"),
            ),
          );
        }

        return data.length;
      };

      const client = new Client({ mode: "wasm", shim: nullShim });
      await client.connect();
      const result = await client.query("SELECT id, name FROM users");

      expect(result.rows[0]).toEqual({ id: "1", name: null });

      await client.end();
    });

    test("supports multiple sequential queries on the same connection", async () => {
      const { Client } = await loadClient();
      const client = new Client({ mode: "wasm", shim });

      await client.connect();

      const r1 = await client.query("SELECT id, name, email FROM users");
      expect(r1.rows).toHaveLength(2);

      const r2 = await client.query("SELECT id, name, email FROM users");
      expect(r2.rows).toHaveLength(2);

      // Only one connect call (single connection)
      expect(shim.connectCount).toBe(1);

      await client.end();
    });

    test("throws if not connected", async () => {
      const { Client } = await loadClient();
      const client = new Client({ mode: "wasm", shim });

      try {
        await client.query("SELECT 1");
        expect(true).toBe(false);
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDatabaseError);
        expect((err as WarpGridDatabaseError).message).toContain(
          "Not connected",
        );
      }
    });
  });

  // ── end() ──────────────────────────────────────────────────────

  describe("end()", () => {
    test("sends Terminate message and closes via shim", async () => {
      const { Client } = await loadClient();
      const client = new Client({ mode: "wasm", shim });

      await client.connect();
      await client.end();

      // Terminate message sent
      const terminateMsg = shim.sendLog.find(
        (log) => log.data[0] === MSG.TERMINATE,
      );
      expect(terminateMsg).toBeDefined();

      // Close called on the handle
      expect(shim.closeLog).toHaveLength(1);
    });

    test("subsequent query() throws after end()", async () => {
      const { Client } = await loadClient();
      const client = new Client({ mode: "wasm", shim });

      await client.connect();
      await client.end();

      try {
        await client.query("SELECT 1");
        expect(true).toBe(false);
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDatabaseError);
        expect((err as WarpGridDatabaseError).message).toMatch(
          /ended|closed|not connected/i,
        );
      }
    });

    test("is safe to call multiple times", async () => {
      const { Client } = await loadClient();
      const client = new Client({ mode: "wasm", shim });

      await client.connect();
      await client.end();
      await client.end(); // Should not throw

      // Only one close call
      expect(shim.closeLog).toHaveLength(1);
    });

    test("can be called without connecting first", async () => {
      const { Client } = await loadClient();
      const client = new Client({ mode: "wasm", shim });

      // Should not throw
      await client.end();
      expect(shim.closeLog).toHaveLength(0);
    });
  });

  // ── Error handling ─────────────────────────────────────────────

  describe("error handling", () => {
    test("Postgres error response surfaces as PostgresError with code, message, detail", async () => {
      const { Client } = await loadClient();

      const errShim = new MockDatabaseShim();
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
                D: "The table was dropped",
              }),
              buildReadyForQuery("I"),
            ),
          );
        }

        return data.length;
      };

      const client = new Client({
        mode: "wasm",
        shim: errShim,
      });

      await client.connect();

      try {
        await client.query("SELECT * FROM missing_table");
        expect(true).toBe(false);
      } catch (err) {
        expect(err).toBeInstanceOf(PostgresError);
        const pgErr = err as PostgresError;
        expect(pgErr.code).toBe("42P01");
        expect(pgErr.message).toContain("missing_table");
        expect(pgErr.detail).toBe("The table was dropped");
        expect(pgErr.severity).toBe("ERROR");
      }

      await client.end();
    });

    test("connection failure wraps original error as cause", async () => {
      const { Client } = await loadClient();
      const failShim = new MockDatabaseShim();
      failShim.failOnConnect(new Error("network unreachable"));

      const client = new Client({
        host: "bad-host",
        mode: "wasm",
        shim: failShim,
      });

      try {
        await client.connect();
        expect(true).toBe(false);
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDatabaseError);
        const dbErr = err as WarpGridDatabaseError;
        expect(dbErr.cause).toBeInstanceOf(Error);
        expect((dbErr.cause as Error).message).toBe("network unreachable");
      }
    });

    test("send error during query throws WarpGridDatabaseError", async () => {
      const { Client } = await loadClient();
      const client = new Client({ mode: "wasm", shim });

      await client.connect();

      // Make send fail after connect
      shim.failOnSend(new Error("broken pipe"));

      try {
        await client.query("SELECT 1");
        expect(true).toBe(false);
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDatabaseError);
      }

      await client.end();
    });

    test("recv failure during query throws WarpGridDatabaseError", async () => {
      const { Client } = await loadClient();
      const recvFailShim = new MockDatabaseShim();

      // Override recv to fail after connect
      let connected = false;
      const origRecv = recvFailShim.recv.bind(recvFailShim);
      recvFailShim.recv = (handle: number, maxBytes: number): Uint8Array => {
        if (connected) {
          throw new Error("connection reset by peer");
        }
        return origRecv(handle, maxBytes);
      };

      const client = new Client({ mode: "wasm", shim: recvFailShim });
      await client.connect();
      connected = true;

      try {
        await client.query("SELECT 1");
        expect(true).toBe(false);
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDatabaseError);
        expect((err as WarpGridDatabaseError).message).toContain(
          "Failed to receive data",
        );
      }

      await client.end();
    });

    test("auth failure during startup throws WarpGridDatabaseError", async () => {
      const { Client } = await loadClient();
      const authFailShim = new MockDatabaseShim();

      const origConnect = authFailShim.connect.bind(authFailShim);
      authFailShim.connect = (config) => {
        const handle = origConnect(config);
        // Server returns ErrorResponse during authentication
        authFailShim.recvQueues.set(handle, [
          concat(
            buildErrorResponse({
              S: "FATAL",
              C: "28P01",
              M: 'password authentication failed for user "baduser"',
            }),
            buildReadyForQuery("I"),
          ),
        ]);
        return handle;
      };

      const client = new Client({
        user: "baduser",
        password: "wrong",
        mode: "wasm",
        shim: authFailShim,
      });

      try {
        await client.connect();
        expect(true).toBe(false);
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDatabaseError);
        expect((err as WarpGridDatabaseError).message).toContain(
          "Authentication failed",
        );
      }
    });
  });

  // ── Mode auto-detection ────────────────────────────────────────

  describe("mode auto-detection", () => {
    test("mode='wasm' with shim creates wasm client", async () => {
      const { Client } = await loadClient();
      const client = new Client({ mode: "wasm", shim });
      await client.connect();
      expect(shim.connectCount).toBe(1);
      await client.end();
    });

    test("mode='wasm' without shim or globals throws", async () => {
      const { Client } = await loadClient();
      const client = new Client({ mode: "wasm" });

      try {
        await client.connect();
        expect(true).toBe(false);
      } catch (err) {
        expect(err).toBeInstanceOf(Error);
        expect((err as Error).message).toContain("DatabaseProxyShim");
      }
    });
  });
});
