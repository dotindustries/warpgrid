#!/usr/bin/env bash
#
# percentile.sh — Percentile computation helper for bench-harness.
#
# Provides functions:
#   compute_percentile <percentile> <sorted_values...>  — nearest-rank percentile
#   compute_stats <values...>                           — JSON with min/max/mean/p50/p95/p99/count
#
# Uses awk for floating-point arithmetic. Values are expected as
# decimal numbers (e.g., 12.345). Sorting is done internally by compute_stats.
#
# This file is sourced, not executed directly.

# compute_percentile <percentile> <sorted_value_1> <sorted_value_2> ...
#
# Returns the value at the given percentile using nearest-rank method.
# Percentile is 0-100. Values must already be sorted ascending.
compute_percentile() {
  local pct="$1"
  shift
  local -a vals=("$@")
  local n=${#vals[@]}

  if [ "$n" -eq 0 ]; then
    echo "0"
    return
  fi

  if [ "$n" -eq 1 ]; then
    echo "${vals[0]}"
    return
  fi

  # nearest-rank: rank = ceil(pct/100 * n), clamped to [1, n]
  local rank
  rank=$(awk -v p="$pct" -v n="$n" 'BEGIN { r = int(p * n / 100 + 0.999999); if (r < 1) r = 1; if (r > n) r = n; print r }')
  local idx=$((rank - 1))
  echo "${vals[$idx]}"
}

# sort_values <value_1> <value_2> ...
#
# Outputs values sorted numerically ascending, one per line.
sort_values() {
  printf '%s\n' "$@" | sort -g
}

# compute_stats <value_1> <value_2> ...
#
# Returns a JSON object: {"min":..., "max":..., "mean":..., "p50":..., "p95":..., "p99":..., "count":...}
# Values do NOT need to be pre-sorted.
compute_stats() {
  local -a raw_vals=("$@")
  local n=${#raw_vals[@]}

  if [ "$n" -eq 0 ]; then
    echo '{"min":0,"max":0,"mean":0,"p50":0,"p95":0,"p99":0,"count":0}'
    return
  fi

  # Sort values
  local -a sorted
  mapfile -t sorted < <(sort_values "${raw_vals[@]}")

  local min_val="${sorted[0]}"
  local max_val="${sorted[$((n - 1))]}"

  # Compute mean using awk
  local mean
  mean=$(printf '%s\n' "${sorted[@]}" | awk '{ s += $1; c++ } END { printf "%.3f", s/c }')

  local p50 p95 p99
  p50=$(compute_percentile 50 "${sorted[@]}")
  p95=$(compute_percentile 95 "${sorted[@]}")
  p99=$(compute_percentile 99 "${sorted[@]}")

  printf '{"min":%s,"max":%s,"mean":%s,"p50":%s,"p95":%s,"p99":%s,"count":%d}' \
    "$min_val" "$max_val" "$mean" "$p50" "$p95" "$p99" "$n"
}
