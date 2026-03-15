import type { WarpGridHandler } from "@warpgrid/bun-sdk";

// Database access (PostgreSQL):
// import { createPool } from "@warpgrid/bun-sdk/postgres";
// const pool = createPool({ database: "mydb" });
// const result = await pool.query("SELECT * FROM users WHERE id = $1", [userId]);

// DNS resolution:
// import { resolve } from "@warpgrid/bun-sdk/dns";
// const addresses = await resolve("example.com", "A");

// Virtual filesystem:
// import { readTextFile, writeFile } from "@warpgrid/bun-sdk/fs";
// const content = await readTextFile("/data/config.json");

const handler: WarpGridHandler = {
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);

    if (url.pathname === "/health") {
      return new Response(JSON.stringify({ status: "ok" }), {
        headers: { "Content-Type": "application/json" },
      });
    }

    // Echo endpoint: return request method and path
    return new Response(
      JSON.stringify({
        method: request.method,
        path: url.pathname,
      }),
      {
        headers: { "Content-Type": "application/json" },
      },
    );
  },
};

export default handler;
