//! H4d/H5 audit helpers: build CloudEvents-compatible envelopes and resolve
//! declarative audit sink declarations without forcing sink I/O onto the
//! workflow response path.

use serde_json::{json, Value};

pub fn cloud_event_envelope(
    workflow_id: &str,
    event_type: &str,
    subject: Option<&str>,
    data: Value,
) -> Value {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    json!({
        "specversion": "1.0",
        "id": format!("{}:{}", event_type, ts),
        "source": format!("vil://workflow/{}", workflow_id),
        "type": event_type,
        "subject": subject.unwrap_or(workflow_id),
        "time_unix_ms": ts,
        "datacontenttype": "application/json",
        "data": data,
    })
}

pub fn audit_event_enabled(audit_log: Option<&Value>, event_type: &str) -> bool {
    let Some(cfg) = audit_log else {
        return false;
    };
    audit_event_enabled_in_value(cfg, event_type)
}

pub fn audit_sinks_for_event(audit_log: Option<&Value>, event_type: &str) -> Vec<Value> {
    let Some(cfg) = audit_log else {
        return Vec::new();
    };
    let mut sinks = Vec::new();
    collect_sinks(cfg, event_type, &mut sinks);
    sinks
}

pub fn validate_sink_config(sink: &Value) -> Result<(), String> {
    let kind = sink
        .get("type")
        .or_else(|| sink.get("kind"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match kind {
        "webhook"
            if sink
                .get("url")
                .or_else(|| sink.get("endpoint"))
                .and_then(|v| v.as_str())
                .is_none() =>
        {
            return Err("webhook audit sink requires url or endpoint".into());
        }
        "nats" if sink.get("subject").and_then(|v| v.as_str()).is_none() => {
            return Err("nats audit sink requires subject".into());
        }
        "" if sink.get("sink_ref").and_then(|v| v.as_str()).is_none() => {
            return Err("audit sink requires type/kind or sink_ref".into());
        }
        _ => {}
    }
    Ok(())
}

fn collect_sinks(cfg: &Value, event_type: &str, out: &mut Vec<Value>) {
    if !audit_event_enabled_in_value(cfg, event_type) {
        return;
    }

    if let Some(items) = cfg.get("sinks").and_then(|v| v.as_array()) {
        for sink in items {
            out.push(sink.clone());
        }
    } else if is_sink_like(cfg) {
        out.push(cfg.clone());
    }

    if let Some(obj) = cfg.as_object() {
        for (key, child) in obj {
            if matches!(key.as_str(), "sinks" | "events" | "on" | "mode" | "enabled") {
                continue;
            }
            if child.is_object()
                && (child.get("sink_ref").is_some()
                    || child.get("sinks").is_some()
                    || child.get("on").is_some())
            {
                collect_sinks(child, event_type, out);
            }
        }
    }
}

fn is_sink_like(value: &Value) -> bool {
    value.get("sink_ref").is_some()
        || value.get("type").is_some()
        || value.get("kind").is_some()
        || value.get("url").is_some()
        || value.get("endpoint").is_some()
        || value.get("subject").is_some()
}

fn audit_event_enabled_in_value(cfg: &Value, event_type: &str) -> bool {
    if cfg.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
        return false;
    }

    if event_list_contains(cfg.get("events"), event_type)
        || event_list_contains(cfg.get("on"), event_type)
    {
        return true;
    }

    if cfg.get("events").is_some() || cfg.get("on").is_some() {
        return false;
    }

    // Named-channel audit_log shape used by the reference examples:
    // audit_log: { compliance_log: { on: [...] }, trace_log: { on: [...] } }
    if let Some(obj) = cfg.as_object() {
        let mut saw_channel = false;
        for (key, child) in obj {
            if matches!(key.as_str(), "sinks" | "mode" | "enabled") || !child.is_object() {
                continue;
            }
            if child.get("on").is_some()
                || child.get("events").is_some()
                || child.get("sink_ref").is_some()
                || child.get("sinks").is_some()
            {
                saw_channel = true;
                if audit_event_enabled_in_value(child, event_type) {
                    return true;
                }
            }
        }
        if saw_channel {
            return false;
        }
    }

    // If no event filter is provided, enabled audit config applies to all events.
    true
}

fn event_list_contains(value: Option<&Value>, event_type: &str) -> bool {
    let Some(Value::Array(events)) = value else {
        return false;
    };
    events.iter().any(|v| {
        v.as_str()
            .map(|s| s == "*" || s == event_type)
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_event_envelope_shape() {
        let ev = cloud_event_envelope("wf", "workflow_started", Some("run-1"), json!({"x": 1}));
        assert_eq!(ev["specversion"], "1.0");
        assert_eq!(ev["source"], "vil://workflow/wf");
        assert_eq!(ev["type"], "workflow_started");
        assert_eq!(ev["subject"], "run-1");
        assert_eq!(ev["data"]["x"], 1);
    }

    #[test]
    fn named_channel_sink_resolution() {
        let cfg = json!({
            "compliance": {
                "enabled": true,
                "sink_ref": "pack://audit/nats",
                "subject": "audit.workflow",
                "on": ["workflow_started"]
            },
            "trace": {
                "enabled": true,
                "sink_ref": "pack://audit/trace",
                "on": ["activity_succeeded"]
            }
        });
        let sinks = audit_sinks_for_event(Some(&cfg), "workflow_started");
        assert_eq!(sinks.len(), 1);
        assert_eq!(sinks[0]["sink_ref"], "pack://audit/nats");
        assert!(audit_sinks_for_event(Some(&cfg), "workflow_failed").is_empty());
    }

    #[test]
    fn malformed_sink_is_rejected() {
        assert!(validate_sink_config(&json!({"type": "webhook"})).is_err());
        assert!(validate_sink_config(&json!({"type": "nats"})).is_err());
        assert!(validate_sink_config(&json!({"type": "nats", "subject": "s"})).is_ok());
    }
}
