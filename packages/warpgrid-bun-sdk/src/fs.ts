/**
 * @warpgrid/bun-sdk/fs — Dual-mode filesystem access.
 *
 * Provides `readFile()`, `readTextFile()`, `writeFile()`, and `exists()`
 * functions that work identically in native Bun (development) and
 * WASI/Wasm (deployed) environments.
 *
 * - In **native mode** (Bun development), delegates to Bun's native file APIs.
 * - In **Wasm mode** (deployed as WASI component), delegates to the
 *   Domain 1 filesystem shim via the virtual filesystem interface.
 *
 * Mode is auto-detected but can be overridden via `options.mode`.
 */

import {
  WarpGridFsNotFoundError,
  WarpGridFsPermissionError,
} from "./errors.ts";

export { WarpGridFsNotFoundError, WarpGridFsPermissionError } from "./errors.ts";

// ── Public Types ──────────────────────────────────────────────────────

/**
 * Low-level filesystem shim interface.
 * Mirrors the `warpgrid:shim/filesystem@0.1.0` WIT interface with
 * an additional `writeVirtual` for write support.
 */
export interface FilesystemShim {
  /** Open a virtual path, returning a handle for subsequent reads. */
  openVirtual(path: string): number;
  /** Read up to `len` bytes from an open virtual file handle. */
  readVirtual(handle: number, len: number): Uint8Array;
  /** Stat a virtual path. Returns null if the path does not exist. */
  statVirtual(path: string): { size: number; isFile: boolean; isDirectory: boolean } | null;
  /** Close a previously opened virtual file handle. */
  closeVirtual(handle: number): void;
  /** Write data to a virtual path. */
  writeVirtual(path: string, data: Uint8Array): void;
}

/** Options for filesystem operations. */
export interface FsOptions {
  /**
   * Execution mode override.
   * - `"native"`: Use Bun's native file APIs.
   * - `"wasm"`: Use the WarpGrid filesystem shim.
   * - `"auto"`: Auto-detect based on runtime environment (default).
   */
  mode?: "native" | "wasm" | "auto";
  /**
   * Injected filesystem shim (Wasm mode).
   * Auto-detected from globals if not provided.
   * Useful for testing with mock shims.
   */
  shim?: FilesystemShim;
  /**
   * Sandbox root directory for native mode path traversal checks.
   * Defaults to "/" (allow all absolute paths).
   */
  sandboxRoot?: string;
}

// ── Internals ─────────────────────────────────────────────────────────

/** Detect whether we're running in native Bun or WASI/Wasm mode. */
export function detectMode(): "native" | "wasm" {
  if (typeof (globalThis as Record<string, unknown>).Bun !== "undefined") {
    if (
      (globalThis as Record<string, unknown>).__WARPGRID_WASM__ !== undefined
    ) {
      return "wasm";
    }
    return "native";
  }
  return "wasm";
}

/**
 * Resolve the filesystem shim from options or globals.
 * Returns undefined if not available.
 */
function resolveShim(options?: FsOptions): FilesystemShim | undefined {
  if (options?.shim) return options.shim;
  const g = globalThis as Record<string, unknown>;
  const wg = g.warpgrid as Record<string, unknown> | undefined;
  return wg?.filesystem as FilesystemShim | undefined;
}

/**
 * Validate that a path is absolute and does not escape the sandbox.
 * Throws WarpGridFsPermissionError for path traversal attempts.
 */
function validatePath(path: string, sandboxRoot?: string): void {
  if (!path.startsWith("/")) {
    throw new WarpGridFsPermissionError(path);
  }

  // Normalize the path to detect traversal via ".."
  const segments: string[] = [];
  for (const segment of path.split("/")) {
    if (segment === "" || segment === ".") continue;
    if (segment === "..") {
      segments.pop();
    } else {
      segments.push(segment);
    }
  }
  const normalized = "/" + segments.join("/");

  // Check sandbox boundary in native mode
  if (sandboxRoot) {
    const normalizedRoot = normalizePath(sandboxRoot);
    if (!normalized.startsWith(normalizedRoot)) {
      throw new WarpGridFsPermissionError(path);
    }
  }
}

/** Normalize a path by resolving `.` and `..` segments. */
function normalizePath(path: string): string {
  const segments: string[] = [];
  for (const segment of path.split("/")) {
    if (segment === "" || segment === ".") continue;
    if (segment === "..") {
      segments.pop();
    } else {
      segments.push(segment);
    }
  }
  return "/" + segments.join("/");
}

// ── Wasm mode implementation ─────────────────────────────────────────

/** Read buffer size for streaming reads from virtual file handles. */
const READ_CHUNK_SIZE = 65536;

async function wasmReadFile(
  path: string,
  shim: FilesystemShim,
): Promise<Uint8Array> {
  let handle: number | undefined;
  try {
    handle = shim.openVirtual(path);
  } catch (err) {
    throw new WarpGridFsNotFoundError(path, { cause: err });
  }

  try {
    const chunks: Uint8Array[] = [];
    let totalLength = 0;
    for (;;) {
      const chunk = shim.readVirtual(handle, READ_CHUNK_SIZE);
      if (chunk.length === 0) break;
      chunks.push(chunk);
      totalLength += chunk.length;
    }

    if (chunks.length === 1) return chunks[0];

    const result = new Uint8Array(totalLength);
    let offset = 0;
    for (const chunk of chunks) {
      result.set(chunk, offset);
      offset += chunk.length;
    }
    return result;
  } finally {
    shim.closeVirtual(handle);
  }
}

async function wasmWriteFile(
  path: string,
  data: Uint8Array | string,
  shim: FilesystemShim,
): Promise<void> {
  const bytes =
    typeof data === "string" ? new TextEncoder().encode(data) : data;
  shim.writeVirtual(path, bytes);
}

async function wasmExists(
  path: string,
  shim: FilesystemShim,
): Promise<boolean> {
  const stat = shim.statVirtual(path);
  return stat !== null;
}

// ── Native mode implementation ───────────────────────────────────────

async function nativeReadFile(path: string): Promise<Uint8Array> {
  const file = Bun.file(path);
  const fileExists = await file.exists();
  if (!fileExists) {
    throw new WarpGridFsNotFoundError(path);
  }
  return new Uint8Array(await file.arrayBuffer());
}

async function nativeWriteFile(
  path: string,
  data: Uint8Array | string,
): Promise<void> {
  await Bun.write(path, data);
}

async function nativeExists(path: string): Promise<boolean> {
  return Bun.file(path).exists();
}

// ── Public API ────────────────────────────────────────────────────────

/**
 * Read a file, returning its contents as a `Uint8Array`.
 *
 * @throws {WarpGridFsNotFoundError} If the file does not exist.
 * @throws {WarpGridFsPermissionError} If the path traverses outside the sandbox.
 */
export async function readFile(
  path: string,
  options?: FsOptions,
): Promise<Uint8Array> {
  const mode =
    options?.mode === "auto" || !options?.mode ? detectMode() : options.mode;

  validatePath(path, mode === "native" ? options?.sandboxRoot : undefined);

  if (mode === "native") {
    return nativeReadFile(path);
  }

  const shim = resolveShim(options);
  if (!shim) {
    throw new Error(
      "Wasm mode requires a FilesystemShim. " +
        "Provide options.shim or ensure globalThis.warpgrid.filesystem is set.",
    );
  }
  return wasmReadFile(path, shim);
}

/**
 * Read a file, returning its contents as a UTF-8 string.
 *
 * @throws {WarpGridFsNotFoundError} If the file does not exist.
 * @throws {WarpGridFsPermissionError} If the path traverses outside the sandbox.
 */
export async function readTextFile(
  path: string,
  options?: FsOptions,
): Promise<string> {
  const data = await readFile(path, options);
  return new TextDecoder().decode(data);
}

/**
 * Write data to a file.
 *
 * Accepts either a `Uint8Array` or a string (encoded as UTF-8).
 *
 * @throws {WarpGridFsPermissionError} If the path traverses outside the sandbox.
 */
export async function writeFile(
  path: string,
  data: Uint8Array | string,
  options?: FsOptions,
): Promise<void> {
  const mode =
    options?.mode === "auto" || !options?.mode ? detectMode() : options.mode;

  validatePath(path, mode === "native" ? options?.sandboxRoot : undefined);

  if (mode === "native") {
    return nativeWriteFile(path, data);
  }

  const shim = resolveShim(options);
  if (!shim) {
    throw new Error(
      "Wasm mode requires a FilesystemShim. " +
        "Provide options.shim or ensure globalThis.warpgrid.filesystem is set.",
    );
  }
  return wasmWriteFile(path, data, shim);
}

/**
 * Check whether a file exists at the given path.
 *
 * @throws {WarpGridFsPermissionError} If the path traverses outside the sandbox.
 */
export async function exists(
  path: string,
  options?: FsOptions,
): Promise<boolean> {
  const mode =
    options?.mode === "auto" || !options?.mode ? detectMode() : options.mode;

  validatePath(path, mode === "native" ? options?.sandboxRoot : undefined);

  if (mode === "native") {
    return nativeExists(path);
  }

  const shim = resolveShim(options);
  if (!shim) {
    throw new Error(
      "Wasm mode requires a FilesystemShim. " +
        "Provide options.shim or ensure globalThis.warpgrid.filesystem is set.",
    );
  }
  return wasmExists(path, shim);
}
