import { describe, it, expect, vi, beforeEach } from "vitest";
import { WarpGridDns } from "../dns.js";
import { WarpGridDNSError } from "../errors.js";
import type { DnsBindings } from "../types.js";

function createMockBindings(
  overrides: Partial<DnsBindings> = {},
): DnsBindings {
  return {
    resolveAddress: vi
      .fn()
      .mockReturnValue([{ address: "10.0.0.1", isIpv6: false }]),
    ...overrides,
  };
}

describe("WarpGridDns", () => {
  let bindings: DnsBindings;
  let dns: WarpGridDns;

  beforeEach(() => {
    bindings = createMockBindings();
    dns = new WarpGridDns(bindings);
  });

  describe("resolve()", () => {
    it("returns an array of IP address strings", () => {
      const result = dns.resolve("db.internal");

      expect(result).toEqual(["10.0.0.1"]);
    });

    it("calls the WIT resolveAddress binding with the hostname", () => {
      dns.resolve("my-service.warp.local");

      expect(bindings.resolveAddress).toHaveBeenCalledWith(
        "my-service.warp.local",
      );
    });

    it("returns multiple addresses for multi-record responses", () => {
      const multiBindings = createMockBindings({
        resolveAddress: vi.fn().mockReturnValue([
          { address: "10.0.0.1", isIpv6: false },
          { address: "10.0.0.2", isIpv6: false },
          { address: "10.0.0.3", isIpv6: false },
        ]),
      });
      const multiDns = new WarpGridDns(multiBindings);

      const result = multiDns.resolve("replicated-svc.warp.local");

      expect(result).toEqual(["10.0.0.1", "10.0.0.2", "10.0.0.3"]);
    });

    it("returns IPv6 addresses alongside IPv4", () => {
      const mixedBindings = createMockBindings({
        resolveAddress: vi.fn().mockReturnValue([
          { address: "10.0.0.1", isIpv6: false },
          { address: "::1", isIpv6: true },
          { address: "2001:db8::1", isIpv6: true },
        ]),
      });
      const mixedDns = new WarpGridDns(mixedBindings);

      const result = mixedDns.resolve("dual-stack.warp.local");

      expect(result).toEqual(["10.0.0.1", "::1", "2001:db8::1"]);
    });

    it("returns IPv6-only results correctly", () => {
      const ipv6Bindings = createMockBindings({
        resolveAddress: vi.fn().mockReturnValue([
          { address: "::1", isIpv6: true },
          { address: "fe80::1", isIpv6: true },
        ]),
      });
      const ipv6Dns = new WarpGridDns(ipv6Bindings);

      const result = ipv6Dns.resolve("ipv6-only.warp.local");

      expect(result).toEqual(["::1", "fe80::1"]);
    });

    it("throws WarpGridDNSError when hostname is empty", () => {
      expect(() => dns.resolve("")).toThrow(WarpGridDNSError);
    });

    it("throws WarpGridDNSError with descriptive message for empty hostname", () => {
      expect(() => dns.resolve("")).toThrow(/hostname/i);
    });

    it("throws WarpGridDNSError when WIT binding throws", () => {
      const failBindings = createMockBindings({
        resolveAddress: vi.fn().mockImplementation(() => {
          throw "HostNotFound: no-such-host.example.com";
        }),
      });
      const failDns = new WarpGridDns(failBindings);

      expect(() => failDns.resolve("no-such-host.example.com")).toThrow(
        WarpGridDNSError,
      );
    });

    it("includes hostname in WarpGridDNSError when WIT binding fails", () => {
      const failBindings = createMockBindings({
        resolveAddress: vi.fn().mockImplementation(() => {
          throw "HostNotFound: ghost.warp.local";
        }),
      });
      const failDns = new WarpGridDns(failBindings);

      try {
        failDns.resolve("ghost.warp.local");
        expect.fail("should have thrown");
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDNSError);
        expect((err as WarpGridDNSError).hostname).toBe("ghost.warp.local");
      }
    });

    it("includes original error as cause in WarpGridDNSError", () => {
      const failBindings = createMockBindings({
        resolveAddress: vi.fn().mockImplementation(() => {
          throw "HostNotFound: unreachable.example.com";
        }),
      });
      const failDns = new WarpGridDns(failBindings);

      try {
        failDns.resolve("unreachable.example.com");
        expect.fail("should have thrown");
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDNSError);
        expect((err as WarpGridDNSError).cause).toBeDefined();
      }
    });

    it("includes readable message in WarpGridDNSError for resolution failures", () => {
      const failBindings = createMockBindings({
        resolveAddress: vi.fn().mockImplementation(() => {
          throw "HostNotFound: timeout.warp.local";
        }),
      });
      const failDns = new WarpGridDns(failBindings);

      expect(() => failDns.resolve("timeout.warp.local")).toThrow(
        /dns.*resolve.*failed/i,
      );
    });

    it("handles Error objects thrown by WIT binding", () => {
      const failBindings = createMockBindings({
        resolveAddress: vi.fn().mockImplementation(() => {
          throw new Error("network unreachable");
        }),
      });
      const failDns = new WarpGridDns(failBindings);

      try {
        failDns.resolve("error-host.example.com");
        expect.fail("should have thrown");
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridDNSError);
        expect((err as WarpGridDNSError).message).toContain(
          "network unreachable",
        );
        expect((err as WarpGridDNSError).cause).toBeInstanceOf(Error);
      }
    });

    it("returns empty array when WIT binding returns empty list", () => {
      const emptyBindings = createMockBindings({
        resolveAddress: vi.fn().mockReturnValue([]),
      });
      const emptyDns = new WarpGridDns(emptyBindings);

      const result = emptyDns.resolve("no-records.example.com");

      expect(result).toEqual([]);
    });
  });
});
