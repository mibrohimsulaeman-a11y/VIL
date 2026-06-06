//! Integration tests for `vil_db_dynamodb` — require a DynamoDB-compatible endpoint.
//!
//! `#[ignore]` by default. To run against LocalStack:
//! ```bash
//! docker run -d --name localstack -p 4566:4566 localstack/localstack
//! VIL_DYNAMO_REGION=us-east-1 VIL_DYNAMO_ENDPOINT=http://localhost:4566 \
//!   cargo test -p vil_db_dynamodb --test integration -- --ignored
//! ```

use vil_db_dynamodb::{DynamoClient, DynamoConfig};

#[tokio::test]
#[ignore = "requires a live DynamoDB/LocalStack endpoint"]
async fn builds_client() {
    let region = std::env::var("VIL_DYNAMO_REGION").unwrap_or_else(|_| "us-east-1".to_string());
    let endpoint =
        std::env::var("VIL_DYNAMO_ENDPOINT").unwrap_or_else(|_| "http://localhost:4566".to_string());
    let cfg = DynamoConfig::new(region).with_endpoint(endpoint);
    let client = DynamoClient::new(cfg).await;
    assert!(client.is_ok(), "expected client construction to succeed");
}
