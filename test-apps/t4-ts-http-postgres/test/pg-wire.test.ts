/**
 * Unit tests for the Postgres wire protocol encoder/decoder.
 *
 * Tests the pure encoding/decoding functions that convert between
 * JavaScript values and raw Postgres wire protocol byte buffers.
 * No WIT dependency — runs in plain Node.js.
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  encodeStartup,
  encodeSimpleQuery,
  encodeExtendedQuery,
  encodeTerminate,
  encodePasswordMessage,
  parseBackendMessages,
  BackendMessageType,
  type BackendMessage,
  type RowDescriptionMessage,
  type DataRowMessage,
  type CommandCompleteMessage,
  type ErrorResponseMessage,
  type AuthenticationMessage,
  type ReadyForQueryMessage,
} from "../src/pg-wire.js";

// ── Encoding Tests ───────────────────────────────────────────────────────────

describe("encodeStartup", () => {
  it("produces a valid startup message with user and database", () => {
    const buf = encodeStartup("testuser", "testdb");

    // Startup message: int32 length, int32 protocol(196608), ...params..., \0
    const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
    const length = view.getInt32(0);
    assert.equal(length, buf.byteLength, "length field matches buffer size");

    const protocol = view.getInt32(4);
    assert.equal(protocol, 196608, "protocol version is 3.0 (196608)");

    // Decode the parameter key-value pairs after the 8-byte header
    const paramsStr = new TextDecoder().decode(buf.subarray(8, buf.byteLength - 1));
    const parts = paramsStr.split("\0");
    assert.ok(parts.includes("user"), "contains 'user' key");
    assert.ok(parts.includes("testuser"), "contains user value");
    assert.ok(parts.includes("database"), "contains 'database' key");
    assert.ok(parts.includes("testdb"), "contains database value");

    // Must end with double null (parameters terminator + message terminator)
    assert.equal(buf[buf.byteLength - 1], 0, "ends with null byte");
  });
});

describe("encodeSimpleQuery", () => {
  it("produces a valid Query message", () => {
    const buf = encodeSimpleQuery("SELECT 1");

    assert.equal(buf[0], 0x51, "message type is 'Q' (0x51)"); // 'Q'
    const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
    const length = view.getInt32(1);
    assert.equal(length, buf.byteLength - 1, "length excludes type byte");

    // Extract the SQL string (after type + length, before trailing null)
    const sql = new TextDecoder().decode(buf.subarray(5, buf.byteLength - 1));
    assert.equal(sql, "SELECT 1");
    assert.equal(buf[buf.byteLength - 1], 0, "ends with null byte");
  });

  it("handles SQL with special characters", () => {
    const sql = "SELECT * FROM users WHERE name = 'O''Brien'";
    const buf = encodeSimpleQuery(sql);

    const decoded = new TextDecoder().decode(buf.subarray(5, buf.byteLength - 1));
    assert.equal(decoded, sql);
  });
});

describe("encodeExtendedQuery", () => {
  it("produces Parse + Bind + Execute + Sync for parameterized query", () => {
    const buf = encodeExtendedQuery("SELECT * FROM users WHERE id = $1", ["42"]);

    // Should contain all four message types: P, B, E, S
    assert.equal(buf[0], 0x50, "first message is Parse ('P')");

    // Find subsequent message boundaries by parsing lengths
    const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
    let offset = 0;
    const types: number[] = [];

    while (offset < buf.byteLength) {
      types.push(buf[offset]);
      const msgLen = view.getInt32(offset + 1);
      offset += 1 + msgLen;
    }

    assert.deepEqual(
      types.map((t) => String.fromCharCode(t)),
      ["P", "B", "E", "S"],
      "contains Parse, Bind, Execute, Sync messages in order"
    );
  });

  it("encodes multiple parameters correctly", () => {
    const buf = encodeExtendedQuery("INSERT INTO users (name, email) VALUES ($1, $2)", [
      "Alice",
      "alice@example.com",
    ]);

    // Verify it's a valid message sequence
    const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
    let offset = 0;
    let msgCount = 0;
    while (offset < buf.byteLength) {
      const msgLen = view.getInt32(offset + 1);
      offset += 1 + msgLen;
      msgCount++;
    }
    assert.equal(msgCount, 4, "four messages total");
  });

  it("handles null parameters", () => {
    const buf = encodeExtendedQuery("INSERT INTO users (name) VALUES ($1)", [null]);

    // Should not throw; null params are encoded with length -1
    assert.ok(buf.byteLength > 0, "produces non-empty buffer");
  });
});

describe("encodeTerminate", () => {
  it("produces a valid Terminate message", () => {
    const buf = encodeTerminate();

    assert.equal(buf[0], 0x58, "message type is 'X' (0x58)");
    const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
    assert.equal(view.getInt32(1), 4, "length is 4 (self-inclusive, no payload)");
    assert.equal(buf.byteLength, 5, "total message is 5 bytes");
  });
});

describe("encodePasswordMessage", () => {
  it("produces a valid password message for cleartext auth", () => {
    const buf = encodePasswordMessage("secret123");

    assert.equal(buf[0], 0x70, "message type is 'p' (0x70)"); // 'p'
    const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
    const length = view.getInt32(1);
    assert.equal(length, buf.byteLength - 1, "length excludes type byte");

    const password = new TextDecoder().decode(buf.subarray(5, buf.byteLength - 1));
    assert.equal(password, "secret123");
    assert.equal(buf[buf.byteLength - 1], 0, "ends with null byte");
  });
});

// ── Decoding Tests ───────────────────────────────────────────────────────────

describe("parseBackendMessages", () => {
  it("parses AuthenticationOk message", () => {
    // 'R' + length(8) + auth_type(0)
    const buf = new Uint8Array([
      0x52, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00,
    ]);
    const messages = parseBackendMessages(buf);

    assert.equal(messages.length, 1);
    assert.equal(messages[0].type, BackendMessageType.Authentication);
    const auth = messages[0] as AuthenticationMessage;
    assert.equal(auth.authType, 0, "auth type is 0 (OK)");
  });

  it("parses AuthenticationCleartextPassword message", () => {
    // 'R' + length(8) + auth_type(3)
    const buf = new Uint8Array([
      0x52, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x03,
    ]);
    const messages = parseBackendMessages(buf);

    assert.equal(messages.length, 1);
    const auth = messages[0] as AuthenticationMessage;
    assert.equal(auth.authType, 3, "auth type is 3 (CleartextPassword)");
  });

  it("parses ReadyForQuery message", () => {
    // 'Z' + length(5) + status('I')
    const buf = new Uint8Array([0x5a, 0x00, 0x00, 0x00, 0x05, 0x49]);
    const messages = parseBackendMessages(buf);

    assert.equal(messages.length, 1);
    assert.equal(messages[0].type, BackendMessageType.ReadyForQuery);
    const rfq = messages[0] as ReadyForQueryMessage;
    assert.equal(rfq.status, "I", "status is Idle");
  });

  it("parses RowDescription with two columns", () => {
    // Build a RowDescription: 'T' + length + num_fields(2) + field1 + field2
    const enc = new TextEncoder();
    const parts: Uint8Array[] = [];

    // Field: name\0 + tableOid(4) + colNo(2) + typeOid(4) + typeLen(2) + typeMod(4) + format(2)
    function encodeField(name: string, typeOid: number): Uint8Array {
      const nameBytes = enc.encode(name);
      const field = new Uint8Array(nameBytes.length + 1 + 18);
      field.set(nameBytes, 0);
      field[nameBytes.length] = 0; // null terminator
      const view = new DataView(field.buffer, field.byteOffset, field.byteLength);
      const off = nameBytes.length + 1;
      view.setInt32(off, 0); // table oid
      view.setInt16(off + 4, 0); // col number
      view.setInt32(off + 6, typeOid); // type oid
      view.setInt16(off + 10, -1); // type length
      view.setInt32(off + 12, -1); // type modifier
      view.setInt16(off + 16, 0); // format (text)
      return field;
    }

    const field1 = encodeField("id", 23); // int4
    const field2 = encodeField("name", 25); // text

    // Calculate total payload: 2 (num_fields) + fields
    const payloadLen = 2 + field1.byteLength + field2.byteLength;
    const msg = new Uint8Array(1 + 4 + payloadLen);
    const view = new DataView(msg.buffer);
    msg[0] = 0x54; // 'T'
    view.setInt32(1, 4 + payloadLen); // length includes self
    view.setInt16(5, 2); // 2 fields
    msg.set(field1, 7);
    msg.set(field2, 7 + field1.byteLength);

    const messages = parseBackendMessages(msg);
    assert.equal(messages.length, 1);
    assert.equal(messages[0].type, BackendMessageType.RowDescription);

    const rd = messages[0] as RowDescriptionMessage;
    assert.equal(rd.fields.length, 2);
    assert.equal(rd.fields[0].name, "id");
    assert.equal(rd.fields[0].typeOid, 23);
    assert.equal(rd.fields[1].name, "name");
    assert.equal(rd.fields[1].typeOid, 25);
  });

  it("parses DataRow with string values", () => {
    // DataRow: 'D' + length + num_cols(2) + col1(len + data) + col2(len + data)
    const enc = new TextEncoder();
    const val1 = enc.encode("1"); // id
    const val2 = enc.encode("Alice"); // name

    const payloadLen = 2 + (4 + val1.length) + (4 + val2.length);
    const msg = new Uint8Array(1 + 4 + payloadLen);
    const view = new DataView(msg.buffer);
    msg[0] = 0x44; // 'D'
    view.setInt32(1, 4 + payloadLen);
    view.setInt16(5, 2); // 2 columns
    let off = 7;
    view.setInt32(off, val1.length);
    msg.set(val1, off + 4);
    off += 4 + val1.length;
    view.setInt32(off, val2.length);
    msg.set(val2, off + 4);

    const messages = parseBackendMessages(msg);
    assert.equal(messages.length, 1);
    assert.equal(messages[0].type, BackendMessageType.DataRow);

    const dr = messages[0] as DataRowMessage;
    assert.equal(dr.values.length, 2);
    assert.equal(new TextDecoder().decode(dr.values[0]!), "1");
    assert.equal(new TextDecoder().decode(dr.values[1]!), "Alice");
  });

  it("parses DataRow with NULL values", () => {
    // NULL is encoded as length -1
    const payloadLen = 2 + 4; // 1 column with length -1
    const msg = new Uint8Array(1 + 4 + payloadLen);
    const view = new DataView(msg.buffer);
    msg[0] = 0x44; // 'D'
    view.setInt32(1, 4 + payloadLen);
    view.setInt16(5, 1); // 1 column
    view.setInt32(7, -1); // NULL

    const messages = parseBackendMessages(msg);
    const dr = messages[0] as DataRowMessage;
    assert.equal(dr.values.length, 1);
    assert.equal(dr.values[0], null, "NULL column is represented as null");
  });

  it("parses CommandComplete message", () => {
    const enc = new TextEncoder();
    const tag = enc.encode("SELECT 5");
    const msg = new Uint8Array(1 + 4 + tag.length + 1);
    const view = new DataView(msg.buffer);
    msg[0] = 0x43; // 'C'
    view.setInt32(1, 4 + tag.length + 1);
    msg.set(tag, 5);
    msg[5 + tag.length] = 0; // null terminator

    const messages = parseBackendMessages(msg);
    assert.equal(messages.length, 1);
    assert.equal(messages[0].type, BackendMessageType.CommandComplete);

    const cc = messages[0] as CommandCompleteMessage;
    assert.equal(cc.tag, "SELECT 5");
  });

  it("parses ErrorResponse message", () => {
    const enc = new TextEncoder();
    // Error fields: S=ERROR, C=42P01, M=relation "foo" does not exist
    const fields = [
      { code: "S", value: "ERROR" },
      { code: "C", value: "42P01" },
      { code: "M", value: 'relation "foo" does not exist' },
    ];

    let payloadSize = 0;
    for (const f of fields) {
      payloadSize += 1 + enc.encode(f.value).length + 1; // code + value + null
    }
    payloadSize += 1; // terminator null

    const msg = new Uint8Array(1 + 4 + payloadSize);
    const view = new DataView(msg.buffer);
    msg[0] = 0x45; // 'E'
    view.setInt32(1, 4 + payloadSize);

    let off = 5;
    for (const f of fields) {
      msg[off] = f.code.charCodeAt(0);
      off++;
      const valBytes = enc.encode(f.value);
      msg.set(valBytes, off);
      off += valBytes.length;
      msg[off] = 0;
      off++;
    }
    msg[off] = 0; // terminator

    const messages = parseBackendMessages(msg);
    assert.equal(messages.length, 1);
    assert.equal(messages[0].type, BackendMessageType.ErrorResponse);

    const err = messages[0] as ErrorResponseMessage;
    assert.equal(err.severity, "ERROR");
    assert.equal(err.code, "42P01");
    assert.equal(err.message, 'relation "foo" does not exist');
  });

  it("parses multiple concatenated messages", () => {
    // AuthenticationOk + ReadyForQuery
    const buf = new Uint8Array([
      // AuthenticationOk: R + len(8) + type(0)
      0x52, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00,
      // ReadyForQuery: Z + len(5) + 'I'
      0x5a, 0x00, 0x00, 0x00, 0x05, 0x49,
    ]);

    const messages = parseBackendMessages(buf);
    assert.equal(messages.length, 2);
    assert.equal(messages[0].type, BackendMessageType.Authentication);
    assert.equal(messages[1].type, BackendMessageType.ReadyForQuery);
  });

  it("skips unknown message types gracefully", () => {
    // ParameterStatus ('S') followed by ReadyForQuery
    const enc = new TextEncoder();
    const key = enc.encode("server_version");
    const val = enc.encode("16.0");
    const psLen = 4 + key.length + 1 + val.length + 1;
    const psBuf = new Uint8Array(1 + psLen);
    const psView = new DataView(psBuf.buffer);
    psBuf[0] = 0x53; // 'S' (ParameterStatus)
    psView.setInt32(1, psLen);
    let off = 5;
    psBuf.set(key, off);
    off += key.length;
    psBuf[off++] = 0;
    psBuf.set(val, off);
    off += val.length;
    psBuf[off] = 0;

    // ReadyForQuery
    const rfqBuf = new Uint8Array([0x5a, 0x00, 0x00, 0x00, 0x05, 0x49]);

    const combined = new Uint8Array(psBuf.length + rfqBuf.length);
    combined.set(psBuf, 0);
    combined.set(rfqBuf, psBuf.length);

    const messages = parseBackendMessages(combined);
    // ParameterStatus is skipped; only ReadyForQuery is returned
    const meaningful = messages.filter(
      (m) => m.type !== BackendMessageType.ParameterStatus
    );
    assert.ok(meaningful.length >= 1, "at least one meaningful message");
    assert.equal(
      meaningful[meaningful.length - 1].type,
      BackendMessageType.ReadyForQuery
    );
  });
});
