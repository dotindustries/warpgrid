/**
 * Compatibility audit infrastructure for npm package testing.
 *
 * Provides types, validation, comparison, and table generation
 * for documenting npm package compatibility with the ComponentizeJS
 * runtime — both baseline (stock) and WarpGrid-extended.
 *
 * Also covers core Node.js API availability auditing (Buffer,
 * crypto, fs, net, dns, http, etc.) to document what platform
 * APIs are usable before and after WarpGrid shims.
 */

/** Compatibility status for a single test level. */
export type CompatStatus = "pass" | "partial" | "fail";

/** A single npm package compatibility entry tested at 3 levels. */
export interface PackageCompatEntry {
  readonly name: string;
  readonly version: string;
  readonly importStatus: CompatStatus;
  readonly functionCallStatus: CompatStatus;
  readonly realisticUsageStatus: CompatStatus;
  readonly blockingIssue: string | null;
  readonly workaround: string | null;
  readonly notes: string;
}

/** A core Node.js API compatibility entry tested at 2 levels. */
export interface CoreApiEntry {
  readonly name: string;
  readonly available: CompatStatus;
  readonly functionalLevel: CompatStatus;
  readonly blockingIssue: string | null;
  readonly workaround: string | null;
  readonly notes: string;
}

/** Full compatibility results for a runtime configuration. */
export interface CompatResults {
  readonly runtime: string;
  readonly runtimeVersion: string;
  readonly shimVersion: string;
  readonly testedAt: string;
  readonly packages: readonly PackageCompatEntry[];
  readonly coreApis?: readonly CoreApiEntry[];
}

/** A package that improved between baseline and WarpGrid-extended runs. */
export interface ImprovedPackage {
  readonly name: string;
  readonly previousStatus: CompatStatus;
  readonly currentStatus: CompatStatus;
}

/** A core API that improved between baseline and WarpGrid-extended runs. */
export interface ImprovedCoreApi {
  readonly name: string;
  readonly previousStatus: CompatStatus;
  readonly currentStatus: CompatStatus;
}

const VALID_STATUSES = new Set<string>(["pass", "partial", "fail"]);

/**
 * Validates that an entry conforms to the PackageCompatEntry schema.
 * Throws if the entry is invalid.
 */
export function validateCompatEntry(entry: PackageCompatEntry): void {
  if (typeof entry.name !== "string" || entry.name.length === 0) {
    throw new Error("PackageCompatEntry must have a non-empty name");
  }
  if (typeof entry.version !== "string" || entry.version.length === 0) {
    throw new Error(`PackageCompatEntry "${entry.name}" must have a non-empty version`);
  }
  for (const field of ["importStatus", "functionCallStatus", "realisticUsageStatus"] as const) {
    const value = entry[field];
    if (!VALID_STATUSES.has(value)) {
      throw new Error(
        `PackageCompatEntry "${entry.name}" has invalid status "${String(value)}" for ${field}. ` +
        `Must be one of: pass, partial, fail`
      );
    }
  }
}

/**
 * Validates that an entry conforms to the CoreApiEntry schema.
 * Throws if the entry is invalid.
 */
export function validateCoreApiEntry(entry: CoreApiEntry): void {
  if (typeof entry.name !== "string" || entry.name.length === 0) {
    throw new Error("CoreApiEntry must have a non-empty name");
  }
  for (const field of ["available", "functionalLevel"] as const) {
    const value = entry[field];
    if (!VALID_STATUSES.has(value)) {
      throw new Error(
        `CoreApiEntry "${entry.name}" has invalid status "${String(value)}" for ${field}. ` +
        `Must be one of: pass, partial, fail`
      );
    }
  }
}

/** Numeric rank for status comparison — higher is better. */
function statusRank(status: CompatStatus | "untested"): number {
  switch (status) {
    case "untested": return 0;
    case "fail": return 1;
    case "partial": return 2;
    case "pass": return 3;
  }
}

/** Compute the overall status from the worst of the three levels. */
function overallStatus(entry: PackageCompatEntry): CompatStatus {
  const statuses = [entry.importStatus, entry.functionCallStatus, entry.realisticUsageStatus];
  const minRank = Math.min(...statuses.map((s) => statusRank(s)));
  if (minRank <= 1) return "fail";
  if (minRank === 2) return "partial";
  return "pass";
}

/** Compute the overall status for a core API from its two levels. */
function coreApiOverallStatus(entry: CoreApiEntry): CompatStatus {
  const minRank = Math.min(statusRank(entry.available), statusRank(entry.functionalLevel));
  if (minRank <= 1) return "fail";
  if (minRank === 2) return "partial";
  return "pass";
}

/**
 * Compares WarpGrid results against a baseline and returns packages
 * that improved due to WarpGrid shims.
 *
 * Only considers packages present in BOTH result sets.
 */
export function compareWithBaseline(
  baseline: CompatResults,
  warpgrid: CompatResults
): readonly ImprovedPackage[] {
  const baselineMap = new Map(
    baseline.packages.map((p) => [p.name, p])
  );

  const improved: ImprovedPackage[] = [];

  for (const pkg of warpgrid.packages) {
    const baselinePkg = baselineMap.get(pkg.name);
    if (!baselinePkg) continue;

    const prevOverall = overallStatus(baselinePkg);
    const currOverall = overallStatus(pkg);

    if (statusRank(currOverall) > statusRank(prevOverall)) {
      improved.push({
        name: pkg.name,
        previousStatus: prevOverall,
        currentStatus: currOverall,
      });
    }
  }

  return improved;
}

/**
 * Compares core API availability between baseline and WarpGrid runs.
 * Returns core APIs that improved due to WarpGrid shims.
 */
export function compareCoreApis(
  baseline: CompatResults,
  warpgrid: CompatResults
): readonly ImprovedCoreApi[] {
  const baselineMap = new Map(
    (baseline.coreApis ?? []).map((a) => [a.name, a])
  );

  const improved: ImprovedCoreApi[] = [];

  for (const api of warpgrid.coreApis ?? []) {
    const baselineApi = baselineMap.get(api.name);
    if (!baselineApi) continue;

    const prevOverall = coreApiOverallStatus(baselineApi);
    const currOverall = coreApiOverallStatus(api);

    if (statusRank(currOverall) > statusRank(prevOverall)) {
      improved.push({
        name: api.name,
        previousStatus: prevOverall,
        currentStatus: currOverall,
      });
    }
  }

  return improved;
}

/**
 * Generates a human-readable markdown compatibility table.
 *
 * Includes a summary section, the full package table, a core APIs
 * section (if present), and highlighted sections for improvements.
 */
export function generateCompatTable(
  results: CompatResults,
  improved: readonly ImprovedPackage[],
  improvedCoreApis?: readonly ImprovedCoreApi[]
): string {
  const improvedNames = new Set(improved.map((p) => p.name));
  const totalPackages = results.packages.length;
  const fullyCompatible = results.packages.filter(
    (p) => overallStatus(p) === "pass"
  ).length;

  const coreApis = results.coreApis ?? [];
  const totalCoreApis = coreApis.length;
  const fullyAvailableCoreApis = coreApis.filter(
    (a) => coreApiOverallStatus(a) === "pass"
  ).length;

  const lines: string[] = [];

  lines.push("# ComponentizeJS npm Package Compatibility");
  lines.push("");
  lines.push(`**Runtime**: ${results.runtime} ${results.runtimeVersion}`);
  lines.push(`**Shims**: ${results.shimVersion}`);
  lines.push(`**Tested**: ${results.testedAt}`);
  lines.push("");
  lines.push("## Summary");
  lines.push("");
  lines.push(`- **Fully compatible**: ${fullyCompatible}/${totalPackages}`);
  lines.push(`- **Improved by shims**: ${improved.length}`);
  if (totalCoreApis > 0) {
    lines.push(`- **Core APIs available**: ${fullyAvailableCoreApis}/${totalCoreApis}`);
    if (improvedCoreApis && improvedCoreApis.length > 0) {
      lines.push(`- **Core APIs improved by shims**: ${improvedCoreApis.length}`);
    }
  }
  lines.push("");
  lines.push("## Compatibility Table");
  lines.push("");
  lines.push("| Package | Version | Import | Function | Realistic | Status | Notes |");
  lines.push("|---------|---------|--------|----------|-----------|--------|-------|");

  for (const pkg of results.packages) {
    const status = overallStatus(pkg);
    const statusLabel = improvedNames.has(pkg.name) ? `${status} IMPROVED` : status;
    const truncatedNotes = pkg.notes.length > 60
      ? pkg.notes.slice(0, 57) + "..."
      : pkg.notes;
    lines.push(
      `| ${pkg.name} | ${pkg.version} | ${pkg.importStatus} | ${pkg.functionCallStatus} | ${pkg.realisticUsageStatus} | ${statusLabel} | ${truncatedNotes} |`
    );
  }

  if (totalCoreApis > 0) {
    const improvedCoreNames = new Set((improvedCoreApis ?? []).map((a) => a.name));

    lines.push("");
    lines.push("## Core Node.js APIs");
    lines.push("");
    lines.push("| API | Available | Functional Level | Status | Notes |");
    lines.push("|-----|-----------|-----------------|--------|-------|");

    for (const api of coreApis) {
      const status = coreApiOverallStatus(api);
      const statusLabel = improvedCoreNames.has(api.name) ? `${status} IMPROVED` : status;
      const truncatedNotes = api.notes.length > 60
        ? api.notes.slice(0, 57) + "..."
        : api.notes;
      lines.push(
        `| ${api.name} | ${api.available} | ${api.functionalLevel} | ${statusLabel} | ${truncatedNotes} |`
      );
    }
  }

  if (improved.length > 0) {
    lines.push("");
    lines.push("## Packages Improved by WarpGrid Shims");
    lines.push("");
    for (const imp of improved) {
      const pkg = results.packages.find((p) => p.name === imp.name);
      lines.push(`### ${imp.name}`);
      lines.push("");
      lines.push(`- **Before**: ${imp.previousStatus}`);
      lines.push(`- **After**: ${imp.currentStatus}`);
      if (pkg) {
        lines.push(`- **Details**: ${pkg.notes}`);
      }
      lines.push("");
    }
  }

  if (improvedCoreApis && improvedCoreApis.length > 0) {
    lines.push("");
    lines.push("## Core APIs Improved by WarpGrid Shims");
    lines.push("");
    for (const imp of improvedCoreApis) {
      const api = coreApis.find((a) => a.name === imp.name);
      lines.push(`### ${imp.name}`);
      lines.push("");
      lines.push(`- **Before**: ${imp.previousStatus}`);
      lines.push(`- **After**: ${imp.currentStatus}`);
      if (api) {
        lines.push(`- **Details**: ${api.notes}`);
      }
      lines.push("");
    }
  }

  return lines.join("\n");
}

/**
 * Parses a JSON string into validated CompatResults.
 * Throws on invalid JSON or missing required fields.
 */
export function loadCompatResults(json: string): CompatResults {
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    throw new Error("Invalid JSON: could not parse compatibility results");
  }

  if (
    typeof parsed !== "object" ||
    parsed === null ||
    !("packages" in parsed) ||
    !Array.isArray((parsed as Record<string, unknown>).packages)
  ) {
    throw new Error(
      "Invalid compatibility results: missing packages array"
    );
  }

  const obj = parsed as Record<string, unknown>;

  const packages = (obj.packages as PackageCompatEntry[]).map((p) => ({
    name: String(p.name),
    version: String(p.version),
    importStatus: p.importStatus as CompatStatus,
    functionCallStatus: p.functionCallStatus as CompatStatus,
    realisticUsageStatus: p.realisticUsageStatus as CompatStatus,
    blockingIssue: p.blockingIssue ?? null,
    workaround: p.workaround ?? null,
    notes: String(p.notes ?? ""),
  }));

  for (const pkg of packages) {
    validateCompatEntry(pkg);
  }

  const coreApis: CoreApiEntry[] = [];
  if (Array.isArray(obj.coreApis)) {
    for (const raw of obj.coreApis as CoreApiEntry[]) {
      const entry: CoreApiEntry = {
        name: String(raw.name),
        available: raw.available as CompatStatus,
        functionalLevel: raw.functionalLevel as CompatStatus,
        blockingIssue: raw.blockingIssue ?? null,
        workaround: raw.workaround ?? null,
        notes: String(raw.notes ?? ""),
      };
      validateCoreApiEntry(entry);
      coreApis.push(entry);
    }
  }

  const results: CompatResults = {
    runtime: String(obj.runtime ?? "unknown"),
    runtimeVersion: String(obj.runtimeVersion ?? "unknown"),
    shimVersion: String(obj.shimVersion ?? "unknown"),
    testedAt: String(obj.testedAt ?? new Date().toISOString()),
    packages,
    coreApis,
  };

  return results;
}
