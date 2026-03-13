#!/usr/bin/env python3
"""
Tests for schedule-next-implementation.py bug fixes.

Critical paths tested:
1. URL double-prefix stripping — TERVEZO_API_URL ending in /api or /api/v1
   must not produce double-prefixed URLs like /api/api/v1/...
2. Non-JSON response handling — tervezo_api must not crash on HTML or other
   non-JSON responses from the server
3. HTTPError with non-JSON body — error responses that are HTML must not crash
"""

import http.server
import json
import os
import sys
import threading
import unittest
from pathlib import Path
from unittest import mock

# Import the schedule module
sys.path.insert(0, str(Path(__file__).parent))
from importlib import import_module
schedule = import_module("schedule-next-implementation")


# ---------------------------------------------------------------------------
# Lightweight HTTP server for integration-style tests
# ---------------------------------------------------------------------------

class _Handler(http.server.BaseHTTPRequestHandler):
    """Records requests and responds with configurable body/status."""

    requests = []
    response_body = b'{"ok": true}'
    response_status = 200
    response_content_type = "application/json"

    def do_GET(self):
        self._handle()

    def do_POST(self):
        self._handle()

    def _handle(self):
        _Handler.requests.append({
            "method": self.command,
            "path": self.path,
            "headers": dict(self.headers),
        })
        self.send_response(_Handler.response_status)
        self.send_header("Content-Type", _Handler.response_content_type)
        self.end_headers()
        self.wfile.write(_Handler.response_body)

    def log_message(self, format, *args):
        pass  # silence logs


def _start_server():
    server = http.server.HTTPServer(("127.0.0.1", 0), _Handler)
    port = server.server_address[1]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, port


class TestTervezoApiUrlConstruction(unittest.TestCase):
    """URL double-prefix bug: TERVEZO_API_URL ending in /api or /api/v1
    must not produce /api/api/v1/... in the final request URL."""

    @classmethod
    def setUpClass(cls):
        cls.server, cls.port = _start_server()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()

    def setUp(self):
        _Handler.requests.clear()
        _Handler.response_body = b'{"items": []}'
        _Handler.response_status = 200
        _Handler.response_content_type = "application/json"

    def _call(self, base_url, path):
        env = {
            "TERVEZO_API_URL": base_url,
            "TERVEZO_API_KEY": "tzv_test_key",
        }
        with mock.patch.dict(os.environ, env, clear=False):
            return schedule.tervezo_api("GET", path)

    def test_base_url_without_suffix_produces_correct_path(self):
        base = f"http://127.0.0.1:{self.port}"
        self._call(base, "workspaces")
        self.assertEqual(len(_Handler.requests), 1)
        self.assertEqual(_Handler.requests[0]["path"], "/api/v1/workspaces")

    def test_base_url_with_api_suffix_strips_it(self):
        base = f"http://127.0.0.1:{self.port}/api"
        self._call(base, "workspaces")
        self.assertEqual(len(_Handler.requests), 1)
        self.assertEqual(_Handler.requests[0]["path"], "/api/v1/workspaces")

    def test_base_url_with_api_v1_suffix_strips_it(self):
        base = f"http://127.0.0.1:{self.port}/api/v1"
        self._call(base, "workspaces")
        self.assertEqual(len(_Handler.requests), 1)
        self.assertEqual(_Handler.requests[0]["path"], "/api/v1/workspaces")

    def test_base_url_with_trailing_slash_handled(self):
        base = f"http://127.0.0.1:{self.port}/api/"
        self._call(base, "workspaces")
        self.assertEqual(len(_Handler.requests), 1)
        # After rstrip("/") then strip "/api" -> correct
        self.assertEqual(_Handler.requests[0]["path"], "/api/v1/workspaces")


class TestTervezoApiNonJsonResponse(unittest.TestCase):
    """Non-JSON response handling: tervezo_api must not crash
    when the server returns HTML or other non-JSON content."""

    @classmethod
    def setUpClass(cls):
        cls.server, cls.port = _start_server()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()

    def setUp(self):
        _Handler.requests.clear()

    def _call(self, path="workspaces"):
        base = f"http://127.0.0.1:{self.port}"
        env = {
            "TERVEZO_API_URL": base,
            "TERVEZO_API_KEY": "tzv_test_key",
        }
        with mock.patch.dict(os.environ, env, clear=False):
            return schedule.tervezo_api("GET", path)

    def test_html_response_returns_empty_dict(self):
        _Handler.response_body = b"<html><body>Service Unavailable</body></html>"
        _Handler.response_status = 200
        _Handler.response_content_type = "text/html"
        result = self._call()
        self.assertEqual(result, {})

    def test_empty_response_returns_empty_dict(self):
        _Handler.response_body = b""
        _Handler.response_status = 200
        _Handler.response_content_type = "application/json"
        result = self._call()
        self.assertEqual(result, {})

    def test_whitespace_only_response_returns_empty_dict(self):
        _Handler.response_body = b"   \n  "
        _Handler.response_status = 200
        _Handler.response_content_type = "application/json"
        result = self._call()
        self.assertEqual(result, {})

    def test_valid_json_response_is_parsed(self):
        _Handler.response_body = json.dumps({"id": "ws-123", "name": "test"}).encode()
        _Handler.response_status = 200
        _Handler.response_content_type = "application/json"
        result = self._call()
        self.assertEqual(result, {"id": "ws-123", "name": "test"})


class TestTervezoApiHttpError(unittest.TestCase):
    """HTTPError handling: error responses with non-JSON bodies
    must not crash the script."""

    @classmethod
    def setUpClass(cls):
        cls.server, cls.port = _start_server()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()

    def setUp(self):
        _Handler.requests.clear()

    def _call(self, path="workspaces"):
        base = f"http://127.0.0.1:{self.port}"
        env = {
            "TERVEZO_API_URL": base,
            "TERVEZO_API_KEY": "tzv_test_key",
        }
        with mock.patch.dict(os.environ, env, clear=False):
            return schedule.tervezo_api("GET", path)

    def test_500_with_json_error_returns_error_dict(self):
        _Handler.response_body = json.dumps({"message": "Internal Server Error"}).encode()
        _Handler.response_status = 500
        _Handler.response_content_type = "application/json"
        result = self._call()
        self.assertIn("error", result)
        self.assertEqual(result["status"], 500)

    def test_500_with_html_error_returns_raw_body(self):
        _Handler.response_body = b"<html><body>500 Error</body></html>"
        _Handler.response_status = 500
        _Handler.response_content_type = "text/html"
        result = self._call()
        self.assertIn("error", result)
        self.assertEqual(result["status"], 500)
        # Non-JSON error body should be captured in raw field
        self.assertIn("raw", result["error"])

    def test_401_returns_error_with_status(self):
        _Handler.response_body = json.dumps({"message": "Unauthorized"}).encode()
        _Handler.response_status = 401
        _Handler.response_content_type = "application/json"
        result = self._call()
        self.assertIn("error", result)
        self.assertEqual(result["status"], 401)


class TestFindReadyIssues(unittest.TestCase):
    """Test find_ready_issues correctly filters and orders candidates."""

    def test_issue_with_all_deps_closed_is_ready(self):
        issues = {
            "agm.1": {"id": "agm.1", "title": "Task 1", "issue_type": "task", "dependencies": []},
            "agm.2": {"id": "agm.2", "title": "Task 2", "issue_type": "task", "dependencies": [
                {"type": "blocks", "depends_on_id": "agm.1"},
            ]},
        }
        mapping = {"agm.1": 1, "agm.2": 2}
        closed = {"agm.1"}
        ready = schedule.find_ready_issues(issues, mapping, closed)
        self.assertEqual(len(ready), 1)
        self.assertEqual(ready[0][0], "agm.2")

    def test_issue_with_unresolved_dep_is_not_ready(self):
        issues = {
            "agm.1": {"id": "agm.1", "title": "Task 1", "issue_type": "task", "dependencies": []},
            "agm.2": {"id": "agm.2", "title": "Task 2", "issue_type": "task", "dependencies": [
                {"type": "blocks", "depends_on_id": "agm.1"},
            ]},
        }
        mapping = {"agm.1": 1, "agm.2": 2}
        closed = set()  # agm.1 not closed
        ready = schedule.find_ready_issues(issues, mapping, closed)
        # Only agm.1 is ready (no deps), agm.2 is blocked
        beads_ids = [r[0] for r in ready]
        self.assertIn("agm.1", beads_ids)
        self.assertNotIn("agm.2", beads_ids)

    def test_epics_are_excluded(self):
        issues = {
            "agm": {"id": "agm", "title": "Epic", "issue_type": "epic", "dependencies": []},
            "agm.1": {"id": "agm.1", "title": "Task 1", "issue_type": "task", "dependencies": []},
        }
        mapping = {"agm": 1, "agm.1": 2}
        closed = set()
        ready = schedule.find_ready_issues(issues, mapping, closed)
        beads_ids = [r[0] for r in ready]
        self.assertNotIn("agm", beads_ids)

    def test_unmapped_issues_are_excluded(self):
        issues = {
            "agm.1": {"id": "agm.1", "issue_type": "task", "dependencies": []},
        }
        mapping = {}  # not mapped
        closed = set()
        ready = schedule.find_ready_issues(issues, mapping, closed)
        self.assertEqual(len(ready), 0)


class TestExtractMilestone(unittest.TestCase):
    """Test milestone extraction (shared with migrate script)."""

    def test_none_description(self):
        self.assertIsNone(schedule.extract_milestone(None))

    def test_empty_description(self):
        self.assertIsNone(schedule.extract_milestone(""))

    def test_valid_milestone(self):
        self.assertEqual(
            schedule.extract_milestone("Milestone: M1.3 | Depends on: US-101"),
            "M1.3",
        )


if __name__ == "__main__":
    os.chdir(Path(__file__).parent.parent)
    unittest.main(verbosity=2)
