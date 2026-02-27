import { describe, it, expect } from "vitest";
import {
  validateCompatEntry,
  loadCompatResults,
  compareWithBaseline,
  generateCompatTable,
  type PackageCompatEntry,
  type CompatResults,
  type CompatStatus,
} from "../compat/audit.js";

describe("validateCompatEntry", () => {
  it("accepts a valid entry with all fields", () => {
    const entry: PackageCompatEntry = {
      name: "zod",
      version: "3.22.4",
      importStatus: "pass",
      functionCallStatus: "pass",
      realisticUsageStatus: "pass",
      blockingIssue: null,
      workaround: null,
      notes: "Pure TypeScript, no Node.js dependencies",
    };
    expect(() => validateCompatEntry(entry)).not.toThrow();
  });

  it("rejects an entry with missing name", () => {
    const entry = {
      version: "1.0.0",
      importStatus: "pass",
      functionCallStatus: "pass",
      realisticUsageStatus: "pass",
      blockingIssue: null,
      workaround: null,
      notes: "",
    };
    expect(() => validateCompatEntry(entry as unknown as PackageCompatEntry)).toThrow(
      /name/
    );
  });

  it("rejects an entry with invalid status value", () => {
    const entry = {
      name: "test",
      version: "1.0.0",
      importStatus: "invalid",
      functionCallStatus: "pass",
      realisticUsageStatus: "pass",
      blockingIssue: null,
      workaround: null,
      notes: "",
    };
    expect(() => validateCompatEntry(entry as unknown as PackageCompatEntry)).toThrow(
      /status/i
    );
  });

  it("accepts entries with null blocking issue and workaround", () => {
    const entry: PackageCompatEntry = {
      name: "lodash-es",
      version: "4.17.21",
      importStatus: "pass",
      functionCallStatus: "pass",
      realisticUsageStatus: "pass",
      blockingIssue: null,
      workaround: null,
      notes: "Pure ESM, fully compatible",
    };
    expect(() => validateCompatEntry(entry)).not.toThrow();
  });
});

describe("compareWithBaseline", () => {
  const baseline: CompatResults = {
    runtime: "componentize-js",
    runtimeVersion: "0.18.4",
    shimVersion: "none",
    testedAt: "2026-02-26T00:00:00Z",
    packages: [
      {
        name: "pg",
        version: "8.13.1",
        importStatus: "fail",
        functionCallStatus: "fail",
        realisticUsageStatus: "fail",
        blockingIssue: "Requires Node.js net module for TCP connections",
        workaround: null,
        notes: "Cannot connect to Postgres without net socket API",
      },
      {
        name: "zod",
        version: "3.22.4",
        importStatus: "pass",
        functionCallStatus: "pass",
        realisticUsageStatus: "pass",
        blockingIssue: null,
        workaround: null,
        notes: "Pure TypeScript, fully compatible",
      },
      {
        name: "dotenv",
        version: "16.4.7",
        importStatus: "pass",
        functionCallStatus: "fail",
        realisticUsageStatus: "fail",
        blockingIssue: "Requires fs.readFileSync for .env file reading",
        workaround: "Use WASI environment variables directly",
        notes: "Import succeeds but config() fails without fs",
      },
    ],
  };

  const warpgrid: CompatResults = {
    runtime: "componentize-js",
    runtimeVersion: "0.18.4",
    shimVersion: "warpgrid-0.1.0",
    testedAt: "2026-02-26T00:00:00Z",
    packages: [
      {
        name: "pg",
        version: "8.13.1",
        importStatus: "pass",
        functionCallStatus: "pass",
        realisticUsageStatus: "pass",
        blockingIssue: null,
        workaround: null,
        notes: "IMPROVED: Works via warpgrid/pg database proxy shim",
      },
      {
        name: "zod",
        version: "3.22.4",
        importStatus: "pass",
        functionCallStatus: "pass",
        realisticUsageStatus: "pass",
        blockingIssue: null,
        workaround: null,
        notes: "Pure TypeScript, fully compatible",
      },
      {
        name: "dotenv",
        version: "16.4.7",
        importStatus: "pass",
        functionCallStatus: "pass",
        realisticUsageStatus: "pass",
        blockingIssue: null,
        workaround: null,
        notes: "IMPROVED: Works via warpgrid.fs.readFile() shim",
      },
    ],
  };

  it("identifies packages improved by WarpGrid shims", () => {
    const improved = compareWithBaseline(baseline, warpgrid);
    const names = improved.map((p) => p.name);
    expect(names).toContain("pg");
    expect(names).toContain("dotenv");
  });

  it("does not mark already-compatible packages as improved", () => {
    const improved = compareWithBaseline(baseline, warpgrid);
    const names = improved.map((p) => p.name);
    expect(names).not.toContain("zod");
  });

  it("detects improvement at any of the three test levels", () => {
    const improved = compareWithBaseline(baseline, warpgrid);
    const dotenv = improved.find((p) => p.name === "dotenv");
    expect(dotenv).toBeDefined();
    expect(dotenv?.previousStatus).toBe("fail");
    expect(dotenv?.currentStatus).toBe("pass");
  });

  it("returns empty array when no improvements exist", () => {
    const improved = compareWithBaseline(baseline, baseline);
    expect(improved).toEqual([]);
  });

  it("handles packages present in warpgrid but not baseline", () => {
    const emptyBaseline: CompatResults = {
      ...baseline,
      packages: [],
    };
    const improved = compareWithBaseline(emptyBaseline, warpgrid);
    expect(improved.length).toBe(0);
  });
});

describe("generateCompatTable", () => {
  it("generates a valid markdown table with headers", () => {
    const results: CompatResults = {
      runtime: "componentize-js",
      runtimeVersion: "0.18.4",
      shimVersion: "warpgrid-0.1.0",
      testedAt: "2026-02-26T00:00:00Z",
      packages: [
        {
          name: "zod",
          version: "3.22.4",
          importStatus: "pass",
          functionCallStatus: "pass",
          realisticUsageStatus: "pass",
          blockingIssue: null,
          workaround: null,
          notes: "Pure TypeScript",
        },
      ],
    };
    const table = generateCompatTable(results, []);
    expect(table).toContain("| Package");
    expect(table).toContain("| zod");
    expect(table).toContain("pass");
  });

  it("marks improved packages in the table", () => {
    const results: CompatResults = {
      runtime: "componentize-js",
      runtimeVersion: "0.18.4",
      shimVersion: "warpgrid-0.1.0",
      testedAt: "2026-02-26T00:00:00Z",
      packages: [
        {
          name: "pg",
          version: "8.13.1",
          importStatus: "pass",
          functionCallStatus: "pass",
          realisticUsageStatus: "pass",
          blockingIssue: null,
          workaround: null,
          notes: "IMPROVED: Works via database proxy",
        },
      ],
    };
    const improved = [{ name: "pg", previousStatus: "fail" as CompatStatus, currentStatus: "pass" as CompatStatus }];
    const table = generateCompatTable(results, improved);
    expect(table).toContain("IMPROVED");
  });

  it("includes summary section with counts", () => {
    const results: CompatResults = {
      runtime: "componentize-js",
      runtimeVersion: "0.18.4",
      shimVersion: "warpgrid-0.1.0",
      testedAt: "2026-02-26T00:00:00Z",
      packages: [
        {
          name: "zod",
          version: "3.22.4",
          importStatus: "pass",
          functionCallStatus: "pass",
          realisticUsageStatus: "pass",
          blockingIssue: null,
          workaround: null,
          notes: "",
        },
        {
          name: "pg",
          version: "8.13.1",
          importStatus: "fail",
          functionCallStatus: "fail",
          realisticUsageStatus: "fail",
          blockingIssue: "net module",
          workaround: null,
          notes: "",
        },
      ],
    };
    const table = generateCompatTable(results, []);
    expect(table).toContain("1/2");
  });

  it("handles empty package list gracefully", () => {
    const results: CompatResults = {
      runtime: "componentize-js",
      runtimeVersion: "0.18.4",
      shimVersion: "none",
      testedAt: "2026-02-26T00:00:00Z",
      packages: [],
    };
    const table = generateCompatTable(results, []);
    expect(table).toContain("0/0");
  });
});

describe("loadCompatResults", () => {
  it("parses valid JSON into CompatResults", () => {
    const json = JSON.stringify({
      runtime: "componentize-js",
      runtimeVersion: "0.18.4",
      shimVersion: "none",
      testedAt: "2026-02-26T00:00:00Z",
      packages: [
        {
          name: "zod",
          version: "3.22.4",
          importStatus: "pass",
          functionCallStatus: "pass",
          realisticUsageStatus: "pass",
          blockingIssue: null,
          workaround: null,
          notes: "Works",
        },
      ],
    });
    const results = loadCompatResults(json);
    expect(results.packages).toHaveLength(1);
    expect(results.packages[0]?.name).toBe("zod");
  });

  it("throws on invalid JSON", () => {
    expect(() => loadCompatResults("not json")).toThrow();
  });

  it("throws when packages array is missing", () => {
    const json = JSON.stringify({
      runtime: "componentize-js",
      runtimeVersion: "0.18.4",
      shimVersion: "none",
      testedAt: "2026-02-26T00:00:00Z",
    });
    expect(() => loadCompatResults(json)).toThrow(/packages/i);
  });
});
