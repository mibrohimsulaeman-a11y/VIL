//! AWS SQS integration tests (require AWS SQS or a LocalStack endpoint).
//!
//! `#[ignore]`d by default. Run manually (LocalStack example):
//! ```bash
//! docker run -d --rm -p 4566:4566 localstack/localstack
//! export AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test AWS_REGION=us-east-1
//! export SQS_ENDPOINT="http://localhost:4566"
//! export SQS_QUEUE_URL="http://localhost:4566/000000000000/vil-test-q"
//! cargo test -p vil_mq_sqs --test integration -- --ignored
//! ```

use vil_mq_sqs::{SqsClient, SqsConfig};

#[tokio::test]
#[ignore = "requires AWS SQS or LocalStack; see module docs and run with --ignored"]
async fn send_receive_delete_roundtrip() {
    let region = std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());
    let queue_url = std::env::var("SQS_QUEUE_URL").expect("SQS_QUEUE_URL must be set");
    let mut cfg = SqsConfig::new(&region, &queue_url);
    if let Ok(ep) = std::env::var("SQS_ENDPOINT") {
        cfg = cfg.with_endpoint(&ep);
    }

    let client = match SqsClient::from_config(cfg).await {
        Ok(c) => c,
        Err(_) => panic!("could not build SQS client"),
    };

    assert!(client.send_message(b"ping").await.is_ok());

    if let Ok(messages) = client.receive_messages().await {
        for m in messages {
            assert!(client.delete_message(&m.receipt_handle).await.is_ok());
        }
    }
}
