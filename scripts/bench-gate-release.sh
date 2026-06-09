#!/usr/bin/env bash
# Release-grade benchmark gate wrapper.
# Warns about noisy host conditions, then delegates to scripts/bench-gate.sh.
# It never converts a failed benchmark into a pass.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

WARNINGS=0
warn() { printf 'WARN: %s
' "$*"; WARNINGS=$((WARNINGS + 1)); }

if [ -r /proc/loadavg ]; then
  LOAD1="$(awk '{print $1}' /proc/loadavg)"
  NPROC="$(nproc 2>/dev/null || echo 1)"
  LOAD_WARN="$(python3 - "$LOAD1" "$NPROC" <<'PYCHECK'
import sys
load = float(sys.argv[1])
nproc = max(int(sys.argv[2]), 1)
if load > nproc * 0.50:
    print(f"high loadavg_1m={load:.2f} nproc={nproc}; use an isolated runner for release signoff")
PYCHECK
)"
  if [ -n "$LOAD_WARN" ]; then warn "$LOAD_WARN"; fi
fi

GOV=/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor
if [ -r "$GOV" ]; then
  GOV_VALUE="$(cat "$GOV")"
  if [ "$GOV_VALUE" = "powersave" ]; then
    warn "cpu0 governor is powersave; prefer performance governor on release runner"
  fi
else
  warn "cpu governor unavailable; archive bench env diagnostics for review"
fi

if [ "$WARNINGS" -gt 0 ]; then
  printf 'Release benchmark preflight emitted %s warning(s). Continuing with transparent gate evidence.
' "$WARNINGS"
fi

export BENCH_GATE_RETRIES="${BENCH_GATE_RETRIES:-2}"
bash scripts/bench-gate.sh
