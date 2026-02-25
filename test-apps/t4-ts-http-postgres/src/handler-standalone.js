/**
 * T4 Standalone Handler — works with current ComponentizeJS tooling.
 *
 * This is a simplified version of handler.js that uses in-memory data
 * instead of warpgrid:shim/database-proxy. It demonstrates the same
 * HTTP routing and response patterns, allowing build verification and
 * HTTP round-trip testing with `jco serve`.
 *
 * When the warpgrid/pg bridge (US-403/404) is ready, the full handler.js
 * will replace this for end-to-end testing.
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

// ── HTTP Handler ────────────────────────────────────────────────────

function jsonResponse(data, status = 200, extraHeaders = {}) {
  const body = JSON.stringify(data);
  const headers = {
    "Content-Type": "application/json",
    ...extraHeaders,
  };

  // Include APP_NAME in response headers if available via environment
  // ComponentizeJS provides environ access when --enable environ is used
  const appName = globalThis.process?.env?.APP_NAME ?? "t4-ts-http-postgres";
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
