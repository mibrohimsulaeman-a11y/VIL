//! Integration tests for `vil_db_mongo` — require a live MongoDB.
//!
//! `#[ignore]` by default. To run:
//! ```bash
//! docker run -d --name mongo -p 27017:27017 mongo:7
//! VIL_MONGO_URI=mongodb://localhost:27017 VIL_MONGO_DB=vil_test \
//!   cargo test -p vil_db_mongo --test integration -- --ignored
//! ```

use vil_db_mongo::{MongoClient, MongoConfig};

#[tokio::test]
#[ignore = "requires a live MongoDB instance"]
async fn connects_to_server() {
    let uri = std::env::var("VIL_MONGO_URI").unwrap_or_else(|_| "mongodb://localhost:27017".to_string());
    let db = std::env::var("VIL_MONGO_DB").unwrap_or_else(|_| "vil_test".to_string());
    let client = MongoClient::new(MongoConfig::new(uri, db)).await;
    assert!(client.is_ok(), "expected a successful connection");
}
