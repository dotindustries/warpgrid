/**
 * US-408: Integration test — DNS, env vars, and file read combined.
 *
 * Validates that all three WarpGrid shims (DNS, process.env, filesystem)
 * work together in a single handler, including partial failure handling.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { setupWarpGridGlobal } from "../global.js";
import { WarpGridDNSError, WarpGridFSError } from "../errors.js";
import type { DnsBindings, FilesystemBindings, DatabaseProxyBindings } from "../types.js";

// ── Test handler ──────────────────────────────────────────────────
// Simulates a realistic handler that reads env, resolves DNS, reads
// a timezone file, and returns all results as JSON. Handles partial
// failures gracefully — each shim call is independent.

interface HandlerResult {
  env: { SERVICE_HOST: string | undefined };
  dns: { hostname: string; addresses: string[] } | { hostname: string; error: string };
  fs: { path: string; content: string } | { path: string; error: string };
}

function handleCombinedRequest(): HandlerResult {
  const serviceHost = globalThis.process?.env?.SERVICE_HOST;
  const hostname = serviceHost ?? "unknown";
  const tzPath = "/usr/share/zoneinfo/UTC";

  // DNS resolution — capture success or error
  let dnsResult: HandlerResult["dns"];
  try {
    if (!globalThis.warpgrid?.dns) {
      throw new Error("DNS shim not available");
    }
    const addresses = globalThis.warpgrid.dns.resolve(hostname);
    dnsResult = { hostname, addresses };
  } catch (err) {
    dnsResult = { hostname, error: err instanceof Error ? err.message : String(err) };
  }

  // Filesystem read — capture success or error
  let fsResult: HandlerResult["fs"];
  try {
    if (!globalThis.warpgrid?.fs) {
      throw new Error("Filesystem shim not available");
    }
    const content = globalThis.warpgrid.fs.readFile(tzPath, "utf-8");
    fsResult = { path: tzPath, content: content as string };
  } catch (err) {
    fsResult = { path: tzPath, error: err instanceof Error ? err.message : String(err) };
  }

  return {
    env: { SERVICE_HOST: serviceHost },
    dns: dnsResult,
    fs: fsResult,
  };
}

// ── Mock factories ────────────────────────────────────────────────

function createMockDnsBindings(
  overrides: Partial<DnsBindings> = {},
): DnsBindings {
  return {
    resolveAddress: vi.fn().mockReturnValue([
      { address: "10.0.1.42", family: "ipv4", ttl: 30 },
      { address: "10.0.1.43", family: "ipv4", ttl: 30 },
    ]),
    ...overrides,
  };
}

function createMockFsBindings(
  overrides: Partial<FilesystemBindings> = {},
): FilesystemBindings {
  return {
    openVirtual: vi.fn().mockReturnValue(1n),
    readVirtual: vi.fn().mockReturnValue(
      new TextEncoder().encode("TZif2\x00\x00\x00UTC timezone data"),
    ),
    closeVirtual: vi.fn(),
    ...overrides,
  };
}

function createMockDbBindings(): DatabaseProxyBindings {
  return {
    connect: vi.fn().mockReturnValue(1n),
    send: vi.fn().mockReturnValue(0),
    recv: vi.fn().mockReturnValue(new Uint8Array([])),
    close: vi.fn(),
  };
}

// ── Tests ─────────────────────────────────────────────────────────

describe("US-408: Integration — DNS, env vars, and file read combined", () => {
  let savedProcess: typeof globalThis.process;

  beforeEach(() => {
    // Save and reset process.env
    savedProcess = globalThis.process;
    globalThis.process = { env: { SERVICE_HOST: "db.staging.warp.local" } } as typeof globalThis.process;

    // Clean warpgrid global
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (globalThis as any).warpgrid;
  });

  afterEach(() => {
    globalThis.process = savedProcess;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (globalThis as any).warpgrid;
  });

  describe("all shims succeed", () => {
    it("returns combined JSON with env, DNS addresses, and file content", () => {
      setupWarpGridGlobal({
        database: createMockDbBindings(),
        dns: createMockDnsBindings(),
        fs: createMockFsBindings(),
      });

      const result = handleCombinedRequest();

      expect(result.env.SERVICE_HOST).toBe("db.staging.warp.local");
      expect("addresses" in result.dns).toBe(true);
      if ("addresses" in result.dns) {
        expect(result.dns.hostname).toBe("db.staging.warp.local");
        expect(result.dns.addresses).toEqual(["10.0.1.42", "10.0.1.43"]);
      }
      expect("content" in result.fs).toBe(true);
      if ("content" in result.fs) {
        expect(result.fs.path).toBe("/usr/share/zoneinfo/UTC");
        expect(result.fs.content).toContain("TZif2");
      }
    });

    it("DNS bindings receive the correct hostname from process.env", () => {
      const dnsBindings = createMockDnsBindings();
      setupWarpGridGlobal({
        database: createMockDbBindings(),
        dns: dnsBindings,
        fs: createMockFsBindings(),
      });

      handleCombinedRequest();

      expect(dnsBindings.resolveAddress).toHaveBeenCalledWith("db.staging.warp.local");
    });

    it("FS bindings read the timezone file path", () => {
      const fsBindings = createMockFsBindings();
      setupWarpGridGlobal({
        database: createMockDbBindings(),
        dns: createMockDnsBindings(),
        fs: fsBindings,
      });

      handleCombinedRequest();

      expect(fsBindings.openVirtual).toHaveBeenCalledWith("/usr/share/zoneinfo/UTC");
    });

    it("FS bindings close the handle after reading", () => {
      const fsBindings = createMockFsBindings();
      setupWarpGridGlobal({
        database: createMockDbBindings(),
        dns: createMockDnsBindings(),
        fs: fsBindings,
      });

      handleCombinedRequest();

      expect(fsBindings.closeVirtual).toHaveBeenCalledWith(1n);
    });
  });

  describe("partial failures — DNS fails, env and file still succeed", () => {
    it("returns env and file results when DNS throws", () => {
      const dnsBindings = createMockDnsBindings({
        resolveAddress: vi.fn().mockImplementation(() => {
          throw "host not found: db.staging.warp.local";
        }),
      });

      setupWarpGridGlobal({
        database: createMockDbBindings(),
        dns: dnsBindings,
        fs: createMockFsBindings(),
      });

      const result = handleCombinedRequest();

      // Env still works
      expect(result.env.SERVICE_HOST).toBe("db.staging.warp.local");

      // DNS failed with error
      expect("error" in result.dns).toBe(true);
      if ("error" in result.dns) {
        expect(result.dns.error).toContain("db.staging.warp.local");
      }

      // FS still works
      expect("content" in result.fs).toBe(true);
      if ("content" in result.fs) {
        expect(result.fs.content).toContain("TZif2");
      }
    });
  });

  describe("partial failures — FS fails, env and DNS still succeed", () => {
    it("returns env and DNS results when filesystem throws", () => {
      const fsBindings = createMockFsBindings({
        openVirtual: vi.fn().mockImplementation(() => {
          throw "not a virtual path: /usr/share/zoneinfo/UTC";
        }),
      });

      setupWarpGridGlobal({
        database: createMockDbBindings(),
        dns: createMockDnsBindings(),
        fs: fsBindings,
      });

      const result = handleCombinedRequest();

      // Env still works
      expect(result.env.SERVICE_HOST).toBe("db.staging.warp.local");

      // DNS still works
      expect("addresses" in result.dns).toBe(true);
      if ("addresses" in result.dns) {
        expect(result.dns.addresses).toEqual(["10.0.1.42", "10.0.1.43"]);
      }

      // FS failed with error
      expect("error" in result.fs).toBe(true);
      if ("error" in result.fs) {
        expect(result.fs.error).toContain("/usr/share/zoneinfo/UTC");
      }
    });
  });

  describe("partial failures — both DNS and FS fail, env still works", () => {
    it("returns env result when both DNS and FS throw", () => {
      const dnsBindings = createMockDnsBindings({
        resolveAddress: vi.fn().mockImplementation(() => {
          throw "network unreachable";
        }),
      });
      const fsBindings = createMockFsBindings({
        openVirtual: vi.fn().mockImplementation(() => {
          throw "permission denied";
        }),
      });

      setupWarpGridGlobal({
        database: createMockDbBindings(),
        dns: dnsBindings,
        fs: fsBindings,
      });

      const result = handleCombinedRequest();

      // Env always works (it's just reading process.env)
      expect(result.env.SERVICE_HOST).toBe("db.staging.warp.local");

      // Both DNS and FS have errors
      expect("error" in result.dns).toBe(true);
      expect("error" in result.fs).toBe(true);
    });
  });

  describe("missing env variable", () => {
    it("uses undefined SERVICE_HOST when env var not set", () => {
      globalThis.process = { env: {} } as typeof globalThis.process;

      setupWarpGridGlobal({
        database: createMockDbBindings(),
        dns: createMockDnsBindings(),
        fs: createMockFsBindings(),
      });

      const result = handleCombinedRequest();

      expect(result.env.SERVICE_HOST).toBeUndefined();
      // DNS resolution uses "unknown" as fallback hostname
      expect("addresses" in result.dns || "error" in result.dns).toBe(true);
    });
  });

  describe("shims not available", () => {
    it("returns errors for DNS and FS when shims are not initialized", () => {
      // Set up warpgrid without DNS or FS
      setupWarpGridGlobal({
        database: createMockDbBindings(),
      });

      const result = handleCombinedRequest();

      // Env still works
      expect(result.env.SERVICE_HOST).toBe("db.staging.warp.local");

      // Both DNS and FS report "not available"
      expect("error" in result.dns).toBe(true);
      if ("error" in result.dns) {
        expect(result.dns.error).toContain("not available");
      }
      expect("error" in result.fs).toBe(true);
      if ("error" in result.fs) {
        expect(result.fs.error).toContain("not available");
      }
    });
  });
});

describe("WarpGridDns unit tests", () => {
  afterEach(() => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (globalThis as any).warpgrid;
  });

  it("resolves a hostname to IP addresses", () => {
    setupWarpGridGlobal({
      dns: createMockDnsBindings(),
    });

    const addresses = globalThis.warpgrid!.dns!.resolve("db.internal");
    expect(addresses).toEqual(["10.0.1.42", "10.0.1.43"]);
  });

  it("throws WarpGridDNSError for empty hostname", () => {
    setupWarpGridGlobal({
      dns: createMockDnsBindings(),
    });

    expect(() => globalThis.warpgrid!.dns!.resolve("")).toThrow(WarpGridDNSError);
  });

  it("throws WarpGridDNSError when binding fails", () => {
    setupWarpGridGlobal({
      dns: createMockDnsBindings({
        resolveAddress: vi.fn().mockImplementation(() => {
          throw "ENOTFOUND";
        }),
      }),
    });

    expect(() => globalThis.warpgrid!.dns!.resolve("bad.host")).toThrow(WarpGridDNSError);
  });

  it("includes hostname in the error object", () => {
    setupWarpGridGlobal({
      dns: createMockDnsBindings({
        resolveAddress: vi.fn().mockImplementation(() => {
          throw "ENOTFOUND";
        }),
      }),
    });

    try {
      globalThis.warpgrid!.dns!.resolve("bad.host");
      expect.fail("should have thrown");
    } catch (err) {
      expect(err).toBeInstanceOf(WarpGridDNSError);
      expect((err as WarpGridDNSError).hostname).toBe("bad.host");
    }
  });
});

describe("WarpGridFs unit tests", () => {
  afterEach(() => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (globalThis as any).warpgrid;
  });

  it("reads a virtual file as Uint8Array", () => {
    setupWarpGridGlobal({
      fs: createMockFsBindings(),
    });

    const data = globalThis.warpgrid!.fs!.readFile("/etc/resolv.conf");
    expect(data).toBeInstanceOf(Uint8Array);
  });

  it("reads a virtual file as UTF-8 string when encoding specified", () => {
    setupWarpGridGlobal({
      fs: createMockFsBindings({
        readVirtual: vi.fn().mockReturnValue(
          new TextEncoder().encode("nameserver 127.0.0.1"),
        ),
      }),
    });

    const data = globalThis.warpgrid!.fs!.readFile("/etc/resolv.conf", "utf-8");
    expect(typeof data).toBe("string");
    expect(data).toBe("nameserver 127.0.0.1");
  });

  it("throws WarpGridFSError for empty path", () => {
    setupWarpGridGlobal({
      fs: createMockFsBindings(),
    });

    expect(() => globalThis.warpgrid!.fs!.readFile("")).toThrow(WarpGridFSError);
  });

  it("throws WarpGridFSError when open fails", () => {
    setupWarpGridGlobal({
      fs: createMockFsBindings({
        openVirtual: vi.fn().mockImplementation(() => {
          throw "not a virtual path: /nonexistent";
        }),
      }),
    });

    expect(() => globalThis.warpgrid!.fs!.readFile("/nonexistent")).toThrow(WarpGridFSError);
  });

  it("includes path in the error object", () => {
    setupWarpGridGlobal({
      fs: createMockFsBindings({
        openVirtual: vi.fn().mockImplementation(() => {
          throw "not a virtual path";
        }),
      }),
    });

    try {
      globalThis.warpgrid!.fs!.readFile("/bad/path");
      expect.fail("should have thrown");
    } catch (err) {
      expect(err).toBeInstanceOf(WarpGridFSError);
      expect((err as WarpGridFSError).path).toBe("/bad/path");
    }
  });

  it("closes the file handle even when read fails", () => {
    const fsBindings = createMockFsBindings({
      readVirtual: vi.fn().mockImplementation(() => {
        throw "read error";
      }),
    });
    setupWarpGridGlobal({ fs: fsBindings });

    try {
      globalThis.warpgrid!.fs!.readFile("/etc/resolv.conf");
    } catch {
      // Expected
    }

    expect(fsBindings.closeVirtual).toHaveBeenCalledWith(1n);
  });
});
