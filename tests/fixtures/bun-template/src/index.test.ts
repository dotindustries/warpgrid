import { describe, expect, it } from "bun:test";
import handler from "./index";

describe("handler", () => {
  it("returns health status", async () => {
    const request = new Request("http://localhost/health");
    const response = await handler.fetch(request);

    expect(response.status).toBe(200);
    expect(response.headers.get("Content-Type")).toBe("application/json");

    const body = await response.json();
    expect(body).toEqual({ status: "ok" });
  });

  it("echoes request method and path", async () => {
    const request = new Request("http://localhost/test-path", {
      method: "POST",
    });
    const response = await handler.fetch(request);

    expect(response.status).toBe(200);

    const body = await response.json();
    expect(body).toEqual({ method: "POST", path: "/test-path" });
  });
});
