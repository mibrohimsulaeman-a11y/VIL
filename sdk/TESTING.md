# SDK testing — manifest-mode smoke tests

Semua thin SDK (Go, Kotlin, Swift, C#, Zig, Java) mengikuti kontrak yang sama
dengan referensi **Python** dan **TypeScript**: API surface `sink` / `source` /
`route` / `compile` (plus `service` / `endpoint` untuk mode server), dan saat
`VIL_COMPILE_MODE=manifest` di-set, `compile()` mencetak YAML manifest ke stdout
lalu keluar (tidak memanggil `vil compile`). / All thin SDKs share the same
contract as the Python/TypeScript reference and print the YAML manifest to
stdout under `VIL_COMPILE_MODE=manifest`.

Test dasar memverifikasi struktur YAML yang dihasilkan (kunci `vil_version`,
`name`, `port`, `nodes`/`services`, `endpoints`, `routes`).

## Go — otomatis / automated

```bash
cd sdk/go && go test ./...
```

`sdk/go/vil_test.go` menguji field helpers, handler-impl builders, `ModeFromEnv`,
serta struktur manifest untuk `VilPipeline` (sink/source/route) dan `VilServer`
(service/endpoint).

## Smoke harness lintas-SDK / cross-SDK smoke harness

```bash
bash sdk/manifest_smoke.sh
```

Menjalankan contoh `003-basic-hello-server` tiap SDK dengan
`VIL_COMPILE_MODE=manifest` dan meng-assert kunci YAML. Toolchain yang tidak
terpasang otomatis **di-SKIP** (tidak menggagalkan), jadi aman dijalankan di
lingkungan mana pun. / Skips missing toolchains; only fails on a produced-but-
invalid manifest.

## Perintah manual per-bahasa / per-language manual commands

Contoh memakai `003-basic-hello-server`. Jalankan dari root repo.

```bash
# Go
(cd examples-sdk/go/003-basic-hello-server && VIL_COMPILE_MODE=manifest go run .)

# Kotlin (butuh sdk/kotlin/vil.kt di classpath)
VIL_COMPILE_MODE=manifest kotlinc -script examples-sdk/kotlin/003-basic-hello-server/main.kt -cp sdk/kotlin

# Swift (kompilasi SDK + contoh bersama)
VIL_COMPILE_MODE=manifest swift sdk/swift/Vil.swift examples-sdk/swift/003-basic-hello-server/main.swift

# C# (dotnet-script)
VIL_COMPILE_MODE=manifest dotnet script examples-sdk/csharp/003-basic-hello-server/Main.cs

# Zig
VIL_COMPILE_MODE=manifest zig run examples-sdk/zig/003-basic-hello-server/main.zig

# Java (Maven module di sdk/java)
VIL_COMPILE_MODE=manifest java -cp sdk/java/target/classes examples-sdk/java/003-basic-hello-server/Main.java
```

Manifest yang valid minimal memuat:

```yaml
vil_version: "6.0.0"
name: <app-name>
port: <port>
# ... nodes:/services:/endpoints:/routes: sesuai contoh
```
