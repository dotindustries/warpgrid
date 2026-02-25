/**
 * T5 Integration Test: Bun HTTP handler with Postgres via WarpGrid shims.
 *
 * This handler demonstrates the Bun-targeted warpgrid SDK pattern:
 * - HTTP routing via web-standard fetch event
 * - Database access via warpgrid:shim/database-proxy WIT imports
 * - Environment variable access via Bun.env polyfill / process.env
 * - Bun.sleep polyfill for WASI clocks
 *
 * Response parity: produces byte-identical responses to T4 for the same
 * requests (same JSON keys, ordering, headers, status codes).
 *
 * Routes:
 *   GET  /users      — list all users from test_users table
 *   POST /users      — insert a new user, return 201
 *   GET  /health     — health check
 *
 * Environment:
 *   APP_NAME          — included in X-App-Name response header
 *   DB_HOST           — Postgres host (default: db.test.warp.local)
 *   DB_PORT           — Postgres port (default: 5432)
 *   DB_NAME           — database name (default: testdb)
 *   DB_USER           — database user (default: testuser)
 */

// WarpGrid shim imports (warpgrid:shim/database-proxy)
import {
  connect as dbConnect,
  send as dbSend,
  recv as dbRecv,
  close as dbClose,
} from "warpgrid:shim/database-proxy@0.1.0";

// ── Bun Polyfill Helpers ────────────────────────────────────────────

function getBunEnv(key) {
  if (typeof globalThis.Bun !== "undefined" && globalThis.Bun.env) {
    return globalThis.Bun.env[key];
  }
  if (typeof globalThis.process !== "undefined" && globalThis.process.env) {
    return globalThis.process.env[key];
  }
  return undefined;
}

// ── Postgres Wire Protocol Helpers ──────────────────────────────────

const TEXT_ENCODER = new TextEncoder();
const TEXT_DECODER = new TextDecoder();

/** Build a Postgres v3.0 startup message. */
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

/** Build a Postgres simple query message. */
function buildQueryMessage(sql) {
  const sqlBytes = TEXT_ENCODER.encode(sql + "\0");
  const len = 4 + sqlBytes.length;
  const buf = new ArrayBuffer(1 + len);
  const view = new DataView(buf);
  view.setUint8(0, 0x51); // 'Q'
  view.setInt32(1, len);
  new Uint8Array(buf).set(sqlBytes, 5);
  return new Uint8Array(buf);
}

/** Parse Postgres response messages (simplified — handles DataRow and ReadyForQuery). */
function parseResponse(data) {
  const rows = [];
  let offset = 0;

  while (offset < data.length) {
    if (offset + 5 > data.length) break;

    const msgType = String.fromCharCode(data[offset]);
    const view = new DataView(data.buffer, data.byteOffset + offset + 1, 4);
    const msgLen = view.getInt32(0);

    if (msgType === "D") {
      // DataRow
      const row = parseDataRow(data, offset + 5, msgLen - 4);
      rows.push(row);
    } else if (msgType === "Z") {
      // ReadyForQuery — we're done
      break;
    }
    // Skip CommandComplete, RowDescription, etc.

    offset += 1 + msgLen;
  }

  return rows;
}

/** Parse a single DataRow message. */
function parseDataRow(data, start, len) {
  const view = new DataView(data.buffer, data.byteOffset + start, len);
  const fieldCount = view.getInt16(0);
  const fields = [];
  let pos = 2;

  for (let i = 0; i < fieldCount; i++) {
    const fieldLen = view.getInt32(pos);
    pos += 4;
    if (fieldLen === -1) {
      fields.push(null);
    } else {
      const fieldData = data.slice(start + pos, start + pos + fieldLen);
      fields.push(TEXT_DECODER.decode(fieldData));
      pos += fieldLen;
    }
  }

  return fields;
}

// ── Database Connection ─────────────────────────────────────────────

let _dbHandle = null;

async function getDbConnection() {
  if (_dbHandle !== null) return _dbHandle;

  const host = getBunEnv("DB_HOST") ?? "db.test.warp.local";
  const port = parseInt(getBunEnv("DB_PORT") ?? "5432", 10);
  const database = getBunEnv("DB_NAME") ?? "testdb";
  const user = getBunEnv("DB_USER") ?? "testuser";

  _dbHandle = dbConnect({ host, port, database, user });

  // Perform startup handshake
  const startup = buildStartupMessage(database, user);
  dbSend(_dbHandle, startup);

  // Read until ReadyForQuery
  let response = dbRecv(_dbHandle, 4096);
  while (response.length > 0) {
    const lastByte = response[response.length - 1];
    if (response.length >= 6) {
      const possibleZ = response[response.length - 6];
      if (possibleZ === 0x5a) break; // 'Z'
    }
    const more = dbRecv(_dbHandle, 4096);
    if (more.length === 0) break;
    const combined = new Uint8Array(response.length + more.length);
    combined.set(response);
    combined.set(more, response.length);
    response = combined;
  }

  return _dbHandle;
}

async function query(sql) {
  const handle = await getDbConnection();
  const msg = buildQueryMessage(sql);
  dbSend(handle, msg);

  let allData = new Uint8Array(0);
  while (true) {
    const chunk = dbRecv(handle, 65536);
    if (chunk.length === 0) break;

    const combined = new Uint8Array(allData.length + chunk.length);
    combined.set(allData);
    combined.set(chunk, allData.length);
    allData = combined;

    for (let i = allData.length - 6; i >= 0; i--) {
      if (allData[i] === 0x5a) { // 'Z'
        return parseResponse(allData);
      }
    }
  }

  return parseResponse(allData);
}

// ── HTTP Handler ────────────────────────────────────────────────────

function jsonResponse(data, status = 200, extraHeaders = {}) {
  const body = JSON.stringify(data);
  const headers = {
    "Content-Type": "application/json",
    ...extraHeaders,
  };

  const appName = getBunEnv("APP_NAME");
  if (appName) {
    headers["X-App-Name"] = appName;
  }

  return new Response(body, { status, headers });
}

async function handleGetUsers() {
  const rows = await query("SELECT id, name, email FROM test_users ORDER BY id");
  const users = rows.map((row) => ({
    id: parseInt(row[0], 10),
    name: row[1],
    email: row[2],
  }));
  return jsonResponse(users);
}

async function handlePostUsers(request) {
  let body;
  try {
    body = await request.json();
  } catch {
    return jsonResponse({ error: "Invalid JSON" }, 400);
  }

  if (!body.name || !body.email) {
    return jsonResponse({ error: "name and email are required" }, 400);
  }

  const name = body.name.replace(/'/g, "''");
  const email = body.email.replace(/'/g, "''");

  const rows = await query(
    `INSERT INTO test_users (name, email) VALUES ('${name}', '${email}') RETURNING id, name, email`
  );

  if (rows.length > 0) {
    const user = {
      id: parseInt(rows[0][0], 10),
      name: rows[0][1],
      email: rows[0][2],
    };
    return jsonResponse(user, 201);
  }

  return jsonResponse({ error: "Insert failed" }, 500);
}

function handleHealth() {
  return jsonResponse({ status: "ok" });
}

// ── Fetch Event Listener ────────────────────────────────────────────

addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  const method = event.request.method;

  let responsePromise;

  if (url.pathname === "/users" && method === "GET") {
    responsePromise = handleGetUsers();
  } else if (url.pathname === "/users" && method === "POST") {
    responsePromise = handlePostUsers(event.request);
  } else if (url.pathname === "/health") {
    responsePromise = Promise.resolve(handleHealth());
  } else {
    responsePromise = Promise.resolve(
      jsonResponse({ error: "Not Found" }, 404)
    );
  }

  event.respondWith(responsePromise);
});
