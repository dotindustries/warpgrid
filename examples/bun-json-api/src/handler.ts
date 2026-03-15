/**
 * Bun JSON API — minimal WarpGrid HTTP handler example.
 *
 * Routes:
 *   GET  /        — JSON greeting with runtime info and timestamp
 *   GET  /health  — health check
 *   POST /echo    — echoes back the request body as JSON
 *   GET  /env     — returns a filtered set of safe environment variables
 */

const SAFE_ENV_PREFIXES = ["APP_", "WARP_", "NODE_ENV", "PORT"];

function getEnv(key: string): string | undefined {
  if (typeof globalThis.Bun !== "undefined" && globalThis.Bun.env) {
    return globalThis.Bun.env[key];
  }
  if (typeof globalThis.process !== "undefined" && globalThis.process.env) {
    return globalThis.process.env[key];
  }
  return undefined;
}

function getAllEnv(): Record<string, string> {
  if (typeof globalThis.Bun !== "undefined" && globalThis.Bun.env) {
    return { ...globalThis.Bun.env } as Record<string, string>;
  }
  if (typeof globalThis.process !== "undefined" && globalThis.process.env) {
    return { ...globalThis.process.env } as Record<string, string>;
  }
  return {};
}

function jsonResponse(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function handleIndex(): Response {
  return jsonResponse({
    message: "Hello from WarpGrid!",
    runtime: "bun",
    timestamp: new Date().toISOString(),
  });
}

function handleHealth(): Response {
  return jsonResponse({ status: "ok" });
}

async function handleEcho(request: Request): Promise<Response> {
  let body: unknown;
  try {
    body = await request.json();
  } catch {
    return jsonResponse({ error: "Invalid JSON" }, 400);
  }

  return jsonResponse({ echo: body });
}

function handleEnv(): Response {
  const allEnv = getAllEnv();
  const filtered: Record<string, string> = {};

  for (const [key, value] of Object.entries(allEnv)) {
    const isSafe = SAFE_ENV_PREFIXES.some(
      (prefix) => key.startsWith(prefix)
    );
    if (isSafe) {
      filtered[key] = value;
    }
  }

  return jsonResponse({ env: filtered });
}

addEventListener("fetch", (event: FetchEvent) => {
  const url = new URL(event.request.url);
  const method = event.request.method;

  let responsePromise: Promise<Response>;

  if (url.pathname === "/" && method === "GET") {
    responsePromise = Promise.resolve(handleIndex());
  } else if (url.pathname === "/health" && method === "GET") {
    responsePromise = Promise.resolve(handleHealth());
  } else if (url.pathname === "/echo" && method === "POST") {
    responsePromise = handleEcho(event.request);
  } else if (url.pathname === "/env" && method === "GET") {
    responsePromise = Promise.resolve(handleEnv());
  } else {
    responsePromise = Promise.resolve(
      jsonResponse({ error: "Not Found" }, 404)
    );
  }

  event.respondWith(responsePromise);
});
