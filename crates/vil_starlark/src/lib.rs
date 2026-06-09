//! VIL Compute — Starlark evaluation engine for VWFD Compute activities.
//!
//! Evaluates Starlark scripts (starlark-rust 0.13) with a JSON context,
//! enforces budget constraints (timeout, output-size), and returns
//! `serde_json::Value`.

use serde_json::Value as Json;
use starlark::environment::{Globals, Module};
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::dict::AllocDict;
use starlark::values::list::AllocList;
use starlark::values::{Heap, Value};

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::Duration;

/// Budget profile for Starlark execution.
#[derive(Debug, Clone)]
pub struct BudgetConfig {
    /// Wall-clock timeout in milliseconds. `None` means no timeout (inline eval).
    pub timeout_ms: Option<u64>,
    /// Maximum serialized output size in bytes (0 = unlimited).
    pub max_output_bytes: usize,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self::from_profile("default")
    }
}

impl BudgetConfig {
    /// Create a `BudgetConfig` from a named profile.
    ///
    /// Supported profiles: `"none"`, `"default"`, `"balanced"`, `"heavy"`.
    /// Unknown profiles fall back to `"default"`.
    pub fn from_profile(profile: &str) -> Self {
        match profile {
            "none" => Self {
                timeout_ms: None,
                max_output_bytes: 16 * 1024 * 1024,
            },
            "heavy" => Self {
                timeout_ms: Some(30_000),
                max_output_bytes: 16 * 1024 * 1024,
            },
            "balanced" => Self {
                timeout_ms: Some(5_000),
                max_output_bytes: 4 * 1024 * 1024,
            },
            _ => Self {
                timeout_ms: Some(1_000),
                max_output_bytes: 1024 * 1024,
            },
        }
    }
}

const AST_CACHE_MAX_ENTRIES: usize = 64;

#[derive(Default)]
struct AstCache {
    order: VecDeque<u64>,
    entries: HashMap<u64, AstCacheEntry>,
}

#[derive(Clone)]
struct AstCacheEntry {
    source: String,
    ast: AstModule,
}

fn ast_cache() -> &'static Mutex<AstCache> {
    static CACHE: OnceLock<Mutex<AstCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(AstCache::default()))
}

fn standard_globals() -> &'static Globals {
    static GLOBALS: OnceLock<Globals> = OnceLock::new();
    GLOBALS.get_or_init(Globals::standard)
}

fn source_key(source: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    source.len().hash(&mut h);
    source.hash(&mut h);
    h.finish()
}

fn parse_uncached(source: &str) -> Result<AstModule, String> {
    AstModule::parse("compute.star", source.to_owned(), &Dialect::Standard)
        .map_err(|e| format!("starlark parse: {}", e))
}

fn parse_cached(source: &str) -> Result<AstModule, String> {
    let key = source_key(source);
    {
        let cache = ast_cache()
            .lock()
            .map_err(|_| "starlark AST cache poisoned".to_string())?;
        if let Some(entry) = cache.entries.get(&key) {
            if entry.source == source {
                return Ok(entry.ast.clone());
            }
        }
    }

    let ast = parse_uncached(source)?;
    let mut cache = ast_cache()
        .lock()
        .map_err(|_| "starlark AST cache poisoned".to_string())?;
    if !cache.entries.contains_key(&key) {
        cache.order.push_back(key);
    }
    cache.entries.insert(
        key,
        AstCacheEntry {
            source: source.to_owned(),
            ast: ast.clone(),
        },
    );
    while cache.entries.len() > AST_CACHE_MAX_ENTRIES {
        if let Some(old) = cache.order.pop_front() {
            cache.entries.remove(&old);
        } else {
            break;
        }
    }
    Ok(ast)
}

/// Parse a Starlark source without using the runtime AST cache.
///
/// This is primarily used by H7 bridge microbenchmarks to isolate parse/compile
/// cost from execute-only cost.
pub fn compile_source(source: &str) -> Result<(), String> {
    parse_uncached(source).map(|_| ())
}

/// Evaluate a Starlark script with a JSON context, returning a JSON value.
pub fn eval(source: &str, entry: &str, ctx: &Json, budget: &BudgetConfig) -> Result<Json, String> {
    match budget.timeout_ms {
        None => {
            let out = eval_inner(source, entry, ctx)?;
            enforce_output(&out, budget.max_output_bytes)
        }
        Some(ms) => {
            // TODO(v-starlark): replace with opcode-VM instruction budget
            let source = source.to_owned();
            let entry = entry.to_owned();
            let ctx = ctx.clone();
            let max_bytes = budget.max_output_bytes;

            let (tx, rx) = mpsc::channel();
            let handle = std::thread::Builder::new()
                .name("vil-starlark".into())
                .spawn(move || {
                    let result = eval_inner(&source, &entry, &ctx);
                    let _ = tx.send(result);
                })
                .map_err(|e| format!("failed to spawn starlark thread: {}", e))?;

            match rx.recv_timeout(Duration::from_millis(ms)) {
                Ok(result) => {
                    handle
                        .join()
                        .map_err(|_| "starlark worker panicked".to_string())?;
                    enforce_output(&result?, max_bytes)
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    Err(format!("compute timeout after {}ms", ms))
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    Err("starlark worker disconnected".to_string())
                }
            }
        }
    }
}

fn eval_inner(source: &str, entry: &str, ctx: &Json) -> Result<Json, String> {
    let ast = parse_cached(source)?;
    let globals = standard_globals();

    Module::with_temp_heap(|module| {
        let ctx_value = json_to_starlark(module.heap(), ctx);

        let mut evaluator = Evaluator::new(&module);
        evaluator
            .eval_module(ast, globals)
            .map_err(|e| format!("starlark eval: {}", e))?;

        let func = module
            .get(entry)
            .ok_or_else(|| format!("entry '{}' not found in module", entry))?;

        let result = evaluator
            .eval_function(func, &[ctx_value], &[])
            .map_err(|e| format!("starlark call '{}': {}", entry, e))?;

        result
            .to_json_value()
            .map_err(|e| format!("starlark to_json: {}", e))
    })
}

fn json_to_starlark<'v>(heap: Heap<'v>, v: &Json) -> Value<'v> {
    match v {
        Json::Null => Value::new_none(),
        Json::Bool(b) => Value::new_bool(*b),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                if i >= i32::MIN as i64 && i <= i32::MAX as i64 {
                    return heap.alloc(i as i32);
                }
                return heap.alloc(i as f64);
            }
            if let Some(u) = n.as_u64() {
                if u <= i32::MAX as u64 {
                    return heap.alloc(u as i32);
                }
                return heap.alloc(u as f64);
            }
            if let Some(f) = n.as_f64() {
                return heap.alloc(f);
            }
            Value::new_none()
        }
        Json::String(s) => heap.alloc(s.as_str()),
        Json::Array(arr) => {
            let items: Vec<Value> = arr.iter().map(|v| json_to_starlark(heap, v)).collect();
            heap.alloc(AllocList(items))
        }
        Json::Object(obj) => {
            let items: Vec<(Value, Value)> = obj
                .iter()
                .map(|(k, v)| {
                    let key = heap.alloc(k.as_str());
                    let val = json_to_starlark(heap, v);
                    (key, val)
                })
                .collect();
            heap.alloc(AllocDict(items))
        }
    }
}

fn enforce_output(out: &Json, max_bytes: usize) -> Result<Json, String> {
    if max_bytes > 0 {
        let serialized =
            serde_json::to_string(out).map_err(|e| format!("serialize output: {}", e))?;
        if serialized.len() > max_bytes {
            return Err(format!(
                "output exceeds {} bytes ({} bytes)",
                max_bytes,
                serialized.len()
            ));
        }
    }
    Ok(out.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_compile_source_uncached() {
        let source = r#"
def run(ctx):
    return {"ok": True}
"#;
        compile_source(source).unwrap();
    }

    #[test]
    fn test_eval_uses_cache_without_changing_result() {
        let source = r#"
def run(ctx):
    return {"result": ctx["x"] + 1}
"#;
        let ctx = json!({"x": 41});
        let budget = BudgetConfig::from_profile("none");
        assert_eq!(eval(source, "run", &ctx, &budget).unwrap()["result"], 42);
        assert_eq!(eval(source, "run", &ctx, &budget).unwrap()["result"], 42);
    }

    #[test]
    fn test_eval_dict_return() {
        let source = r#"
def run(ctx):
    return {"greeting": "hello " + ctx["name"], "count": len(ctx["items"])}
"#;
        let ctx = json!({"name": "world", "items": [1, 2, 3]});
        let budget = BudgetConfig::default();
        let result = eval(source, "run", &ctx, &budget).unwrap();
        assert_eq!(result["greeting"], "hello world");
        assert_eq!(result["count"], 3);
    }

    #[test]
    fn test_eval_scalar_return() {
        let source = r#"
def run(ctx):
    return ctx["x"] * ctx["y"]
"#;
        let ctx = json!({"x": 6, "y": 7});
        let budget = BudgetConfig::default();
        let result = eval(source, "run", &ctx, &budget).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn test_eval_array_return() {
        let source = r#"
def run(ctx):
    return [ctx["a"], ctx["b"], ctx["c"]]
"#;
        let ctx = json!({"a": 1, "b": 2, "c": 3});
        let budget = BudgetConfig::default();
        let result = eval(source, "run", &ctx, &budget).unwrap();
        assert_eq!(result, json!([1, 2, 3]));
    }

    #[test]
    fn test_eval_object_return() {
        let source = r#"
def run(ctx):
    return {"sum": ctx["x"] + ctx["y"], "product": ctx["x"] * ctx["y"]}
"#;
        let ctx = json!({"x": 3, "y": 4});
        let budget = BudgetConfig::default();
        let result = eval(source, "run", &ctx, &budget).unwrap();
        assert_eq!(result["sum"], 7);
        assert_eq!(result["product"], 12);
    }

    #[test]
    fn test_entry_not_found() {
        let source = "x = 1";
        let ctx = json!({});
        let budget = BudgetConfig::default();
        let result = eval(source, "run", &ctx, &budget);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_non_dict_return_ok() {
        let source = r#"
def run(ctx):
    return 42
"#;
        let ctx = json!({});
        let budget = BudgetConfig::default();
        let result = eval(source, "run", &ctx, &budget).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn test_timeout() {
        let source = r#"
def run(ctx):
    x = 0
    for i in range(100000):
        for j in range(100000):
            x = x + 1
    return x
"#;
        let ctx = json!({});
        let budget = BudgetConfig {
            timeout_ms: Some(50),
            max_output_bytes: 1024,
        };
        let result = eval(source, "run", &ctx, &budget);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("timeout"));
    }
}
