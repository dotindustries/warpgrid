/**
 * Bun Postgres Handler — dual-mode HTTP handler for US-609 E2E testing.
 *
 * Routes:
 *   POST /users      — insert a new user, return 201 with created user JSON
 *   GET  /users/:id  — fetch user by ID, return 200 or 404
 *
 * The handler accepts an injected Pool instance so the same code works
 * with both NativePool (Bun dev) and WasmPool (WASI deploy) backends.
 */

import type { Pool } from "@warpgrid/bun-sdk/postgres";

export interface User {
  id: string;
  name: string;
  email: string;
}

export interface CreateUserBody {
  name: string;
  email: string;
}

/**
 * Create a request handler backed by the given Pool.
 * Returns a fetch-compatible handler function.
 */
export function createHandler(pool: Pool): (request: Request) => Promise<Response> {
  return async (request: Request): Promise<Response> => {
    const url = new URL(request.url);
    const path = url.pathname;

    // POST /users — create a user
    if (request.method === "POST" && path === "/users") {
      return handleCreateUser(pool, request);
    }

    // GET /users/:id — fetch user by ID
    const userMatch = path.match(/^\/users\/(\d+)$/);
    if (request.method === "GET" && userMatch) {
      return handleGetUser(pool, userMatch[1]);
    }

    return new Response(
      JSON.stringify({ error: "Not Found" }),
      { status: 404, headers: { "content-type": "application/json" } },
    );
  };
}

async function handleCreateUser(
  pool: Pool,
  request: Request,
): Promise<Response> {
  let body: CreateUserBody;
  try {
    body = (await request.json()) as CreateUserBody;
  } catch {
    return new Response(
      JSON.stringify({ error: "Invalid JSON body" }),
      { status: 400, headers: { "content-type": "application/json" } },
    );
  }

  if (!body.name || !body.email) {
    return new Response(
      JSON.stringify({ error: "Missing required fields: name, email" }),
      { status: 400, headers: { "content-type": "application/json" } },
    );
  }

  try {
    const result = await pool.query(
      "INSERT INTO users (name, email) VALUES ($1, $2) RETURNING id, name, email",
      [body.name, body.email],
    );
    const user = result.rows[0] as unknown as User;
    return new Response(
      JSON.stringify(user),
      { status: 201, headers: { "content-type": "application/json" } },
    );
  } catch (err) {
    return new Response(
      JSON.stringify({ error: "Database error", detail: String(err) }),
      { status: 503, headers: { "content-type": "application/json" } },
    );
  }
}

async function handleGetUser(
  pool: Pool,
  id: string,
): Promise<Response> {
  try {
    const result = await pool.query(
      "SELECT id, name, email FROM users WHERE id = $1",
      [id],
    );

    if (result.rows.length === 0) {
      return new Response(
        JSON.stringify({ error: "User not found" }),
        { status: 404, headers: { "content-type": "application/json" } },
      );
    }

    const user = result.rows[0] as unknown as User;
    return new Response(
      JSON.stringify(user),
      { status: 200, headers: { "content-type": "application/json" } },
    );
  } catch (err) {
    return new Response(
      JSON.stringify({ error: "Database error", detail: String(err) }),
      { status: 503, headers: { "content-type": "application/json" } },
    );
  }
}
