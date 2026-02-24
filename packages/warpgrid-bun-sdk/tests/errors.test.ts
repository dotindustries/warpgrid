import { describe, test, expect } from "bun:test";
import { WarpGridError, WarpGridDatabaseError } from "../src/errors.ts";

describe("WarpGridError", () => {
  test("is an instance of Error", () => {
    const err = new WarpGridError("test");
    expect(err).toBeInstanceOf(Error);
    expect(err).toBeInstanceOf(WarpGridError);
  });

  test("has correct name", () => {
    const err = new WarpGridError("test");
    expect(err.name).toBe("WarpGridError");
  });

  test("stores message", () => {
    const err = new WarpGridError("something went wrong");
    expect(err.message).toBe("something went wrong");
  });

  test("stores cause when provided", () => {
    const cause = new Error("root cause");
    const err = new WarpGridError("wrapper", { cause });
    expect(err.cause).toBe(cause);
  });

  test("has undefined cause when not provided", () => {
    const err = new WarpGridError("no cause");
    expect(err.cause).toBeUndefined();
  });
});

describe("WarpGridDatabaseError", () => {
  test("is an instance of WarpGridError and Error", () => {
    const err = new WarpGridDatabaseError("db error");
    expect(err).toBeInstanceOf(Error);
    expect(err).toBeInstanceOf(WarpGridError);
    expect(err).toBeInstanceOf(WarpGridDatabaseError);
  });

  test("has correct name", () => {
    const err = new WarpGridDatabaseError("db error");
    expect(err.name).toBe("WarpGridDatabaseError");
  });

  test("stores message and cause", () => {
    const cause = new TypeError("type mismatch");
    const err = new WarpGridDatabaseError("connection failed", { cause });
    expect(err.message).toBe("connection failed");
    expect(err.cause).toBe(cause);
  });

  test("cause can be a non-Error value", () => {
    const err = new WarpGridDatabaseError("failed", {
      cause: "string cause",
    });
    expect(err.cause).toBe("string cause");
  });
});
