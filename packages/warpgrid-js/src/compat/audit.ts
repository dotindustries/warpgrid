/**
 * Compatibility audit infrastructure for npm package testing.
 *
 * Provides types, validation, comparison, and table generation
 * for documenting npm package compatibility with the ComponentizeJS
 * runtime — both baseline (stock) and WarpGrid-extended.
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

/** Full compatibility results for a runtime configuration. */
export interface CompatResults {
  readonly runtime: string;
  readonly runtimeVersion: string;
  readonly shimVersion: string;
  readonly testedAt: string;
  readonly packages: readonly PackageCompatEntry[];
}

/** A package that improved between baseline and WarpGrid-extended runs. */
export interface ImprovedPackage {
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
 * Generates a human-readable markdown compatibility table.
 *
 * Includes a summary section, the full package table, and a
 * highlighted section for improved packages.
 */
export function generateCompatTable(
  results: CompatResults,
  improved: readonly ImprovedPackage[]
): string {
  const improvedNames = new Set(improved.map((p) => p.name));
  const totalPackages = results.packages.length;
  const fullyCompatible = results.packages.filter(
    (p) => overallStatus(p) === "pass"
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
  const results: CompatResults = {
    runtime: String(obj.runtime ?? "unknown"),
    runtimeVersion: String(obj.runtimeVersion ?? "unknown"),
    shimVersion: String(obj.shimVersion ?? "unknown"),
    testedAt: String(obj.testedAt ?? new Date().toISOString()),
    packages: (obj.packages as PackageCompatEntry[]).map((p) => ({
      name: String(p.name),
      version: String(p.version),
      importStatus: p.importStatus as CompatStatus,
      functionCallStatus: p.functionCallStatus as CompatStatus,
      realisticUsageStatus: p.realisticUsageStatus as CompatStatus,
      blockingIssue: p.blockingIssue ?? null,
      workaround: p.workaround ?? null,
      notes: String(p.notes ?? ""),
    })),
  };

  for (const pkg of results.packages) {
    validateCompatEntry(pkg);
  }

  return results;
}
