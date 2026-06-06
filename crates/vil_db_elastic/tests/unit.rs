//! Pure unit tests for `vil_db_elastic` — no live node required.
//! Run: `cargo test -p vil_db_elastic`

use vil_db_elastic::{ElasticConfig, ElasticFault};

#[test]
fn default_config_values() {
    let cfg = ElasticConfig::default();
    assert_eq!(cfg.url, "http://localhost:9200");
    assert!(cfg.username.is_none());
    assert!(cfg.password.is_none());
}

#[test]
fn config_serde_round_trip() {
    let cfg = ElasticConfig {
        url: "http://es:9200".into(),
        username: Some("elastic".into()),
        password: Some("changeme".into()),
    };
    let json = serde_json::to_string(&cfg).expect("serialize");
    let back: ElasticConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.url, cfg.url);
    assert_eq!(back.username, cfg.username);
    assert_eq!(back.password, cfg.password);
}

#[test]
fn fault_metadata_is_consistent() {
    let conn = ElasticFault::ConnectionFailed { url_hash: 1, reason_code: 2 };
    assert_eq!(conn.kind(), "ConnectionFailed");
    assert!(conn.is_retryable());

    let nf = ElasticFault::NotFound { index_hash: 1, id_hash: 2 };
    assert_eq!(nf.kind(), "NotFound");
    assert!(!nf.is_retryable());

    let to = ElasticFault::Timeout { operation_hash: 1, elapsed_ms: 5 };
    assert!(to.is_retryable());
    assert_ne!(conn.error_code(), nf.error_code());
}
