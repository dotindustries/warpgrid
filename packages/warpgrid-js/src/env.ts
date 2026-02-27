/**
 * process.env polyfill for ComponentizeJS runtime.
 *
 * Reads WASI environment variables via the `wasi:cli/environment`
 * interface and presents them as a plain object matching Node.js
 * `process.env` semantics.
 *
 * The environment is read once at construction time and cached —
 * WASI environment variables are immutable for the module lifetime.
 */

import type { EnvironmentBindings } from "./types.js";

/**
 * Create a `process.env`-compatible object from WASI environment bindings.
 *
 * Returns a frozen object where each WASI environment variable
 * is accessible as a property. Missing variables return `undefined`.
 * Supports `in` operator and `Object.keys()` enumeration.
 */
export function createProcessEnv(
  bindings: EnvironmentBindings,
): Record<string, string | undefined> {
  const pairs = bindings.getEnvironment();
  const envMap = new Map<string, string>();
  for (const [key, value] of pairs) {
    envMap.set(key, value);
  }

  return new Proxy(Object.create(null) as Record<string, string | undefined>, {
    get(_target, prop: string | symbol): string | undefined {
      if (typeof prop !== "string") {
        return undefined;
      }
      return envMap.get(prop);
    },

    has(_target, prop: string | symbol): boolean {
      if (typeof prop !== "string") {
        return false;
      }
      return envMap.has(prop);
    },

    ownKeys(): string[] {
      return [...envMap.keys()];
    },

    getOwnPropertyDescriptor(_target, prop: string | symbol) {
      if (typeof prop !== "string" || !envMap.has(prop)) {
        return undefined;
      }
      return {
        value: envMap.get(prop),
        writable: false,
        enumerable: true,
        configurable: true,
      };
    },
  });
}
