/**
 * TDD test suite for the bun-json-api handler logic.
 *
 * US-605: Tests realistic Bun handler through compilation pipeline.
 *
 * The handler is a JSON transformation API:
 *   POST /transform  — accepts JSON, validates required fields, transforms, returns result
 *   POST /validate   — validates a payload against a schema, returns pass/fail
 *   GET  /health     — health check
 *
 * These tests exercise the pure logic extracted into handler-logic.ts,
 * independently of the ComponentizeJS fetch-event runtime.
 */

import { describe, test, expect } from "bun:test";
import {
  type TransformRequest,
  type ValidationRequest,
  type TransformResult,
  type ValidationResult,
  handleTransform,
  handleValidate,
  handleHealth,
  handleRequest,
  type RequestDescriptor,
} from "../src/handler-logic";

// Typed response body parser — avoids `unknown` property access errors
interface SuccessBody<T> {
  success: true;
  data: T;
}
interface ErrorBody {
  success: false;
  error: string;
}

function parseSuccess<T>(body: string): SuccessBody<T> {
  return JSON.parse(body) as SuccessBody<T>;
}

function parseError(body: string): ErrorBody {
  return JSON.parse(body) as ErrorBody;
}

// ── Transform endpoint tests ─────────────────────────────────────────

describe("POST /transform", () => {
  test("transforms user data with uppercase name and domain extraction", () => {
    const input: TransformRequest = {
      name: "alice johnson",
      email: "alice@example.com",
      tags: ["developer", "golang"],
    };

    const result = handleTransform(JSON.stringify(input));

    expect(result.status).toBe(200);
    const body = parseSuccess<TransformResult>(result.body);
    expect(body.success).toBe(true);
    expect(body.data).toBeDefined();
    expect(body.data.displayName).toBe("ALICE JOHNSON");
    expect(body.data.emailDomain).toBe("example.com");
    expect(body.data.tagCount).toBe(2);
    expect(body.data.slug).toBe("alice-johnson");
  });

  test("returns 400 for invalid JSON", () => {
    const result = handleTransform("not-json{{{");

    expect(result.status).toBe(400);
    const body = parseError(result.body);
    expect(body.success).toBe(false);
    expect(body.error).toContain("Invalid JSON");
  });

  test("returns 400 for null body", () => {
    const result = handleTransform(null);

    expect(result.status).toBe(400);
    const body = parseError(result.body);
    expect(body.success).toBe(false);
    expect(body.error).toContain("Invalid JSON");
  });

  test("returns 400 when name is missing", () => {
    const input = { email: "test@example.com", tags: [] };
    const result = handleTransform(JSON.stringify(input));

    expect(result.status).toBe(400);
    const body = parseError(result.body);
    expect(body.success).toBe(false);
    expect(body.error).toContain("name");
  });

  test("returns 400 when email is missing", () => {
    const input = { name: "Bob", tags: [] };
    const result = handleTransform(JSON.stringify(input));

    expect(result.status).toBe(400);
    const body = parseError(result.body);
    expect(body.success).toBe(false);
    expect(body.error).toContain("email");
  });

  test("returns 400 when email format is invalid", () => {
    const input = { name: "Bob", email: "not-an-email", tags: [] };
    const result = handleTransform(JSON.stringify(input));

    expect(result.status).toBe(400);
    const body = parseError(result.body);
    expect(body.success).toBe(false);
    expect(body.error).toContain("email");
  });

  test("handles empty tags array", () => {
    const input: TransformRequest = {
      name: "Carol",
      email: "carol@test.org",
      tags: [],
    };

    const result = handleTransform(JSON.stringify(input));

    expect(result.status).toBe(200);
    const body = parseSuccess<TransformResult>(result.body);
    expect(body.data.tagCount).toBe(0);
  });

  test("handles missing tags field (defaults to empty)", () => {
    const input = { name: "Dave", email: "dave@test.io" };
    const result = handleTransform(JSON.stringify(input));

    expect(result.status).toBe(200);
    const body = parseSuccess<TransformResult>(result.body);
    expect(body.data.tagCount).toBe(0);
  });

  test("generates correct slug from multi-word name with special chars", () => {
    const input = {
      name: "  Jean-Pierre  O'Brien  ",
      email: "jp@test.com",
      tags: [],
    };
    const result = handleTransform(JSON.stringify(input));

    expect(result.status).toBe(200);
    const body = parseSuccess<TransformResult>(result.body);
    expect(body.data.slug).toBe("jean-pierre-obrien");
  });

  test("sets Content-Type header to application/json", () => {
    const input = { name: "Eve", email: "eve@test.com", tags: [] };
    const result = handleTransform(JSON.stringify(input));

    expect(result.headers["content-type"]).toBe("application/json");
  });
});

// ── Validate endpoint tests ──────────────────────────────────────────

describe("POST /validate", () => {
  test("validates a correct payload returns pass", () => {
    const input: ValidationRequest = {
      schema: "user",
      payload: { name: "Alice", email: "alice@example.com", age: 30 },
    };

    const result = handleValidate(JSON.stringify(input));

    expect(result.status).toBe(200);
    const body = parseSuccess<ValidationResult>(result.body);
    expect(body.success).toBe(true);
    expect(body.data.valid).toBe(true);
    expect(body.data.errors).toEqual([]);
  });

  test("validates missing required fields returns errors", () => {
    const input: ValidationRequest = {
      schema: "user",
      payload: { age: 30 },
    };

    const result = handleValidate(JSON.stringify(input));

    expect(result.status).toBe(200);
    const body = parseSuccess<ValidationResult>(result.body);
    expect(body.success).toBe(true);
    expect(body.data.valid).toBe(false);
    expect(body.data.errors.length).toBeGreaterThan(0);
    expect(body.data.errors).toContain("name is required");
    expect(body.data.errors).toContain("email is required");
  });

  test("validates invalid email format", () => {
    const input: ValidationRequest = {
      schema: "user",
      payload: { name: "Bob", email: "not-email", age: 25 },
    };

    const result = handleValidate(JSON.stringify(input));

    expect(result.status).toBe(200);
    const body = parseSuccess<ValidationResult>(result.body);
    expect(body.data.valid).toBe(false);
    expect(body.data.errors).toContain("email must be a valid email address");
  });

  test("validates age must be a positive number", () => {
    const input: ValidationRequest = {
      schema: "user",
      payload: { name: "Carol", email: "carol@test.com", age: -5 },
    };

    const result = handleValidate(JSON.stringify(input));

    expect(result.status).toBe(200);
    const body = parseSuccess<ValidationResult>(result.body);
    expect(body.data.valid).toBe(false);
    expect(body.data.errors).toContain("age must be a positive number");
  });

  test("returns 400 for unknown schema", () => {
    const input: ValidationRequest = {
      schema: "unknown_schema",
      payload: {},
    };

    const result = handleValidate(JSON.stringify(input));

    expect(result.status).toBe(400);
    const body = parseError(result.body);
    expect(body.success).toBe(false);
    expect(body.error).toContain("unknown_schema");
  });

  test("returns 400 for invalid JSON body", () => {
    const result = handleValidate("{{broken");

    expect(result.status).toBe(400);
    const body = parseError(result.body);
    expect(body.success).toBe(false);
    expect(body.error).toContain("Invalid JSON");
  });
});

// ── Health endpoint tests ────────────────────────────────────────────

describe("GET /health", () => {
  test("returns 200 with status ok", () => {
    const result = handleHealth();

    expect(result.status).toBe(200);
    const body = parseSuccess<{ status: string }>(result.body);
    expect(body.success).toBe(true);
    expect(body.data.status).toBe("ok");
  });
});

// ── Router tests ─────────────────────────────────────────────────────

describe("handleRequest (router)", () => {
  test("routes POST /transform to handleTransform", () => {
    const req: RequestDescriptor = {
      method: "POST",
      url: "http://localhost/transform",
      body: JSON.stringify({
        name: "Router Test",
        email: "test@route.com",
        tags: ["a"],
      }),
    };

    const result = handleRequest(req);

    expect(result.status).toBe(200);
    const body = parseSuccess<TransformResult>(result.body);
    expect(body.data.displayName).toBe("ROUTER TEST");
  });

  test("routes POST /validate to handleValidate", () => {
    const req: RequestDescriptor = {
      method: "POST",
      url: "http://localhost/validate",
      body: JSON.stringify({
        schema: "user",
        payload: { name: "Test", email: "t@t.com", age: 1 },
      }),
    };

    const result = handleRequest(req);

    expect(result.status).toBe(200);
    const body = parseSuccess<ValidationResult>(result.body);
    expect(body.data.valid).toBe(true);
  });

  test("routes GET /health to handleHealth", () => {
    const req: RequestDescriptor = {
      method: "GET",
      url: "http://localhost/health",
      body: null,
    };

    const result = handleRequest(req);

    expect(result.status).toBe(200);
    const body = parseSuccess<{ status: string }>(result.body);
    expect(body.data.status).toBe("ok");
  });

  test("returns 404 for unknown routes", () => {
    const req: RequestDescriptor = {
      method: "GET",
      url: "http://localhost/nonexistent",
      body: null,
    };

    const result = handleRequest(req);

    expect(result.status).toBe(404);
    const body = parseError(result.body);
    expect(body.error).toContain("Not Found");
  });

  test("returns 405 for wrong HTTP method on /transform", () => {
    const req: RequestDescriptor = {
      method: "GET",
      url: "http://localhost/transform",
      body: null,
    };

    const result = handleRequest(req);

    expect(result.status).toBe(405);
    const body = parseError(result.body);
    expect(body.error).toContain("Method Not Allowed");
  });
});
