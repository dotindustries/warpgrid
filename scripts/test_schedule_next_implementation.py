#!/usr/bin/env python3
"""
Tests for schedule-next-implementation.py.

Critical paths tested:
1. tervezo_api URL construction — double /api prefix bug (the fix)
2. tervezo_api empty/whitespace response handling (the fix)
3. tervezo_api HTTP error handling
4. find_ready_issues dependency graph analysis
5. get_blocking_deps extraction
6. extract_milestone parsing
"""

import json
import os
import sys
import http.server
import threading
import unittest
from pathlib import Path
from unittest.mock import patch, MagicMock

# Import the schedule module
sys.path.insert(0, str(Path(__file__).parent))
from importlib import import_module
schedule = import_module("schedule-next-implementation")


# ---------------------------------------------------------------------------
# tervezo_api URL construction (Bug fix #2: double /api prefix)
# ---------------------------------------------------------------------------

class TestTervezoApiUrlConstruction(unittest.TestCase):
    """Test that tervezo_api constructs the correct URL for various base URL forms."""

    def _capture_url(self, base_url, path):
        """Call tervezo_api and capture the URL it constructs without making a real request."""
        captured = {}

        def fake_urlopen(req, **kwargs):
            captured["url"] = req.full_url
            # Return a mock response
            mock_resp = MagicMock()
            mock_resp.read.return_value = b"{}"
            mock_resp.__enter__ = lambda s: s
            mock_resp.__exit__ = MagicMock(return_value=False)
            return mock_resp

        with patch.dict(os.environ, {
            "TERVEZO_API_URL": base_url,
            "TERVEZO_API_KEY": "test-key",
        }):
            with patch("urllib.request.urlopen", side_effect=fake_urlopen):
                schedule.tervezo_api("GET", path)

        return captured.get("url")

    def test_default_base_url_no_trailing_slash(self):
        url = self._capture_url("https://app.tervezo.ai", "workspaces")
        self.assertEqual(url, "https://app.tervezo.ai/api/v1/workspaces")

    def test_base_url_with_trailing_slash(self):
        url = self._capture_url("https://app.tervezo.ai/", "workspaces")
        self.assertEqual(url, "https://app.tervezo.ai/api/v1/workspaces")

    def test_base_url_ending_with_api_no_double_prefix(self):
        """This is the bug that was fixed — /api should not be doubled."""
        url = self._capture_url("https://app.tervezo.ai/api", "workspaces")
        self.assertEqual(url, "https://app.tervezo.ai/api/v1/workspaces")

    def test_base_url_ending_with_api_slash_no_double_prefix(self):
        url = self._capture_url("https://app.tervezo.ai/api/", "workspaces")
        self.assertEqual(url, "https://app.tervezo.ai/api/v1/workspaces")

    def test_path_with_leading_slash(self):
        url = self._capture_url("https://app.tervezo.ai", "/workspaces")
        self.assertEqual(url, "https://app.tervezo.ai/api/v1/workspaces")

    def test_path_with_query_params(self):
        url = self._capture_url("https://app.tervezo.ai", "implementations?status=pending&limit=100")
        self.assertEqual(url, "https://app.tervezo.ai/api/v1/implementations?status=pending&limit=100")


# ---------------------------------------------------------------------------
# tervezo_api response handling (Bug fix #1: empty/whitespace responses)
# ---------------------------------------------------------------------------

class TestTervezoApiResponseHandling(unittest.TestCase):
    """Test that tervezo_api correctly handles various response body forms."""

    def _mock_response(self, body_bytes, status=200):
        """Create a mock urllib response."""
        mock_resp = MagicMock()
        mock_resp.read.return_value = body_bytes
        mock_resp.__enter__ = lambda s: s
        mock_resp.__exit__ = MagicMock(return_value=False)
        return mock_resp

    def _call_api(self, body_bytes):
        """Call tervezo_api with a mocked response returning the given bytes."""
        with patch.dict(os.environ, {
            "TERVEZO_API_URL": "https://app.tervezo.ai",
            "TERVEZO_API_KEY": "test-key",
        }):
            with patch("urllib.request.urlopen", return_value=self._mock_response(body_bytes)):
                return schedule.tervezo_api("GET", "workspaces")

    def test_valid_json_response(self):
        result = self._call_api(b'{"items": [1, 2, 3]}')
        self.assertEqual(result, {"items": [1, 2, 3]})

    def test_empty_string_response(self):
        result = self._call_api(b"")
        self.assertEqual(result, {})

    def test_whitespace_only_response(self):
        """This is the bug that was fixed — whitespace should not crash json.loads."""
        result = self._call_api(b"   \n  \t  ")
        self.assertEqual(result, {})

    def test_newline_only_response(self):
        result = self._call_api(b"\n")
        self.assertEqual(result, {})

    def test_html_response_returns_empty_dict(self):
        """Non-JSON (e.g. HTML error page) should return {} instead of crashing."""
        result = self._call_api(b"<html><body>Not Found</body></html>")
        self.assertEqual(result, {})

    def test_json_with_surrounding_whitespace(self):
        result = self._call_api(b'  \n  {"ok": true}  \n  ')
        self.assertEqual(result, {"ok": True})

    def test_json_list_response(self):
        result = self._call_api(b'[{"id": "ws1", "slug": "lumen"}]')
        self.assertEqual(result, [{"id": "ws1", "slug": "lumen"}])


class TestTervezoApiErrorHandling(unittest.TestCase):
    """Test tervezo_api HTTP error handling paths."""

    def test_http_error_returns_error_dict(self):
        import urllib.error
        error = urllib.error.HTTPError(
            url="https://app.tervezo.ai/api/v1/workspaces",
            code=500,
            msg="Internal Server Error",
            hdrs={},
            fp=MagicMock(read=MagicMock(return_value=b'{"message": "server error"}')),
        )
        with patch.dict(os.environ, {
            "TERVEZO_API_URL": "https://app.tervezo.ai",
            "TERVEZO_API_KEY": "test-key",
        }):
            with patch("urllib.request.urlopen", side_effect=error):
                result = schedule.tervezo_api("GET", "workspaces")
        self.assertIn("error", result)
        self.assertEqual(result["status"], 500)

    def test_http_error_with_non_json_body(self):
        import urllib.error
        error = urllib.error.HTTPError(
            url="https://app.tervezo.ai/api/v1/workspaces",
            code=502,
            msg="Bad Gateway",
            hdrs={},
            fp=MagicMock(read=MagicMock(return_value=b"<html>Bad Gateway</html>")),
        )
        with patch.dict(os.environ, {
            "TERVEZO_API_URL": "https://app.tervezo.ai",
            "TERVEZO_API_KEY": "test-key",
        }):
            with patch("urllib.request.urlopen", side_effect=error):
                result = schedule.tervezo_api("GET", "workspaces")
        self.assertIn("error", result)
        self.assertEqual(result["status"], 502)
        self.assertIn("raw", result["error"])

    def test_url_error_returns_error_dict(self):
        import urllib.error
        error = urllib.error.URLError("Connection refused")
        with patch.dict(os.environ, {
            "TERVEZO_API_URL": "https://app.tervezo.ai",
            "TERVEZO_API_KEY": "test-key",
        }):
            with patch("urllib.request.urlopen", side_effect=error):
                result = schedule.tervezo_api("GET", "workspaces")
        self.assertIn("error", result)
        self.assertEqual(result["status"], 0)


# ---------------------------------------------------------------------------
# find_ready_issues and get_blocking_deps
# ---------------------------------------------------------------------------

class TestGetBlockingDeps(unittest.TestCase):
    """Test extraction of blocking dependency IDs."""

    def test_no_dependencies(self):
        issue = {"id": "a.1", "dependencies": []}
        self.assertEqual(schedule.get_blocking_deps(issue, {"a.1"}), set())

    def test_blocks_dependency(self):
        issue = {"id": "a.2", "dependencies": [
            {"type": "blocks", "depends_on_id": "a.1"},
        ]}
        result = schedule.get_blocking_deps(issue, {"a.1", "a.2"})
        self.assertEqual(result, {"a.1"})

    def test_ignores_parent_child_dependency(self):
        issue = {"id": "a.2", "dependencies": [
            {"type": "parent-child", "depends_on_id": "a.1"},
        ]}
        result = schedule.get_blocking_deps(issue, {"a.1", "a.2"})
        self.assertEqual(result, set())

    def test_ignores_deps_outside_all_ids(self):
        issue = {"id": "a.2", "dependencies": [
            {"type": "blocks", "depends_on_id": "external.1"},
        ]}
        result = schedule.get_blocking_deps(issue, {"a.1", "a.2"})
        self.assertEqual(result, set())


class TestFindReadyIssues(unittest.TestCase):
    """Test the dependency graph analysis that determines which issues are ready."""

    def _make_issues(self):
        """Create a small test issue graph:
        a.1 (closed) -> a.2 (open, ready) -> a.3 (open, blocked)
        a.4 (open, no deps, ready)
        a.5 (epic, skipped)
        """
        return {
            "warpgrid-agm": {
                "id": "warpgrid-agm", "title": "Epic",
                "issue_type": "epic", "status": "open",
                "dependencies": [],
            },
            "warpgrid-agm.1": {
                "id": "warpgrid-agm.1", "title": "Task 1 (done)",
                "issue_type": "task", "status": "closed",
                "dependencies": [],
            },
            "warpgrid-agm.2": {
                "id": "warpgrid-agm.2", "title": "Task 2 (depends on 1)",
                "issue_type": "task", "status": "open",
                "dependencies": [{"type": "blocks", "depends_on_id": "warpgrid-agm.1"}],
            },
            "warpgrid-agm.3": {
                "id": "warpgrid-agm.3", "title": "Task 3 (depends on 2)",
                "issue_type": "task", "status": "open",
                "dependencies": [{"type": "blocks", "depends_on_id": "warpgrid-agm.2"}],
            },
            "warpgrid-agm.4": {
                "id": "warpgrid-agm.4", "title": "Task 4 (no deps)",
                "issue_type": "task", "status": "open",
                "dependencies": [],
            },
        }

    def _make_mapping(self):
        return {
            "warpgrid-agm": 1,
            "warpgrid-agm.1": 2,
            "warpgrid-agm.2": 3,
            "warpgrid-agm.3": 4,
            "warpgrid-agm.4": 5,
        }

    def test_finds_unblocked_issues(self):
        issues = self._make_issues()
        mapping = self._make_mapping()
        closed = {"warpgrid-agm.1"}

        ready = schedule.find_ready_issues(issues, mapping, closed)
        ready_ids = [r[0] for r in ready]

        # Task 2 should be ready (its blocker a.1 is closed)
        self.assertIn("warpgrid-agm.2", ready_ids)
        # Task 4 should be ready (no blockers)
        self.assertIn("warpgrid-agm.4", ready_ids)

    def test_excludes_blocked_issues(self):
        issues = self._make_issues()
        mapping = self._make_mapping()
        closed = {"warpgrid-agm.1"}

        ready = schedule.find_ready_issues(issues, mapping, closed)
        ready_ids = [r[0] for r in ready]

        # Task 3 is blocked by Task 2 (still open)
        self.assertNotIn("warpgrid-agm.3", ready_ids)

    def test_excludes_already_closed_issues(self):
        issues = self._make_issues()
        mapping = self._make_mapping()
        closed = {"warpgrid-agm.1"}

        ready = schedule.find_ready_issues(issues, mapping, closed)
        ready_ids = [r[0] for r in ready]

        self.assertNotIn("warpgrid-agm.1", ready_ids)

    def test_excludes_epics(self):
        issues = self._make_issues()
        mapping = self._make_mapping()
        closed = set()

        ready = schedule.find_ready_issues(issues, mapping, closed)
        ready_ids = [r[0] for r in ready]

        self.assertNotIn("warpgrid-agm", ready_ids)

    def test_excludes_unmapped_issues(self):
        issues = self._make_issues()
        mapping = self._make_mapping()
        del mapping["warpgrid-agm.4"]  # Remove from mapping
        closed = {"warpgrid-agm.1"}

        ready = schedule.find_ready_issues(issues, mapping, closed)
        ready_ids = [r[0] for r in ready]

        self.assertNotIn("warpgrid-agm.4", ready_ids)

    def test_returns_tuples_with_correct_structure(self):
        issues = self._make_issues()
        mapping = self._make_mapping()
        closed = {"warpgrid-agm.1"}

        ready = schedule.find_ready_issues(issues, mapping, closed)

        for beads_id, gh_num, title in ready:
            self.assertIn(beads_id, issues)
            self.assertEqual(gh_num, mapping[beads_id])
            self.assertEqual(title, issues[beads_id]["title"])

    def test_all_closed_returns_empty(self):
        issues = self._make_issues()
        mapping = self._make_mapping()
        all_ids = set(issues.keys())

        ready = schedule.find_ready_issues(issues, mapping, all_ids)
        self.assertEqual(ready, [])


# ---------------------------------------------------------------------------
# extract_milestone
# ---------------------------------------------------------------------------

class TestExtractMilestoneSchedule(unittest.TestCase):
    """Test milestone extraction (schedule script's own copy)."""

    def test_extracts_milestone(self):
        self.assertEqual(schedule.extract_milestone("Milestone: M1.1 | Dep"), "M1.1")

    def test_none_input(self):
        self.assertIsNone(schedule.extract_milestone(None))

    def test_empty_string(self):
        self.assertIsNone(schedule.extract_milestone(""))

    def test_no_milestone_in_text(self):
        self.assertIsNone(schedule.extract_milestone("No milestone here"))


# ---------------------------------------------------------------------------
# _get_api_key
# ---------------------------------------------------------------------------

class TestGetApiKey(unittest.TestCase):
    """Test API key retrieval from environment."""

    def test_returns_key_when_set(self):
        with patch.dict(os.environ, {"TERVEZO_API_KEY": "tzv_test123"}):
            self.assertEqual(schedule._get_api_key(), "tzv_test123")

    def test_exits_when_not_set(self):
        with patch.dict(os.environ, {}, clear=True):
            # Remove TERVEZO_API_KEY if present
            os.environ.pop("TERVEZO_API_KEY", None)
            with self.assertRaises(SystemExit):
                schedule._get_api_key()


if __name__ == "__main__":
    os.chdir(Path(__file__).parent.parent)
    unittest.main(verbosity=2)
