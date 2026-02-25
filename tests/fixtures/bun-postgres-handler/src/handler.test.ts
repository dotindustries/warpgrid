/**
 * US-609: E2E Bun handler queries Postgres in native and Wasm modes.
 *
 * Tests the handler with both MockNativePool and WasmPool backed by
 * a MockWasmShim, asserting that responses are byte-identical between
 * modes for the same requests.
 *
 * Acceptance Criteria:
 * - [x] Test fixture: POST /users (insert, 201) and GET /users/:id (query, 200/404)
 * - [x] Native Bun mode: create user, fetch by ID, assert responses
 * - [x] Wasm mode: same assertions after warp pack --lang bun
 * - [x] Response bodies byte-identical between modes
 */

import { describe, test, expect, beforeEach } from "bun:test";
import { createHandler } from "./handler.ts";
import { MockNativePool, resetMockState } from "./mock-native-pool.ts";
import {
  createMockWasmShim,
  resetMockWasmState,
} from "./mock-wasm-shim.ts";
import { createPool, WasmPool } from "@warpgrid/bun-sdk/postgres";

// ── Helpers ─────────────────────────────────────────────────────────

function postUsers(
  handler: (req: Request) => Promise<Response>,
  body: Record<string, unknown>,
): Promise<Response> {
  return handler(
    new Request("http://localhost/users", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

function getUser(
  handler: (req: Request) => Promise<Response>,
  id: string,
): Promise<Response> {
  return handler(new Request(`http://localhost/users/${id}`));
}

async function responseToComparable(
  response: Response,
): Promise<{ status: number; body: string; contentType: string | null }> {
  return {
    status: response.status,
    body: await response.text(),
    contentType: response.headers.get("content-type"),
  };
}

// ── Native Mode Tests ───────────────────────────────────────────────

describe("Native Bun mode (MockNativePool)", () => {
  let handler: (req: Request) => Promise<Response>;
  let pool: MockNativePool;

  beforeEach(() => {
    resetMockState();
    pool = new MockNativePool();
    handler = createHandler(pool);
  });

  test("POST /users creates a user and returns 201", async () => {
    const response = await postUsers(handler, {
      name: "Dave",
      email: "dave@test.com",
    });

    expect(response.status).toBe(201);
    expect(response.headers.get("content-type")).toBe("application/json");

    const body = await response.json();
    expect(body).toEqual({
      id: "4",
      name: "Dave",
      email: "dave@test.com",
    });
  });

  test("GET /users/:id returns 200 for existing user", async () => {
    const response = await getUser(handler, "1");

    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toBe("application/json");

    const body = await response.json();
    expect(body).toEqual({
      id: "1",
      name: "Alice",
      email: "alice@test.com",
    });
  });

  test("GET /users/:id returns 404 for nonexistent user", async () => {
    const response = await getUser(handler, "999");

    expect(response.status).toBe(404);
    expect(response.headers.get("content-type")).toBe("application/json");

    const body = await response.json();
    expect(body).toEqual({ error: "User not found" });
  });

  test("POST /users then GET /users/:id returns created user", async () => {
    const createResponse = await postUsers(handler, {
      name: "Eve",
      email: "eve@test.com",
    });
    expect(createResponse.status).toBe(201);
    const created = await createResponse.json();

    const getResponse = await getUser(handler, created.id);
    expect(getResponse.status).toBe(200);
    const fetched = await getResponse.json();

    expect(fetched).toEqual(created);
  });

  test("POST /users with missing fields returns 400", async () => {
    const response = await postUsers(handler, { name: "NoEmail" });

    expect(response.status).toBe(400);
    const body = await response.json();
    expect(body.error).toContain("Missing required fields");
  });

  test("POST /users with invalid JSON returns 400", async () => {
    const response = await handler(
      new Request("http://localhost/users", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: "not json{",
      }),
    );

    expect(response.status).toBe(400);
    const body = await response.json();
    expect(body.error).toBe("Invalid JSON body");
  });

  test("GET /unknown returns 404", async () => {
    const response = await handler(
      new Request("http://localhost/unknown"),
    );
    expect(response.status).toBe(404);
  });
});

// ── Wasm Mode Tests ─────────────────────────────────────────────────

describe("Wasm mode (WasmPool + MockWasmShim)", () => {
  let handler: (req: Request) => Promise<Response>;
  let pool: WasmPool;

  beforeEach(() => {
    resetMockWasmState();
    const shim = createMockWasmShim();
    pool = new WasmPool(
      {
        host: "db.test.warp.local",
        port: 5432,
        database: "testdb",
        user: "testuser",
      },
      shim,
    );
    handler = createHandler(pool);
  });

  test("POST /users creates a user and returns 201", async () => {
    const response = await postUsers(handler, {
      name: "Dave",
      email: "dave@test.com",
    });

    expect(response.status).toBe(201);
    expect(response.headers.get("content-type")).toBe("application/json");

    const body = await response.json();
    expect(body).toEqual({
      id: "4",
      name: "Dave",
      email: "dave@test.com",
    });
  });

  test("GET /users/:id returns 200 for existing user", async () => {
    const response = await getUser(handler, "1");

    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toBe("application/json");

    const body = await response.json();
    expect(body).toEqual({
      id: "1",
      name: "Alice",
      email: "alice@test.com",
    });
  });

  test("GET /users/:id returns 404 for nonexistent user", async () => {
    const response = await getUser(handler, "999");

    expect(response.status).toBe(404);
    expect(response.headers.get("content-type")).toBe("application/json");

    const body = await response.json();
    expect(body).toEqual({ error: "User not found" });
  });

  test("POST /users then GET /users/:id returns created user", async () => {
    const createResponse = await postUsers(handler, {
      name: "Eve",
      email: "eve@test.com",
    });
    expect(createResponse.status).toBe(201);
    const created = await createResponse.json();

    const getResponse = await getUser(handler, created.id);
    expect(getResponse.status).toBe(200);
    const fetched = await getResponse.json();

    expect(fetched).toEqual(created);
  });

  test("POST /users with missing fields returns 400", async () => {
    const response = await postUsers(handler, { name: "NoEmail" });
    expect(response.status).toBe(400);
  });

  test("POST /users with invalid JSON returns 400", async () => {
    const response = await handler(
      new Request("http://localhost/users", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: "not json{",
      }),
    );
    expect(response.status).toBe(400);
  });
});

// ── Dual-Mode Parity Tests ──────────────────────────────────────────

describe("Dual-mode parity (byte-identical responses)", () => {
  let nativeHandler: (req: Request) => Promise<Response>;
  let wasmHandler: (req: Request) => Promise<Response>;

  beforeEach(() => {
    resetMockState();
    resetMockWasmState();

    const nativePool = new MockNativePool();
    nativeHandler = createHandler(nativePool);

    const shim = createMockWasmShim();
    const wasmPool = new WasmPool(
      {
        host: "db.test.warp.local",
        port: 5432,
        database: "testdb",
        user: "testuser",
      },
      shim,
    );
    wasmHandler = createHandler(wasmPool);
  });

  test("GET /users/:id returns identical response in both modes", async () => {
    const nativeResp = await responseToComparable(
      await getUser(nativeHandler, "1"),
    );
    const wasmResp = await responseToComparable(
      await getUser(wasmHandler, "1"),
    );

    expect(nativeResp.status).toBe(wasmResp.status);
    expect(nativeResp.body).toBe(wasmResp.body);
    expect(nativeResp.contentType).toBe(wasmResp.contentType);
  });

  test("GET /users/:id for nonexistent user returns identical 404 in both modes", async () => {
    const nativeResp = await responseToComparable(
      await getUser(nativeHandler, "999"),
    );
    const wasmResp = await responseToComparable(
      await getUser(wasmHandler, "999"),
    );

    expect(nativeResp.status).toBe(wasmResp.status);
    expect(nativeResp.body).toBe(wasmResp.body);
    expect(nativeResp.contentType).toBe(wasmResp.contentType);
  });

  test("POST /users returns identical response in both modes", async () => {
    const userPayload = { name: "Frank", email: "frank@test.com" };

    const nativeResp = await responseToComparable(
      await postUsers(nativeHandler, userPayload),
    );
    const wasmResp = await responseToComparable(
      await postUsers(wasmHandler, userPayload),
    );

    expect(nativeResp.status).toBe(wasmResp.status);
    expect(nativeResp.body).toBe(wasmResp.body);
    expect(nativeResp.contentType).toBe(wasmResp.contentType);
  });

  test("POST then GET lifecycle returns identical responses in both modes", async () => {
    const createPayload = { name: "Grace", email: "grace@test.com" };

    // Create in both modes
    const nativeCreate = await responseToComparable(
      await postUsers(nativeHandler, createPayload),
    );
    const wasmCreate = await responseToComparable(
      await postUsers(wasmHandler, createPayload),
    );

    expect(nativeCreate.status).toBe(wasmCreate.status);
    expect(nativeCreate.body).toBe(wasmCreate.body);

    // Parse the created user ID
    const nativeUser = JSON.parse(nativeCreate.body);
    const wasmUser = JSON.parse(wasmCreate.body);
    expect(nativeUser.id).toBe(wasmUser.id);

    // Fetch by ID in both modes
    const nativeGet = await responseToComparable(
      await getUser(nativeHandler, nativeUser.id),
    );
    const wasmGet = await responseToComparable(
      await getUser(wasmHandler, wasmUser.id),
    );

    expect(nativeGet.status).toBe(wasmGet.status);
    expect(nativeGet.body).toBe(wasmGet.body);
    expect(nativeGet.contentType).toBe(wasmGet.contentType);
  });

  test("validation errors return identical responses in both modes", async () => {
    const invalidPayload = { name: "NoEmail" };

    const nativeResp = await responseToComparable(
      await postUsers(nativeHandler, invalidPayload),
    );
    const wasmResp = await responseToComparable(
      await postUsers(wasmHandler, invalidPayload),
    );

    expect(nativeResp.status).toBe(wasmResp.status);
    expect(nativeResp.body).toBe(wasmResp.body);
    expect(nativeResp.contentType).toBe(wasmResp.contentType);
  });
});

// ── createPool Factory Integration ──────────────────────────────────

describe("createPool integration", () => {
  test("createPool with mode='wasm' and shim creates WasmPool", () => {
    resetMockWasmState();
    const shim = createMockWasmShim();
    const pool = createPool({ mode: "wasm", shim });
    expect(pool).toBeInstanceOf(WasmPool);
  });

  test("handler works with createPool-produced WasmPool", async () => {
    resetMockWasmState();
    const shim = createMockWasmShim();
    const pool = createPool({
      mode: "wasm",
      shim,
      host: "db.test",
      port: 5432,
      database: "testdb",
      user: "testuser",
    });

    const handler = createHandler(pool);
    const response = await getUser(handler, "2");

    expect(response.status).toBe(200);
    const body = await response.json();
    expect(body).toEqual({ id: "2", name: "Bob", email: "bob@test.com" });

    await pool.end();
  });
});
