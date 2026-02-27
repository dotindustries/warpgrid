/**
 * @warpgrid/bun-sdk — WarpGrid SDK for Bun.
 *
 * Provides dual-mode modules that work identically in native Bun
 * (development) and WASI/Wasm (deployed) environments.
 */

import { WarpGridHandlerValidationError } from "./errors.ts";

export {
  WarpGridError,
  WarpGridDatabaseError,
  WarpGridDnsError,
  WarpGridHandlerValidationError,
  PostgresError,
} from "./errors.ts";
export {
  createPool,
  detectMode,
  type Pool,
  type PoolConfig,
  type QueryResult,
  type FieldInfo,
  type DatabaseProxyShim,
} from "./postgres.ts";

// ── Handler Interface ─────────────────────────────────────────────

/**
 * WarpGrid handler contract for Bun modules.
 *
 * Implements the web-standard `fetch` pattern: receives a `Request`,
 * returns a `Response` (or `Promise<Response>`).
 *
 * Optional `init()` lifecycle hook runs once before the first request.
 */
export interface WarpGridHandler {
  fetch(request: Request): Response | Promise<Response>;
  init?(): Promise<void>;
}

/**
 * Runtime validation that an object conforms to the `WarpGridHandler` contract.
 *
 * Throws `WarpGridHandlerValidationError` if the handler is missing a
 * `fetch` method or if `fetch` is not a function.
 */
export function validateHandler(
  handler: unknown,
): asserts handler is WarpGridHandler {
  if (handler === null || handler === undefined) {
    throw new WarpGridHandlerValidationError(
      "Handler must be an object with a fetch() method, got " +
        String(handler),
    );
  }

  const obj = handler as Record<string, unknown>;

  if (!("fetch" in obj)) {
    throw new WarpGridHandlerValidationError(
      "Handler is missing a fetch() method. " +
        "Expected: { fetch(request: Request): Response | Promise<Response> }",
    );
  }

  if (typeof obj.fetch !== "function") {
    throw new WarpGridHandlerValidationError(
      "Handler.fetch must be a function, got " + typeof obj.fetch,
    );
  }
}
