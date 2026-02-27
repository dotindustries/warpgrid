/**
 * @warpgrid/bun-sdk/dns — Dual-mode DNS resolution.
 *
 * Provides a `resolve()` function for hostname resolution that works
 * identically in native Bun (development) and WASI/Wasm (deployed) modes.
 *
 * - In **native mode** (Bun development), delegates to Bun's built-in DNS.
 * - In **Wasm mode** (deployed as WASI component), delegates to the
 *   Domain 1 DNS shim via `warpgrid:shim/dns.resolve-address`.
 *
 * Supports A, AAAA, and CNAME record types.
 */

import { WarpGridDnsError } from "./errors.ts";
import { detectMode } from "./postgres.ts";

export { WarpGridDnsError } from "./errors.ts";

// ── Public Types ──────────────────────────────────────────────────────

/** A resolved IP address record from the WarpGrid DNS shim. */
export interface IpAddressRecord {
  /** IP address string (e.g., "10.0.0.1" or "::1"). */
  address: string;
  /** Whether this is an IPv6 address. */
  isIpv6: boolean;
}

/**
 * Low-level DNS shim interface.
 * Mirrors the `warpgrid:shim/dns@0.1.0` WIT interface.
 */
export interface DnsShim {
  /**
   * Resolve a hostname to a list of IP address records.
   * Throws if the hostname cannot be resolved.
   */
  resolveAddress(hostname: string): IpAddressRecord[];
}

/** Supported DNS record types. */
export type RRType = "A" | "AAAA" | "CNAME";

/** Options for the `resolve()` function. */
export interface ResolveOptions {
  /**
   * Execution mode override.
   * - `"native"`: Use Bun's built-in DNS.
   * - `"wasm"`: Use the WarpGrid DNS shim.
   * - `"auto"`: Auto-detect based on runtime environment (default).
   */
  mode?: "native" | "wasm" | "auto";
  /**
   * Injected DNS shim (Wasm mode).
   * Auto-detected from globals if not provided.
   * Useful for testing with mock shims.
   */
  shim?: DnsShim;
  /**
   * Resolution timeout in milliseconds.
   * If exceeded, throws WarpGridDnsError with code ETIMEOUT.
   */
  timeout?: number;
}

// ── Shim Resolution ──────────────────────────────────────────────────

/**
 * Resolve the DNS shim from options or globalThis.warpgrid.dns.
 * Returns undefined if not available.
 */
export function resolveShim(options: ResolveOptions): DnsShim | undefined {
  if (options.shim) return options.shim;
  const g = globalThis as Record<string, unknown>;
  const wg = g.warpgrid as Record<string, unknown> | undefined;
  return wg?.dns as DnsShim | undefined;
}

// ── Core Resolve Function ────────────────────────────────────────────

/**
 * Resolve a hostname to an array of IP address strings.
 *
 * @param hostname - The hostname to resolve.
 * @param rrtype - Record type: "A" (default), "AAAA", or "CNAME".
 * @param options - Mode, shim injection, and timeout configuration.
 * @returns Array of resolved IP address strings.
 * @throws WarpGridDnsError with code "ENOTFOUND" if hostname cannot be resolved.
 * @throws WarpGridDnsError with code "ETIMEOUT" if resolution exceeds timeout.
 */
export async function resolve(
  hostname: string,
  rrtype?: RRType,
  options: ResolveOptions = {},
): Promise<string[]> {
  const effectiveRrtype = rrtype ?? "A";
  const mode =
    options.mode === "auto" || !options.mode ? detectMode() : options.mode;

  if (mode === "native") {
    return resolveNative(hostname, effectiveRrtype, options.timeout);
  }

  return resolveWasm(hostname, effectiveRrtype, options);
}

// ── Native Mode ──────────────────────────────────────────────────────

async function resolveNative(
  hostname: string,
  rrtype: RRType,
  timeout?: number,
): Promise<string[]> {
  const family =
    rrtype === "AAAA" ? 6 : rrtype === "A" ? 4 : undefined;

  const doResolve = async (): Promise<string[]> => {
    try {
      // Use node:dns lookup which delegates to getaddrinfo — checks /etc/hosts
      const dns = await import("node:dns");
      const results = await dns.promises.lookup(hostname, {
        all: true,
        family: family ?? 0,
      });

      if (rrtype === "CNAME") {
        return results.map((r) => r.address);
      }
      return results
        .filter((r) =>
          rrtype === "AAAA" ? r.family === 6 : r.family === 4,
        )
        .map((r) => r.address);
    } catch (err: unknown) {
      const code = (err as { code?: string }).code;
      if (
        code === "ENOTFOUND" ||
        code === "ENODATA" ||
        code === "ESERVFAIL" ||
        code === "DNS_ENOTFOUND" ||
        code === "EAI_NONAME"
      ) {
        throw new WarpGridDnsError(
          `DNS resolution failed for "${hostname}": ${code}`,
          "ENOTFOUND",
          { cause: err },
        );
      }
      throw new WarpGridDnsError(
        `DNS resolution failed for "${hostname}"`,
        "ENOTFOUND",
        { cause: err },
      );
    }
  };

  if (timeout !== undefined) {
    return withTimeout(doResolve(), hostname, timeout);
  }

  return doResolve();
}

// ── Wasm Mode ────────────────────────────────────────────────────────

async function resolveWasm(
  hostname: string,
  rrtype: RRType,
  options: ResolveOptions,
): Promise<string[]> {
  const shim = resolveShim(options);
  if (!shim) {
    throw new WarpGridDnsError(
      "Wasm mode requires a DnsShim. " +
        "Provide options.shim or ensure globalThis.warpgrid.dns is set.",
      "ENOTFOUND",
    );
  }

  const doResolve = async (): Promise<string[]> => {
    try {
      const records = shim.resolveAddress(hostname);
      return filterByRRType(records, rrtype);
    } catch (err: unknown) {
      const message =
        err instanceof Error ? err.message : String(err);
      throw new WarpGridDnsError(
        `DNS resolution failed for "${hostname}": ${message}`,
        "ENOTFOUND",
        { cause: err },
      );
    }
  };

  if (options.timeout !== undefined) {
    return withTimeout(doResolve(), hostname, options.timeout);
  }

  return doResolve();
}

// ── Helpers ──────────────────────────────────────────────────────────

/** Filter IP address records by DNS record type. */
function filterByRRType(records: IpAddressRecord[], rrtype: RRType): string[] {
  if (rrtype === "CNAME") {
    // CNAME returns all addresses regardless of type
    return records.map((r) => r.address);
  }
  if (rrtype === "AAAA") {
    return records.filter((r) => r.isIpv6).map((r) => r.address);
  }
  // Default: "A" — return IPv4 only
  return records.filter((r) => !r.isIpv6).map((r) => r.address);
}

/** Race a promise against a timeout, throwing ETIMEOUT on expiry. */
async function withTimeout<T>(
  promise: Promise<T>,
  hostname: string,
  timeoutMs: number,
): Promise<T> {
  const timer = new Promise<never>((_resolve, reject) => {
    setTimeout(() => {
      reject(
        new WarpGridDnsError(
          `DNS resolution timed out for "${hostname}" after ${timeoutMs}ms`,
          "ETIMEOUT",
        ),
      );
    }, timeoutMs);
  });

  return Promise.race([promise, timer]);
}
