//! Pure unit tests for `vil_db_sqlx` — no live database required.
//! Run: `cargo test -p vil_db_sqlx`

use vil_db_sqlx::SqlxConfig;

#[test]
fn default_config_values() {
    let cfg = SqlxConfig::default();
    assert_eq!(cfg.driver, "sqlite");
    assert_eq!(cfg.max_connections, 10);
    assert_eq!(cfg.min_connections, 1);
    assert_eq!(cfg.connect_timeout_secs, 5);
    assert_eq!(cfg.idle_timeout_secs, 300);
    assert_eq!(cfg.ssl_mode, "prefer");
    assert!(cfg.services.is_empty());
}

#[test]
fn driver_constructors_and_builders() {
    let pg = SqlxConfig::postgres("postgres://localhost/db")
        .max_connections(25)
        .min_connections(3)
        .timeout(9);
    assert_eq!(pg.driver, "postgres");
    assert_eq!(pg.url, "postgres://localhost/db");
    assert_eq!(pg.max_connections, 25);
    assert_eq!(pg.min_connections, 3);
    assert_eq!(pg.connect_timeout_secs, 9);

    assert_eq!(SqlxConfig::mysql("mysql://x/db").driver, "mysql");
    assert_eq!(SqlxConfig::sqlite("sqlite::memory:").driver, "sqlite");
}

#[test]
fn is_for_service_matches_rules() {
    let all = SqlxConfig::sqlite("sqlite::memory:");
    assert!(all.is_for_service("anything"));

    let mut wildcard = SqlxConfig::sqlite("sqlite::memory:");
    wildcard.services = vec!["*".to_string()];
    assert!(wildcard.is_for_service("orders"));

    let mut scoped = SqlxConfig::sqlite("sqlite::memory:");
    scoped.services = vec!["orders".to_string()];
    assert!(scoped.is_for_service("orders"));
    assert!(!scoped.is_for_service("billing"));
}

#[test]
fn serde_applies_defaults_for_missing_fields() {
    let cfg: SqlxConfig =
        serde_json::from_str(r#"{"url":"sqlite::memory:"}"#).expect("deserialize");
    assert_eq!(cfg.driver, "sqlite");
    assert_eq!(cfg.ssl_mode, "prefer");
    assert_eq!(cfg.max_connections, 10);
}
