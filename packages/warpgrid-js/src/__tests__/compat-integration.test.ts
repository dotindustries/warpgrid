/**
 * Integration tests for npm package and core API compatibility testing.
 *
 * These tests validate the actual compat-db JSON files (not mocks)
 * and verify the comparison logic produces correct results for both
 * npm packages (US-409) and core Node.js APIs (US-402).
 */
import { describe, it, expect, beforeAll } from "vitest";
import { readFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import {
  loadCompatResults,
  compareWithBaseline,
  compareCoreApis,
  generateCompatTable,
  type CompatResults,
  type ImprovedPackage,
  type ImprovedCoreApi,
} from "../compat/audit.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const PROJECT_ROOT = resolve(__dirname, "..", "..", "..", "..");

let baseline: CompatResults;
let warpgrid: CompatResults;
let improved: readonly ImprovedPackage[];
let improvedCoreApis: readonly ImprovedCoreApi[];

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
  improvedCoreApis = compareCoreApis(baseline, warpgrid);
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

describe("baseline core APIs validation", () => {
  it("contains exactly 11 core API entries", () => {
    expect(baseline.coreApis).toHaveLength(11);
  });

  it("includes all 11 expected API names", () => {
    const expected = [
      "Buffer", "TextEncoder/TextDecoder", "crypto", "URL", "console",
      "setTimeout", "process.env", "fs", "net", "dns", "http",
    ];
    const names = (baseline.coreApis ?? []).map((a) => a.name);
    for (const name of expected) {
      expect(names).toContain(name);
    }
  });

  it("has valid status values for every core API entry", () => {
    const validStatuses = ["pass", "partial", "fail"];
    for (const api of baseline.coreApis ?? []) {
      expect(validStatuses).toContain(api.available);
      expect(validStatuses).toContain(api.functionalLevel);
      expect(typeof api.notes).toBe("string");
    }
  });

  it("marks platform-available APIs as pass", () => {
    const passApis = ["Buffer", "TextEncoder/TextDecoder", "URL", "console"];
    for (const name of passApis) {
      const api = (baseline.coreApis ?? []).find((a) => a.name === name);
      expect(api?.available).toBe("pass");
      expect(api?.functionalLevel).toBe("pass");
    }
  });

  it("marks missing platform APIs as fail", () => {
    const failApis = ["process.env", "fs", "net", "dns", "http"];
    for (const name of failApis) {
      const api = (baseline.coreApis ?? []).find((a) => a.name === name);
      expect(api?.available).toBe("fail");
      expect(api?.functionalLevel).toBe("fail");
    }
  });

  it("marks partially available APIs as partial", () => {
    const partialApis = ["crypto", "setTimeout"];
    for (const name of partialApis) {
      const api = (baseline.coreApis ?? []).find((a) => a.name === name);
      expect(api?.available).toBe("partial");
      expect(api?.functionalLevel).toBe("partial");
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

describe("warpgrid core APIs validation", () => {
  it("contains exactly 11 core API entries", () => {
    expect(warpgrid.coreApis).toHaveLength(11);
  });

  it("includes the same 11 API names as baseline", () => {
    const baselineNames = new Set((baseline.coreApis ?? []).map((a) => a.name));
    const warpgridNames = new Set((warpgrid.coreApis ?? []).map((a) => a.name));
    expect(warpgridNames).toEqual(baselineNames);
  });

  it("has valid status values for every core API entry", () => {
    const validStatuses = ["pass", "partial", "fail"];
    for (const api of warpgrid.coreApis ?? []) {
      expect(validStatuses).toContain(api.available);
      expect(validStatuses).toContain(api.functionalLevel);
    }
  });

  it("shows process.env improved to pass with WarpGrid shims", () => {
    const api = (warpgrid.coreApis ?? []).find((a) => a.name === "process.env");
    expect(api?.available).toBe("pass");
    expect(api?.functionalLevel).toBe("pass");
  });

  it("shows fs improved to partial with WarpGrid virtual filesystem", () => {
    const api = (warpgrid.coreApis ?? []).find((a) => a.name === "fs");
    expect(api?.available).toBe("partial");
    expect(api?.functionalLevel).toBe("partial");
  });

  it("shows dns improved to pass with WarpGrid DNS shim", () => {
    const api = (warpgrid.coreApis ?? []).find((a) => a.name === "dns");
    expect(api?.available).toBe("pass");
    expect(api?.functionalLevel).toBe("pass");
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

describe("comparison — improved core APIs", () => {
  it("identifies process.env as improved", () => {
    expect(improvedCoreApis.map((a) => a.name)).toContain("process.env");
  });

  it("identifies fs as improved", () => {
    expect(improvedCoreApis.map((a) => a.name)).toContain("fs");
  });

  it("identifies dns as improved", () => {
    expect(improvedCoreApis.map((a) => a.name)).toContain("dns");
  });

  it("does not mark already-available APIs as improved", () => {
    const names = improvedCoreApis.map((a) => a.name);
    expect(names).not.toContain("Buffer");
    expect(names).not.toContain("TextEncoder/TextDecoder");
    expect(names).not.toContain("URL");
    expect(names).not.toContain("console");
  });

  it("does not mark still-failing APIs as improved", () => {
    const names = improvedCoreApis.map((a) => a.name);
    expect(names).not.toContain("net");
    expect(names).not.toContain("http");
  });

  it("tracks correct before/after for process.env", () => {
    const api = improvedCoreApis.find((a) => a.name === "process.env");
    expect(api?.previousStatus).toBe("fail");
    expect(api?.currentStatus).toBe("pass");
  });

  it("tracks correct before/after for fs", () => {
    const api = improvedCoreApis.find((a) => a.name === "fs");
    expect(api?.previousStatus).toBe("fail");
    expect(api?.currentStatus).toBe("partial");
  });

  it("tracks correct before/after for dns", () => {
    const api = improvedCoreApis.find((a) => a.name === "dns");
    expect(api?.previousStatus).toBe("fail");
    expect(api?.currentStatus).toBe("pass");
  });
});

describe("markdown table generation", () => {
  it("generates a table with all 20 packages", () => {
    const table = generateCompatTable(warpgrid, improved, improvedCoreApis);
    const dataLines = table.split("\n").filter((line) =>
      line.startsWith("| ") && !line.startsWith("| Package") && !line.startsWith("|---")
        && !line.startsWith("| API")
    );
    // 20 packages + 11 core APIs = 31 data rows
    expect(dataLines).toHaveLength(31);
  });

  it("includes IMPROVED markers for improved packages", () => {
    const table = generateCompatTable(warpgrid, improved, improvedCoreApis);
    expect(table).toContain("IMPROVED");
    const improvedCount = (table.match(/IMPROVED/g) ?? []).length;
    // At minimum: improved packages + improved core APIs (in table rows and detail sections)
    expect(improvedCount).toBeGreaterThanOrEqual(improved.length + improvedCoreApis.length);
  });

  it("includes all package names in the table", () => {
    const table = generateCompatTable(warpgrid, improved, improvedCoreApis);
    for (const pkg of warpgrid.packages) {
      expect(table).toContain(pkg.name);
    }
  });

  it("includes Core Node.js APIs section", () => {
    const table = generateCompatTable(warpgrid, improved, improvedCoreApis);
    expect(table).toContain("## Core Node.js APIs");
  });

  it("includes all core API names in the table", () => {
    const table = generateCompatTable(warpgrid, improved, improvedCoreApis);
    for (const api of warpgrid.coreApis ?? []) {
      expect(table).toContain(api.name);
    }
  });

  it("includes core API improvement details section", () => {
    const table = generateCompatTable(warpgrid, improved, improvedCoreApis);
    expect(table).toContain("## Core APIs Improved by WarpGrid Shims");
  });

  it("includes core API counts in summary", () => {
    const table = generateCompatTable(warpgrid, improved, improvedCoreApis);
    expect(table).toContain("Core APIs available");
  });
});
