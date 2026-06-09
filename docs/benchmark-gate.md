# Benchmark gate

The benchmark gate protects VIL's evaluation and workflow hot paths from silent
performance regressions. Every phase of the vflow-dialect work must keep all
gated benches at or below their committed baseline.

## What is gated

| Target | Crate | Bench file | Measures |
| --- | --- | --- | --- |
| `expr_eval` | `vil_expr` | `crates/vil_expr/benches/expr_eval.rs` | VIL Expression eval (field access, arithmetic, ternary, `size()`, string methods, JSON construction) |
| `workflow_exec` | `vil_vwfd` | `crates/vil_vwfd/benches/workflow_exec.rs` | compile + execute a Trigger -> Transform -> EndTrigger fastpath workflow |

Baselines (median ns/op per benched function) live in
`benchmarks/baselines/<target>.json` and are committed to the repo.

## Run locally

```bash
bash scripts/bench-gate.sh
python3 -m json.tool target/bench-gate-summary.json >/dev/null
```

This runs `cargo bench` for each target, parses criterion's
`target/criterion/<group>/<function>/new/estimates.json`, and compares the
median against the baseline. It prints a markdown summary, writes a machine-readable
JSON summary to `target/bench-gate-summary.json` by default, writes benchmark
environment diagnostics to `target/bench-gate-env.txt`, and exits non-zero if any
function regressed beyond the threshold.

### Threshold

Default regression threshold is **5%**. Override per run:

```bash
BENCH_GATE_THRESHOLD=10 bash scripts/bench-gate.sh
```

A function fails the gate when its current median is more than `THRESHOLD%`
slower than the baseline. Improvements (faster) never fail. Raw Criterion
"Performance has regressed" messages compare against local previous samples;
the authoritative result is the final `RESULT: PASS/FAIL` line and the JSON
summary status.

### Environment diagnostics

Every run writes:

```bash
target/bench-gate-env.txt
```

The diagnostics include timestamp, kernel/CPU metadata, Rust/Cargo versions,
`/proc/loadavg`, CPU governor/frequency when available, and a top CPU consumers
snapshot. The JSON summary includes `env_path` so a failed benchmark can be
correlated with host load. Diagnostics never turn a fail into a pass.


### Retry policy for noisy local machines

Default behavior remains one benchmark attempt:

```bash
bash scripts/bench-gate.sh
```

For local diagnosis on noisy machines, use strict retry mode:

```bash
BENCH_GATE_RETRIES=2 bash scripts/bench-gate.sh
```

`BENCH_GATE_RETRIES=2` runs three full attempts. The final verdict uses
`median_of_attempts`; every attempt remains recorded in
`target/bench-gate-summary.json` with per-attempt deltas and statuses. This is
not "rerun until green": a row only passes if its median attempt value is within
the threshold. CI/release runners should prefer isolation over retries.

## Baselines and re-baselining

The committed baselines start as a bootstrap sentinel:

```json
{ "__bootstrap__": true }
```

A baseline that is missing or still holds `__bootstrap__: true` is **seeded** on
the next run: the script writes the current measurements as the new baseline and
passes. This lets the gate self-seed real numbers in CI on first run without
hand-checking machine-specific timings into the repo.

To **intentionally re-baseline** after a deliberate, reviewed performance change:

```bash
UPDATE_BASELINE=1 bash scripts/bench-gate.sh
```

then commit the updated `benchmarks/baselines/*.json`. Re-baselining must be a
deliberate, reviewed commit — never bundle it with an unrelated change.

New benched functions (e.g. the Phase 2 `.filter`/`.map` rows or the Phase 3
`Trigger -> Compute -> EndTrigger` row) that are not yet in the baseline are
reported as `new, ungated`.

A new row becomes gated only after a deliberate baseline update. The expected
process is:

1. Run `bash scripts/bench-gate.sh` at least twice on a warm tree.
2. Confirm existing gated rows remain `OK`; do not mask regressions by changing
   old rows.
3. Add only the new rows to `benchmarks/baselines/<target>.json` with the chosen
   warm-tree median values.
4. Run `bash scripts/bench-gate.sh` again and verify the new rows are reported as
   compared `OK`, not `new, ungated`.

H5 used this process to gate `filter`, `map`, `matches`, `timestamp_accessor`,
`transformList`, and `exec_compute_starlark` without increasing the 5% threshold.
For noisy new rows, H5 deliberately chose the highest median observed across the
warm verification runs and did not alter older H0 baselines.

## CI

`.github/workflows/bench-gate.yml` runs the gate on every pull request. The job
installs `protobuf-compiler`/`cmake`, caches cargo with `Swatinem/rust-cache@v2`,
and runs `scripts/bench-gate.sh`.

Recommended benchmark CI/release shape:

- run on an isolated runner with no concurrent builds/tests;
- prefer a performance CPU governor where the host permits it;
- archive `target/bench-gate-env.txt` and `target/bench-gate-summary.json` as CI artifacts;
- keep deterministic unit/integration gates separate from benchmark gates;
- use retry mode for local diagnosis, but release-grade signoff should use an isolated runner.


### Release helper

For release-candidate signoff, use:

```bash
bash scripts/bench-gate-release.sh
python3 -m json.tool target/bench-gate-summary.json >/dev/null
test -s target/bench-gate-env.txt
```

The helper prints host-load and CPU-governor warnings, defaults to transparent retry evidence, and delegates final pass/fail to `scripts/bench-gate.sh`.

## Release sign-off: `vil bench --json`

For throughput sign-off against a running server, `vil bench` accepts `--json`:

```bash
vil run --mock &
vil bench -r 3000 -c 200 --json
```

It emits a single parseable JSON object (requests, concurrency, success rate,
rps, p50/p99/avg/fastest/slowest latency in ms) suitable for recording
native/wasm/sidecar req/s in the release checklist.


## Machine-readable summary

By default the gate writes:

```bash
target/bench-gate-summary.json
```

Override the path with:

```bash
BENCH_GATE_SUMMARY_JSON=/tmp/vil-bench-summary.json bash scripts/bench-gate.sh
```

The JSON contains `status`, `threshold_pct`, `env_path`, `retry_policy`, and
per-target row objects with median/best current values, baseline values,
`delta_pct`, aggregate status, pass-attempt counts, and full per-attempt evidence.
