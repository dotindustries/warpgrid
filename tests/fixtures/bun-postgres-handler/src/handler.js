/**
 * Wasm entry point for bun-postgres-handler fixture.
 *
 * This is the ComponentizeJS-compatible version of handler.ts. It uses
 * raw WIT imports (warpgrid:shim/database-proxy) instead of the
 * @warpgrid/bun-sdk Pool abstraction, and speaks Postgres wire protocol
 * directly.
 *
 * Routes (must match handler.ts exactly):
 *   POST /users      — insert a new user, return 201 with {id, name, email}
 *   GET  /users/:id  — fetch user by ID, return 200 with {id, name, email} or 404
 *   *    *           — return 404 with {error: "Not Found"}
 *
 * Response bodies are byte-identical to handler.ts for parity testing.
 */

import {
  connect as dbConnect,
  send as dbSend,
  recv as dbRecv,
  close as dbClose,
} from "warpgrid:shim/database-proxy@0.1.0";

// ── Text Encoding ───────────────────────────────────────────────────

const TEXT_ENCODER = new TextEncoder();
const TEXT_DECODER = new TextDecoder();

// ── Postgres Wire Protocol ──────────────────────────────────────────

function buildStartupMessage(database, user) {
  const params = `user\0${user}\0database\0${database}\0\0`;
  const paramsBytes = TEXT_ENCODER.encode(params);
  const len = 4 + 4 + paramsBytes.length;
  const buf = new ArrayBuffer(len);
  const view = new DataView(buf);
  view.setInt32(0, len);
  view.setInt32(4, 196608); // protocol version 3.0
  new Uint8Array(buf).set(paramsBytes, 8);
  return new Uint8Array(buf);
}

function buildExtendedQuery(sql, params) {
  const parts = [];

  // Parse message ('P')
  const sqlBytes = TEXT_ENCODER.encode(sql + "\0");
  const parsePayloadLen = 1 + sqlBytes.length + 2; // portal name NUL + sql + param count
  const parseBuf = new ArrayBuffer(1 + 4 + parsePayloadLen);
  const parseView = new DataView(parseBuf);
  parseView.setUint8(0, 0x50); // 'P'
  parseView.setInt32(1, 4 + parsePayloadLen);
  const parseArr = new Uint8Array(parseBuf);
  parseArr[5] = 0; // unnamed statement
  parseArr.set(sqlBytes, 6);
  parseView.setInt16(6 + sqlBytes.length, 0); // no param type OIDs
  parts.push(parseArr);

  // Bind message ('B')
  const paramBuffers = params.map((p) => TEXT_ENCODER.encode(String(p)));
  let bindPayloadLen = 1 + 1 + 2 + 2; // dest portal NUL + src stmt NUL + format count + param count
  bindPayloadLen += params.length * 4; // length prefixes
  for (const pb of paramBuffers) bindPayloadLen += pb.length;
  bindPayloadLen += 2; // result format count

  const bindBuf = new ArrayBuffer(1 + 4 + bindPayloadLen);
  const bindView = new DataView(bindBuf);
  bindView.setUint8(0, 0x42); // 'B'
  bindView.setInt32(1, 4 + bindPayloadLen);
  const bindArr = new Uint8Array(bindBuf);
  let offset = 5;
  bindArr[offset++] = 0; // unnamed portal
  bindArr[offset++] = 0; // unnamed statement
  bindView.setInt16(offset, 0); // all text format
  offset += 2;
  bindView.setInt16(offset, params.length);
  offset += 2;
  for (let i = 0; i < params.length; i++) {
    bindView.setInt32(offset, paramBuffers[i].length);
    offset += 4;
    bindArr.set(paramBuffers[i], offset);
    offset += paramBuffers[i].length;
  }
  bindView.setInt16(offset, 0); // all text result format
  parts.push(bindArr);

  // Describe message ('D') — describe portal
  const describeBuf = new ArrayBuffer(1 + 4 + 1 + 1);
  const describeView = new DataView(describeBuf);
  describeView.setUint8(0, 0x44); // 'D'
  describeView.setInt32(1, 4 + 2);
  new Uint8Array(describeBuf)[5] = 0x53; // 'S' = statement
  new Uint8Array(describeBuf)[6] = 0; // unnamed
  parts.push(new Uint8Array(describeBuf));

  // Execute message ('E')
  const execBuf = new ArrayBuffer(1 + 4 + 1 + 4);
  const execView = new DataView(execBuf);
  execView.setUint8(0, 0x45); // 'E'
  execView.setInt32(1, 4 + 1 + 4);
  new Uint8Array(execBuf)[5] = 0; // unnamed portal
  execView.setInt32(6, 0); // no row limit
  parts.push(new Uint8Array(execBuf));

  // Sync message ('S')
  const syncBuf = new ArrayBuffer(1 + 4);
  const syncView = new DataView(syncBuf);
  syncView.setUint8(0, 0x53); // 'S'
  syncView.setInt32(1, 4);
  parts.push(new Uint8Array(syncBuf));

  // Concatenate all parts
  const totalLen = parts.reduce((sum, p) => sum + p.length, 0);
  const result = new Uint8Array(totalLen);
  let pos = 0;
  for (const p of parts) {
    result.set(p, pos);
    pos += p.length;
  }
  return result;
}

// ── Response Parsing ────────────────────────────────────────────────

function parseMessages(data) {
  const messages = [];
  let offset = 0;
  while (offset + 5 <= data.length) {
    const type = data[offset];
    const len = new DataView(data.buffer, data.byteOffset + offset + 1, 4).getInt32(0);
    if (offset + 1 + len > data.length) break;
    const payload = data.slice(offset + 5, offset + 1 + len);
    messages.push({ type, payload });
    offset += 1 + len;
  }
  return messages;
}

function parseRowDescription(payload) {
  const view = new DataView(payload.buffer, payload.byteOffset, payload.length);
  const fieldCount = view.getInt16(0);
  const fields = [];
  let pos = 2;
  for (let i = 0; i < fieldCount; i++) {
    const nameEnd = payload.indexOf(0, pos);
    const name = TEXT_DECODER.decode(payload.slice(pos, nameEnd));
    pos = nameEnd + 1 + 4 + 2; // skip NUL, table OID, column attr
    const typeOID = view.getInt32(pos);
    pos += 4 + 2 + 4 + 2; // skip typeOID, size, modifier, format
    fields.push({ name, typeOID });
  }
  return fields;
}

function parseDataRow(payload) {
  const view = new DataView(payload.buffer, payload.byteOffset, payload.length);
  const fieldCount = view.getInt16(0);
  const values = [];
  let pos = 2;
  for (let i = 0; i < fieldCount; i++) {
    const len = view.getInt32(pos);
    pos += 4;
    if (len === -1) {
      values.push(null);
    } else {
      values.push(TEXT_DECODER.decode(payload.slice(pos, pos + len)));
      pos += len;
    }
  }
  return values;
}

// ── Database Connection ─────────────────────────────────────────────

let _dbHandle = null;
let _fieldNames = null;

function ensureConnection() {
  if (_dbHandle !== null) return;

  const host = "localhost";
  const port = 5432;
  const database = "postgres";
  const user = "postgres";

  _dbHandle = dbConnect({ host, port, database, user });

  const startup = buildStartupMessage(database, user);
  dbSend(_dbHandle, startup);

  // Read until ReadyForQuery ('Z')
  let buffer = new Uint8Array(0);
  for (let attempts = 0; attempts < 100; attempts++) {
    const chunk = dbRecv(_dbHandle, 65536);
    if (chunk.length === 0) {
      if (buffer.length > 0) break;
      continue;
    }
    const combined = new Uint8Array(buffer.length + chunk.length);
    combined.set(buffer);
    combined.set(chunk, buffer.length);
    buffer = combined;
    if (containsReadyForQuery(buffer)) break;
  }
}

function containsReadyForQuery(data) {
  for (let i = data.length - 6; i >= 0; i--) {
    if (data[i] === 0x5a) {
      const view = new DataView(data.buffer, data.byteOffset + i + 1, 4);
      if (view.getInt32(0) === 5) return true;
    }
  }
  return false;
}

function queryExtended(sql, params) {
  ensureConnection();
  const msg = buildExtendedQuery(sql, params);
  dbSend(_dbHandle, msg);

  let buffer = new Uint8Array(0);
  for (let attempts = 0; attempts < 100; attempts++) {
    const chunk = dbRecv(_dbHandle, 65536);
    if (chunk.length === 0) {
      if (buffer.length > 0) break;
      continue;
    }
    const combined = new Uint8Array(buffer.length + chunk.length);
    combined.set(buffer);
    combined.set(chunk, buffer.length);
    buffer = combined;
    if (containsReadyForQuery(buffer)) break;
  }

  const messages = parseMessages(buffer);
  let fields = [];
  const rows = [];

  for (const m of messages) {
    if (m.type === 0x54) { // RowDescription 'T'
      fields = parseRowDescription(m.payload);
    } else if (m.type === 0x44) { // DataRow 'D'
      const values = parseDataRow(m.payload);
      const row = {};
      for (let i = 0; i < fields.length && i < values.length; i++) {
        row[fields[i].name] = values[i];
      }
      rows.push(row);
    } else if (m.type === 0x45) { // ErrorResponse 'E'
      throw new Error("Database query failed");
    }
  }

  return { rows, fields };
}

// ── HTTP Handler ────────────────────────────────────────────────────

function jsonResponse(data, status) {
  return new Response(JSON.stringify(data), {
    status: status,
    headers: { "content-type": "application/json" },
  });
}

async function handleCreateUser(request) {
  let body;
  try {
    body = await request.json();
  } catch {
    return jsonResponse({ error: "Invalid JSON body" }, 400);
  }

  if (!body.name || !body.email) {
    return jsonResponse({ error: "Missing required fields: name, email" }, 400);
  }

  try {
    const result = queryExtended(
      "INSERT INTO users (name, email) VALUES ($1, $2) RETURNING id, name, email",
      [body.name, body.email],
    );
    const user = result.rows[0];
    return jsonResponse(user, 201);
  } catch (err) {
    return jsonResponse({ error: "Database error", detail: String(err) }, 503);
  }
}

function handleGetUser(id) {
  try {
    const result = queryExtended(
      "SELECT id, name, email FROM users WHERE id = $1",
      [id],
    );

    if (result.rows.length === 0) {
      return jsonResponse({ error: "User not found" }, 404);
    }

    return jsonResponse(result.rows[0], 200);
  } catch (err) {
    return jsonResponse({ error: "Database error", detail: String(err) }, 503);
  }
}

// ── Fetch Event Listener ────────────────────────────────────────────

addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  const path = url.pathname;
  const method = event.request.method;

  let responsePromise;

  if (method === "POST" && path === "/users") {
    responsePromise = handleCreateUser(event.request);
  } else {
    const userMatch = path.match(/^\/users\/(\d+)$/);
    if (method === "GET" && userMatch) {
      responsePromise = Promise.resolve(handleGetUser(userMatch[1]));
    } else {
      responsePromise = Promise.resolve(
        jsonResponse({ error: "Not Found" }, 404),
      );
    }
  }

  event.respondWith(responsePromise);
});
