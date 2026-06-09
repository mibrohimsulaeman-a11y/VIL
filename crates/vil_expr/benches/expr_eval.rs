// expr_eval.rs — vil_expr evaluation hot-path microbenchmarks (criterion).
// Gated by scripts/bench-gate.sh vs benchmarks/baselines/expr_eval.json.
// Phase 2 adds .filter/.map rows once V-CEL list macros land.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use serde_json::{json, Value};
use std::collections::HashMap;
use vil_expr::evaluate;

fn make_vars() -> HashMap<String, Value> {
    let mut v: HashMap<String, Value> = HashMap::new();
    v.insert("a".into(), json!(7));
    v.insert("b".into(), json!(6));
    v.insert("c".into(), json!(5));
    v.insert("id".into(), json!(42));
    v.insert("score".into(), json!(91));
    v.insert("name".into(), json!("Alice"));
    v.insert("items".into(), json!([1, 2, 3, 4, 5]));
    v.insert(
        "user".into(),
        json!({"profile": {"name": "Alice", "age": 30}}),
    );
    v
}

fn bench_expr(c: &mut Criterion) {
    let vars = make_vars();
    let cases: &[(&str, &str)] = &[
        ("field_access", "user.profile.name"),
        ("arithmetic", "a * b + c"),
        ("ternary", r#"score > 80 ? "high" : "low""#),
        ("size", "size(items)"),
        (
            "string_methods",
            r#"name.startsWith("A") && name.contains("li")"#,
        ),
        (
            "json_construction",
            r#"{"id": id, "name": name, "ok": score > 80, "sum": a + b + c}"#,
        ),
        ("filter", "items.filter(i, i > 2)"),
        ("map", "items.map(i, i * 2)"),
        ("transformList", "transformList(items, x, x > 0, x)"),
        ("matches", r#"matches(name, "^A")"#),
        (
            "timestamp_accessor",
            r#"getHours(timestamp("2026-01-02T03:04:05Z"), "Asia/Jakarta")"#,
        ),
    ];

    let mut group = c.benchmark_group("expr_eval");
    group.throughput(Throughput::Elements(1));
    group.sample_size(500);
    for (label, expr) in cases {
        group.bench_function(*label, |b| {
            b.iter(|| {
                let out = evaluate(black_box(expr), black_box(&vars));
                black_box(out.ok());
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_expr);
criterion_main!(benches);
