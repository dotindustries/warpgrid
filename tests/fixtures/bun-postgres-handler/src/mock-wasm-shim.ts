/**
 * MockWasmShim — A DatabaseProxyShim that speaks Postgres wire protocol.
 *
 * Simulates the host-side database proxy for Wasm mode testing.
 * Returns Postgres wire protocol responses equivalent to the seed data
 * used by MockNativePool, enabling byte-identical parity testing.
 */

import type { DatabaseProxyShim } from "@warpgrid/bun-sdk/postgres";

const TEXT_ENCODER = new TextEncoder();

// ── Seed Data (must match MockNativePool) ───────────────────────────

const SEED_USERS = [
  { id: "1", name: "Alice", email: "alice@test.com" },
  { id: "2", name: "Bob", email: "bob@test.com" },
  { id: "3", name: "Charlie", email: "charlie@test.com" },
];

const USER_FIELDS = [
  { name: "id", typeOID: 23 },
  { name: "name", typeOID: 25 },
  { name: "email", typeOID: 25 },
];

// ── Postgres Wire Protocol Builders ─────────────────────────────────

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
  new DataView(buf).setInt32(0, 0); // AUTH_OK
  return buildBackendMessage(0x52, new Uint8Array(buf)); // 'R'
}

function buildParamStatus(name: string, value: string): Uint8Array {
  return buildBackendMessage(
    0x53, // 'S'
    TEXT_ENCODER.encode(`${name}\0${value}\0`),
  );
}

function buildBackendKeyData(): Uint8Array {
  const buf = new ArrayBuffer(8);
  const view = new DataView(buf);
  view.setInt32(0, 1234);
  view.setInt32(4, 5678);
  return buildBackendMessage(0x4b, new Uint8Array(buf)); // 'K'
}

function buildReadyForQuery(status = "I"): Uint8Array {
  return buildBackendMessage(
    0x5a, // 'Z'
    new Uint8Array([status.charCodeAt(0)]),
  );
}

function buildRowDescription(
  fields: { name: string; typeOID: number }[],
): Uint8Array {
  const parts: number[] = [];
  parts.push((fields.length >> 8) & 0xff, fields.length & 0xff);
  for (const f of fields) {
    const nameBytes = TEXT_ENCODER.encode(f.name);
    for (const b of nameBytes) parts.push(b);
    parts.push(0); // null terminator
    parts.push(0, 0, 0, 0); // table OID
    parts.push(0, 0); // column attribute
    parts.push(
      (f.typeOID >> 24) & 0xff,
      (f.typeOID >> 16) & 0xff,
      (f.typeOID >> 8) & 0xff,
      f.typeOID & 0xff,
    );
    parts.push(0xff, 0xff); // data type size
    parts.push(0xff, 0xff, 0xff, 0xff); // type modifier
    parts.push(0, 0); // format code
  }
  return buildBackendMessage(0x54, new Uint8Array(parts)); // 'T'
}

function buildDataRow(values: (string | null)[]): Uint8Array {
  const parts: number[] = [];
  parts.push((values.length >> 8) & 0xff, values.length & 0xff);
  for (const v of values) {
    if (v === null) {
      parts.push(0xff, 0xff, 0xff, 0xff);
    } else {
      const bytes = TEXT_ENCODER.encode(v);
      parts.push(
        (bytes.length >> 24) & 0xff,
        (bytes.length >> 16) & 0xff,
        (bytes.length >> 8) & 0xff,
        bytes.length & 0xff,
      );
      for (const b of bytes) parts.push(b);
    }
  }
  return buildBackendMessage(0x44, new Uint8Array(parts)); // 'D'
}

function buildCommandComplete(tag: string): Uint8Array {
  return buildBackendMessage(
    0x43, // 'C'
    TEXT_ENCODER.encode(tag + "\0"),
  );
}

function buildParseComplete(): Uint8Array {
  return buildBackendMessage(0x31, new Uint8Array(0)); // '1'
}

function buildBindComplete(): Uint8Array {
  return buildBackendMessage(0x32, new Uint8Array(0)); // '2'
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

// ── Query Parsing ───────────────────────────────────────────────────

const TEXT_DECODER = new TextDecoder();

function extractSimpleQuery(data: Uint8Array): string | null {
  if (data.length > 0 && data[0] === 0x51) {
    // 'Q' simple query
    const payload = data.slice(5);
    const nullIdx = payload.indexOf(0);
    if (nullIdx >= 0) {
      return TEXT_DECODER.decode(payload.slice(0, nullIdx));
    }
  }
  return null;
}

function extractExtendedQuerySql(data: Uint8Array): string | null {
  if (data.length > 0 && data[0] === 0x50) {
    // 'P' parse
    // Skip type byte + length (4) + portal name (1 null byte)
    const payloadStart = 5;
    const portalEnd = data.indexOf(0, payloadStart);
    if (portalEnd < 0) return null;
    const sqlStart = portalEnd + 1;
    const sqlEnd = data.indexOf(0, sqlStart);
    if (sqlEnd < 0) return null;
    return TEXT_DECODER.decode(data.slice(sqlStart, sqlEnd));
  }
  return null;
}

function extractBindParams(data: Uint8Array): string[] {
  // Find the Bind message ('B' = 0x42) in the data
  let offset = 0;
  while (offset < data.length) {
    const msgType = data[offset];
    if (offset + 5 > data.length) break;
    const msgLen = new DataView(data.buffer, data.byteOffset + offset + 1, 4).getInt32(0);
    if (msgType === 0x42) {
      // Bind message found
      return parseBindParams(data.slice(offset + 5, offset + 1 + msgLen));
    }
    offset += 1 + msgLen;
  }
  return [];
}

function parseBindParams(payload: Uint8Array): string[] {
  const view = new DataView(payload.buffer, payload.byteOffset, payload.length);
  let offset = 0;

  // portal name (null-terminated)
  while (offset < payload.length && payload[offset] !== 0) offset++;
  offset++; // skip null
  // statement name (null-terminated)
  while (offset < payload.length && payload[offset] !== 0) offset++;
  offset++; // skip null

  // format codes count
  const formatCodesCount = view.getInt16(offset);
  offset += 2;
  offset += formatCodesCount * 2; // skip format codes

  // param count
  const paramCount = view.getInt16(offset);
  offset += 2;

  const params: string[] = [];
  for (let i = 0; i < paramCount; i++) {
    const len = view.getInt32(offset);
    offset += 4;
    if (len === -1) {
      params.push("");
    } else {
      params.push(TEXT_DECODER.decode(payload.slice(offset, offset + len)));
      offset += len;
    }
  }
  return params;
}

// ── Mock Shim Implementation ────────────────────────────────────────

let nextId = SEED_USERS.length + 1;
const insertedUsers: { id: string; name: string; email: string }[] = [];

export function resetMockWasmState(): void {
  nextId = SEED_USERS.length + 1;
  insertedUsers.length = 0;
}

export function createMockWasmShim(): DatabaseProxyShim {
  const handles = new Set<number>();
  let handleCounter = 1;
  const recvQueues = new Map<number, Uint8Array[]>();

  function queueResponse(handle: number, data: Uint8Array): void {
    const queue = recvQueues.get(handle) ?? [];
    queue.push(data);
    recvQueues.set(handle, queue);
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

  function buildSelectByIdResponse(id: string): Uint8Array {
    const allUsers = [...SEED_USERS, ...insertedUsers];
    const user = allUsers.find((u) => u.id === id);
    if (!user) {
      return concat(
        buildRowDescription(USER_FIELDS),
        buildCommandComplete("SELECT 0"),
        buildReadyForQuery("I"),
      );
    }
    return concat(
      buildRowDescription(USER_FIELDS),
      buildDataRow([user.id, user.name, user.email]),
      buildCommandComplete("SELECT 1"),
      buildReadyForQuery("I"),
    );
  }

  function buildInsertResponse(name: string, email: string): Uint8Array {
    const id = String(nextId++);
    insertedUsers.push({ id, name, email });
    return concat(
      buildParseComplete(),
      buildBindComplete(),
      buildRowDescription(USER_FIELDS),
      buildDataRow([id, name, email]),
      buildCommandComplete("INSERT 0 1"),
      buildReadyForQuery("I"),
    );
  }

  function buildSelectByIdExtendedResponse(id: string): Uint8Array {
    const allUsers = [...SEED_USERS, ...insertedUsers];
    const user = allUsers.find((u) => u.id === id);
    if (!user) {
      return concat(
        buildParseComplete(),
        buildBindComplete(),
        buildRowDescription(USER_FIELDS),
        buildCommandComplete("SELECT 0"),
        buildReadyForQuery("I"),
      );
    }
    return concat(
      buildParseComplete(),
      buildBindComplete(),
      buildRowDescription(USER_FIELDS),
      buildDataRow([user.id, user.name, user.email]),
      buildCommandComplete("SELECT 1"),
      buildReadyForQuery("I"),
    );
  }

  return {
    connect(): number {
      const handle = handleCounter++;
      handles.add(handle);
      queueResponse(handle, buildStartupResponse());
      return handle;
    },

    send(handle: number, data: Uint8Array): number {
      if (!handles.has(handle)) throw new Error(`invalid handle: ${handle}`);

      // Detect query type and queue appropriate response
      const simpleQuery = extractSimpleQuery(data);
      if (simpleQuery) {
        // Simple query — not used in our handler (it always uses params)
        queueResponse(handle, concat(
          buildRowDescription(USER_FIELDS),
          buildCommandComplete("SELECT 0"),
          buildReadyForQuery("I"),
        ));
        return data.length;
      }

      const extendedSql = extractExtendedQuerySql(data);
      if (extendedSql) {
        const params = extractBindParams(data);

        if (extendedSql.startsWith("INSERT INTO users")) {
          const name = params[0] ?? "";
          const email = params[1] ?? "";
          queueResponse(handle, buildInsertResponse(name, email));
        } else if (extendedSql.includes("WHERE id = $1")) {
          const id = params[0] ?? "";
          queueResponse(handle, buildSelectByIdExtendedResponse(id));
        } else {
          queueResponse(handle, concat(
            buildParseComplete(),
            buildBindComplete(),
            buildCommandComplete("SELECT 0"),
            buildReadyForQuery("I"),
          ));
        }
        return data.length;
      }

      // Terminate or unknown — no response
      return data.length;
    },

    recv(handle: number, maxBytes: number): Uint8Array {
      if (!handles.has(handle)) throw new Error(`invalid handle: ${handle}`);
      const queue = recvQueues.get(handle) ?? [];
      if (queue.length === 0) return new Uint8Array(0);
      const response = queue.shift()!;
      return response.slice(0, maxBytes);
    },

    close(handle: number): void {
      handles.delete(handle);
      recvQueues.delete(handle);
    },
  };
}
