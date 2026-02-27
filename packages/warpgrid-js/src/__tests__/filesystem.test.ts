import { describe, it, expect, vi, beforeEach } from "vitest";
import { WarpGridFilesystem } from "../filesystem.js";
import { WarpGridFSError } from "../errors.js";
import type { FilesystemBindings } from "../types.js";

function createMockBindings(
  overrides: Partial<FilesystemBindings> = {},
): FilesystemBindings {
  return {
    openVirtual: vi.fn().mockReturnValue(1n),
    readVirtual: vi
      .fn()
      .mockReturnValueOnce(new Uint8Array([72, 101, 108, 108, 111]))
      .mockReturnValue(new Uint8Array(0)),
    closeVirtual: vi.fn(),
    ...overrides,
  };
}

describe("WarpGridFilesystem", () => {
  let bindings: FilesystemBindings;
  let fs: WarpGridFilesystem;

  beforeEach(() => {
    bindings = createMockBindings();
    fs = new WarpGridFilesystem(bindings);
  });

  describe("readFile(path)", () => {
    it("returns a Uint8Array with file content", async () => {
      const result = await fs.readFile("/etc/resolv.conf");

      expect(result).toBeInstanceOf(Uint8Array);
      expect(result.length).toBe(5);
      expect(result).toEqual(new Uint8Array([72, 101, 108, 108, 111]));
    });

    it("calls openVirtual, readVirtual, and closeVirtual in sequence", async () => {
      await fs.readFile("/etc/hosts");

      expect(bindings.openVirtual).toHaveBeenCalledWith("/etc/hosts");
      expect(bindings.readVirtual).toHaveBeenCalledWith(1n, expect.any(Number));
      expect(bindings.closeVirtual).toHaveBeenCalledWith(1n);
    });

    it("reads until empty chunk is returned (EOF)", async () => {
      const chunk1 = new Uint8Array([1, 2, 3]);
      const chunk2 = new Uint8Array([4, 5]);
      const emptyChunk = new Uint8Array(0);

      const multiChunkBindings = createMockBindings({
        readVirtual: vi
          .fn()
          .mockReturnValueOnce(chunk1)
          .mockReturnValueOnce(chunk2)
          .mockReturnValue(emptyChunk),
      });
      const multiFs = new WarpGridFilesystem(multiChunkBindings);

      const result = await multiFs.readFile("/some/path");
      expect(result).toEqual(new Uint8Array([1, 2, 3, 4, 5]));
    });

    it("closes the handle even when readVirtual throws", async () => {
      const failBindings = createMockBindings({
        readVirtual: vi.fn().mockImplementation(() => {
          throw "read error";
        }),
      });
      const failFs = new WarpGridFilesystem(failBindings);

      await expect(failFs.readFile("/some/path")).rejects.toThrow(
        WarpGridFSError,
      );
      expect(failBindings.closeVirtual).toHaveBeenCalledWith(1n);
    });
  });

  describe("readFile(path, encoding)", () => {
    it("returns a string when encoding is 'utf-8'", async () => {
      const result = await fs.readFile("/etc/resolv.conf", "utf-8");

      expect(typeof result).toBe("string");
      expect(result).toBe("Hello");
    });

    it("supports 'utf8' as an alias for 'utf-8'", async () => {
      const result = await fs.readFile("/etc/resolv.conf", "utf8");

      expect(typeof result).toBe("string");
      expect(result).toBe("Hello");
    });
  });

  describe("error handling", () => {
    it("throws WarpGridFSError when path does not exist", async () => {
      const failBindings = createMockBindings({
        openVirtual: vi.fn().mockImplementation(() => {
          throw "not a virtual path: /nonexistent";
        }),
      });
      const failFs = new WarpGridFilesystem(failBindings);

      await expect(failFs.readFile("/nonexistent")).rejects.toThrow(
        WarpGridFSError,
      );
    });

    it("includes the path in the error message for nonexistent files", async () => {
      const failBindings = createMockBindings({
        openVirtual: vi.fn().mockImplementation(() => {
          throw "not a virtual path: /missing/file";
        }),
      });
      const failFs = new WarpGridFilesystem(failBindings);

      await expect(failFs.readFile("/missing/file")).rejects.toThrow(
        /\/missing\/file/,
      );
    });

    it("WarpGridFSError has correct name", () => {
      const err = new WarpGridFSError("/some/path", "file not found");
      expect(err.name).toBe("WarpGridFSError");
    });

    it("WarpGridFSError extends WarpGridError", async () => {
      const { WarpGridError } = await import("../errors.js");
      const err = new WarpGridFSError("/some/path", "file not found");
      expect(err).toBeInstanceOf(WarpGridError);
    });

    it("WarpGridFSError preserves the path", () => {
      const err = new WarpGridFSError("/etc/hosts", "permission denied");
      expect(err.path).toBe("/etc/hosts");
    });
  });

  describe("path traversal prevention", () => {
    it("rejects paths with '..' components", async () => {
      await expect(fs.readFile("/../etc/passwd")).rejects.toThrow(
        WarpGridFSError,
      );
      await expect(fs.readFile("/etc/../etc/passwd")).rejects.toThrow(
        WarpGridFSError,
      );
    });

    it("rejects relative paths (not starting with /)", async () => {
      await expect(fs.readFile("etc/resolv.conf")).rejects.toThrow(
        WarpGridFSError,
      );
    });

    it("rejects empty paths", async () => {
      await expect(fs.readFile("")).rejects.toThrow(WarpGridFSError);
    });

    it("does not call bindings for rejected paths", async () => {
      try {
        await fs.readFile("../../etc/passwd");
      } catch {
        // expected
      }

      expect(bindings.openVirtual).not.toHaveBeenCalled();
    });
  });
});
