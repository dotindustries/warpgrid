/**
 * Integration tests for US-409: npm package compatibility testing.
 *
 * These tests validate the actual compat-db JSON files (not mocks)
 * and verify the comparison logic produces correct results.
 */
import { describe, it, expect, beforeAll } from "vitest";
import { readFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import {
  loadCompatResults,
  compareWithBaseline,
  generateCompatTable,
  type CompatResults,
  type ImprovedPackage,
} from "../compat/audit.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const PROJECT_ROOT = resolve(__dirname, "..", "..", "..", "..");

let baseline: CompatResults;
let warpgrid: CompatResults;
let improved: readonly ImprovedPackage[];

beforeAll(() => {
  const baselineJson = readFileSync(
    resolve(PROJECT_ROOT, "compat-db/componentize-js-baseline.json"),
    "utf-8"
  );
  const warpgridJson = readFileSync(
    resolve(PROJECT_ROOT, "compat-db/componentize-js-warpgrid.json"),
    "utf-8"
  );
  baseline = loadCompatResults(baselineJson);
  warpgrid = loadCompatResults(warpgridJson);
  improved = compareWithBaseline(baseline, warpgrid);
});

describe("baseline JSON validation", () => {
  it("contains exactly 20 packages", () => {
    expect(baseline.packages).toHaveLength(20);
  });

  it("has shimVersion set to none", () => {
    expect(baseline.shimVersion).toBe("none");
  });

  it("has runtime set to componentize-js", () => {
    expect(baseline.runtime).toBe("componentize-js");
  });

  it("includes all 20 expected package names", () => {
    const expected = [
      "pg", "ioredis", "express", "hono", "zod", "jose", "uuid",
      "lodash-es", "date-fns", "nanoid", "superjson", "drizzle-orm",
      "kysely", "better-sqlite3", "node-fetch", "axios", "dotenv",
      "pino", "fast-json-stringify", "ajv",
    ];
    const names = baseline.packages.map((p) => p.name);
    for (const name of expected) {
      expect(names).toContain(name);
    }
  });
});

describe("warpgrid JSON validation", () => {
  it("contains exactly 20 packages", () => {
    expect(warpgrid.packages).toHaveLength(20);
  });

  it("has shimVersion set to warpgrid-0.1.0", () => {
    expect(warpgrid.shimVersion).toBe("warpgrid-0.1.0");
  });

  it("tests at all three levels for every package", () => {
    for (const pkg of warpgrid.packages) {
      expect(["pass", "partial", "fail"]).toContain(pkg.importStatus);
      expect(["pass", "partial", "fail"]).toContain(pkg.functionCallStatus);
      expect(["pass", "partial", "fail"]).toContain(pkg.realisticUsageStatus);
    }
  });

  it("includes the same 20 package names as baseline", () => {
    const baselineNames = new Set(baseline.packages.map((p) => p.name));
    const warpgridNames = new Set(warpgrid.packages.map((p) => p.name));
    expect(warpgridNames).toEqual(baselineNames);
  });
});

describe("comparison — improved packages", () => {
  it("identifies pg as improved", () => {
    expect(improved.map((p) => p.name)).toContain("pg");
  });

  it("identifies drizzle-orm as improved", () => {
    expect(improved.map((p) => p.name)).toContain("drizzle-orm");
  });

  it("identifies dotenv as improved", () => {
    expect(improved.map((p) => p.name)).toContain("dotenv");
  });

  it("identifies jose as improved", () => {
    expect(improved.map((p) => p.name)).toContain("jose");
  });

  it("identifies kysely as improved", () => {
    expect(improved.map((p) => p.name)).toContain("kysely");
  });

  it("identifies pino as improved", () => {
    expect(improved.map((p) => p.name)).toContain("pino");
  });

  it("does not mark pure-JS packages as improved", () => {
    const names = improved.map((p) => p.name);
    expect(names).not.toContain("zod");
    expect(names).not.toContain("lodash-es");
    expect(names).not.toContain("hono");
    expect(names).not.toContain("uuid");
  });

  it("does not mark fundamentally incompatible packages as improved", () => {
    const names = improved.map((p) => p.name);
    expect(names).not.toContain("better-sqlite3");
    expect(names).not.toContain("express");
  });
});

describe("markdown table generation", () => {
  it("generates a table with all 20 packages", () => {
    const table = generateCompatTable(warpgrid, improved);
    const dataLines = table.split("\n").filter((line) =>
      line.startsWith("| ") && !line.startsWith("| Package") && !line.startsWith("|---")
    );
    expect(dataLines).toHaveLength(20);
  });

  it("includes IMPROVED markers for improved packages", () => {
    const table = generateCompatTable(warpgrid, improved);
    expect(table).toContain("IMPROVED");
    const improvedCount = (table.match(/IMPROVED/g) ?? []).length;
    expect(improvedCount).toBeGreaterThanOrEqual(improved.length);
  });

  it("includes all package names in the table", () => {
    const table = generateCompatTable(warpgrid, improved);
    for (const pkg of warpgrid.packages) {
      expect(table).toContain(pkg.name);
    }
  });
});
