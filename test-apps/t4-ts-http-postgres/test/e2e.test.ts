/**
 * End-to-end integration tests for the T4 TypeScript HTTP handler.
 *
 * These tests exercise the full TypeScript pipeline:
 *   handleRequest() → real Client (pg-client.ts) → wire protocol → MockTransport
 *
 * Unlike handler.test.ts (which mocks PgClientLike), these tests use the real
 * Client class backed by a MockTransport that speaks Postgres wire protocol.
 * This validates that the handler, client, and wire protocol layers work together.
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";

import { handleRequest, type PgClientLike } from "../src/handler-logic.js";
import { Client, type Transport, type ClientConfig } from "../src/pg-client.js";

// ── Mock Transport ───────────────────────────────────────────────────────────

/**
 * A MockTransport that simulates a Postgres server at the wire protocol level.
 * Queues raw backend messages that the real Client class will parse.
 */
class MockTransport implements Transport {
  connected = false;
  closed = false;
  handle = 1n;
  sentBuffers: Uint8Array[] = [];
  recvQueue: Uint8Array[] = [];
  connectError: string | null = null;

  connect(config: { host: string; port: number; database: string; user: string; password?: string }): bigint {
    if (this.connectError) throw new Error(this.connectError);
    this.connected = true;
    return this.handle;
  }

  send(handle: bigint, data: Uint8Array): number {
    this.sentBuffers.push(new Uint8Array(data));
    return data.byteLength;
  }

  recv(handle: bigint, maxBytes: number): Uint8Array {
    const next = this.recvQueue.shift();
    if (!next) return new Uint8Array(0);
    if (next.byteLength <= maxBytes) return next;
    this.recvQueue.unshift(next.subarray(maxBytes));
    return next.subarray(0, maxBytes);
  }

  close(handle: bigint): void {
    this.closed = true;
  }

  queueResponse(...messages: Uint8Array[]): void {
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

// ── Wire Protocol Helpers ────────────────────────────────────────────────────

function buildAuthOk(): Uint8Array {
  return new Uint8Array([0x52, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00]);
}

function buildReadyForQuery(status = "I"): Uint8Array {
  return new Uint8Array([0x5a, 0x00, 0x00, 0x00, 0x05, status.charCodeAt(0)]);
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
    view.setInt32(off, 0);
    view.setInt16(off + 4, 0);
    view.setInt32(off + 6, f.typeOid);
    view.setInt16(off + 10, -1);
    view.setInt32(off + 12, -1);
    view.setInt16(off + 16, 0);
    fieldBuffers.push(fieldBuf);
  }

  let payloadLen = 2;
  for (const fb of fieldBuffers) payloadLen += fb.byteLength;

  const msg = new Uint8Array(1 + 4 + payloadLen);
  const view = new DataView(msg.buffer);
  msg[0] = 0x54;
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
  let payloadLen = 2;
  const encoded: Array<Uint8Array | null> = [];

  for (const v of values) {
    if (v === null) {
      encoded.push(null);
      payloadLen += 4;
    } else {
      const bytes = enc.encode(v);
      encoded.push(bytes);
      payloadLen += 4 + bytes.length;
    }
  }

  const msg = new Uint8Array(1 + 4 + payloadLen);
  const view = new DataView(msg.buffer);
  msg[0] = 0x44;
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
  msg[0] = 0x43;
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
  payloadSize += 1;

  const msg = new Uint8Array(1 + 4 + payloadSize);
  const view = new DataView(msg.buffer);
  msg[0] = 0x45;
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
  return new Uint8Array([0x31, 0x00, 0x00, 0x00, 0x04]);
}

function buildBindComplete(): Uint8Array {
  return new Uint8Array([0x32, 0x00, 0x00, 0x00, 0x04]);
}

// ── Test Helpers ─────────────────────────────────────────────────────────────

const USER_FIELDS = [
  { name: "id", typeOid: 23 },
  { name: "name", typeOid: 25 },
];

const DEFAULT_CONFIG: ClientConfig = {
  host: "localhost",
  port: 5432,
  database: "testdb",
  user: "testuser",
};

/**
 * Create a client factory that returns a real Client backed by a MockTransport.
 * The transport is pre-configured with startup handshake responses and query responses.
 */
function createE2EClientFactory(
  querySetup: (transport: MockTransport) => void,
  options?: { connectError?: string }
): { factory: () => PgClientLike; transport: MockTransport } {
  const transport = new MockTransport();

  if (options?.connectError) {
    transport.connectError = options.connectError;
  } else {
    // Queue startup handshake response
    transport.queueResponse(buildAuthOk(), buildReadyForQuery("I"));
  }

  // Let the caller queue query-specific responses
  querySetup(transport);

  const factory = () => new Client(DEFAULT_CONFIG, transport) as unknown as PgClientLike;
  return { factory, transport };
}

// ── E2E Tests ────────────────────────────────────────────────────────────────

describe("E2E: GET /users", () => {
  it("returns JSON array of users through full handler→client→wire pipeline", () => {
    const { factory } = createE2EClientFactory((t) => {
      t.queueResponse(
        buildRowDescription(USER_FIELDS),
        buildDataRow(["1", "Alice Johnson"]),
        buildDataRow(["2", "Bob Smith"]),
        buildDataRow(["3", "Carol Williams"]),
        buildCommandComplete("SELECT 3"),
        buildReadyForQuery("I")
      );
    });

    const response = handleRequest(
      { method: "GET", url: "http://localhost/users", body: null },
      factory
    );

    assert.equal(response.status, 200);
    assert.equal(response.headers["content-type"], "application/json");

    const body = JSON.parse(response.body);
    assert.equal(body.length, 3);
    assert.deepEqual(body[0], { id: "1", name: "Alice Johnson" });
    assert.deepEqual(body[1], { id: "2", name: "Bob Smith" });
    assert.deepEqual(body[2], { id: "3", name: "Carol Williams" });
  });

  it("returns empty JSON array when no users exist", () => {
    const { factory } = createE2EClientFactory((t) => {
      t.queueResponse(
        buildRowDescription(USER_FIELDS),
        buildCommandComplete("SELECT 0"),
        buildReadyForQuery("I")
      );
    });

    const response = handleRequest(
      { method: "GET", url: "http://localhost/users", body: null },
      factory
    );

    assert.equal(response.status, 200);
    const body = JSON.parse(response.body);
    assert.deepEqual(body, []);
  });

  it("returns 500 when database query fails with wire protocol error", () => {
    const { factory } = createE2EClientFactory((t) => {
      t.queueResponse(
        buildErrorResponse("ERROR", "42P01", 'relation "test_users" does not exist'),
        buildReadyForQuery("E")
      );
    });

    const response = handleRequest(
      { method: "GET", url: "http://localhost/users", body: null },
      factory
    );

    assert.equal(response.status, 500);
    assert.equal(response.headers["content-type"], "application/json");

    const body = JSON.parse(response.body);
    assert.ok(body.error.includes("42P01"));
  });
});

describe("E2E: GET /users/:id", () => {
  it("returns single user JSON through parameterized query pipeline", () => {
    const { factory } = createE2EClientFactory((t) => {
      // Parameterized query responses (Extended Query protocol)
      t.queueResponse(
        buildParseComplete(),
        buildBindComplete(),
        buildRowDescription(USER_FIELDS),
        buildDataRow(["1", "Alice Johnson"]),
        buildCommandComplete("SELECT 1"),
        buildReadyForQuery("I")
      );
    });

    const response = handleRequest(
      { method: "GET", url: "http://localhost/users/1", body: null },
      factory
    );

    assert.equal(response.status, 200);
    assert.equal(response.headers["content-type"], "application/json");

    const body = JSON.parse(response.body);
    assert.deepEqual(body, { id: "1", name: "Alice Johnson" });
  });

  it("returns 404 when user not found via parameterized query", () => {
    const { factory } = createE2EClientFactory((t) => {
      t.queueResponse(
        buildParseComplete(),
        buildBindComplete(),
        buildRowDescription(USER_FIELDS),
        buildCommandComplete("SELECT 0"),
        buildReadyForQuery("I")
      );
    });

    const response = handleRequest(
      { method: "GET", url: "http://localhost/users/999", body: null },
      factory
    );

    assert.equal(response.status, 404);
    const body = JSON.parse(response.body);
    assert.ok(body.error.includes("not found"));
  });

  it("returns 400 for non-numeric user ID (abc)", () => {
    // No transport setup needed — validation happens before DB call
    const { factory } = createE2EClientFactory(() => {});

    const response = handleRequest(
      { method: "GET", url: "http://localhost/users/abc", body: null },
      factory
    );

    assert.equal(response.status, 400);
    const body = JSON.parse(response.body);
    assert.ok(body.error.includes("invalid"));
  });

  it("returns 400 for negative user ID (-1)", () => {
    const { factory } = createE2EClientFactory(() => {});

    const response = handleRequest(
      { method: "GET", url: "http://localhost/users/-1", body: null },
      factory
    );

    assert.equal(response.status, 400);
    const body = JSON.parse(response.body);
    assert.ok(body.error.includes("invalid"));
  });
});

describe("E2E: POST /users", () => {
  it("creates user and returns 201 with new user through parameterized INSERT", () => {
    const { factory, transport } = createE2EClientFactory((t) => {
      t.queueResponse(
        buildParseComplete(),
        buildBindComplete(),
        buildRowDescription(USER_FIELDS),
        buildDataRow(["6", "NewUser"]),
        buildCommandComplete("INSERT 0 1"),
        buildReadyForQuery("I")
      );
    });

    const response = handleRequest(
      { method: "POST", url: "http://localhost/users", body: JSON.stringify({ name: "NewUser" }) },
      factory
    );

    assert.equal(response.status, 201);
    assert.equal(response.headers["content-type"], "application/json");

    const body = JSON.parse(response.body);
    assert.equal(body.id, "6");
    assert.equal(body.name, "NewUser");

    // Verify the client actually sent wire protocol messages:
    // sentBuffers[0] = startup, sentBuffers[1] = extended query, sentBuffers[2] = terminate
    assert.ok(transport.sentBuffers.length >= 2, "at least startup + query messages sent");
  });

  it("returns 400 for missing request body", () => {
    const { factory } = createE2EClientFactory(() => {});

    const response = handleRequest(
      { method: "POST", url: "http://localhost/users", body: null },
      factory
    );

    assert.equal(response.status, 400);
    const body = JSON.parse(response.body);
    assert.ok(body.error.includes("required") || body.error.includes("body"));
  });

  it("returns 400 for invalid JSON body", () => {
    const { factory } = createE2EClientFactory(() => {});

    const response = handleRequest(
      { method: "POST", url: "http://localhost/users", body: "{not valid json}" },
      factory
    );

    assert.equal(response.status, 400);
    const body = JSON.parse(response.body);
    assert.ok(body.error.includes("invalid") || body.error.includes("JSON"));
  });

  it("returns 400 for missing name field", () => {
    const { factory } = createE2EClientFactory(() => {});

    const response = handleRequest(
      { method: "POST", url: "http://localhost/users", body: JSON.stringify({ email: "a@b.com" }) },
      factory
    );

    assert.equal(response.status, 400);
    const body = JSON.parse(response.body);
    assert.ok(body.error.includes("name"));
  });

  it("returns 400 for empty name field", () => {
    const { factory } = createE2EClientFactory(() => {});

    const response = handleRequest(
      { method: "POST", url: "http://localhost/users", body: JSON.stringify({ name: "" }) },
      factory
    );

    assert.equal(response.status, 400);
    const body = JSON.parse(response.body);
    assert.ok(body.error.includes("name"));
  });

  it("returns 500 when database INSERT fails with wire protocol error", () => {
    const { factory } = createE2EClientFactory((t) => {
      t.queueResponse(
        buildErrorResponse("ERROR", "23505", "duplicate key value violates unique constraint"),
        buildReadyForQuery("E")
      );
    });

    const response = handleRequest(
      { method: "POST", url: "http://localhost/users", body: JSON.stringify({ name: "Alice" }) },
      factory
    );

    assert.equal(response.status, 500);
    const body = JSON.parse(response.body);
    assert.ok(body.error.includes("23505") || body.error.includes("duplicate"));
  });
});

describe("E2E: Connection failure", () => {
  it("returns 500 when transport connection fails", () => {
    const { factory } = createE2EClientFactory(() => {}, {
      connectError: "ECONNREFUSED: connection refused",
    });

    const response = handleRequest(
      { method: "GET", url: "http://localhost/users", body: null },
      factory
    );

    assert.equal(response.status, 500);
    assert.equal(response.headers["content-type"], "application/json");

    const body = JSON.parse(response.body);
    assert.ok(body.error.includes("ECONNREFUSED") || body.error.includes("connection"));
  });
});

describe("E2E: Routing edge cases", () => {
  it("returns 404 for unknown route", () => {
    const { factory } = createE2EClientFactory(() => {});

    const response = handleRequest(
      { method: "GET", url: "http://localhost/nonexistent", body: null },
      factory
    );

    assert.equal(response.status, 404);
    const body = JSON.parse(response.body);
    assert.ok(body.error.includes("not found"));
  });

  it("returns 405 for unsupported method on /users", () => {
    const { factory } = createE2EClientFactory(() => {});

    const response = handleRequest(
      { method: "DELETE", url: "http://localhost/users", body: null },
      factory
    );

    assert.equal(response.status, 405);
    const body = JSON.parse(response.body);
    assert.ok(body.error.includes("not allowed"));
  });
});

describe("E2E: Content-Type validation", () => {
  it("all responses have application/json Content-Type", () => {
    // Test success response
    const { factory: f1 } = createE2EClientFactory((t) => {
      t.queueResponse(
        buildRowDescription(USER_FIELDS),
        buildCommandComplete("SELECT 0"),
        buildReadyForQuery("I")
      );
    });
    const r1 = handleRequest({ method: "GET", url: "http://localhost/users", body: null }, f1);
    assert.equal(r1.headers["content-type"], "application/json", "200 response");

    // Test 400 error
    const { factory: f2 } = createE2EClientFactory(() => {});
    const r2 = handleRequest({ method: "GET", url: "http://localhost/users/abc", body: null }, f2);
    assert.equal(r2.headers["content-type"], "application/json", "400 response");

    // Test 404 error
    const { factory: f3 } = createE2EClientFactory(() => {});
    const r3 = handleRequest({ method: "GET", url: "http://localhost/notfound", body: null }, f3);
    assert.equal(r3.headers["content-type"], "application/json", "404 response");

    // Test 500 error
    const { factory: f4 } = createE2EClientFactory(() => {}, { connectError: "fail" });
    const r4 = handleRequest({ method: "GET", url: "http://localhost/users", body: null }, f4);
    assert.equal(r4.headers["content-type"], "application/json", "500 response");
  });
});
