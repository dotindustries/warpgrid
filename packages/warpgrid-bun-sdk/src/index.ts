/**
 * @warpgrid/bun-sdk — Write Bun TypeScript handlers that deploy as WASI components.
 *
 * This package provides the core interface contract for WarpGrid Bun handlers.
 * A handler implementing `WarpGridHandler` can run in both native Bun (development)
 * and compiled-to-Wasm (deployment) modes with zero code changes.
 */

/**
 * The core handler interface for WarpGrid Bun modules.
 *
 * Mirrors the Bun.serve() `fetch` convention so that existing Bun handlers
 * can be adapted with minimal changes. The `fetch` method receives a standard
 * `Request` and must return a `Response` or `Promise<Response>`.
 *
 * @example
 * ```ts
 * import type { WarpGridHandler } from "@warpgrid/bun-sdk";
 *
 * const handler: WarpGridHandler = {
 *   async fetch(request: Request): Promise<Response> {
 *     return new Response("Hello from WarpGrid!");
 *   }
 * };
 *
 * export default handler;
 * ```
 */
export interface WarpGridHandler {
  /**
   * Handle an incoming HTTP request and return a response.
   *
   * This is the primary entry point for your handler. The request and
   * response use the standard Web API `Request`/`Response` types.
   */
  fetch(request: Request): Response | Promise<Response>;

  /**
   * Optional lifecycle hook called once before the first request.
   *
   * Use this to perform one-time initialization such as loading
   * configuration or establishing database connections.
   */
  init?(): Promise<void>;
}
