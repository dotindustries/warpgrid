/**
 * Unit tests for the Postgres client (pg-client.ts).
 *
 * Uses a mock Transport that simulates the Postgres wire protocol server side,
 * allowing us to test the client's connection, query, and error handling logic
 * without a real database or WIT shim.
 */
import { describe, it, beforeEach } from "node:test";
import assert from "node:assert/strict";

import { Client, type Transport, type ClientConfig, type QueryResult } from "../src/pg-client.js";
import {
  encodeStartup,
  BackendMessageType,
} from "../src/pg-wire.js";

// ── Mock Transport ───────────────────────────────────────────────────────────

/**
 * A mock Transport that records sent data and returns pre-configured responses.
 * Simulates the Postgres server side of the wire protocol.
 */
class MockTransport implements Transport {
  connected = false;
  closed = false;
  handle = 1n;
  sentBuffers: Uint8Array[] = [];
  recvQueue: Uint8Array[] = [];
  connectError: string | null = null;
  sendError: string | null = null;
  recvError: string | null = null;

  connect(config: { host: string; port: number; database: string; user: string; password?: string }): bigint {
    if (this.connectError) {
      throw new Error(this.connectError);
    }
    this.connected = true;
    return this.handle;
  }

  send(handle: bigint, data: Uint8Array): number {
    if (this.sendError) {
      throw new Error(this.sendError);
    }
    this.sentBuffers.push(new Uint8Array(data));
    return data.byteLength;
  }

  recv(handle: bigint, maxBytes: number): Uint8Array {
    if (this.recvError) {
      throw new Error(this.recvError);
    }
    const next = this.recvQueue.shift();
    if (!next) {
      return new Uint8Array(0);
    }
    if (next.byteLength <= maxBytes) {
      return next;
    }
    // Split: return up to maxBytes, re-queue the rest
    this.recvQueue.unshift(next.subarray(maxBytes));
    return next.subarray(0, maxBytes);
  }

  close(handle: bigint): void {
    this.closed = true;
  }

  /** Queue a complete Postgres backend response for the mock to return. */
  queueResponse(...messages: Uint8Array[]): void {
    // Concatenate all messages into one buffer (simulating a single recv)
    let totalLen = 0;
    for (const m of messages) totalLen += m.byteLength;
    const buf = new Uint8Array(totalLen);
    let offset = 0;
    for (const m of messages) {
      buf.set(m, offset);
      offset += m.byteLength;
    }
    this.recvQueue.push(buf);
  }
}

// ── Helper: Build backend messages ───────────────────────────────────────────

function buildAuthOk(): Uint8Array {
  // R + len(8) + type(0)
  return new Uint8Array([0x52, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00]);
}

function buildAuthCleartextPassword(): Uint8Array {
  return new Uint8Array([0x52, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x03]);
}

function buildReadyForQuery(status: string = "I"): Uint8Array {
  return new Uint8Array([0x5a, 0x00, 0x00, 0x00, 0x05, status.charCodeAt(0)]);
}

function buildParameterStatus(key: string, value: string): Uint8Array {
  const enc = new TextEncoder();
  const keyBytes = enc.encode(key);
  const valBytes = enc.encode(value);
  const len = 4 + keyBytes.length + 1 + valBytes.length + 1;
  const buf = new Uint8Array(1 + len);
  const view = new DataView(buf.buffer);
  buf[0] = 0x53; // 'S'
  view.setInt32(1, len);
  let off = 5;
  buf.set(keyBytes, off);
  off += keyBytes.length;
  buf[off++] = 0;
  buf.set(valBytes, off);
  off += valBytes.length;
  buf[off] = 0;
  return buf;
}

function buildRowDescription(fields: Array<{ name: string; typeOid: number }>): Uint8Array {
  const enc = new TextEncoder();
  const fieldBuffers: Uint8Array[] = [];

  for (const f of fields) {
    const nameBytes = enc.encode(f.name);
    const fieldBuf = new Uint8Array(nameBytes.length + 1 + 18);
    fieldBuf.set(nameBytes, 0);
    fieldBuf[nameBytes.length] = 0;
    const view = new DataView(fieldBuf.buffer, fieldBuf.byteOffset, fieldBuf.byteLength);
    const off = nameBytes.length + 1;
    view.setInt32(off, 0);      // table oid
    view.setInt16(off + 4, 0);  // col number
    view.setInt32(off + 6, f.typeOid); // type oid
    view.setInt16(off + 10, -1); // type length
    view.setInt32(off + 12, -1); // type modifier
    view.setInt16(off + 16, 0);  // format (text)
    fieldBuffers.push(fieldBuf);
  }

  let payloadLen = 2;
  for (const fb of fieldBuffers) payloadLen += fb.byteLength;

  const msg = new Uint8Array(1 + 4 + payloadLen);
  const view = new DataView(msg.buffer);
  msg[0] = 0x54; // 'T'
  view.setInt32(1, 4 + payloadLen);
  view.setInt16(5, fields.length);
  let off = 7;
  for (const fb of fieldBuffers) {
    msg.set(fb, off);
    off += fb.byteLength;
  }
  return msg;
}

function buildDataRow(values: Array<string | null>): Uint8Array {
  const enc = new TextEncoder();
  let payloadLen = 2; // num_cols

  const encoded: Array<Uint8Array | null> = [];
  for (const v of values) {
    if (v === null) {
      encoded.push(null);
      payloadLen += 4; // -1 length
    } else {
      const bytes = enc.encode(v);
      encoded.push(bytes);
      payloadLen += 4 + bytes.length;
    }
  }

  const msg = new Uint8Array(1 + 4 + payloadLen);
  const view = new DataView(msg.buffer);
  msg[0] = 0x44; // 'D'
  view.setInt32(1, 4 + payloadLen);
  view.setInt16(5, values.length);
  let off = 7;
  for (const e of encoded) {
    if (e === null) {
      view.setInt32(off, -1);
      off += 4;
    } else {
      view.setInt32(off, e.length);
      msg.set(e, off + 4);
      off += 4 + e.length;
    }
  }
  return msg;
}

function buildCommandComplete(tag: string): Uint8Array {
  const enc = new TextEncoder();
  const tagBytes = enc.encode(tag);
  const len = 4 + tagBytes.length + 1;
  const msg = new Uint8Array(1 + len);
  const view = new DataView(msg.buffer);
  msg[0] = 0x43; // 'C'
  view.setInt32(1, len);
  msg.set(tagBytes, 5);
  msg[5 + tagBytes.length] = 0;
  return msg;
}

function buildErrorResponse(severity: string, code: string, message: string): Uint8Array {
  const enc = new TextEncoder();
  const fields = [
    { code: "S", value: severity },
    { code: "C", value: code },
    { code: "M", value: message },
  ];

  let payloadSize = 0;
  for (const f of fields) {
    payloadSize += 1 + enc.encode(f.value).length + 1;
  }
  payloadSize += 1; // terminator

  const msg = new Uint8Array(1 + 4 + payloadSize);
  const view = new DataView(msg.buffer);
  msg[0] = 0x45; // 'E'
  view.setInt32(1, 4 + payloadSize);
  let off = 5;
  for (const f of fields) {
    msg[off++] = f.code.charCodeAt(0);
    const valBytes = enc.encode(f.value);
    msg.set(valBytes, off);
    off += valBytes.length;
    msg[off++] = 0;
  }
  msg[off] = 0;
  return msg;
}

function buildParseComplete(): Uint8Array {
  return new Uint8Array([0x31, 0x00, 0x00, 0x00, 0x04]); // '1' + len(4)
}

function buildBindComplete(): Uint8Array {
  return new Uint8Array([0x32, 0x00, 0x00, 0x00, 0x04]); // '2' + len(4)
}

// ── Client Tests ─────────────────────────────────────────────────────────────

describe("Client.connect", () => {
  it("performs startup handshake and reaches ready state", () => {
    const transport = new MockTransport();

    // Queue the server's startup response
    transport.queueResponse(
      buildAuthOk(),
      buildParameterStatus("server_version", "16.0"),
      buildReadyForQuery("I")
    );

    const client = new Client(
      { host: "localhost", port: 5432, database: "testdb", user: "testuser" },
      transport
    );

    client.connect();

    assert.ok(transport.connected, "transport was connected");
    assert.equal(transport.sentBuffers.length, 1, "one message sent (startup)");

    // Verify the startup message was sent
    const startup = transport.sentBuffers[0];
    const view = new DataView(startup.buffer, startup.byteOffset, startup.byteLength);
    assert.equal(view.getInt32(4), 196608, "protocol version 3.0");
  });

  it("handles cleartext password authentication", () => {
    const transport = new MockTransport();

    // Queue: auth request, then auth ok + ready
    transport.queueResponse(buildAuthCleartextPassword());
    transport.queueResponse(
      buildAuthOk(),
      buildParameterStatus("server_version", "16.0"),
      buildReadyForQuery("I")
    );

    const client = new Client(
      { host: "localhost", port: 5432, database: "testdb", user: "testuser", password: "secret" },
      transport
    );

    client.connect();

    assert.equal(transport.sentBuffers.length, 2, "startup + password messages sent");
    // Second message should be password
    assert.equal(transport.sentBuffers[1][0], 0x70, "second message is PasswordMessage ('p')");
  });

  it("throws on connection failure", () => {
    const transport = new MockTransport();
    transport.connectError = "connection refused";

    const client = new Client(
      { host: "localhost", port: 5432, database: "testdb", user: "testuser" },
      transport
    );

    assert.throws(() => client.connect(), /connection refused/);
  });
});

describe("Client.query (simple)", () => {
  function createConnectedClient(): { client: Client; transport: MockTransport } {
    const transport = new MockTransport();
    transport.queueResponse(
      buildAuthOk(),
      buildReadyForQuery("I")
    );

    const client = new Client(
      { host: "localhost", port: 5432, database: "testdb", user: "testuser" },
      transport
    );
    client.connect();

    // Clear sent buffers from startup
    transport.sentBuffers = [];
    return { client, transport };
  }

  it("executes SELECT and returns rows", () => {
    const { client, transport } = createConnectedClient();

    transport.queueResponse(
      buildRowDescription([
        { name: "id", typeOid: 23 },
        { name: "name", typeOid: 25 },
      ]),
      buildDataRow(["1", "Alice"]),
      buildDataRow(["2", "Bob"]),
      buildCommandComplete("SELECT 2"),
      buildReadyForQuery("I")
    );

    const result = client.query("SELECT id, name FROM test_users ORDER BY id");

    assert.equal(result.rowCount, 2);
    assert.equal(result.fields.length, 2);
    assert.equal(result.fields[0].name, "id");
    assert.equal(result.fields[1].name, "name");
    assert.equal(result.rows.length, 2);
    assert.deepEqual(result.rows[0], { id: "1", name: "Alice" });
    assert.deepEqual(result.rows[1], { id: "2", name: "Bob" });
  });

  it("executes INSERT and returns affected row count", () => {
    const { client, transport } = createConnectedClient();

    transport.queueResponse(
      buildCommandComplete("INSERT 0 1"),
      buildReadyForQuery("I")
    );

    const result = client.query("INSERT INTO test_users (name) VALUES ('Charlie')");

    assert.equal(result.rowCount, 1);
    assert.equal(result.rows.length, 0);
  });

  it("handles empty result set", () => {
    const { client, transport } = createConnectedClient();

    transport.queueResponse(
      buildRowDescription([
        { name: "id", typeOid: 23 },
        { name: "name", typeOid: 25 },
      ]),
      buildCommandComplete("SELECT 0"),
      buildReadyForQuery("I")
    );

    const result = client.query("SELECT * FROM test_users WHERE 1=0");

    assert.equal(result.rowCount, 0);
    assert.equal(result.rows.length, 0);
    assert.equal(result.fields.length, 2);
  });

  it("throws on database error", () => {
    const { client, transport } = createConnectedClient();

    transport.queueResponse(
      buildErrorResponse("ERROR", "42P01", 'relation "nonexistent" does not exist'),
      buildReadyForQuery("E")
    );

    assert.throws(
      () => client.query("SELECT * FROM nonexistent"),
      (err: unknown) => {
        const e = err as Error;
        return e.message.includes("42P01") && e.message.includes("nonexistent");
      }
    );
  });
});

describe("Client.query (parameterized)", () => {
  function createConnectedClient(): { client: Client; transport: MockTransport } {
    const transport = new MockTransport();
    transport.queueResponse(
      buildAuthOk(),
      buildReadyForQuery("I")
    );

    const client = new Client(
      { host: "localhost", port: 5432, database: "testdb", user: "testuser" },
      transport
    );
    client.connect();
    transport.sentBuffers = [];
    return { client, transport };
  }

  it("executes parameterized SELECT and returns rows", () => {
    const { client, transport } = createConnectedClient();

    transport.queueResponse(
      buildParseComplete(),
      buildBindComplete(),
      buildRowDescription([
        { name: "id", typeOid: 23 },
        { name: "name", typeOid: 25 },
      ]),
      buildDataRow(["1", "Alice"]),
      buildCommandComplete("SELECT 1"),
      buildReadyForQuery("I")
    );

    const result = client.query("SELECT * FROM test_users WHERE id = $1", ["1"]);

    assert.equal(result.rowCount, 1);
    assert.deepEqual(result.rows[0], { id: "1", name: "Alice" });
  });

  it("executes parameterized INSERT RETURNING", () => {
    const { client, transport } = createConnectedClient();

    transport.queueResponse(
      buildParseComplete(),
      buildBindComplete(),
      buildRowDescription([
        { name: "id", typeOid: 23 },
      ]),
      buildDataRow(["6"]),
      buildCommandComplete("INSERT 0 1"),
      buildReadyForQuery("I")
    );

    const result = client.query(
      "INSERT INTO test_users (name) VALUES ($1) RETURNING id",
      ["NewUser"]
    );

    assert.equal(result.rowCount, 1);
    assert.deepEqual(result.rows[0], { id: "6" });
  });

  it("handles NULL parameters", () => {
    const { client, transport } = createConnectedClient();

    transport.queueResponse(
      buildParseComplete(),
      buildBindComplete(),
      buildCommandComplete("INSERT 0 1"),
      buildReadyForQuery("I")
    );

    // Should not throw
    const result = client.query("INSERT INTO t (col) VALUES ($1)", [null]);
    assert.equal(result.rowCount, 1);
  });
});

describe("Client.end", () => {
  it("sends Terminate message and closes transport", () => {
    const transport = new MockTransport();
    transport.queueResponse(buildAuthOk(), buildReadyForQuery("I"));

    const client = new Client(
      { host: "localhost", port: 5432, database: "testdb", user: "testuser" },
      transport
    );
    client.connect();
    transport.sentBuffers = [];

    client.end();

    assert.ok(transport.closed, "transport was closed");
    // Should have sent Terminate message
    assert.equal(transport.sentBuffers.length, 1);
    assert.equal(transport.sentBuffers[0][0], 0x58, "sent Terminate ('X') message");
  });

  it("is idempotent — calling end() twice does not throw", () => {
    const transport = new MockTransport();
    transport.queueResponse(buildAuthOk(), buildReadyForQuery("I"));

    const client = new Client(
      { host: "localhost", port: 5432, database: "testdb", user: "testuser" },
      transport
    );
    client.connect();

    client.end();
    client.end(); // Should not throw
    assert.ok(true, "double end() did not throw");
  });
});
