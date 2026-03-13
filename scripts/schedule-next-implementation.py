#!/usr/bin/env python3
"""
Schedule the next implementation via the tervezo public-api.

Reads the beads dependency graph and GitHub issue state to determine the
next unblocked issue, then calls the tervezo API to start a new
implementation for that issue.

Intended to be called by an agent after completing its current task so
that the next issue is automatically picked up.

Usage:
    python3 scripts/schedule-next-implementation.py              # Schedule next ready issue
    python3 scripts/schedule-next-implementation.py --dry-run    # Preview without scheduling
    python3 scripts/schedule-next-implementation.py --list-ready # List all ready issues

Environment variables:
    TERVEZO_API_KEY        API key for authentication (tzv_... format)
    TERVEZO_WORKSPACE_SLUG Workspace slug (used to resolve workspace ID)
    TERVEZO_API_URL        Base URL override (default: https://app.tervezo.ai)
    GITHUB_TOKEN           (optional) GitHub token for issue state queries
"""

import json
import os
import subprocess
import sys
import argparse
import urllib.request
import urllib.error
from collections import defaultdict
from pathlib import Path

REPO = "dotindustries/warpgrid"
ISSUES_FILE = ".beads/issues.jsonl"
MAPPING_FILE = "beads-to-github-mapping.json"


# ---------------------------------------------------------------------------
# Data helpers (shared logic with migrate-beads-to-github.py)
# ---------------------------------------------------------------------------

def load_issues(path):
    """Load all issues from JSONL file."""
    issues = {}
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            issue = json.loads(line)
            issues[issue["id"]] = issue
    return issues


def load_mapping(path):
    """Load beads-to-github mapping."""
    if os.path.exists(path):
        with open(path) as f:
            return json.load(f)
    return {}


def get_blocking_deps(issue, all_ids):
    """Return the set of beads IDs that block this issue."""
    blockers = set()
    for dep in issue.get("dependencies", []):
        if dep["type"] == "blocks" and dep["depends_on_id"] in all_ids:
            blockers.add(dep["depends_on_id"])
    return blockers


# ---------------------------------------------------------------------------
# GitHub helpers
# ---------------------------------------------------------------------------

def run_gh(args, check=True):
    """Run a gh CLI command and return output."""
    cmd = ["gh"] + args
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, check=False)
    except FileNotFoundError:
        return None
    if check and result.returncode != 0:
        return None
    return result.stdout.strip()


def fetch_closed_issue_numbers(mapped_numbers):
    """Check which of the mapped issue numbers are closed on GitHub.

    Queries closed issues from the repo and intersects with the mapped
    set to determine which beads issues have been completed.

    Returns (closed_numbers, success) where success indicates whether
    the GitHub API call succeeded. When it fails (e.g. bad credentials),
    the caller should fall back to local beads state.
    """
    result = run_gh([
        "issue", "list", "--repo", REPO,
        "--state", "closed",
        "--json", "number",
        "--limit", "500",
    ])
    if not result:
        return set(), False
    try:
        all_closed = {i["number"] for i in json.loads(result)}
        return all_closed & mapped_numbers, True
    except (json.JSONDecodeError, KeyError):
        return set(), False


def fetch_issue_details(gh_number):
    """Fetch title and labels for a GitHub issue."""
    result = run_gh([
        "issue", "view", str(gh_number), "--repo", REPO,
        "--json", "number,title,state,labels,milestone",
    ])
    if not result:
        return None
    try:
        return json.loads(result)
    except json.JSONDecodeError:
        return None


# ---------------------------------------------------------------------------
# Dependency graph analysis
# ---------------------------------------------------------------------------

def find_ready_issues(issues, mapping, closed_beads_ids):
    """
    Find issues whose blocking dependencies are all satisfied (closed).

    Returns a list of (beads_id, gh_number, title) tuples, ordered by
    topological priority (earliest in dependency chain first).
    """
    all_ids = set(issues.keys())
    ready = []

    # Topological ordering for deterministic priority
    order = topological_sort(issues)
    ordered_set = {iid: idx for idx, iid in enumerate(order)}

    for beads_id in order:
        issue = issues[beads_id]

        # Skip the epic itself
        if issue.get("issue_type") == "epic":
            continue

        # Skip already-closed issues
        if beads_id in closed_beads_ids:
            continue

        # Skip issues not mapped to GitHub
        if beads_id not in mapping:
            continue

        # Check if all blocking dependencies are closed
        blockers = get_blocking_deps(issue, all_ids)
        unresolved = blockers - closed_beads_ids
        if not unresolved:
            ready.append((beads_id, mapping[beads_id], issue["title"]))

    return ready


def topological_sort(issues):
    """
    Topological sort of issues based on 'blocks' dependencies.
    Returns list of issue IDs in creation order (dependencies first).
    """
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

    def sort_key(iid):
        if iid == "warpgrid-agm":
            return (0, 0)
        try:
            num = int(iid.split(".")[-1])
        except ValueError:
            num = 99999
        return (1, num)

    queue = sorted(
        [iid for iid in all_ids if in_degree[iid] == 0],
        key=sort_key,
    )

    result = []
    while queue:
        queue.sort(key=sort_key)
        node = queue.pop(0)
        result.append(node)
        for neighbor in graph[node]:
            in_degree[neighbor] -= 1
            if in_degree[neighbor] == 0:
                queue.append(neighbor)

    # Include any remaining (cycle) nodes
    if len(result) != len(all_ids):
        missing = all_ids - set(result)
        for iid in sorted(missing):
            result.append(iid)

    return result


# ---------------------------------------------------------------------------
# Tervezo public API v1 client
# ---------------------------------------------------------------------------

def _get_api_key():
    """Return the API key or exit with an error."""
    api_key = os.environ.get("TERVEZO_API_KEY")
    if not api_key:
        print("ERROR: TERVEZO_API_KEY environment variable is not set")
        sys.exit(1)
    return api_key


def tervezo_api(method, path, body=None):
    """
    Call the Tervezo public REST API (v1).

    All endpoints are under /api/v1/. Authentication is via Bearer token
    using the TERVEZO_API_KEY env var.
    """
    base_url = os.environ.get(
        "TERVEZO_API_URL", "https://app.tervezo.ai"
    ).rstrip("/")
    api_key = _get_api_key()

    url = f"{base_url}/api/v1/{path.lstrip('/')}"
    headers = {
        "Authorization": f"Bearer {api_key}",
        "Content-Type": "application/json",
        "Accept": "application/json",
    }

    data = json.dumps(body).encode("utf-8") if body else None
    req = urllib.request.Request(url, data=data, headers=headers, method=method)

    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            resp_body = resp.read().decode("utf-8")
            return json.loads(resp_body) if resp_body else {}
    except urllib.error.HTTPError as e:
        resp_body = e.read().decode("utf-8", errors="replace")
        try:
            error_data = json.loads(resp_body)
        except json.JSONDecodeError:
            error_data = {"raw": resp_body[:500]}
        return {"error": error_data, "status": e.code}
    except urllib.error.URLError as e:
        return {"error": str(e), "status": 0}


def resolve_workspace_id(slug):
    """
    Resolve a workspace slug to its ID via GET /workspaces.

    Fetches the list of accessible workspaces and matches by slug.
    Returns the workspace ID string, or exits with an error if not found.
    """
    print(f"  Resolving workspace slug '{slug}'...")
    result = tervezo_api("GET", "workspaces")

    if "error" in result:
        print(f"ERROR: Failed to fetch workspaces: {result['error']}")
        sys.exit(1)

    workspaces = result if isinstance(result, list) else result.get("items", result.get("data", []))
    if not isinstance(workspaces, list):
        print(f"ERROR: Unexpected workspaces response: {json.dumps(result)[:300]}")
        sys.exit(1)

    for ws in workspaces:
        if ws.get("slug") == slug:
            print(f"  Resolved workspace: {ws['name']} ({ws['id']})")
            return ws["id"]

    available = ", ".join(ws.get("slug", "?") for ws in workspaces)
    print(f"ERROR: Workspace slug '{slug}' not found. Available: {available}")
    sys.exit(1)


def find_active_implementation(gh_issue_number):
    """
    Check if there is already an active implementation for a GitHub issue.

    Queries GET /implementations for non-terminal statuses and matches
    by issue number in the title or branch name.
    """
    active_statuses = ["pending", "queued", "running"]
    issue_marker = f"issue-{gh_issue_number}"
    issue_hash = f"#{gh_issue_number}"

    for status in active_statuses:
        result = tervezo_api("GET", f"implementations?status={status}&limit=100")
        if "error" in result:
            continue

        items = result.get("items", [])
        for impl in items:
            title = impl.get("title") or ""
            branch = impl.get("branch") or ""
            if issue_marker in branch or issue_hash in title:
                return impl

    return None


def schedule_implementation(gh_issue_number, gh_issue_title, beads_id,
                            workspace_id, mode="feature", base_branch="main"):
    """
    Schedule a new implementation via POST /api/v1/implementations.

    Checks for an existing active implementation first to avoid duplicates.
    Creates a feature (or bugfix) implementation with the GitHub issue
    details as the prompt.
    """
    issue_url = f"https://github.com/{REPO}/issues/{gh_issue_number}"

    prompt = (
        f"Implement GitHub issue #{gh_issue_number}: {gh_issue_title}\n\n"
        f"Issue URL: {issue_url}\n"
        f"Beads ID: {beads_id}\n\n"
        f"Please read the full issue description from the URL above and "
        f"implement all acceptance criteria."
    )

    print(f"  Scheduling implementation for #{gh_issue_number}: {gh_issue_title}")
    print(f"  Issue URL: {issue_url}")
    print(f"  Beads ID: {beads_id}")
    print(f"  Mode: {mode}")
    print(f"  Base branch: {base_branch}")

    body = {
        "prompt": prompt,
        "mode": mode,
        "workspaceId": workspace_id,
        "repositoryName": REPO,
        "baseBranch": base_branch,
    }

    result = tervezo_api("POST", "implementations", body)

    if "error" in result:
        error = result["error"]
        status_code = result.get("status", "unknown")

        if isinstance(error, dict):
            error_msg = error.get("message", str(error))
        else:
            error_msg = str(error)

        print(f"\n  API returned {status_code}: {error_msg}")

        if status_code == 401:
            print("\n  Your API key does not have permission to create")
            print("  implementations. Check your TERVEZO_API_KEY.")
            print(f"\n  Next issue to implement: {issue_url}")

        return {"scheduled": False, "issue_url": issue_url, "error": error_msg}

    impl_id = result.get("id")
    impl_url = result.get("url")
    impl_branch = result.get("branch")

    print(f"  Implementation created: {impl_id}")
    if impl_url:
        print(f"  URL: {impl_url}")
    if impl_branch:
        print(f"  Branch: {impl_branch}")

    return {
        "scheduled": True,
        "issue_url": issue_url,
        "implementation_id": impl_id,
        "implementation_url": impl_url,
        "branch": impl_branch,
    }


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="Schedule the next implementation via the tervezo public API (v1)"
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would be scheduled without calling the API",
    )
    parser.add_argument(
        "--list-ready",
        action="store_true",
        help="List all issues that are ready to be implemented",
    )
    parser.add_argument(
        "--issue",
        type=str,
        help="Schedule a specific beads issue ID (e.g. warpgrid-agm.10)",
    )
    parser.add_argument(
        "--workspace",
        type=str,
        default=os.environ.get("TERVEZO_WORKSPACE_SLUG"),
        help="Workspace slug (or set TERVEZO_WORKSPACE_SLUG env var)",
    )
    parser.add_argument(
        "--mode",
        choices=["feature", "bugfix"],
        default="feature",
        help="Implementation mode (default: feature)",
    )
    parser.add_argument(
        "--base-branch",
        type=str,
        default="main",
        help="Base branch for the implementation (default: main)",
    )
    parser.add_argument(
        "--count",
        type=int,
        default=1,
        help="Number of new implementations to schedule (skips don't count)",
    )
    args = parser.parse_args()

    # Change to repo root
    repo_root = Path(__file__).parent.parent
    os.chdir(repo_root)

    # Load data
    print(f"Repository: {REPO}")

    if not os.path.exists(ISSUES_FILE):
        print(f"ERROR: Issues file not found: {ISSUES_FILE}")
        sys.exit(1)
    if not os.path.exists(MAPPING_FILE):
        print(f"ERROR: Mapping file not found: {MAPPING_FILE}")
        sys.exit(1)

    issues = load_issues(ISSUES_FILE)
    mapping = load_mapping(MAPPING_FILE)

    print(f"Loaded {len(issues)} issues, {len(mapping)} mapped to GitHub")

    # Determine which issues are closed (both from beads state and GitHub)
    mapped_numbers = set(mapping.values())
    print("\nFetching issue state from GitHub...")
    closed_gh_numbers, gh_success = fetch_closed_issue_numbers(mapped_numbers)
    reverse_mapping = {v: k for k, v in mapping.items()}

    # Build the set of closed beads IDs
    closed_beads_ids = set()
    for beads_id, issue in issues.items():
        if issue.get("status") == "closed":
            closed_beads_ids.add(beads_id)

    if gh_success:
        # Also mark issues closed on GitHub (but not yet in beads) as done
        for gh_num in closed_gh_numbers:
            beads_id = reverse_mapping.get(gh_num)
            if beads_id:
                closed_beads_ids.add(beads_id)
        print(f"  {len(closed_gh_numbers)} issues closed on GitHub")
    else:
        print("  WARNING: Could not fetch from GitHub, using local beads state")

    print(f"  {len(closed_beads_ids)} issues completed total")

    # Find ready issues
    ready = find_ready_issues(issues, mapping, closed_beads_ids)

    if not ready:
        print("\nNo issues are ready for implementation.")
        print("All remaining issues have unresolved blocking dependencies,")
        print("or all issues are already completed.")
        return

    # --list-ready: show all ready issues
    if args.list_ready:
        print(f"\n{len(ready)} issues ready for implementation:\n")
        for beads_id, gh_num, title in ready:
            issue = issues[beads_id]
            priority = issue.get("priority", 0)
            milestone = extract_milestone(issue.get("description", ""))
            ms_str = f" [{milestone}]" if milestone else ""
            print(f"  #{gh_num:3d}  P{priority}  {beads_id:20s}{ms_str}")
            print(f"        {title}")
        return

    # Build candidate list
    if args.issue:
        target_beads_id = args.issue
        if target_beads_id not in issues:
            print(f"ERROR: Beads ID '{target_beads_id}' not found")
            sys.exit(1)
        if target_beads_id not in mapping:
            print(f"ERROR: Beads ID '{target_beads_id}' not mapped to GitHub")
            sys.exit(1)
        gh_num = mapping[target_beads_id]
        title = issues[target_beads_id]["title"]
        candidates = [(target_beads_id, gh_num, title)]
    else:
        candidates = ready

    # Resolve workspace ID (needed for scheduling, not dry-run)
    workspace_id = None
    if not args.dry_run:
        if not args.workspace:
            print("ERROR: --workspace slug or TERVEZO_WORKSPACE_SLUG env var required")
            sys.exit(1)
        workspace_id = resolve_workspace_id(args.workspace)

    # Schedule up to --count new implementations (skips don't count)
    scheduled_count = 0
    skipped_count = 0
    failed_count = 0
    results = []

    for beads_id, gh_num, title in candidates:
        if scheduled_count >= args.count:
            break

        issue = issues[beads_id]
        priority = issue.get("priority", 0)
        milestone = extract_milestone(issue.get("description", ""))

        print(f"\n--- [{scheduled_count + 1}/{args.count}] ---")
        print(f"  GitHub: #{gh_num}")
        print(f"  Beads:  {beads_id}")
        print(f"  Title:  {title}")
        print(f"  Priority: P{priority}")
        if milestone:
            print(f"  Milestone: {milestone}")

        # Show blockers that are satisfied
        blockers = get_blocking_deps(issue, set(issues.keys()))
        if blockers:
            print(f"  Blocking deps (all satisfied):")
            for b in sorted(blockers):
                b_num = mapping.get(b, "?")
                print(f"    #{b_num} ({b})")

        # Check for existing active implementation (both dry-run and real)
        existing = find_active_implementation(gh_num)
        if existing:
            print(f"  SKIPPED: already in progress ({existing.get('status')})")
            skipped_count += 1
            continue

        if args.dry_run:
            issue_url = f"https://github.com/{REPO}/issues/{gh_num}"
            print(f"  [DRY RUN] Would schedule {args.mode}: {issue_url}")
            scheduled_count += 1
            results.append({
                "dry_run": True,
                "beads_id": beads_id,
                "github_issue": gh_num,
                "issue_url": issue_url,
                "title": title,
                "mode": args.mode,
            })
            continue

        result = schedule_implementation(
            gh_num, title, beads_id,
            workspace_id=workspace_id,
            mode=args.mode,
            base_branch=args.base_branch,
        )

        if result.get("scheduled"):
            print(f"  SUCCESS: scheduled")
            scheduled_count += 1
        else:
            print(f"  FAILED: {result.get('error', 'unknown error')}")
            failed_count += 1
            # Count failures toward the target to avoid infinite attempts
            scheduled_count += 1

        entry = {
            "beads_id": beads_id,
            "github_issue": gh_num,
            "issue_url": f"https://github.com/{REPO}/issues/{gh_num}",
            "title": title,
            "mode": args.mode,
            "scheduled": result.get("scheduled", False),
        }
        if result.get("implementation_id"):
            entry["implementation_id"] = result["implementation_id"]
        if result.get("implementation_url"):
            entry["implementation_url"] = result["implementation_url"]
        if result.get("branch"):
            entry["branch"] = result["branch"]
        if result.get("error"):
            entry["error"] = result["error"]
        results.append(entry)

    # Summary
    print("\n" + "=" * 60)
    print(f"Scheduled: {scheduled_count - failed_count}  "
          f"Skipped: {skipped_count}  "
          f"Failed: {failed_count}  "
          f"Ready: {len(candidates)}")
    print("=" * 60)

    print(f"\n{json.dumps(results, indent=2)}")


def extract_milestone(description):
    """Extract milestone name from issue description."""
    if not description:
        return None
    if "Milestone:" in description:
        return description.split("Milestone:")[1].split("|")[0].strip()
    return None


if __name__ == "__main__":
    main()
