//! Integration tests for `vil_db_clickhouse` — require a live ClickHouse.
//!
//! `#[ignore]` by default. To run:
//! ```bash
//! docker run -d --name clickhouse -p 8123:8123 clickhouse/clickhouse-server
//! VIL_CLICKHOUSE_URL=http://localhost:8123 \
//!   cargo test -p vil_db_clickhouse --test integration -- --ignored
//! ```

use vil_db_clickhouse::{ChClient, ClickHouseConfig};

#[tokio::test]
#[ignore = "requires a live ClickHouse server"]
async fn connects_and_executes() {
    let mut cfg = ClickHouseConfig::default();
    if let Ok(url) = std::env::var("VIL_CLICKHOUSE_URL") {
        cfg.url = url;
    }
    let client = ChClient::new(cfg);
    let res = client.execute("SELECT 1").await;
    assert!(res.is_ok(), "expected query to succeed against live ClickHouse");
}
