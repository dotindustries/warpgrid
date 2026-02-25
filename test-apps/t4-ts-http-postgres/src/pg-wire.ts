/**
 * Postgres wire protocol encoder/decoder.
 *
 * Implements the frontend (client → server) message encoding and
 * backend (server → client) message parsing for the Postgres v3 protocol.
 *
 * This module is pure TypeScript with no external dependencies — it operates
 * on raw byte buffers (Uint8Array) and can run in any JavaScript environment
 * including ComponentizeJS's SpiderMonkey runtime.
 *
 * Reference: https://www.postgresql.org/docs/current/protocol-message-formats.html
 */

// ── Backend Message Types ────────────────────────────────────────────────────

export const enum BackendMessageType {
  Authentication = "Authentication",
  ParameterStatus = "ParameterStatus",
  BackendKeyData = "BackendKeyData",
  ReadyForQuery = "ReadyForQuery",
  RowDescription = "RowDescription",
  DataRow = "DataRow",
  CommandComplete = "CommandComplete",
  ErrorResponse = "ErrorResponse",
  ParseComplete = "ParseComplete",
  BindComplete = "BindComplete",
  NoData = "NoData",
  Unknown = "Unknown",
}

// ── Backend Message Interfaces ───────────────────────────────────────────────

export interface AuthenticationMessage {
  type: BackendMessageType.Authentication;
  authType: number;
  salt?: Uint8Array;
}

export interface ParameterStatusMessage {
  type: BackendMessageType.ParameterStatus;
  key: string;
  value: string;
}

export interface ReadyForQueryMessage {
  type: BackendMessageType.ReadyForQuery;
  status: string; // 'I' (idle), 'T' (in transaction), 'E' (error)
}

export interface FieldDescription {
  name: string;
  tableOid: number;
  columnNumber: number;
  typeOid: number;
  typeLength: number;
  typeModifier: number;
  format: number;
}

export interface RowDescriptionMessage {
  type: BackendMessageType.RowDescription;
  fields: FieldDescription[];
}

export interface DataRowMessage {
  type: BackendMessageType.DataRow;
  values: Array<Uint8Array | null>;
}

export interface CommandCompleteMessage {
  type: BackendMessageType.CommandComplete;
  tag: string;
}

export interface ErrorResponseMessage {
  type: BackendMessageType.ErrorResponse;
  severity: string;
  code: string;
  message: string;
  detail?: string;
}

export interface ParseCompleteMessage {
  type: BackendMessageType.ParseComplete;
}

export interface BindCompleteMessage {
  type: BackendMessageType.BindComplete;
}

export interface UnknownMessage {
  type: BackendMessageType.Unknown;
  typeCode: number;
}

export type BackendMessage =
  | AuthenticationMessage
  | ParameterStatusMessage
  | ReadyForQueryMessage
  | RowDescriptionMessage
  | DataRowMessage
  | CommandCompleteMessage
  | ErrorResponseMessage
  | ParseCompleteMessage
  | BindCompleteMessage
  | UnknownMessage;

// ── Text Encoder/Decoder ─────────────────────────────────────────────────────

const encoder = new TextEncoder();
const decoder = new TextDecoder();

// ── Frontend (Client → Server) Encoding ──────────────────────────────────────

/**
 * Encode a Postgres startup message (no type byte — special format).
 *
 * Format: int32 length, int32 protocol(196608), key=value pairs, \0
 */
export function encodeStartup(user: string, database: string): Uint8Array {
  const params = `user\0${user}\0database\0${database}\0`;
  const paramsBytes = encoder.encode(params);

  // length(4) + protocol(4) + params + trailing null
  const totalLength = 4 + 4 + paramsBytes.byteLength + 1;
  const buf = new Uint8Array(totalLength);
  const view = new DataView(buf.buffer);

  view.setInt32(0, totalLength);  // length includes self
  view.setInt32(4, 196608);       // protocol version 3.0
  buf.set(paramsBytes, 8);
  buf[totalLength - 1] = 0;      // trailing null terminator

  return buf;
}

/**
 * Encode a Simple Query message.
 *
 * Format: 'Q' + int32 length + string sql + \0
 */
export function encodeSimpleQuery(sql: string): Uint8Array {
  const sqlBytes = encoder.encode(sql);
  const payloadLen = 4 + sqlBytes.byteLength + 1; // length field + sql + null
  const buf = new Uint8Array(1 + payloadLen);
  const view = new DataView(buf.buffer);

  buf[0] = 0x51; // 'Q'
  view.setInt32(1, payloadLen);
  buf.set(sqlBytes, 5);
  buf[5 + sqlBytes.byteLength] = 0;

  return buf;
}

/**
 * Encode an Extended Query sequence: Parse + Bind + Execute + Sync.
 *
 * Uses unnamed prepared statement and unnamed portal for simplicity.
 * All parameters are sent as text format.
 */
export function encodeExtendedQuery(sql: string, params: Array<string | null>): Uint8Array {
  const parseMsg = encodeParse(sql, params.length);
  const bindMsg = encodeBind(params);
  const executeMsg = encodeExecute();
  const syncMsg = encodeSync();

  const total = parseMsg.byteLength + bindMsg.byteLength + executeMsg.byteLength + syncMsg.byteLength;
  const buf = new Uint8Array(total);
  let offset = 0;

  buf.set(parseMsg, offset);
  offset += parseMsg.byteLength;
  buf.set(bindMsg, offset);
  offset += bindMsg.byteLength;
  buf.set(executeMsg, offset);
  offset += executeMsg.byteLength;
  buf.set(syncMsg, offset);

  return buf;
}

/**
 * Encode a Terminate message.
 *
 * Format: 'X' + int32 length(4)
 */
export function encodeTerminate(): Uint8Array {
  const buf = new Uint8Array(5);
  const view = new DataView(buf.buffer);
  buf[0] = 0x58; // 'X'
  view.setInt32(1, 4);
  return buf;
}

/**
 * Encode a PasswordMessage (cleartext).
 *
 * Format: 'p' + int32 length + string password + \0
 */
export function encodePasswordMessage(password: string): Uint8Array {
  const passBytes = encoder.encode(password);
  const payloadLen = 4 + passBytes.byteLength + 1;
  const buf = new Uint8Array(1 + payloadLen);
  const view = new DataView(buf.buffer);

  buf[0] = 0x70; // 'p'
  view.setInt32(1, payloadLen);
  buf.set(passBytes, 5);
  buf[5 + passBytes.byteLength] = 0;

  return buf;
}

// ── Internal Encoding Helpers ────────────────────────────────────────────────

/**
 * Encode a Parse message (part of Extended Query).
 *
 * Format: 'P' + int32 length + string stmt_name + \0 + string sql + \0
 *         + int16 num_param_types + int32[] param_oids
 */
function encodeParse(sql: string, numParams: number): Uint8Array {
  const sqlBytes = encoder.encode(sql);
  // stmt_name = "" (unnamed), 1 null + sql + 1 null + 2 (num_params) + 4*numParams (all 0 = unspecified)
  const payloadLen = 4 + 1 + sqlBytes.byteLength + 1 + 2 + 4 * numParams;
  const buf = new Uint8Array(1 + payloadLen);
  const view = new DataView(buf.buffer);

  buf[0] = 0x50; // 'P'
  view.setInt32(1, payloadLen);

  let off = 5;
  buf[off++] = 0; // unnamed statement (empty string + null)
  buf.set(sqlBytes, off);
  off += sqlBytes.byteLength;
  buf[off++] = 0; // null terminator for SQL

  view.setInt16(off, numParams);
  off += 2;
  // Parameter type OIDs: all 0 (let server infer types)
  for (let i = 0; i < numParams; i++) {
    view.setInt32(off, 0);
    off += 4;
  }

  return buf;
}

/**
 * Encode a Bind message (part of Extended Query).
 *
 * Format: 'B' + int32 length + string portal + \0 + string stmt + \0
 *         + int16 num_formats + int16[] formats
 *         + int16 num_params + (int32 len + bytes)[] param_values
 *         + int16 num_result_formats + int16[] result_formats
 */
function encodeBind(params: Array<string | null>): Uint8Array {
  // Calculate total size
  let paramDataLen = 0;
  const encodedParams: Array<Uint8Array | null> = [];

  for (const p of params) {
    if (p === null) {
      encodedParams.push(null);
      paramDataLen += 4; // just the -1 length
    } else {
      const bytes = encoder.encode(p);
      encodedParams.push(bytes);
      paramDataLen += 4 + bytes.byteLength; // length + data
    }
  }

  // portal("" + \0) + stmt("" + \0) + num_formats(2) + num_params(2) + paramData
  // + num_result_formats(2) = 1+1+2+2+paramData+2 = 8 + paramData
  const payloadLen = 4 + 1 + 1 + 2 + 2 + paramDataLen + 2;
  const buf = new Uint8Array(1 + payloadLen);
  const view = new DataView(buf.buffer);

  buf[0] = 0x42; // 'B'
  view.setInt32(1, payloadLen);

  let off = 5;
  buf[off++] = 0; // unnamed portal
  buf[off++] = 0; // unnamed statement

  // Parameter format codes: 0 = all text (specify 0 format codes → use default text)
  view.setInt16(off, 0);
  off += 2;

  // Number of parameters
  view.setInt16(off, params.length);
  off += 2;

  // Parameter values
  for (const ep of encodedParams) {
    if (ep === null) {
      view.setInt32(off, -1); // NULL
      off += 4;
    } else {
      view.setInt32(off, ep.byteLength);
      off += 4;
      buf.set(ep, off);
      off += ep.byteLength;
    }
  }

  // Result format codes: 0 = all text
  view.setInt16(off, 0);

  return buf;
}

/**
 * Encode an Execute message (part of Extended Query).
 *
 * Format: 'E' + int32 length + string portal + \0 + int32 max_rows(0 = unlimited)
 */
function encodeExecute(): Uint8Array {
  const buf = new Uint8Array(1 + 4 + 1 + 4); // type + length + portal(\0) + max_rows
  const view = new DataView(buf.buffer);

  buf[0] = 0x45; // 'E'
  view.setInt32(1, 4 + 1 + 4); // length
  buf[5] = 0;                   // unnamed portal
  view.setInt32(6, 0);          // max_rows = 0 (unlimited)

  return buf;
}

/**
 * Encode a Sync message (part of Extended Query).
 *
 * Format: 'S' + int32 length(4)
 */
function encodeSync(): Uint8Array {
  const buf = new Uint8Array(5);
  const view = new DataView(buf.buffer);
  buf[0] = 0x53; // 'S'
  view.setInt32(1, 4);
  return buf;
}

// ── Backend (Server → Client) Parsing ────────────────────────────────────────

/**
 * Parse a buffer of backend messages into typed message objects.
 *
 * Handles concatenated messages (multiple messages in a single buffer).
 * Unknown message types are included as UnknownMessage rather than causing errors.
 */
export function parseBackendMessages(buf: Uint8Array): BackendMessage[] {
  const messages: BackendMessage[] = [];
  const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  let offset = 0;

  while (offset < buf.byteLength) {
    if (offset + 5 > buf.byteLength) break; // need at least type + length

    const typeCode = buf[offset];
    const length = view.getInt32(offset + 1); // length includes self (4 bytes)
    const msgEnd = offset + 1 + length;

    if (msgEnd > buf.byteLength) break; // incomplete message

    const payload = buf.subarray(offset + 5, msgEnd);
    const payloadView = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);

    const msg = parseOneMessage(typeCode, payload, payloadView);
    messages.push(msg);

    offset = msgEnd;
  }

  return messages;
}

function parseOneMessage(
  typeCode: number,
  payload: Uint8Array,
  payloadView: DataView
): BackendMessage {
  switch (typeCode) {
    case 0x52: // 'R' — Authentication
      return parseAuthentication(payload, payloadView);
    case 0x53: // 'S' — ParameterStatus
      return parseParameterStatus(payload);
    case 0x4b: // 'K' — BackendKeyData
      return { type: BackendMessageType.Unknown, typeCode };
    case 0x5a: // 'Z' — ReadyForQuery
      return { type: BackendMessageType.ReadyForQuery, status: String.fromCharCode(payload[0]) };
    case 0x54: // 'T' — RowDescription
      return parseRowDescription(payload, payloadView);
    case 0x44: // 'D' — DataRow
      return parseDataRow(payload, payloadView);
    case 0x43: // 'C' — CommandComplete
      return parseCommandComplete(payload);
    case 0x45: // 'E' — ErrorResponse
      return parseErrorResponse(payload);
    case 0x31: // '1' — ParseComplete
      return { type: BackendMessageType.ParseComplete };
    case 0x32: // '2' — BindComplete
      return { type: BackendMessageType.BindComplete };
    case 0x6e: // 'n' — NoData
      return { type: BackendMessageType.Unknown, typeCode };
    default:
      return { type: BackendMessageType.Unknown, typeCode };
  }
}

function parseAuthentication(payload: Uint8Array, view: DataView): AuthenticationMessage {
  const authType = view.getInt32(0);
  const msg: AuthenticationMessage = {
    type: BackendMessageType.Authentication,
    authType,
  };

  // MD5 auth includes a 4-byte salt
  if (authType === 5 && payload.byteLength >= 8) {
    msg.salt = payload.subarray(4, 8);
  }

  return msg;
}

function parseParameterStatus(payload: Uint8Array): ParameterStatusMessage {
  const str = decoder.decode(payload);
  const nullIdx = str.indexOf("\0");
  const key = str.substring(0, nullIdx);
  const value = str.substring(nullIdx + 1, str.length - 1); // strip trailing null

  return { type: BackendMessageType.ParameterStatus, key, value };
}

function parseRowDescription(payload: Uint8Array, view: DataView): RowDescriptionMessage {
  const numFields = view.getInt16(0);
  const fields: FieldDescription[] = [];
  let off = 2;

  for (let i = 0; i < numFields; i++) {
    // Read null-terminated name
    const nameStart = off;
    while (off < payload.byteLength && payload[off] !== 0) off++;
    const name = decoder.decode(payload.subarray(nameStart, off));
    off++; // skip null terminator

    const fieldView = new DataView(payload.buffer, payload.byteOffset + off, 18);
    fields.push({
      name,
      tableOid: fieldView.getInt32(0),
      columnNumber: fieldView.getInt16(4),
      typeOid: fieldView.getInt32(6),
      typeLength: fieldView.getInt16(10),
      typeModifier: fieldView.getInt32(12),
      format: fieldView.getInt16(16),
    });
    off += 18;
  }

  return { type: BackendMessageType.RowDescription, fields };
}

function parseDataRow(payload: Uint8Array, view: DataView): DataRowMessage {
  const numCols = view.getInt16(0);
  const values: Array<Uint8Array | null> = [];
  let off = 2;

  for (let i = 0; i < numCols; i++) {
    const colView = new DataView(payload.buffer, payload.byteOffset + off, 4);
    const len = colView.getInt32(0);
    off += 4;

    if (len === -1) {
      values.push(null);
    } else {
      values.push(new Uint8Array(payload.buffer, payload.byteOffset + off, len));
      off += len;
    }
  }

  return { type: BackendMessageType.DataRow, values };
}

function parseCommandComplete(payload: Uint8Array): CommandCompleteMessage {
  // Tag is null-terminated
  const str = decoder.decode(payload);
  const tag = str.substring(0, str.indexOf("\0"));
  return { type: BackendMessageType.CommandComplete, tag };
}

function parseErrorResponse(payload: Uint8Array): ErrorResponseMessage {
  const fields: Record<string, string> = {};
  let off = 0;

  while (off < payload.byteLength && payload[off] !== 0) {
    const fieldCode = String.fromCharCode(payload[off]);
    off++;

    const valueStart = off;
    while (off < payload.byteLength && payload[off] !== 0) off++;
    const value = decoder.decode(payload.subarray(valueStart, off));
    off++; // skip null terminator

    fields[fieldCode] = value;
  }

  return {
    type: BackendMessageType.ErrorResponse,
    severity: fields["S"] ?? "ERROR",
    code: fields["C"] ?? "00000",
    message: fields["M"] ?? "unknown error",
    detail: fields["D"],
  };
}
