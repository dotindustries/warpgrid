/**
 * WarpGrid test fixture: HTTP handler that uses database proxy.
 *
 * Demonstrates warpgrid.database.connect() usage in a componentized
 * handler. When componentized with the database-proxy WIT import,
 * the warpgrid global provides proxied database connectivity.
 *
 * Note: The warpgrid global is set up by the warp pack pipeline,
 * which imports WIT bindings and calls setupWarpGridGlobal().
 */

// In production, this import is injected by warp pack.
// For this test fixture, warpgrid global would be set up externally.

addEventListener("fetch", (event) => {
  try {
    // This would be called after warpgrid global is set up by the pipeline
    if (typeof globalThis.warpgrid === "undefined") {
      event.respondWith(
        new Response("warpgrid global not available", { status: 503 })
      );
      return;
    }

    const conn = warpgrid.database.connect({
      host: "db.test.warp.local",
      port: 5432,
      database: "testdb",
      username: "testuser",
    });

    // Send a simple Postgres query (SELECT 1)
    const query = new TextEncoder().encode("SELECT 1");
    conn.send(query);

    // Receive response
    const result = conn.recv(4096);
    conn.close();

    const body = new TextDecoder().decode(result);
    event.respondWith(
      new Response(JSON.stringify({ status: "ok", data: body }), {
        headers: { "content-type": "application/json" },
      })
    );
  } catch (err) {
    event.respondWith(
      new Response(JSON.stringify({ error: String(err) }), {
        status: 500,
        headers: { "content-type": "application/json" },
      })
    );
  }
});
