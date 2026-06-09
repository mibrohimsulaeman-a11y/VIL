use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use vil_vwfd::executor::{AuditSinkFn, ConnectorFn};
use vil_vwfd::manifests::{
    dry_run_apply_resource, parse_iac_resource, parse_pack_manifest, parse_tier_manifest,
    validate_iac_resource, validate_pack_manifest, validate_tier_manifest,
    validate_workflow_against_tier,
};
use vil_vwfd::{compile, execute, ExecConfig};

type BoxedConnectorFuture = Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;

fn boxed_connector<F>(f: F) -> ConnectorFn
where
    F: Fn(&str, &str, &Value) -> Result<Value, String> + Send + Sync + 'static,
{
    Arc::new(move |connector_ref, operation, input| {
        let result = f(connector_ref, operation, input);
        Box::pin(async move { result }) as BoxedConnectorFuture
    })
}

#[tokio::test]
async fn h5b_trigger_runtime_smokes_injected_events() {
    let cases = [
        (
            "nats_js",
            "nats_js: { stream: orders, subject: orders.created }",
        ),
        ("nats_kv", "nats_kv: { bucket: kv, key: orders.* }"),
        ("cdc", "cdc: { source: postgres, table: orders }"),
        ("db_poll", "db_poll: { table: orders, interval_ms: 1000 }"),
        ("fs", "fs: { path: /tmp/inbox, pattern: '*.json' }"),
    ];

    for (trigger_type, extra) in cases {
        let yaml = format!(
            r#"
version: "3.0"
metadata: {{ id: h5-trigger-{trigger_type} }}
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
        let graph = compile(&yaml).unwrap();
        let result = execute(
            &graph,
            json!({"event_id": format!("{trigger_type}-1"), "body": {"amount": 7}}),
            &ExecConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(result.variables["_trigger"], trigger_type);
        assert_eq!(result.variables["trigger_payload"]["body"]["amount"], 7);
    }
}

#[tokio::test]
async fn h5b_grpc_body_schema_exposes_typed_trigger_body() {
    let graph = compile(
        r#"
version: "3.0"
metadata: { id: h5-grpc-body-schema }
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config:
        trigger_type: grpc
        grpc: { service: QuoteService, method: Quote }
        body_schema: "examples.QuoteRequest"
        proto_field: payload
      output_variable: trigger_payload
    - id: normalize
      activity_type: Transform
      input_mappings:
        - target: amount
          source: { language: v-cel, source: trigger_body.amount }
        - target: country
          source: { language: v-cel, source: trigger_body.country }
      output_variable: normalized
    - id: end
      activity_type: End
  flows:
    - { id: f1, from: { node: trigger }, to: { node: normalize } }
    - { id: f2, from: { node: normalize }, to: { node: end } }
"#,
    )
    .unwrap();
    let result = execute(
        &graph,
        json!({"body": {"amount": 125, "country": "ID"}}),
        &ExecConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(result.variables["_trigger"], "grpc");
    assert_eq!(result.variables["_body_schema"], "examples.QuoteRequest");
    assert_eq!(result.variables["_proto_field"], "payload");
    assert_eq!(result.variables["normalized"]["amount"], 125);
    assert_eq!(result.variables["normalized"]["country"], "ID");
}

#[tokio::test]
async fn h5c_connector_runtime_matrix_uses_controlled_stub() {
    let cases = [
        ("vastar.http", "http", "get"),
        ("vastar.grpc", "grpc", "call"),
        ("vastar.db.postgres", "postgres", "raw_query"),
        ("vastar.redis", "redis", "get"),
        ("vastar.mongo", "mongo", "find"),
        ("vastar.cassandra", "cassandra", "query"),
        ("vastar.clickhouse", "clickhouse", "query"),
        ("vastar.elastic", "elastic", "search"),
        ("vastar.s3", "s3", "put"),
        ("vastar.gcs", "gcs", "put"),
        ("vastar.azure", "azure", "put"),
        ("vastar.nats", "nats", "publish"),
        ("vastar.kafka", "kafka", "publish"),
        ("vastar.mqtt", "mqtt", "publish"),
        ("vastar.rabbitmq", "rabbitmq", "publish"),
        ("vastar.codec.protobuf", "protobuf", "encode"),
        ("vastar.codec.msgpack", "msgpack", "encode"),
        ("vastar.codec.iso8583", "iso8583", "encode"),
        ("vastar.modbus", "modbus", "read"),
        ("vastar.opcua", "opcua", "read"),
    ];

    for (connector_ref, connector_type, operation) in cases {
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let calls_for_stub = calls.clone();
        let connector_fn = boxed_connector(move |got_ref, got_op, input| {
            calls_for_stub.lock().unwrap().push(json!({
                "connector_ref": got_ref,
                "operation": got_op,
                "input": input,
            }));
            Ok(json!({"ok": true, "connector_ref": got_ref, "operation": got_op}))
        });
        let yaml = format!(
            r#"
version: "3.0"
metadata: {{ id: h5-connector-{connector_type} }}
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config: {{ trigger_type: webhook, webhook_config: {{ path: /c }} }}
      output_variable: trigger_payload
    - id: call
      activity_type: Connector
      connector_config:
        connector_ref: {connector_ref}
        connector_type: {connector_type}
        operation: {operation}
      input_mappings:
        - target: payload
          source: {{ language: v-cel, source: trigger_payload.payload }}
      output_variable: call_result
    - id: end
      activity_type: End
  flows:
    - {{ id: f1, from: {{ node: trigger }}, to: {{ node: call }} }}
    - {{ id: f2, from: {{ node: call }}, to: {{ node: end }} }}
"#
        );
        let graph = compile(&yaml).unwrap();
        let cfg = ExecConfig {
            connector_fn: Some(connector_fn),
            ..ExecConfig::default()
        };
        let result = execute(&graph, json!({"payload": {"id": connector_type}}), &cfg)
            .await
            .unwrap();
        assert_eq!(result.variables["call_result"]["ok"], true);
        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0]["connector_ref"], connector_ref);
        assert_eq!(recorded[0]["operation"], operation);
        assert_eq!(recorded[0]["input"]["_connector_ref"], connector_ref);
        assert_eq!(recorded[0]["input"]["_connector_type"], connector_type);
        assert_eq!(recorded[0]["input"]["_operation"], operation);
        assert_eq!(recorded[0]["input"]["payload"]["id"], connector_type);
    }
}

#[tokio::test]
async fn h5c_connector_negative_cases_are_precise() {
    let missing_ref = compile(
        r#"
version: "3.0"
metadata: { id: h5-missing-ref }
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config: { trigger_type: webhook, webhook_config: { path: /x } }
    - id: call
      activity_type: Connector
      connector_config: { operation: get }
    - id: end
      activity_type: End
  flows:
    - { id: f1, from: { node: trigger }, to: { node: call } }
    - { id: f2, from: { node: call }, to: { node: end } }
"#,
    )
    .unwrap();
    let cfg = ExecConfig {
        connector_fn: Some(boxed_connector(|connector_ref, _, _| {
            if connector_ref.is_empty() {
                Err("missing connector_ref".into())
            } else {
                Ok(json!({}))
            }
        })),
        ..ExecConfig::default()
    };
    let err = execute(&missing_ref, json!({}), &cfg).await.unwrap_err();
    assert!(err.to_string().contains("missing connector_ref"));

    let bad_op = compile(
        r#"
version: "3.0"
metadata: { id: h5-bad-op }
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config: { trigger_type: webhook, webhook_config: { path: /x } }
    - id: call
      activity_type: Connector
      connector_config: { connector_ref: vastar.http, operation: explode }
    - id: end
      activity_type: End
  flows:
    - { id: f1, from: { node: trigger }, to: { node: call } }
    - { id: f2, from: { node: call }, to: { node: end } }
"#,
    )
    .unwrap();
    let cfg = ExecConfig {
        connector_fn: Some(boxed_connector(|_, operation, _| {
            if operation == "explode" {
                Err("unsupported operation explode".into())
            } else {
                Ok(json!({}))
            }
        })),
        ..ExecConfig::default()
    };
    let err = execute(&bad_op, json!({}), &cfg).await.unwrap_err();
    assert!(err.to_string().contains("unsupported operation"));

    let bad_bytes = compile(
        r#"
version: "3.0"
metadata: { id: h5-bad-bytes }
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config: { trigger_type: webhook, webhook_config: { path: /x } }
    - id: call
      activity_type: Connector
      connector_config: { connector_ref: vastar.codec.protobuf, operation: encode }
      input_mappings:
        - target: raw
          source: { language: bytes_ref, source: "$.missing_bytes" }
    - id: end
      activity_type: End
  flows:
    - { id: f1, from: { node: trigger }, to: { node: call } }
    - { id: f2, from: { node: call }, to: { node: end } }
"#,
    )
    .unwrap();
    let cfg = ExecConfig {
        connector_fn: Some(boxed_connector(|_, _, input| {
            if input["raw"].is_null() {
                Err("invalid raw-bytes mapping: raw is null".into())
            } else {
                Ok(json!({}))
            }
        })),
        ..ExecConfig::default()
    };
    let err = execute(&bad_bytes, json!({}), &cfg).await.unwrap_err();
    assert!(err.to_string().contains("invalid raw-bytes mapping"));

    let dialect_mismatch = compile(
        r#"
version: "3.0"
metadata: { id: h5-dialect-mismatch }
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config: { trigger_type: webhook, webhook_config: { path: /x } }
      output_variable: trigger_payload
    - id: call
      activity_type: Connector
      connector_config: { connector_ref: vastar.cassandra, connector_type: cassandra, operation: query }
      input_mappings:
        - target: query
          source:
            language: vil_query
            source: |
              dialect: postgres
              select("orders").where_eq("id", trigger_payload.id)
    - id: end
      activity_type: End
  flows:
    - { id: f1, from: { node: trigger }, to: { node: call } }
    - { id: f2, from: { node: call }, to: { node: end } }
"#,
    )
    .unwrap();
    let cfg = ExecConfig {
        connector_fn: Some(boxed_connector(|connector_ref, _, input| {
            if connector_ref.contains("cassandra")
                && input["sql"].as_str().unwrap_or("").contains("$1")
            {
                Err("dialect mismatch: postgres placeholder sent to cassandra".into())
            } else {
                Ok(json!({}))
            }
        })),
        ..ExecConfig::default()
    };
    let err = execute(&dialect_mismatch, json!({"id": 1}), &cfg)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("dialect mismatch"));
}

#[tokio::test]
async fn h5d_audit_sink_dispatch_uses_test_sinks_only() {
    let delivered = Arc::new(Mutex::new(Vec::<Value>::new()));
    let delivered_for_sink = delivered.clone();
    let audit_sink_fn: AuditSinkFn = Arc::new(move |sink, event| {
        delivered_for_sink
            .lock()
            .unwrap()
            .push(json!({"sink": sink, "event": event}));
        Ok(())
    });

    let graph = compile(
        r#"
version: "3.0"
metadata: { id: h5-audit-sinks }
spec:
  audit_log:
    events: [workflow_started, activity_started, activity_succeeded, workflow_succeeded]
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
    - id: call
      activity_type: Connector
      connector_config: { connector_ref: vastar.http, connector_type: http, operation: get }
      output_variable: call_result
    - id: end
      activity_type: End
  flows:
    - { id: f1, from: { node: trigger }, to: { node: call } }
    - { id: f2, from: { node: call }, to: { node: end } }
"#,
    )
    .unwrap();
    let cfg = ExecConfig {
        audit_sink_fn: Some(audit_sink_fn),
        ..ExecConfig::default()
    };
    let result = execute(&graph, json!({"ok": true}), &cfg).await.unwrap();
    let events = result.variables["_audit_events"].as_array().unwrap();
    assert!(events.iter().any(|e| e["type"] == "workflow_started"));
    assert!(events.iter().any(|e| e["type"] == "activity_started"));
    assert!(events.iter().any(|e| e["type"] == "activity_succeeded"));
    assert!(events.iter().any(|e| e["type"] == "workflow_succeeded"));

    let delivered = delivered.lock().unwrap();
    assert!(delivered.iter().any(|d| d["sink"]["type"] == "webhook"));
    assert!(delivered.iter().any(|d| d["sink"]["type"] == "nats"));
    assert!(delivered.iter().all(|d| d["event"]["specversion"] == "1.0"));
}

#[tokio::test]
async fn h5d_audit_disabled_event_and_malformed_sink_are_distinguishable() {
    assert!(vil_vwfd::audit::validate_sink_config(&json!({"type": "webhook"})).is_err());

    let graph = compile(
        r#"
version: "3.0"
metadata: { id: h5-audit-disabled-event }
spec:
  audit_log:
    events: [workflow_started]
    sinks:
      - type: webhook
        url: http://audit.local/events
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config: { trigger_type: webhook, webhook_config: { path: /audit } }
    - id: end
      activity_type: End
  flows:
    - { id: f1, from: { node: trigger }, to: { node: end } }
"#,
    )
    .unwrap();
    let result = execute(&graph, json!({}), &ExecConfig::default())
        .await
        .unwrap();
    let events = result.variables["_audit_events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["type"], "workflow_started");
}

#[test]
fn h5e_manifest_validation_and_dry_run_apply() {
    let pack = parse_pack_manifest(include_str!(
        "../../../docs-dev-jangan-ditrack/vil-compat-gap/aa-dev-ref/examples-vflow/packs/hello-db/pack.yaml"
    ))
    .unwrap();
    validate_pack_manifest(&pack).unwrap();

    let tier = parse_tier_manifest(include_str!(
        "../../../docs-dev-jangan-ditrack/vil-compat-gap/aa-dev-ref/examples-vflow/tiers/starter.yaml"
    ))
    .unwrap();
    validate_tier_manifest(&tier).unwrap();

    let res = parse_iac_resource(
        r#"
apiVersion: vflow.cloud/v1
kind: Pack
metadata: { name: examples-hello-db }
spec:
  pack_id: examples/hello-db
  version: 0.1.0
  bundle: { digest: "sha256:abc", path: /var/lib/vflow/packs/hello-db.tar.gz }
  tier_ref: starter
"#,
    )
    .unwrap();
    validate_iac_resource(&res).unwrap();
    let plan = dry_run_apply_resource(&res).unwrap();
    assert_eq!(plan.kind, "Pack");
    assert!(plan
        .effects
        .iter()
        .any(|e| e.contains("without connector allocation")));
}

#[test]
fn h5e_tier_policy_enforces_allowed_connector_and_trigger() {
    let tier = parse_tier_manifest(
        r#"
name: starter
connectors:
  protocol: { allow: [http] }
triggers: { allow: [webhook] }
"#,
    )
    .unwrap();
    let ok = compile(
        r#"
version: "3.0"
metadata: { id: h5-tier-ok }
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config: { trigger_type: webhook, webhook_config: { path: /ok } }
    - id: call
      activity_type: Connector
      connector_config: { connector_ref: vastar.http, connector_type: http, operation: get }
    - id: end
      activity_type: End
  flows:
    - { id: f1, from: { node: trigger }, to: { node: call } }
    - { id: f2, from: { node: call }, to: { node: end } }
"#,
    )
    .unwrap();
    validate_workflow_against_tier(&ok, &tier).unwrap();

    let denied_tier =
        parse_tier_manifest("name: locked\nconnectors: [redis]\ntriggers: [cron]\n").unwrap();
    let errors = validate_workflow_against_tier(&ok, &denied_tier).unwrap_err();
    assert!(errors
        .iter()
        .any(|e| e.contains("trigger 'webhook' denied")));
    assert!(errors.iter().any(|e| e.contains("connector 'http' denied")));
}
