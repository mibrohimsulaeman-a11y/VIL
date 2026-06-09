//! VwfdExecutor — async workflow execution engine for VIL.
//!
//! Walks VilwGraph nodes following edges, evaluates mappings via eval_bridge,
//! dispatches connector calls (async), handles loops/guards/ErrorBoundary.
//!
//! Control flow follows vflow kernel pattern:
//! - Loops: walk full body subgraph per iteration (not just 1 node)
//! - ErrorBoundary: walk body subgraph, catch errors → error edge
//! - Parallel: tokio::join! for actual parallel branches
//! - Gateway: guard condition evaluation with priority

use crate::eval_bridge;
use crate::graph::*;
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Execution result.
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub output: Value,
    pub variables: HashMap<String, Value>,
    pub steps: u32,
}

/// Execution error.
#[derive(Debug)]
pub struct ExecError {
    pub message: String,
    pub node_id: Option<String>,
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        if let Some(ref nid) = self.node_id {
            write!(f, "node '{}': {}", nid, self.message)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

/// Async connector dispatch function.
pub type ConnectorFn = Arc<
    dyn Fn(&str, &str, &Value) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>
        + Send
        + Sync,
>;

/// Rule evaluation function (sync — CPU-bound).
pub type RuleFn = Box<dyn Fn(&str, &Value) -> Result<Value, String> + Send + Sync>;

/// Audit sink dispatch function used by H5 live-smoke tests.
///
/// The first argument is the resolved sink declaration, the second is the
/// CloudEvents-compatible envelope. The default runtime keeps audit egress
/// in-memory only, so no external network is touched unless callers inject a
/// sink function.
pub type AuditSinkFn = Arc<dyn Fn(&Value, &Value) -> Result<(), String> + Send + Sync>;

/// Executor configuration.
pub struct ExecConfig {
    pub connector_fn: Option<ConnectorFn>,
    pub rule_fn: Option<RuleFn>,
    pub max_steps: u32,
    pub max_loop_iterations: u32,
    /// Durability store for execution checkpoint/recovery.
    /// If None, execution is stateless (no checkpoint, no recovery).
    pub durability: Option<Arc<crate::DurabilityStore>>,
    pub audit_sink_fn: Option<AuditSinkFn>,
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            connector_fn: None,
            rule_fn: None,
            max_steps: 10_000,
            max_loop_iterations: 1_000,
            durability: None,
            audit_sink_fn: None,
        }
    }
}

/// Execute a compiled VilwGraph with input (async).
pub async fn execute(
    graph: &VilwGraph,
    input: Value,
    config: &ExecConfig,
) -> Result<ExecResult, ExecError> {
    let mut vars: HashMap<String, Value> = HashMap::new();

    if let Value::Object(ref map) = input {
        for (k, v) in map {
            vars.insert(k.clone(), v.clone());
        }
    }
    vars.insert("trigger_payload".into(), input.clone());
    seed_trigger_special_vars(graph, &mut vars);

    // Generate execution ID
    let exec_id = format!(
        "exec_{:016x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let workflow_id = graph.id.clone();

    // Durability: begin execution
    if let Some(ref store) = config.durability {
        store.begin(
            &exec_id,
            &workflow_id,
            vars.get("trigger_payload").unwrap_or(&Value::Null),
        );
    }

    emit_audit_event(
        graph,
        &mut vars,
        config,
        "workflow_started",
        Some(&exec_id),
        serde_json::json!({ "workflow_id": graph.id, "exec_id": exec_id }),
    );

    let mut steps: u32 = 0;
    let result = walk_subgraph(
        graph.entry_node,
        None,
        graph,
        &mut vars,
        config,
        &mut steps,
        &exec_id,
    )
    .await;

    // Durability: complete or fail
    if let Some(ref store) = config.durability {
        match &result {
            Ok(_) => store.complete(&exec_id),
            Err(e) => store.fail(&exec_id, &e.to_string()),
        }
    }

    match result {
        Ok(output) => {
            emit_audit_event(
                graph,
                &mut vars,
                config,
                "workflow_succeeded",
                Some(&exec_id),
                serde_json::json!({ "workflow_id": graph.id, "exec_id": exec_id, "steps": steps }),
            );
            Ok(ExecResult {
                output,
                variables: vars,
                steps,
            })
        }
        Err(e) => {
            emit_audit_event(
                graph,
                &mut vars,
                config,
                "workflow_failed",
                Some(&exec_id),
                serde_json::json!({ "workflow_id": graph.id, "exec_id": exec_id, "error": e.to_string(), "steps": steps }),
            );
            Err(e)
        }
    }
}

// ── Core walker — walks subgraph until terminal or scope boundary ───────────

/// Walk from `start_idx` executing nodes, following edges.
/// Stops at End/EndTrigger, or when reaching `scope_boundary` (loop back-edge).
/// Returns the output value.
///
/// Uses Box::pin for recursive async (ErrorBoundary, loops call walk_subgraph).
fn walk_subgraph<'a>(
    start_idx: usize,
    scope_boundary: Option<usize>,
    graph: &'a VilwGraph,
    vars: &'a mut HashMap<String, Value>,
    config: &'a ExecConfig,
    steps: &'a mut u32,
    exec_id: &'a str,
) -> Pin<Box<dyn Future<Output = Result<Value, ExecError>> + Send + 'a>> {
    Box::pin(async move {
        let mut current_idx = start_idx;

        loop {
            if *steps >= config.max_steps {
                return Err(ExecError {
                    message: format!("exceeded max steps ({})", config.max_steps),
                    node_id: None,
                });
            }
            *steps += 1;

            let node = &graph.nodes[current_idx];
            let audit_activity = !matches!(node.kind, NodeKind::End)
                && (graph.audit_log.is_some() || node.audit_log.is_some());
            if audit_activity {
                emit_activity_audit_event(
                    graph,
                    node,
                    vars,
                    config,
                    "activity_started",
                    serde_json::json!({ "workflow_id": graph.id, "node_id": node.id, "steps": *steps }),
                );
            }

            match node.kind {
                NodeKind::Trigger => {}

                NodeKind::Connector => {
                    let result = execute_connector(node, vars, config).await?;
                    store_output(node, &result, vars);
                    if let Some(ref store) = config.durability {
                        store.checkpoint(exec_id, &node.id, *steps, vars);
                    }
                }

                NodeKind::Transform => {
                    let result = execute_transform(node, vars)?;
                    tracing::debug!(
                        "Transform '{}' output_var={:?} result={}",
                        node.id,
                        node.output_variable,
                        result
                    );
                    store_output(node, &result, vars);
                    if let Some(ref store) = config.durability {
                        store.checkpoint(exec_id, &node.id, *steps, vars);
                    }
                }

                NodeKind::VilRules => {
                    let result = execute_rules(node, vars, config)?;
                    store_output(node, &result, vars);
                    if let Some(ref store) = config.durability {
                        store.checkpoint(exec_id, &node.id, *steps, vars);
                    }
                }

                NodeKind::EndTrigger => {
                    return execute_end_trigger(node, vars);
                }

                NodeKind::End => {
                    return Ok(vars.get("_last_output").cloned().unwrap_or(Value::Null));
                }

                NodeKind::LoopWhile => {
                    execute_loop_while(current_idx, graph, vars, config, steps, exec_id).await?;
                    // After loop, advance via _exit edge
                    current_idx = find_exit_edge(current_idx, graph)?;
                    continue;
                }

                NodeKind::LoopForEach => {
                    execute_loop_foreach(current_idx, graph, vars, config, steps, exec_id).await?;
                    current_idx = find_exit_edge(current_idx, graph)?;
                    continue;
                }

                NodeKind::LoopRepeat => {
                    execute_loop_repeat(current_idx, graph, vars, config, steps, exec_id).await?;
                    current_idx = find_exit_edge(current_idx, graph)?;
                    continue;
                }

                NodeKind::ErrorBoundary => {
                    // Walk body subgraph, catch errors → route to error edge
                    let normal_edges: Vec<_> = graph
                        .outgoing_edges(current_idx)
                        .iter()
                        .filter(|e| e.condition.is_none())
                        .cloned()
                        .collect();
                    let error_edges: Vec<_> = graph
                        .outgoing_edges(current_idx)
                        .iter()
                        .filter(|e| e.condition.as_deref() == Some("_error"))
                        .cloned()
                        .collect();

                    if let Some(normal) = normal_edges.first() {
                        let saved_vars = vars.clone();
                        match walk_subgraph(
                            normal.to_idx,
                            None,
                            graph,
                            vars,
                            config,
                            steps,
                            exec_id,
                        )
                        .await
                        {
                            Ok(result) => return Ok(result),
                            Err(e) => {
                                *vars = saved_vars;
                                vars.insert(
                                    "_error".into(),
                                    serde_json::json!({
                                        "message": e.message,
                                        "node_id": e.node_id,
                                    }),
                                );
                                if let Some(err_edge) = error_edges.first() {
                                    current_idx = err_edge.to_idx;
                                    continue;
                                } else {
                                    return Err(e);
                                }
                            }
                        }
                    }
                }

                NodeKind::ExclusiveGateway | NodeKind::InclusiveGateway => {
                    let next = evaluate_gateway(
                        current_idx,
                        graph,
                        vars,
                        node.kind == NodeKind::InclusiveGateway,
                    )?;
                    if next.is_empty() {
                        return Err(ExecError {
                            message: "no guard condition matched".into(),
                            node_id: Some(node.id.clone()),
                        });
                    }
                    current_idx = next[0];
                    continue;
                }

                NodeKind::Function => {
                    let result = execute_wasm_function(node, vars, config).await?;
                    store_output(node, &result, vars);
                    if let Some(ref store) = config.durability {
                        store.checkpoint(exec_id, &node.id, *steps, vars);
                    }
                }

                NodeKind::Sidecar => {
                    let result = execute_sidecar(node, vars, config).await?;
                    tracing::debug!(
                        "Sidecar '{}' output_var={:?} result={}",
                        node.id,
                        node.output_variable,
                        result
                    );
                    store_output(node, &result, vars);
                    if let Some(ref store) = config.durability {
                        store.checkpoint(exec_id, &node.id, *steps, vars);
                    }
                }

                NodeKind::SubWorkflow => {
                    let result = execute_sub_workflow(node, vars, config).await?;
                    store_output(node, &result, vars);
                    if let Some(ref store) = config.durability {
                        store.checkpoint(exec_id, &node.id, *steps, vars);
                    }
                }

                NodeKind::HumanTask => {
                    let result = execute_human_task(node, vars, config).await?;
                    store_output(node, &result, vars);
                    if let Some(ref store) = config.durability {
                        store.checkpoint(exec_id, &node.id, *steps, vars);
                    }
                }

                NodeKind::NativeCode => {
                    let result = execute_native_code(node, vars, config).await?;
                    tracing::debug!(
                        "NativeCode '{}' output_var={:?} result={}",
                        node.id,
                        node.output_variable,
                        result
                    );
                    store_output(node, &result, vars);
                    if let Some(ref store) = config.durability {
                        store.checkpoint(exec_id, &node.id, *steps, vars);
                    }
                }

                NodeKind::Compute => {
                    let result = execute_compute(node, vars, config).await?;
                    tracing::debug!(
                        "Compute '{}' output_var={:?} result={}",
                        node.id,
                        node.output_variable,
                        result
                    );
                    store_output(node, &result, vars);
                    if let Some(ref store) = config.durability {
                        store.checkpoint(exec_id, &node.id, *steps, vars);
                    }
                }

                NodeKind::Validate => {
                    let result = execute_validate(node, vars)?;
                    tracing::debug!(
                        "Validate '{}' output_var={:?} result={}",
                        node.id,
                        node.output_variable,
                        result
                    );
                    store_output(node, &result, vars);
                    if let Some(ref store) = config.durability {
                        store.checkpoint(exec_id, &node.id, *steps, vars);
                    }
                }

                NodeKind::Timer => {
                    let result = execute_timer(node, vars).await?;
                    tracing::debug!("Timer '{}' result={}", node.id, result);
                    store_output(node, &result, vars);
                    if let Some(ref store) = config.durability {
                        store.checkpoint(exec_id, &node.id, *steps, vars);
                    }
                }

                NodeKind::Signal => {
                    let result = execute_signal(node, vars)?;
                    tracing::debug!("Signal '{}' result={}", node.id, result);
                    store_output(node, &result, vars);
                    if let Some(ref store) = config.durability {
                        store.checkpoint(exec_id, &node.id, *steps, vars);
                    }
                }

                NodeKind::EventGateway => {
                    let result = execute_event_gateway(node, vars).await?;
                    tracing::debug!("EventGateway '{}' result={}", node.id, result);
                    store_output(node, &result, vars);
                    if let Some(ref store) = config.durability {
                        store.checkpoint(exec_id, &node.id, *steps, vars);
                    }
                }

                NodeKind::Parallel => {
                    // Fork: execute all outgoing branches concurrently
                    let edges = graph.outgoing_edges(current_idx);
                    if edges.len() > 1 {
                        let join_idx = find_join_for_parallel(current_idx, graph);
                        let branch_starts: Vec<usize> = edges.iter().map(|e| e.to_idx).collect();

                        // Execute branches sequentially but concurrently via join_all
                        // (walk_subgraph borrows &mut vars, so true parallel needs cloning)
                        let mut branch_results: Vec<(Value, HashMap<String, Value>)> = Vec::new();

                        for &branch_start in &branch_starts {
                            let mut branch_vars = vars.clone();
                            let mut branch_steps = 0u32;
                            let result = walk_subgraph(
                                branch_start,
                                join_idx,
                                graph,
                                &mut branch_vars,
                                config,
                                &mut branch_steps,
                                exec_id,
                            )
                            .await?;
                            *steps += branch_steps;
                            branch_results.push((result, branch_vars));
                        }

                        // Merge all branch results into main vars
                        for (result, branch_vars) in branch_results {
                            for (k, v) in branch_vars {
                                if k != "trigger_payload" && k != "_tenant_id" {
                                    vars.insert(k, v);
                                }
                            }
                            vars.insert("_last_output".into(), result);
                        }

                        // Skip to Join node
                        if let Some(ji) = join_idx {
                            current_idx = ji;
                            continue;
                        }
                    }
                }

                NodeKind::Join | NodeKind::Noop => {
                    // Join: barrier — branches already merged in Parallel handler
                    // Noop: passthrough
                }
            }

            if audit_activity {
                emit_activity_audit_event(
                    graph,
                    node,
                    vars,
                    config,
                    "activity_succeeded",
                    serde_json::json!({
                        "workflow_id": graph.id,
                        "node_id": node.id,
                        "steps": *steps,
                        "output": vars.get("_last_output").cloned().unwrap_or(Value::Null),
                    }),
                );
            }

            // Advance to next node
            current_idx = match find_next_node(current_idx, graph, vars)? {
                Some(next) => {
                    // Check scope boundary (loop back-edge)
                    if let Some(boundary) = scope_boundary {
                        if next == boundary {
                            return Ok(vars.get("_last_output").cloned().unwrap_or(Value::Null));
                        }
                    }
                    next
                }
                None => {
                    return Ok(vars.get("_last_output").cloned().unwrap_or(Value::Null));
                }
            };
        }
    }) // close Box::pin(async move { ... })
}

fn emit_audit_event(
    graph: &VilwGraph,
    vars: &mut HashMap<String, Value>,
    config: &ExecConfig,
    event_type: &str,
    subject: Option<&str>,
    data: Value,
) {
    if graph.audit_log.is_none() {
        return;
    }
    if !crate::audit::audit_event_enabled(graph.audit_log.as_ref(), event_type) {
        return;
    }
    let event = crate::audit::cloud_event_envelope(&graph.id, event_type, subject, data);
    match vars.get_mut("_audit_events") {
        Some(Value::Array(events)) => events.push(event.clone()),
        _ => {
            vars.insert("_audit_events".into(), Value::Array(vec![event.clone()]));
        }
    }

    let sinks = crate::audit::audit_sinks_for_event(graph.audit_log.as_ref(), event_type);
    if sinks.is_empty() {
        return;
    }
    for sink in sinks {
        if let Some(ref sink_fn) = config.audit_sink_fn {
            if let Err(err) = sink_fn(&sink, &event) {
                let record = serde_json::json!({
                    "sink": sink,
                    "event_type": event_type,
                    "error": err,
                });
                match vars.get_mut("_audit_sink_errors") {
                    Some(Value::Array(errors)) => errors.push(record),
                    _ => {
                        vars.insert("_audit_sink_errors".into(), Value::Array(vec![record]));
                    }
                }
            }
        } else {
            let record = serde_json::json!({"sink": sink, "event": event});
            match vars.get_mut("_audit_sink_events") {
                Some(Value::Array(events)) => events.push(record),
                _ => {
                    vars.insert("_audit_sink_events".into(), Value::Array(vec![record]));
                }
            }
        }
    }
}

fn emit_activity_audit_event(
    graph: &VilwGraph,
    node: &VilwNode,
    vars: &mut HashMap<String, Value>,
    config: &ExecConfig,
    event_type: &str,
    data: Value,
) {
    let Some(audit_log) = node.audit_log.as_ref().or(graph.audit_log.as_ref()) else {
        return;
    };
    if !crate::audit::audit_event_enabled(Some(audit_log), event_type) {
        return;
    }
    let event = crate::audit::cloud_event_envelope(&graph.id, event_type, Some(&node.id), data);
    match vars.get_mut("_audit_events") {
        Some(Value::Array(events)) => events.push(event.clone()),
        _ => {
            vars.insert("_audit_events".into(), Value::Array(vec![event.clone()]));
        }
    }

    for sink in crate::audit::audit_sinks_for_event(Some(audit_log), event_type) {
        if let Some(ref sink_fn) = config.audit_sink_fn {
            if let Err(err) = sink_fn(&sink, &event) {
                let record = serde_json::json!({
                    "sink": sink,
                    "event_type": event_type,
                    "error": err,
                });
                match vars.get_mut("_audit_sink_errors") {
                    Some(Value::Array(errors)) => errors.push(record),
                    _ => {
                        vars.insert("_audit_sink_errors".into(), Value::Array(vec![record]));
                    }
                }
            }
        } else {
            let record = serde_json::json!({"sink": sink, "event": event});
            match vars.get_mut("_audit_sink_events") {
                Some(Value::Array(events)) => events.push(record),
                _ => {
                    vars.insert("_audit_sink_events".into(), Value::Array(vec![record]));
                }
            }
        }
    }
}

fn store_output(node: &VilwNode, result: &Value, vars: &mut HashMap<String, Value>) {
    if let Some(ref out_var) = node.output_variable {
        vars.insert(out_var.clone(), result.clone());
    }
    vars.insert("_last_output".into(), result.clone());
}

fn seed_trigger_special_vars(graph: &VilwGraph, vars: &mut HashMap<String, Value>) {
    vars.entry("_loop_done".into())
        .or_insert(Value::Bool(false));
    vars.entry("_trigger".into())
        .or_insert_with(|| Value::String(graph.trigger_type.clone()));

    let trigger_node = &graph.nodes[graph.entry_node];
    if let Some(body_schema) = trigger_node.config.get("body_schema") {
        vars.entry("_body_schema".into())
            .or_insert_with(|| body_schema.clone());
    }
    if let Some(proto_field) = trigger_node.config.get("proto_field") {
        vars.entry("_proto_field".into())
            .or_insert_with(|| proto_field.clone());
    }
    if (trigger_node.config.get("body_schema").is_some()
        || trigger_node.config.get("proto_field").is_some())
        && !vars.contains_key("trigger_body")
    {
        if let Some(body) = vars.get("body").cloned() {
            vars.insert("trigger_body".into(), body);
        } else if let Some(body) = vars
            .get("trigger_payload")
            .and_then(|v| v.get("body"))
            .cloned()
        {
            vars.insert("trigger_body".into(), body);
        }
    }

    if graph.trigger_type == "cron" {
        let schedule = trigger_node
            .config
            .get("cron")
            .and_then(|c| c.get("expression"))
            .and_then(|v| v.as_str())
            .or_else(|| {
                trigger_node
                    .config
                    .get("expression")
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("")
            .to_string();
        vars.insert("_schedule".into(), Value::String(schedule));
        vars.insert("_fired_at".into(), Value::Number(unix_millis().into()));
    }
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── Loops — walk full body subgraph per iteration ───────────────────────────

async fn execute_loop_while(
    loop_idx: usize,
    graph: &VilwGraph,
    vars: &mut HashMap<String, Value>,
    config: &ExecConfig,
    steps: &mut u32,
    exec_id: &str,
) -> Result<(), ExecError> {
    let node = &graph.nodes[loop_idx];
    let condition = node
        .config
        .get("condition")
        .and_then(|v| v.as_str())
        .unwrap_or("false");
    let max_iter = node
        .config
        .get("max_iterations")
        .and_then(|v| v.as_u64())
        .unwrap_or(config.max_loop_iterations as u64) as u32;

    let body_idx = find_body_edge(loop_idx, graph);

    let mut iteration = 0u32;
    let mut loop_results = Vec::new();
    while iteration < max_iter {
        match vil_expr::evaluate_bool(condition, vars) {
            Ok(true) => {}
            Ok(false) => break,
            Err(e) => {
                return Err(ExecError {
                    message: format!("loop condition: {}", e),
                    node_id: Some(node.id.clone()),
                })
            }
        }
        vars.insert("_loop_index".into(), Value::Number(iteration.into()));
        iteration += 1;

        if let Some(bidx) = body_idx {
            // Walk FULL body subgraph — stop when edge points back to loop node
            let result =
                walk_subgraph(bidx, Some(loop_idx), graph, vars, config, steps, exec_id).await?;
            loop_results.push(result);
        } else {
            loop_results.push(vars.get("_last_output").cloned().unwrap_or(Value::Null));
        }
    }

    vars.insert("_loop_done".into(), Value::Bool(true));
    vars.insert("_loop_results".into(), Value::Array(loop_results));

    Ok(())
}

async fn execute_loop_foreach(
    loop_idx: usize,
    graph: &VilwGraph,
    vars: &mut HashMap<String, Value>,
    config: &ExecConfig,
    steps: &mut u32,
    exec_id: &str,
) -> Result<(), ExecError> {
    let node = &graph.nodes[loop_idx];
    let collection_expr = node
        .config
        .get("collection")
        .and_then(|v| v.as_str())
        .unwrap_or("[]");
    let item_var = node
        .config
        .get("item_variable")
        .and_then(|v| v.as_str())
        .unwrap_or("_item");

    let collection = vil_expr::evaluate(collection_expr, vars).map_err(|e| ExecError {
        message: format!("foreach collection: {}", e),
        node_id: Some(node.id.clone()),
    })?;

    let items = match &collection {
        Value::Array(arr) => arr.clone(),
        _ => Vec::new(),
    };

    let body_idx = find_body_edge(loop_idx, graph);

    let mut loop_results = Vec::new();
    for (i, item) in items.iter().enumerate() {
        vars.insert(item_var.into(), item.clone());
        vars.insert("_loop_index".into(), Value::Number(i.into()));

        if let Some(bidx) = body_idx {
            let result =
                walk_subgraph(bidx, Some(loop_idx), graph, vars, config, steps, exec_id).await?;
            loop_results.push(result);
        } else {
            loop_results.push(vars.get("_last_output").cloned().unwrap_or(Value::Null));
        }
    }

    vars.insert("_loop_done".into(), Value::Bool(true));
    vars.insert("_loop_results".into(), Value::Array(loop_results));

    Ok(())
}

async fn execute_loop_repeat(
    loop_idx: usize,
    graph: &VilwGraph,
    vars: &mut HashMap<String, Value>,
    config: &ExecConfig,
    steps: &mut u32,
    exec_id: &str,
) -> Result<(), ExecError> {
    let node = &graph.nodes[loop_idx];
    let count = node
        .config
        .get("repeat_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(1) as u32;

    let body_idx = find_body_edge(loop_idx, graph);

    let mut loop_results = Vec::new();
    for i in 0..count {
        vars.insert("_loop_index".into(), Value::Number(i.into()));

        if let Some(bidx) = body_idx {
            let result =
                walk_subgraph(bidx, Some(loop_idx), graph, vars, config, steps, exec_id).await?;
            loop_results.push(result);
        } else {
            loop_results.push(vars.get("_last_output").cloned().unwrap_or(Value::Null));
        }
    }

    vars.insert("_loop_done".into(), Value::Bool(true));
    vars.insert("_loop_results".into(), Value::Array(loop_results));

    Ok(())
}

/// Find body edge (non-exit, non-error) from loop node.
fn find_body_edge(loop_idx: usize, graph: &VilwGraph) -> Option<usize> {
    graph
        .outgoing_edges(loop_idx)
        .iter()
        .find(|e| e.condition.is_none() || e.condition.as_deref() != Some("_exit"))
        .map(|e| e.to_idx)
}

/// Find exit edge from loop node.
fn find_exit_edge(loop_idx: usize, graph: &VilwGraph) -> Result<usize, ExecError> {
    let edges = graph.outgoing_edges(loop_idx);
    // Prefer _exit edge
    if let Some(exit) = edges
        .iter()
        .find(|e| e.condition.as_deref() == Some("_exit"))
    {
        return Ok(exit.to_idx);
    }
    // Fallback: last edge or next node
    edges.last().map(|e| e.to_idx).ok_or(ExecError {
        message: "loop has no exit edge".into(),
        node_id: Some(graph.nodes[loop_idx].id.clone()),
    })
}

/// Find the Join node that corresponds to a Parallel fork.
/// Walks outgoing edges from Parallel → follows first branch → looks for Join node.
fn find_join_for_parallel(_parallel_idx: usize, graph: &VilwGraph) -> Option<usize> {
    // Simple heuristic: scan all nodes for a Join that has edges coming from
    // branches that start at this Parallel.
    for (i, node) in graph.nodes.iter().enumerate() {
        if node.kind == NodeKind::Join {
            // Check if any branch path from this parallel leads to this join
            let incoming = graph.edges.iter().filter(|e| e.to_idx == i).count();
            if incoming >= 2 {
                return Some(i);
            }
        }
    }
    None
}

// ── Node executors ──────────────────────────────────────────────────────────

async fn execute_connector(
    node: &VilwNode,
    vars: &HashMap<String, Value>,
    config: &ExecConfig,
) -> Result<Value, ExecError> {
    let input = eval_bridge::eval_all_mappings(&node.mappings, vars).map_err(|e| ExecError {
        message: e,
        node_id: Some(node.id.clone()),
    })?;

    let connector_ref = node
        .config
        .get("connector_ref")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let operation = node
        .config
        .get("operation")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut input_value = serde_json::to_value(&input).unwrap_or(Value::Null);

    // Inject connector metadata only for custom ConnectorFn dispatch. The default
    // stub fastpath is benchmarked and does not need to pay for cloning config.
    let expose_connector_metadata = config.connector_fn.is_some();
    if let Value::Object(ref mut map) = input_value {
        if expose_connector_metadata {
            map.insert(
                "_connector_ref".into(),
                Value::String(connector_ref.to_string()),
            );
            map.insert("_operation".into(), Value::String(operation.to_string()));
            if let Some(connector_type) = node.config.get("connector_type") {
                map.insert("_connector_type".into(), connector_type.clone());
            }
            map.insert("_connector_config".into(), node.config.clone());
        }
        if let Some(streaming) = node.config.get("streaming") {
            map.insert("_streaming".into(), streaming.clone());
        }
        if let Some(dialect) = node.config.get("dialect") {
            map.insert("_dialect".into(), dialect.clone());
        }
        if let Some(tap) = node.config.get("json_tap") {
            map.insert("_json_tap".into(), tap.clone());
        }
        if let Some(fmt) = node.config.get("stream_format") {
            map.insert("_stream_format".into(), fmt.clone());
        }
    }

    if let Some(ref connector_fn) = config.connector_fn {
        connector_fn(connector_ref, operation, &input_value)
            .await
            .map_err(|e| ExecError {
                message: e,
                node_id: Some(node.id.clone()),
            })
    } else {
        Ok(serde_json::json!({
            "_stub": true,
            "connector_ref": connector_ref,
            "operation": operation,
            "input": input_value,
        }))
    }
}

fn execute_transform(node: &VilwNode, vars: &HashMap<String, Value>) -> Result<Value, ExecError> {
    let result = eval_bridge::eval_all_mappings(&node.mappings, vars).map_err(|e| ExecError {
        message: e,
        node_id: Some(node.id.clone()),
    })?;

    // Unwrap single-mapping transforms: if there's exactly one mapping and its
    // target matches the output_variable, return the value directly instead of
    // wrapping it as {"target_name": value}. This prevents double nesting when
    // downstream nodes reference output_variable.field.
    if node.mappings.len() == 1 {
        if let Some(ref out_var) = node.output_variable {
            if node.mappings[0].target == *out_var {
                if let Some(val) = result.get(out_var) {
                    return Ok(val.clone());
                }
            }
        }
    }

    Ok(serde_json::to_value(&result).unwrap_or(Value::Null))
}

fn execute_rules(
    node: &VilwNode,
    vars: &HashMap<String, Value>,
    config: &ExecConfig,
) -> Result<Value, ExecError> {
    let rule_set_id = node
        .config
        .get("rule_set_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let input = eval_bridge::eval_all_mappings(&node.mappings, vars).map_err(|e| ExecError {
        message: e,
        node_id: Some(node.id.clone()),
    })?;
    let input_value = serde_json::to_value(&input).unwrap_or(Value::Null);

    if let Some(ref rule_fn) = config.rule_fn {
        rule_fn(rule_set_id, &input_value).map_err(|e| ExecError {
            message: e,
            node_id: Some(node.id.clone()),
        })
    } else {
        Ok(serde_json::json!({
            "_stub": true, "_rule": rule_set_id,
        }))
    }
}

fn execute_end_trigger(node: &VilwNode, vars: &HashMap<String, Value>) -> Result<Value, ExecError> {
    if let Some(fr) = node.config.get("final_response") {
        let lang = fr
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or("vil-expr");
        let source = fr
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("_last_output");

        match lang {
            "vil-expr" | "cel" => vil_expr::evaluate(source, vars).map_err(|e| ExecError {
                message: e,
                node_id: Some(node.id.clone()),
            }),
            "literal" => Ok(Value::String(source.to_string())),
            "spv1" => Ok(Value::String(crate::spv1::eval_template(source, vars))),
            _ => Ok(vars.get("_last_output").cloned().unwrap_or(Value::Null)),
        }
    } else {
        Ok(vars.get("_last_output").cloned().unwrap_or(Value::Null))
    }
}

// ── WASM Function ───────────────────────────────────────────────────────────

async fn execute_wasm_function(
    node: &VilwNode,
    vars: &HashMap<String, Value>,
    config: &ExecConfig,
) -> Result<Value, ExecError> {
    let module_ref = node
        .config
        .get("module_ref")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let function_name = node
        .config
        .get("function_name")
        .and_then(|v| v.as_str())
        .unwrap_or("execute");

    let input = eval_bridge::eval_all_mappings(&node.mappings, vars).map_err(|e| ExecError {
        message: e,
        node_id: Some(node.id.clone()),
    })?;
    let mut input_value = serde_json::to_value(&input).unwrap_or(Value::Null);

    // Inject WASM metadata for registry dispatch
    if let Value::Object(ref mut map) = input_value {
        map.insert("_wasm_module".into(), Value::String(module_ref.into()));
        map.insert("_wasm_function".into(), Value::String(function_name.into()));
    }

    // Dispatch via connector_fn with vastar.wasm.{module} ref
    let connector_ref = format!("vastar.wasm.{}", module_ref);
    if let Some(ref connector_fn) = config.connector_fn {
        connector_fn(&connector_ref, function_name, &input_value)
            .await
            .map_err(|e| ExecError {
                message: e,
                node_id: Some(node.id.clone()),
            })
    } else {
        Ok(serde_json::json!({"_stub": true, "_wasm": module_ref, "_function": function_name}))
    }
}

// ── Sidecar ─────────────────────────────────────────────────────────────────

async fn execute_sidecar(
    node: &VilwNode,
    vars: &HashMap<String, Value>,
    config: &ExecConfig,
) -> Result<Value, ExecError> {
    let target = node
        .config
        .get("target")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let method = node
        .config
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("execute");

    let input = eval_bridge::eval_all_mappings(&node.mappings, vars).map_err(|e| ExecError {
        message: e,
        node_id: Some(node.id.clone()),
    })?;
    let mut input_value = serde_json::to_value(&input).unwrap_or(Value::Null);

    if let Value::Object(ref mut map) = input_value {
        map.insert("_sidecar_target".into(), Value::String(target.into()));
        map.insert("_sidecar_method".into(), Value::String(method.into()));
    }

    let connector_ref = format!("vastar.sidecar.{}", target);
    if let Some(ref connector_fn) = config.connector_fn {
        connector_fn(&connector_ref, method, &input_value)
            .await
            .map_err(|e| ExecError {
                message: e,
                node_id: Some(node.id.clone()),
            })
    } else {
        Ok(serde_json::json!({"_stub": true, "_sidecar": target, "_method": method}))
    }
}

// ── SubWorkflow ─────────────────────────────────────────────────────────────

async fn execute_sub_workflow(
    node: &VilwNode,
    vars: &HashMap<String, Value>,
    config: &ExecConfig,
) -> Result<Value, ExecError> {
    let workflow_ref = node
        .config
        .get("workflow_ref")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Build sub-workflow input from mappings or pass all vars
    let input = if !node.mappings.is_empty() {
        let mapped =
            eval_bridge::eval_all_mappings(&node.mappings, vars).map_err(|e| ExecError {
                message: e,
                node_id: Some(node.id.clone()),
            })?;
        serde_json::to_value(&mapped).unwrap_or(Value::Null)
    } else {
        serde_json::to_value(vars).unwrap_or(Value::Null)
    };

    // Dispatch as connector call — handler layer resolves workflow_ref → graph
    let connector_ref = format!("vastar.workflow.{}", workflow_ref);
    if let Some(ref connector_fn) = config.connector_fn {
        connector_fn(&connector_ref, "execute", &input)
            .await
            .map_err(|e| ExecError {
                message: e,
                node_id: Some(node.id.clone()),
            })
    } else {
        Ok(serde_json::json!({"_stub": true, "_sub_workflow": workflow_ref}))
    }
}

// ── HumanTask ───────────────────────────────────────────────────────────────

async fn execute_human_task(
    node: &VilwNode,
    _vars: &HashMap<String, Value>,
    _config: &ExecConfig,
) -> Result<Value, ExecError> {
    let task_type = node
        .config
        .get("task_type")
        .and_then(|v| v.as_str())
        .unwrap_or("approval");
    let assignee = node.config.get("assignee").and_then(|v| v.as_str());

    // HumanTask requires external task management system.
    // In VIL free tier: return stub. In vflow: parks token until task completed.
    Ok(serde_json::json!({
        "_human_task": true,
        "task_type": task_type,
        "assignee": assignee,
        "_note": "HumanTask requires external task manager. Auto-approved in VIL free tier.",
        "approved": true,
    }))
}

// ── NativeCode ──────────────────────────────────────────────────────────────

async fn execute_native_code(
    node: &VilwNode,
    vars: &HashMap<String, Value>,
    config: &ExecConfig,
) -> Result<Value, ExecError> {
    let handler_ref = node
        .config
        .get("handler_ref")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let input = eval_bridge::eval_all_mappings(&node.mappings, vars).map_err(|e| ExecError {
        message: e,
        node_id: Some(node.id.clone()),
    })?;
    let input_value = serde_json::to_value(&input).unwrap_or(Value::Null);

    // Dispatch via ConnectorFn with vastar.code.{handler_ref}
    let connector_ref = format!("vastar.code.{}", handler_ref);
    if let Some(ref connector_fn) = config.connector_fn {
        connector_fn(&connector_ref, "execute", &input_value)
            .await
            .map_err(|e| ExecError {
                message: e,
                node_id: Some(node.id.clone()),
            })
    } else {
        Ok(serde_json::json!({"_stub": true, "_native_code": handler_ref}))
    }
}

// ── Compute (Starlark) ──────────────────────────────────────────────────────

async fn execute_compute(
    node: &VilwNode,
    vars: &HashMap<String, Value>,
    config: &ExecConfig,
) -> Result<Value, ExecError> {
    let language = node
        .config
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("starlark");
    let entry = node
        .config
        .get("entry")
        .and_then(|v| v.as_str())
        .unwrap_or("run");
    let source = node
        .config
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let budget_profile = node
        .config
        .get("budget_profile")
        .and_then(|v| v.as_str())
        .unwrap_or("default");
    let timeout_ms = node.config.get("timeout_ms").and_then(|v| v.as_u64());

    let mapped = eval_bridge::eval_all_mappings(&node.mappings, vars).map_err(|e| ExecError {
        message: e,
        node_id: Some(node.id.clone()),
    })?;
    let mut ctx = serde_json::to_value(&mapped).unwrap_or(Value::Null);
    if let Value::Object(ref mut map) = ctx {
        if let Some(trigger_payload) = vars.get("trigger_payload") {
            map.entry("trigger_payload")
                .or_insert_with(|| trigger_payload.clone());
            if let Some(body) = trigger_payload.get("body") {
                map.entry("body").or_insert_with(|| body.clone());
            }
        }
    }

    // Inject rule_result if vdicl_rule is configured
    if let Some(vdicl) = node.config.get("vdicl_rule") {
        if !vdicl.is_null() {
            let rule_set_id = vdicl
                .get("rule_set_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let rule_result = if let Some(ref rule_fn) = config.rule_fn {
                rule_fn(rule_set_id, &ctx)
                    .unwrap_or_else(|_| serde_json::json!({"_stub": true, "_rule": rule_set_id}))
            } else {
                serde_json::json!({"_stub": true, "_rule": rule_set_id})
            };
            if let Value::Object(ref mut map) = ctx {
                map.insert("rule_result".into(), rule_result);
            }
        }
    }

    match language {
        "starlark" | "v-starlark" => {
            #[cfg(feature = "compute-starlark")]
            {
                let mut budget = vil_starlark::BudgetConfig::from_profile(budget_profile);
                if let Some(t) = timeout_ms {
                    budget.timeout_ms = Some(t);
                }
                vil_starlark::eval(source, entry, &ctx, &budget).map_err(|e| ExecError {
                    message: format!("compute starlark: {}", e),
                    node_id: Some(node.id.clone()),
                })
            }
            #[cfg(not(feature = "compute-starlark"))]
            {
                let _ = (&ctx, entry, source, budget_profile, timeout_ms);
                Err(ExecError {
                    message: "Compute requires building with --features compute-starlark".into(),
                    node_id: Some(node.id.clone()),
                })
            }
        }
        other => Err(ExecError {
            message: format!(
                "compute language '{}' not supported; use 'starlark' or 'v-starlark'",
                other
            ),
            node_id: Some(node.id.clone()),
        }),
    }
}

fn execute_validate(
    node: &VilwNode,
    vars: &mut HashMap<String, Value>,
) -> Result<Value, ExecError> {
    let mapped = if node.mappings.is_empty() {
        HashMap::new()
    } else {
        eval_bridge::eval_all_mappings(&node.mappings, vars).map_err(|e| ExecError {
            message: e,
            node_id: Some(node.id.clone()),
        })?
    };

    let candidate = if let Some(target) = node.config.get("target").and_then(|v| v.as_str()) {
        mapped
            .get(target)
            .cloned()
            .unwrap_or_else(|| resolve_value_ref(target, vars))
    } else if mapped.len() == 1 {
        mapped.values().next().cloned().unwrap_or(Value::Null)
    } else if !mapped.is_empty() {
        Value::Object(mapped.into_iter().collect())
    } else {
        vars.get("_last_output")
            .cloned()
            .or_else(|| vars.get("trigger_payload").cloned())
            .unwrap_or(Value::Null)
    };

    let schema = node.config.get("schema").ok_or_else(|| ExecError {
        message: "validate activity requires schema".into(),
        node_id: Some(node.id.clone()),
    })?;

    let validation = vil_validate_schema::validate_schema(&[candidate.clone(), schema.clone()])
        .map_err(|e| ExecError {
            message: e,
            node_id: Some(node.id.clone()),
        })?;

    if validation
        .get("valid")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        vars.insert("_validation".into(), validation);
        Ok(candidate)
    } else {
        vars.insert("_validation_error".into(), validation.clone());
        Err(ExecError {
            message: format!("validation failed: {}", validation),
            node_id: Some(node.id.clone()),
        })
    }
}

async fn execute_timer(
    node: &VilwNode,
    vars: &mut HashMap<String, Value>,
) -> Result<Value, ExecError> {
    let delay_ms = node
        .config
        .get("delay_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let until = node
        .config
        .get("until")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if delay_ms > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
    }

    let fired_at = now_epoch_millis();
    let result = serde_json::json!({
        "delay_ms": delay_ms,
        "until": until,
        "fired_at": fired_at,
        "parked": false,
    });
    vars.insert("_timer".into(), result.clone());
    vars.entry("_fired_at".into())
        .or_insert(Value::Number(fired_at.into()));
    Ok(result)
}

fn execute_signal(node: &VilwNode, vars: &mut HashMap<String, Value>) -> Result<Value, ExecError> {
    let signal = node
        .config
        .get("signal")
        .and_then(|v| v.as_str())
        .unwrap_or("custom")
        .to_string();
    let target = node
        .config
        .get("target")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    vars.insert("_signal".into(), Value::String(signal.clone()));
    if let Some(ref target) = target {
        vars.insert("_signal_target".into(), Value::String(target.clone()));
    }
    Ok(serde_json::json!({
        "signal": signal,
        "target": target,
    }))
}

async fn execute_event_gateway(
    node: &VilwNode,
    vars: &mut HashMap<String, Value>,
) -> Result<Value, ExecError> {
    let expected = node
        .config
        .get("event")
        .and_then(|v| v.as_str())
        .or_else(|| {
            node.config
                .get("await_events")
                .and_then(|v| v.as_array())
                .and_then(|items| items.first())
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            node.config
                .get("await")
                .and_then(|v| v.as_array())
                .and_then(|items| items.first())
                .and_then(|v| v.as_str())
        })
        .unwrap_or("event")
        .to_string();
    let timeout_ms = node
        .config
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let observed = vars
        .get("_event")
        .or_else(|| vars.get("event"))
        .cloned()
        .unwrap_or(Value::Null);
    let matched = observed.as_str().map(|s| s == expected).unwrap_or(false)
        || observed
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s == expected)
            .unwrap_or(false);
    let result = serde_json::json!({
        "event": expected,
        "timeout_ms": timeout_ms,
        "observed": observed,
        "matched": matched,
        "parked": !matched,
        "resume_semantics": "local-stub: matched input _event/event resumes immediately; otherwise returns parked marker without sleeping",
    });
    vars.insert("_event_gateway".into(), result.clone());
    Ok(result)
}

fn resolve_value_ref(path: &str, vars: &HashMap<String, Value>) -> Value {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "$" {
        return vars.get("trigger_payload").cloned().unwrap_or(Value::Null);
    }
    if !trimmed.starts_with("$.") {
        if let Ok(value) = vil_expr::evaluate(trimmed, vars) {
            return value;
        }
    }
    let normalized = trimmed.strip_prefix("$.").unwrap_or(trimmed);
    let mut parts = normalized.split('.');
    let Some(root_name) = parts.next() else {
        return Value::Null;
    };
    let mut current = vars.get(root_name).cloned().unwrap_or(Value::Null);
    for key in parts {
        current = match current {
            Value::Object(ref obj) => obj.get(key).cloned().unwrap_or(Value::Null),
            Value::Array(ref arr) => key
                .parse::<usize>()
                .ok()
                .and_then(|idx| arr.get(idx).cloned())
                .unwrap_or(Value::Null),
            _ => Value::Null,
        };
    }
    current
}

fn now_epoch_millis() -> u64 {
    unix_millis()
}

// ── Flow navigation ─────────────────────────────────────────────────────────

fn find_next_node(
    current_idx: usize,
    graph: &VilwGraph,
    vars: &HashMap<String, Value>,
) -> Result<Option<usize>, ExecError> {
    let mut edges: Vec<_> = graph.outgoing_edges(current_idx);
    edges.sort_by_key(|edge| std::cmp::Reverse(edge.priority));

    for edge in &edges {
        if edge.detached {
            continue;
        }
        if let Some(ref cond) = edge.condition {
            if cond == "_error" || cond == "_exit" {
                continue;
            }
            match vil_expr::evaluate_bool(cond, vars) {
                Ok(true) => return Ok(Some(edge.to_idx)),
                Ok(false) => continue,
                Err(e) => {
                    return Err(ExecError {
                        message: format!("guard eval: {}", e),
                        node_id: None,
                    })
                }
            }
        } else {
            return Ok(Some(edge.to_idx));
        }
    }

    Ok(None)
}

fn evaluate_gateway(
    node_idx: usize,
    graph: &VilwGraph,
    vars: &HashMap<String, Value>,
    inclusive: bool,
) -> Result<Vec<usize>, ExecError> {
    let mut edges: Vec<_> = graph.outgoing_edges(node_idx);
    edges.sort_by_key(|edge| std::cmp::Reverse(edge.priority));

    let mut matched = Vec::new();
    for edge in &edges {
        if edge.detached {
            continue;
        }
        if let Some(ref cond) = edge.condition {
            if cond == "_error" || cond == "_exit" {
                continue;
            }
            match vil_expr::evaluate_bool(cond, vars) {
                Ok(true) => {
                    matched.push(edge.to_idx);
                    if !inclusive {
                        break;
                    }
                }
                Ok(false) => {}
                Err(e) => {
                    return Err(ExecError {
                        message: format!("guard eval: {}", e),
                        node_id: None,
                    })
                }
            }
        } else {
            matched.push(edge.to_idx);
            if !inclusive {
                break;
            }
        }
    }

    Ok(matched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler;
    use serde_json::json;

    const SIMPLE_WF: &str = r#"
version: "3.0"
metadata:
  id: test-exec
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config:
        trigger_type: webhook
        webhook_config: { path: /test }
        response_mode: buffered
        end_activity: respond
      output_variable: trigger_payload
    - id: step1
      activity_type: Connector
      connector_config:
        connector_ref: vastar.http
        operation: post
      input_mappings:
        - target: url
          source: { language: literal, source: "http://example.com" }
        - target: body
          source: { language: vil-expr, source: '{"name": trigger_payload.name}' }
      output_variable: step1_result
    - id: respond
      activity_type: EndTrigger
      end_trigger_config:
        trigger_ref: trigger
        final_response:
          language: vil-expr
          source: '{"result": step1_result, "input_name": trigger_payload.name}'
    - id: end
      activity_type: End
  flows:
    - { id: f1, from: { node: trigger }, to: { node: step1 } }
    - { id: f2, from: { node: step1 }, to: { node: respond } }
    - { id: f3, from: { node: respond }, to: { node: end } }
  variables:
    - { name: trigger_payload, type: object }
    - { name: step1_result, type: object }
"#;

    #[tokio::test]
    async fn test_execute_simple_stub() {
        let graph = compiler::compile(SIMPLE_WF).unwrap();
        let input = json!({"name": "Alice"});
        let config = ExecConfig::default();
        let result = execute(&graph, input, &config).await.unwrap();
        assert_eq!(result.output["input_name"], "Alice");
        assert!(result.output["result"]["_stub"].as_bool().unwrap_or(false));
        assert_eq!(result.steps, 3);
    }

    #[tokio::test]
    async fn test_execute_with_connector_fn() {
        let graph = compiler::compile(SIMPLE_WF).unwrap();
        let input = json!({"name": "Bob"});
        let config = ExecConfig {
            connector_fn: Some(Arc::new(|_ref, _op, input| {
                let input = input.clone();
                Box::pin(async move { Ok(json!({"status": "ok", "echo": input})) })
            })),
            ..Default::default()
        };
        let result = execute(&graph, input, &config).await.unwrap();
        assert_eq!(result.output["input_name"], "Bob");
        assert_eq!(result.output["result"]["status"], "ok");
    }

    const GUARD_WF: &str = r#"
version: "3.0"
metadata:
  id: test-guard
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config:
        trigger_type: webhook
        webhook_config: { path: /guard }
        response_mode: buffered
        end_activity: high-resp
      output_variable: trigger_payload
    - id: gateway
      activity_type: ExclusiveGateway
    - id: high
      activity_type: Connector
      connector_config: { connector_ref: vastar.http, operation: post }
      output_variable: high_result
    - id: low
      activity_type: Connector
      connector_config: { connector_ref: vastar.http, operation: post }
      output_variable: low_result
    - id: high-resp
      activity_type: EndTrigger
      end_trigger_config:
        trigger_ref: trigger
        final_response: { language: vil-expr, source: '{"route": "high"}' }
    - id: low-resp
      activity_type: EndTrigger
      end_trigger_config:
        trigger_ref: trigger
        final_response: { language: vil-expr, source: '{"route": "low"}' }
    - id: end
      activity_type: End
  flows:
    - { id: f1, from: { node: trigger }, to: { node: gateway } }
    - { id: f2, from: { node: gateway }, to: { node: high }, condition: "trigger_payload.score > 80", priority: 1 }
    - { id: f3, from: { node: gateway }, to: { node: low }, condition: "trigger_payload.score <= 80", priority: 0 }
    - { id: f4, from: { node: high }, to: { node: high-resp } }
    - { id: f5, from: { node: low }, to: { node: low-resp } }
    - { id: f6, from: { node: high-resp }, to: { node: end } }
    - { id: f7, from: { node: low-resp }, to: { node: end } }
"#;

    #[tokio::test]
    async fn test_guard_high() {
        let graph = compiler::compile(GUARD_WF).unwrap();
        let result = execute(&graph, json!({"score": 90}), &ExecConfig::default())
            .await
            .unwrap();
        assert_eq!(result.output["route"], "high");
    }

    #[tokio::test]
    async fn test_guard_low() {
        let graph = compiler::compile(GUARD_WF).unwrap();
        let result = execute(&graph, json!({"score": 50}), &ExecConfig::default())
            .await
            .unwrap();
        assert_eq!(result.output["route"], "low");
    }

    #[cfg(feature = "compute-starlark")]
    #[tokio::test]
    async fn test_execute_compute_starlark_019() {
        use crate::compiler;

        const WF: &str = r#"
version: "3.0"
metadata:
  id: test-compute
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config:
        trigger_type: webhook
        response_mode: buffered
        end_activity: respond
        webhook_config: { path: /compute }
      output_variable: trigger_payload
    - id: pricing
      activity_type: Compute
      compute_config:
        language: starlark
        entry_fn: run
        source: |
          def run(ctx):
              subtotal = ctx["subtotal"]
              tiers = ctx["tiers"]
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
    - { id: f1, from: { node: trigger }, to: { node: pricing } }
    - { id: f2, from: { node: pricing }, to: { node: respond } }
    - { id: f3, from: { node: respond }, to: { node: end } }
"#;

        let graph = compiler::compile(WF).unwrap();
        let input = json!({
            "body": {
                "subtotal": 100,
                "tiers": [
                    {"min": 50, "pct": 10},
                    {"min": 200, "pct": 20}
                ]
            }
        });
        let result = execute(&graph, input, &ExecConfig::default())
            .await
            .unwrap();
        assert_eq!(result.output["final_price"], json!(90.0));
        assert_eq!(result.output["discount_pct"], 10);
    }

    #[cfg(feature = "compute-starlark")]
    #[tokio::test]
    async fn test_compute_starlark_v_starlark_parity() {
        use crate::compiler;

        let wf_tpl = r#"
version: "3.0"
metadata:
  id: test-parity
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config:
        trigger_type: webhook
        webhook_config: { path: /p }
        response_mode: buffered
        end_activity: respond
      output_variable: trigger_payload
    - id: calc
      activity_type: Compute
      compute_config:
        language: __LANG__
        entry_fn: run
        source: |
          def run(ctx):
              return {"doubled": ctx["x"] * 2}
      input_mappings:
        - target: x
          source: { language: spv1, source: "$.trigger_payload.x" }
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

        let starlark_wf = wf_tpl.replace("__LANG__", "starlark");
        let v_starlark_wf = wf_tpl.replace("__LANG__", "v-starlark");

        let input = json!({"x": 21});
        let g1 = compiler::compile(&starlark_wf).unwrap();
        let g2 = compiler::compile(&v_starlark_wf).unwrap();
        let r1 = execute(&g1, input.clone(), &ExecConfig::default())
            .await
            .unwrap();
        let r2 = execute(&g2, input, &ExecConfig::default()).await.unwrap();
        assert_eq!(r1.output, r2.output);
    }

    const H4_VALIDATE_WF: &str = r#"
version: "3.0"
metadata:
  id: h4-validate-exec
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config:
        trigger_type: webhook
        webhook_config:
          path: /validate
      output_variable: trigger_payload
    - id: validate
      activity_type: Validate
      validate_config:
        target: trigger_payload
        schema:
          type: object
          required: [body]
      output_variable: validated
    - id: end
      activity_type: End
  flows:
    - { id: f1, from: { node: trigger }, to: { node: validate } }
    - { id: f2, from: { node: validate }, to: { node: end } }
"#;

    #[tokio::test]
    async fn test_execute_validate_pass_and_records_validation() {
        let graph = compiler::compile(H4_VALIDATE_WF).unwrap();
        let result = execute(&graph, json!({"body": {"id": 1}}), &ExecConfig::default())
            .await
            .unwrap();
        assert_eq!(result.output["body"]["id"], 1);
        assert_eq!(result.variables["_validation"]["valid"], true);
    }

    #[tokio::test]
    async fn test_execute_validate_fail_is_errorboundary_ready() {
        let graph = compiler::compile(H4_VALIDATE_WF).unwrap();
        let err = execute(&graph, json!({"missing": true}), &ExecConfig::default())
            .await
            .unwrap_err();
        assert!(
            err.message.contains("validation failed"),
            "got: {}",
            err.message
        );
    }

    const H4_TIMER_SIGNAL_GATEWAY_WF: &str = r#"
version: "3.0"
metadata:
  id: h4-event-exec
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config:
        trigger_type: webhook
        webhook_config:
          path: /event
      output_variable: trigger_payload
    - id: wait
      activity_type: Timer
      timer_config:
        delay_ms: 0
      output_variable: wait_result
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
    - { id: f1, from: { node: trigger }, to: { node: wait } }
    - { id: f2, from: { node: wait }, to: { node: signal } }
    - { id: f3, from: { node: signal }, to: { node: gateway } }
    - { id: f4, from: { node: gateway }, to: { node: end } }
"#;

    #[tokio::test]
    async fn test_execute_timer_signal_event_gateway_special_vars() {
        let graph = compiler::compile(H4_TIMER_SIGNAL_GATEWAY_WF).unwrap();
        let result = execute(&graph, json!({"ok": true}), &ExecConfig::default())
            .await
            .unwrap();
        assert_eq!(result.variables["_signal"], "approved");
        assert!(result.variables.contains_key("gateway_result"));
        assert!(result.variables.contains_key("wait_result"));
        assert!(result.steps >= 4);
    }

    const H4_AUDIT_WF: &str = r#"
version: "3.0"
metadata:
  id: h4-audit-exec
spec:
  audit_log:
    events: [workflow_started, workflow_succeeded, workflow_failed]
    mode: async_best_effort
    sinks:
      - type: webhook
        url: http://audit.local/events
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config:
        trigger_type: webhook
        webhook_config:
          path: /audit
      output_variable: trigger_payload
    - id: end
      activity_type: End
  flows:
    - { id: f1, from: { node: trigger }, to: { node: end } }
"#;

    #[tokio::test]
    async fn test_execute_audit_log_records_cloudevents() {
        let graph = compiler::compile(H4_AUDIT_WF).unwrap();
        let result = execute(&graph, json!({"ok": true}), &ExecConfig::default())
            .await
            .unwrap();
        let events = result.variables["_audit_events"].as_array().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["specversion"], "1.0");
        assert_eq!(events[0]["type"], "workflow_started");
        assert_eq!(events[1]["type"], "workflow_succeeded");
    }
}
