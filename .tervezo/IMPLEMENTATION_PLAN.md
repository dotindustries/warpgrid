# Implementation Plan: US-710 — Cross-Domain Performance Baseline

## Task List

- [x] Create `test-infra/bench-harness/lib/percentile.sh` — percentile computation helper (p50/p95/p99, stats)
- [x] Create `test-infra/bench-harness/bench-harness.test.sh` — TDD tests for harness and percentile lib
- [x] Create `test-infra/bench-harness/bench-harness.sh` — main benchmark harness executable
- [x] Create `scripts/compare-perf.sh` — regression detection script (diffs against main baseline)
- [x] Run TDD tests and validate end-to-end with --dry-run

## Notes

- `bc` is not available in the CI environment; all floating-point arithmetic uses `awk` instead
- 63 tests pass (55 unit + 8 integration) covering: file existence, percentile computation, JSON schema, quality gate logic, regression detection, argument parsing, and end-to-end sample generation
