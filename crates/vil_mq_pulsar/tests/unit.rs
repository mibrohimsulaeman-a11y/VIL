//! Pure unit tests for `vil_mq_pulsar` (no live broker required).

use vil_mq_pulsar::{PulsarConfig, PulsarFault};

#[test]
fn config_new_applies_defaults() {
    let c = PulsarConfig::new("pulsar://localhost:6650", "public", "default");
    assert_eq!(c.url, "pulsar://localhost:6650");
    assert_eq!(c.tenant, "public");
    assert_eq!(c.namespace, "default");
    assert!(c.auth_token.is_none());
    assert_eq!(c.operation_timeout_ms, 30_000);
    assert_eq!(c.connection_timeout_ms, 5_000);
}

#[test]
fn config_with_token_sets_auth() {
    let c = PulsarConfig::new("pulsar://h", "t", "n").with_token("secret");
    assert_eq!(c.auth_token.as_deref(), Some("secret"));
}

#[test]
fn topic_fqn_builds_persistent_path() {
    let c = PulsarConfig::new("pulsar://h", "acme", "ns1");
    assert_eq!(c.topic_fqn("orders"), "persistent://acme/ns1/orders");
}

#[test]
fn config_serde_roundtrip_and_defaults() {
    let c = PulsarConfig::new("pulsar://h", "t", "n").with_token("tok");
    let json = serde_json::to_string(&c).expect("serialize");
    let back: PulsarConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.url, "pulsar://h");
    assert_eq!(back.tenant, "t");
    assert_eq!(back.namespace, "n");
    assert_eq!(back.auth_token.as_deref(), Some("tok"));

    let partial = r#"{"url":"pulsar://h","tenant":"t","namespace":"n"}"#;
    let d: PulsarConfig = serde_json::from_str(partial).expect("deserialize");
    assert!(d.auth_token.is_none());
    assert_eq!(d.operation_timeout_ms, 30_000);
    assert_eq!(d.connection_timeout_ms, 5_000);
}

#[test]
fn fault_variants_carry_expected_fields() {
    let f = PulsarFault::ConsumerFailed { topic_hash: 11, subscription_hash: 22 };
    match f {
        PulsarFault::ConsumerFailed { topic_hash, subscription_hash } => {
            assert_eq!(topic_hash, 11);
            assert_eq!(subscription_hash, 22);
        }
        _ => panic!("unexpected variant"),
    }

    let s = PulsarFault::SendFailed { topic_hash: 1, error_code: 7 };
    match s {
        PulsarFault::SendFailed { topic_hash, error_code } => {
            assert_eq!(topic_hash, 1);
            assert_eq!(error_code, 7);
        }
        _ => panic!("unexpected variant"),
    }
}
