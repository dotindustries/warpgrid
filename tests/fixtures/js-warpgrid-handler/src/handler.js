/**
 * Test fixture: JS handler exercising WarpGrid shim globals.
 *
 * Demonstrates:
 * - warpgrid.database.connect() (database proxy)
 * - warpgrid.dns.resolve() (DNS resolution)
 * - process.env (environment variables)
 * - warpgrid.fs.readFile() (virtual filesystem)
 *
 * These globals are auto-injected by `warp pack --lang js` via the WarpGrid
 * shim prelude. The handler itself uses the high-level globals, not raw WIT imports.
 */

addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  const method = event.request.method;

  let responsePromise;

  if (url.pathname === "/health") {
    responsePromise = Promise.resolve(handleHealth());
  } else if (url.pathname === "/users" && method === "GET") {
    responsePromise = handleGetUsers();
  } else if (url.pathname === "/dns" && method === "GET") {
    responsePromise = handleDnsResolve(url);
  } else {
    responsePromise = Promise.resolve(
      new Response(JSON.stringify({ error: "Not Found" }), {
        status: 404,
        headers: { "Content-Type": "application/json" },
      })
    );
  }

  event.respondWith(responsePromise);
});

function handleHealth() {
  const appName = process.env.APP_NAME ?? "js-warpgrid-handler";
  return new Response(
    JSON.stringify({ status: "ok", app: appName }),
    {
      status: 200,
      headers: {
        "Content-Type": "application/json",
        "X-App-Name": appName,
      },
    }
  );
}

async function handleGetUsers() {
  // Use warpgrid.database global (injected by prelude)
  if (!globalThis.warpgrid?.database) {
    return new Response(
      JSON.stringify({ error: "Database shim not available" }),
      { status: 503, headers: { "Content-Type": "application/json" } }
    );
  }

  try {
    const handle = globalThis.warpgrid.database.connect({
      host: process.env.DB_HOST ?? "db.test.warp.local",
      port: parseInt(process.env.DB_PORT ?? "5432", 10),
      database: process.env.DB_NAME ?? "testdb",
      user: process.env.DB_USER ?? "testuser",
    });

    // In a real handler, we'd do wire protocol here via send/recv
    globalThis.warpgrid.database.close(handle);

    return new Response(JSON.stringify({ users: [] }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  } catch (err) {
    return new Response(
      JSON.stringify({ error: String(err) }),
      { status: 500, headers: { "Content-Type": "application/json" } }
    );
  }
}

async function handleDnsResolve(url) {
  const hostname = url.searchParams.get("hostname") ?? "localhost";

  if (!globalThis.warpgrid?.dns) {
    return new Response(
      JSON.stringify({ error: "DNS shim not available" }),
      { status: 503, headers: { "Content-Type": "application/json" } }
    );
  }

  try {
    const addresses = globalThis.warpgrid.dns.resolve(hostname);
    return new Response(
      JSON.stringify({ hostname, addresses }),
      { status: 200, headers: { "Content-Type": "application/json" } }
    );
  } catch (err) {
    return new Response(
      JSON.stringify({ error: String(err) }),
      { status: 500, headers: { "Content-Type": "application/json" } }
    );
  }
}
