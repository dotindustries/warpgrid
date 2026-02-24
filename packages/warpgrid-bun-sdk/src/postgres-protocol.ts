/**
 * Postgres v3.0 wire protocol helpers for the Wasm backend.
 *
 * Handles message building (startup, query, extended query) and
 * response parsing (auth, row description, data rows, errors).
 *
 * Reference: https://www.postgresql.org/docs/current/protocol-message-formats.html
 */

const TEXT_ENCODER = new TextEncoder();
const TEXT_DECODER = new TextDecoder();

// ── Message Types ─────────────────────────────────────────────────────

export const MSG = {
  // Frontend (client → server)
  QUERY: 0x51, // 'Q'
  PARSE: 0x50, // 'P'
  BIND: 0x42, // 'B'
  EXECUTE: 0x45, // 'E'
  SYNC: 0x53, // 'S'
  PASSWORD: 0x70, // 'p'
  TERMINATE: 0x58, // 'X'

  // Backend (server → client)
  AUTH: 0x52, // 'R'
  PARAM_STATUS: 0x53, // 'S'
  BACKEND_KEY: 0x4b, // 'K'
  READY_FOR_QUERY: 0x5a, // 'Z'
  ROW_DESCRIPTION: 0x54, // 'T'
  DATA_ROW: 0x44, // 'D'
  COMMAND_COMPLETE: 0x43, // 'C'
  ERROR_RESPONSE: 0x45, // 'E'
  PARSE_COMPLETE: 0x31, // '1'
  BIND_COMPLETE: 0x32, // '2'
  NO_DATA: 0x6e, // 'n'
} as const;

// ── Auth Types ────────────────────────────────────────────────────────

export const AUTH_OK = 0;
export const AUTH_CLEARTEXT = 3;
export const AUTH_MD5 = 5;

// ── Parsed Types ──────────────────────────────────────────────────────

export interface PgMessage {
  type: number;
  length: number;
  payload: Uint8Array;
}

export interface RowDescriptionField {
  name: string;
  tableOID: number;
  columnIndex: number;
  dataTypeOID: number;
  dataTypeSize: number;
  typeModifier: number;
  formatCode: number;
}

export interface AuthResult {
  type: number;
  salt?: Uint8Array;
}

export interface PgError {
  severity: string;
  code: string;
  message: string;
  detail?: string;
  hint?: string;
}

// ── Message Building ──────────────────────────────────────────────────

/** Build a Postgres v3.0 StartupMessage (no type byte). */
export function buildStartupMessage(
  user: string,
  database: string,
): Uint8Array {
  const params = TEXT_ENCODER.encode(
    `user\0${user}\0database\0${database}\0\0`,
  );
  const length = 4 + 4 + params.length;
  const buf = new ArrayBuffer(length);
  const view = new DataView(buf);
  view.setInt32(0, length); // Length includes self
  view.setInt32(4, 196608); // Protocol version 3.0
  new Uint8Array(buf).set(params, 8);
  return new Uint8Array(buf);
}

/** Build a PasswordMessage for cleartext auth. */
export function buildCleartextPasswordMessage(password: string): Uint8Array {
  const passwordBytes = TEXT_ENCODER.encode(password + "\0");
  const length = 4 + passwordBytes.length;
  const buf = new ArrayBuffer(1 + length);
  const view = new DataView(buf);
  view.setUint8(0, MSG.PASSWORD);
  view.setInt32(1, length);
  new Uint8Array(buf).set(passwordBytes, 5);
  return new Uint8Array(buf);
}

/**
 * Build a PasswordMessage for MD5 auth.
 * MD5 password = "md5" + md5(md5(password + user) + salt)
 */
export async function buildMD5PasswordMessage(
  user: string,
  password: string,
  salt: Uint8Array,
): Promise<Uint8Array> {
  const inner = await md5hex(TEXT_ENCODER.encode(password + user));
  const outerInput = new Uint8Array(inner.length + salt.length);
  outerInput.set(TEXT_ENCODER.encode(inner));
  outerInput.set(salt, inner.length);
  const outer = await md5hex(outerInput);
  return buildCleartextPasswordMessage("md5" + outer);
}

/** Build a Simple Query message ('Q'). */
export function buildSimpleQuery(sql: string): Uint8Array {
  const sqlBytes = TEXT_ENCODER.encode(sql + "\0");
  const length = 4 + sqlBytes.length;
  const buf = new ArrayBuffer(1 + length);
  const view = new DataView(buf);
  view.setUint8(0, MSG.QUERY);
  view.setInt32(1, length);
  new Uint8Array(buf).set(sqlBytes, 5);
  return new Uint8Array(buf);
}

/**
 * Build an Extended Query message batch (Parse + Bind + Execute + Sync).
 * Parameters are sent as text format.
 */
export function buildExtendedQuery(
  sql: string,
  params: unknown[],
): Uint8Array {
  const parts: Uint8Array[] = [
    buildParseMessage(sql, params.length),
    buildBindMessage(params),
    buildExecuteMessage(),
    buildSyncMessage(),
  ];

  const totalLength = parts.reduce((sum, p) => sum + p.length, 0);
  const result = new Uint8Array(totalLength);
  let offset = 0;
  for (const part of parts) {
    result.set(part, offset);
    offset += part.length;
  }
  return result;
}

/** Build a Terminate message ('X'). */
export function buildTerminateMessage(): Uint8Array {
  const buf = new ArrayBuffer(5);
  const view = new DataView(buf);
  view.setUint8(0, MSG.TERMINATE);
  view.setInt32(1, 4);
  return new Uint8Array(buf);
}

// ── Internal Builders ─────────────────────────────────────────────────

function buildParseMessage(sql: string, paramCount: number): Uint8Array {
  const sqlBytes = TEXT_ENCODER.encode(sql + "\0");
  // unnamed statement: empty string + null terminator = 1 byte
  const length = 4 + 1 + sqlBytes.length + 2 + paramCount * 4;
  const buf = new ArrayBuffer(1 + length);
  const view = new DataView(buf);
  let offset = 0;
  view.setUint8(offset, MSG.PARSE);
  offset += 1;
  view.setInt32(offset, length);
  offset += 4;
  view.setUint8(offset, 0); // unnamed statement
  offset += 1;
  new Uint8Array(buf).set(sqlBytes, offset);
  offset += sqlBytes.length;
  view.setInt16(offset, paramCount);
  offset += 2;
  // All param types = 0 (let server infer)
  for (let i = 0; i < paramCount; i++) {
    view.setInt32(offset, 0);
    offset += 4;
  }
  return new Uint8Array(buf);
}

function buildBindMessage(params: unknown[]): Uint8Array {
  // Serialize all params as text
  const serialized: (Uint8Array | null)[] = params.map((p) => {
    if (p === null || p === undefined) return null;
    if (p instanceof Uint8Array) return p;
    const str = typeof p === "object" ? JSON.stringify(p) : String(p);
    return TEXT_ENCODER.encode(str);
  });

  // Calculate length
  let bodyLen =
    1 + // portal name (empty, null-terminated)
    1 + // statement name (empty, null-terminated)
    2 + // format codes count (0 = all text)
    2; // param count

  for (const s of serialized) {
    bodyLen += 4; // length field
    if (s !== null) bodyLen += s.length;
  }
  bodyLen += 2; // result format codes count (0 = all text)

  const length = 4 + bodyLen;
  const buf = new ArrayBuffer(1 + length);
  const view = new DataView(buf);
  let offset = 0;

  view.setUint8(offset, MSG.BIND);
  offset += 1;
  view.setInt32(offset, length);
  offset += 4;
  view.setUint8(offset, 0); // portal name
  offset += 1;
  view.setUint8(offset, 0); // statement name
  offset += 1;
  view.setInt16(offset, 0); // format codes count (all text)
  offset += 2;
  view.setInt16(offset, serialized.length);
  offset += 2;

  for (const s of serialized) {
    if (s === null) {
      view.setInt32(offset, -1); // NULL
      offset += 4;
    } else {
      view.setInt32(offset, s.length);
      offset += 4;
      new Uint8Array(buf).set(s, offset);
      offset += s.length;
    }
  }

  view.setInt16(offset, 0); // result format codes (all text)
  return new Uint8Array(buf);
}

function buildExecuteMessage(): Uint8Array {
  const buf = new ArrayBuffer(1 + 4 + 1 + 4);
  const view = new DataView(buf);
  view.setUint8(0, MSG.EXECUTE);
  view.setInt32(1, 4 + 1 + 4); // length
  view.setUint8(5, 0); // portal name (empty, null-terminated)
  view.setInt32(6, 0); // max rows (0 = all)
  return new Uint8Array(buf);
}

function buildSyncMessage(): Uint8Array {
  const buf = new ArrayBuffer(5);
  const view = new DataView(buf);
  view.setUint8(0, MSG.SYNC);
  view.setInt32(1, 4);
  return new Uint8Array(buf);
}

// ── Message Parsing ───────────────────────────────────────────────────

/** Parse raw bytes into individual Postgres backend messages. */
export function parseMessages(data: Uint8Array): PgMessage[] {
  const messages: PgMessage[] = [];
  let offset = 0;

  while (offset + 5 <= data.length) {
    const type = data[offset];
    const view = new DataView(data.buffer, data.byteOffset + offset + 1, 4);
    const length = view.getInt32(0);

    if (offset + 1 + length > data.length) break; // incomplete message

    const payload = data.slice(offset + 5, offset + 1 + length);
    messages.push({ type, length, payload });
    offset += 1 + length;
  }

  return messages;
}

/** Parse an Authentication response payload. */
export function parseAuthResponse(payload: Uint8Array): AuthResult {
  const view = new DataView(
    payload.buffer,
    payload.byteOffset,
    payload.length,
  );
  const authType = view.getInt32(0);

  if (authType === AUTH_MD5 && payload.length >= 8) {
    return { type: authType, salt: payload.slice(4, 8) };
  }

  return { type: authType };
}

/** Parse a RowDescription payload into field metadata. */
export function parseRowDescription(payload: Uint8Array): RowDescriptionField[] {
  const view = new DataView(
    payload.buffer,
    payload.byteOffset,
    payload.length,
  );
  const fieldCount = view.getInt16(0);
  const fields: RowDescriptionField[] = [];
  let offset = 2;

  for (let i = 0; i < fieldCount; i++) {
    // Read null-terminated field name
    let nameEnd = offset;
    while (nameEnd < payload.length && payload[nameEnd] !== 0) nameEnd++;
    const name = TEXT_DECODER.decode(payload.slice(offset, nameEnd));
    offset = nameEnd + 1; // skip null

    const fieldView = new DataView(
      payload.buffer,
      payload.byteOffset + offset,
      18,
    );
    fields.push({
      name,
      tableOID: fieldView.getInt32(0),
      columnIndex: fieldView.getInt16(4),
      dataTypeOID: fieldView.getInt32(6),
      dataTypeSize: fieldView.getInt16(10),
      typeModifier: fieldView.getInt32(12),
      formatCode: fieldView.getInt16(16),
    });
    offset += 18;
  }

  return fields;
}

/** Parse a DataRow payload into an array of string|null values. */
export function parseDataRow(payload: Uint8Array): (string | null)[] {
  const view = new DataView(
    payload.buffer,
    payload.byteOffset,
    payload.length,
  );
  const fieldCount = view.getInt16(0);
  const fields: (string | null)[] = [];
  let offset = 2;

  for (let i = 0; i < fieldCount; i++) {
    const fieldLen = view.getInt32(offset);
    offset += 4;
    if (fieldLen === -1) {
      fields.push(null);
    } else {
      fields.push(
        TEXT_DECODER.decode(payload.slice(offset, offset + fieldLen)),
      );
      offset += fieldLen;
    }
  }

  return fields;
}

/** Parse an ErrorResponse payload into structured error fields. */
export function parseErrorResponse(payload: Uint8Array): PgError {
  const result: Record<string, string> = {};
  let offset = 0;

  while (offset < payload.length) {
    const fieldType = payload[offset];
    if (fieldType === 0) break; // terminator
    offset += 1;

    let valueEnd = offset;
    while (valueEnd < payload.length && payload[valueEnd] !== 0) valueEnd++;
    const value = TEXT_DECODER.decode(payload.slice(offset, valueEnd));
    offset = valueEnd + 1;

    const key = String.fromCharCode(fieldType);
    result[key] = value;
  }

  return {
    severity: result["S"] ?? "ERROR",
    code: result["C"] ?? "00000",
    message: result["M"] ?? "Unknown error",
    detail: result["D"],
    hint: result["H"],
  };
}

/** Parse a CommandComplete tag to extract row count. */
export function parseCommandComplete(payload: Uint8Array): number {
  const tag = TEXT_DECODER.decode(payload).replace(/\0$/, "");
  // Formats: "SELECT 5", "INSERT 0 3", "UPDATE 2", "DELETE 1"
  const parts = tag.split(" ");
  const last = parts[parts.length - 1];
  const count = parseInt(last, 10);
  return isNaN(count) ? 0 : count;
}

// ── Utility ───────────────────────────────────────────────────────────

async function md5hex(data: Uint8Array): Promise<string> {
  // Use Web Crypto API (available in both Bun and Wasm runtimes)
  const buf = new ArrayBuffer(data.byteLength);
  new Uint8Array(buf).set(data);
  const hashBuffer = await crypto.subtle.digest("MD5", buf);
  const hashArray = new Uint8Array(hashBuffer);
  return Array.from(hashArray)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}
