import { describe, it, expect } from "bun:test";
import type { WarpGridHandler } from "./index";

describe("WarpGridHandler", () => {
  it("accepts a handler returning a synchronous Response", () => {
    const handler: WarpGridHandler = {
      fetch(_request: Request): Response {
        return new Response("ok");
      },
    };

    expect(handler.fetch).toBeFunction();
  });

  it("accepts a handler returning a Promise<Response>", async () => {
    const handler: WarpGridHandler = {
      async fetch(_request: Request): Promise<Response> {
        return new Response("async ok");
      },
    };

    const response = await handler.fetch(new Request("http://localhost/"));
    expect(response).toBeInstanceOf(Response);
    expect(await response.text()).toBe("async ok");
  });

  it("accepts a handler with an optional init lifecycle hook", async () => {
    let initialized = false;

    const handler: WarpGridHandler = {
      async init(): Promise<void> {
        initialized = true;
      },
      async fetch(_request: Request): Promise<Response> {
        return new Response(initialized ? "ready" : "not ready");
      },
    };

    expect(handler.init).toBeFunction();
    await handler.init!();
    expect(initialized).toBe(true);

    const response = await handler.fetch(new Request("http://localhost/"));
    expect(await response.text()).toBe("ready");
  });

  it("works without the optional init hook", () => {
    const handler: WarpGridHandler = {
      fetch(_request: Request): Response {
        return new Response("no init");
      },
    };

    expect(handler.init).toBeUndefined();
  });
});
