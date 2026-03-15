addEventListener("fetch", (event: any) => {
  event.respondWith(handleRequest(event.request));
});

async function handleRequest(request: Request): Promise<Response> {
  const url = new URL(request.url, "http://localhost");

  if (url.pathname === "/health") {
    return new Response(JSON.stringify({ status: "ok" }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  }

  // Echo request info as JSON
  const body = {
    method: request.method,
    uri: url.pathname,
  };

  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}
