import { describe, it, expect, vi, afterEach } from "vitest";
import { setupWarpGridGlobal } from "../global.js";
import { WarpGridError } from "../errors.js";
import type { DatabaseProxyBindings, DnsBindings, FilesystemBindings } from "../types.js";

function createMockDbBindings(): DatabaseProxyBindings {
  return {
    connect: vi.fn().mockReturnValue(42n),
    send: vi.fn().mockReturnValue(5),
    recv: vi.fn().mockReturnValue(new Uint8Array([1, 2, 3])),
    close: vi.fn(),
  };
}

function createMockDnsBindings(): DnsBindings {
  return {
    resolveAddress: vi.fn().mockReturnValue([
      { address: "10.0.0.1", family: "ipv4", ttl: 30 },
    ]),
  };
}

function createMockFsBindings(): FilesystemBindings {
  return {
    openVirtual: vi.fn().mockReturnValue(1n),
    readVirtual: vi.fn().mockReturnValue(new Uint8Array([72, 101, 108, 108, 111])),
    closeVirtual: vi.fn(),
  };
}

describe("setupWarpGridGlobal()", () => {
  afterEach(() => {
    // Clean up globalThis.warpgrid between tests
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (globalThis as any).warpgrid;
  });

  it("sets globalThis.warpgrid with a database property (legacy form)", () => {
    setupWarpGridGlobal(createMockDbBindings());

    expect(globalThis.warpgrid).toBeDefined();
    expect(globalThis.warpgrid.database).toBeDefined();
    expect(typeof globalThis.warpgrid.database!.connect).toBe("function");
  });

  it("warpgrid.database.connect() returns a connection object (legacy form)", () => {
    setupWarpGridGlobal(createMockDbBindings());

    const conn = warpgrid.database!.connect({
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

  it("delegates connect() to the WIT bindings (legacy form)", () => {
    const bindings = createMockDbBindings();
    setupWarpGridGlobal(bindings);

    warpgrid.database!.connect({
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

  it("delegates send/recv/close to the WIT bindings through the connection (legacy form)", () => {
    const bindings = createMockDbBindings();
    setupWarpGridGlobal(bindings);

    const conn = warpgrid.database!.connect({
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
    setupWarpGridGlobal(createMockDbBindings());

    expect(() => setupWarpGridGlobal(createMockDbBindings())).toThrow(
      WarpGridError,
    );
    expect(() => setupWarpGridGlobal(createMockDbBindings())).toThrow(
      /already initialized/,
    );
  });

  describe("config object form", () => {
    it("sets up database, dns, and fs when all provided", () => {
      setupWarpGridGlobal({
        database: createMockDbBindings(),
        dns: createMockDnsBindings(),
        fs: createMockFsBindings(),
      });

      expect(globalThis.warpgrid.database).toBeDefined();
      expect(globalThis.warpgrid.dns).toBeDefined();
      expect(globalThis.warpgrid.fs).toBeDefined();
    });

    it("sets up only provided shims", () => {
      setupWarpGridGlobal({
        dns: createMockDnsBindings(),
      });

      expect(globalThis.warpgrid.database).toBeUndefined();
      expect(globalThis.warpgrid.dns).toBeDefined();
      expect(globalThis.warpgrid.fs).toBeUndefined();
    });

    it("dns.resolve returns IP address strings", () => {
      setupWarpGridGlobal({
        dns: createMockDnsBindings(),
      });

      const addresses = globalThis.warpgrid.dns!.resolve("test.local");
      expect(addresses).toEqual(["10.0.0.1"]);
    });

    it("fs.readFile returns file content", () => {
      setupWarpGridGlobal({
        fs: createMockFsBindings(),
      });

      const data = globalThis.warpgrid.fs!.readFile("/etc/hosts");
      expect(data).toBeInstanceOf(Uint8Array);
      expect((data as Uint8Array).length).toBe(5); // "Hello"
    });
  });
});
