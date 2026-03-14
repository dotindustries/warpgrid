import { describe, it, expect } from "vitest";
import {
  validateCompatEntry,
  validateCoreApiEntry,
  loadCompatResults,
  compareWithBaseline,
  compareCoreApis,
  generateCompatTable,
  type PackageCompatEntry,
  type CoreApiEntry,
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

  it("rejects an entry with empty version", () => {
    const entry = {
      name: "test",
      version: "",
      importStatus: "pass",
      functionCallStatus: "pass",
      realisticUsageStatus: "pass",
      blockingIssue: null,
      workaround: null,
      notes: "",
    };
    expect(() => validateCompatEntry(entry as unknown as PackageCompatEntry)).toThrow(
      /version/
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

describe("validateCoreApiEntry", () => {
  it("accepts a valid core API entry with all fields", () => {
    const entry: CoreApiEntry = {
      name: "Buffer",
      available: "pass",
      functionalLevel: "pass",
      blockingIssue: null,
      workaround: null,
      notes: "Polyfilled by StarlingMonkey",
    };
    expect(() => validateCoreApiEntry(entry)).not.toThrow();
  });

  it("accepts entries with partial status", () => {
    const entry: CoreApiEntry = {
      name: "crypto",
      available: "partial",
      functionalLevel: "partial",
      blockingIssue: "No createHash/createHmac",
      workaround: "Use SubtleCrypto API",
      notes: "crypto.getRandomValues + SubtleCrypto available",
    };
    expect(() => validateCoreApiEntry(entry)).not.toThrow();
  });

  it("accepts entries with fail status", () => {
    const entry: CoreApiEntry = {
      name: "fs",
      available: "fail",
      functionalLevel: "fail",
      blockingIssue: "No filesystem access in stock ComponentizeJS",
      workaround: null,
      notes: "Not available",
    };
    expect(() => validateCoreApiEntry(entry)).not.toThrow();
  });

  it("accepts all valid status combinations", () => {
    const statuses: CompatStatus[] = ["pass", "partial", "fail"];
    for (const available of statuses) {
      for (const functionalLevel of statuses) {
        const entry: CoreApiEntry = {
          name: "test-api",
          available,
          functionalLevel,
          blockingIssue: null,
          workaround: null,
          notes: "",
        };
        expect(() => validateCoreApiEntry(entry)).not.toThrow();
      }
    }
  });

  it("rejects an entry with missing name", () => {
    const entry = {
      available: "pass",
      functionalLevel: "pass",
      blockingIssue: null,
      workaround: null,
      notes: "",
    };
    expect(() => validateCoreApiEntry(entry as unknown as CoreApiEntry)).toThrow(
      /name/
    );
  });

  it("rejects an entry with empty name", () => {
    const entry: CoreApiEntry = {
      name: "",
      available: "pass",
      functionalLevel: "pass",
      blockingIssue: null,
      workaround: null,
      notes: "",
    };
    expect(() => validateCoreApiEntry(entry)).toThrow(/name/);
  });

  it("rejects an entry with invalid available status", () => {
    const entry = {
      name: "test",
      available: "invalid",
      functionalLevel: "pass",
      blockingIssue: null,
      workaround: null,
      notes: "",
    };
    expect(() => validateCoreApiEntry(entry as unknown as CoreApiEntry)).toThrow(
      /status/i
    );
  });

  it("rejects an entry with invalid functionalLevel status", () => {
    const entry = {
      name: "test",
      available: "pass",
      functionalLevel: "unknown",
      blockingIssue: null,
      workaround: null,
      notes: "",
    };
    expect(() => validateCoreApiEntry(entry as unknown as CoreApiEntry)).toThrow(
      /status/i
    );
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

  it("excludes packages that regressed from baseline to warpgrid", () => {
    const regressed: CompatResults = {
      ...warpgrid,
      packages: [
        {
          name: "pg",
          version: "8.13.1",
          importStatus: "fail",
          functionCallStatus: "fail",
          realisticUsageStatus: "fail",
          blockingIssue: "Hypothetical regression",
          workaround: null,
          notes: "Regressed",
        },
      ],
    };
    const improved = compareWithBaseline(baseline, regressed);
    expect(improved.map((p) => p.name)).not.toContain("pg");
    expect(improved.length).toBe(0);
  });
});

describe("compareCoreApis", () => {
  const baseline: CompatResults = {
    runtime: "componentize-js",
    runtimeVersion: "0.18.4",
    shimVersion: "none",
    testedAt: "2026-02-26T00:00:00Z",
    packages: [],
    coreApis: [
      {
        name: "process.env",
        available: "fail",
        functionalLevel: "fail",
        blockingIssue: "Not available in stock ComponentizeJS",
        workaround: null,
        notes: "No process global",
      },
      {
        name: "Buffer",
        available: "pass",
        functionalLevel: "pass",
        blockingIssue: null,
        workaround: null,
        notes: "Polyfilled by StarlingMonkey",
      },
      {
        name: "fs",
        available: "fail",
        functionalLevel: "fail",
        blockingIssue: "No filesystem",
        workaround: null,
        notes: "Not available",
      },
    ],
  };

  const warpgrid: CompatResults = {
    runtime: "componentize-js",
    runtimeVersion: "0.18.4",
    shimVersion: "warpgrid-0.1.0",
    testedAt: "2026-02-26T00:00:00Z",
    packages: [],
    coreApis: [
      {
        name: "process.env",
        available: "pass",
        functionalLevel: "pass",
        blockingIssue: null,
        workaround: null,
        notes: "IMPROVED: WASI env polyfill",
      },
      {
        name: "Buffer",
        available: "pass",
        functionalLevel: "pass",
        blockingIssue: null,
        workaround: null,
        notes: "Polyfilled by StarlingMonkey",
      },
      {
        name: "fs",
        available: "partial",
        functionalLevel: "partial",
        blockingIssue: "Only virtual filesystem paths",
        workaround: null,
        notes: "IMPROVED: Virtual filesystem for specific paths",
      },
    ],
  };

  it("identifies core APIs improved by WarpGrid shims", () => {
    const improved = compareCoreApis(baseline, warpgrid);
    const names = improved.map((a) => a.name);
    expect(names).toContain("process.env");
    expect(names).toContain("fs");
  });

  it("does not mark already-available APIs as improved", () => {
    const improved = compareCoreApis(baseline, warpgrid);
    const names = improved.map((a) => a.name);
    expect(names).not.toContain("Buffer");
  });

  it("tracks correct before/after statuses", () => {
    const improved = compareCoreApis(baseline, warpgrid);
    const processEnv = improved.find((a) => a.name === "process.env");
    expect(processEnv).toBeDefined();
    expect(processEnv?.previousStatus).toBe("fail");
    expect(processEnv?.currentStatus).toBe("pass");

    const fs = improved.find((a) => a.name === "fs");
    expect(fs).toBeDefined();
    expect(fs?.previousStatus).toBe("fail");
    expect(fs?.currentStatus).toBe("partial");
  });

  it("returns empty array when no improvements exist", () => {
    const improved = compareCoreApis(baseline, baseline);
    expect(improved).toEqual([]);
  });

  it("handles missing coreApis gracefully", () => {
    const noCoreApis: CompatResults = {
      runtime: "componentize-js",
      runtimeVersion: "0.18.4",
      shimVersion: "none",
      testedAt: "2026-02-26T00:00:00Z",
      packages: [],
    };
    const improved = compareCoreApis(noCoreApis, warpgrid);
    expect(improved).toEqual([]);
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

  it("includes Core Node.js APIs section when coreApis present", () => {
    const results: CompatResults = {
      runtime: "componentize-js",
      runtimeVersion: "0.18.4",
      shimVersion: "warpgrid-0.1.0",
      testedAt: "2026-02-26T00:00:00Z",
      packages: [],
      coreApis: [
        {
          name: "Buffer",
          available: "pass",
          functionalLevel: "pass",
          blockingIssue: null,
          workaround: null,
          notes: "Polyfilled by StarlingMonkey",
        },
        {
          name: "fs",
          available: "fail",
          functionalLevel: "fail",
          blockingIssue: "No filesystem",
          workaround: null,
          notes: "Not available",
        },
      ],
    };
    const table = generateCompatTable(results, []);
    expect(table).toContain("## Core Node.js APIs");
    expect(table).toContain("| API");
    expect(table).toContain("| Buffer");
    expect(table).toContain("| fs");
    expect(table).toContain("Available");
    expect(table).toContain("Functional Level");
  });

  it("marks improved core APIs in the table", () => {
    const results: CompatResults = {
      runtime: "componentize-js",
      runtimeVersion: "0.18.4",
      shimVersion: "warpgrid-0.1.0",
      testedAt: "2026-02-26T00:00:00Z",
      packages: [],
      coreApis: [
        {
          name: "process.env",
          available: "pass",
          functionalLevel: "pass",
          blockingIssue: null,
          workaround: null,
          notes: "IMPROVED: WASI env polyfill",
        },
      ],
    };
    const improvedCoreApis = [
      { name: "process.env", previousStatus: "fail" as CompatStatus, currentStatus: "pass" as CompatStatus },
    ];
    const table = generateCompatTable(results, [], improvedCoreApis);
    expect(table).toContain("IMPROVED");
    expect(table).toContain("Core APIs Improved by WarpGrid Shims");
  });

  it("includes core API counts in summary", () => {
    const results: CompatResults = {
      runtime: "componentize-js",
      runtimeVersion: "0.18.4",
      shimVersion: "warpgrid-0.1.0",
      testedAt: "2026-02-26T00:00:00Z",
      packages: [],
      coreApis: [
        {
          name: "Buffer",
          available: "pass",
          functionalLevel: "pass",
          blockingIssue: null,
          workaround: null,
          notes: "",
        },
        {
          name: "fs",
          available: "fail",
          functionalLevel: "fail",
          blockingIssue: null,
          workaround: null,
          notes: "",
        },
      ],
    };
    const table = generateCompatTable(results, []);
    expect(table).toContain("Core APIs available");
    expect(table).toContain("1/2");
  });

  it("does not include core APIs section when coreApis is empty", () => {
    const results: CompatResults = {
      runtime: "componentize-js",
      runtimeVersion: "0.18.4",
      shimVersion: "none",
      testedAt: "2026-02-26T00:00:00Z",
      packages: [],
      coreApis: [],
    };
    const table = generateCompatTable(results, []);
    expect(table).not.toContain("Core Node.js APIs");
  });

  it("does not include core APIs section when coreApis is undefined", () => {
    const results: CompatResults = {
      runtime: "componentize-js",
      runtimeVersion: "0.18.4",
      shimVersion: "none",
      testedAt: "2026-02-26T00:00:00Z",
      packages: [],
    };
    const table = generateCompatTable(results, []);
    expect(table).not.toContain("Core Node.js APIs");
  });

  it("truncates notes longer than 60 characters in the table", () => {
    const longNote = "A".repeat(80);
    const results: CompatResults = {
      runtime: "componentize-js",
      runtimeVersion: "0.18.4",
      shimVersion: "none",
      testedAt: "2026-02-26T00:00:00Z",
      packages: [
        {
          name: "test-pkg",
          version: "1.0.0",
          importStatus: "pass",
          functionCallStatus: "pass",
          realisticUsageStatus: "pass",
          blockingIssue: null,
          workaround: null,
          notes: longNote,
        },
      ],
    };
    const table = generateCompatTable(results, []);
    expect(table).not.toContain(longNote);
    expect(table).toContain("...");
    // Truncated to 57 chars + "..." = 60 chars
    expect(table).toContain("A".repeat(57) + "...");
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

  it("parses JSON with coreApis array", () => {
    const json = JSON.stringify({
      runtime: "componentize-js",
      runtimeVersion: "0.18.4",
      shimVersion: "none",
      testedAt: "2026-02-26T00:00:00Z",
      packages: [],
      coreApis: [
        {
          name: "Buffer",
          available: "pass",
          functionalLevel: "pass",
          blockingIssue: null,
          workaround: null,
          notes: "Polyfilled by StarlingMonkey",
        },
        {
          name: "fs",
          available: "fail",
          functionalLevel: "fail",
          blockingIssue: "No filesystem",
          workaround: null,
          notes: "Not available",
        },
      ],
    });
    const results = loadCompatResults(json);
    expect(results.coreApis).toHaveLength(2);
    expect(results.coreApis?.[0]?.name).toBe("Buffer");
    expect(results.coreApis?.[0]?.available).toBe("pass");
    expect(results.coreApis?.[1]?.name).toBe("fs");
    expect(results.coreApis?.[1]?.available).toBe("fail");
  });

  it("defaults coreApis to empty array when not present in JSON", () => {
    const json = JSON.stringify({
      runtime: "componentize-js",
      runtimeVersion: "0.18.4",
      shimVersion: "none",
      testedAt: "2026-02-26T00:00:00Z",
      packages: [],
    });
    const results = loadCompatResults(json);
    expect(results.coreApis).toEqual([]);
  });

  it("validates coreApis entries and rejects invalid ones", () => {
    const json = JSON.stringify({
      runtime: "componentize-js",
      runtimeVersion: "0.18.4",
      shimVersion: "none",
      testedAt: "2026-02-26T00:00:00Z",
      packages: [],
      coreApis: [
        {
          name: "bad",
          available: "invalid-status",
          functionalLevel: "pass",
          blockingIssue: null,
          workaround: null,
          notes: "",
        },
      ],
    });
    expect(() => loadCompatResults(json)).toThrow(/status/i);
  });

  it("defaults missing metadata fields to 'unknown'", () => {
    const json = JSON.stringify({
      packages: [
        {
          name: "zod",
          version: "3.22.4",
          importStatus: "pass",
          functionCallStatus: "pass",
          realisticUsageStatus: "pass",
          notes: "",
        },
      ],
    });
    const results = loadCompatResults(json);
    expect(results.runtime).toBe("unknown");
    expect(results.runtimeVersion).toBe("unknown");
    expect(results.shimVersion).toBe("unknown");
  });

  it("defaults missing blockingIssue and workaround to null", () => {
    const json = JSON.stringify({
      runtime: "componentize-js",
      runtimeVersion: "0.18.4",
      shimVersion: "none",
      testedAt: "2026-02-26T00:00:00Z",
      packages: [
        {
          name: "test",
          version: "1.0.0",
          importStatus: "pass",
          functionCallStatus: "pass",
          realisticUsageStatus: "pass",
        },
      ],
    });
    const results = loadCompatResults(json);
    expect(results.packages[0]?.blockingIssue).toBeNull();
    expect(results.packages[0]?.workaround).toBeNull();
    expect(results.packages[0]?.notes).toBe("");
  });
});
