import { describe, it, expect } from "vitest";
import { WarpGridError } from "../errors.js";

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
