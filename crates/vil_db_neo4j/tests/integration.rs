//! Integration tests for `vil_db_neo4j` — require a live Neo4j.
//!
//! `#[ignore]` by default. To run:
//! ```bash
//! docker run -d --name neo4j -p 7687:7687 -e NEO4J_AUTH=neo4j/password neo4j:5
//! VIL_NEO4J_URI=bolt://localhost:7687 VIL_NEO4J_USER=neo4j VIL_NEO4J_PASSWORD=password \
//!   cargo test -p vil_db_neo4j --test integration -- --ignored
//! ```

use vil_db_neo4j::{Neo4jClient, Neo4jConfig};

#[tokio::test]
#[ignore = "requires a live Neo4j instance"]
async fn connects_to_server() {
    let uri = std::env::var("VIL_NEO4J_URI").unwrap_or_else(|_| "bolt://localhost:7687".to_string());
    let user = std::env::var("VIL_NEO4J_USER").unwrap_or_else(|_| "neo4j".to_string());
    let password = std::env::var("VIL_NEO4J_PASSWORD").unwrap_or_else(|_| "password".to_string());
    let client = Neo4jClient::new(Neo4jConfig::new(uri, user, password)).await;
    assert!(client.is_ok(), "expected a successful connection");
}
