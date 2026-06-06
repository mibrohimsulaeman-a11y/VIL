//! Integration tests for `vil_db_cassandra` — require a live Cassandra/ScyllaDB.
//!
//! These tests are `#[ignore]` by default. To run:
//! ```bash
//! docker run -d --name scylla -p 9042:9042 scylladb/scylla
//! VIL_CASSANDRA_ADDR=127.0.0.1:9042 VIL_CASSANDRA_KEYSPACE=system \
//!   cargo test -p vil_db_cassandra --test integration -- --ignored
//! ```

use vil_db_cassandra::{CassandraClient, CassandraConfig};

fn addr() -> String {
    std::env::var("VIL_CASSANDRA_ADDR").unwrap_or_else(|_| "127.0.0.1:9042".to_string())
}
fn keyspace() -> String {
    std::env::var("VIL_CASSANDRA_KEYSPACE").unwrap_or_else(|_| "system".to_string())
}

#[tokio::test]
#[ignore = "requires a live Cassandra/ScyllaDB node"]
async fn connects_to_cluster() {
    let cfg = CassandraConfig::new(addr(), keyspace());
    let client = CassandraClient::new(cfg).await;
    assert!(client.is_ok(), "expected a successful connection");
}
