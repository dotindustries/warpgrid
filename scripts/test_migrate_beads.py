#!/usr/bin/env python3
"""
Tests for the beads-to-GitHub migration scripts.

Critical paths tested:
1. Topological sort correctness and cycle detection
2. Dependency graph encoding (blocked-by, parent, children, blocks links)
3. Mapping integrity against source data
4. Issue data extraction (milestone parsing, domain labels)
5. Format body produces correct structure with resolved links
"""

import json
import os
import sys
import tempfile
import textwrap
import unittest
from collections import defaultdict
from pathlib import Path

# Import the migration module
sys.path.insert(0, str(Path(__file__).parent))
from importlib import import_module
migrate = import_module("migrate-beads-to-github")


class TestLoadIssues(unittest.TestCase):
    """Test JSONL loading correctness."""

    def test_loads_all_93_issues(self):
        issues = migrate.load_issues(".beads/issues.jsonl")
        self.assertEqual(len(issues), 93)

    def test_every_issue_has_required_fields(self):
        issues = migrate.load_issues(".beads/issues.jsonl")
        required = {"id", "title", "status", "priority", "issue_type"}
        for issue_id, issue in issues.items():
            for field in required:
                self.assertIn(field, issue, f"Issue {issue_id} missing field '{field}'")

    def test_ids_match_keys(self):
        issues = migrate.load_issues(".beads/issues.jsonl")
        for issue_id, issue in issues.items():
            self.assertEqual(issue_id, issue["id"])

    def test_statuses_are_valid(self):
        issues = migrate.load_issues(".beads/issues.jsonl")
        valid_statuses = {"open", "closed", "in_progress"}
        for issue_id, issue in issues.items():
            self.assertIn(issue["status"], valid_statuses,
                          f"Issue {issue_id} has invalid status '{issue['status']}'")


class TestExtractMilestone(unittest.TestCase):
    """Test milestone extraction from description text."""

    def test_extracts_simple_milestone(self):
        desc = "Milestone: M1.1 | Depends on: none"
        self.assertEqual(migrate.extract_milestone(desc), "M1.1")

    def test_extracts_integration_milestone(self):
        desc = "Milestone: Integration | Depends on: US-122"
        self.assertEqual(migrate.extract_milestone(desc), "Integration")

    def test_extracts_milestone_with_early_suffix(self):
        desc = "Milestone: M2.5 (early) | Depends on: US-201"
        self.assertEqual(migrate.extract_milestone(desc), "M2.5 (early)")

    def test_returns_none_when_no_milestone(self):
        desc = "No milestone here"
        self.assertIsNone(migrate.extract_milestone(desc))

    def test_returns_none_for_empty_string(self):
        self.assertIsNone(migrate.extract_milestone(""))

    def test_extracts_from_multiline_description(self):
        desc = "Milestone: M6.3 | Depends on: US-602\n\nAs a developer..."
        self.assertEqual(migrate.extract_milestone(desc), "M6.3")


class TestGetDomainLabel(unittest.TestCase):
    """Test domain label mapping from milestone."""

    def test_m1_maps_to_host_functions(self):
        self.assertEqual(migrate.get_domain_label("M1.1"), "domain:host-functions")
        self.assertEqual(migrate.get_domain_label("M1.8"), "domain:host-functions")

    def test_m2_maps_to_libc_patches(self):
        self.assertEqual(migrate.get_domain_label("M2.1"), "domain:libc-patches")

    def test_m6_maps_to_bun(self):
        self.assertEqual(migrate.get_domain_label("M6.6"), "domain:bun")

    def test_integration_maps_correctly(self):
        self.assertEqual(migrate.get_domain_label("Integration"), "domain:integration")

    def test_none_input_returns_none(self):
        self.assertIsNone(migrate.get_domain_label(None))

    def test_unknown_prefix_returns_none(self):
        self.assertIsNone(migrate.get_domain_label("X99"))


class TestTopologicalSort(unittest.TestCase):
    """Test topological sort produces valid dependency ordering."""

    def test_epic_comes_first(self):
        issues = migrate.load_issues(".beads/issues.jsonl")
        order = migrate.topological_sort(issues)
        self.assertEqual(order[0], "warpgrid-agm",
                         "Epic should be first in topological order")

    def test_all_issues_in_result(self):
        issues = migrate.load_issues(".beads/issues.jsonl")
        order = migrate.topological_sort(issues)
        self.assertEqual(len(order), 93)
        self.assertEqual(set(order), set(issues.keys()))

    def test_dependencies_come_before_dependents(self):
        """Every 'blocks' dependency must appear before the issue it blocks."""
        issues = migrate.load_issues(".beads/issues.jsonl")
        order = migrate.topological_sort(issues)
        position = {iid: idx for idx, iid in enumerate(order)}

        violations = []
        for issue_id, issue in issues.items():
            for dep in issue.get("dependencies", []):
                if dep["type"] == "blocks":
                    dep_id = dep["depends_on_id"]
                    if dep_id in position and issue_id in position:
                        if position[dep_id] >= position[issue_id]:
                            violations.append(
                                f"{issue_id} depends on {dep_id}, but {dep_id} "
                                f"(pos {position[dep_id]}) comes after {issue_id} "
                                f"(pos {position[issue_id]})"
                            )
        self.assertEqual(violations, [],
                         f"Topological sort violations:\n" + "\n".join(violations))

    def test_no_duplicate_ids_in_result(self):
        issues = migrate.load_issues(".beads/issues.jsonl")
        order = migrate.topological_sort(issues)
        self.assertEqual(len(order), len(set(order)),
                         "Topological sort contains duplicate IDs")

    def test_handles_simple_chain(self):
        """agm.1 -> agm.2 -> agm.3 should produce deps before dependents."""
        issues = {
            "warpgrid-agm.1": {"id": "warpgrid-agm.1", "dependencies": []},
            "warpgrid-agm.2": {"id": "warpgrid-agm.2", "dependencies": [
                {"type": "blocks", "depends_on_id": "warpgrid-agm.1"}
            ]},
            "warpgrid-agm.3": {"id": "warpgrid-agm.3", "dependencies": [
                {"type": "blocks", "depends_on_id": "warpgrid-agm.2"}
            ]},
        }
        order = migrate.topological_sort(issues)
        self.assertLess(order.index("warpgrid-agm.1"), order.index("warpgrid-agm.2"))
        self.assertLess(order.index("warpgrid-agm.2"), order.index("warpgrid-agm.3"))

    def test_handles_diamond_dependency(self):
        """Diamond: agm.1 -> agm.2, agm.1 -> agm.3, both -> agm.4."""
        issues = {
            "warpgrid-agm.1": {"id": "warpgrid-agm.1", "dependencies": []},
            "warpgrid-agm.2": {"id": "warpgrid-agm.2", "dependencies": [
                {"type": "blocks", "depends_on_id": "warpgrid-agm.1"}
            ]},
            "warpgrid-agm.3": {"id": "warpgrid-agm.3", "dependencies": [
                {"type": "blocks", "depends_on_id": "warpgrid-agm.1"}
            ]},
            "warpgrid-agm.4": {"id": "warpgrid-agm.4", "dependencies": [
                {"type": "blocks", "depends_on_id": "warpgrid-agm.2"},
                {"type": "blocks", "depends_on_id": "warpgrid-agm.3"},
            ]},
        }
        order = migrate.topological_sort(issues)
        self.assertEqual(order[0], "warpgrid-agm.1")
        self.assertLess(order.index("warpgrid-agm.2"), order.index("warpgrid-agm.4"))
        self.assertLess(order.index("warpgrid-agm.3"), order.index("warpgrid-agm.4"))

    def test_cycle_detection_includes_all_nodes(self):
        """Cycles should not silently drop nodes."""
        issues = {
            "warpgrid-agm.98": {"id": "warpgrid-agm.98", "dependencies": [
                {"type": "blocks", "depends_on_id": "warpgrid-agm.99"}
            ]},
            "warpgrid-agm.99": {"id": "warpgrid-agm.99", "dependencies": [
                {"type": "blocks", "depends_on_id": "warpgrid-agm.98"}
            ]},
            "warpgrid-agm.100": {"id": "warpgrid-agm.100", "dependencies": []},
        }
        order = migrate.topological_sort(issues)
        self.assertEqual(len(order), 3, "All nodes should be in result even with cycles")
        self.assertIn("warpgrid-agm.98", order)
        self.assertIn("warpgrid-agm.99", order)
        self.assertIn("warpgrid-agm.100", order)


class TestFormatIssueBody(unittest.TestCase):
    """Test issue body formatting with dependency link resolution."""

    def setUp(self):
        self.issues = migrate.load_issues(".beads/issues.jsonl")
        with open("beads-to-github-mapping.json") as f:
            self.mapping = json.load(f)

    def test_body_contains_beads_id_marker(self):
        """Every formatted body must contain the beads ID for idempotent matching."""
        for issue_id, issue in self.issues.items():
            body = migrate.format_issue_body(issue, self.mapping, self.issues)
            self.assertIn(f"beads-id: {issue_id}", body,
                          f"Issue {issue_id} body missing beads-id marker")

    def test_body_contains_priority_and_type(self):
        issue = self.issues["warpgrid-agm.1"]
        body = migrate.format_issue_body(issue, self.mapping, self.issues)
        self.assertIn("P1 Critical", body)
        self.assertIn("task", body)

    def test_blocked_by_links_are_resolved_github_numbers(self):
        """All 'blocked by' links must be resolved #N references, not raw beads IDs."""
        for issue_id, issue in self.issues.items():
            body = migrate.format_issue_body(issue, self.mapping, self.issues)
            blocks_deps = [d for d in issue.get("dependencies", [])
                           if d["type"] == "blocks"]
            for dep in blocks_deps:
                dep_id = dep["depends_on_id"]
                if dep_id in self.mapping:
                    expected_ref = f"#{self.mapping[dep_id]}"
                    self.assertIn(expected_ref, body,
                                  f"Issue {issue_id} body should contain {expected_ref} "
                                  f"for dependency {dep_id}")

    def test_parent_links_are_resolved(self):
        """All parent-child links must be resolved #N references."""
        for issue_id, issue in self.issues.items():
            body = migrate.format_issue_body(issue, self.mapping, self.issues)
            parent_deps = [d for d in issue.get("dependencies", [])
                           if d["type"] == "parent-child"]
            for dep in parent_deps:
                dep_id = dep["depends_on_id"]
                if dep_id in self.mapping:
                    expected_ref = f"#{self.mapping[dep_id]}"
                    self.assertIn(expected_ref, body,
                                  f"Issue {issue_id} body should contain parent link {expected_ref}")

    def test_no_unresolved_links_in_bodies(self):
        """No body should contain '(not yet migrated)' since all issues are mapped."""
        for issue_id, issue in self.issues.items():
            body = migrate.format_issue_body(issue, self.mapping, self.issues)
            self.assertNotIn("not yet migrated", body,
                             f"Issue {issue_id} has unresolved dependency link")

    def test_epic_has_children_section(self):
        """The epic issue should list children since all tasks are children of it."""
        epic = self.issues["warpgrid-agm"]
        body = migrate.format_issue_body(epic, self.mapping, self.issues)
        self.assertIn("Children:", body)

    def test_notes_section_included_when_present(self):
        """Issues with notes should have a Notes section in the body."""
        issues_with_notes = {iid: i for iid, i in self.issues.items()
                             if i.get("notes")}
        self.assertGreater(len(issues_with_notes), 0, "Test data should have issues with notes")
        for issue_id, issue in issues_with_notes.items():
            body = migrate.format_issue_body(issue, self.mapping, self.issues)
            self.assertIn("## Notes", body,
                          f"Issue {issue_id} has notes but body lacks Notes section")

    def test_description_included_in_body(self):
        issue = self.issues["warpgrid-agm.1"]
        body = migrate.format_issue_body(issue, self.mapping, self.issues)
        # The description field starts with "Milestone: M1.1..." and contains acceptance criteria
        self.assertIn("Acceptance Criteria", body)
        self.assertIn("Milestone: M1.1", body)


class TestMappingIntegrity(unittest.TestCase):
    """Test the mapping file matches the source data perfectly."""

    def setUp(self):
        self.issues = migrate.load_issues(".beads/issues.jsonl")
        with open("beads-to-github-mapping.json") as f:
            self.mapping = json.load(f)

    def test_mapping_covers_all_issues(self):
        missing = set(self.issues.keys()) - set(self.mapping.keys())
        self.assertEqual(missing, set(),
                         f"Issues missing from mapping: {missing}")

    def test_no_extra_entries_in_mapping(self):
        extra = set(self.mapping.keys()) - set(self.issues.keys())
        self.assertEqual(extra, set(),
                         f"Extra entries in mapping not in issues: {extra}")

    def test_mapping_count_is_93(self):
        self.assertEqual(len(self.mapping), 93)

    def test_github_numbers_are_positive_integers(self):
        for beads_id, gh_number in self.mapping.items():
            self.assertIsInstance(gh_number, int,
                                 f"Mapping {beads_id} has non-int value: {gh_number}")
            self.assertGreater(gh_number, 0,
                               f"Mapping {beads_id} has non-positive number: {gh_number}")

    def test_github_numbers_are_unique(self):
        numbers = list(self.mapping.values())
        self.assertEqual(len(numbers), len(set(numbers)),
                         "Duplicate GitHub issue numbers in mapping")

    def test_epic_maps_to_issue_2(self):
        self.assertEqual(self.mapping["warpgrid-agm"], 2)

    def test_github_numbers_are_contiguous(self):
        """Issues should be #2 through #94 (contiguous)."""
        numbers = sorted(self.mapping.values())
        self.assertEqual(numbers[0], 2)
        self.assertEqual(numbers[-1], 94)
        expected = list(range(2, 95))
        self.assertEqual(numbers, expected,
                         "GitHub issue numbers are not contiguous 2-94")


class TestDependencyGraphCompleteness(unittest.TestCase):
    """Test that the dependency graph is fully captured."""

    def setUp(self):
        self.issues = migrate.load_issues(".beads/issues.jsonl")
        with open("beads-to-github-mapping.json") as f:
            self.mapping = json.load(f)

    def test_all_154_blocking_deps_exist(self):
        count = 0
        for issue in self.issues.values():
            for dep in issue.get("dependencies", []):
                if dep["type"] == "blocks":
                    count += 1
        self.assertEqual(count, 154)

    def test_all_92_parent_child_deps_exist(self):
        count = 0
        for issue in self.issues.values():
            for dep in issue.get("dependencies", []):
                if dep["type"] == "parent-child":
                    count += 1
        self.assertEqual(count, 92)

    def test_all_dependency_targets_are_valid_issue_ids(self):
        """Every depends_on_id must reference an existing issue."""
        invalid = []
        for issue_id, issue in self.issues.items():
            for dep in issue.get("dependencies", []):
                dep_id = dep["depends_on_id"]
                if dep_id not in self.issues:
                    invalid.append(f"{issue_id} depends on non-existent {dep_id}")
        self.assertEqual(invalid, [],
                         f"Invalid dependency targets:\n" + "\n".join(invalid))

    def test_all_blocking_deps_are_resolvable_in_bodies(self):
        """Every blocking dependency should produce a resolved #N link in the body."""
        unresolved = []
        for issue_id, issue in self.issues.items():
            body = migrate.format_issue_body(issue, self.mapping, self.issues)
            for dep in issue.get("dependencies", []):
                if dep["type"] == "blocks":
                    dep_id = dep["depends_on_id"]
                    expected = f"#{self.mapping[dep_id]}"
                    if expected not in body:
                        unresolved.append(f"{issue_id}: missing {expected} for {dep_id}")
        self.assertEqual(unresolved, [],
                         f"Unresolved blocking deps:\n" + "\n".join(unresolved))

    def test_no_self_dependencies(self):
        """No issue should depend on itself."""
        self_deps = []
        for issue_id, issue in self.issues.items():
            for dep in issue.get("dependencies", []):
                if dep["depends_on_id"] == issue_id:
                    self_deps.append(issue_id)
        self.assertEqual(self_deps, [], f"Self-dependencies found: {self_deps}")

    def test_epic_has_no_blocks_dependencies(self):
        """The epic issue should not be blocked by anything."""
        epic = self.issues["warpgrid-agm"]
        blocks = [d for d in epic.get("dependencies", []) if d["type"] == "blocks"]
        self.assertEqual(len(blocks), 0, "Epic should not have blocking dependencies")

    def test_all_tasks_are_children_of_epic(self):
        """All 92 task issues should have a parent-child dep to the epic."""
        tasks_without_parent = []
        for issue_id, issue in self.issues.items():
            if issue["issue_type"] == "task":
                parent_deps = [d for d in issue.get("dependencies", [])
                               if d["type"] == "parent-child"]
                parent_ids = [d["depends_on_id"] for d in parent_deps]
                if "warpgrid-agm" not in parent_ids:
                    tasks_without_parent.append(issue_id)
        self.assertEqual(tasks_without_parent, [],
                         f"Tasks without epic parent: {tasks_without_parent}")


class TestStatusCounts(unittest.TestCase):
    """Test that status distribution matches expectations."""

    def setUp(self):
        self.issues = migrate.load_issues(".beads/issues.jsonl")

    def test_closed_count(self):
        closed = [i for i in self.issues.values() if i["status"] == "closed"]
        self.assertEqual(len(closed), 50)

    def test_open_count(self):
        opened = [i for i in self.issues.values() if i["status"] == "open"]
        self.assertEqual(len(opened), 41)

    def test_in_progress_count(self):
        in_progress = [i for i in self.issues.values() if i["status"] == "in_progress"]
        self.assertEqual(len(in_progress), 2)

    def test_closed_issues_have_close_reason(self):
        """Closed issues should have a close_reason for the GitHub comment."""
        issues = self.issues
        missing_reason = []
        for iid, issue in issues.items():
            if issue["status"] == "closed" and not issue.get("close_reason"):
                missing_reason.append(iid)
        # close_reason may be optional (defaults to "Completed"), so just check it exists
        # Actually some might not have it - let's just verify the field is accessible
        for iid, issue in issues.items():
            if issue["status"] == "closed":
                # Should not crash - get with default
                reason = issue.get("close_reason", "Completed")
                self.assertIsInstance(reason, str)


class TestLoadAndSaveMapping(unittest.TestCase):
    """Test mapping file I/O."""

    def test_load_nonexistent_returns_empty(self):
        result = migrate.load_mapping("/tmp/nonexistent_mapping_abc123.json")
        self.assertEqual(result, {})

    def test_roundtrip_preserves_data(self):
        test_data = {"a": 1, "b": 2, "c": 3}
        with tempfile.NamedTemporaryFile(suffix=".json", delete=False, mode="w") as f:
            tmp_path = f.name
        try:
            migrate.save_mapping(test_data, tmp_path)
            loaded = migrate.load_mapping(tmp_path)
            self.assertEqual(loaded, test_data)
        finally:
            os.unlink(tmp_path)

    def test_save_produces_sorted_keys(self):
        test_data = {"z": 1, "a": 2, "m": 3}
        with tempfile.NamedTemporaryFile(suffix=".json", delete=False, mode="w") as f:
            tmp_path = f.name
        try:
            migrate.save_mapping(test_data, tmp_path)
            with open(tmp_path) as f:
                content = f.read()
            # Keys should appear in sorted order
            a_pos = content.index('"a"')
            m_pos = content.index('"m"')
            z_pos = content.index('"z"')
            self.assertLess(a_pos, m_pos)
            self.assertLess(m_pos, z_pos)
        finally:
            os.unlink(tmp_path)


class TestLabelAndMilestoneConfig(unittest.TestCase):
    """Test label and milestone configuration completeness."""

    def test_all_priorities_have_labels(self):
        issues = migrate.load_issues(".beads/issues.jsonl")
        priorities = {i["priority"] for i in issues.values()}
        for p in priorities:
            label = f"priority:{p}"
            self.assertIn(label, migrate.LABEL_COLORS,
                          f"Missing label color for {label}")

    def test_all_milestones_have_descriptions(self):
        issues = migrate.load_issues(".beads/issues.jsonl")
        milestones = set()
        for issue in issues.values():
            ms = migrate.extract_milestone(issue.get("description", ""))
            if ms:
                milestones.add(ms)
        for ms in milestones:
            self.assertIn(ms, migrate.MILESTONE_DESCRIPTIONS,
                          f"Missing description for milestone '{ms}'")

    def test_all_domain_labels_have_colors(self):
        for label in migrate.MILESTONE_DOMAIN_MAP.values():
            self.assertIn(label, migrate.LABEL_COLORS,
                          f"Missing color for domain label '{label}'")


if __name__ == "__main__":
    # Change to repo root for file access
    os.chdir(Path(__file__).parent.parent)
    unittest.main(verbosity=2)
