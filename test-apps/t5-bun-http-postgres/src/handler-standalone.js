/**
 * T5 Standalone Handler — Bun HTTP + Postgres integration test.
 *
 * This is a standalone version using in-memory data that produces
 * byte-identical HTTP responses to the T4 standalone handler. It
 * demonstrates the same routing pattern and response format, with
 * additional Bun polyfill usage (Bun.env, Bun.sleep).
 *
 * When the @warpgrid/bun-sdk/postgres bridge is ready, the full
 * handler.ts will replace this for end-to-end testing.
 *
 * Routes:
 *   GET  /users      — list all users from in-memory store
 *   POST /users      — insert new user, return 201
 *   GET  /health     — health check
 *
 * Response parity: responses are byte-identical to T4 for the same
 * requests (same JSON keys, same ordering, same headers).
 */

// ── In-memory seed data (mirrors test-infra/seed.sql) ──────────────

let nextId = 6;
const users = [
  { id: 1, name: "Alice Johnson", email: "alice@example.com" },
  { id: 2, name: "Bob Smith", email: "bob@example.com" },
  { id: 3, name: "Carol Williams", email: "carol@example.com" },
  { id: 4, name: "Dave Brown", email: "dave@example.com" },
  { id: 5, name: "Eve Davis", email: "eve@example.com" },
];

// ── Bun Polyfill Verification ───────────────────────────────────────

/**
 * Verify Bun polyfills are available.
 * In native Bun mode, globalThis.Bun is provided by the runtime.
 * In Wasm mode, the @warpgrid/bun-polyfills package shims it.
 * In ComponentizeJS standalone mode, neither is available — we
 * gracefully degrade to environment-provided values.
 */
function getBunEnv(key) {
  // Try Bun.env first (native or polyfilled)
  if (typeof globalThis.Bun !== "undefined" && globalThis.Bun.env) {
    return globalThis.Bun.env[key];
  }
  // Fallback to process.env (ComponentizeJS provides this)
  if (typeof globalThis.process !== "undefined" && globalThis.process.env) {
    return globalThis.process.env[key];
  }
  return undefined;
}

async function bunSleep(ms) {
  // Try Bun.sleep first (native or polyfilled)
  if (typeof globalThis.Bun !== "undefined" && typeof globalThis.Bun.sleep === "function") {
    await globalThis.Bun.sleep(ms);
    return true;
  }
  // Fallback: setTimeout-based sleep
  await new Promise((resolve) => setTimeout(resolve, ms));
  return false;
}

// ── HTTP Handler ────────────────────────────────────────────────────

function jsonResponse(data, status = 200, extraHeaders = {}) {
  const body = JSON.stringify(data);
  const headers = {
    "Content-Type": "application/json",
    ...extraHeaders,
  };

  // Include APP_NAME in response headers if available via environment
  // Matches T4 behavior: use process.env with fallback to default
  const appName = getBunEnv("APP_NAME") ?? "t5-bun-http-postgres";
  headers["X-App-Name"] = appName;

  return new Response(body, { status, headers });
}

function handleGetUsers() {
  return jsonResponse(users);
}

async function handlePostUsers(request) {
  let body;
  try {
    body = await request.json();
  } catch {
    return jsonResponse({ error: "Invalid JSON" }, 400);
  }

  if (!body.name || !body.email) {
    return jsonResponse({ error: "name and email are required" }, 400);
  }

  const newUser = {
    id: nextId++,
    name: body.name,
    email: body.email,
  };
  users.push(newUser);

  return jsonResponse(newUser, 201);
}

function handleHealth() {
  return jsonResponse({ status: "ok" });
}

// ── Fetch Event Listener ────────────────────────────────────────────

addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  const method = event.request.method;

  let responsePromise;

  if (url.pathname === "/users" && method === "GET") {
    responsePromise = Promise.resolve(handleGetUsers());
  } else if (url.pathname === "/users" && method === "POST") {
    responsePromise = handlePostUsers(event.request);
  } else if (url.pathname === "/health") {
    responsePromise = Promise.resolve(handleHealth());
  } else {
    responsePromise = Promise.resolve(
      jsonResponse({ error: "Not Found" }, 404)
    );
  }

  event.respondWith(responsePromise);
});
