//! Pure unit tests for `vil_db_mongo` — no live MongoDB required.
//! Run: `cargo test -p vil_db_mongo`

use vil_db_mongo::{MongoConfig, MongoFault};

#[test]
fn config_new_defaults() {
    let cfg = MongoConfig::new("mongodb://localhost:27017", "myapp");
    assert_eq!(cfg.uri, "mongodb://localhost:27017");
    assert_eq!(cfg.database, "myapp");
    assert!(cfg.min_pool.is_none());
    assert!(cfg.max_pool.is_none());
}

#[test]
fn config_serde_round_trip() {
    let mut cfg = MongoConfig::new("mongodb://h:27017", "db");
    cfg.min_pool = Some(2);
    cfg.max_pool = Some(16);
    let json = serde_json::to_string(&cfg).expect("serialize");
    let back: MongoConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.min_pool, Some(2));
    assert_eq!(back.max_pool, Some(16));
}

#[test]
fn deserialize_minimal_json_defaults_pool_to_none() {
    let back: MongoConfig =
        serde_json::from_str(r#"{"uri":"mongodb://x","database":"d"}"#).expect("deserialize");
    assert!(back.min_pool.is_none());
    assert!(back.max_pool.is_none());
}

#[test]
fn fault_metadata_is_consistent() {
    let conn = MongoFault::ConnectionFailed { uri_hash: 1, reason_code: 2 };
    assert_eq!(conn.kind(), "ConnectionFailed");
    assert!(conn.is_retryable());

    let ins = MongoFault::InsertFailed { collection_hash: 3, reason_code: 4 };
    assert_eq!(ins.kind(), "InsertFailed");
    assert!(!ins.is_retryable());
}
