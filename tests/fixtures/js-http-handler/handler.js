/**
 * WarpGrid test fixture: minimal HTTP handler for ComponentizeJS verification.
 *
 * Uses the web-standard fetch event pattern. When componentized with
 * `--enable http --enable fetch-event`, this becomes a WASI HTTP
 * incoming-handler component that responds with "ok" to all requests.
 */
addEventListener("fetch", (event) =>
  event.respondWith(new Response("ok"))
);
