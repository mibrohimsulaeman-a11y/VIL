// workflow_exec.rs — vil_vwfd compile + execute fastpath microbenchmark (criterion).
// Gated by scripts/bench-gate.sh vs benchmarks/baselines/workflow_exec.json.
// Phase 3 adds a Trigger->Compute->EndTrigger row once Starlark Compute lands.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use serde_json::json;
use vil_vwfd::{compile, execute, ExecConfig};

// Trigger -> Transform(Connector stub) -> EndTrigger. Mirrors the proven executor
// integration test so it always compiles + runs under ExecConfig::default()
// (stub connector mode, no real network).
const FASTPATH_WF: &str = r#"
version: "3.0"
metadata:
  id: bench-fastpath
  name: "Bench Fastpath"
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config:
        trigger_type: webhook
        response_mode: buffered
        end_activity: respond
        webhook_config: { path: /bench }
      output_variable: trigger_payload
    - id: transform
      activity_type: Connector
      connector_config:
        connector_ref: vastar.http
        operation: post
      input_mappings:
        - target: url
          source: { language: literal, source: "http://example.com" }
        - target: body
          source: { language: vil-expr, source: '{"name": trigger_payload.name, "active": true}' }
      output_variable: result
    - id: respond
      activity_type: EndTrigger
      end_trigger_config:
        trigger_ref: trigger
        final_response:
          language: vil-expr
          source: '{"result": result, "input_name": trigger_payload.name}'
    - id: end
      activity_type: End
  flows:
    - { id: f1, from: { node: trigger }, to: { node: transform } }
    - { id: f2, from: { node: transform }, to: { node: respond } }
    - { id: f3, from: { node: respond }, to: { node: end } }
  variables:
    - { name: trigger_payload, type: object }
    - { name: result, type: object }
"#;

#[cfg(feature = "compute-starlark")]
const COMPUTE_WF: &str = r#"
version: "3.0"
metadata:
  id: bench-compute
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config:
        trigger_type: webhook
        response_mode: buffered
        end_activity: respond
        webhook_config: { path: /comp }
      output_variable: trigger_payload
    - id: calc
      activity_type: Compute
      compute_config:
        language: starlark
        entry_fn: run
        source: |
          def run(ctx):
              return {"result": ctx["subtotal"] * 2}
      input_mappings:
        - target: subtotal
          source: { language: spv1, source: "$.trigger_payload.subtotal" }
      output_variable: calc_result
    - id: respond
      activity_type: EndTrigger
      end_trigger_config:
        trigger_ref: trigger
        final_response:
          language: vil-expr
          source: calc_result
    - id: end
      activity_type: End
  flows:
    - { id: f1, from: { node: trigger }, to: { node: calc } }
    - { id: f2, from: { node: calc }, to: { node: respond } }
    - { id: f3, from: { node: respond }, to: { node: end } }
"#;

fn bench_workflow(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("tokio runtime");

    let mut group = c.benchmark_group("workflow_exec");
    group.throughput(Throughput::Elements(1));
    group.sample_size(200);

    group.bench_function("compile_fastpath", |b| {
        b.iter(|| {
            let graph = compile(black_box(FASTPATH_WF)).expect("compile");
            black_box(graph);
        });
    });

    let graph = compile(FASTPATH_WF).expect("compile");
    group.bench_function("exec_fastpath", |b| {
        b.iter(|| {
            let out = rt.block_on(async {
                execute(
                    black_box(&graph),
                    json!({ "name": "Alice" }),
                    &ExecConfig::default(),
                )
                .await
            });
            black_box(out.ok());
        });
    });

    #[cfg(feature = "compute-starlark")]
    {
        let cgraph = compile(COMPUTE_WF).expect("compile compute");
        group.bench_function("exec_compute_starlark", |b| {
            b.iter(|| {
                let out = rt.block_on(async {
                    execute(
                        black_box(&cgraph),
                        json!({"subtotal": 50}),
                        &ExecConfig::default(),
                    )
                    .await
                });
                black_box(out.ok());
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_workflow);
criterion_main!(benches);
