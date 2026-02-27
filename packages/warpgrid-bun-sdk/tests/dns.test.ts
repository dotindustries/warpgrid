import { describe, test, expect } from "bun:test";
import { WarpGridDnsError } from "../src/errors.ts";
import type { DnsShim } from "../src/dns.ts";

// ── Mock Shim ────────────────────────────────────────────────────────

/** Minimal DnsShim mock for Wasm-mode testing. */
function createMockShim(
  records: Record<string, Array<{ address: string; isIpv6: boolean }>>,
): DnsShim {
  return {
    resolveAddress(hostname: string) {
      const result = records[hostname];
      if (!result) {
        throw new Error(`HostNotFound: ${hostname}`);
      }
      return result;
    },
  };
}

// ── Resolve interface tests ──────────────────────────────────────────

describe("@warpgrid/bun-sdk/dns", () => {
  describe("resolve() with mock shim (wasm mode)", () => {
    test("resolves hostname returning A records", async () => {
      const { resolve } = await import("../src/dns.ts");
      const shim = createMockShim({
        "db.internal": [{ address: "10.0.0.5", isIpv6: false }],
      });

      const result = await resolve("db.internal", "A", {
        shim,
        mode: "wasm",
      });
      expect(result).toEqual(["10.0.0.5"]);
    });

    test("resolves hostname returning AAAA records", async () => {
      const { resolve } = await import("../src/dns.ts");
      const shim = createMockShim({
        "api.internal": [
          { address: "10.0.0.1", isIpv6: false },
          { address: "::1", isIpv6: true },
        ],
      });

      const result = await resolve("api.internal", "AAAA", {
        shim,
        mode: "wasm",
      });
      expect(result).toEqual(["::1"]);
    });

    test("resolves with default rrtype (A) when omitted", async () => {
      const { resolve } = await import("../src/dns.ts");
      const shim = createMockShim({
        "cache.internal": [{ address: "10.0.0.9", isIpv6: false }],
      });

      const result = await resolve("cache.internal", undefined, {
        shim,
        mode: "wasm",
      });
      expect(result).toEqual(["10.0.0.9"]);
    });

    test("resolves multiple A records", async () => {
      const { resolve } = await import("../src/dns.ts");
      const shim = createMockShim({
        "multi.internal": [
          { address: "10.0.0.1", isIpv6: false },
          { address: "10.0.0.2", isIpv6: false },
          { address: "10.0.0.3", isIpv6: false },
        ],
      });

      const result = await resolve("multi.internal", "A", {
        shim,
        mode: "wasm",
      });
      expect(result).toEqual(["10.0.0.1", "10.0.0.2", "10.0.0.3"]);
    });

    test("CNAME returns all addresses (no type filtering)", async () => {
      const { resolve } = await import("../src/dns.ts");
      const shim = createMockShim({
        "alias.internal": [
          { address: "10.0.0.1", isIpv6: false },
          { address: "::1", isIpv6: true },
        ],
      });

      const result = await resolve("alias.internal", "CNAME", {
        shim,
        mode: "wasm",
      });
      expect(result).toEqual(["10.0.0.1", "::1"]);
    });

    test("throws WarpGridDnsError with ENOTFOUND for unknown hostname", async () => {
      const { resolve } = await import("../src/dns.ts");
      const shim = createMockShim({});

      try {
        await resolve("nonexistent.host", "A", { shim, mode: "wasm" });
        expect(true).toBe(false); // should not reach
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDnsError);
        const dnsErr = err as WarpGridDnsError;
        expect(dnsErr.code).toBe("ENOTFOUND");
        expect(dnsErr.message).toContain("nonexistent.host");
      }
    });

    test("returns empty array when no matching record type", async () => {
      const { resolve } = await import("../src/dns.ts");
      const shim = createMockShim({
        "ipv4only.internal": [{ address: "10.0.0.1", isIpv6: false }],
      });

      const result = await resolve("ipv4only.internal", "AAAA", {
        shim,
        mode: "wasm",
      });
      expect(result).toEqual([]);
    });
  });

  describe("resolve() with timeout", () => {
    test("rejects with ETIMEOUT for native mode when timeout exceeded", async () => {
      const { resolve } = await import("../src/dns.ts");

      // Use native mode with a very short timeout and a hostname that
      // requires real DNS — native DNS is async and can be timed out
      try {
        await resolve(
          "this-domain-definitely-does-not-exist.invalid",
          "A",
          { mode: "native", timeout: 1 },
        );
        expect(true).toBe(false); // should not reach
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDnsError);
        const dnsErr = err as WarpGridDnsError;
        // Either ETIMEOUT (timeout fires first) or ENOTFOUND (DNS fails first)
        expect(["ETIMEOUT", "ENOTFOUND"]).toContain(dnsErr.code);
      }
    });

    test("wasm mode succeeds within timeout", async () => {
      const { resolve } = await import("../src/dns.ts");
      const shim = createMockShim({
        "fast.host": [{ address: "10.0.0.1", isIpv6: false }],
      });

      const result = await resolve("fast.host", "A", {
        shim,
        mode: "wasm",
        timeout: 5000,
      });
      expect(result).toEqual(["10.0.0.1"]);
    });

    test("native mode succeeds within generous timeout", async () => {
      const { resolve } = await import("../src/dns.ts");

      const result = await resolve("localhost", "A", {
        mode: "native",
        timeout: 5000,
      });
      expect(result.length).toBeGreaterThanOrEqual(1);
    });
  });

  describe("resolve() in native mode", () => {
    test("resolves localhost to at least one address", async () => {
      const { resolve } = await import("../src/dns.ts");

      // Native mode — uses Bun's built-in DNS
      const result = await resolve("localhost", "A", { mode: "native" });
      expect(result.length).toBeGreaterThanOrEqual(1);
      // Should contain 127.0.0.1 or similar loopback
      expect(
        result.some(
          (addr) => addr === "127.0.0.1" || addr.startsWith("127."),
        ),
      ).toBe(true);
    });

    test("rejects with ENOTFOUND for non-existent domain in native mode", async () => {
      const { resolve } = await import("../src/dns.ts");

      try {
        await resolve(
          "this-domain-definitely-does-not-exist.invalid",
          "A",
          { mode: "native" },
        );
        expect(true).toBe(false);
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDnsError);
        expect((err as WarpGridDnsError).code).toBe("ENOTFOUND");
      }
    });
  });

  describe("DnsShim interface", () => {
    test("shim resolveAddress returns typed records", () => {
      const shim = createMockShim({
        "test.host": [
          { address: "192.168.1.1", isIpv6: false },
          { address: "fe80::1", isIpv6: true },
        ],
      });

      const records = shim.resolveAddress("test.host");
      expect(records).toHaveLength(2);
      expect(records[0]).toEqual({ address: "192.168.1.1", isIpv6: false });
      expect(records[1]).toEqual({ address: "fe80::1", isIpv6: true });
    });

    test("shim resolveAddress throws for unknown host", () => {
      const shim = createMockShim({});

      expect(() => shim.resolveAddress("unknown")).toThrow("HostNotFound");
    });
  });

  describe("resolveShim()", () => {
    test("returns provided shim from options", async () => {
      const { resolveShim } = await import("../src/dns.ts");
      const shim = createMockShim({});

      expect(resolveShim({ shim })).toBe(shim);
    });

    test("returns undefined when no shim and no globals", async () => {
      const { resolveShim } = await import("../src/dns.ts");
      const g = globalThis as Record<string, unknown>;
      const saved = g.warpgrid;
      delete g.warpgrid;

      try {
        expect(resolveShim({})).toBeUndefined();
      } finally {
        if (saved !== undefined) {
          g.warpgrid = saved;
        }
      }
    });

    test("resolves shim from globalThis.warpgrid.dns", async () => {
      const { resolveShim } = await import("../src/dns.ts");
      const shim = createMockShim({});

      const g = globalThis as Record<string, unknown>;
      const saved = g.warpgrid;
      g.warpgrid = { dns: shim };

      try {
        expect(resolveShim({})).toBe(shim);
      } finally {
        if (saved !== undefined) {
          g.warpgrid = saved;
        } else {
          delete g.warpgrid;
        }
      }
    });
  });
});
