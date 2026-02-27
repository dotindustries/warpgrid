import { describe, test, expect, beforeEach, afterEach } from "bun:test";
import {
  readFile,
  readTextFile,
  writeFile,
  exists,
  detectMode,
  type FilesystemShim,
} from "../src/fs.ts";
import {
  WarpGridFsNotFoundError,
  WarpGridFsPermissionError,
} from "../src/errors.ts";

// ── Mock shim ────────────────────────────────────────────────────────

/** In-memory filesystem shim for testing Wasm mode. */
function createMockShim(
  files: Record<string, Uint8Array> = {},
): FilesystemShim & { written: Record<string, Uint8Array> } {
  let nextHandle = 1;
  const openHandles = new Map<number, { path: string; offset: number }>();
  const written: Record<string, Uint8Array> = {};

  return {
    written,
    openVirtual(path: string): number {
      if (!(path in files) && !(path in written)) {
        throw new Error(`not a virtual path: ${path}`);
      }
      const handle = nextHandle++;
      openHandles.set(handle, { path, offset: 0 });
      return handle;
    },
    readVirtual(handle: number, len: number): Uint8Array {
      const entry = openHandles.get(handle);
      if (!entry) throw new Error(`invalid handle: ${handle}`);
      const data = files[entry.path] ?? written[entry.path];
      if (!data) return new Uint8Array(0);
      const slice = data.slice(entry.offset, entry.offset + len);
      entry.offset += slice.length;
      return slice;
    },
    statVirtual(path: string): { size: number; isFile: boolean; isDirectory: boolean } | null {
      const data = files[path] ?? written[path];
      if (!data) return null;
      return { size: data.length, isFile: true, isDirectory: false };
    },
    closeVirtual(handle: number): void {
      if (!openHandles.has(handle)) throw new Error(`invalid handle: ${handle}`);
      openHandles.delete(handle);
    },
    writeVirtual(path: string, data: Uint8Array): void {
      written[path] = data;
    },
  };
}

// ── Error type tests ─────────────────────────────────────────────────

describe("WarpGridFsNotFoundError", () => {
  test("is an instance of WarpGridError and Error", () => {
    const err = new WarpGridFsNotFoundError("/missing/file");
    expect(err).toBeInstanceOf(Error);
    expect(err.name).toBe("WarpGridFsNotFoundError");
    expect(err.path).toBe("/missing/file");
    expect(err.message).toBe("File not found: /missing/file");
  });

  test("stores cause when provided", () => {
    const cause = new Error("underlying");
    const err = new WarpGridFsNotFoundError("/x", { cause });
    expect(err.cause).toBe(cause);
  });
});

describe("WarpGridFsPermissionError", () => {
  test("is an instance of WarpGridError and Error", () => {
    const err = new WarpGridFsPermissionError("../../etc/passwd");
    expect(err).toBeInstanceOf(Error);
    expect(err.name).toBe("WarpGridFsPermissionError");
    expect(err.path).toBe("../../etc/passwd");
    expect(err.message).toBe("Path traversal denied: ../../etc/passwd");
  });

  test("stores cause when provided", () => {
    const cause = new Error("sandbox violation");
    const err = new WarpGridFsPermissionError("/escape", { cause });
    expect(err.cause).toBe(cause);
  });
});

// ── Mode detection tests ─────────────────────────────────────────────

describe("detectMode", () => {
  const g = globalThis as Record<string, unknown>;
  let originalWasm: unknown;

  beforeEach(() => {
    originalWasm = g.__WARPGRID_WASM__;
  });

  afterEach(() => {
    if (originalWasm === undefined) {
      delete g.__WARPGRID_WASM__;
    } else {
      g.__WARPGRID_WASM__ = originalWasm;
    }
  });

  test("returns 'native' in Bun without WASM flag", () => {
    delete g.__WARPGRID_WASM__;
    expect(detectMode()).toBe("native");
  });

  test("returns 'wasm' when __WARPGRID_WASM__ is set", () => {
    g.__WARPGRID_WASM__ = true;
    expect(detectMode()).toBe("wasm");
  });
});

// ── Wasm mode tests (mock shim) ─────────────────────────────────────

describe("fs (wasm mode)", () => {
  const textContent = new TextEncoder().encode("hello warpgrid");
  const binaryContent = new Uint8Array([0x00, 0xff, 0x42, 0x13]);

  describe("readFile", () => {
    test("reads a file as Uint8Array", async () => {
      const shim = createMockShim({ "/data/test.bin": binaryContent });
      const result = await readFile("/data/test.bin", { mode: "wasm", shim });
      expect(result).toEqual(binaryContent);
    });

    test("reads a text file as Uint8Array", async () => {
      const shim = createMockShim({ "/etc/resolv.conf": textContent });
      const result = await readFile("/etc/resolv.conf", { mode: "wasm", shim });
      expect(result).toEqual(textContent);
    });

    test("throws WarpGridFsNotFoundError for missing file", async () => {
      const shim = createMockShim({});
      await expect(
        readFile("/nonexistent", { mode: "wasm", shim }),
      ).rejects.toBeInstanceOf(WarpGridFsNotFoundError);
    });

    test("throws WarpGridFsPermissionError for path traversal", async () => {
      const shim = createMockShim({});
      await expect(
        readFile("../../etc/passwd", { mode: "wasm", shim }),
      ).rejects.toBeInstanceOf(WarpGridFsPermissionError);
    });

    test("throws WarpGridFsPermissionError for relative paths", async () => {
      const shim = createMockShim({});
      await expect(
        readFile("relative/path.txt", { mode: "wasm", shim }),
      ).rejects.toBeInstanceOf(WarpGridFsPermissionError);
    });
  });

  describe("readTextFile", () => {
    test("reads a file as UTF-8 string", async () => {
      const shim = createMockShim({ "/etc/hostname": textContent });
      const result = await readTextFile("/etc/hostname", { mode: "wasm", shim });
      expect(result).toBe("hello warpgrid");
    });

    test("throws WarpGridFsNotFoundError for missing file", async () => {
      const shim = createMockShim({});
      await expect(
        readTextFile("/missing", { mode: "wasm", shim }),
      ).rejects.toBeInstanceOf(WarpGridFsNotFoundError);
    });
  });

  describe("writeFile", () => {
    test("writes Uint8Array data", async () => {
      const shim = createMockShim({});
      await writeFile("/tmp/output.bin", binaryContent, { mode: "wasm", shim });
      expect(shim.written["/tmp/output.bin"]).toEqual(binaryContent);
    });

    test("writes string data as UTF-8", async () => {
      const shim = createMockShim({});
      await writeFile("/tmp/output.txt", "hello", { mode: "wasm", shim });
      const expected = new TextEncoder().encode("hello");
      expect(shim.written["/tmp/output.txt"]).toEqual(expected);
    });

    test("throws WarpGridFsPermissionError for path traversal", async () => {
      const shim = createMockShim({});
      await expect(
        writeFile("../../escape", "data", { mode: "wasm", shim }),
      ).rejects.toBeInstanceOf(WarpGridFsPermissionError);
    });
  });

  describe("exists", () => {
    test("returns true for existing file", async () => {
      const shim = createMockShim({ "/etc/hosts": textContent });
      const result = await exists("/etc/hosts", { mode: "wasm", shim });
      expect(result).toBe(true);
    });

    test("returns false for missing file", async () => {
      const shim = createMockShim({});
      const result = await exists("/nonexistent", { mode: "wasm", shim });
      expect(result).toBe(false);
    });

    test("throws WarpGridFsPermissionError for path traversal", async () => {
      const shim = createMockShim({});
      await expect(
        exists("../escape", { mode: "wasm", shim }),
      ).rejects.toBeInstanceOf(WarpGridFsPermissionError);
    });
  });
});

// ── Native mode tests ────────────────────────────────────────────────

describe("fs (native mode)", () => {
  const tmpDir = "/tmp/warpgrid-fs-test-" + Date.now();

  beforeEach(async () => {
    await Bun.write(`${tmpDir}/hello.txt`, "hello native");
    await Bun.write(`${tmpDir}/binary.bin`, new Uint8Array([0xde, 0xad]));
  });

  afterEach(async () => {
    const { rm } = await import("node:fs/promises");
    await rm(tmpDir, { recursive: true, force: true });
  });

  describe("readFile", () => {
    test("reads a file as Uint8Array", async () => {
      const result = await readFile(`${tmpDir}/hello.txt`, { mode: "native", sandboxRoot: tmpDir });
      expect(new TextDecoder().decode(result)).toBe("hello native");
    });

    test("throws WarpGridFsNotFoundError for missing file", async () => {
      await expect(
        readFile(`${tmpDir}/nope.txt`, { mode: "native", sandboxRoot: tmpDir }),
      ).rejects.toBeInstanceOf(WarpGridFsNotFoundError);
    });

    test("throws WarpGridFsPermissionError for path traversal outside sandbox", async () => {
      await expect(
        readFile(`${tmpDir}/../../etc/passwd`, { mode: "native", sandboxRoot: tmpDir }),
      ).rejects.toBeInstanceOf(WarpGridFsPermissionError);
    });
  });

  describe("readTextFile", () => {
    test("reads a file as UTF-8 string", async () => {
      const result = await readTextFile(`${tmpDir}/hello.txt`, { mode: "native", sandboxRoot: tmpDir });
      expect(result).toBe("hello native");
    });
  });

  describe("writeFile", () => {
    test("writes string data", async () => {
      await writeFile(`${tmpDir}/new.txt`, "written", { mode: "native", sandboxRoot: tmpDir });
      const content = await Bun.file(`${tmpDir}/new.txt`).text();
      expect(content).toBe("written");
    });

    test("writes Uint8Array data", async () => {
      const data = new Uint8Array([0x01, 0x02]);
      await writeFile(`${tmpDir}/new.bin`, data, { mode: "native", sandboxRoot: tmpDir });
      const content = new Uint8Array(await Bun.file(`${tmpDir}/new.bin`).arrayBuffer());
      expect(content).toEqual(data);
    });

    test("throws WarpGridFsPermissionError for path traversal outside sandbox", async () => {
      await expect(
        writeFile(`${tmpDir}/../escape.txt`, "bad", { mode: "native", sandboxRoot: tmpDir }),
      ).rejects.toBeInstanceOf(WarpGridFsPermissionError);
    });
  });

  describe("exists", () => {
    test("returns true for existing file", async () => {
      const result = await exists(`${tmpDir}/hello.txt`, { mode: "native", sandboxRoot: tmpDir });
      expect(result).toBe(true);
    });

    test("returns false for missing file", async () => {
      const result = await exists(`${tmpDir}/missing`, { mode: "native", sandboxRoot: tmpDir });
      expect(result).toBe(false);
    });
  });
});
