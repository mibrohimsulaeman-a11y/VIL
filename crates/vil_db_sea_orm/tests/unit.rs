//! Pure unit tests for `vil_db_sea_orm` — no live database required.
//! Run: `cargo test -p vil_db_sea_orm`

use vil_db_sea_orm::SeaOrmConfig;

#[test]
fn default_config_values() {
    let cfg = SeaOrmConfig::default();
    assert_eq!(cfg.driver, "sqlite");
    assert_eq!(cfg.url, "");
    assert_eq!(cfg.max_connections, 10);
    assert_eq!(cfg.min_connections, 1);
    assert_eq!(cfg.connect_timeout_secs, 5);
    assert_eq!(cfg.idle_timeout_secs, 300);
    assert!(cfg.schema.is_none());
    assert!(cfg.services.is_empty());
}

#[test]
fn driver_constructors_and_builder() {
    let pg = SeaOrmConfig::postgres("postgres://localhost/db");
    assert_eq!(pg.driver, "postgres");
    assert_eq!(pg.url, "postgres://localhost/db");

    let my = SeaOrmConfig::mysql("mysql://localhost/db");
    assert_eq!(my.driver, "mysql");

    let lite = SeaOrmConfig::sqlite("sqlite::memory:").max_connections(42);
    assert_eq!(lite.driver, "sqlite");
    assert_eq!(lite.max_connections, 42);
}

#[test]
fn serde_applies_defaults_for_missing_fields() {
    let cfg: SeaOrmConfig =
        serde_json::from_str(r#"{"url":"sqlite::memory:"}"#).expect("deserialize");
    assert_eq!(cfg.driver, "sqlite");
    assert_eq!(cfg.max_connections, 10);
    assert_eq!(cfg.min_connections, 1);
}
