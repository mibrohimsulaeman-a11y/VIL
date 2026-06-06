//! Pure unit tests for `vil_db_dynamodb` — no AWS endpoint required.
//! Run: `cargo test -p vil_db_dynamodb`

use vil_db_dynamodb::{DynamoConfig, DynamoFault};

#[test]
fn config_new_defaults() {
    let cfg = DynamoConfig::new("us-east-1");
    assert_eq!(cfg.region, "us-east-1");
    assert!(cfg.endpoint_url.is_none());
    assert_eq!(cfg.pool_id, 0);
}

#[test]
fn with_endpoint_overrides_url() {
    let cfg = DynamoConfig::new("us-west-2").with_endpoint("http://localhost:4566");
    assert_eq!(cfg.region, "us-west-2");
    assert_eq!(cfg.endpoint_url.as_deref(), Some("http://localhost:4566"));
}

#[test]
fn config_serde_round_trip() {
    let cfg = DynamoConfig::new("eu-west-1").with_endpoint("http://ls:4566");
    let json = serde_json::to_string(&cfg).expect("serialize");
    let back: DynamoConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.region, cfg.region);
    assert_eq!(back.endpoint_url, cfg.endpoint_url);
    assert_eq!(back.pool_id, cfg.pool_id);
}

#[test]
fn fault_metadata_is_consistent() {
    let get = DynamoFault::GetFailed { table_hash: 1, reason_code: 2 };
    assert_eq!(get.kind(), "GetFailed");
    assert!(!get.is_retryable());

    let cfg_f = DynamoFault::ConfigFailed { reason_code: 9 };
    assert_eq!(cfg_f.kind(), "ConfigFailed");
    assert_ne!(get.error_code(), cfg_f.error_code());
}
