/// Evaluator — walk AST against variable map, produce serde_json::Value.
use crate::ast::*;
use serde_json::Value;
use std::collections::HashMap;

pub type Vars = HashMap<String, Value>;

pub fn eval(expr: &Expr, vars: &Vars) -> Result<Value, String> {
    match expr {
        // ── Literals ──
        Expr::Int(n) => Ok(Value::Number((*n).into())),
        Expr::Float(n) => Ok(serde_json::Number::from_f64(*n)
            .map(Value::Number)
            .unwrap_or(Value::Null)),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::String(s) => Ok(Value::String(s.clone())),
        Expr::Null => Ok(Value::Null),

        // ── Collections ──
        Expr::List(items) => {
            let vals: Result<Vec<Value>, _> = items.iter().map(|e| eval(e, vars)).collect();
            Ok(Value::Array(vals?))
        }
        Expr::Map(entries) => {
            let mut map = serde_json::Map::new();
            for (k, v) in entries {
                let key = val_to_string(&eval(k, vars)?);
                let val = eval(v, vars)?;
                map.insert(key, val);
            }
            Ok(Value::Object(map))
        }

        // ── Ident (variable lookup) ──
        Expr::Ident(name) => Ok(vars.get(name).cloned().unwrap_or(Value::Null)),

        // ── Field access: expr.field ──
        Expr::Field(obj, field) => {
            let val = eval(obj, vars)?;
            Ok(field_access(&val, field))
        }

        // ── Index: expr[index] ──
        Expr::Index(obj, idx) => {
            let val = eval(obj, vars)?;
            let i = eval(idx, vars)?;
            match (&val, &i) {
                (Value::Array(arr), Value::Number(n)) => {
                    let idx = n.as_u64().unwrap_or(0) as usize;
                    Ok(arr.get(idx).cloned().unwrap_or(Value::Null))
                }
                (Value::Object(map), Value::String(key)) => {
                    Ok(map.get(key).cloned().unwrap_or(Value::Null))
                }
                _ => Ok(Value::Null),
            }
        }

        // ── Unary ──
        Expr::Unary(op, e) => {
            let v = eval(e, vars)?;
            match op {
                UnaryOp::Not => Ok(Value::Bool(!val_to_bool(&v))),
                UnaryOp::Neg => match &v {
                    Value::Number(n) => {
                        if let Some(i) = n.as_i64() {
                            Ok(Value::Number((-i).into()))
                        } else if let Some(f) = n.as_f64() {
                            Ok(serde_json::Number::from_f64(-f)
                                .map(Value::Number)
                                .unwrap_or(Value::Null))
                        } else {
                            Ok(Value::Null)
                        }
                    }
                    _ => Err(format!("cannot negate {:?}", v)),
                },
            }
        }

        // ── Binary ──
        Expr::Binary(op, left, right) => {
            let l = eval(left, vars)?;
            // Short-circuit for && and ||
            match op {
                BinaryOp::And => {
                    if !val_to_bool(&l) {
                        return Ok(Value::Bool(false));
                    }
                    let r = eval(right, vars)?;
                    return Ok(Value::Bool(val_to_bool(&r)));
                }
                BinaryOp::Or => {
                    if val_to_bool(&l) {
                        return Ok(Value::Bool(true));
                    }
                    let r = eval(right, vars)?;
                    return Ok(Value::Bool(val_to_bool(&r)));
                }
                _ => {}
            }
            let r = eval(right, vars)?;
            eval_binary(*op, &l, &r)
        }

        // ── Ternary ──
        Expr::Ternary(cond, then, else_) => {
            if val_to_bool(&eval(cond, vars)?) {
                eval(then, vars)
            } else {
                eval(else_, vars)
            }
        }

        // ── In ──
        Expr::In(item, collection) => {
            let item_val = eval(item, vars)?;
            let coll_val = eval(collection, vars)?;
            Ok(Value::Bool(val_in(&item_val, &coll_val)))
        }

        // ── Not In ──
        Expr::NotIn(item, collection) => {
            let item_val = eval(item, vars)?;
            let coll_val = eval(collection, vars)?;
            Ok(Value::Bool(!val_in(&item_val, &coll_val)))
        }

        // ── IS NULL ──
        Expr::IsNull(e) => {
            let v = eval(e, vars)?;
            Ok(Value::Bool(v.is_null()))
        }

        // ── IS NOT NULL ──
        Expr::IsNotNull(e) => {
            let v = eval(e, vars)?;
            Ok(Value::Bool(!v.is_null()))
        }

        // ── Function call ──
        Expr::FnCall(name, args) => eval_function(name, args, vars),

        // ── Comprehension method calls (V-CEL lambdas) ──
        // Intercept BEFORE the generic MethodCall arm: the lambda body must stay
        // unevaluated AST and be re-run per element with the binder bound.
        Expr::MethodCall(obj, method, args)
            if matches!(
                method.as_str(),
                "filter" | "map" | "all" | "exists" | "exists_one"
            ) =>
        {
            eval_comprehension(obj, method, args, vars)
        }

        // ── Method call ──
        Expr::MethodCall(obj, method, args) => {
            let obj_val = eval(obj, vars)?;
            let arg_vals: Result<Vec<Value>, _> = args.iter().map(|a| eval(a, vars)).collect();
            eval_method(&obj_val, method, &arg_vals?)
        }
    }
}

// ── Comprehension evaluation (V-CEL list macros, method form) ──
//
// The body is re-evaluated against a child scope per element. We clone `vars`
// once and overwrite only the binder slot each iteration to stay allocation-light.
fn eval_comprehension(
    obj: &Expr,
    method: &str,
    args: &[Expr],
    vars: &Vars,
) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!(
            ".{}(binder, body) requires exactly 2 arguments",
            method
        ));
    }
    let binder = match &args[0] {
        Expr::Ident(name) => name.clone(),
        _ => return Err(format!(".{}() binder must be a plain identifier", method)),
    };
    let body = &args[1];
    let coll = eval(obj, vars)?;
    let items = match &coll {
        Value::Array(a) => a,
        other => {
            return Err(format!(
                ".{}() requires a list receiver, got {}",
                method,
                obj_type_name(other)
            ))
        }
    };

    let mut scope = vars.clone();
    match method {
        "map" => {
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                scope.insert(binder.clone(), it.clone());
                out.push(eval(body, &scope)?);
            }
            Ok(Value::Array(out))
        }
        "filter" => {
            let mut out = Vec::new();
            for it in items {
                scope.insert(binder.clone(), it.clone());
                if val_to_bool(&eval(body, &scope)?) {
                    out.push(it.clone());
                }
            }
            Ok(Value::Array(out))
        }
        "all" => {
            for it in items {
                scope.insert(binder.clone(), it.clone());
                if !val_to_bool(&eval(body, &scope)?) {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        }
        "exists" => {
            for it in items {
                scope.insert(binder.clone(), it.clone());
                if val_to_bool(&eval(body, &scope)?) {
                    return Ok(Value::Bool(true));
                }
            }
            Ok(Value::Bool(false))
        }
        "exists_one" => {
            let mut count = 0usize;
            for it in items {
                scope.insert(binder.clone(), it.clone());
                if val_to_bool(&eval(body, &scope)?) {
                    count += 1;
                    if count > 1 {
                        return Ok(Value::Bool(false));
                    }
                }
            }
            Ok(Value::Bool(count == 1))
        }
        _ => unreachable!("comprehension method already filtered by guard"),
    }
}

// ── Binary operator evaluation ──

fn eval_binary(op: BinaryOp, l: &Value, r: &Value) -> Result<Value, String> {
    match op {
        // String concat or numeric add
        BinaryOp::Add => {
            if l.is_string() || r.is_string() {
                Ok(Value::String(val_to_string(l) + &val_to_string(r)))
            } else if let (Some(a), Some(b)) = (l.as_f64(), r.as_f64()) {
                // If both are integers, keep as integer
                if l.is_i64() && r.is_i64() {
                    Ok(Value::Number(
                        (l.as_i64().unwrap() + r.as_i64().unwrap()).into(),
                    ))
                } else {
                    Ok(serde_json::Number::from_f64(a + b)
                        .map(Value::Number)
                        .unwrap_or(Value::Null))
                }
            } else if let (Some(a), Some(b)) = (l.as_array(), r.as_array()) {
                // List concat
                let mut combined = a.clone();
                combined.extend(b.iter().cloned());
                Ok(Value::Array(combined))
            } else {
                Ok(Value::String(val_to_string(l) + &val_to_string(r)))
            }
        }
        BinaryOp::Sub => num_op(l, r, |a, b| a - b, |a, b| a - b),
        BinaryOp::Mul => num_op(l, r, |a, b| a * b, |a, b| a * b),
        BinaryOp::Div => {
            if r.as_f64() == Some(0.0) {
                return Err("division by zero".into());
            }
            num_op(l, r, |a, b| a / b, |a, b| a / b)
        }
        BinaryOp::Mod => num_op(l, r, |a, b| a % b, |a, b| a % b),

        // Comparison
        BinaryOp::Eq => Ok(Value::Bool(val_eq(l, r))),
        BinaryOp::Neq => Ok(Value::Bool(!val_eq(l, r))),
        BinaryOp::Lt => cmp_op(l, r, |ord| ord == std::cmp::Ordering::Less),
        BinaryOp::Lte => cmp_op(l, r, |ord| ord != std::cmp::Ordering::Greater),
        BinaryOp::Gt => cmp_op(l, r, |ord| ord == std::cmp::Ordering::Greater),
        BinaryOp::Gte => cmp_op(l, r, |ord| ord != std::cmp::Ordering::Less),

        // And/Or handled above (short-circuit)
        BinaryOp::And | BinaryOp::Or => unreachable!(),
    }
}

fn num_op(
    l: &Value,
    r: &Value,
    int_op: fn(i64, i64) -> i64,
    float_op: fn(f64, f64) -> f64,
) -> Result<Value, String> {
    if let (Some(a), Some(b)) = (l.as_i64(), r.as_i64()) {
        Ok(Value::Number(int_op(a, b).into()))
    } else if let (Some(a), Some(b)) = (l.as_f64(), r.as_f64()) {
        Ok(serde_json::Number::from_f64(float_op(a, b))
            .map(Value::Number)
            .unwrap_or(Value::Null))
    } else {
        Err(format!("cannot apply arithmetic to {:?} and {:?}", l, r))
    }
}

fn cmp_op(l: &Value, r: &Value, pred: fn(std::cmp::Ordering) -> bool) -> Result<Value, String> {
    if let (Some(a), Some(b)) = (l.as_f64(), r.as_f64()) {
        Ok(Value::Bool(pred(
            a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal),
        )))
    } else if let (Some(a), Some(b)) = (l.as_str(), r.as_str()) {
        Ok(Value::Bool(pred(a.cmp(b))))
    } else {
        Ok(Value::Bool(false))
    }
}

// ── Function evaluation (vil-expr §3.2.6, §3.2.8) ──

fn eval_function(name: &str, args: &[Expr], vars: &Vars) -> Result<Value, String> {
    match name {
        "size" | "SIZE" => {
            if args.len() != 1 {
                return Err("size() takes 1 argument".into());
            }
            let v = eval(&args[0], vars)?;
            Ok(Value::Number(
                match &v {
                    Value::String(s) => s.len() as i64,
                    Value::Array(a) => a.len() as i64,
                    Value::Object(m) => m.len() as i64,
                    _ => 0,
                }
                .into(),
            ))
        }
        // vdicl: ISBLANK(x) — true if null, empty string, or whitespace-only
        "ISBLANK" | "isblank" => {
            if args.len() != 1 {
                return Err("ISBLANK() takes 1 argument".into());
            }
            let v = eval(&args[0], vars)?;
            Ok(Value::Bool(match &v {
                Value::Null => true,
                Value::String(s) => s.trim().is_empty(),
                _ => false,
            }))
        }
        // vdicl: LENGTH(x) — string/array length
        "LENGTH" | "length" => {
            if args.len() != 1 {
                return Err("LENGTH() takes 1 argument".into());
            }
            let v = eval(&args[0], vars)?;
            Ok(Value::Number(
                match &v {
                    Value::String(s) => s.len() as i64,
                    Value::Array(a) => a.len() as i64,
                    Value::Null => 0,
                    _ => 0,
                }
                .into(),
            ))
        }
        "has" => {
            // has(obj.field) — check field existence
            // In vil-expr, `has` takes a field select expression
            if args.len() != 1 {
                return Err("has() takes 1 argument".into());
            }
            match &args[0] {
                Expr::Field(obj, field) => {
                    let v = eval(obj, vars)?;
                    Ok(Value::Bool(match &v {
                        Value::Object(m) => m.contains_key(field),
                        _ => false,
                    }))
                }
                _ => {
                    let v = eval(&args[0], vars)?;
                    Ok(Value::Bool(!v.is_null()))
                }
            }
        }
        "int" => {
            if args.len() != 1 {
                return Err("int() takes 1 argument".into());
            }
            let v = eval(&args[0], vars)?;
            Ok(match &v {
                Value::Number(n) => {
                    let i = n
                        .as_i64()
                        .unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64);
                    Value::Number(i.into())
                }
                Value::String(s) => Value::Number(s.parse::<i64>().unwrap_or(0).into()),
                Value::Bool(b) => Value::Number(if *b { 1i64 } else { 0 }.into()),
                _ => Value::Number(0i64.into()),
            })
        }
        "double" | "float" => {
            if args.len() != 1 {
                return Err(format!("{}() takes 1 argument", name));
            }
            let v = eval(&args[0], vars)?;
            let f = match &v {
                Value::Number(n) => n.as_f64().unwrap_or(0.0),
                Value::String(s) => s.parse().unwrap_or(0.0),
                _ => 0.0,
            };
            Ok(serde_json::Number::from_f64(f)
                .map(Value::Number)
                .unwrap_or(Value::Null))
        }
        "string" => {
            if args.len() != 1 {
                return Err("string() takes 1 argument".into());
            }
            let v = eval(&args[0], vars)?;
            Ok(Value::String(val_to_string(&v)))
        }
        "type" => {
            if args.len() != 1 {
                return Err("type() takes 1 argument".into());
            }
            let v = eval(&args[0], vars)?;
            Ok(Value::String(
                match &v {
                    Value::Null => "null",
                    Value::Bool(_) => "bool",
                    Value::Number(_) => "number",
                    Value::String(_) => "string",
                    Value::Array(_) => "list",
                    Value::Object(_) => "map",
                }
                .into(),
            ))
        }
        "max" | "greatest" => {
            if args.is_empty() {
                return Err("max() needs at least 1 argument".into());
            }
            let vals: Result<Vec<Value>, _> = args.iter().map(|a| eval(a, vars)).collect();
            let vals = vals?;
            let mut best = &vals[0];
            for v in &vals[1..] {
                if let (Some(a), Some(b)) = (v.as_f64(), best.as_f64()) {
                    if a > b {
                        best = v;
                    }
                }
            }
            Ok(best.clone())
        }
        "min" | "least" => {
            if args.is_empty() {
                return Err("min() needs at least 1 argument".into());
            }
            let vals: Result<Vec<Value>, _> = args.iter().map(|a| eval(a, vars)).collect();
            let vals = vals?;
            let mut best = &vals[0];
            for v in &vals[1..] {
                if let (Some(a), Some(b)) = (v.as_f64(), best.as_f64()) {
                    if a < b {
                        best = v;
                    }
                }
            }
            Ok(best.clone())
        }
        // ── V-CEL casts ──
        "uint" => {
            if args.len() != 1 {
                return Err("uint() takes 1 argument".into());
            }
            let v = eval(&args[0], vars)?;
            let i = match &v {
                Value::Number(n) => n
                    .as_i64()
                    .unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64),
                Value::String(s) => s
                    .trim()
                    .parse::<i64>()
                    .map_err(|_| format!("uint(): cannot parse '{}'", s))?,
                Value::Bool(true) => 1,
                Value::Bool(false) => 0,
                _ => 0,
            };
            if i < 0 {
                return Err("uint(): value is negative".into());
            }
            Ok(Value::Number(i.into()))
        }
        "dyn" => {
            if args.len() != 1 {
                return Err("dyn() takes 1 argument".into());
            }
            eval(&args[0], vars)
        }
        "bytes" => {
            if args.len() != 1 {
                return Err("bytes() takes 1 argument".into());
            }
            let v = eval(&args[0], vars)?;
            // bytes(string) -> JSON array of UTF-8 byte values; bytes(list) -> passthrough.
            let out = match &v {
                Value::String(s) => s
                    .as_bytes()
                    .iter()
                    .map(|b| Value::Number((*b as i64).into()))
                    .collect::<Vec<_>>(),
                Value::Array(a) => a.clone(),
                _ => return Err("bytes(): expected a string".into()),
            };
            Ok(Value::Array(out))
        }

        // ── Regex (core, non-feature-gated) ──
        "matches" => {
            if args.len() != 2 {
                return Err("matches(s, re) takes 2 arguments".into());
            }
            let s = eval(&args[0], vars)?;
            let re = eval(&args[1], vars)?;
            let text = s.as_str().unwrap_or("");
            let pat = re.as_str().ok_or("matches(): pattern must be a string")?;
            let compiled = regex::Regex::new(pat).map_err(|e| format!("matches(): {}", e))?;
            Ok(Value::Bool(compiled.is_match(text)))
        }

        // ── String free-function forms (mirror the method forms) ──
        "startsWith" | "endsWith" | "contains" | "replace" | "split" | "substring" | "trim"
        | "to_lower" | "to_upper" | "toLowerCase" | "toUpperCase" => {
            let vals: Result<Vec<Value>, _> = args.iter().map(|a| eval(a, vars)).collect();
            let vals = vals?;
            let (recv, rest) = vals
                .split_first()
                .ok_or_else(|| format!("{}() requires a receiver argument", name))?;
            eval_method(recv, name, rest)
        }

        // ── Encoding ──
        "base64_encode" => {
            if args.len() != 1 {
                return Err("base64_encode() takes 1 argument".into());
            }
            use base64::Engine;
            let v = eval(&args[0], vars)?;
            let bytes = match &v {
                Value::String(s) => s.as_bytes().to_vec(),
                other => val_to_string(other).into_bytes(),
            };
            Ok(Value::String(
                base64::engine::general_purpose::STANDARD.encode(bytes),
            ))
        }
        "base64_decode" => {
            if args.len() != 1 {
                return Err("base64_decode() takes 1 argument".into());
            }
            use base64::Engine;
            let v = eval(&args[0], vars)?;
            let s = v.as_str().ok_or("base64_decode(): string required")?;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(s)
                .map_err(|e| format!("base64_decode(): {}", e))?;
            Ok(Value::String(
                String::from_utf8_lossy(&decoded).into_owned(),
            ))
        }

        // ── JSON ──
        "json_parse" => {
            if args.len() != 1 {
                return Err("json_parse() takes 1 argument".into());
            }
            let v = eval(&args[0], vars)?;
            let s = v.as_str().ok_or("json_parse(): string required")?;
            serde_json::from_str(s).map_err(|e| format!("json_parse(): {}", e))
        }
        "ndjson_parse" => {
            if args.len() != 1 {
                return Err("ndjson_parse() takes 1 argument".into());
            }
            let v = eval(&args[0], vars)?;
            let s = v.as_str().ok_or("ndjson_parse(): string required")?;
            let mut out = Vec::new();
            for line in s.lines() {
                let t = line.trim();
                if t.is_empty() {
                    continue;
                }
                out.push(serde_json::from_str(t).map_err(|e| format!("ndjson_parse(): {}", e))?);
            }
            Ok(Value::Array(out))
        }

        // ── Temporal (core, chrono + chrono-tz) ──
        // timestamp() returns a normalized RFC3339 string; accessors parse it.
        // duration() returns {"__duration_ms__": i64}.
        "timestamp" => {
            if args.len() != 1 {
                return Err("timestamp() takes 1 argument".into());
            }
            let v = eval(&args[0], vars)?;
            let s = v.as_str().ok_or("timestamp(): RFC3339 string required")?;
            let dt = chrono::DateTime::parse_from_rfc3339(s)
                .map_err(|e| format!("timestamp(): invalid RFC3339 '{}': {}", s, e))?;
            Ok(Value::String(dt.to_rfc3339()))
        }
        "duration" if args.len() == 1 => {
            let v = eval(&args[0], vars)?;
            let s = v.as_str().ok_or("duration(): string required")?;
            let ms = parse_duration_ms(s)?;
            let mut m = serde_json::Map::new();
            m.insert("__duration_ms__".into(), Value::Number(ms.into()));
            Ok(Value::Object(m))
        }
        "getYear" | "getMonth" | "getDayOfMonth" | "getDate" | "getDayOfWeek" | "getDayOfYear"
        | "getHours" | "getMinutes" | "getSeconds" | "getMilliseconds" => {
            if args.is_empty() || args.len() > 2 {
                return Err(format!("{}(timestamp, [tz]) takes 1 or 2 arguments", name));
            }
            let v = eval(&args[0], vars)?;
            let s = v
                .as_str()
                .ok_or_else(|| format!("{}(): timestamp string required", name))?;
            let tz = if args.len() == 2 {
                let tzv = eval(&args[1], vars)?;
                Some(
                    tzv.as_str()
                        .ok_or_else(|| format!("{}(): timezone must be a string", name))?
                        .to_string(),
                )
            } else {
                None
            };
            temporal_accessor(name, s, tz.as_deref())
        }

        // ── Comprehension macros (free-function form) ──
        "transformList" => eval_transform_list(args, vars),
        "transformMap" => eval_transform_map(args, vars),

        // ── Built-in FaaS functions (feature-gated) ──
        _ => {
            let evaluated_args: Result<Vec<Value>, _> =
                args.iter().map(|a| eval(a, vars)).collect();
            let evaluated_args = evaluated_args?;
            dispatch_faas(name, &evaluated_args)
        }
    }
}

/// Dispatch to optional FaaS crate functions.
fn dispatch_faas(name: &str, #[allow(unused)] args: &[Value]) -> Result<Value, String> {
    #[cfg(feature = "faas-core")]
    match name {
        "sha256" => return vil_hash::sha256(args),
        "md5" => return vil_hash::md5(args),
        "hmac_sha256" => return vil_hash::hmac_sha256(args),
        _ => {}
    }
    #[cfg(feature = "faas-core")]
    match name {
        "aes_encrypt" => return vil_crypto::aes_encrypt(args),
        "aes_decrypt" => return vil_crypto::aes_decrypt(args),
        _ => {}
    }
    #[cfg(feature = "faas-core")]
    match name {
        "jwt_sign" => return vil_jwt::jwt_sign(args),
        "jwt_verify" => return vil_jwt::jwt_verify(args),
        _ => {}
    }
    #[cfg(feature = "faas-core")]
    match name {
        "uuid_v4" => return vil_id_gen::uuid_v4(args),
        "uuid_v7" => return vil_id_gen::uuid_v7(args),
        "ulid" => return vil_id_gen::ulid(args),
        "nanoid" => return vil_id_gen::nanoid(args),
        _ => {}
    }
    #[cfg(feature = "faas-core")]
    match name {
        "parse_date" => return vil_datefmt::parse_date(args),
        "format_date" => return vil_datefmt::format_date(args),
        "now" => return vil_datefmt::now(args),
        _ => {}
    }
    #[cfg(feature = "faas-core")]
    match name {
        "age" => return vil_duration::age(args),
        "duration" => return vil_duration::duration(args),
        _ => {}
    }
    #[cfg(feature = "faas-core")]
    if name == "parse_csv" {
        return vil_parse_csv::parse_csv(args);
    }
    #[cfg(feature = "faas-core")]
    match name {
        "parse_xml" => return vil_parse_xml::parse_xml(args),
        "xpath" => return vil_parse_xml::xpath(args),
        _ => {}
    }
    #[cfg(feature = "faas-core")]
    match name {
        "regex_match" => return vil_regex::regex_match(args),
        "regex_extract" => return vil_regex::regex_extract(args),
        "regex_replace" => return vil_regex::regex_replace(args),
        _ => {}
    }
    #[cfg(feature = "faas-core")]
    if name == "parse_phone" {
        return vil_phone::parse_phone(args);
    }

    // ── Batch 2: Transform + Stats + Notification + Geo ──
    #[cfg(feature = "faas-full")]
    if name == "validate_schema" {
        return vil_validate_schema::validate_schema(args);
    }
    #[cfg(feature = "faas-full")]
    if name == "mask_pii" {
        return vil_mask::mask_pii(args);
    }
    #[cfg(feature = "faas-full")]
    if name == "reshape" {
        return vil_reshape::reshape(args);
    }
    #[cfg(feature = "faas-full")]
    if name == "render_template" {
        return vil_template::render_template(args);
    }
    #[cfg(feature = "faas-full")]
    if name == "validate_email" {
        return vil_email_validate::validate_email(args);
    }
    #[cfg(feature = "faas-full")]
    match name {
        "mean" => return vil_stats::mean(args),
        "median" => return vil_stats::median(args),
        "stdev" => return vil_stats::stdev(args),
        "percentile" => return vil_stats::percentile(args),
        "variance" => return vil_stats::variance(args),
        _ => {}
    }
    #[cfg(feature = "faas-full")]
    if name == "is_anomaly" {
        return vil_anomaly::is_anomaly(args);
    }
    #[cfg(feature = "faas-full")]
    if name == "send_email" {
        return vil_email::send_email(args);
    }
    #[cfg(feature = "faas-full")]
    if name == "send_webhook" {
        return vil_webhook_out::send_webhook(args);
    }
    #[cfg(feature = "faas-full")]
    if name == "geo_distance" {
        return vil_geodist::geo_distance(args);
    }

    Err(format!(
        "unknown function: {}(). Enable 'faas-core' or 'faas-full' feature.",
        name
    ))
}

// ── Method evaluation (vil-expr §3.2.2-4) ──

fn eval_method(obj: &Value, method: &str, args: &[Value]) -> Result<Value, String> {
    match (obj, method) {
        // String methods
        (Value::String(s), "contains") => {
            let arg = args.first().and_then(|a| a.as_str()).unwrap_or("");
            Ok(Value::Bool(s.contains(arg)))
        }
        (Value::String(s), "startsWith") => {
            let arg = args.first().and_then(|a| a.as_str()).unwrap_or("");
            Ok(Value::Bool(s.starts_with(arg)))
        }
        (Value::String(s), "endsWith") => {
            let arg = args.first().and_then(|a| a.as_str()).unwrap_or("");
            Ok(Value::Bool(s.ends_with(arg)))
        }
        (Value::String(s), "size") | (Value::String(s), "length") => {
            Ok(Value::Number((s.len() as i64).into()))
        }
        (Value::String(s), "split") => {
            let delim = args.first().and_then(|a| a.as_str()).unwrap_or("/");
            let parts: Vec<Value> = s
                .split(delim)
                .map(|p| Value::String(p.to_string()))
                .collect();
            Ok(Value::Array(parts))
        }
        (Value::String(s), "trim") => Ok(Value::String(s.trim().to_string())),
        (Value::String(s), "toUpperCase") | (Value::String(s), "to_upper") => {
            Ok(Value::String(s.to_uppercase()))
        }
        (Value::String(s), "toLowerCase") | (Value::String(s), "to_lower") => {
            Ok(Value::String(s.to_lowercase()))
        }
        (Value::String(s), "matches") => {
            let pat = args.first().and_then(|a| a.as_str()).unwrap_or("");
            let compiled = regex::Regex::new(pat).map_err(|e| format!("matches(): {}", e))?;
            Ok(Value::Bool(compiled.is_match(s)))
        }
        (Value::String(s), "replace") => {
            let from = args.first().and_then(|a| a.as_str()).unwrap_or("");
            let to = args.get(1).and_then(|a| a.as_str()).unwrap_or("");
            Ok(Value::String(s.replace(from, to)))
        }
        (Value::String(s), "substring") => {
            let start = args.first().and_then(|a| a.as_u64()).unwrap_or(0) as usize;
            let end = args
                .get(1)
                .and_then(|a| a.as_u64())
                .map(|e| e as usize)
                .unwrap_or(s.len());
            Ok(Value::String(
                s.get(start..end.min(s.len())).unwrap_or("").to_string(),
            ))
        }

        // List/Map size method
        (Value::Array(a), "size") | (Value::Array(a), "length") => {
            Ok(Value::Number((a.len() as i64).into()))
        }
        (Value::Array(a), "last") => Ok(a.last().cloned().unwrap_or(Value::Null)),
        (Value::Array(a), "first") => Ok(a.first().cloned().unwrap_or(Value::Null)),
        (Value::Object(m), "size") => Ok(Value::Number((m.len() as i64).into())),

        // Unsupported macros → clear error
        (_, "map" | "filter" | "all" | "exists" | "exists_one") => Err(format!(
            ".{}() is a vil-expr list macro that requires VFlow cloud compiler. \
                 Rewrite using basic expressions or use: vflow compile --cloud",
            method
        )),

        _ => Err(format!(
            "unknown method .{}() on {:?}",
            method,
            obj_type_name(obj)
        )),
    }
}

// ── Helpers ──

fn field_access(val: &Value, field: &str) -> Value {
    match val {
        Value::Object(map) => map.get(field).cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

fn val_to_bool(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

fn val_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        _ => v.to_string(),
    }
}

fn val_eq(a: &Value, b: &Value) -> bool {
    // Numeric equality across int/float
    if let (Some(x), Some(y)) = (a.as_f64(), b.as_f64()) {
        return x == y;
    }
    a == b
}

fn val_in(item: &Value, collection: &Value) -> bool {
    match collection {
        Value::Array(arr) => arr.iter().any(|v| val_eq(item, v)),
        Value::Object(map) => {
            let key = val_to_string(item);
            map.contains_key(&key)
        }
        _ => false,
    }
}

fn obj_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "list",
        Value::Object(_) => "map",
    }
}

// ── V-CEL comprehension macros (free-function form) ──
fn eval_transform_list(args: &[Expr], vars: &Vars) -> Result<Value, String> {
    // transformList(coll, x, body) | transformList(coll, x, filter, body)
    if args.len() != 3 && args.len() != 4 {
        return Err("transformList(coll, x, [filter,] body) takes 3 or 4 arguments".into());
    }
    let coll = eval(&args[0], vars)?;
    let items = match &coll {
        Value::Array(a) => a,
        other => {
            return Err(format!(
                "transformList(): first argument must be a list, got {}",
                obj_type_name(other)
            ))
        }
    };
    let binder = match &args[1] {
        Expr::Ident(n) => n.clone(),
        _ => return Err("transformList(): binder must be a plain identifier".into()),
    };
    let (filter, body) = if args.len() == 4 {
        (Some(&args[2]), &args[3])
    } else {
        (None, &args[2])
    };
    let mut scope = vars.clone();
    let mut out = Vec::new();
    for it in items {
        scope.insert(binder.clone(), it.clone());
        if let Some(f) = filter {
            if !val_to_bool(&eval(f, &scope)?) {
                continue;
            }
        }
        out.push(eval(body, &scope)?);
    }
    Ok(Value::Array(out))
}

fn eval_transform_map(args: &[Expr], vars: &Vars) -> Result<Value, String> {
    // transformMap(map, k, v, body) | transformMap(map, k, v, filter, body)
    if args.len() != 4 && args.len() != 5 {
        return Err("transformMap(map, k, v, [filter,] body) takes 4 or 5 arguments".into());
    }
    let src = eval(&args[0], vars)?;
    let obj = match &src {
        Value::Object(m) => m,
        other => {
            return Err(format!(
                "transformMap(): first argument must be a map, got {}",
                obj_type_name(other)
            ))
        }
    };
    let kbind = match &args[1] {
        Expr::Ident(n) => n.clone(),
        _ => return Err("transformMap(): key binder must be a plain identifier".into()),
    };
    let vbind = match &args[2] {
        Expr::Ident(n) => n.clone(),
        _ => return Err("transformMap(): value binder must be a plain identifier".into()),
    };
    let (filter, body) = if args.len() == 5 {
        (Some(&args[3]), &args[4])
    } else {
        (None, &args[3])
    };
    let mut scope = vars.clone();
    let mut out = serde_json::Map::new();
    for (k, v) in obj {
        scope.insert(kbind.clone(), Value::String(k.clone()));
        scope.insert(vbind.clone(), v.clone());
        if let Some(f) = filter {
            if !val_to_bool(&eval(f, &scope)?) {
                continue;
            }
        }
        out.insert(k.clone(), eval(body, &scope)?);
    }
    Ok(Value::Object(out))
}

// ── Temporal helpers (chrono + chrono-tz) ──
fn parse_duration_ms(s: &str) -> Result<i64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("duration(): empty string".into());
    }
    let bytes = s.as_bytes();
    let mut i = 0usize;
    let mut total_ms = 0f64;
    let mut found = false;
    while i < bytes.len() {
        let start = i;
        if bytes[i] == b'-' || bytes[i] == b'+' {
            i += 1;
        }
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            i += 1;
        }
        if i == start {
            return Err(format!("duration(): invalid number in '{}'", s));
        }
        let num: f64 = s[start..i]
            .parse()
            .map_err(|_| format!("duration(): bad number in '{}'", s))?;
        let ustart = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_digit()
            && bytes[i] != b'.'
            && bytes[i] != b'-'
            && bytes[i] != b'+'
        {
            i += 1;
        }
        let unit = &s[ustart..i];
        let ms = match unit {
            "h" => num * 3_600_000.0,
            "m" => num * 60_000.0,
            "s" => num * 1_000.0,
            "ms" => num,
            "us" | "µs" => num / 1_000.0,
            "ns" => num / 1_000_000.0,
            "" => return Err(format!("duration(): missing unit in '{}'", s)),
            other => return Err(format!("duration(): unknown unit '{}'", other)),
        };
        total_ms += ms;
        found = true;
    }
    if !found {
        return Err(format!("duration(): invalid '{}'", s));
    }
    Ok(total_ms.round() as i64)
}

fn temporal_accessor(name: &str, s: &str, tz: Option<&str>) -> Result<Value, String> {
    let dt = chrono::DateTime::parse_from_rfc3339(s)
        .map_err(|e| format!("{}(): invalid timestamp '{}': {}", name, s, e))?;
    let comp = match tz {
        Some(tzname) => {
            let zone: chrono_tz::Tz = tzname
                .parse()
                .map_err(|_| format!("{}(): unknown timezone '{}'", name, tzname))?;
            extract_component(name, &dt.with_timezone(&zone))
        }
        None => extract_component(name, &dt.with_timezone(&chrono::Utc)),
    };
    Ok(Value::Number(comp.into()))
}

// CEL-compatible component semantics: getMonth/getDayOfMonth/getDayOfYear are
// 0-based; getDate is 1-based; getDayOfWeek is 0-based with Sunday = 0.
fn extract_component<T: chrono::Datelike + chrono::Timelike>(name: &str, dt: &T) -> i64 {
    match name {
        "getYear" => dt.year() as i64,
        "getMonth" => dt.month0() as i64,
        "getDayOfMonth" => dt.day0() as i64,
        "getDate" => dt.day() as i64,
        "getDayOfWeek" => dt.weekday().num_days_from_sunday() as i64,
        "getDayOfYear" => dt.ordinal0() as i64,
        "getHours" => dt.hour() as i64,
        "getMinutes" => dt.minute() as i64,
        "getSeconds" => dt.second() as i64,
        "getMilliseconds" => (dt.nanosecond() / 1_000_000) as i64,
        _ => 0,
    }
}
