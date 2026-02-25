import { describe, it, expect, vi, afterEach } from "vitest";
import { setupWarpGridGlobal } from "../global.js";
import { WarpGridError } from "../errors.js";
import type { DatabaseProxyBindings } from "../types.js";

function createMockBindings(): DatabaseProxyBindings {
  return {
    connect: vi.fn().mockReturnValue(42n),
    send: vi.fn().mockReturnValue(5),
    recv: vi.fn().mockReturnValue(new Uint8Array([1, 2, 3])),
    close: vi.fn(),
  };
}

describe("setupWarpGridGlobal()", () => {
  afterEach(() => {
    // Clean up globalThis.warpgrid between tests
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (globalThis as any).warpgrid;
  });

  it("sets globalThis.warpgrid with a database property", () => {
    setupWarpGridGlobal(createMockBindings());

    expect(globalThis.warpgrid).toBeDefined();
    expect(globalThis.warpgrid.database).toBeDefined();
    expect(typeof globalThis.warpgrid.database.connect).toBe("function");
  });

  it("warpgrid.database.connect() returns a connection object", () => {
    setupWarpGridGlobal(createMockBindings());

    const conn = warpgrid.database.connect({
      host: "localhost",
      port: 5432,
      database: "testdb",
      username: "admin",
    });

    expect(conn).toBeDefined();
    expect(typeof conn.send).toBe("function");
    expect(typeof conn.recv).toBe("function");
    expect(typeof conn.close).toBe("function");
  });

  it("delegates connect() to the WIT bindings", () => {
    const bindings = createMockBindings();
    setupWarpGridGlobal(bindings);

    warpgrid.database.connect({
      host: "db.internal",
      port: 5432,
      database: "mydb",
      username: "appuser",
      password: "secret",
    });

    expect(bindings.connect).toHaveBeenCalledWith({
      host: "db.internal",
      port: 5432,
      database: "mydb",
      user: "appuser",
      password: "secret",
    });
  });

  it("delegates send/recv/close to the WIT bindings through the connection", () => {
    const bindings = createMockBindings();
    setupWarpGridGlobal(bindings);

    const conn = warpgrid.database.connect({
      host: "localhost",
      port: 5432,
      database: "testdb",
      username: "admin",
    });

    const data = new Uint8Array([0x51]);
    conn.send(data);
    expect(bindings.send).toHaveBeenCalledWith(42n, data);

    conn.recv(1024);
    expect(bindings.recv).toHaveBeenCalledWith(42n, 1024);

    conn.close();
    expect(bindings.close).toHaveBeenCalledWith(42n);
  });

  it("throws WarpGridError when called twice", () => {
    setupWarpGridGlobal(createMockBindings());

    expect(() => setupWarpGridGlobal(createMockBindings())).toThrow(
      WarpGridError,
    );
    expect(() => setupWarpGridGlobal(createMockBindings())).toThrow(
      /already initialized/,
    );
  });
});
