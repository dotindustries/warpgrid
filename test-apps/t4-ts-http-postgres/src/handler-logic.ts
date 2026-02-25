/**
 * HTTP handler logic for the T4 test application.
 *
 * Pure functions that process HTTP requests and return response descriptors.
 * No WIT imports — testable in plain Node.js.
 *
 * Routes:
 *   GET  /users       — list all users
 *   GET  /users/:id   — get user by ID
 *   POST /users       — create a new user
 */

// ── Interfaces ───────────────────────────────────────────────────────────────

/** Minimal pg Client interface needed by the handler. */
export interface PgClientLike {
  connect(): void;
  query(sql: string, params?: Array<string | null>): {
    rows: Record<string, string | null>[];
    rowCount: number;
    fields: Array<{ name: string; typeOid: number }>;
  };
  end(): void;
}

export interface RequestDescriptor {
  method: string;
  url: string;
  body: string | null;
}

export interface ResponseDescriptor {
  status: number;
  headers: Record<string, string>;
  body: string;
}

/** Factory function that creates a connected PgClient. */
type ClientFactory = () => PgClientLike;

// ── Route Handlers ───────────────────────────────────────────────────────────

/**
 * Main request handler. Routes to the appropriate handler function.
 *
 * @param req - The incoming HTTP request descriptor
 * @param createClient - Factory that creates a connected PgClient
 * @returns A response descriptor with status, headers, and body
 */
export function handleRequest(
  req: RequestDescriptor,
  createClient: ClientFactory
): ResponseDescriptor {
  const url = new URL(req.url);
  const path = url.pathname;

  // Route: GET /users
  if (path === "/users" && req.method === "GET") {
    return handleGetUsers(createClient);
  }

  // Route: POST /users
  if (path === "/users" && req.method === "POST") {
    return handlePostUser(req.body, createClient);
  }

  // Route: GET /users/:id
  const userIdMatch = path.match(/^\/users\/([^/]+)$/);
  if (userIdMatch && req.method === "GET") {
    return handleGetUserById(userIdMatch[1], createClient);
  }

  // Route: unsupported method on /users
  if (path === "/users" || (userIdMatch && req.method !== "GET")) {
    return jsonResponse(405, { error: "method not allowed" });
  }

  // Route: not found
  return jsonResponse(404, { error: "not found" });
}

function handleGetUsers(createClient: ClientFactory): ResponseDescriptor {
  let client: PgClientLike | null = null;
  try {
    client = createClient();
    client.connect();
    const result = client.query("SELECT id, name FROM test_users ORDER BY id");
    return jsonResponse(200, result.rows);
  } catch (err) {
    return jsonResponse(500, { error: formatError(err) });
  } finally {
    client?.end();
  }
}

function handleGetUserById(idStr: string, createClient: ClientFactory): ResponseDescriptor {
  const id = parseInt(idStr, 10);
  if (isNaN(id) || id <= 0) {
    return jsonResponse(400, { error: "invalid user ID: must be a positive integer" });
  }

  let client: PgClientLike | null = null;
  try {
    client = createClient();
    client.connect();
    const result = client.query("SELECT id, name FROM test_users WHERE id = $1", [String(id)]);

    if (result.rows.length === 0) {
      return jsonResponse(404, { error: `user ${id} not found` });
    }

    return jsonResponse(200, result.rows[0]);
  } catch (err) {
    return jsonResponse(500, { error: formatError(err) });
  } finally {
    client?.end();
  }
}

function handlePostUser(body: string | null, createClient: ClientFactory): ResponseDescriptor {
  // Parse and validate request body
  if (!body) {
    return jsonResponse(400, { error: "request body is required" });
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(body);
  } catch {
    return jsonResponse(400, { error: "invalid JSON in request body" });
  }

  if (typeof parsed !== "object" || parsed === null) {
    return jsonResponse(400, { error: "invalid JSON: expected an object" });
  }

  const name = (parsed as Record<string, unknown>).name;
  if (typeof name !== "string" || name.trim() === "") {
    return jsonResponse(400, { error: "name field is required and must be a non-empty string" });
  }

  let client: PgClientLike | null = null;
  try {
    client = createClient();
    client.connect();
    const result = client.query(
      "INSERT INTO test_users (name) VALUES ($1) RETURNING id, name",
      [name.trim()]
    );

    if (result.rows.length === 0) {
      return jsonResponse(500, { error: "insert did not return a row" });
    }

    return jsonResponse(201, result.rows[0]);
  } catch (err) {
    return jsonResponse(500, { error: formatError(err) });
  } finally {
    client?.end();
  }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

function jsonResponse(status: number, data: unknown): ResponseDescriptor {
  return {
    status,
    headers: { "content-type": "application/json" },
    body: JSON.stringify(data),
  };
}

function formatError(err: unknown): string {
  if (err instanceof Error) return err.message;
  return String(err);
}
