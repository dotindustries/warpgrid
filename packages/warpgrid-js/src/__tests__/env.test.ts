import { describe, it, expect, vi } from "vitest";
import { createProcessEnv } from "../env.js";
import type { EnvironmentBindings } from "../types.js";

function createMockBindings(
  vars: Array<[string, string]> = [],
): EnvironmentBindings {
  return {
    getEnvironment: vi.fn().mockReturnValue(vars),
  };
}

describe("createProcessEnv()", () => {
  it("returns an object with environment variables", () => {
    const env = createProcessEnv(
      createMockBindings([
        ["FOO", "bar"],
        ["DB_HOST", "localhost"],
      ]),
    );

    expect(env.FOO).toBe("bar");
    expect(env.DB_HOST).toBe("localhost");
  });

  it("returns undefined for missing variables", () => {
    const env = createProcessEnv(createMockBindings([["FOO", "bar"]]));

    expect(env.MISSING_VAR).toBeUndefined();
  });

  it("returns an empty object when no variables are set", () => {
    const env = createProcessEnv(createMockBindings([]));

    expect(env.ANY_VAR).toBeUndefined();
  });

  it("supports 'in' operator for checking variable existence", () => {
    const env = createProcessEnv(
      createMockBindings([["PRESENT", "value"]]),
    );

    expect("PRESENT" in env).toBe(true);
    expect("ABSENT" in env).toBe(false);
  });

  it("supports Object.keys() enumeration", () => {
    const env = createProcessEnv(
      createMockBindings([
        ["A", "1"],
        ["B", "2"],
        ["C", "3"],
      ]),
    );

    const keys = Object.keys(env);
    expect(keys).toContain("A");
    expect(keys).toContain("B");
    expect(keys).toContain("C");
    expect(keys.length).toBe(3);
  });

  it("handles empty string values", () => {
    const env = createProcessEnv(createMockBindings([["EMPTY", ""]]));

    expect(env.EMPTY).toBe("");
    expect("EMPTY" in env).toBe(true);
  });

  it("handles values with special characters", () => {
    const env = createProcessEnv(
      createMockBindings([
        ["URL", "postgres://user:p@ss@localhost/db"],
        ["JSON", '{"key":"value"}'],
      ]),
    );

    expect(env.URL).toBe("postgres://user:p@ss@localhost/db");
    expect(env.JSON).toBe('{"key":"value"}');
  });

  it("calls getEnvironment only once (caches result)", () => {
    const bindings = createMockBindings([["FOO", "bar"]]);
    const env = createProcessEnv(bindings);

    // Access multiple times
    void env.FOO;
    void env.FOO;
    void env.BAR;

    expect(bindings.getEnvironment).toHaveBeenCalledTimes(1);
  });
});
