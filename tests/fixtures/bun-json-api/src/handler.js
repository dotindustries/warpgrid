/**
 * Bun JSON API Handler — standalone single-file handler for ComponentizeJS.
 *
 * US-605: Realistic Bun handler with JSON parsing, validation, and transformation.
 *
 * This file is self-contained (no imports) because ComponentizeJS
 * componentizes a single JS file. The logic here mirrors handler-logic.ts
 * which is tested separately via `bun test`.
 *
 * Routes:
 *   POST /transform  — parse JSON, validate fields, transform data
 *   POST /validate   — validate a payload against a named schema
 *   GET  /health     — health check
 *   *                — 404 Not Found
 */

// ── Helpers ──────────────────────────────────────────────────────────

function slugify(name) {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9\s-]/g, "")
    .replace(/\s+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "");
}

function extractDomain(email) {
  const atIndex = email.indexOf("@");
  return atIndex >= 0 ? email.slice(atIndex + 1) : "";
}

function isValidEmail(email) {
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email);
}

function jsonResponse(status, body) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

// ── Route: POST /transform ───────────────────────────────────────────

async function handleTransform(request) {
  let body;
  try {
    body = await request.json();
  } catch {
    return jsonResponse(400, {
      success: false,
      error: "Invalid JSON: failed to parse request body",
    });
  }

  const errors = [];

  if (!body.name || typeof body.name !== "string") {
    errors.push("name is required and must be a string");
  }
  if (!body.email || typeof body.email !== "string") {
    errors.push("email is required and must be a string");
  } else if (!isValidEmail(body.email)) {
    errors.push("email must be a valid email address");
  }

  if (errors.length > 0) {
    return jsonResponse(400, { success: false, error: errors.join("; ") });
  }

  const tags = Array.isArray(body.tags) ? body.tags : [];

  return jsonResponse(200, {
    success: true,
    data: {
      displayName: body.name.trim().toUpperCase(),
      emailDomain: extractDomain(body.email),
      tagCount: tags.length,
      slug: slugify(body.name),
    },
  });
}

// ── Route: POST /validate ────────────────────────────────────────────

async function handleValidate(request) {
  let body;
  try {
    body = await request.json();
  } catch {
    return jsonResponse(400, {
      success: false,
      error: "Invalid JSON: failed to parse request body",
    });
  }

  const schema = body.schema;
  const payload = body.payload;

  if (!schema || typeof schema !== "string") {
    return jsonResponse(400, { success: false, error: "schema is required" });
  }

  if (!payload || typeof payload !== "object") {
    return jsonResponse(400, { success: false, error: "payload is required" });
  }

  if (schema === "user") {
    const validationErrors = [];

    if (!payload.name || typeof payload.name !== "string") {
      validationErrors.push("name is required");
    }
    if (!payload.email || typeof payload.email !== "string") {
      validationErrors.push("email is required");
    } else if (!isValidEmail(payload.email)) {
      validationErrors.push("email must be a valid email address");
    }
    if (payload.age !== undefined) {
      if (typeof payload.age !== "number" || payload.age <= 0) {
        validationErrors.push("age must be a positive number");
      }
    }

    return jsonResponse(200, {
      success: true,
      data: {
        valid: validationErrors.length === 0,
        errors: validationErrors,
      },
    });
  }

  return jsonResponse(400, {
    success: false,
    error: "Unknown schema: " + schema,
  });
}

// ── Route: GET /health ───────────────────────────────────────────────

function handleHealth() {
  return jsonResponse(200, { success: true, data: { status: "ok" } });
}

// ── Fetch Event Listener ─────────────────────────────────────────────

addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  const method = event.request.method;

  let responsePromise;

  if (url.pathname === "/transform" && method === "POST") {
    responsePromise = handleTransform(event.request);
  } else if (url.pathname === "/validate" && method === "POST") {
    responsePromise = handleValidate(event.request);
  } else if (url.pathname === "/health") {
    responsePromise = Promise.resolve(handleHealth());
  } else if (
    (url.pathname === "/transform" || url.pathname === "/validate") &&
    method !== "POST"
  ) {
    responsePromise = Promise.resolve(
      jsonResponse(405, {
        success: false,
        error: "Method Not Allowed: use POST",
      })
    );
  } else {
    responsePromise = Promise.resolve(
      jsonResponse(404, { success: false, error: "Not Found" })
    );
  }

  event.respondWith(responsePromise);
});
