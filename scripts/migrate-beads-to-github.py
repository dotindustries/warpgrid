#!/usr/bin/env python3
"""
Migrate beads issues to GitHub Issues with full dependency graph.

Usage:
    python3 scripts/migrate-beads-to-github.py                    # Full migration
    python3 scripts/migrate-beads-to-github.py --dry-run           # Preview only
    python3 scripts/migrate-beads-to-github.py --verify            # Verify existing migration
    python3 scripts/migrate-beads-to-github.py --phase labels      # Only create labels
    python3 scripts/migrate-beads-to-github.py --phase milestones  # Only create milestones
    python3 scripts/migrate-beads-to-github.py --phase issues      # Only create issues
    python3 scripts/migrate-beads-to-github.py --phase close       # Only close issues
    python3 scripts/migrate-beads-to-github.py --phase verify      # Only verify
"""

import json
import subprocess
import sys
import time
import os
import argparse
from collections import defaultdict
from pathlib import Path

REPO = "dotindustries/warpgrid"
ISSUES_FILE = ".beads/issues.jsonl"
MAPPING_FILE = "beads-to-github-mapping.json"
RATE_LIMIT_SECONDS = 1.0

# Domain mapping from milestone prefix to label
MILESTONE_DOMAIN_MAP = {
    "M1": "domain:host-functions",
    "M2": "domain:libc-patches",
    "M3": "domain:tinygo",
    "M4": "domain:componentize-js",
    "M5": "domain:wasi-async",
    "M6": "domain:bun",
    "Integration": "domain:integration",
}

# Milestone descriptions
MILESTONE_DESCRIPTIONS = {
    "M1.1": "Crate scaffold & engine configuration",
    "M1.2": "Filesystem host functions",
    "M1.3": "DNS resolution host functions",
    "M1.4": "Signal handling host functions",
    "M1.5": "Database proxy host functions",
    "M1.6": "Thread-pool shim host functions",
    "M1.7": "Engine configuration & multi-module linking",
    "M1.8": "Host function integration tests",
    "M2.1": "Fork wasi-libc & patch infrastructure",
    "M2.2": "Socket/networking patches",
    "M2.3": "Filesystem extended attribute patches",
    "M2.4": "Database client (libpq) cross-compilation",
    "M2.5": "libc integration tests",
    "M2.5 (early)": "libc early integration tests",
    "M3.1": "Fork TinyGo & overlay infrastructure",
    "M3.2": "net/http overlay for TinyGo",
    "M3.3": "database/sql overlay for TinyGo",
    "M3.4": "os/signal overlay for TinyGo",
    "M3.5": "TinyGo integration tests",
    "M4.1": "Fork ComponentizeJS & shim infrastructure",
    "M4.2": "Node.js net module shim",
    "M4.3": "Node.js child_process shim",
    "M4.4": "ComponentizeJS integration tests",
    "M5.1": "WASI 0.3 async WIT definitions",
    "M5.2": "Async poll/stream host functions",
    "M5.3": "Async handler compilation",
    "M5.4": "WASI 0.3 integration tests",
    "M6.1": "Bun WASI runtime scaffold",
    "M6.2": "Bun WASI I/O & networking",
    "M6.3": "Bun database proxy client",
    "M6.4": "Bun component model support",
    "M6.5": "Bun WASI performance & monitoring",
    "M6.6": "Bun integration tests",
    "Integration": "Cross-domain integration tests",
}

# Label colors
LABEL_COLORS = {
    "priority:1": "d73a4a",   # red - critical
    "priority:2": "fbca04",   # yellow - important
    "priority:3": "0e8a16",   # green - nice-to-have
    "domain:host-functions": "1d76db",
    "domain:libc-patches": "5319e7",
    "domain:tinygo": "0075ca",
    "domain:componentize-js": "e4e669",
    "domain:wasi-async": "d876e3",
    "domain:bun": "f9d0c4",
    "domain:integration": "c5def5",
    "epic": "3e4b9e",
    "task": "bfd4f2",
    "status:in-progress": "ff9f1c",
    "beads-migrated": "ededed",
}


def run_gh(args, capture=True, check=True):
    """Run a gh CLI command and return output."""
    cmd = ["gh"] + args
    result = subprocess.run(
        cmd,
        capture_output=capture,
        text=True,
        check=False,
    )
    if check and result.returncode != 0:
        print(f"  ERROR: gh {' '.join(args[:3])}...")
        print(f"  stderr: {result.stderr.strip()}")
        return None
    return result.stdout.strip() if capture else ""


def load_issues(path):
    """Load all issues from JSONL file."""
    issues = {}
    with open(path) as f:
        for line in f:
            issue = json.loads(line.strip())
            issues[issue["id"]] = issue
    return issues


def load_mapping(path):
    """Load existing beads-to-github mapping."""
    if os.path.exists(path):
        with open(path) as f:
            return json.load(f)
    return {}


def save_mapping(mapping, path):
    """Save beads-to-github mapping incrementally."""
    with open(path, "w") as f:
        json.dump(mapping, f, indent=2, sort_keys=True)


def extract_milestone(description):
    """Extract milestone name from issue description."""
    if "Milestone:" in description:
        return description.split("Milestone:")[1].split("|")[0].strip()
    return None


def get_domain_label(milestone):
    """Map a milestone to its domain label."""
    if not milestone:
        return None
    if milestone == "Integration":
        return "domain:integration"
    for prefix, label in MILESTONE_DOMAIN_MAP.items():
        if milestone.startswith(prefix):
            return label
    return None


def topological_sort(issues):
    """
    Topological sort of issues based on 'blocks' dependencies.
    Returns list of issue IDs in creation order (dependencies first).
    """
    # Build adjacency list: edge from dependency to dependent
    # If A blocks B (B depends_on A), then A must come before B
    in_degree = defaultdict(int)
    graph = defaultdict(list)
    all_ids = set(issues.keys())

    for issue_id, issue in issues.items():
        if issue_id not in in_degree:
            in_degree[issue_id] = 0
        for dep in issue.get("dependencies", []):
            if dep["type"] == "blocks":
                dep_id = dep["depends_on_id"]
                if dep_id in all_ids:
                    graph[dep_id].append(issue_id)
                    in_degree[issue_id] += 1

    # Kahn's algorithm
    queue = []
    for issue_id in all_ids:
        if in_degree[issue_id] == 0:
            queue.append(issue_id)

    # Sort the queue for deterministic ordering (epic first, then numeric)
    def sort_key(iid):
        if iid == "warpgrid-agm":
            return (0, 0)
        num = int(iid.split(".")[-1])
        return (1, num)

    queue.sort(key=sort_key)

    result = []
    while queue:
        queue.sort(key=sort_key)
        node = queue.pop(0)
        result.append(node)
        for neighbor in graph[node]:
            in_degree[neighbor] -= 1
            if in_degree[neighbor] == 0:
                queue.append(neighbor)

    if len(result) != len(all_ids):
        missing = all_ids - set(result)
        print(f"WARNING: Cycle detected! {len(missing)} issues not in sort: {missing}")
        # Add remaining issues anyway
        for iid in sorted(missing):
            result.append(iid)

    return result


def format_issue_body(issue, mapping, issues):
    """Format the GitHub issue body with all metadata."""
    beads_id = issue["id"]
    description = issue.get("description", "")
    notes = issue.get("notes", "")
    priority = issue.get("priority", 0)
    issue_type = issue.get("issue_type", "task")

    # Build dependency sections
    blocks_deps = []
    parent_deps = []
    for dep in issue.get("dependencies", []):
        dep_id = dep["depends_on_id"]
        if dep["type"] == "blocks":
            if dep_id in mapping:
                blocks_deps.append(f"#{mapping[dep_id]}")
            else:
                blocks_deps.append(f"`{dep_id}` (not yet migrated)")
        elif dep["type"] == "parent-child":
            if dep_id in mapping:
                parent_deps.append(f"#{mapping[dep_id]}")
            else:
                parent_deps.append(f"`{dep_id}` (not yet migrated)")

    # Find children (issues that have this issue as parent)
    children = []
    for other_id, other_issue in issues.items():
        if other_id == beads_id:
            continue
        for dep in other_issue.get("dependencies", []):
            if dep["type"] == "parent-child" and dep["depends_on_id"] == beads_id:
                if other_id in mapping:
                    children.append(f"#{mapping[other_id]}")
                else:
                    children.append(f"`{other_id}`")

    # Find issues blocked by this one (reverse of blocks)
    blocks_others = []
    for other_id, other_issue in issues.items():
        if other_id == beads_id:
            continue
        for dep in other_issue.get("dependencies", []):
            if dep["type"] == "blocks" and dep["depends_on_id"] == beads_id:
                if other_id in mapping:
                    blocks_others.append(f"#{mapping[other_id]}")
                else:
                    blocks_others.append(f"`{other_id}`")

    # Build body
    body_parts = []

    # Beads ID marker (HTML comment for idempotent matching)
    body_parts.append(f"<!-- beads-id: {beads_id} -->")
    body_parts.append("")

    # Priority badge
    priority_labels = {1: "P1 Critical", 2: "P2 Important", 3: "P3 Nice-to-have"}
    priority_label = priority_labels.get(priority, f"P{priority}")
    body_parts.append(f"> **Priority:** {priority_label} | **Type:** {issue_type}")
    body_parts.append(f"> **Beads ID:** `{beads_id}`")
    body_parts.append("")

    # Description
    body_parts.append(description)
    body_parts.append("")

    # Notes section
    if notes:
        body_parts.append("---")
        body_parts.append("")
        body_parts.append("## Notes")
        body_parts.append("")
        body_parts.append(notes)
        body_parts.append("")

    # Dependencies section
    if blocks_deps or parent_deps or children or blocks_others:
        body_parts.append("---")
        body_parts.append("")
        body_parts.append("## Dependencies")
        body_parts.append("")

        if parent_deps:
            body_parts.append(f"**Parent:** {', '.join(parent_deps)}")
            body_parts.append("")

        if children:
            # Sort children by issue number for readability
            children_sorted = sorted(children, key=lambda x: int(x.lstrip('#')) if x.startswith('#') else 99999)
            body_parts.append(f"**Children:** {', '.join(children_sorted)}")
            body_parts.append("")

        if blocks_deps:
            body_parts.append(f"**Blocked by:** {', '.join(blocks_deps)}")
            body_parts.append("")

        if blocks_others:
            blocks_others_sorted = sorted(blocks_others, key=lambda x: int(x.lstrip('#')) if x.startswith('#') else 99999)
            body_parts.append(f"**Blocks:** {', '.join(blocks_others_sorted)}")
            body_parts.append("")

    return "\n".join(body_parts).strip()


def find_existing_issue(beads_id):
    """Find an existing GitHub issue by beads ID marker."""
    search_query = f"beads-id: {beads_id} in:body repo:{REPO}"
    result = run_gh(
        ["issue", "list", "--repo", REPO, "--search", search_query,
         "--state", "all", "--json", "number,title", "--limit", "5"],
        check=False,
    )
    if result:
        try:
            found = json.loads(result)
            if found:
                return found[0]["number"]
        except json.JSONDecodeError:
            pass
    return None


def create_labels(dry_run=False):
    """Create all required labels."""
    print("\n=== Creating Labels ===")

    # Get existing labels
    existing = run_gh(
        ["label", "list", "--repo", REPO, "--json", "name", "--limit", "100"]
    )
    existing_names = set()
    if existing:
        try:
            existing_names = {l["name"] for l in json.loads(existing)}
        except json.JSONDecodeError:
            pass

    for name, color in LABEL_COLORS.items():
        if name in existing_names:
            print(f"  Label '{name}' already exists, skipping")
            continue
        if dry_run:
            print(f"  [DRY RUN] Would create label '{name}' (#{color})")
        else:
            print(f"  Creating label '{name}'...")
            run_gh(
                ["label", "create", name, "--repo", REPO,
                 "--color", color, "--force"],
                check=False,
            )
            time.sleep(0.3)

    print(f"  Done. {len(LABEL_COLORS)} labels processed.")


def create_milestones(issues, dry_run=False):
    """Create all required milestones."""
    print("\n=== Creating Milestones ===")

    # Extract unique milestones from issues
    milestones = set()
    for issue in issues.values():
        ms = extract_milestone(issue.get("description", ""))
        if ms:
            milestones.add(ms)

    # Get existing milestones
    existing = run_gh(
        ["api", f"repos/{REPO}/milestones", "--method", "GET",
         "-q", ".[].title", "--paginate"],
        check=False,
    )
    existing_titles = set()
    if existing:
        existing_titles = set(existing.strip().split("\n")) if existing.strip() else set()

    milestone_map = {}  # title -> number

    # Get existing milestone numbers
    existing_data = run_gh(
        ["api", f"repos/{REPO}/milestones?state=all&per_page=100", "--method", "GET"],
        check=False,
    )
    if existing_data:
        try:
            for ms in json.loads(existing_data):
                milestone_map[ms["title"]] = ms["number"]
        except json.JSONDecodeError:
            pass

    for ms_title in sorted(milestones):
        desc = MILESTONE_DESCRIPTIONS.get(ms_title, "")
        if ms_title in milestone_map:
            print(f"  Milestone '{ms_title}' already exists (#{milestone_map[ms_title]})")
            continue
        if dry_run:
            print(f"  [DRY RUN] Would create milestone '{ms_title}': {desc}")
        else:
            print(f"  Creating milestone '{ms_title}'...")
            body = json.dumps({"title": ms_title, "description": desc})
            result = run_gh(
                ["api", f"repos/{REPO}/milestones", "--method", "POST",
                 "--input", "-"],
                check=False,
            )
            # Use direct API call with input
            proc = subprocess.run(
                ["gh", "api", f"repos/{REPO}/milestones", "--method", "POST",
                 "-f", f"title={ms_title}", "-f", f"description={desc}"],
                capture_output=True, text=True, check=False,
            )
            if proc.returncode == 0:
                try:
                    data = json.loads(proc.stdout)
                    milestone_map[ms_title] = data["number"]
                    print(f"    Created milestone #{data['number']}")
                except (json.JSONDecodeError, KeyError):
                    print(f"    WARNING: Could not parse milestone response")
            else:
                print(f"    ERROR: {proc.stderr.strip()}")
            time.sleep(0.3)

    print(f"  Done. {len(milestones)} milestones processed.")
    return milestone_map


def create_issues(issues, milestone_map, dry_run=False):
    """Create all issues in topological order."""
    print("\n=== Creating Issues ===")

    # Load existing mapping
    mapping = load_mapping(MAPPING_FILE)

    # Topological sort
    order = topological_sort(issues)
    print(f"  Topological order: {len(order)} issues")

    created_count = 0
    skipped_count = 0

    for i, issue_id in enumerate(order):
        issue = issues[issue_id]
        title = issue["title"]

        # Check if already mapped
        if issue_id in mapping:
            print(f"  [{i+1}/{len(order)}] {issue_id}: already mapped to #{mapping[issue_id]}, skipping")
            skipped_count += 1
            continue

        # Check if already exists on GitHub (idempotent)
        existing_num = find_existing_issue(issue_id)
        if existing_num:
            print(f"  [{i+1}/{len(order)}] {issue_id}: found existing #{existing_num}")
            mapping[issue_id] = existing_num
            save_mapping(mapping, MAPPING_FILE)
            skipped_count += 1
            continue

        # Build labels
        labels = [f"priority:{issue['priority']}"]
        labels.append(issue.get("issue_type", "task"))
        labels.append("beads-migrated")

        if issue.get("status") == "in_progress":
            labels.append("status:in-progress")

        milestone_name = extract_milestone(issue.get("description", ""))
        domain_label = get_domain_label(milestone_name)
        if domain_label:
            labels.append(domain_label)

        # Format body
        body = format_issue_body(issue, mapping, issues)

        if dry_run:
            print(f"  [{i+1}/{len(order)}] [DRY RUN] Would create: {issue_id} - {title}")
            print(f"    Labels: {', '.join(labels)}")
            if milestone_name:
                print(f"    Milestone: {milestone_name}")
            continue

        # Create the issue
        print(f"  [{i+1}/{len(order)}] Creating {issue_id}: {title[:60]}...")

        gh_args = [
            "issue", "create", "--repo", REPO,
            "--title", title,
            "--body", body,
        ]

        for label in labels:
            gh_args.extend(["--label", label])

        if milestone_name and milestone_name in milestone_map:
            gh_args.extend(["--milestone", milestone_name])

        result = run_gh(gh_args, check=False)
        if result:
            # Extract issue number from URL
            # Result is like: https://github.com/dotindustries/warpgrid/issues/42
            try:
                gh_number = int(result.strip().split("/")[-1])
                mapping[issue_id] = gh_number
                save_mapping(mapping, MAPPING_FILE)
                print(f"    Created #{gh_number}")
                created_count += 1
            except (ValueError, IndexError):
                print(f"    WARNING: Could not parse issue number from: {result}")
        else:
            print(f"    ERROR: Failed to create issue {issue_id}")

        time.sleep(RATE_LIMIT_SECONDS)

    print(f"\n  Summary: {created_count} created, {skipped_count} skipped")
    return mapping


def update_dependency_links(issues, mapping, dry_run=False):
    """Second pass: update all issue bodies with resolved dependency links."""
    print("\n=== Updating Dependency Links ===")

    updated = 0
    for issue_id, issue in issues.items():
        if issue_id not in mapping:
            print(f"  WARNING: {issue_id} not in mapping, skipping update")
            continue

        gh_number = mapping[issue_id]

        # Re-format body with fully resolved links
        body = format_issue_body(issue, mapping, issues)

        if dry_run:
            print(f"  [DRY RUN] Would update #{gh_number} ({issue_id}) with resolved links")
            continue

        print(f"  Updating #{gh_number} ({issue_id})...")

        # Use API to update issue body
        proc = subprocess.run(
            ["gh", "api", f"repos/{REPO}/issues/{gh_number}",
             "--method", "PATCH", "-f", f"body={body}"],
            capture_output=True, text=True, check=False,
        )
        if proc.returncode == 0:
            updated += 1
        else:
            print(f"    ERROR: {proc.stderr.strip()[:200]}")

        time.sleep(0.5)

    print(f"  Updated {updated} issues with resolved dependency links.")


def close_completed_issues(issues, mapping, dry_run=False):
    """Close issues that are closed in beads."""
    print("\n=== Closing Completed Issues ===")

    closed_count = 0
    for issue_id, issue in issues.items():
        if issue["status"] != "closed":
            continue
        if issue_id not in mapping:
            print(f"  WARNING: {issue_id} not in mapping, skipping close")
            continue

        gh_number = mapping[issue_id]
        close_reason = issue.get("close_reason", "Completed")

        if dry_run:
            print(f"  [DRY RUN] Would close #{gh_number} ({issue_id}): {close_reason[:60]}")
            continue

        # Check if already closed
        result = run_gh(
            ["issue", "view", str(gh_number), "--repo", REPO, "--json", "state"],
            check=False,
        )
        if result:
            try:
                state = json.loads(result).get("state", "OPEN")
                if state == "CLOSED":
                    print(f"  #{gh_number} ({issue_id}): already closed")
                    closed_count += 1
                    continue
            except json.JSONDecodeError:
                pass

        # Add close reason as comment
        if close_reason:
            comment = f"**Closed in beads tracker:**\n\n{close_reason}"
            run_gh(
                ["issue", "comment", str(gh_number), "--repo", REPO,
                 "--body", comment],
                check=False,
            )
            time.sleep(0.3)

        # Close the issue
        run_gh(
            ["issue", "close", str(gh_number), "--repo", REPO,
             "--reason", "completed"],
            check=False,
        )
        print(f"  Closed #{gh_number} ({issue_id})")
        closed_count += 1
        time.sleep(0.5)

    print(f"  Closed {closed_count} issues.")


def verify_migration(issues, mapping):
    """Verify the migration results."""
    print("\n=== Verification ===")

    errors = []

    # Check mapping count
    print(f"  Mapping entries: {len(mapping)} (expected {len(issues)})")
    if len(mapping) != len(issues):
        missing = set(issues.keys()) - set(mapping.keys())
        errors.append(f"Missing from mapping: {missing}")

    # Count open/closed on GitHub
    open_result = run_gh(
        ["issue", "list", "--repo", REPO, "--state", "open",
         "--label", "beads-migrated", "--json", "number", "--limit", "200"],
        check=False,
    )
    closed_result = run_gh(
        ["issue", "list", "--repo", REPO, "--state", "closed",
         "--label", "beads-migrated", "--json", "number", "--limit", "200"],
        check=False,
    )

    open_count = 0
    closed_count = 0
    if open_result:
        try:
            open_count = len(json.loads(open_result))
        except json.JSONDecodeError:
            pass
    if closed_result:
        try:
            closed_count = len(json.loads(closed_result))
        except json.JSONDecodeError:
            pass

    expected_open = sum(1 for i in issues.values() if i["status"] in ("open", "in_progress"))
    expected_closed = sum(1 for i in issues.values() if i["status"] == "closed")

    print(f"  GitHub open issues: {open_count} (expected {expected_open})")
    print(f"  GitHub closed issues: {closed_count} (expected {expected_closed})")
    print(f"  GitHub total: {open_count + closed_count} (expected {len(issues)})")

    if open_count != expected_open:
        errors.append(f"Open count mismatch: {open_count} vs {expected_open}")
    if closed_count != expected_closed:
        errors.append(f"Closed count mismatch: {closed_count} vs {expected_closed}")

    # Spot-check 5 issues for dependency links
    spot_check_ids = ["warpgrid-agm.10", "warpgrid-agm.21", "warpgrid-agm.34", "warpgrid-agm.84", "warpgrid-agm.89"]
    print("\n  Spot-checking dependency links:")
    for beads_id in spot_check_ids:
        if beads_id not in mapping:
            print(f"    {beads_id}: NOT IN MAPPING")
            errors.append(f"Spot-check {beads_id} not in mapping")
            continue

        gh_number = mapping[beads_id]
        result = run_gh(
            ["issue", "view", str(gh_number), "--repo", REPO, "--json", "body"],
            check=False,
        )
        if result:
            try:
                body = json.loads(result).get("body", "")
                has_deps = "## Dependencies" in body or "Blocked by" in body or "Parent" in body
                has_beads_marker = f"beads-id: {beads_id}" in body
                issue = issues[beads_id]
                expected_deps = len([d for d in issue.get("dependencies", []) if d["type"] == "blocks"])
                actual_refs = body.count("#") - body.count("##")  # rough count

                status = "OK" if (has_beads_marker and (has_deps or expected_deps == 0)) else "ISSUE"
                print(f"    #{gh_number} ({beads_id}): {status} - beads_marker={has_beads_marker}, has_deps={has_deps}")
                if status == "ISSUE":
                    errors.append(f"Spot-check {beads_id} (#{gh_number}) failed")
            except json.JSONDecodeError:
                print(f"    #{gh_number} ({beads_id}): PARSE ERROR")

    if errors:
        print(f"\n  ERRORS FOUND ({len(errors)}):")
        for err in errors:
            print(f"    - {err}")
        return False
    else:
        print("\n  All checks passed!")
        return True


def main():
    parser = argparse.ArgumentParser(description="Migrate beads issues to GitHub")
    parser.add_argument("--dry-run", action="store_true", help="Preview without making changes")
    parser.add_argument("--verify", action="store_true", help="Only verify existing migration")
    parser.add_argument("--phase", choices=["labels", "milestones", "issues", "update-links", "close", "verify"],
                        help="Run only a specific phase")
    args = parser.parse_args()

    # Change to repo root
    repo_root = Path(__file__).parent.parent
    os.chdir(repo_root)

    print(f"Repository: {REPO}")
    print(f"Issues file: {ISSUES_FILE}")
    print(f"Mapping file: {MAPPING_FILE}")

    # Load issues
    issues = load_issues(ISSUES_FILE)
    print(f"Loaded {len(issues)} issues from beads")

    if args.verify:
        mapping = load_mapping(MAPPING_FILE)
        verify_migration(issues, mapping)
        return

    if args.phase:
        if args.phase == "labels":
            create_labels(args.dry_run)
        elif args.phase == "milestones":
            milestone_map = create_milestones(issues, args.dry_run)
        elif args.phase == "issues":
            # Need milestones for issue creation
            milestone_map = get_existing_milestones()
            create_issues(issues, milestone_map, args.dry_run)
        elif args.phase == "update-links":
            mapping = load_mapping(MAPPING_FILE)
            update_dependency_links(issues, mapping, args.dry_run)
        elif args.phase == "close":
            mapping = load_mapping(MAPPING_FILE)
            close_completed_issues(issues, mapping, args.dry_run)
        elif args.phase == "verify":
            mapping = load_mapping(MAPPING_FILE)
            verify_migration(issues, mapping)
        return

    # Full migration
    print("\n" + "=" * 60)
    print("FULL MIGRATION")
    print("=" * 60)

    # Phase 1: Labels
    create_labels(args.dry_run)

    # Phase 2: Milestones
    milestone_map = create_milestones(issues, args.dry_run)

    # Phase 3: Create issues
    mapping = create_issues(issues, milestone_map, args.dry_run)

    if not args.dry_run:
        # Phase 4: Update dependency links (second pass)
        update_dependency_links(issues, mapping, args.dry_run)

        # Phase 5: Close completed issues
        close_completed_issues(issues, mapping, args.dry_run)

        # Phase 6: Verify
        verify_migration(issues, mapping)

    print("\n" + "=" * 60)
    print("MIGRATION COMPLETE")
    print("=" * 60)


def get_existing_milestones():
    """Get existing milestone title->number mapping from GitHub."""
    milestone_map = {}
    result = run_gh(
        ["api", f"repos/{REPO}/milestones?state=all&per_page=100", "--method", "GET"],
        check=False,
    )
    if result:
        try:
            for ms in json.loads(result):
                milestone_map[ms["title"]] = ms["number"]
        except json.JSONDecodeError:
            pass
    return milestone_map


if __name__ == "__main__":
    main()
