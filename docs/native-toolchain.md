# Native toolchain for full-feature VIL builds

Deterministic H5 gates do not require external brokers, databases, cloud storage,
or native adapter toolchains. Full optional builds do require native build tools
because optional dependencies such as Kafka/librdkafka are compiled by crates such
as `rdkafka-sys`.

## Preflight

```bash
bash scripts/check-native-toolchain.sh
```

The preflight checks:

- `cmake`
- C compiler (`cc`)
- C++ compiler (`c++`)
- `pkg-config`
- libcurl via `pkg-config`
- `curl/curl.h`
- OpenSSL pkg-config or headers where available

## Full feature build

```bash
cargo build -p vil_vwfd --features all
```

## CI split

Regular pull-request CI should keep deterministic H5 gates only:

```bash
cargo test -p vil_expr --lib
cargo test -p vil_starlark
cargo test -p vil_vwfd --features compute-starlark
bash scripts/bench-gate.sh
bash scripts/vflow-compat-live-smoke.sh
```

A separate native/full job should install the native packages above and run:

```bash
bash scripts/check-native-toolchain.sh
cargo build -p vil_vwfd --features all
```

External live infrastructure remains opt-in and should use:

```bash
VIL_EXTERNAL_LIVE=1 bash scripts/vflow-compat-external-live.sh
```
