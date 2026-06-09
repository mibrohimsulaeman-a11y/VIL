#!/usr/bin/env bash
# Native dependency preflight for VIL full-feature builds.
set -euo pipefail

STATUS=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL: $*"; STATUS=1; }
pass() { say "PASS: $*"; }
warn() { say "WARN: $*"; }

say "==> VIL native toolchain preflight"

if command -v cmake >/dev/null 2>&1; then
  pass "cmake: $(command -v cmake) ($(cmake --version | head -1))"
else
  fail "cmake missing. Install cmake or add it to PATH; required by native deps such as rdkafka-sys."
fi

if command -v cc >/dev/null 2>&1; then
  pass "C compiler: $(command -v cc)"
else
  fail "C compiler missing. Install build-essential/gcc/clang."
fi

if command -v c++ >/dev/null 2>&1; then
  pass "C++ compiler: $(command -v c++)"
else
  fail "C++ compiler missing. Install build-essential/g++/clang++."
fi

if command -v pkg-config >/dev/null 2>&1; then
  pass "pkg-config: $(command -v pkg-config)"
else
  fail "pkg-config missing. Install pkg-config so native libraries can be discovered."
fi

if pkg-config --exists libcurl 2>/dev/null; then
  pass "pkg-config libcurl: $(pkg-config --modversion libcurl)"
else
  fail "pkg-config cannot find libcurl. Install libcurl development package, e.g. libcurl4-openssl-dev."
fi

if [ -f /usr/include/x86_64-linux-gnu/curl/curl.h ] || [ -f /usr/include/curl/curl.h ]; then
  pass "curl header: OK"
else
  fail "curl/curl.h missing. Install libcurl development headers."
fi

if pkg-config --exists openssl 2>/dev/null; then
  pass "pkg-config openssl: $(pkg-config --modversion openssl)"
elif [ -f /usr/include/openssl/ssl.h ]; then
  pass "OpenSSL header: OK"
else
  warn "OpenSSL pkg-config/header not detected. Some all-feature native deps may use vendored TLS, but distro OpenSSL headers are recommended."
fi

if [ "$STATUS" -ne 0 ]; then
  say "RESULT: FAIL — native toolchain incomplete for cargo build -p vil_vwfd --features all."
  exit 1
fi

say "RESULT: PASS — native toolchain preflight satisfied."
