//! Pure unit tests for `vil_mq_sqs` (no AWS calls required).

use vil_mq_sqs::{SqsConfig, SqsFault, SqsMessage};

#[test]
fn config_new_applies_defaults() {
    let c = SqsConfig::new("us-east-1", "https://sqs.us-east-1.amazonaws.com/123/q");
    assert_eq!(c.region, "us-east-1");
    assert_eq!(c.queue_url, "https://sqs.us-east-1.amazonaws.com/123/q");
    assert!(c.endpoint.is_none());
    assert_eq!(c.max_messages, 10);
    assert_eq!(c.visibility_timeout_secs, 30);
    assert_eq!(c.wait_time_secs, 20);
}

#[test]
fn with_endpoint_sets_custom_endpoint() {
    let c = SqsConfig::new("us-east-1", "q").with_endpoint("http://localhost:4566");
    assert_eq!(c.endpoint.as_deref(), Some("http://localhost:4566"));
}

#[test]
fn with_max_messages_clamps_to_valid_range() {
    assert_eq!(SqsConfig::new("r", "q").with_max_messages(50).max_messages, 10);
    assert_eq!(SqsConfig::new("r", "q").with_max_messages(0).max_messages, 1);
    assert_eq!(SqsConfig::new("r", "q").with_max_messages(5).max_messages, 5);
}

#[test]
fn config_serde_roundtrip_and_defaults() {
    let c = SqsConfig::new("eu-west-1", "q").with_endpoint("http://e");
    let json = serde_json::to_string(&c).expect("serialize");
    let back: SqsConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.region, "eu-west-1");
    assert_eq!(back.endpoint.as_deref(), Some("http://e"));

    let partial = r#"{"region":"us-east-1","queue_url":"q"}"#;
    let d: SqsConfig = serde_json::from_str(partial).expect("deserialize");
    assert!(d.endpoint.is_none());
    assert_eq!(d.max_messages, 10);
    assert_eq!(d.visibility_timeout_secs, 30);
    assert_eq!(d.wait_time_secs, 20);
}

#[test]
fn message_holds_body_and_metadata() {
    let m = SqsMessage {
        body: b"payload".to_vec(),
        receipt_handle: "rh-123".to_string(),
        queue_hash: 5,
        receive_count: 2,
    };
    let cloned = m.clone();
    assert_eq!(cloned.body, b"payload".to_vec());
    assert_eq!(cloned.receipt_handle, "rh-123");
    assert_eq!(cloned.queue_hash, 5);
    assert_eq!(cloned.receive_count, 2);
}

#[test]
fn fault_variants_carry_expected_fields() {
    let f = SqsFault::SendFailed { queue_hash: 9, error_code: 3 };
    match f {
        SqsFault::SendFailed { queue_hash, error_code } => {
            assert_eq!(queue_hash, 9);
            assert_eq!(error_code, 3);
        }
        _ => panic!("unexpected variant"),
    }

    let _ = SqsFault::InvalidMessage;
}
