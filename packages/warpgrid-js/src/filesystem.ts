/**
 * WarpGrid virtual filesystem module.
 *
 * Wraps the low-level `warpgrid:shim/filesystem` WIT bindings
 * in a developer-friendly `readFile()` API. The WIT bindings
 * are injected via constructor for testability — in production,
 * they come from the ComponentizeJS WIT import; in tests, mocked.
 */

import { WarpGridFSError } from "./errors.js";
import type { FilesystemBindings } from "./types.js";

// TextDecoder is available in ComponentizeJS runtime (SpiderMonkey)
// and Node/Bun but not in our ES2022 lib target. Declare the subset we use.
declare class TextDecoder {
  constructor(label?: string);
  decode(input?: ArrayBufferView | ArrayBuffer): string;
}

const READ_CHUNK_SIZE = 65536;

function validatePath(path: string): void {
  if (!path) {
    throw new WarpGridFSError(path, "path must be non-empty");
  }

  if (!path.startsWith("/")) {
    throw new WarpGridFSError(
      path,
      "path must be absolute (start with '/')",
    );
  }

  const segments = path.split("/");
  for (const segment of segments) {
    if (segment === "..") {
      throw new WarpGridFSError(
        path,
        "path traversal ('..') is not allowed",
      );
    }
  }
}

function wrapWitError(path: string, operation: string, err: unknown): never {
  const message =
    typeof err === "string"
      ? err
      : err instanceof Error
        ? err.message
        : String(err);

  throw new WarpGridFSError(path, `${operation} failed: ${message}`, {
    cause: err,
  });
}

/**
 * WarpGrid filesystem module providing `readFile()` for reading
 * virtual files through the host's filesystem shim.
 */
export class WarpGridFilesystem {
  private readonly bindings: FilesystemBindings;

  constructor(bindings: FilesystemBindings) {
    this.bindings = bindings;
  }

  /**
   * Read a file from the WASI virtual filesystem.
   *
   * Opens the file, reads all content, and closes the handle.
   * @returns The file content as a Uint8Array
   * @throws {WarpGridFSError} if the path is invalid or the file cannot be read
   */
  async readFile(path: string): Promise<Uint8Array>;
  /**
   * Read a file from the WASI virtual filesystem as a string.
   *
   * @param encoding - Text encoding to use (supports "utf-8" and "utf8")
   * @returns The file content as a string
   * @throws {WarpGridFSError} if the path is invalid or the file cannot be read
   */
  async readFile(path: string, encoding: "utf-8" | "utf8"): Promise<string>;
  async readFile(
    path: string,
    encoding?: "utf-8" | "utf8",
  ): Promise<Uint8Array | string> {
    validatePath(path);

    let handle: bigint;
    try {
      handle = this.bindings.openVirtual(path);
    } catch (err) {
      wrapWitError(path, "open", err);
    }

    const chunks: Uint8Array[] = [];
    try {
      for (;;) {
        const chunk = this.bindings.readVirtual(handle, READ_CHUNK_SIZE);
        if (chunk.length === 0) {
          break;
        }
        chunks.push(chunk);
      }
    } catch (err) {
      // Ensure handle is closed even on read error
      try {
        this.bindings.closeVirtual(handle);
      } catch {
        // Swallow close error — the read error is more important
      }
      wrapWitError(path, "read", err);
    }

    try {
      this.bindings.closeVirtual(handle);
    } catch (err) {
      wrapWitError(path, "close", err);
    }

    const result = concatChunks(chunks);

    if (encoding !== undefined) {
      return new TextDecoder("utf-8").decode(result);
    }

    return result;
  }
}

function concatChunks(chunks: Uint8Array[]): Uint8Array {
  if (chunks.length === 0) {
    return new Uint8Array(0);
  }
  if (chunks.length === 1) {
    return chunks[0]!;
  }

  let totalLength = 0;
  for (const chunk of chunks) {
    totalLength += chunk.length;
  }

  const result = new Uint8Array(totalLength);
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.length;
  }

  return result;
}
