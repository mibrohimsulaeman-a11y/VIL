//! Pure unit tests for `vil_mq_rabbitmq` (no live broker required).
//!
//! Covers config construction, builder defaults/overrides, serde round-trip
//! and serde defaults, message construction, and fault variant shapes.

use bytes::Bytes;
use vil_mq_rabbitmq::{RabbitConfig, RabbitFault, RabbitMessage};

#[test]
fn config_new_applies_defaults() {
    let c = RabbitConfig::new("amqp://guest:guest@localhost:5672/%2F", "ex", "q");
    assert_eq!(c.uri, "amqp://guest:guest@localhost:5672/%2F");
    assert_eq!(c.exchange, "ex");
    assert_eq!(c.queue, "q");
    assert_eq!(c.consumer_tag, "vil-consumer");
    assert_eq!(c.connection_timeout_ms, 5_000);
    assert_eq!(c.prefetch_count, 10);
}

#[test]
fn config_builders_override_defaults() {
    let c = RabbitConfig::new("amqp://h", "ex", "q")
        .with_prefetch(64)
        .with_consumer_tag("worker-1");
    assert_eq!(c.prefetch_count, 64);
    assert_eq!(c.consumer_tag, "worker-1");
}

#[test]
fn config_serde_roundtrip_preserves_fields() {
    let c = RabbitConfig::new("amqp://h", "ex", "q").with_prefetch(7);
    let json = serde_json::to_string(&c).expect("serialize");
    let back: RabbitConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.uri, "amqp://h");
    assert_eq!(back.exchange, "ex");
    assert_eq!(back.queue, "q");
    assert_eq!(back.prefetch_count, 7);
}

#[test]
fn config_serde_uses_defaults_for_missing_fields() {
    let json = r#"{"uri":"amqp://h","exchange":"ex","queue":"q"}"#;
    let c: RabbitConfig = serde_json::from_str(json).expect("deserialize");
    assert_eq!(c.consumer_tag, "vil-consumer");
    assert_eq!(c.connection_timeout_ms, 5_000);
    assert_eq!(c.prefetch_count, 10);
}

#[test]
fn message_holds_payload_and_metadata() {
    let m = RabbitMessage {
        payload: Bytes::from_static(b"hello"),
        delivery_tag: 42,
        routing_key_hash: 7,
        exchange_hash: 9,
    };
    assert_eq!(&m.payload[..], &b"hello"[..]);
    assert_eq!(m.payload.len(), 5);
    assert_eq!(m.delivery_tag, 42);
    assert_eq!(m.routing_key_hash, 7);
    assert_eq!(m.exchange_hash, 9);
}

#[test]
fn fault_variants_carry_expected_fields() {
    let f = RabbitFault::ConnectionFailed { uri_hash: 1, elapsed_ms: 2 };
    match f {
        RabbitFault::ConnectionFailed { uri_hash, elapsed_ms } => {
            assert_eq!(uri_hash, 1);
            assert_eq!(elapsed_ms, 2);
        }
        _ => panic!("unexpected variant"),
    }

    let p = RabbitFault::PublishFailed { exchange_hash: 3, routing_key_hash: 4 };
    match p {
        RabbitFault::PublishFailed { exchange_hash, routing_key_hash } => {
            assert_eq!(exchange_hash, 3);
            assert_eq!(routing_key_hash, 4);
        }
        _ => panic!("unexpected variant"),
    }

    let _ = RabbitFault::NotConnected;
}
