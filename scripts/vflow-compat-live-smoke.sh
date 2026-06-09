#!/usr/bin/env bash
# Deterministic VFlow/VWFD compatibility runtime smokes for H5.
# No external brokers, databases, cloud storage, or industrial devices are used.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

echo "==> H5 runtime parity live-smokes (in-process, no external infra)"
echo "    cargo test -p vil_vwfd --test h5_runtime_parity -- --nocapture"
cargo test -p vil_vwfd --test h5_runtime_parity -- --nocapture

echo
echo "==> H4 reference compile smoke remains covered"
echo "    cargo test -p vil_vwfd compiler::tests::test_h4_reference_examples_compile -- --nocapture"
cargo test -p vil_vwfd compiler::tests::test_h4_reference_examples_compile -- --nocapture

echo
echo "RESULT: PASS — H5 in-process runtime parity smokes completed without external services."
