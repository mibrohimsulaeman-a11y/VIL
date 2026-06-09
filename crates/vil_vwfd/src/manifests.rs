//! H4g/H5 manifest parsers and semantic validators for pack/tier/IaC
//! compile-contract validation.
//!
//! These parsers deliberately preserve unknown fields as YAML values so newer
//! manifest versions can be accepted without pulling heavy control-plane engines
//! into the default build. H5 adds semantic validation and dry-run evidence only;
//! no infrastructure mutation happens here.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::graph::{NodeKind, VilwGraph};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PackManifest {
    pub pack: Option<serde_yaml::Value>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub metadata: Option<serde_yaml::Value>,
    pub connections: Option<Vec<serde_yaml::Value>>,
    pub workflows: Option<Vec<serde_yaml::Value>>,
    pub triggers: Option<Vec<serde_yaml::Value>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TierManifest {
    pub version: Option<serde_yaml::Value>,
    pub kind: Option<String>,
    pub name: Option<String>,
    pub metadata: Option<serde_yaml::Value>,
    pub capabilities: Option<serde_yaml::Value>,
    pub connectors: Option<serde_yaml::Value>,
    pub triggers: Option<serde_yaml::Value>,
    pub rate_limits: Option<serde_yaml::Value>,
    pub limits: Option<serde_yaml::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IacResource {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: Option<serde_yaml::Value>,
    pub spec: Option<serde_yaml::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DryRunPlan {
    pub action: String,
    pub kind: String,
    pub identity: String,
    pub effects: Vec<String>,
}

pub fn parse_pack_manifest(yaml: &str) -> Result<PackManifest, serde_yaml::Error> {
    serde_yaml::from_str(yaml)
}

pub fn parse_tier_manifest(yaml: &str) -> Result<TierManifest, serde_yaml::Error> {
    serde_yaml::from_str(yaml)
}

pub fn parse_iac_resource(yaml: &str) -> Result<IacResource, serde_yaml::Error> {
    serde_yaml::from_str(yaml)
}

pub fn validate_pack_manifest(pack: &PackManifest) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let id = pack_identity(pack);
    if id.is_none() {
        errors.push("pack identity required: name or pack.id".into());
    }
    if pack_version(pack).is_none() {
        errors.push("pack version required: version or pack.version".into());
    }
    if pack
        .workflows
        .as_ref()
        .map(|w| w.is_empty())
        .unwrap_or(true)
    {
        errors.push("pack requires at least one workflow reference".into());
    }
    for (idx, conn) in pack
        .connections
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .enumerate()
    {
        let name = yaml_get_str(conn, &["name", "id"]);
        let kind = yaml_get_str(conn, &["kind", "connector", "connector_type"]);
        if name.is_none() {
            errors.push(format!("connections[{idx}] requires name or id"));
        }
        if kind.is_none() {
            errors.push(format!("connections[{idx}] requires kind/connector"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub fn validate_tier_manifest(tier: &TierManifest) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if tier_identity(tier).is_none() {
        errors.push("tier identity required: name or metadata.id/name".into());
    }
    if tier.triggers.is_none() {
        errors.push("tier trigger allow-list required".into());
    }
    if tier.connectors.is_none() {
        errors.push("tier connector allow-list required".into());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub fn validate_iac_resource(res: &IacResource) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if res.api_version.trim().is_empty() {
        errors.push("apiVersion is required".into());
    }
    let allowed = ["Tenant", "FleetHost", "Tier", "Pack", "Snapshot"];
    if !allowed.contains(&res.kind.as_str()) {
        errors.push(format!(
            "unsupported kind '{}'; expected one of {:?}",
            res.kind, allowed
        ));
    }
    if resource_identity(res).is_none() {
        errors.push("metadata.name or metadata.namespace identity is required".into());
    }
    match res.kind.as_str() {
        "Tenant" => {
            for path in ["tier_ref", "bundle", "resources"] {
                if !yaml_exists(res.spec.as_ref(), &[path]) {
                    errors.push(format!("Tenant.spec.{path} is required"));
                }
            }
        }
        "Pack" => {
            for path in ["pack_id", "version", "bundle"] {
                if !yaml_exists(res.spec.as_ref(), &[path]) {
                    errors.push(format!("Pack.spec.{path} is required"));
                }
            }
        }
        "Tier" if res.spec.is_none() => {
            errors.push("Tier.spec is required".into());
        }
        "Tier" => {}
        _ => {}
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub fn dry_run_apply_resource(res: &IacResource) -> Result<DryRunPlan, Vec<String>> {
    validate_iac_resource(res)?;
    let identity = resource_identity(res).unwrap_or_else(|| "unknown".into());
    let effects = match res.kind.as_str() {
        "Tenant" => vec![
            "validate tenant bundle digest/path".into(),
            "plan tenant resources and exposed ports".into(),
            "no provision side effects in dry-run".into(),
        ],
        "Tier" => vec![
            "validate tier allow-lists and limits".into(),
            "record tier policy plan".into(),
        ],
        "Pack" => vec![
            "validate pack identity/version bundle".into(),
            "plan pack install without connector allocation".into(),
        ],
        other => vec![format!("validate {other} resource")],
    };
    Ok(DryRunPlan {
        action: "dry_run_apply".into(),
        kind: res.kind.clone(),
        identity,
        effects,
    })
}

pub fn validate_workflow_against_tier(
    graph: &VilwGraph,
    tier: &TierManifest,
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let allowed_connectors = tier_connectors(tier);
    let allowed_triggers = tier_triggers(tier);

    if !allowed_triggers.is_empty() && !allowed_triggers.contains(&graph.trigger_type) {
        errors.push(format!(
            "trigger '{}' denied by tier policy (allowed: {})",
            graph.trigger_type,
            allowed_triggers
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(",")
        ));
    }

    for node in &graph.nodes {
        if node.kind != NodeKind::Connector {
            continue;
        }
        let marker = node
            .config
            .get("connector_type")
            .or_else(|| node.config.get("connector_ref"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let family = connector_family(marker);
        if !allowed_connectors.is_empty()
            && !allowed_connectors.contains(marker)
            && !allowed_connectors.contains(&family)
        {
            errors.push(format!(
                "connector '{}' denied by tier policy (family '{}')",
                marker, family
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub fn tier_connectors(tier: &TierManifest) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    collect_allow_values(tier.connectors.as_ref(), &mut out);
    out
}

pub fn tier_triggers(tier: &TierManifest) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    collect_allow_values(tier.triggers.as_ref(), &mut out);
    out
}

fn collect_allow_values(value: Option<&serde_yaml::Value>, out: &mut BTreeSet<String>) {
    let Some(value) = value else {
        return;
    };
    match value {
        serde_yaml::Value::Sequence(items) => {
            for item in items {
                if let Some(s) = item.as_str() {
                    out.insert(s.to_string());
                }
            }
        }
        serde_yaml::Value::Mapping(map) => {
            if let Some(allow) = yaml_map_get(map, "allow") {
                collect_allow_values(Some(allow), out);
            }
            for (_, child) in map {
                collect_allow_values(Some(child), out);
            }
        }
        serde_yaml::Value::String(s) => {
            out.insert(s.clone());
        }
        _ => {}
    }
}

fn pack_identity(pack: &PackManifest) -> Option<String> {
    pack.name
        .clone()
        .or_else(|| yaml_path(pack.pack.as_ref(), &["id"]))
}

fn pack_version(pack: &PackManifest) -> Option<String> {
    pack.version
        .clone()
        .or_else(|| yaml_path(pack.pack.as_ref(), &["version"]))
}

fn tier_identity(tier: &TierManifest) -> Option<String> {
    tier.name
        .clone()
        .or_else(|| yaml_path(tier.metadata.as_ref(), &["id"]))
        .or_else(|| yaml_path(tier.metadata.as_ref(), &["name"]))
}

fn resource_identity(res: &IacResource) -> Option<String> {
    yaml_path(res.metadata.as_ref(), &["name"])
        .or_else(|| yaml_path(res.metadata.as_ref(), &["namespace"]))
}

fn yaml_get_str(value: &serde_yaml::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| yaml_path(Some(value), &[*k]))
}

fn yaml_exists(value: Option<&serde_yaml::Value>, path: &[&str]) -> bool {
    let Some(mut current) = value else {
        return false;
    };
    for key in path {
        let serde_yaml::Value::Mapping(map) = current else {
            return false;
        };
        let Some(next) = yaml_map_get(map, key) else {
            return false;
        };
        current = next;
    }
    true
}

fn yaml_path(value: Option<&serde_yaml::Value>, path: &[&str]) -> Option<String> {
    let mut current = value?;
    for key in path {
        let serde_yaml::Value::Mapping(map) = current else {
            return None;
        };
        current = yaml_map_get(map, key)?;
    }
    match current {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn yaml_map_get<'a>(map: &'a serde_yaml::Mapping, key: &str) -> Option<&'a serde_yaml::Value> {
    map.get(serde_yaml::Value::String(key.to_string()))
}

fn connector_family(marker: &str) -> String {
    let m = marker
        .trim()
        .trim_start_matches("vastar.")
        .trim_start_matches("pack://");
    if m.contains("postgres") || m.contains("sqlite") || m.contains("sql") {
        "database".into()
    } else if m.contains("redis") {
        "redis".into()
    } else if m.contains("mongo") {
        "mongo".into()
    } else if m.contains("cassandra") {
        "cassandra".into()
    } else if m.contains("clickhouse") {
        "clickhouse".into()
    } else if m.contains("elastic") {
        "elastic".into()
    } else if m.contains("nats")
        || m.contains("kafka")
        || m.contains("mqtt")
        || m.contains("rabbitmq")
    {
        "mq".into()
    } else if m.contains("s3") || m.contains("gcs") || m.contains("azure") {
        "storage".into()
    } else if m.contains("grpc")
        || m.contains("http")
        || m.contains("modbus")
        || m.contains("opcua")
    {
        "protocol".into()
    } else {
        m.split(['.', '/']).next().unwrap_or(m).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile;

    #[test]
    fn parses_pack_manifest_shape() {
        let pack = parse_pack_manifest(
            r#"
name: retail-pack
version: "1.0"
connections:
  - id: pg
    connector: postgres
workflows:
  - path: workflows/order.yaml
"#,
        )
        .unwrap();
        assert_eq!(pack.name.as_deref(), Some("retail-pack"));
        assert_eq!(pack.connections.as_ref().unwrap().len(), 1);
        assert_eq!(pack.workflows.as_ref().unwrap().len(), 1);
        assert!(validate_pack_manifest(&pack).is_ok());
    }

    #[test]
    fn parses_reference_pack_manifest_shape() {
        let pack = parse_pack_manifest(include_str!("../../../docs-dev-jangan-ditrack/vil-compat-gap/aa-dev-ref/examples-vflow/packs/hello-db/pack.yaml")).unwrap();
        assert_eq!(pack_identity(&pack).as_deref(), Some("examples/hello-db"));
        validate_pack_manifest(&pack).unwrap();
    }

    #[test]
    fn parses_tier_manifest_shape() {
        let tier = parse_tier_manifest(
            r#"
name: pro
capabilities: [workflow_exec, audit_log]
connectors: [http, postgres, nats]
triggers: [webhook, cron, nats_js]
rate_limits:
  workflow_exec_per_minute: 1000
"#,
        )
        .unwrap();
        let connectors = tier_connectors(&tier);
        assert!(connectors.contains("postgres"));
        assert!(tier_triggers(&tier).contains("nats_js"));
        assert!(validate_tier_manifest(&tier).is_ok());
    }

    #[test]
    fn parses_reference_tier_manifest_shape() {
        let tier = parse_tier_manifest(include_str!("../../../docs-dev-jangan-ditrack/vil-compat-gap/aa-dev-ref/examples-vflow/tiers/starter.yaml")).unwrap();
        validate_tier_manifest(&tier).unwrap();
        assert!(tier_triggers(&tier).contains("webhook"));
        assert!(tier_connectors(&tier).contains("sqlite"));
    }

    #[test]
    fn parses_iac_resource_shape() {
        let res = parse_iac_resource(
            r#"
apiVersion: vil.vastar.ai/v1
kind: Pack
metadata:
  name: retail-pack
spec:
  pack_id: retail-pack
  version: "1.0"
  bundle: { digest: "sha256:abc", path: /tmp/pack.tar.gz }
"#,
        )
        .unwrap();
        assert_eq!(res.kind, "Pack");
        assert_eq!(res.api_version, "vil.vastar.ai/v1");
        assert!(validate_iac_resource(&res).is_ok());
        let plan = dry_run_apply_resource(&res).unwrap();
        assert_eq!(plan.action, "dry_run_apply");
        assert!(plan
            .effects
            .iter()
            .any(|e| e.contains("without connector allocation")));
    }

    #[test]
    fn invalid_manifests_report_precise_errors() {
        let pack = parse_pack_manifest("connections: [{}]\nworkflows: []\n").unwrap();
        let errors = validate_pack_manifest(&pack).unwrap_err();
        assert!(errors.iter().any(|e| e.contains("identity")));
        assert!(errors.iter().any(|e| e.contains("version")));
        assert!(errors.iter().any(|e| e.contains("workflow")));

        let tier = parse_tier_manifest("name: starter\n").unwrap();
        let errors = validate_tier_manifest(&tier).unwrap_err();
        assert!(errors.iter().any(|e| e.contains("trigger")));

        let res = parse_iac_resource(
            "apiVersion: vflow.cloud/v1\nkind: Unknown\nmetadata: { name: bad }\n",
        )
        .unwrap();
        assert!(validate_iac_resource(&res).unwrap_err()[0].contains("unsupported kind"));
    }

    #[test]
    fn tier_policy_allows_and_denies_workflow_declarations() {
        let tier = parse_tier_manifest(
            r#"
name: starter
connectors:
  protocol: { allow: [http] }
triggers: { allow: [webhook] }
"#,
        )
        .unwrap();
        let graph = compile(
            r#"
version: "3.0"
metadata: { id: tier-ok }
spec:
  activities:
    - id: trigger
      activity_type: Trigger
      trigger_config: { trigger_type: webhook, webhook_config: { path: /ok } }
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
        validate_workflow_against_tier(&graph, &tier).unwrap();

        let denied =
            parse_tier_manifest("name: locked\nconnectors: [redis]\ntriggers: [cron]\n").unwrap();
        let errors = validate_workflow_against_tier(&graph, &denied).unwrap_err();
        assert!(errors
            .iter()
            .any(|e| e.contains("trigger 'webhook' denied")));
        assert!(errors.iter().any(|e| e.contains("connector 'http' denied")));
    }
}
