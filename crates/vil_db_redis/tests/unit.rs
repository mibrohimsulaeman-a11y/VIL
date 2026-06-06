//! Pure unit tests for `vil_db_redis` — no live Redis required.
//! Run: `cargo test -p vil_db_redis`

use vil_db_redis::RedisConfig;

#[test]
fn default_config_values() {
    let cfg = RedisConfig::default();
    assert_eq!(cfg.url, "redis://127.0.0.1:6379");
    assert_eq!(cfg.max_connections, 20);
    assert_eq!(cfg.database, 0);
    assert!(cfg.password.is_none());
    assert!(cfg.services.is_empty());
}

#[test]
fn new_sets_url_keeps_defaults() {
    let cfg = RedisConfig::new("redis://h:6379");
    assert_eq!(cfg.url, "redis://h:6379");
    assert_eq!(cfg.max_connections, 20);
    assert_eq!(cfg.database, 0);
}

#[test]
fn serde_applies_defaults_for_missing_fields() {
    let cfg: RedisConfig =
        serde_json::from_str(r#"{"url":"redis://x:6379"}"#).expect("deserialize");
    assert_eq!(cfg.url, "redis://x:6379");
    assert_eq!(cfg.max_connections, 20);
    assert_eq!(cfg.database, 0);
    assert!(cfg.services.is_empty());
}
