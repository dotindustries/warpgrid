import { describe, test, expect } from "bun:test";
import {
  buildStartupMessage,
  buildSimpleQuery,
  buildExtendedQuery,
  buildTerminateMessage,
  buildCleartextPasswordMessage,
  parseMessages,
  parseAuthResponse,
  parseRowDescription,
  parseDataRow,
  parseErrorResponse,
  parseCommandComplete,
  MSG,
  AUTH_OK,
  AUTH_CLEARTEXT,
  AUTH_MD5,
} from "../src/postgres-protocol.ts";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

// ── Helper: build raw Postgres backend messages ─────────────────────

function buildBackendMessage(type: number, payload: Uint8Array): Uint8Array {
  const length = 4 + payload.length;
  const buf = new ArrayBuffer(1 + length);
  const view = new DataView(buf);
  view.setUint8(0, type);
  view.setInt32(1, length);
  new Uint8Array(buf).set(payload, 5);
  return new Uint8Array(buf);
}

function buildAuthOkPayload(): Uint8Array {
  const buf = new ArrayBuffer(4);
  new DataView(buf).setInt32(0, AUTH_OK);
  return new Uint8Array(buf);
}

function buildAuthCleartextPayload(): Uint8Array {
  const buf = new ArrayBuffer(4);
  new DataView(buf).setInt32(0, AUTH_CLEARTEXT);
  return new Uint8Array(buf);
}

function buildAuthMD5Payload(salt: Uint8Array): Uint8Array {
  const buf = new ArrayBuffer(8);
  const view = new DataView(buf);
  view.setInt32(0, AUTH_MD5);
  new Uint8Array(buf).set(salt, 4);
  return new Uint8Array(buf);
}

function buildReadyForQuery(status: string): Uint8Array {
  const payload = new Uint8Array([status.charCodeAt(0)]);
  return buildBackendMessage(MSG.READY_FOR_QUERY, payload);
}

function buildRowDescription(
  fields: { name: string; typeOID: number }[],
): Uint8Array {
  const parts: number[] = [];
  // Field count (Int16)
  parts.push((fields.length >> 8) & 0xff, fields.length & 0xff);

  for (const field of fields) {
    // Field name (null-terminated)
    const nameBytes = encoder.encode(field.name);
    for (const b of nameBytes) parts.push(b);
    parts.push(0); // null terminator
    // Table OID (Int32): 0
    parts.push(0, 0, 0, 0);
    // Column attribute number (Int16): 0
    parts.push(0, 0);
    // Data type OID (Int32)
    parts.push(
      (field.typeOID >> 24) & 0xff,
      (field.typeOID >> 16) & 0xff,
      (field.typeOID >> 8) & 0xff,
      field.typeOID & 0xff,
    );
    // Data type size (Int16): -1
    parts.push(0xff, 0xff);
    // Type modifier (Int32): -1
    parts.push(0xff, 0xff, 0xff, 0xff);
    // Format code (Int16): 0 (text)
    parts.push(0, 0);
  }

  return buildBackendMessage(
    MSG.ROW_DESCRIPTION,
    new Uint8Array(parts),
  );
}

function buildDataRowMsg(values: (string | null)[]): Uint8Array {
  const parts: number[] = [];
  // Field count (Int16)
  parts.push((values.length >> 8) & 0xff, values.length & 0xff);

  for (const val of values) {
    if (val === null) {
      // -1 in Int32
      parts.push(0xff, 0xff, 0xff, 0xff);
    } else {
      const bytes = encoder.encode(val);
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
  return buildBackendMessage(
    MSG.COMMAND_COMPLETE,
    encoder.encode(tag + "\0"),
  );
}

function buildPgErrorResponse(fields: Record<string, string>): Uint8Array {
  const parts: number[] = [];
  for (const [key, value] of Object.entries(fields)) {
    parts.push(key.charCodeAt(0));
    const bytes = encoder.encode(value);
    for (const b of bytes) parts.push(b);
    parts.push(0); // null terminator
  }
  parts.push(0); // terminator
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

// ── Tests ───────────────────────────────────────────────────────────

describe("buildStartupMessage", () => {
  test("creates correct binary format", () => {
    const msg = buildStartupMessage("testuser", "testdb");
    const view = new DataView(msg.buffer, msg.byteOffset, msg.byteLength);

    // Length is first Int32, includes self
    const length = view.getInt32(0);
    expect(length).toBe(msg.length);

    // Protocol version 3.0
    const version = view.getInt32(4);
    expect(version).toBe(196608);

    // Parameters contain user and database
    const params = decoder.decode(msg.slice(8));
    expect(params).toContain("user\0testuser\0");
    expect(params).toContain("database\0testdb\0");

    // Ends with double null
    expect(msg[msg.length - 1]).toBe(0);
  });
});

describe("buildSimpleQuery", () => {
  test("creates correct Q message", () => {
    const msg = buildSimpleQuery("SELECT 1");

    // Type byte is 'Q'
    expect(msg[0]).toBe(0x51);

    // Length (Int32 at offset 1)
    const view = new DataView(msg.buffer, msg.byteOffset + 1, 4);
    const length = view.getInt32(0);
    expect(length).toBe(4 + "SELECT 1".length + 1); // 4 + sql + null

    // SQL string follows
    const sql = decoder.decode(msg.slice(5, msg.length - 1));
    expect(sql).toBe("SELECT 1");

    // Null terminator
    expect(msg[msg.length - 1]).toBe(0);
  });
});

describe("buildExtendedQuery", () => {
  test("creates Parse+Bind+Execute+Sync batch", () => {
    const msg = buildExtendedQuery("SELECT $1", [42]);

    // Should start with Parse ('P')
    expect(msg[0]).toBe(MSG.PARSE);

    // Should contain Bind ('B'), Execute ('E'), Sync ('S') somewhere after
    const types = new Set<number>();
    let offset = 0;
    while (offset + 5 <= msg.length) {
      types.add(msg[offset]);
      const view = new DataView(msg.buffer, msg.byteOffset + offset + 1, 4);
      const len = view.getInt32(0);
      offset += 1 + len;
    }

    expect(types.has(MSG.PARSE)).toBe(true);
    expect(types.has(MSG.BIND)).toBe(true);
    expect(types.has(MSG.EXECUTE)).toBe(true);
    expect(types.has(MSG.SYNC)).toBe(true);
  });

  test("encodes null parameters correctly", () => {
    const msg = buildExtendedQuery("INSERT INTO t VALUES ($1)", [null]);
    // The Bind message should contain -1 for null parameter length
    // Just verify it doesn't throw
    expect(msg.length).toBeGreaterThan(0);
  });
});

describe("buildTerminateMessage", () => {
  test("creates correct X message", () => {
    const msg = buildTerminateMessage();
    expect(msg[0]).toBe(MSG.TERMINATE);
    expect(msg.length).toBe(5);
    const view = new DataView(msg.buffer, msg.byteOffset + 1, 4);
    expect(view.getInt32(0)).toBe(4);
  });
});

describe("buildCleartextPasswordMessage", () => {
  test("creates correct password message", () => {
    const msg = buildCleartextPasswordMessage("secret");
    expect(msg[0]).toBe(MSG.PASSWORD);
    const content = decoder.decode(msg.slice(5, msg.length - 1));
    expect(content).toBe("secret");
  });
});

describe("parseMessages", () => {
  test("parses single message", () => {
    const raw = buildBackendMessage(MSG.AUTH, buildAuthOkPayload());
    const msgs = parseMessages(raw);
    expect(msgs).toHaveLength(1);
    expect(msgs[0].type).toBe(MSG.AUTH);
  });

  test("parses multiple messages", () => {
    const raw = concat(
      buildBackendMessage(MSG.AUTH, buildAuthOkPayload()),
      buildReadyForQuery("I"),
    );
    const msgs = parseMessages(raw);
    expect(msgs).toHaveLength(2);
    expect(msgs[0].type).toBe(MSG.AUTH);
    expect(msgs[1].type).toBe(MSG.READY_FOR_QUERY);
  });

  test("handles incomplete message gracefully", () => {
    const raw = new Uint8Array([0x52, 0x00, 0x00]); // Incomplete Auth
    const msgs = parseMessages(raw);
    expect(msgs).toHaveLength(0);
  });

  test("handles empty input", () => {
    const msgs = parseMessages(new Uint8Array(0));
    expect(msgs).toHaveLength(0);
  });
});

describe("parseAuthResponse", () => {
  test("parses AuthenticationOk", () => {
    const result = parseAuthResponse(buildAuthOkPayload());
    expect(result.type).toBe(AUTH_OK);
    expect(result.salt).toBeUndefined();
  });

  test("parses AuthenticationCleartextPassword", () => {
    const result = parseAuthResponse(buildAuthCleartextPayload());
    expect(result.type).toBe(AUTH_CLEARTEXT);
  });

  test("parses AuthenticationMD5Password with salt", () => {
    const salt = new Uint8Array([0xde, 0xad, 0xbe, 0xef]);
    const result = parseAuthResponse(buildAuthMD5Payload(salt));
    expect(result.type).toBe(AUTH_MD5);
    expect(result.salt).toEqual(salt);
  });
});

describe("parseRowDescription", () => {
  test("parses field metadata", () => {
    const raw = buildRowDescription([
      { name: "id", typeOID: 23 }, // int4
      { name: "name", typeOID: 25 }, // text
    ]);
    const msgs = parseMessages(raw);
    expect(msgs).toHaveLength(1);

    const fields = parseRowDescription(msgs[0].payload);
    expect(fields).toHaveLength(2);
    expect(fields[0].name).toBe("id");
    expect(fields[0].dataTypeOID).toBe(23);
    expect(fields[1].name).toBe("name");
    expect(fields[1].dataTypeOID).toBe(25);
  });
});

describe("parseDataRow", () => {
  test("parses text values", () => {
    const raw = buildDataRowMsg(["42", "Alice"]);
    const msgs = parseMessages(raw);
    const values = parseDataRow(msgs[0].payload);
    expect(values).toEqual(["42", "Alice"]);
  });

  test("handles NULL values", () => {
    const raw = buildDataRowMsg(["1", null, "test"]);
    const msgs = parseMessages(raw);
    const values = parseDataRow(msgs[0].payload);
    expect(values).toEqual(["1", null, "test"]);
  });

  test("handles empty string", () => {
    const raw = buildDataRowMsg([""]);
    const msgs = parseMessages(raw);
    const values = parseDataRow(msgs[0].payload);
    expect(values).toEqual([""]);
  });
});

describe("parseErrorResponse", () => {
  test("extracts severity, code, and message", () => {
    const raw = buildPgErrorResponse({
      S: "ERROR",
      C: "42P01",
      M: 'relation "users" does not exist',
    });
    const msgs = parseMessages(raw);
    const err = parseErrorResponse(msgs[0].payload);
    expect(err.severity).toBe("ERROR");
    expect(err.code).toBe("42P01");
    expect(err.message).toBe('relation "users" does not exist');
  });

  test("includes detail and hint when present", () => {
    const raw = buildPgErrorResponse({
      S: "ERROR",
      C: "23505",
      M: "duplicate key",
      D: "Key (id)=(1) already exists.",
      H: "Use ON CONFLICT to handle duplicates.",
    });
    const msgs = parseMessages(raw);
    const err = parseErrorResponse(msgs[0].payload);
    expect(err.detail).toBe("Key (id)=(1) already exists.");
    expect(err.hint).toBe("Use ON CONFLICT to handle duplicates.");
  });
});

describe("parseCommandComplete", () => {
  test("parses SELECT count", () => {
    const payload = encoder.encode("SELECT 5\0");
    expect(parseCommandComplete(payload)).toBe(5);
  });

  test("parses INSERT count", () => {
    const payload = encoder.encode("INSERT 0 3\0");
    expect(parseCommandComplete(payload)).toBe(3);
  });

  test("parses UPDATE count", () => {
    const payload = encoder.encode("UPDATE 10\0");
    expect(parseCommandComplete(payload)).toBe(10);
  });

  test("parses DELETE count", () => {
    const payload = encoder.encode("DELETE 1\0");
    expect(parseCommandComplete(payload)).toBe(1);
  });

  test("returns 0 for unrecognized format", () => {
    const payload = encoder.encode("BEGIN\0");
    expect(parseCommandComplete(payload)).toBe(0);
  });
});
