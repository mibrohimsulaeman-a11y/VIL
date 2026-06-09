#!/usr/bin/env bash
# bench-gate.sh — criterion benchmark regression gate.
# Runs gated bench targets, parses median ns/op from criterion estimates, and
# compares against committed baselines in benchmarks/baselines/<target>.json.
# H7 adds environment diagnostics and optional strict retry policy.
# See docs/benchmark-gate.md.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

THRESHOLD="${BENCH_GATE_THRESHOLD:-5}"
UPDATE_BASELINE="${UPDATE_BASELINE:-0}"
RETRIES="${BENCH_GATE_RETRIES:-0}"
BASELINE_DIR="benchmarks/baselines"
CRITERION_DIR="target/criterion"
SUMMARY_JSON="${BENCH_GATE_SUMMARY_JSON:-target/bench-gate-summary.json}"
ENV_FILE="${BENCH_GATE_ENV_FILE:-target/bench-gate-env.txt}"

TARGETS=(
  "expr_eval:vil_expr:expr_eval:"
  "workflow_exec:vil_vwfd:workflow_exec:compute-starlark"
)

GATED_TARGET_NAMES=()
for spec in "${TARGETS[@]}"; do
  IFS=: read -r gated_name _crate _bench _features <<<"$spec"
  GATED_TARGET_NAMES+=("$gated_name")
done

mkdir -p "$BASELINE_DIR" "$(dirname "$SUMMARY_JSON")" "$(dirname "$ENV_FILE")"

case "$RETRIES" in
  ''|*[!0-9]*)
    echo "BENCH_GATE_RETRIES must be a non-negative integer, got '$RETRIES'" >&2
    exit 2
    ;;
esac
ATTEMPTS=$((RETRIES + 1))

write_env_diagnostics() {
  {
    echo "timestamp_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "workdir: $ROOT"
    echo "uname: $(uname -a)"
    echo "rustc: $(rustc --version 2>/dev/null || echo missing)"
    echo "cargo: $(cargo --version 2>/dev/null || echo missing)"
    echo "nproc: $(nproc 2>/dev/null || echo unknown)"
    if [ -r /proc/loadavg ]; then echo "loadavg: $(cat /proc/loadavg)"; fi
    if [ -r /proc/cpuinfo ]; then
      awk -F: '/model name/ {gsub(/^ +/, "", $2); print "cpu_model: "$2; exit}' /proc/cpuinfo
    fi
    GOV=/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor
    FREQ=/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq
    if [ -r "$GOV" ]; then echo "cpu0_governor: $(cat "$GOV")"; else echo "cpu0_governor: unavailable"; fi
    if [ -r "$FREQ" ]; then echo "cpu0_cur_freq_khz: $(cat "$FREQ")"; else echo "cpu0_cur_freq_khz: unavailable"; fi
    echo "top_cpu_consumers:"
    ps -eo pid,comm,pcpu,pmem --sort=-pcpu | head -12 | sed 's/^/  /'
  } > "$ENV_FILE"
}

collect_attempt_json() {
  local attempt="$1"
  local out_json="$2"
  python3 - "$attempt" "$CRITERION_DIR" "$out_json" "${GATED_TARGET_NAMES[@]}" <<'COLLECTPY'
import glob, json, os, sys
attempt, crit_root, out_path, *gated_names = sys.argv[1:]
targets = []
if os.path.isdir(crit_root):
    for name in gated_names:
        crit_dir = os.path.join(crit_root, name)
        if not os.path.isdir(crit_dir):
            continue
        rows = {}
        for est in glob.glob(os.path.join(crit_dir, "*", "new", "estimates.json")):
            func = os.path.basename(os.path.dirname(os.path.dirname(est)))
            with open(est) as fh:
                data = json.load(fh)
            point = (data.get("median") or data.get("mean") or {}).get("point_estimate")
            if point is not None:
                rows[func] = round(float(point), 2)
        if rows:
            targets.append({"name": name, "rows": dict(sorted(rows.items()))})
with open(out_path, "w") as fh:
    json.dump({"attempt": int(attempt), "targets": targets}, fh, indent=2)
    fh.write("\n")
COLLECTPY
}

run_bench_targets() {
  echo "==> Running criterion benches"
  for spec in "${TARGETS[@]}"; do
    IFS=: read -r name crate bench features <<<"$spec"
    feat_args=""
    if [ -n "$features" ]; then
      feat_args="--features $features"
    fi
    echo "    cargo bench -p ${crate} --bench ${bench} ${feat_args}"
    cargo bench -p "$crate" --bench "$bench" $feat_args
  done
}

write_env_diagnostics
echo "Benchmark env diagnostics: $ENV_FILE"

ATTEMPT_FILES=()
for attempt in $(seq 1 "$ATTEMPTS"); do
  if [ "$ATTEMPTS" -gt 1 ]; then
    echo
    echo "==> Benchmark attempt ${attempt}/${ATTEMPTS}"
  fi
  run_bench_targets
  attempt_json="target/bench-gate-attempt-${attempt}.json"
  collect_attempt_json "$attempt" "$attempt_json"
  ATTEMPT_FILES+=("$attempt_json")
done

echo
echo "## Benchmark gate summary"
python3 - "$BASELINE_DIR" "$THRESHOLD" "$UPDATE_BASELINE" "$SUMMARY_JSON" "$ENV_FILE" "$RETRIES" "${ATTEMPT_FILES[@]}" <<'FINALPY'
import json, os, statistics, sys
baseline_dir, threshold, update, summary_path, env_path, retries, *attempt_files = sys.argv[1:]
threshold = float(threshold)
update = update == "1"
retries = int(retries)

attempts = []
for path in attempt_files:
    with open(path) as fh:
        attempts.append(json.load(fh))

measurements = {}
for attempt in attempts:
    attempt_no = attempt["attempt"]
    for target in attempt.get("targets", []):
        t = target["name"]
        for row, value in target.get("rows", {}).items():
            measurements.setdefault(t, {}).setdefault(row, []).append((attempt_no, float(value)))

if not measurements:
    summary = {
        "status": "FAIL",
        "threshold_pct": threshold,
        "env_path": env_path,
        "retry_policy": {"retries": retries, "attempts": len(attempts), "policy": "median_of_attempts"},
        "targets": [],
        "error": "no criterion measurements found",
    }
    with open(summary_path, "w") as fh:
        json.dump(summary, fh, indent=2)
        fh.write("\n")
    print("ERROR no criterion measurements found")
    print("JSON summary: " + summary_path)
    sys.exit(1)

summary_targets = []
overall_status = "PASS"

for target_name in sorted(measurements):
    baseline_path = os.path.join(baseline_dir, target_name + ".json")
    baseline = None
    if os.path.exists(baseline_path):
        try:
            with open(baseline_path) as fh:
                baseline = json.load(fh)
        except Exception:
            baseline = None
    is_bootstrap = baseline is None or (isinstance(baseline, dict) and baseline.get("__bootstrap__")) or update

    if is_bootstrap:
        seeded = {}
        for row, vals in measurements[target_name].items():
            seeded[row] = round(statistics.median(v for _, v in vals), 2)
        with open(baseline_path, "w") as fh:
            json.dump(dict(sorted(seeded.items())), fh, indent=2)
            fh.write("\n")
        reason = "UPDATE_BASELINE" if update else "bootstrap seed"
        print(f"- **{target_name}**: SEEDED ({reason}), {len(seeded)} rows")
        rows = [
            {
                "name": row,
                "status": "SEEDED",
                "baseline_ns": value,
                "median_ns": value,
                "best_ns": value,
                "attempts": [],
            }
            for row, value in sorted(seeded.items())
        ]
        summary_targets.append({"name": target_name, "status": "PASS", "rows": rows})
        continue

    print(f"- **{target_name}** (threshold {threshold:.1f}%, attempts {len(attempts)}, policy median_of_attempts):")
    target_status = "PASS"
    rows = []
    for row in sorted(measurements[target_name]):
        vals = measurements[target_name][row]
        values = [v for _, v in vals]
        median = statistics.median(values)
        best = min(values)
        base = baseline.get(row) if isinstance(baseline, dict) else None
        attempt_details = []
        pass_attempts = 0
        if base is None:
            status = "NEW_UNGATED"
            delta = None
            print(f"    - {row}: {median:.1f} ns median ({len(values)} attempts, new, ungated)")
            for attempt_no, value in vals:
                attempt_details.append({
                    "attempt": attempt_no,
                    "current_ns": round(value, 2),
                    "delta_pct": None,
                    "status": "NEW_UNGATED",
                })
        else:
            base = float(base)
            for attempt_no, value in vals:
                attempt_delta = ((value - base) / base * 100.0) if base else 0.0
                attempt_status = "REGRESSED" if attempt_delta > threshold else "OK"
                if attempt_status == "OK":
                    pass_attempts += 1
                attempt_details.append({
                    "attempt": attempt_no,
                    "current_ns": round(value, 2),
                    "delta_pct": round(attempt_delta, 2),
                    "status": attempt_status,
                })
            delta = ((median - base) / base * 100.0) if base else 0.0
            status = "REGRESSED" if delta > threshold else "OK"
            if status == "REGRESSED":
                target_status = "FAIL"
                overall_status = "FAIL"
            print(
                f"    - {row}: median {median:.1f} ns vs {base:.1f} ns "
                f"({delta:+.1f}%) {status}; best {best:.1f} ns; "
                f"pass_attempts {pass_attempts}/{len(values)}"
            )
        rows.append({
            "name": row,
            "status": status,
            "baseline_ns": None if base is None else round(float(base), 2),
            "median_ns": round(median, 2),
            "best_ns": round(best, 2),
            "delta_pct": None if delta is None else round(delta, 2),
            "pass_attempts": pass_attempts,
            "attempt_count": len(values),
            "attempts": attempt_details,
        })
    summary_targets.append({"name": target_name, "status": target_status, "rows": rows})

summary = {
    "status": overall_status,
    "threshold_pct": threshold,
    "env_path": env_path,
    "retry_policy": {
        "retries": retries,
        "attempts": len(attempts),
        "policy": "median_of_attempts",
        "default_attempts_without_retries": 1,
    },
    "targets": summary_targets,
}
with open(summary_path, "w") as fh:
    json.dump(summary, fh, indent=2)
    fh.write("\n")

print("JSON summary: " + summary_path)
print()
if overall_status != "PASS":
    print(f"RESULT: FAIL — median-of-attempts benchmark gate regressed beyond {threshold:.1f}%.")
    sys.exit(1)
print("RESULT: PASS")
FINALPY
