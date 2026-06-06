//! Pure unit tests for `vil_db_cassandra` — no live cluster required.
//! Run: `cargo test -p vil_db_cassandra`

use vil_db_cassandra::{CassandraConfig, CassandraFault};

#[test]
fn config_new_sets_single_contact_point_and_defaults() {
    let cfg = CassandraConfig::new("127.0.0.1:9042", "myapp");
    assert_eq!(cfg.contact_points, vec!["127.0.0.1:9042".to_string()]);
    assert_eq!(cfg.keyspace, "myapp");
    assert_eq!(cfg.pool_id, 0);
}

#[test]
fn config_struct_literal_supports_multiple_contact_points() {
    let cfg = CassandraConfig {
        contact_points: vec!["a:9042".into(), "b:9042".into()],
        keyspace: "k".into(),
        pool_id: 7,
    };
    assert_eq!(cfg.contact_points.len(), 2);
    assert_eq!(cfg.pool_id, 7);
}

#[test]
fn config_serde_round_trip() {
    let cfg = CassandraConfig::new("10.0.0.1:9042", "ks");
    let json = serde_json::to_string(&cfg).expect("serialize");
    let back: CassandraConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.contact_points, cfg.contact_points);
    assert_eq!(back.keyspace, cfg.keyspace);
    assert_eq!(back.pool_id, cfg.pool_id);
}

#[test]
fn fault_metadata_is_consistent() {
    let conn = CassandraFault::ConnectionFailed { uri_hash: 1, reason_code: 2 };
    assert_eq!(conn.kind(), "ConnectionFailed");
    assert!(conn.is_retryable());
    assert!(conn.error_code() >= 1);

    let q = CassandraFault::QueryFailed { query_hash: 9, reason_code: 7 };
    assert_eq!(q.kind(), "QueryFailed");
    assert!(!q.is_retryable());
    assert_ne!(conn.error_code(), q.error_code());
}
