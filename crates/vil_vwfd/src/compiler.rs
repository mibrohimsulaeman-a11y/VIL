//! VIL VWFD Compiler — VWFD YAML → VilwGraph.
//!
//! Validates expressions, compiles vil_query to SQL, rejects unsupported features.

use crate::graph::*;
use crate::spec::*;
use std::collections::HashMap;

#[derive(Debug)]
pub struct CompileError {
    pub message: String,
    pub location: Option<String>,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        if let Some(ref loc) = self.location {
            write!(f, "{}: {}", loc, self.message)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

/// Compile VWFD YAML string → VilwGraph.
pub fn compile(yaml: &str) -> Result<VilwGraph, CompileError> {
    // 1. Parse YAML
    let doc: VwfdDocument = serde_yaml::from_str(yaml).map_err(|e| CompileError {
        message: format!("YAML parse: {}", e),
        location: None,
    })?;

    // 1b. Resolve and validate workflow dialect
    let dialect = doc
        .metadata
        .as_ref()
        .and_then(|m| m.dialect.clone())
        .unwrap_or_else(|| "vil".to_string())
        .to_lowercase();
    match dialect.as_str() {
        "vil" | "vflow" => {}
        other => {
            return Err(CompileError {
                message: format!("unknown dialect '{}'. Valid dialects: vil, vflow", other),
                location: Some("metadata.dialect".into()),
            });
        }
    }

    // 2. Build node index
    let mut node_map: HashMap<String, usize> = HashMap::new();
    let mut nodes = Vec::new();

    for (idx, act) in doc.spec.activities.iter().enumerate() {
        node_map.insert(act.id.clone(), idx);

        let kind = NodeKind::from_activity_type(&act.activity_type);
        let config = build_node_config(act);
        if kind == NodeKind::Validate && act.validate_config.is_none() {
            return Err(CompileError {
                message: "Validate activity requires validate_config".into(),
                location: Some(format!("activity.{}", act.id)),
            });
        }
        if kind == NodeKind::Compute {
            if act.compute_config.is_none() {
                return Err(CompileError {
                    message: "Compute activity requires compute_config".into(),
                    location: Some(format!("activity.{}", act.id)),
                });
            }
            let cc = act.compute_config.as_ref().unwrap();
            match cc.language.as_str() {
                "starlark" | "v-starlark" => {}
                other => {
                    return Err(CompileError {
                        message: format!(
                            "compute language '{}' not supported; use 'starlark' or 'v-starlark'",
                            other
                        ),
                        location: Some(format!("activity.{}.compute_config.language", act.id)),
                    });
                }
            }
            if cc.source.trim().is_empty() {
                return Err(CompileError {
                    message: "compute source must not be empty".into(),
                    location: Some(format!("activity.{}.compute_config.source", act.id)),
                });
            }
        }
        let mappings = compile_mappings(&act.input_mappings, &act.id)?;

        nodes.push(VilwNode {
            id: act.id.clone(),
            kind,
            output_variable: act.output_variable.clone(),
            durability: act.durability.clone(),
            config,
            mappings,
            compensation: act
                .compensation
                .as_ref()
                .map(|c| serde_json::to_value(c).unwrap_or_default()),
            audit_log: act
                .audit_log
                .as_ref()
                .map(|a| serde_json::to_value(a).unwrap_or_default()),
        });
    }

    // Add control nodes
    if let Some(ref controls) = doc.spec.controls {
        for ctrl in controls {
            let kind = match ctrl.control_type.as_deref() {
                Some("exclusive") => NodeKind::ExclusiveGateway,
                Some("inclusive") => NodeKind::InclusiveGateway,
                Some("parallel") => NodeKind::Parallel,
                Some("join") => NodeKind::Join,
                _ => NodeKind::Noop,
            };
            let idx = nodes.len();
            node_map.insert(ctrl.id.clone(), idx);
            nodes.push(VilwNode {
                id: ctrl.id.clone(),
                kind,
                output_variable: None,
                durability: None,
                config: serde_json::json!({}),
                mappings: Vec::new(),
                compensation: None,
                audit_log: None,
            });
        }
    }

    // Always add implicit End if not present
    if !nodes.iter().any(|n| n.kind == NodeKind::End) {
        let idx = nodes.len();
        node_map.insert("end".into(), idx);
        nodes.push(VilwNode {
            id: "end".into(),
            kind: NodeKind::End,
            output_variable: None,
            durability: None,
            config: serde_json::json!({}),
            mappings: Vec::new(),
            compensation: None,
            audit_log: None,
        });
    }

    // 3. Build edges
    let mut edges = Vec::new();
    for flow in &doc.spec.flows {
        let from_idx = node_map
            .get(&flow.from.node)
            .copied()
            .ok_or_else(|| CompileError {
                message: format!("flow {}: unknown from node '{}'", flow.id, flow.from.node),
                location: Some(format!("flow.{}", flow.id)),
            })?;
        let to_idx = node_map
            .get(&flow.to.node)
            .copied()
            .ok_or_else(|| CompileError {
                message: format!("flow {}: unknown to node '{}'", flow.id, flow.to.node),
                location: Some(format!("flow.{}", flow.id)),
            })?;

        // Validate guard condition if present.
        // In "vflow" dialect, bare-string conditions default to V-CEL (Phase 2),
        // so skip the vil_expr feature gate here.
        if let Some(ref cond) = flow.condition {
            if dialect == "vil" {
                vil_expr::check_supported(cond).map_err(|e| CompileError {
                    message: format!("guard condition: {}", e),
                    location: Some(format!("flow.{}.condition", flow.id)),
                })?;
            }
        }

        edges.push(VilwEdge {
            from_idx,
            to_idx,
            condition: flow.condition.clone(),
            priority: flow.priority.unwrap_or(0),
            detached: flow.detached.unwrap_or(false),
        });
    }

    // 4. Find entry node (first Trigger)
    let entry_node = nodes
        .iter()
        .position(|n| n.kind == NodeKind::Trigger)
        .ok_or_else(|| CompileError {
            message: "no Trigger activity found".into(),
            location: None,
        })?;

    // 5. Extract metadata
    let id = doc
        .metadata
        .as_ref()
        .and_then(|m| m.id.clone())
        .unwrap_or_else(|| "unnamed".into());
    let name = doc
        .metadata
        .as_ref()
        .and_then(|m| m.name.clone())
        .unwrap_or_else(|| id.clone());

    let trigger_node = &nodes[entry_node];
    let trigger_config: Option<TriggerConfig> =
        serde_json::from_value(trigger_node.config.clone()).ok();
    let webhook_route = trigger_config.as_ref().and_then(|tc| tc.webhook_path());
    let webhook_method = trigger_config
        .as_ref()
        .and_then(|tc| tc.webhook_config.as_ref())
        .and_then(|wc| wc.method.clone())
        .unwrap_or_else(|| "POST".into())
        .to_uppercase();
    let trigger_type = trigger_config
        .as_ref()
        .and_then(|tc| tc.trigger_type.clone())
        .unwrap_or_else(|| "webhook".into());

    let durability_default = doc
        .spec
        .durability
        .as_ref()
        .and_then(|d| d.default_mode.clone())
        .unwrap_or_else(|| "eventual".into());

    let variables = doc
        .spec
        .variables
        .as_ref()
        .map(|vars| vars.iter().filter_map(|v| v.name.clone()).collect())
        .unwrap_or_default();

    Ok(VilwGraph {
        id,
        name,
        nodes,
        edges,
        variables,
        entry_node,
        durability_default,
        webhook_route,
        webhook_method,
        trigger_type,
        dialect,
        audit_log: doc
            .spec
            .audit_log
            .as_ref()
            .map(|a| serde_json::to_value(a).unwrap_or_default()),
    })
}

fn build_node_config(act: &VwfdActivity) -> serde_json::Value {
    if let Some(ref tc) = act.trigger_config {
        return serde_json::to_value(tc).unwrap_or_default();
    }
    if let Some(ref cc) = act.connector_config {
        return serde_json::to_value(cc).unwrap_or_default();
    }
    if let Some(ref rc) = act.rule_config {
        return serde_json::to_value(rc).unwrap_or_default();
    }
    if let Some(ref etc) = act.end_trigger_config {
        return serde_json::to_value(etc).unwrap_or_default();
    }
    if let Some(ref lc) = act.loop_config {
        return serde_json::to_value(lc).unwrap_or_default();
    }
    if let Some(ref wc) = act.wasm_config {
        return serde_json::to_value(wc).unwrap_or_default();
    }
    if let Some(ref sc) = act.sidecar_config {
        return serde_json::to_value(sc).unwrap_or_default();
    }
    if let Some(ref sw) = act.sub_workflow_config {
        return serde_json::to_value(sw).unwrap_or_default();
    }
    if let Some(ref ht) = act.human_task_config {
        return serde_json::to_value(ht).unwrap_or_default();
    }
    if let Some(ref nc) = act.code_config {
        return serde_json::to_value(nc).unwrap_or_default();
    }
    if let Some(ref cc) = act.compute_config {
        return serde_json::to_value(cc).unwrap_or_default();
    }
    if let Some(ref vc) = act.validate_config {
        return serde_json::to_value(vc).unwrap_or_default();
    }
    if let Some(ref tc) = act.timer_config {
        return serde_json::to_value(tc).unwrap_or_default();
    }
    if let Some(ref sc) = act.signal_config {
        return serde_json::to_value(sc).unwrap_or_default();
    }
    if let Some(ref egc) = act.event_gateway_config {
        return serde_json::to_value(egc).unwrap_or_default();
    }
    serde_json::json!({})
}

fn compile_mappings(
    mappings: &Option<Vec<InputMapping>>,
    activity_id: &str,
) -> Result<Vec<CompiledMapping>, CompileError> {
    let Some(maps) = mappings else {
        return Ok(Vec::new());
    };
    let mut compiled = Vec::new();

    for m in maps {
        let target = m.target.as_deref().unwrap_or("").to_string();
        let source_obj = m.source.as_ref();
        let lang = source_obj
            .and_then(|s| s.language.as_deref())
            .unwrap_or("literal");
        let src = source_obj
            .and_then(|s| s.source.as_ref())
            .map(|v| match v {
                serde_yaml::Value::String(s) => s.clone(),
                other => format!("{:?}", other),
            })
            .unwrap_or_default();

        // Validate and compile based on language
        match lang {
            "literal" | "spv1" => {
                compiled.push(CompiledMapping {
                    target,
                    language: lang.into(),
                    source: src,
                    compiled_sql: None,
                    param_refs: None,
                    optional: None,
                });
            }
            "vil-expr" | "cel" | "v-cel" | "vcel" => {
                // H2: v-cel/vcel now share the real vil-expr compile path.
                // Validate VIL Expression expression is supported by vil_expr
                vil_expr::check_supported(&src).map_err(|e| CompileError {
                    message: e,
                    location: Some(format!(
                        "activity.{}.input_mappings.{}",
                        activity_id, target
                    )),
                })?;
                compiled.push(CompiledMapping {
                    target,
                    language: "vil-expr".into(),
                    source: src,
                    compiled_sql: None,
                    param_refs: None,
                    optional: None,
                });
            }
            "vil_query" => {
                // Compile VilQuery DSL → SQL + param_refs at compile time
                let (sql, param_refs, query_optional) =
                    compile_vil_query(&src).map_err(|e| CompileError {
                        message: format!("vil_query compile: {}", e),
                        location: Some(format!(
                            "activity.{}.input_mappings.{}",
                            activity_id, target
                        )),
                    })?;
                compiled.push(CompiledMapping {
                    target,
                    language: "vil_query".into(),
                    source: src,
                    compiled_sql: Some(sql),
                    param_refs: Some(param_refs),
                    optional: query_optional,
                });
            }
            // H2: v-cel/vcel are now handled by the "vil-expr" | "cel" arm above.
            "bytes_ref" => {
                // Accept now. Strip optional leading "$." from the variable ref;
                // runtime router reads the raw value from the variable slot.
                let source = src.strip_prefix("$.").map(|s| s.to_string()).unwrap_or(src);
                compiled.push(CompiledMapping {
                    target,
                    language: "bytes_ref".into(),
                    source,
                    compiled_sql: None,
                    param_refs: None,
                    optional: None,
                });
            }
            "starlark" | "v-starlark" => {
                // H3: replace with the real Compute/Starlark path.
                return Err(CompileError {
                    message: "language 'starlark' is supported from H3 (Compute activity); not yet enabled".into(),
                    location: Some(format!("activity.{}.input_mappings.{}", activity_id, target)),
                });
            }
            other => {
                return Err(CompileError {
                    message: format!(
                        "language '{}' not supported by vil compiler. \
                         Use vflow compile --cloud for vil-expr/vrule support.",
                        other
                    ),
                    location: Some(format!(
                        "activity.{}.input_mappings.{}",
                        activity_id, target
                    )),
                });
            }
        }
    }

    Ok(compiled)
}

/// Compile VilQuery inline DSL → SQL string + param_refs + optional metadata.
///
/// The compiler emits a concrete parameterized SQL string for the normal path.
/// If `.where_eq_if(col, ref)` is present, it also emits one alternate SQL plan
/// without that clause so the runtime can select it when `ref` resolves to
/// null/empty-string without recompiling SQL on the hot path.
fn compile_vil_query(
    source: &str,
) -> Result<(String, Vec<String>, Option<serde_json::Value>), String> {
    let mut dialect = "postgres".to_string();
    let mut chain_lines = Vec::new();

    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("dialect:") {
            dialect = rest
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_lowercase();
            continue;
        }
        chain_lines.push(trimmed);
    }

    let clean = chain_lines.join("");
    let calls = parse_query_chain(&clean)?;

    // Allow `.dialect("sqlite")` anywhere in the chain, while still validating
    // all dialect-specific methods against the final selected dialect.
    for (method, args) in &calls {
        if method == "dialect" {
            let arg = args.first().ok_or("dialect() requires one argument")?;
            dialect = unquote(arg).to_lowercase();
        }
    }
    validate_query_dialect(&dialect)?;

    let mut table = String::new();
    let mut columns = vec!["*".to_string()];
    let mut joins = Vec::new();
    let mut array_joins = Vec::new();
    let mut conditions: Vec<QueryCondition> = Vec::new();
    let mut group_by = Vec::new();
    let mut having = Vec::new();
    let mut order_clauses = Vec::new();
    let mut limit = LimitOffset::default();
    let mut offset = LimitOffset::default();
    let mut final_clause = false;
    let mut sample_clause: Option<String> = None;
    let mut limit_by: Option<String> = None;
    let mut allow_filtering = false;
    let mut optional_ref: Option<String> = None;

    for (method, args) in &calls {
        validate_query_method_for_dialect(&dialect, method)?;
        match method.as_str() {
            "dialect" => {}
            "select" => {
                table = require_arg(method, args, 0)?;
            }
            "columns" | "cols" => {
                columns = require_arg(method, args, 0)?
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if columns.is_empty() {
                    return Err("columns() requires at least one column".into());
                }
            }
            "select_expr" => {
                let expr = require_arg(method, args, 0)?;
                if columns == ["*".to_string()] {
                    columns.clear();
                }
                columns.push(expr);
            }
            "bucket_by_time" => {
                let bucket = require_arg(method, args, 0)?;
                let ts_col = require_arg(method, args, 1)?;
                if columns == ["*".to_string()] {
                    columns.clear();
                }
                columns.push(format!("time_bucket('{}', {}) AS bucket", bucket, ts_col));
            }
            "join" | "inner_join" => {
                let t = require_arg(method, args, 0)?;
                let on = args.get(1).map(|a| unquote(a)).unwrap_or_default();
                joins.push(format!("JOIN {} ON {}", t, on));
            }
            "left_join" => {
                let t = require_arg(method, args, 0)?;
                let on = args.get(1).map(|a| unquote(a)).unwrap_or_default();
                joins.push(format!("LEFT JOIN {} ON {}", t, on));
            }
            "where_eq" | "and_eq" => {
                push_bound_condition(&mut conditions, method, args, "=", false)?
            }
            "where_eq_if" => {
                if optional_ref.is_some() {
                    return Err("only one .where_eq_if(...) allowed per query (multiple optionals would need 2^N pre-built variants)".into());
                }
                let param_ref = classify_value(
                    args.get(1)
                        .ok_or_else(|| format!("{} requires argument 1", method))?,
                );
                optional_ref = Some(param_ref.clone());
                conditions.push(QueryCondition::Bound {
                    column: require_arg(method, args, 0)?,
                    op: "=".into(),
                    param_ref,
                    optional: true,
                });
            }
            "where_gt" => push_bound_condition(&mut conditions, method, args, ">", false)?,
            "where_gte" | "where_ge" => {
                push_bound_condition(&mut conditions, method, args, ">=", false)?
            }
            "where_lt" => push_bound_condition(&mut conditions, method, args, "<", false)?,
            "where_lte" | "where_le" => {
                push_bound_condition(&mut conditions, method, args, "<=", false)?
            }
            "where_ne" | "where_neq" => {
                push_bound_condition(&mut conditions, method, args, "!=", false)?
            }
            "where_like" => push_bound_condition(&mut conditions, method, args, "LIKE", false)?,
            "where_null" => conditions.push(QueryCondition::Raw(format!(
                "{} IS NULL",
                require_arg(method, args, 0)?
            ))),
            "where_not_null" => conditions.push(QueryCondition::Raw(format!(
                "{} IS NOT NULL",
                require_arg(method, args, 0)?
            ))),
            "where_raw" => conditions.push(QueryCondition::Raw(require_arg(method, args, 0)?)),
            "group_by" => group_by.push(require_arg(method, args, 0)?),
            "having" => having.push(require_arg(method, args, 0)?),
            "order_by" => order_clauses.push(require_arg(method, args, 0)?),
            "order_by_asc" => order_clauses.push(format!("{} ASC", require_arg(method, args, 0)?)),
            "order_by_desc" => {
                order_clauses.push(format!("{} DESC", require_arg(method, args, 0)?))
            }
            "limit" => limit.literal = Some(parse_i64_arg(method, args, 0)?),
            "offset" => offset.literal = Some(parse_i64_arg(method, args, 0)?),
            "limit_var" => {
                limit.param_ref = Some(classify_value(
                    args.first().ok_or("limit_var requires argument 0")?,
                ))
            }
            "offset_var" => {
                offset.param_ref = Some(classify_value(
                    args.first().ok_or("offset_var requires argument 0")?,
                ))
            }
            "final_clause" => final_clause = true,
            "sample" => sample_clause = Some(require_arg(method, args, 0)?),
            "array_join" => array_joins.push(require_arg(method, args, 0)?),
            "limit_by" => {
                let n = parse_i64_arg(method, args, 0)?;
                let cols = require_arg(method, args, 1)?;
                limit_by = Some(format!("LIMIT {} BY {}", n, cols));
            }
            "allow_filtering" => allow_filtering = true,
            other => return Err(format!("unsupported vil_query method '{}'", other)),
        }
    }

    if table.trim().is_empty() {
        return Err("select(table) is required".into());
    }

    let plan = QuerySqlPlan {
        dialect: &dialect,
        table: &table,
        columns: &columns,
        joins: &joins,
        array_joins: &array_joins,
        conditions: &conditions,
        group_by: &group_by,
        having: &having,
        order_clauses: &order_clauses,
        limit: &limit,
        offset: &offset,
        final_clause,
        sample_clause: sample_clause.as_deref(),
        limit_by: limit_by.as_deref(),
        allow_filtering,
    };

    let (sql, param_refs) = build_query_sql(&plan, true);
    let optional = optional_ref.map(|if_param_ref| {
        let (alt_sql, alt_param_refs) = build_query_sql(&plan, false);
        serde_json::json!({
            "strategy": "where_eq_if_null_or_empty",
            "if_param_ref": if_param_ref,
            "alt_sql": alt_sql,
            "alt_param_refs": alt_param_refs,
        })
    });

    Ok((sql, param_refs, optional))
}

#[derive(Debug, Clone)]
enum QueryCondition {
    Raw(String),
    Bound {
        column: String,
        op: String,
        param_ref: String,
        optional: bool,
    },
}

#[derive(Debug, Clone, Default)]
struct LimitOffset {
    literal: Option<i64>,
    param_ref: Option<String>,
}

struct QuerySqlPlan<'a> {
    dialect: &'a str,
    table: &'a str,
    columns: &'a [String],
    joins: &'a [String],
    array_joins: &'a [String],
    conditions: &'a [QueryCondition],
    group_by: &'a [String],
    having: &'a [String],
    order_clauses: &'a [String],
    limit: &'a LimitOffset,
    offset: &'a LimitOffset,
    final_clause: bool,
    sample_clause: Option<&'a str>,
    limit_by: Option<&'a str>,
    allow_filtering: bool,
}

fn build_query_sql(plan: &QuerySqlPlan<'_>, include_optional: bool) -> (String, Vec<String>) {
    let mut bind_counter = 0usize;
    let mut param_refs = Vec::new();
    let mut sql = format!("SELECT {} FROM {}", plan.columns.join(", "), plan.table);

    if plan.final_clause {
        sql.push_str(" FINAL");
    }
    if let Some(sample) = plan.sample_clause {
        sql.push_str(" SAMPLE ");
        sql.push_str(sample);
    }
    for j in plan.joins {
        sql.push(' ');
        sql.push_str(j);
    }
    for aj in plan.array_joins {
        sql.push_str(" ARRAY JOIN ");
        sql.push_str(aj);
    }

    let mut condition_sql = Vec::new();
    for cond in plan.conditions {
        match cond {
            QueryCondition::Raw(raw) => condition_sql.push(raw.clone()),
            QueryCondition::Bound {
                column,
                op,
                param_ref,
                optional,
            } => {
                if *optional && !include_optional {
                    continue;
                }
                bind_counter += 1;
                condition_sql.push(format!(
                    "{} {} {}",
                    column,
                    op,
                    placeholder(plan.dialect, bind_counter)
                ));
                param_refs.push(param_ref.clone());
            }
        }
    }
    if !condition_sql.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&condition_sql.join(" AND "));
    }
    if !plan.group_by.is_empty() {
        sql.push_str(" GROUP BY ");
        sql.push_str(&plan.group_by.join(", "));
    }
    if !plan.having.is_empty() {
        sql.push_str(" HAVING ");
        sql.push_str(&plan.having.join(" AND "));
    }
    if !plan.order_clauses.is_empty() {
        sql.push_str(" ORDER BY ");
        sql.push_str(&plan.order_clauses.join(", "));
    }
    append_limit_offset(
        &mut sql,
        &mut param_refs,
        &mut bind_counter,
        plan.dialect,
        "LIMIT",
        plan.limit,
    );
    append_limit_offset(
        &mut sql,
        &mut param_refs,
        &mut bind_counter,
        plan.dialect,
        "OFFSET",
        plan.offset,
    );
    if let Some(limit_by) = plan.limit_by {
        sql.push(' ');
        sql.push_str(limit_by);
    }
    if plan.allow_filtering {
        sql.push_str(" ALLOW FILTERING");
    }

    (sql, param_refs)
}

fn append_limit_offset(
    sql: &mut String,
    param_refs: &mut Vec<String>,
    bind_counter: &mut usize,
    dialect: &str,
    keyword: &str,
    spec: &LimitOffset,
) {
    if let Some(literal) = spec.literal {
        sql.push_str(&format!(" {} {}", keyword, literal));
    } else if let Some(ref param_ref) = spec.param_ref {
        *bind_counter += 1;
        sql.push_str(&format!(
            " {} {}",
            keyword,
            placeholder(dialect, *bind_counter)
        ));
        param_refs.push(param_ref.clone());
    }
}

fn placeholder(dialect: &str, idx: usize) -> String {
    match dialect {
        "mysql" | "sqlite" => "?".to_string(),
        _ => format!("${}", idx),
    }
}

fn require_arg(method: &str, args: &[String], idx: usize) -> Result<String, String> {
    args.get(idx)
        .map(|s| unquote(s))
        .ok_or_else(|| format!("{} requires argument {}", method, idx))
}

fn parse_i64_arg(method: &str, args: &[String], idx: usize) -> Result<i64, String> {
    let raw = args
        .get(idx)
        .ok_or_else(|| format!("{} requires argument {}", method, idx))?;
    raw.trim()
        .parse::<i64>()
        .map_err(|_| format!("{} argument {} must be an integer literal", method, idx))
}

fn push_bound_condition(
    conditions: &mut Vec<QueryCondition>,
    method: &str,
    args: &[String],
    op: &str,
    optional: bool,
) -> Result<(), String> {
    let column = require_arg(method, args, 0)?;
    let param_ref = classify_value(
        args.get(1)
            .ok_or_else(|| format!("{} requires argument 1", method))?,
    );
    conditions.push(QueryCondition::Bound {
        column,
        op: op.into(),
        param_ref,
        optional,
    });
    Ok(())
}

fn validate_query_dialect(dialect: &str) -> Result<(), String> {
    match dialect {
        "postgres" | "mysql" | "sqlite" | "clickhouse" | "cassandra" => Ok(()),
        other => Err(format!("unsupported vil_query dialect '{}'; expected postgres, mysql, sqlite, clickhouse, or cassandra", other)),
    }
}

fn validate_query_method_for_dialect(dialect: &str, method: &str) -> Result<(), String> {
    const CASSANDRA_ONLY: &[&str] = &["allow_filtering"];
    const CLICKHOUSE_ONLY: &[&str] = &["final_clause", "sample", "array_join", "limit_by"];
    const POSTGRES_ONLY: &[&str] = &["bucket_by_time"];

    if CASSANDRA_ONLY.contains(&method) && dialect != "cassandra" {
        return Err(format!(
            "vil_query method '{}' is cassandra-only (current dialect: {})",
            method, dialect
        ));
    }
    if CLICKHOUSE_ONLY.contains(&method) && dialect != "clickhouse" {
        return Err(format!(
            "vil_query method '{}' is clickhouse-only (current dialect: {})",
            method, dialect
        ));
    }
    if POSTGRES_ONLY.contains(&method) && dialect != "postgres" {
        return Err(format!(
            "vil_query method '{}' is postgres-only (current dialect: {})",
            method, dialect
        ));
    }
    Ok(())
}

fn classify_value(raw: &str) -> String {
    let t = raw.trim();
    if (t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')) {
        format!("_literal_str:{}", &t[1..t.len() - 1])
    } else if t.parse::<i64>().is_ok() || t.parse::<f64>().is_ok() {
        format!("_literal_num:{}", t)
    } else if t == "true" || t == "false" {
        format!("_literal_bool:{}", t)
    } else {
        t.to_string() // variable reference
    }
}

// ── VilQuery DSL Parser (same as vflow_compiler) ──

fn parse_query_chain(src: &str) -> Result<Vec<(String, Vec<String>)>, String> {
    let mut result = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let len = chars.len();
    let mut pos = 0;
    while pos < len && chars[pos].is_whitespace() {
        pos += 1;
    }
    loop {
        if pos >= len {
            break;
        }
        if chars[pos] == '.' {
            pos += 1;
            while pos < len && chars[pos].is_whitespace() {
                pos += 1;
            }
        }
        let ns = pos;
        while pos < len && (chars[pos].is_alphanumeric() || chars[pos] == '_') {
            pos += 1;
        }
        if pos == ns {
            break;
        }
        let method: String = chars[ns..pos].iter().collect();
        while pos < len && chars[pos].is_whitespace() {
            pos += 1;
        }
        if pos >= len || chars[pos] != '(' {
            return Err(format!("expected '(' after '{}'", method));
        }
        pos += 1;
        let args = parse_query_args(&chars, &mut pos)?;
        result.push((method, args));
        while pos < len && chars[pos].is_whitespace() {
            pos += 1;
        }
    }
    Ok(result)
}

fn parse_query_args(chars: &[char], pos: &mut usize) -> Result<Vec<String>, String> {
    let len = chars.len();
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth = 1;
    let mut in_string = false;
    let mut string_char = '"';
    while *pos < len && depth > 0 {
        let ch = chars[*pos];
        if in_string {
            current.push(ch);
            if ch == string_char && (*pos == 0 || chars[*pos - 1] != '\\') {
                in_string = false;
            }
        } else {
            match ch {
                '"' | '\'' => {
                    in_string = true;
                    string_char = ch;
                    current.push(ch);
                }
                '(' => {
                    depth += 1;
                    current.push(ch);
                }
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        let t = current.trim().to_string();
                        if !t.is_empty() {
                            args.push(t);
                        }
                    } else {
                        current.push(ch);
                    }
                }
                ',' if depth == 1 => {
                    args.push(current.trim().to_string());
                    current.clear();
                }
                _ => {
                    current.push(ch);
                }
            }
        }
        *pos += 1;
    }
    Ok(args)
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    if (t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')) {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_WORKFLOW: &str = r#"
version: "3.0"
metadata:
  id: test-simple
  name: "Simple Test"
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config:
        trigger_type: webhook
        response_mode: buffered
        end_activity: respond
        webhook_config:
          path: /test
      output_variable: trigger_payload

    - id: transform
      activity_type: Connector
      connector_config:
        connector_ref: vastar.http
        operation: post
      input_mappings:
        - target: url
          source:
            language: literal
            source: "http://api.example.com"
        - target: body
          source:
            language: vil-expr
            source: '{"name": trigger_payload.name, "active": true}'
      output_variable: result

    - id: respond
      activity_type: EndTrigger
      end_trigger_config:
        trigger_ref: trigger
        final_response:
          language: vil-expr
          source: 'result'

    - id: end
      activity_type: End

  flows:
    - id: f1
      from: { node: trigger }
      to: { node: transform }
    - id: f2
      from: { node: transform }
      to: { node: respond }
    - id: f3
      from: { node: respond }
      to: { node: end }

  variables:
    - name: trigger_payload
      type: object
    - name: result
      type: object
"#;

    #[test]
    fn test_compile_simple() {
        let graph = compile(SIMPLE_WORKFLOW).unwrap();
        assert_eq!(graph.id, "test-simple");
        assert_eq!(graph.nodes.len(), 4);
        assert_eq!(graph.edges.len(), 3);
        assert_eq!(graph.entry_node, 0);
        assert_eq!(graph.webhook_route, Some("/test".into()));
        assert_eq!(graph.trigger_type, "webhook");
    }

    #[test]
    fn test_compile_mappings() {
        let graph = compile(SIMPLE_WORKFLOW).unwrap();
        let transform_node = &graph.nodes[1]; // transform
        assert_eq!(transform_node.mappings.len(), 2);
        assert_eq!(transform_node.mappings[0].language, "literal");
        assert_eq!(transform_node.mappings[1].language, "vil-expr");
    }

    #[test]
    fn test_compile_variables() {
        let graph = compile(SIMPLE_WORKFLOW).unwrap();
        assert!(graph.variables.contains(&"trigger_payload".to_string()));
        assert!(graph.variables.contains(&"result".to_string()));
    }

    #[test]
    fn test_reject_unsupported_vcel() {
        let yaml = r#"
version: "3.0"
metadata:
  id: test-reject
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config:
        trigger_type: webhook
        webhook_config:
          path: /test
      output_variable: trigger_payload
    - id: transform
      activity_type: Connector
      connector_config:
        connector_ref: vastar.http
        operation: post
      input_mappings:
        - target: body
          source:
            language: vil-expr
            source: 'data.map(x, x * 2)'
    - id: end
      activity_type: End
  flows:
    - id: f1
      from: { node: trigger }
      to: { node: transform }
    - id: f2
      from: { node: transform }
      to: { node: end }
"#;
        let result = compile(yaml);
        assert!(
            result.is_ok(),
            "V-CEL lambda mappings are supported after H2: {result:?}"
        );
    }

    const VIL_QUERY_WORKFLOW: &str = r#"
version: "3.0"
metadata:
  id: test-vilquery
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config:
        trigger_type: webhook
        webhook_config:
          path: /query
      output_variable: trigger_payload
    - id: query
      activity_type: Connector
      connector_config:
        connector_ref: vastar.db.postgres
        operation: raw_query
      input_mappings:
        - target: query
          source:
            language: vil_query
            source: |
              select("users")
                .columns("id, name")
                .where_gt("score", trigger_payload.min_score)
                .order_by_desc("score")
                .limit(10)
      output_variable: query_result
    - id: end
      activity_type: End
  flows:
    - id: f1
      from: { node: trigger }
      to: { node: query }
    - id: f2
      from: { node: query }
      to: { node: end }
"#;

    #[test]
    fn test_compile_vil_query() {
        let graph = compile(VIL_QUERY_WORKFLOW).unwrap();
        let query_node = &graph.nodes[1];
        assert_eq!(query_node.mappings.len(), 1);
        assert_eq!(query_node.mappings[0].language, "vil_query");
        let sql = query_node.mappings[0].compiled_sql.as_ref().unwrap();
        assert!(sql.contains("SELECT id, name FROM users"));
        assert!(sql.contains("WHERE score > $1"));
        assert!(sql.contains("ORDER BY score DESC"));
        assert!(sql.contains("LIMIT 10"));
        let refs = query_node.mappings[0].param_refs.as_ref().unwrap();
        assert_eq!(refs[0], "trigger_payload.min_score");
    }

    const VFLOW_MINIMAL: &str = r#"
version: "3.0"
metadata:
  id: hello-workflow
  name: "Hello VFlow"
  dialect: vflow
spec:
  activities:
    - id: trig
      activity_type: Trigger
      trigger_config:
        trigger_type: webhook
        response_mode: buffered
        end_activity: respond
        webhook_config:
          path: /api/hello
      output_variable: trigger_payload
    - id: respond
      activity_type: EndTrigger
      end_trigger_config:
        trigger_ref: trig
        final_response:
          language: literal
          source: '{"_status": 200, "body": {"ok": true}}'
  flows:
    - id: f1
      from: { node: trig }
      to: { node: respond }
"#;

    const VCEL_COND: &str = r#"
version: "3.0"
metadata:
  id: vcel-cond
  dialect: vflow
spec:
  activities:
    - id: trig
      activity_type: Trigger
      trigger_config:
        trigger_type: webhook
        webhook_config:
          path: /c
      output_variable: trigger_payload
    - id: gate
      activity_type: Connector
      connector_config:
        connector_ref: vastar.http
        operation: post
    - id: end
      activity_type: End
  flows:
    - id: f1
      from: { node: trig }
      to: { node: gate }
    - id: f2
      from: { node: gate }
      to: { node: end }
      condition: 'items.map(i, i.x).size() > 0'
"#;

    #[test]
    fn test_compile_vflow_dialect() {
        let graph = compile(VFLOW_MINIMAL).unwrap();
        assert_eq!(graph.dialect, "vflow");
        assert_eq!(graph.id, "hello-workflow");
    }

    #[test]
    fn test_default_dialect_is_vil() {
        let graph = compile(SIMPLE_WORKFLOW).unwrap();
        assert_eq!(graph.dialect, "vil");
    }

    #[test]
    fn test_unknown_dialect_rejected() {
        let yaml = VFLOW_MINIMAL.replace("dialect: vflow", "dialect: bogus");
        let result = compile(&yaml);
        assert!(result.is_err());
        let msg = result.unwrap_err().message;
        assert!(msg.contains("vil") && msg.contains("vflow"), "got: {}", msg);
    }

    #[test]
    fn test_vflow_bare_condition_is_vcel() {
        let graph = compile(VCEL_COND).unwrap();
        assert_eq!(graph.dialect, "vflow");
        assert!(graph.edges.iter().any(|e| e.condition.is_some()));
    }

    #[test]
    fn test_vil_bare_condition_still_validated() {
        let yaml = VCEL_COND
            .replace("dialect: vflow", "dialect: vil")
            .replace("items.map(i, i.x).size() > 0", "items.map(i, i.x");
        let result = compile(&yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_vcel_mapping_compiles() {
        // H2: v-cel mappings now compile through the real vil-expr path.
        let yaml = SIMPLE_WORKFLOW.replace("language: vil-expr", "language: v-cel");
        let graph = compile(&yaml).unwrap();
        assert_eq!(graph.nodes[1].mappings[1].language, "vil-expr");
    }

    #[test]
    fn test_vcel_lambda_compile_smoke() {
        // The dedicated example workflow file is absent in this tree, so we
        // exercise a lambda comprehension inline to prove V-CEL compiles e2e.
        let yaml = SIMPLE_WORKFLOW
            .replace("language: vil-expr", "language: v-cel")
            .replace(
                "'{\"name\": trigger_payload.name, \"active\": true}'",
                "'trigger_payload.items.filter(i, i.price > 100000).map(i, i.name)'",
            );
        let graph = compile(&yaml).unwrap();
        assert_eq!(graph.nodes[1].mappings[1].language, "vil-expr");
    }

    #[test]
    fn test_starlark_mapping_staged_error() {
        let yaml = SIMPLE_WORKFLOW.replace("language: vil-expr", "language: starlark");
        let result = compile(&yaml);
        assert!(result.is_err());
        let msg = result.unwrap_err().message;
        assert!(msg.contains("H3"), "got: {}", msg);
    }

    const COMPUTE_STARLARK_019: &str = r#"
version: "3.0"
metadata:
  id: compute-starlark
  name: "019 Compute Starlark"
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config:
        trigger_type: webhook
        response_mode: buffered
        end_activity: respond
        webhook_config:
          path: /compute
      output_variable: trigger_payload
    - id: pricing
      activity_type: Compute
      compute_config:
        language: v-starlark
        entry_fn: run
        budget_profile: balanced
        timeout_ms: 3000
        source: |
          def run(ctx):
              subtotal = ctx["body"]["subtotal"]
              tiers = ctx["body"]["tiers"]
              discount_pct = 0
              for tier in tiers:
                  if subtotal >= tier["min"]:
                      discount_pct = tier["pct"]
              final_price = subtotal * (100 - discount_pct) / 100
              return {"final_price": final_price, "discount_pct": discount_pct}
      input_mappings:
        - target: subtotal
          source:
            language: spv1
            source: "$.trigger_payload.body.subtotal"
        - target: tiers
          source:
            language: spv1
            source: "$.trigger_payload.body.tiers"
      output_variable: price_quote
    - id: respond
      activity_type: EndTrigger
      end_trigger_config:
        trigger_ref: trigger
        final_response:
          language: vil-expr
          source: price_quote
    - id: end
      activity_type: End
  flows:
    - id: f1
      from: { node: trigger }
      to: { node: pricing }
    - id: f2
      from: { node: pricing }
      to: { node: respond }
    - id: f3
      from: { node: respond }
      to: { node: end }
"#;

    #[test]
    fn test_compile_019_compute_starlark() {
        let graph = compile(COMPUTE_STARLARK_019).unwrap();
        let pricing = graph.nodes.iter().find(|n| n.id == "pricing").unwrap();
        assert_eq!(pricing.kind, NodeKind::Compute);
        assert_eq!(pricing.config["language"], "v-starlark");
        assert_eq!(pricing.config["entry"], "run");
        assert_eq!(pricing.mappings.len(), 2);
    }

    #[test]
    fn test_graph_serialization() {
        let graph = compile(SIMPLE_WORKFLOW).unwrap();
        let bytes = graph.to_bytes();
        assert!(!bytes.is_empty());
        let restored = crate::graph::VilwGraph::from_bytes(&bytes).unwrap();
        assert_eq!(restored.id, graph.id);
        assert_eq!(restored.nodes.len(), graph.nodes.len());
    }

    const H4_CONTROL_NODES: &str = r#"
version: "3.0"
metadata:
  id: h4-control
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config:
        trigger_type: webhook
        webhook_config:
          path: /h4
      output_variable: trigger_payload
    - id: validate
      activity_type: Validate
      validate_config:
        target: trigger_payload
        schema:
          type: object
      output_variable: validated
    - id: wait
      activity_type: Timer
      timer_config:
        delay_ms: 0
      output_variable: timer_result
    - id: signal
      activity_type: Signal
      signal_config:
        signal: approved
      output_variable: signal_result
    - id: gateway
      activity_type: EventGateway
      event_gateway_config:
        event: approved
        timeout_ms: 1
      output_variable: gateway_result
    - id: end
      activity_type: End
  flows:
    - { id: f1, from: { node: trigger }, to: { node: validate } }
    - { id: f2, from: { node: validate }, to: { node: wait } }
    - { id: f3, from: { node: wait }, to: { node: signal } }
    - { id: f4, from: { node: signal }, to: { node: gateway } }
    - { id: f5, from: { node: gateway }, to: { node: end } }
"#;

    #[test]
    fn test_compile_h4_control_nodes() {
        let graph = compile(H4_CONTROL_NODES).unwrap();
        assert!(graph.nodes.iter().any(|n| n.kind == NodeKind::Validate));
        assert!(graph.nodes.iter().any(|n| n.kind == NodeKind::Timer));
        assert!(graph.nodes.iter().any(|n| n.kind == NodeKind::Signal));
        assert!(graph.nodes.iter().any(|n| n.kind == NodeKind::EventGateway));
        let validate = graph.nodes.iter().find(|n| n.id == "validate").unwrap();
        assert_eq!(validate.config["target"], "trigger_payload");
        let gateway = graph.nodes.iter().find(|n| n.id == "gateway").unwrap();
        assert_eq!(gateway.config["event"], "approved");
    }

    const H4_VIL_QUERY_ADVANCED: &str = r#"
version: "3.0"
metadata:
  id: h4-query
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config:
        trigger_type: webhook
        webhook_config:
          path: /query
      output_variable: trigger_payload
    - id: query
      activity_type: Connector
      connector_config:
        connector_ref: vastar.db.postgres
        operation: raw_query
      input_mappings:
        - target: query
          source:
            language: vil_query
            source: |
              dialect: postgres
              select("orders")
                .columns("customer_id, count(*) AS total")
                .where_eq_if("status", trigger_payload.status)
                .group_by("customer_id")
                .having("count(*) > 0")
                .limit_var(trigger_payload.limit)
                .offset_var(trigger_payload.offset)
      output_variable: rows
    - id: end
      activity_type: End
  flows:
    - { id: f1, from: { node: trigger }, to: { node: query } }
    - { id: f2, from: { node: query }, to: { node: end } }
"#;

    #[test]
    fn test_compile_vil_query_h4_advanced_optional() {
        let graph = compile(H4_VIL_QUERY_ADVANCED).unwrap();
        let mapping = &graph.nodes[1].mappings[0];
        let sql = mapping.compiled_sql.as_ref().unwrap();
        assert!(
            sql.contains("SELECT customer_id, count(*) AS total FROM orders"),
            "{}",
            sql
        );
        assert!(sql.contains("WHERE status = $1"), "{}", sql);
        assert!(sql.contains("GROUP BY customer_id"), "{}", sql);
        assert!(sql.contains("HAVING count(*) > 0"), "{}", sql);
        assert!(sql.contains("LIMIT $2 OFFSET $3"), "{}", sql);
        assert_eq!(
            mapping.param_refs.as_ref().unwrap(),
            &vec![
                "trigger_payload.status".to_string(),
                "trigger_payload.limit".to_string(),
                "trigger_payload.offset".to_string(),
            ]
        );
        let optional = mapping.optional.as_ref().unwrap();
        assert_eq!(optional["if_param_ref"], "trigger_payload.status");
        assert_eq!(optional["alt_param_refs"][0], "trigger_payload.limit");
        assert_eq!(optional["alt_param_refs"][1], "trigger_payload.offset");
        assert!(!optional["alt_sql"]
            .as_str()
            .unwrap()
            .contains("WHERE status"));
    }

    #[test]
    fn test_vil_query_dialect_specific_methods_rejected() {
        let yaml = H4_VIL_QUERY_ADVANCED
            .replace("dialect: postgres", "dialect: mysql")
            .replace(
                ".group_by(\"customer_id\")",
                ".final_clause()\n                .group_by(\"customer_id\")",
            );
        let err = compile(&yaml).unwrap_err();
        assert!(
            err.message.contains("clickhouse-only"),
            "got: {}",
            err.message
        );
    }

    fn compile_ref_example(name: &str, yaml: &str) -> VilwGraph {
        compile(yaml).unwrap_or_else(|e| panic!("reference example {name} must compile: {e}"))
    }

    #[test]
    fn test_h4_reference_examples_compile() {
        let refs = [
            (
                "011-vil-query-sql",
                include_str!("../../../docs-dev-jangan-ditrack/vil-compat-gap/aa-dev-ref/examples-vflow/provision-pattern/011-vil-query-sql/workflow.yaml"),
            ),
            (
                "014-control-flow-node-matrix",
                include_str!("../../../docs-dev-jangan-ditrack/vil-compat-gap/aa-dev-ref/examples-vflow/provision-pattern/014-control-flow-node-matrix/workflow.yaml"),
            ),
            (
                "015-trigger-matrix",
                include_str!("../../../docs-dev-jangan-ditrack/vil-compat-gap/aa-dev-ref/examples-vflow/provision-pattern/015-trigger-matrix/workflow.yaml"),
            ),
            (
                "016-connector-matrix",
                include_str!("../../../docs-dev-jangan-ditrack/vil-compat-gap/aa-dev-ref/examples-vflow/provision-pattern/016-connector-matrix/workflow.yaml"),
            ),
            (
                "020-retail-order-intake",
                include_str!("../../../docs-dev-jangan-ditrack/vil-compat-gap/aa-dev-ref/examples-vflow/provision-pattern/020-retail-order-intake/workflow.yaml"),
            ),
            (
                "023-human-approval-event-gateway",
                include_str!("../../../docs-dev-jangan-ditrack/vil-compat-gap/aa-dev-ref/examples-vflow/provision-pattern/023-human-approval-event-gateway/workflow.yaml"),
            ),
            (
                "025-internal-grpc-proto-contract",
                include_str!("../../../docs-dev-jangan-ditrack/vil-compat-gap/aa-dev-ref/examples-vflow/provision-pattern/025-internal-grpc-proto-contract/workflow.yaml"),
            ),
            (
                "030-audit-observability-timeline",
                include_str!("../../../docs-dev-jangan-ditrack/vil-compat-gap/aa-dev-ref/examples-vflow/provision-pattern/030-audit-observability-timeline/workflow.yaml"),
            ),
        ];
        for (name, yaml) in refs {
            let graph = compile_ref_example(name, yaml);
            assert!(
                graph.node_count() >= 2,
                "{name} should compile to a non-trivial graph"
            );
        }
    }

    #[test]
    fn test_h4d_audit_log_compiles_and_propagates() {
        let yaml = r#"
version: "3.0"
metadata: { id: h4-audit }
spec:
  audit_log:
    events: [workflow_started, workflow_succeeded, activity_started, activity_succeeded]
    mode: async_best_effort
    sinks:
      - type: webhook
        url: http://audit.local/events
      - type: nats
        subject: audit.events
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config: { trigger_type: webhook, webhook_config: { path: /audit } }
      output_variable: trigger_payload
    - id: call
      activity_type: Connector
      audit_log:
        events: [activity_started, activity_succeeded]
        mode: async_best_effort
      connector_config: { connector_ref: vastar.http, operation: post }
      output_variable: call_result
    - id: end
      activity_type: End
  flows:
    - { id: f1, from: { node: trigger }, to: { node: call } }
    - { id: f2, from: { node: call }, to: { node: end } }
"#;
        let graph = compile(yaml).unwrap();
        assert_eq!(
            graph.audit_log.as_ref().unwrap()["mode"],
            "async_best_effort"
        );
        assert_eq!(
            graph.audit_log.as_ref().unwrap()["sinks"][0]["type"],
            "webhook"
        );
        assert_eq!(
            graph.nodes[1].audit_log.as_ref().unwrap()["events"][0],
            "activity_started"
        );
    }

    #[test]
    fn test_h4e_trigger_matrix_compile_contract() {
        let cases = [
            ("nats_js", "nats_js: { stream: orders, subject: orders.created }"),
            ("nats_kv", "nats_kv: { bucket: kv, key: orders.* }"),
            ("cdc", "cdc: { source: postgres, table: orders }"),
            ("db_poll", "db_poll: { table: orders, interval_ms: 1000 }"),
            ("fs", "fs: { path: /tmp/inbox, pattern: '*.json' }"),
            ("grpc", "grpc: { service: OrderService, method: Create }\n        body_schema: { type: object }\n        proto_field: payload"),
        ];
        for (trigger_type, extra) in cases {
            let yaml = format!(
                r#"
version: "3.0"
metadata: {{ id: h4-trigger-{trigger_type} }}
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config:
        trigger_type: {trigger_type}
        {extra}
      output_variable: trigger_payload
    - id: end
      activity_type: End
  flows:
    - {{ id: f1, from: {{ node: trigger }}, to: {{ node: end }} }}
"#
            );
            let graph = compile(&yaml).unwrap_or_else(|e| panic!("{trigger_type}: {e}"));
            assert_eq!(graph.trigger_type, trigger_type);
        }
    }

    #[test]
    fn test_h4f_connector_matrix_compile_contract() {
        let connectors = [
            ("http", "http"),
            ("grpc", "grpc"),
            ("postgres", "sql"),
            ("redis", "redis"),
            ("mongo", "mongo"),
            ("cassandra", "cassandra"),
            ("clickhouse", "clickhouse"),
            ("elastic", "elastic"),
            ("s3", "s3"),
            ("gcs", "gcs"),
            ("azure", "azure"),
            ("nats", "nats"),
            ("kafka", "kafka"),
            ("mqtt", "mqtt"),
            ("rabbitmq", "rabbitmq"),
            ("protobuf", "codec"),
            ("msgpack", "codec"),
            ("iso8583", "codec"),
            ("modbus", "codec"),
            ("opcua", "codec"),
        ];
        let mut activities = String::from(
            r#"
    - id: trigger
      activity_type: Trigger
      trigger_config: { trigger_type: webhook, webhook_config: { path: /matrix } }
      output_variable: trigger_payload
"#,
        );
        let mut flows = String::new();
        let mut prev = "trigger".to_string();
        for (idx, (name, field)) in connectors.iter().enumerate() {
            let id = format!("c{}", idx);
            activities.push_str(&format!(
                r#"
    - id: {id}
      activity_type: Connector
      connector_config:
        connector_type: {name}
        connector_ref: vastar.{name}
        operation: call
        {field}: {{ enabled: true }}
        params: {{ sample: true }}
      output_variable: {id}_out
"#
            ));
            flows.push_str(&format!(
                "    - {{ id: f{idx}, from: {{ node: {prev} }}, to: {{ node: {id} }} }}\n"
            ));
            prev = id;
        }
        activities.push_str("\n    - id: end\n      activity_type: End\n");
        flows.push_str(&format!(
            "    - {{ id: fend, from: {{ node: {prev} }}, to: {{ node: end }} }}\n"
        ));
        let yaml = format!("version: \"3.0\"\nmetadata: {{ id: h4-connectors }}\nspec:\n  activities:{activities}\n  flows:\n{flows}");
        let graph = compile(&yaml).unwrap();
        assert_eq!(
            graph
                .nodes
                .iter()
                .filter(|n| n.kind == NodeKind::Connector)
                .count(),
            connectors.len()
        );
    }
}
