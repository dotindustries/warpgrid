How to use schedule-next-implementation.py

  Prerequisites

  Set your API token in the environment:

  export TERVEZO_API_KEY="tzv_your-api-token-here"
  export TERVEZO_WORKSPACE_SLUG="your-workspace-slug"

  Optional env vars:
  - TERVEZO_API_URL — defaults to https://app.tervezo.ai

  Usage

  1. See what's ready to implement:
  python3 scripts/schedule-next-implementation.py --list-ready
  This analyzes the beads dependency graph + GitHub issue state and shows all unblocked issues in topological order.

  2. Preview the next scheduled issue (no API call):
  python3 scripts/schedule-next-implementation.py --dry-run

  3. Schedule the next ready issue as a feature:
  python3 scripts/schedule-next-implementation.py --workspace my-workspace
  This picks the first unblocked issue (highest topological priority) and calls POST /api/v1/implementations to start an implementation.

  4. Schedule a specific issue as a bugfix:
  python3 scripts/schedule-next-implementation.py --issue warpgrid-agm.10 --mode bugfix

  5. Use a different base branch:
  python3 scripts/schedule-next-implementation.py --base-branch develop

  What happens under the hood

  1. Loads beads issues from .beads/issues.jsonl and the GitHub mapping from beads-to-github-mapping.json
  2. Queries GitHub (via gh CLI) for closed issues to determine what's already done
  3. Runs topological sort on the dependency graph to find issues whose blockers are all satisfied
  4. Resolves the workspace slug to a workspace ID via GET /api/v1/workspaces
  5. Calls POST /api/v1/implementations with the prompt, mode, workspaceId, and repositoryName
  6. Tervezo picks it up and starts an implementation pipeline

  API Reference

  The script uses the Tervezo public API v1 (https://app.tervezo.ai/api/v1/docs):
  - GET /api/v1/workspaces — list accessible workspaces (returns id, name, slug, logo)
  - POST /api/v1/implementations — create a new implementation (requires prompt, mode, workspaceId)
