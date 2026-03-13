#!/usr/bin/env python3
"""
Post-migration fixup script for GitHub issues.

This script requires elevated permissions (personal access token with repo scope).
Run with: GH_TOKEN=ghp_xxx python3 scripts/fixup-github-issues.py

It performs the following operations that the bot token cannot:
1. Apply labels to all 93 issues
2. Apply milestones to all 92 task issues
3. Update issue bodies with fully resolved "Blocks" links
4. Close the 50 completed issues with close_reason comments
5. Add status:in-progress label to 2 in-progress issues
6. Close/delete test issue #95

Usage:
    python3 scripts/fixup-github-issues.py                    # Full fixup
    python3 scripts/fixup-github-issues.py --phase labels     # Only apply labels
    python3 scripts/fixup-github-issues.py --phase milestones # Only apply milestones
    python3 scripts/fixup-github-issues.py --phase bodies     # Only update bodies
    python3 scripts/fixup-github-issues.py --phase close      # Only close issues
    python3 scripts/fixup-github-issues.py --phase cleanup    # Delete test issue
    python3 scripts/fixup-github-issues.py --dry-run          # Preview only
"""

import json
import subprocess
import sys
import time
import os
import argparse
from pathlib import Path

REPO = "dotindustries/warpgrid"
ISSUES_FILE = ".beads/issues.jsonl"
MAPPING_FILE = "beads-to-github-mapping.json"

# Milestone prefix to domain label
MILESTONE_DOMAIN_MAP = {
    "M1": "domain:host-functions",
    "M2": "domain:libc-patches",
    "M3": "domain:tinygo",
    "M4": "domain:componentize-js",
    "M5": "domain:wasi-async",
    "M6": "domain:bun",
    "Integration": "domain:integration",
}


def run_gh(args, check=True):
    cmd = ["gh"] + args
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, check=False)
    except FileNotFoundError:
        if check:
            print("  ERROR: gh CLI not found")
        return None
    if check and result.returncode != 0:
        print(f"  ERROR: {result.stderr.strip()[:200]}")
        return None
    return result.stdout.strip()


def load_issues(path):
    issues = {}
    with open(path) as f:
        for line in f:
            issue = json.loads(line.strip())
            issues[issue["id"]] = issue
    return issues


def load_mapping(path):
    with open(path) as f:
        return json.load(f)


def extract_milestone(description):
    if "Milestone:" in description:
        return description.split("Milestone:")[1].split("|")[0].strip()
    return None


def get_domain_label(milestone):
    if not milestone:
        return None
    if milestone == "Integration":
        return "domain:integration"
    for prefix, label in MILESTONE_DOMAIN_MAP.items():
        if milestone.startswith(prefix):
            return label
    return None


def get_existing_milestones():
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


def apply_labels(issues, mapping, dry_run=False):
    """Apply labels to all issues."""
    print("\n=== Applying Labels ===")
    count = 0
    for issue_id, issue in issues.items():
        if issue_id not in mapping:
            continue
        gh_number = mapping[issue_id]

        labels = [f"priority:{issue['priority']}"]
        labels.append(issue.get("issue_type", "task"))
        labels.append("beads-migrated")

        if issue.get("status") == "in_progress":
            labels.append("status:in-progress")

        milestone_name = extract_milestone(issue.get("description", ""))
        domain_label = get_domain_label(milestone_name)
        if domain_label:
            labels.append(domain_label)

        if dry_run:
            print(f"  [DRY RUN] #{gh_number} ({issue_id}): {', '.join(labels)}")
            continue

        # Apply labels via REST API
        labels_json = json.dumps({"labels": labels})
        proc = subprocess.run(
            ["gh", "api", f"repos/{REPO}/issues/{gh_number}/labels",
             "--method", "POST", "--input", "-"],
            input=labels_json, capture_output=True, text=True, check=False,
        )
        if proc.returncode == 0:
            count += 1
            if count % 10 == 0:
                print(f"  Applied labels to {count} issues...")
        else:
            print(f"  ERROR #{gh_number}: {proc.stderr.strip()[:100]}")
        time.sleep(0.3)

    print(f"  Done. Applied labels to {count} issues.")


def apply_milestones(issues, mapping, dry_run=False):
    """Apply milestones to all issues."""
    print("\n=== Applying Milestones ===")
    milestone_map = get_existing_milestones()
    count = 0

    for issue_id, issue in issues.items():
        if issue_id not in mapping:
            continue
        gh_number = mapping[issue_id]

        milestone_name = extract_milestone(issue.get("description", ""))
        if not milestone_name or milestone_name not in milestone_map:
            continue

        ms_number = milestone_map[milestone_name]

        if dry_run:
            print(f"  [DRY RUN] #{gh_number} ({issue_id}): milestone '{milestone_name}' (#{ms_number})")
            continue

        proc = subprocess.run(
            ["gh", "api", f"repos/{REPO}/issues/{gh_number}",
             "--method", "PATCH", "-F", f"milestone={ms_number}"],
            capture_output=True, text=True, check=False,
        )
        if proc.returncode == 0:
            count += 1
            if count % 10 == 0:
                print(f"  Applied milestones to {count} issues...")
        else:
            print(f"  ERROR #{gh_number}: {proc.stderr.strip()[:100]}")
        time.sleep(0.3)

    print(f"  Done. Applied milestones to {count} issues.")


def update_bodies(issues, mapping, dry_run=False):
    """Update issue bodies with fully resolved dependency links."""
    print("\n=== Updating Issue Bodies ===")

    # Import format function from main script
    sys.path.insert(0, str(Path(__file__).parent))
    from importlib import import_module
    migrate = import_module("migrate-beads-to-github")

    count = 0
    for issue_id, issue in issues.items():
        if issue_id not in mapping:
            continue
        gh_number = mapping[issue_id]

        body = migrate.format_issue_body(issue, mapping, issues)

        if dry_run:
            print(f"  [DRY RUN] #{gh_number} ({issue_id}): body update")
            continue

        proc = subprocess.run(
            ["gh", "api", f"repos/{REPO}/issues/{gh_number}",
             "--method", "PATCH", "-f", f"body={body}"],
            capture_output=True, text=True, check=False,
        )
        if proc.returncode == 0:
            count += 1
            if count % 10 == 0:
                print(f"  Updated {count} issue bodies...")
        else:
            print(f"  ERROR #{gh_number}: {proc.stderr.strip()[:100]}")
        time.sleep(0.5)

    print(f"  Done. Updated {count} issue bodies.")


def close_issues(issues, mapping, dry_run=False):
    """Close completed issues with close_reason comments."""
    print("\n=== Closing Completed Issues ===")
    count = 0

    for issue_id, issue in issues.items():
        if issue["status"] != "closed":
            continue
        if issue_id not in mapping:
            continue

        gh_number = mapping[issue_id]
        close_reason = issue.get("close_reason", "Completed")

        if dry_run:
            print(f"  [DRY RUN] Close #{gh_number} ({issue_id}): {close_reason[:60]}")
            continue

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
        count += 1
        if count % 10 == 0:
            print(f"  Closed {count} issues...")
        time.sleep(0.5)

    print(f"  Done. Closed {count} issues.")


def cleanup_test_issue(dry_run=False):
    """Close/remove test issue #95."""
    print("\n=== Cleanup ===")
    if dry_run:
        print("  [DRY RUN] Would close test issue #95")
        return

    run_gh(
        ["issue", "close", "95", "--repo", REPO, "--reason", "not_planned"],
        check=False,
    )
    print("  Closed test issue #95")


def main():
    parser = argparse.ArgumentParser(description="Post-migration fixup for GitHub issues")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--phase", choices=["labels", "milestones", "bodies", "close", "cleanup"])
    args = parser.parse_args()

    repo_root = Path(__file__).parent.parent
    os.chdir(repo_root)

    issues = load_issues(ISSUES_FILE)
    mapping = load_mapping(MAPPING_FILE)
    print(f"Loaded {len(issues)} issues, {len(mapping)} mappings")

    if args.phase:
        if args.phase == "labels":
            apply_labels(issues, mapping, args.dry_run)
        elif args.phase == "milestones":
            apply_milestones(issues, mapping, args.dry_run)
        elif args.phase == "bodies":
            update_bodies(issues, mapping, args.dry_run)
        elif args.phase == "close":
            close_issues(issues, mapping, args.dry_run)
        elif args.phase == "cleanup":
            cleanup_test_issue(args.dry_run)
        return

    # Full fixup
    apply_labels(issues, mapping, args.dry_run)
    apply_milestones(issues, mapping, args.dry_run)
    update_bodies(issues, mapping, args.dry_run)
    close_issues(issues, mapping, args.dry_run)
    cleanup_test_issue(args.dry_run)


if __name__ == "__main__":
    main()
