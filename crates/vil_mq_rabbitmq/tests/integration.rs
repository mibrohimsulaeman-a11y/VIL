//! Broker integration tests for `vil_mq_rabbitmq`.
//!
//! These require a live RabbitMQ broker and are `#[ignore]`d by default so they
//! never run in normal CI.
//!
//! Run them manually:
//! ```bash
//! docker run -d --rm -p 5672:5672 rabbitmq:3-management
//! export RABBITMQ_URI="amqp://guest:guest@localhost:5672/%2F"
//! cargo test -p vil_mq_rabbitmq --test integration -- --ignored
//! ```

use vil_mq_rabbitmq::{RabbitClient, RabbitConfig};

#[tokio::test]
#[ignore = "requires a live RabbitMQ broker; see module docs and run with --ignored"]
async fn publish_smoke() {
    let uri = std::env::var("RABBITMQ_URI")
        .unwrap_or_else(|_| "amqp://guest:guest@localhost:5672/%2F".to_string());
    let cfg = RabbitConfig::new(&uri, "", "vil.test.q");

    let client = match RabbitClient::connect(cfg).await {
        Ok(c) => c,
        Err(_) => panic!("could not connect to RabbitMQ broker at {uri}"),
    };

    assert!(client.publish("", "vil.test.q", b"ping").await.is_ok());
    client.close().await;
}
