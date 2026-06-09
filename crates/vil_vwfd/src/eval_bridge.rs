//! Eval bridge — map VWFD language tags to appropriate evaluator.

use crate::graph::CompiledMapping;
use serde_json::Value;
use std::collections::HashMap;

/// Evaluate a compiled mapping against variable store.
pub fn eval_mapping(
    mapping: &CompiledMapping,
    vars: &HashMap<String, Value>,
) -> Result<Value, String> {
    match mapping.language.as_str() {
        "literal" => {
            // Try parse as JSON first, fallback to string
            Ok(serde_json::from_str::<Value>(&mapping.source)
                .unwrap_or_else(|_| Value::String(mapping.source.clone())))
        }

        "spv1" => {
            let result = crate::spv1::eval_template(&mapping.source, vars);
            // Try parse result as JSON
            Ok(serde_json::from_str(&result).unwrap_or(Value::String(result)))
        }

        "vil-expr" | "cel" | "v-cel" | "vcel" => {
            // H2: v-cel/vcel now evaluate through the real vil_expr engine.
            vil_expr::evaluate(&mapping.source, vars)
        }

        "vil_query" => {
            // Pre-compiled SQL — resolve param_refs at runtime. If the compiler
            // emitted a where_eq_if alternate plan, select the alternate when the
            // controlling ref resolves to null/empty-string.
            if let Some(ref compiled_sql) = mapping.compiled_sql {
                let mut selected_sql = compiled_sql.clone();
                let mut selected_refs = mapping.param_refs.clone().unwrap_or_default();

                if let Some(ref optional) = mapping.optional {
                    let if_param_ref = optional.get("if_param_ref").and_then(|v| v.as_str());
                    let alt_sql = optional.get("alt_sql").and_then(|v| v.as_str());
                    let alt_refs = optional.get("alt_param_refs").and_then(|v| v.as_array());
                    if let (Some(if_ref), Some(alt_sql), Some(alt_refs)) = (if_param_ref, alt_sql, alt_refs) {
                        let control_value = resolve_param_ref(if_ref, vars);
                        if is_null_or_empty(&control_value) {
                            selected_sql = alt_sql.to_string();
                            selected_refs = alt_refs
                                .iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect();
                        }
                    }
                }

                let params: Vec<Value> = selected_refs
                    .iter()
                    .map(|r| resolve_param_ref(r, vars))
                    .collect();

                Ok(serde_json::json!({
                    "operation": "raw_query",
                    "sql": selected_sql,
                    "params": params,
                    "_vil_query": true,
                    "_compiled_sql": compiled_sql,
                    "_param_refs": mapping.param_refs.clone().unwrap_or_default(),
                    "_optional": mapping.optional.clone(),
                }))
            } else {
                Err("vil_query mapping has no compiled_sql".into())
            }
        }

        // H2: v-cel/vcel are now handled by the "vil-expr" | "cel" arm above.

        "bytes_ref" => {
            // Read raw bytes from a variable handle slot. Optional "$." prefix.
            let path = mapping.source.strip_prefix("$.").unwrap_or(mapping.source.as_str());
            Ok(resolve_param_ref(path, vars))
        }

        "starlark" | "v-starlark" => {
            Err("starlark: build with --features compute-starlark (Phase 3)".to_string())
        }

        other => Err(format!(
            "language '{}' not recognized; supported: literal, spv1, vil-expr/cel, v-cel/vcel, bytes_ref, starlark/v-starlark, vil_query.",
            other
        )),
    }
}

/// Resolve a param_ref — literal or variable path.
fn resolve_param_ref(ref_str: &str, vars: &HashMap<String, Value>) -> Value {
    if let Some(val) = ref_str.strip_prefix("_literal_str:") {
        return Value::String(val.to_string());
    }
    if let Some(val) = ref_str.strip_prefix("_literal_num:") {
        if let Ok(n) = val.parse::<i64>() {
            return Value::Number(n.into());
        }
        if let Ok(n) = val.parse::<f64>() {
            return serde_json::Number::from_f64(n)
                .map(Value::Number)
                .unwrap_or(Value::Null);
        }
        return Value::String(val.to_string());
    }
    if let Some(val) = ref_str.strip_prefix("_literal_bool:") {
        return Value::Bool(val == "true");
    }

    // Variable path: trigger_payload.min_amount
    let parts: Vec<&str> = ref_str.splitn(2, '.').collect();
    if let Some(root) = vars.get(parts[0]) {
        if parts.len() == 1 {
            return root.clone();
        }
        let mut current = root.clone();
        for key in parts[1].split('.') {
            current = match current {
                Value::Object(ref obj) => obj.get(key).cloned().unwrap_or(Value::Null),
                _ => Value::Null,
            };
        }
        current
    } else {
        Value::Null
    }
}

fn is_null_or_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

/// Evaluate all mappings for an activity → produce key-value input.
pub fn eval_all_mappings(
    mappings: &[CompiledMapping],
    vars: &HashMap<String, Value>,
) -> Result<HashMap<String, Value>, String> {
    let mut result = HashMap::new();
    for m in mappings {
        let val = eval_mapping(m, vars)?;
        // For vil_query: flatten into top-level
        if m.language == "vil_query" {
            if let Some(obj) = val.as_object() {
                for (k, v) in obj {
                    result.insert(k.clone(), v.clone());
                }
                continue;
            }
        }
        result.insert(m.target.clone(), val);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_vars() -> HashMap<String, Value> {
        let mut v = HashMap::new();
        v.insert(
            "trigger_payload".into(),
            json!({"name": "Alice", "amount": 100}),
        );
        v.insert("status".into(), json!("active"));
        v
    }

    #[test]
    fn test_literal() {
        let m = CompiledMapping {
            target: "url".into(),
            language: "literal".into(),
            source: "http://example.com".into(),
            compiled_sql: None,
            param_refs: None,
            optional: None,
        };
        let result = eval_mapping(&m, &test_vars()).unwrap();
        assert_eq!(result, json!("http://example.com"));
    }

    #[test]
    fn test_vcel() {
        let m = CompiledMapping {
            target: "body".into(),
            language: "vil-expr".into(),
            source: r#"{"name": trigger_payload.name, "total": trigger_payload.amount}"#.into(),
            compiled_sql: None,
            param_refs: None,
            optional: None,
        };
        let result = eval_mapping(&m, &test_vars()).unwrap();
        assert_eq!(result["name"], "Alice");
        assert_eq!(result["total"], 100);
    }

    #[test]
    fn test_spv1() {
        let m = CompiledMapping {
            target: "greeting".into(),
            language: "spv1".into(),
            source: "Hello $.trigger_payload.name".into(),
            compiled_sql: None,
            param_refs: None,
            optional: None,
        };
        let result = eval_mapping(&m, &test_vars()).unwrap();
        assert_eq!(result, json!("Hello Alice"));
    }

    #[test]
    fn test_vil_query() {
        let m = CompiledMapping {
            target: "query".into(),
            language: "vil_query".into(),
            source: "original DSL".into(),
            compiled_sql: Some("SELECT * FROM users WHERE amount > $1".into()),
            param_refs: Some(vec!["trigger_payload.amount".into()]),
            optional: None,
        };
        let result = eval_mapping(&m, &test_vars()).unwrap();
        assert_eq!(result["sql"], "SELECT * FROM users WHERE amount > $1");
        assert_eq!(result["params"][0], 100);
    }

    #[test]
    fn test_bytes_ref_roundtrip() {
        let m = CompiledMapping {
            target: "raw".into(),
            language: "bytes_ref".into(),
            source: "$.status".into(),
            compiled_sql: None,
            param_refs: None,
            optional: None,
        };
        let result = eval_mapping(&m, &test_vars()).unwrap();
        assert_eq!(result, json!("active"));
    }

    #[test]
    fn test_bytes_ref_no_prefix() {
        let m = CompiledMapping {
            target: "raw".into(),
            language: "bytes_ref".into(),
            source: "status".into(),
            compiled_sql: None,
            param_refs: None,
            optional: None,
        };
        let result = eval_mapping(&m, &test_vars()).unwrap();
        assert_eq!(result, json!("active"));
    }

    #[test]
    fn test_bytes_ref_missing_is_null() {
        let m = CompiledMapping {
            target: "raw".into(),
            language: "bytes_ref".into(),
            source: "nope".into(),
            compiled_sql: None,
            param_refs: None,
            optional: None,
        };
        let result = eval_mapping(&m, &test_vars()).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn test_vcel_real_eval() {
        let m = CompiledMapping {
            target: "x".into(),
            language: "v-cel".into(),
            source: "trigger_payload.amount + 1".into(),
            compiled_sql: None,
            param_refs: None,
            optional: None,
        };
        assert_eq!(eval_mapping(&m, &test_vars()).unwrap(), json!(101));
    }

    #[test]
    fn test_starlark_staged_error() {
        let m = CompiledMapping {
            target: "x".into(),
            language: "starlark".into(),
            source: "1 + 1".into(),
            compiled_sql: None,
            param_refs: None,
            optional: None,
        };
        let err = eval_mapping(&m, &test_vars()).unwrap_err();
        assert!(err.contains("Phase 3"), "got: {}", err);
    }

    #[test]
    fn test_vil_query_optional_switches_to_alt_when_control_is_null() {
        let mut vars = test_vars();
        vars.insert(
            "trigger_payload".into(),
            json!({"limit": 5, "offset": 10, "status": null}),
        );
        let m = CompiledMapping {
            target: "query".into(),
            language: "vil_query".into(),
            source: "original DSL".into(),
            compiled_sql: Some("SELECT * FROM orders WHERE status = $1 LIMIT $2 OFFSET $3".into()),
            param_refs: Some(vec![
                "trigger_payload.status".into(),
                "trigger_payload.limit".into(),
                "trigger_payload.offset".into(),
            ]),
            optional: Some(json!({
                "strategy": "where_eq_if_null_or_empty",
                "if_param_ref": "trigger_payload.status",
                "alt_sql": "SELECT * FROM orders LIMIT $1 OFFSET $2",
                "alt_param_refs": ["trigger_payload.limit", "trigger_payload.offset"]
            })),
        };
        let result = eval_mapping(&m, &vars).unwrap();
        assert_eq!(result["sql"], "SELECT * FROM orders LIMIT $1 OFFSET $2");
        assert_eq!(result["params"], json!([5, 10]));
        assert_eq!(
            result["_compiled_sql"],
            "SELECT * FROM orders WHERE status = $1 LIMIT $2 OFFSET $3"
        );
    }

    #[test]
    fn test_vil_query_optional_keeps_primary_when_control_present() {
        let mut vars = test_vars();
        vars.insert(
            "trigger_payload".into(),
            json!({"limit": 5, "offset": 10, "status": "paid"}),
        );
        let m = CompiledMapping {
            target: "query".into(),
            language: "vil_query".into(),
            source: "original DSL".into(),
            compiled_sql: Some("SELECT * FROM orders WHERE status = $1 LIMIT $2 OFFSET $3".into()),
            param_refs: Some(vec![
                "trigger_payload.status".into(),
                "trigger_payload.limit".into(),
                "trigger_payload.offset".into(),
            ]),
            optional: Some(json!({
                "strategy": "where_eq_if_null_or_empty",
                "if_param_ref": "trigger_payload.status",
                "alt_sql": "SELECT * FROM orders LIMIT $1 OFFSET $2",
                "alt_param_refs": ["trigger_payload.limit", "trigger_payload.offset"]
            })),
        };
        let result = eval_mapping(&m, &vars).unwrap();
        assert_eq!(
            result["sql"],
            "SELECT * FROM orders WHERE status = $1 LIMIT $2 OFFSET $3"
        );
        assert_eq!(result["params"], json!(["paid", 5, 10]));
    }
}
