//! Pure unit tests for `vil_mq_pubsub` (no GCP calls required).

use vil_mq_pubsub::{PubSubConfig, PubSubFault};

#[test]
fn config_new_applies_defaults() {
    let c = PubSubConfig::new("proj", "topic", "sub");
    assert_eq!(c.project_id, "proj");
    assert_eq!(c.topic, "topic");
    assert_eq!(c.subscription, "sub");
    assert!(c.emulator_host.is_none());
    assert_eq!(c.max_messages, 10);
    assert_eq!(c.ack_deadline_secs, 60);
}

#[test]
fn with_emulator_sets_host() {
    let c = PubSubConfig::new("p", "t", "s").with_emulator("localhost:8085");
    assert_eq!(c.emulator_host.as_deref(), Some("localhost:8085"));
}

#[test]
fn resource_paths_are_well_formed() {
    let c = PubSubConfig::new("proj", "orders", "orders-sub");
    assert_eq!(c.topic_path(), "projects/proj/topics/orders");
    assert_eq!(c.subscription_path(), "projects/proj/subscriptions/orders-sub");
}

#[test]
fn config_serde_roundtrip_and_defaults() {
    let c = PubSubConfig::new("p", "t", "s").with_emulator("localhost:8085");
    let json = serde_json::to_string(&c).expect("serialize");
    let back: PubSubConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.project_id, "p");
    assert_eq!(back.emulator_host.as_deref(), Some("localhost:8085"));

    let partial = r#"{"project_id":"p","topic":"t","subscription":"s"}"#;
    let d: PubSubConfig = serde_json::from_str(partial).expect("deserialize");
    assert!(d.emulator_host.is_none());
    assert_eq!(d.max_messages, 10);
    assert_eq!(d.ack_deadline_secs, 60);
}

#[test]
fn fault_variants_carry_expected_fields() {
    let f = PubSubFault::PublishFailed { topic_hash: 4, status_code: 13 };
    match f {
        PubSubFault::PublishFailed { topic_hash, status_code } => {
            assert_eq!(topic_hash, 4);
            assert_eq!(status_code, 13);
        }
        _ => panic!("unexpected variant"),
    }

    let s = PubSubFault::SubscriberFailed { subscription_hash: 99 };
    match s {
        PubSubFault::SubscriberFailed { subscription_hash } => {
            assert_eq!(subscription_hash, 99);
        }
        _ => panic!("unexpected variant"),
    }
}
