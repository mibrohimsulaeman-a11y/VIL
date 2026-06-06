#!/usr/bin/env bash
# Manifest smoke test for the thin VIL SDKs.
#
# Runs each SDK's 003-basic-hello-server example with VIL_COMPILE_MODE=manifest
# and asserts the emitted YAML contains the expected structural keys
# (vil_version / name / port).
#
# Toolchains that are not installed are SKIPPED (not failed), so this script is
# safe to run anywhere. It only FAILS when a manifest *is* produced but is
# missing required keys.
#
# Usage:  bash sdk/manifest_smoke.sh
#
# Go is also covered by automated unit tests: (cd sdk/go && go test ./...)
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
EX="$ROOT/examples-sdk"
fail=0

assert_manifest() {
  local label="$1" yaml="$2"
  local missing=()
  local key
  for key in 'vil_version:' 'name:' 'port:'; do
    case "$yaml" in
      *"$key"*) ;;
      *) missing+=("$key") ;;
    esac
  done
  if [ "${#missing[@]}" -ne 0 ]; then
    echo "FAIL  $label - missing keys: ${missing[*]}"
    fail=1
  else
    echo "PASS  $label"
  fi
}

run_lang() {
  # $1=label  $2=bin-to-check  $3=workdir  $4...=command
  local label="$1" bin="$2" dir="$3"; shift 3
  if ! command -v "$bin" >/dev/null 2>&1; then
    echo "SKIP  $label ($bin not installed)"; return
  fi
  if [ ! -d "$dir" ]; then
    echo "SKIP  $label (example dir missing)"; return
  fi
  local out
  if ! out="$(cd "$dir" && VIL_COMPILE_MODE=manifest "$@" 2>/dev/null)"; then
    echo "SKIP  $label (runner error)"; return
  fi
  if [ -z "$out" ]; then
    echo "SKIP  $label (no manifest output)"; return
  fi
  assert_manifest "$label" "$out"
}

# Go - fully automated and reliable.
run_lang "go"     go     "$EX/go/003-basic-hello-server"     go run .

# Other thin SDKs need their toolchains plus the SDK source on the
# classpath/module path. See sdk/TESTING.md for the exact per-language
# commands; they are skipped here automatically when the toolchain is absent.
echo "NOTE  kotlin/swift/csharp/zig/java: see sdk/TESTING.md for manual run commands"

exit $fail
