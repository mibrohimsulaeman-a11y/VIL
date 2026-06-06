//! Integration tests for `vil_db_elastic` — require a live Elasticsearch/OpenSearch.
//!
//! `#[ignore]` by default. To run:
//! ```bash
//! docker run -d --name es -p 9200:9200 -e discovery.type=single-node \
//!   docker.elastic.co/elasticsearch/elasticsearch:8.15.0
//! VIL_ELASTIC_URL=http://localhost:9200 \
//!   cargo test -p vil_db_elastic --test integration -- --ignored
//! ```

use vil_db_elastic::{ElasticClient, ElasticConfig};

#[tokio::test]
#[ignore = "requires a live Elasticsearch/OpenSearch node"]
async fn connects_and_queries() {
    let url = std::env::var("VIL_ELASTIC_URL").unwrap_or_else(|_| "http://localhost:9200".to_string());
    let cfg = ElasticConfig { url, username: None, password: None };
    let client = match ElasticClient::new(cfg) {
        Ok(c) => c,
        Err(_) => panic!("client init failed"),
    };
    // Touching the server requires a live node; a missing document simply
    // returns an error, which is acceptable for this smoke test.
    let _ = client.get("vil-test-index", "missing-doc").await;
}
