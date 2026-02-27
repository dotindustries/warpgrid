/**
 * WarpGrid DNS resolution module.
 *
 * Wraps the low-level `warpgrid:shim/dns` WIT bindings
 * in a developer-friendly API. Returns a simple `string[]`
 * of IP addresses from the WIT record type.
 */

import { WarpGridDNSError } from "./errors.js";
import type { DnsBindings } from "./types.js";

function wrapDnsError(hostname: string, err: unknown): never {
  const message =
    typeof err === "string"
      ? err
      : err instanceof Error
        ? err.message
        : String(err);

  throw new WarpGridDNSError(
    hostname,
    `DNS resolution failed for '${hostname}': ${message}`,
    { cause: err },
  );
}

/**
 * WarpGrid DNS module providing `resolve()` for hostname resolution
 * through the host's DNS shim (service registry → /etc/hosts → system DNS).
 */
export class WarpGridDns {
  private readonly bindings: DnsBindings;

  constructor(bindings: DnsBindings) {
    this.bindings = bindings;
  }

  /**
   * Resolve a hostname to IP addresses via the WarpGrid DNS chain.
   *
   * @returns Array of IP address strings (IPv4 and/or IPv6)
   * @throws {WarpGridDNSError} if the hostname cannot be resolved
   */
  resolve(hostname: string): string[] {
    if (!hostname) {
      throw new WarpGridDNSError(
        hostname,
        "DNS resolve failed: hostname must be non-empty",
      );
    }

    try {
      const records = this.bindings.resolveAddress(hostname);
      return records.map((r) => r.address);
    } catch (err) {
      wrapDnsError(hostname, err);
    }
  }
}
