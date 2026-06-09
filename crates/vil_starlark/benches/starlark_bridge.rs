use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use serde_json::json;
use vil_starlark::{compile_source, eval, BudgetConfig};

const SOURCE: &str = r#"
def run(ctx):
    return {"result": ctx["subtotal"] * 2, "label": ctx["label"]}
"#;

fn bench_starlark_bridge(c: &mut Criterion) {
    let ctx = json!({"subtotal": 50, "label": "bench", "items": [1, 2, 3]});
    let inline_budget = BudgetConfig::from_profile("none");
    let timeout_budget = BudgetConfig::default();

    // Warm the bounded AST cache so execute-only rows do not include parse cost.
    let _ = eval(SOURCE, "run", &ctx, &inline_budget).expect("warm cache");

    let mut group = c.benchmark_group("starlark_bridge");
    group.throughput(Throughput::Elements(1));
    group.sample_size(100);

    group.bench_function("parse_compile", |b| {
        b.iter(|| compile_source(black_box(SOURCE)).expect("compile starlark"));
    });

    group.bench_function("execute_cached_inline", |b| {
        b.iter(|| {
            let out = eval(black_box(SOURCE), "run", black_box(&ctx), &inline_budget)
                .expect("eval inline");
            black_box(out);
        });
    });

    group.bench_function("execute_cached_timeout", |b| {
        b.iter(|| {
            let out = eval(black_box(SOURCE), "run", black_box(&ctx), &timeout_budget)
                .expect("eval timeout");
            black_box(out);
        });
    });

    group.bench_function("json_conversion_nested", |b| {
        let nested = json!({
            "subtotal": 50,
            "label": "bench",
            "body": {"customer": {"id": "c1", "score": 7}},
            "items": [
                {"sku": "A", "qty": 1},
                {"sku": "B", "qty": 2},
                {"sku": "C", "qty": 3}
            ]
        });
        b.iter(|| {
            let out = eval(black_box(SOURCE), "run", black_box(&nested), &inline_budget)
                .expect("eval nested");
            black_box(out);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_starlark_bridge);
criterion_main!(benches);
