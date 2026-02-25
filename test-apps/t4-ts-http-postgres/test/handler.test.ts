/**
 * Tests for the HTTP handler logic.
 *
 * Validates that handleRequest() correctly routes HTTP methods,
 * parses request bodies, formats JSON responses, and returns
 * appropriate status codes and Content-Type headers.
 *
 * Uses a mock pg Client to isolate handler logic from database concerns.
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";

import { handleRequest, type PgClientLike } from "../src/handler-logic.js";

// ── Mock PgClient ────────────────────────────────────────────────────────────

class MockPgClient implements PgClientLike {
  connected = false;
  queries: Array<{ sql: string; params?: Array<string | null> }> = [];
  queryResults: Array<{ rows: Record<string, string | null>[]; rowCount: number; fields: Array<{ name: string; typeOid: number }> }> = [];
  queryErrors: Array<Error | null> = [];

  connect(): void {
    this.connected = true;
  }

  query(sql: string, params?: Array<string | null>): { rows: Record<string, string | null>[]; rowCount: number; fields: Array<{ name: string; typeOid: number }> } {
    this.queries.push({ sql, params });
    const error = this.queryErrors.shift();
    if (error) throw error;
    const result = this.queryResults.shift();
    if (!result) return { rows: [], rowCount: 0, fields: [] };
    return result;
  }

  end(): void {
    this.connected = false;
  }
}

// ── Handler Tests ────────────────────────────────────────────────────────────

describe("GET /users", () => {
  it("returns all users as JSON with 200 status", () => {
    const mockClient = new MockPgClient();
    mockClient.queryResults.push({
      rows: [
        { id: "1", name: "Alice" },
        { id: "2", name: "Bob" },
      ],
      rowCount: 2,
      fields: [
        { name: "id", typeOid: 23 },
        { name: "name", typeOid: 25 },
      ],
    });

    const response = handleRequest(
      { method: "GET", url: "http://localhost/users", body: null },
      () => mockClient
    );

    assert.equal(response.status, 200);
    assert.equal(response.headers["content-type"], "application/json");

    const body = JSON.parse(response.body);
    assert.equal(body.length, 2);
    assert.deepEqual(body[0], { id: "1", name: "Alice" });
    assert.deepEqual(body[1], { id: "2", name: "Bob" });
  });

  it("returns empty array when no users exist", () => {
    const mockClient = new MockPgClient();
    mockClient.queryResults.push({ rows: [], rowCount: 0, fields: [] });

    const response = handleRequest(
      { method: "GET", url: "http://localhost/users", body: null },
      () => mockClient
    );

    assert.equal(response.status, 200);
    const body = JSON.parse(response.body);
    assert.deepEqual(body, []);
  });

  it("returns 500 when database query fails", () => {
    const mockClient = new MockPgClient();
    mockClient.queryErrors.push(new Error("connection lost"));

    const response = handleRequest(
      { method: "GET", url: "http://localhost/users", body: null },
      () => mockClient
    );

    assert.equal(response.status, 500);
    assert.equal(response.headers["content-type"], "application/json");

    const body = JSON.parse(response.body);
    assert.ok(body.error, "response has error field");
    assert.ok(body.error.includes("connection lost"));
  });
});

describe("GET /users/:id", () => {
  it("returns a single user as JSON", () => {
    const mockClient = new MockPgClient();
    mockClient.queryResults.push({
      rows: [{ id: "1", name: "Alice" }],
      rowCount: 1,
      fields: [
        { name: "id", typeOid: 23 },
        { name: "name", typeOid: 25 },
      ],
    });

    const response = handleRequest(
      { method: "GET", url: "http://localhost/users/1", body: null },
      () => mockClient
    );

    assert.equal(response.status, 200);
    assert.equal(response.headers["content-type"], "application/json");

    const body = JSON.parse(response.body);
    assert.deepEqual(body, { id: "1", name: "Alice" });
  });

  it("returns 404 when user not found", () => {
    const mockClient = new MockPgClient();
    mockClient.queryResults.push({ rows: [], rowCount: 0, fields: [] });

    const response = handleRequest(
      { method: "GET", url: "http://localhost/users/999", body: null },
      () => mockClient
    );

    assert.equal(response.status, 404);
    const body = JSON.parse(response.body);
    assert.ok(body.error.includes("not found"));
  });

  it("returns 400 for non-numeric user ID", () => {
    const mockClient = new MockPgClient();

    const response = handleRequest(
      { method: "GET", url: "http://localhost/users/abc", body: null },
      () => mockClient
    );

    assert.equal(response.status, 400);
    const body = JSON.parse(response.body);
    assert.ok(body.error.includes("invalid"));
  });
});

describe("POST /users", () => {
  it("creates a user and returns 201 with the new user", () => {
    const mockClient = new MockPgClient();
    mockClient.queryResults.push({
      rows: [{ id: "6", name: "Charlie" }],
      rowCount: 1,
      fields: [
        { name: "id", typeOid: 23 },
        { name: "name", typeOid: 25 },
      ],
    });

    const response = handleRequest(
      {
        method: "POST",
        url: "http://localhost/users",
        body: JSON.stringify({ name: "Charlie" }),
      },
      () => mockClient
    );

    assert.equal(response.status, 201);
    assert.equal(response.headers["content-type"], "application/json");

    const body = JSON.parse(response.body);
    assert.equal(body.id, "6");
    assert.equal(body.name, "Charlie");

    // Verify parameterized query was used
    assert.equal(mockClient.queries.length, 1);
    assert.ok(mockClient.queries[0].sql.includes("INSERT"));
    assert.deepEqual(mockClient.queries[0].params, ["Charlie"]);
  });

  it("returns 400 when body is not valid JSON", () => {
    const mockClient = new MockPgClient();

    const response = handleRequest(
      { method: "POST", url: "http://localhost/users", body: "not json" },
      () => mockClient
    );

    assert.equal(response.status, 400);
    const body = JSON.parse(response.body);
    assert.ok(body.error.includes("invalid") || body.error.includes("JSON"));
  });

  it("returns 400 when name field is missing", () => {
    const mockClient = new MockPgClient();

    const response = handleRequest(
      { method: "POST", url: "http://localhost/users", body: JSON.stringify({ email: "a@b.com" }) },
      () => mockClient
    );

    assert.equal(response.status, 400);
    const body = JSON.parse(response.body);
    assert.ok(body.error.includes("name"));
  });

  it("returns 400 when name is empty string", () => {
    const mockClient = new MockPgClient();

    const response = handleRequest(
      { method: "POST", url: "http://localhost/users", body: JSON.stringify({ name: "" }) },
      () => mockClient
    );

    assert.equal(response.status, 400);
    const body = JSON.parse(response.body);
    assert.ok(body.error.includes("name"));
  });

  it("returns 500 when database insert fails", () => {
    const mockClient = new MockPgClient();
    mockClient.queryErrors.push(new Error("unique_violation: duplicate key"));

    const response = handleRequest(
      { method: "POST", url: "http://localhost/users", body: JSON.stringify({ name: "Alice" }) },
      () => mockClient
    );

    assert.equal(response.status, 500);
    const body = JSON.parse(response.body);
    assert.ok(body.error);
  });
});

describe("unsupported routes", () => {
  it("returns 404 for unknown path", () => {
    const mockClient = new MockPgClient();

    const response = handleRequest(
      { method: "GET", url: "http://localhost/unknown", body: null },
      () => mockClient
    );

    assert.equal(response.status, 404);
  });

  it("returns 405 for unsupported HTTP method on /users", () => {
    const mockClient = new MockPgClient();

    const response = handleRequest(
      { method: "DELETE", url: "http://localhost/users", body: null },
      () => mockClient
    );

    assert.equal(response.status, 405);
  });
});

describe("Content-Type header", () => {
  it("always sets application/json content-type", () => {
    const mockClient = new MockPgClient();
    mockClient.queryResults.push({ rows: [], rowCount: 0, fields: [] });

    const response = handleRequest(
      { method: "GET", url: "http://localhost/users", body: null },
      () => mockClient
    );

    assert.equal(response.headers["content-type"], "application/json");
  });
});
