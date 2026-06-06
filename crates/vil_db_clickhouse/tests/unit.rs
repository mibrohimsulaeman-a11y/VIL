//! Pure unit tests for `vil_db_clickhouse` — no live server required.
//! Run: `cargo test -p vil_db_clickhouse`

use vil_db_clickhouse::{ChFault, ClickHouseConfig};

#[test]
fn default_config_values() {
    let cfg = ClickHouseConfig::default();
    assert_eq!(cfg.url, "http://localhost:8123");
    assert_eq!(cfg.database, "default");
    assert!(cfg.username.is_none());
    assert!(cfg.password.is_none());
}

#[test]
fn struct_literal_with_auth() {
    let cfg = ClickHouseConfig {
        url: "http://ch:8123".into(),
        database: "analytics".into(),
        username: Some("default".into()),
        password: None,
    };
    assert_eq!(cfg.database, "analytics");
    assert_eq!(cfg.username.as_deref(), Some("default"));
}

#[test]
fn fault_metadata_is_consistent() {
    let to = ChFault::Timeout { operation_hash: 1, elapsed_ms: 10 };
    assert_eq!(to.kind(), "Timeout");
    assert!(to.is_retryable());

    let qf = ChFault::QueryFailed { query_hash: 2, reason_code: 3 };
    assert_eq!(qf.kind(), "QueryFailed");
    assert!(!qf.is_retryable());
    assert_ne!(to.error_code(), qf.error_code());
}
