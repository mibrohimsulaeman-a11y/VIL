//! Pure unit tests for `vil_db_neo4j` — no live database required.
//! Run: `cargo test -p vil_db_neo4j`

use vil_db_neo4j::{Neo4jConfig, Neo4jFault};

#[test]
fn config_new_defaults() {
    let cfg = Neo4jConfig::new("bolt://localhost:7687", "neo4j", "password");
    assert_eq!(cfg.uri, "bolt://localhost:7687");
    assert_eq!(cfg.user, "neo4j");
    assert_eq!(cfg.password, "password");
    assert_eq!(cfg.pool_id, 0);
}

#[test]
fn config_serde_round_trip() {
    let cfg = Neo4jConfig::new("bolt://h:7687", "u", "p");
    let json = serde_json::to_string(&cfg).expect("serialize");
    let back: Neo4jConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.uri, cfg.uri);
    assert_eq!(back.user, cfg.user);
    assert_eq!(back.password, cfg.password);
    assert_eq!(back.pool_id, cfg.pool_id);
}

#[test]
fn fault_metadata_is_consistent() {
    let conn = Neo4jFault::ConnectionFailed { uri_hash: 1, reason_code: 2 };
    assert_eq!(conn.kind(), "ConnectionFailed");
    assert!(conn.is_retryable());

    let ex = Neo4jFault::ExecuteFailed { query_hash: 3, reason_code: 4 };
    assert_eq!(ex.kind(), "ExecuteFailed");
    assert!(!ex.is_retryable());
    assert_ne!(conn.error_code(), ex.error_code());
}
