/**
 * WarpGrid virtual filesystem module.
 *
 * Wraps the low-level `warpgrid:shim/filesystem` WIT bindings
 * (open → read → close) into a single `readFile()` API.
 */

import { WarpGridFSError } from "./errors.js";
import type { FilesystemBindings } from "./types.js";

// TextDecoder is available in all target runtimes (Node.js, SpiderMonkey/ComponentizeJS)
// but not in the ES2022 lib type declarations.
declare const TextDecoder: { new (): { decode(input?: ArrayBufferView | ArrayBuffer): string } };

const MAX_READ_BYTES = 1048576; // 1 MiB default

function wrapFsError(path: string, err: unknown): never {
  const message =
    typeof err === "string"
      ? err
      : err instanceof Error
        ? err.message
        : String(err);

  throw new WarpGridFSError(
    path,
    `Filesystem operation failed for '${path}': ${message}`,
    { cause: err },
  );
}

/**
 * WarpGrid filesystem module providing `readFile()` for reading
 * virtual files from the host's filesystem shim.
 */
export class WarpGridFs {
  private readonly bindings: FilesystemBindings;

  constructor(bindings: FilesystemBindings) {
    this.bindings = bindings;
  }

  /**
   * Read a virtual file's contents.
   *
   * Opens the file via WIT, reads up to 1 MiB, and closes the handle.
   * Optionally decodes as UTF-8 string.
   *
   * @param path - Virtual file path (e.g., "/usr/share/zoneinfo/UTC")
   * @param encoding - If "utf-8" or "utf8", returns a string; otherwise Uint8Array
   * @throws {WarpGridFSError} if the file cannot be read
   */
  readFile(path: string, encoding?: string): Uint8Array | string {
    if (!path) {
      throw new WarpGridFSError(path, "readFile failed: path must be non-empty");
    }

    const handle = this.openOrThrow(path);
    let result: Uint8Array | string;

    try {
      const data = this.bindings.readVirtual(handle, MAX_READ_BYTES);
      const bytes = new Uint8Array(data);

      result =
        encoding === "utf-8" || encoding === "utf8"
          ? new TextDecoder().decode(bytes)
          : bytes;
    } catch (err) {
      // Close handle before re-throwing
      this.safeClose(handle);
      wrapFsError(path, err);
    }

    this.safeClose(handle);
    return result;
  }

  private safeClose(handle: bigint): void {
    try {
      this.bindings.closeVirtual(handle);
    } catch {
      // Best-effort close — don't mask the original error
    }
  }

  private openOrThrow(path: string): bigint {
    try {
      return this.bindings.openVirtual(path);
    } catch (err) {
      wrapFsError(path, err);
    }
  }
}
