//! Pure unit tests for `vil_db_timeseries` — no live backend required.
//! Run: `cargo test -p vil_db_timeseries`

use vil_db_timeseries::{TimeseriesConfig, TimeseriesFault};

#[test]
fn config_new_defaults() {
    let cfg = TimeseriesConfig::new("http://localhost:8086", "myorg", "my-token", "metrics");
    assert_eq!(cfg.host, "http://localhost:8086");
    assert_eq!(cfg.org, "myorg");
    assert_eq!(cfg.token, "my-token");
    assert_eq!(cfg.bucket, "metrics");
    assert_eq!(cfg.pool_id, 0);
}

#[test]
fn config_serde_round_trip() {
    let cfg = TimeseriesConfig::new("http://h:8086", "o", "t", "b");
    let json = serde_json::to_string(&cfg).expect("serialize");
    let back: TimeseriesConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.host, cfg.host);
    assert_eq!(back.org, cfg.org);
    assert_eq!(back.token, cfg.token);
    assert_eq!(back.bucket, cfg.bucket);
}

#[test]
fn fault_metadata_is_consistent() {
    let conn = TimeseriesFault::ConnectionFailed { host_hash: 1, reason_code: 2 };
    assert_eq!(conn.kind(), "ConnectionFailed");
    assert!(conn.is_retryable());

    let wr = TimeseriesFault::WriteFailed { bucket_hash: 3, reason_code: 4 };
    assert_eq!(wr.kind(), "WriteFailed");
    assert!(!wr.is_retryable());

    let fne = TimeseriesFault::FeatureNotEnabled { feature_hash: 5 };
    assert_eq!(fne.kind(), "FeatureNotEnabled");
    assert!(!fne.is_retryable());
}
