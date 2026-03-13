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

Environment variables (set automatically in tervezo sandboxes):
    TERVEZO_API_URL        Base URL for the tervezo API
    TERVEZO_API_KEY        JWT or API key for authentication
    TERVEZO_WORKSPACE_ID   Workspace ID
    TERVEZO_PROJECT_ID     Project identifier
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
# Tervezo API client
# ---------------------------------------------------------------------------

def tervezo_api_call(method, path, body=None):
    """
    Make a tervezo API call.

    Supports both tRPC (POST to /api/trpc/<procedure>) and REST patterns.
    Authentication is via the TERVEZO_API_KEY env var.
    """
    api_url = os.environ.get("TERVEZO_API_URL", "https://app.tervezo.ai/api")
    api_key = os.environ.get("TERVEZO_API_KEY")

    if not api_key:
        print("ERROR: TERVEZO_API_KEY environment variable is not set")
        sys.exit(1)

    url = f"{api_url}/{path.lstrip('/')}"
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


def trpc_mutation(procedure, input_data):
    """Call a tRPC mutation (POST)."""
    return tervezo_api_call("POST", f"trpc/{procedure}", {"json": input_data})


def trpc_query(procedure, input_data=None):
    """Call a tRPC query (GET with input parameter)."""
    import urllib.parse
    path = f"trpc/{procedure}"
    if input_data is not None:
        encoded = urllib.parse.quote(json.dumps({"json": input_data}))
        path += f"?input={encoded}"
    return tervezo_api_call("GET", path)


def schedule_implementation(gh_issue_number, gh_issue_title, beads_id):
    """
    Schedule a new implementation for the given GitHub issue via the
    tervezo API.

    Creates a brief with the issue details, which triggers an
    implementation pipeline in tervezo.
    """
    workspace_id = os.environ.get("TERVEZO_WORKSPACE_ID")
    project_id = os.environ.get("TERVEZO_PROJECT_ID")
    issue_url = f"https://github.com/{REPO}/issues/{gh_issue_number}"

    print(f"\n  Scheduling implementation for #{gh_issue_number}: {gh_issue_title}")
    print(f"  Issue URL: {issue_url}")
    print(f"  Beads ID: {beads_id}")
    print(f"  Workspace: {workspace_id}")
    print(f"  Project: {project_id}")

    # Create a brief via tRPC — this triggers the implementation pipeline.
    # The brief includes the GitHub issue URL so tervezo can link them.
    brief_input = {
        "issueUrl": issue_url,
        "title": gh_issue_title,
    }
    if workspace_id:
        brief_input["workspaceId"] = workspace_id
    if project_id:
        brief_input["project"] = project_id

    result = trpc_mutation("briefs.create", brief_input)

    if "error" in result:
        error = result["error"]
        status_code = result.get("status", "unknown")

        # If briefs.create fails with UNAUTHORIZED, the sandbox token
        # doesn't have permission. Fall back to providing clear
        # instructions for manual scheduling.
        if isinstance(error, dict):
            error_msg = error.get("json", {}).get("message", str(error))
        else:
            error_msg = str(error)

        print(f"\n  API returned {status_code}: {error_msg}")

        if status_code == 401:
            print("\n  The sandbox API key does not have permission to")
            print("  create briefs directly. To schedule this issue,")
            print("  use the tervezo dashboard or a workspace-scoped API key.")
            print(f"\n  Next issue to implement: {issue_url}")
            return {"scheduled": False, "issue_url": issue_url, "error": error_msg}

        return {"scheduled": False, "issue_url": issue_url, "error": error_msg}

    brief_id = result.get("result", {}).get("data", {}).get("json", {}).get("id")
    if brief_id:
        print(f"  Brief created: {brief_id}")
    else:
        print(f"  Response: {json.dumps(result, indent=2)[:500]}")

    return {"scheduled": True, "issue_url": issue_url, "brief_id": brief_id}


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="Schedule the next implementation via the tervezo API"
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

    # --issue: schedule a specific issue
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
        target = (target_beads_id, gh_num, title)
    else:
        # Pick the first ready issue (highest topological priority)
        target = ready[0]

    beads_id, gh_num, title = target
    issue = issues[beads_id]
    priority = issue.get("priority", 0)
    milestone = extract_milestone(issue.get("description", ""))

    print(f"\nNext implementation:")
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

    if args.dry_run:
        issue_url = f"https://github.com/{REPO}/issues/{gh_num}"
        print(f"\n[DRY RUN] Would schedule implementation for: {issue_url}")

        # Output machine-readable result for piping
        result = {
            "dry_run": True,
            "beads_id": beads_id,
            "github_issue": gh_num,
            "issue_url": issue_url,
            "title": title,
            "ready_count": len(ready),
        }
        print(f"\n{json.dumps(result, indent=2)}")
        return

    # Schedule the implementation
    result = schedule_implementation(gh_num, title, beads_id)

    # Output summary
    print("\n" + "=" * 60)
    if result.get("scheduled"):
        print("SUCCESS: Implementation scheduled")
    else:
        print("NOTICE: Could not auto-schedule via API")
        print(f"Next issue URL: {result.get('issue_url')}")
    print("=" * 60)

    # Output machine-readable result
    output = {
        "beads_id": beads_id,
        "github_issue": gh_num,
        "issue_url": f"https://github.com/{REPO}/issues/{gh_num}",
        "title": title,
        "scheduled": result.get("scheduled", False),
        "ready_count": len(ready),
    }
    if result.get("brief_id"):
        output["brief_id"] = result["brief_id"]
    if result.get("error"):
        output["error"] = result["error"]

    print(f"\n{json.dumps(output, indent=2)}")


def extract_milestone(description):
    """Extract milestone name from issue description."""
    if not description:
        return None
    if "Milestone:" in description:
        return description.split("Milestone:")[1].split("|")[0].strip()
    return None


if __name__ == "__main__":
    main()
