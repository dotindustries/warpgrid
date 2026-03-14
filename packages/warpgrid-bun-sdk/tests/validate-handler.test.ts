import { describe, test, expect } from "bun:test";
import {
  validateHandler,
  WarpGridHandlerValidationError,
  type WarpGridHandler,
} from "../src/index.ts";

describe("validateHandler", () => {
  describe("valid handlers", () => {
    test("accepts handler with sync fetch", () => {
      const handler = {
        fetch(_req: Request) {
          return new Response("ok");
        },
      };
      expect(() => validateHandler(handler)).not.toThrow();
    });

    test("accepts handler with async fetch", () => {
      const handler = {
        async fetch(_req: Request) {
          return new Response("ok");
        },
      };
      expect(() => validateHandler(handler)).not.toThrow();
    });

    test("accepts handler with fetch and init", () => {
      const handler = {
        fetch(_req: Request) {
          return new Response("ok");
        },
        async init() {},
      };
      expect(() => validateHandler(handler)).not.toThrow();
    });
  });

  describe("invalid handlers", () => {
    test("throws for null", () => {
      expect(() => validateHandler(null)).toThrow(
        WarpGridHandlerValidationError,
      );
    });

    test("throws for undefined", () => {
      expect(() => validateHandler(undefined)).toThrow(
        WarpGridHandlerValidationError,
      );
    });

    test("throws for empty object (missing fetch)", () => {
      expect(() => validateHandler({})).toThrow(
        WarpGridHandlerValidationError,
      );
    });

    test("throws when fetch is a string", () => {
      expect(() => validateHandler({ fetch: "not a function" })).toThrow(
        WarpGridHandlerValidationError,
      );
    });

    test("throws when fetch is a number", () => {
      expect(() => validateHandler({ fetch: 42 })).toThrow(
        WarpGridHandlerValidationError,
      );
    });

    test("throws when fetch is an object", () => {
      expect(() => validateHandler({ fetch: {} })).toThrow(
        WarpGridHandlerValidationError,
      );
    });
  });

  describe("error messages", () => {
    test("null handler message includes 'null'", () => {
      expect(() => validateHandler(null)).toThrow(/null/);
    });

    test("undefined handler message includes 'undefined'", () => {
      expect(() => validateHandler(undefined)).toThrow(/undefined/);
    });

    test("missing fetch message includes guidance", () => {
      expect(() => validateHandler({})).toThrow(/missing a fetch/);
    });

    test("non-function fetch message includes actual type", () => {
      expect(() => validateHandler({ fetch: "oops" })).toThrow(/string/);
    });
  });

  describe("type narrowing", () => {
    test("narrows unknown to WarpGridHandler after validation", () => {
      const handler: unknown = {
        fetch(_req: Request) {
          return new Response("typed");
        },
      };
      validateHandler(handler);
      // After validation, handler is typed as WarpGridHandler
      const typed: WarpGridHandler = handler;
      expect(typeof typed.fetch).toBe("function");
    });
  });
});
