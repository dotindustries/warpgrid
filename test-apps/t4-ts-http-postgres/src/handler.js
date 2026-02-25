/**
 * WarpGrid T4 test fixture: TypeScript HTTP handler with Postgres connectivity.
 *
 * This handler is componentized by jco into a WASI HTTP Wasm component.
 * It imports the WarpGrid database proxy WIT interface for raw Postgres
 * wire protocol access and wraps it in a pg.Client-compatible API.
 *
 * Routes:
 *   GET  /users       — list all users as JSON
 *   GET  /users/:id   — get user by ID
 *   POST /users       — create a new user (body: { "name": "..." })
 *
 * Build: npm run build (or: scripts/build.sh)
 * Compile: jco componentize src/handler.js --wit wit/ --world-name handler ...
 */

// ── WIT Imports ──────────────────────────────────────────────────────────────
// These resolve to host-provided functions when running as a Wasm component.
// The jco componentize tool links them to the warpgrid:shim/database-proxy WIT.

import {
  connect as dbConnect,
  send as dbSend,
  recv as dbRecv,
  close as dbClose,
} from "warpgrid:shim/database-proxy@0.1.0";

// ── Postgres Wire Protocol (inline for single-file componentization) ─────────

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

function pgEncodeStartup(user, database) {
  const params = `user\0${user}\0database\0${database}\0`;
  const paramsBytes = textEncoder.encode(params);
  const totalLength = 4 + 4 + paramsBytes.byteLength + 1;
  const buf = new Uint8Array(totalLength);
  const view = new DataView(buf.buffer);
  view.setInt32(0, totalLength);
  view.setInt32(4, 196608);
  buf.set(paramsBytes, 8);
  buf[totalLength - 1] = 0;
  return buf;
}

function pgEncodeSimpleQuery(sql) {
  const sqlBytes = textEncoder.encode(sql);
  const payloadLen = 4 + sqlBytes.byteLength + 1;
  const buf = new Uint8Array(1 + payloadLen);
  const view = new DataView(buf.buffer);
  buf[0] = 0x51;
  view.setInt32(1, payloadLen);
  buf.set(sqlBytes, 5);
  buf[5 + sqlBytes.byteLength] = 0;
  return buf;
}

function pgEncodeExtendedQuery(sql, params) {
  // Parse message
  const sqlBytes = textEncoder.encode(sql);
  const parsePayload = 4 + 1 + sqlBytes.byteLength + 1 + 2 + 4 * params.length;
  const parseBuf = new Uint8Array(1 + parsePayload);
  let pv = new DataView(parseBuf.buffer);
  parseBuf[0] = 0x50;
  pv.setInt32(1, parsePayload);
  let off = 5;
  parseBuf[off++] = 0;
  parseBuf.set(sqlBytes, off);
  off += sqlBytes.byteLength;
  parseBuf[off++] = 0;
  pv.setInt16(off, params.length);
  off += 2;
  for (let i = 0; i < params.length; i++) { pv.setInt32(off, 0); off += 4; }

  // Bind message
  const encodedParams = params.map((p) =>
    p === null ? null : textEncoder.encode(String(p))
  );
  let paramDataLen = 0;
  for (const ep of encodedParams) paramDataLen += 4 + (ep ? ep.byteLength : 0);
  const bindPayload = 4 + 1 + 1 + 2 + 2 + paramDataLen + 2;
  const bindBuf = new Uint8Array(1 + bindPayload);
  let bv = new DataView(bindBuf.buffer);
  bindBuf[0] = 0x42;
  bv.setInt32(1, bindPayload);
  off = 5;
  bindBuf[off++] = 0;
  bindBuf[off++] = 0;
  bv.setInt16(off, 0); off += 2;
  bv.setInt16(off, params.length); off += 2;
  for (const ep of encodedParams) {
    if (ep === null) { bv.setInt32(off, -1); off += 4; }
    else { bv.setInt32(off, ep.byteLength); off += 4; bindBuf.set(ep, off); off += ep.byteLength; }
  }
  bv.setInt16(off, 0);

  // Execute message
  const execBuf = new Uint8Array(10);
  const ev = new DataView(execBuf.buffer);
  execBuf[0] = 0x45;
  ev.setInt32(1, 9);
  execBuf[5] = 0;
  ev.setInt32(6, 0);

  // Sync message
  const syncBuf = new Uint8Array(5);
  const sv = new DataView(syncBuf.buffer);
  syncBuf[0] = 0x53;
  sv.setInt32(1, 4);

  const total = parseBuf.byteLength + bindBuf.byteLength + execBuf.byteLength + syncBuf.byteLength;
  const result = new Uint8Array(total);
  let pos = 0;
  result.set(parseBuf, pos); pos += parseBuf.byteLength;
  result.set(bindBuf, pos); pos += bindBuf.byteLength;
  result.set(execBuf, pos); pos += execBuf.byteLength;
  result.set(syncBuf, pos);
  return result;
}

function pgEncodeTerminate() {
  const buf = new Uint8Array(5);
  const view = new DataView(buf.buffer);
  buf[0] = 0x58;
  view.setInt32(1, 4);
  return buf;
}

function pgEncodePassword(password) {
  const passBytes = textEncoder.encode(password);
  const payloadLen = 4 + passBytes.byteLength + 1;
  const buf = new Uint8Array(1 + payloadLen);
  const view = new DataView(buf.buffer);
  buf[0] = 0x70;
  view.setInt32(1, payloadLen);
  buf.set(passBytes, 5);
  buf[5 + passBytes.byteLength] = 0;
  return buf;
}

function pgParseMessages(buf) {
  const messages = [];
  const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  let offset = 0;

  while (offset < buf.byteLength) {
    if (offset + 5 > buf.byteLength) break;
    const typeCode = buf[offset];
    const length = view.getInt32(offset + 1);
    const msgEnd = offset + 1 + length;
    if (msgEnd > buf.byteLength) break;
    const payload = buf.subarray(offset + 5, msgEnd);
    const pv = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);

    switch (typeCode) {
      case 0x52: { // Authentication
        const authType = pv.getInt32(0);
        messages.push({ type: "auth", authType });
        break;
      }
      case 0x5a: // ReadyForQuery
        messages.push({ type: "ready", status: String.fromCharCode(payload[0]) });
        break;
      case 0x54: { // RowDescription
        const numFields = pv.getInt16(0);
        const fields = [];
        let fo = 2;
        for (let i = 0; i < numFields; i++) {
          const ns = fo;
          while (fo < payload.byteLength && payload[fo] !== 0) fo++;
          const name = textDecoder.decode(payload.subarray(ns, fo));
          fo++;
          const fv = new DataView(payload.buffer, payload.byteOffset + fo, 18);
          fields.push({ name, typeOid: fv.getInt32(6) });
          fo += 18;
        }
        messages.push({ type: "rowDesc", fields });
        break;
      }
      case 0x44: { // DataRow
        const numCols = pv.getInt16(0);
        const values = [];
        let ro = 2;
        for (let i = 0; i < numCols; i++) {
          const cv = new DataView(payload.buffer, payload.byteOffset + ro, 4);
          const len = cv.getInt32(0);
          ro += 4;
          if (len === -1) values.push(null);
          else { values.push(textDecoder.decode(payload.subarray(ro, ro + len))); ro += len; }
        }
        messages.push({ type: "dataRow", values });
        break;
      }
      case 0x43: { // CommandComplete
        const str = textDecoder.decode(payload);
        messages.push({ type: "complete", tag: str.substring(0, str.indexOf("\0")) });
        break;
      }
      case 0x45: { // ErrorResponse
        const fields = {};
        let eo = 0;
        while (eo < payload.byteLength && payload[eo] !== 0) {
          const fc = String.fromCharCode(payload[eo++]);
          const vs = eo;
          while (eo < payload.byteLength && payload[eo] !== 0) eo++;
          fields[fc] = textDecoder.decode(payload.subarray(vs, eo));
          eo++;
        }
        messages.push({ type: "error", severity: fields["S"], code: fields["C"], message: fields["M"] });
        break;
      }
      case 0x31: messages.push({ type: "parseComplete" }); break;
      case 0x32: messages.push({ type: "bindComplete" }); break;
      default: messages.push({ type: "unknown", typeCode }); break;
    }
    offset = msgEnd;
  }
  return messages;
}

// ── Postgres Client ──────────────────────────────────────────────────────────

class PgClient {
  constructor(config) {
    this.config = config;
    this.handle = null;
  }

  connect() {
    this.handle = dbConnect({
      host: this.config.host,
      port: this.config.port,
      database: this.config.database,
      user: this.config.user,
      password: this.config.password,
    });
    this._send(pgEncodeStartup(this.config.user, this.config.database));
    this._processStartup();
  }

  query(sql, params) {
    if (params && params.length > 0) {
      this._send(pgEncodeExtendedQuery(sql, params));
    } else {
      this._send(pgEncodeSimpleQuery(sql));
    }
    return this._readQueryResult();
  }

  end() {
    if (this.handle === null) return;
    try { this._send(pgEncodeTerminate()); } catch { /* ignore */ }
    try { dbClose(this.handle); } catch { /* ignore */ }
    this.handle = null;
  }

  _send(data) {
    dbSend(this.handle, data);
  }

  _recv() {
    return dbRecv(this.handle, 65536);
  }

  _readMessages() {
    const data = this._recv();
    if (!data || data.byteLength === 0) return [];
    return pgParseMessages(data);
  }

  _readUntil(pred) {
    const collected = [];
    for (let i = 0; i < 100; i++) {
      const msgs = this._readMessages();
      for (const m of msgs) {
        collected.push(m);
        if (pred(m)) return collected;
      }
    }
    throw new Error("Timeout waiting for backend message");
  }

  _processStartup() {
    const msgs = this._readUntil((m) => m.type === "ready");
    for (const m of msgs) {
      if (m.type === "auth" && m.authType === 3) {
        if (!this.config.password) throw new Error("Password required");
        this._send(pgEncodePassword(this.config.password));
      } else if (m.type === "error") {
        throw new Error(`Startup error [${m.code}]: ${m.message}`);
      }
    }
  }

  _readQueryResult() {
    const msgs = this._readUntil((m) => m.type === "ready");
    let fields = [];
    const rows = [];
    let rowCount = 0;
    for (const m of msgs) {
      if (m.type === "error") throw new Error(`[${m.code}]: ${m.message}`);
      if (m.type === "rowDesc") fields = m.fields;
      if (m.type === "dataRow") {
        const row = {};
        for (let i = 0; i < m.values.length && i < fields.length; i++) {
          row[fields[i].name] = m.values[i];
        }
        rows.push(row);
      }
      if (m.type === "complete") {
        const parts = m.tag.split(" ");
        const n = parseInt(parts[parts.length - 1], 10);
        rowCount = isNaN(n) ? rows.length : n;
      }
    }
    return { rows, rowCount, fields };
  }
}

// ── Database Configuration ───────────────────────────────────────────────────

const DB_CONFIG = {
  host: "db.test.warp.local",
  port: 5432,
  database: "testdb",
  user: "testuser",
  password: undefined,
};

// ── HTTP Handler ─────────────────────────────────────────────────────────────

function jsonResponse(status, data) {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function withClient(fn) {
  const client = new PgClient(DB_CONFIG);
  try {
    client.connect();
    return fn(client);
  } finally {
    client.end();
  }
}

function handleGetUsers() {
  try {
    return withClient((client) => {
      const result = client.query("SELECT id, name FROM test_users ORDER BY id");
      return jsonResponse(200, result.rows);
    });
  } catch (err) {
    return jsonResponse(500, { error: err.message || String(err) });
  }
}

function handleGetUserById(idStr) {
  const id = parseInt(idStr, 10);
  if (isNaN(id) || id <= 0) {
    return jsonResponse(400, { error: "invalid user ID: must be a positive integer" });
  }
  try {
    return withClient((client) => {
      const result = client.query("SELECT id, name FROM test_users WHERE id = $1", [String(id)]);
      if (result.rows.length === 0) return jsonResponse(404, { error: `user ${id} not found` });
      return jsonResponse(200, result.rows[0]);
    });
  } catch (err) {
    return jsonResponse(500, { error: err.message || String(err) });
  }
}

function handlePostUser(body) {
  if (!body) return jsonResponse(400, { error: "request body is required" });

  let parsed;
  try { parsed = JSON.parse(body); } catch { return jsonResponse(400, { error: "invalid JSON in request body" }); }

  if (typeof parsed !== "object" || parsed === null) return jsonResponse(400, { error: "invalid JSON: expected an object" });

  const name = parsed.name;
  if (typeof name !== "string" || name.trim() === "") return jsonResponse(400, { error: "name field is required and must be a non-empty string" });

  try {
    return withClient((client) => {
      const result = client.query("INSERT INTO test_users (name) VALUES ($1) RETURNING id, name", [name.trim()]);
      if (result.rows.length === 0) return jsonResponse(500, { error: "insert did not return a row" });
      return jsonResponse(201, result.rows[0]);
    });
  } catch (err) {
    return jsonResponse(500, { error: err.message || String(err) });
  }
}

// ── Fetch Event Listener ─────────────────────────────────────────────────────

addEventListener("fetch", (event) => {
  const request = event.request;
  const url = new URL(request.url);
  const path = url.pathname;
  const method = request.method;

  let response;

  if (path === "/users" && method === "GET") {
    response = handleGetUsers();
  } else if (path === "/users" && method === "POST") {
    const body = request.text ? request.text() : null;
    response = handlePostUser(body);
  } else {
    const match = path.match(/^\/users\/([^/]+)$/);
    if (match && method === "GET") {
      response = handleGetUserById(match[1]);
    } else if (path.startsWith("/users")) {
      response = jsonResponse(405, { error: "method not allowed" });
    } else {
      response = jsonResponse(404, { error: "not found" });
    }
  }

  event.respondWith(response);
});
