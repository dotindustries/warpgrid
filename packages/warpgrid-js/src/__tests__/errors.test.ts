import { describe, it, expect } from "vitest";
import { WarpGridError, WarpGridDNSError, WarpGridFSError } from "../errors.js";

describe("WarpGridError", () => {
  it("is an instance of Error", () => {
    const err = new WarpGridError("something failed");
    expect(err).toBeInstanceOf(Error);
    expect(err).toBeInstanceOf(WarpGridError);
  });

  it("preserves the error message", () => {
    const err = new WarpGridError("connection refused");
    expect(err.message).toBe("connection refused");
  });

  it("has the correct name property", () => {
    const err = new WarpGridError("test");
    expect(err.name).toBe("WarpGridError");
  });

  it("preserves the original cause when provided", () => {
    const cause = new Error("underlying issue");
    const err = new WarpGridError("wrapper message", { cause });
    expect(err.cause).toBe(cause);
  });

  it("captures a stack trace", () => {
    const err = new WarpGridError("with stack");
    expect(err.stack).toBeDefined();
    expect(err.stack).toContain("WarpGridError");
  });
});

describe("WarpGridDNSError", () => {
  it("is an instance of WarpGridError and Error", () => {
    const err = new WarpGridDNSError("bad.host", "ENOTFOUND");
    expect(err).toBeInstanceOf(Error);
    expect(err).toBeInstanceOf(WarpGridError);
    expect(err).toBeInstanceOf(WarpGridDNSError);
  });

  it("stores the hostname", () => {
    const err = new WarpGridDNSError("db.internal", "resolution failed");
    expect(err.hostname).toBe("db.internal");
  });

  it("has the correct name property", () => {
    const err = new WarpGridDNSError("host", "msg");
    expect(err.name).toBe("WarpGridDNSError");
  });

  it("preserves the original cause", () => {
    const cause = new Error("network error");
    const err = new WarpGridDNSError("host", "msg", { cause });
    expect(err.cause).toBe(cause);
  });
});

describe("WarpGridFSError", () => {
  it("is an instance of WarpGridError and Error", () => {
    const err = new WarpGridFSError("/etc/hosts", "not found");
    expect(err).toBeInstanceOf(Error);
    expect(err).toBeInstanceOf(WarpGridError);
    expect(err).toBeInstanceOf(WarpGridFSError);
  });

  it("stores the path", () => {
    const err = new WarpGridFSError("/usr/share/zoneinfo/UTC", "read failed");
    expect(err.path).toBe("/usr/share/zoneinfo/UTC");
  });

  it("has the correct name property", () => {
    const err = new WarpGridFSError("/path", "msg");
    expect(err.name).toBe("WarpGridFSError");
  });

  it("preserves the original cause", () => {
    const cause = new Error("permission denied");
    const err = new WarpGridFSError("/path", "msg", { cause });
    expect(err.cause).toBe(cause);
  });
});
