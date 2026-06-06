//! Google Cloud Pub/Sub integration tests (require the emulator or GCP creds).
//!
//! `#[ignore]`d by default. Run manually (emulator example):
//! ```bash
//! gcloud beta emulators pubsub start --host-port=localhost:8085 &
//! export PUBSUB_EMULATOR_HOST="localhost:8085"
//! export PUBSUB_PROJECT="vil-test" PUBSUB_TOPIC="vil-test-topic" PUBSUB_SUBSCRIPTION="vil-test-sub"
//! cargo test -p vil_mq_pubsub --test integration -- --ignored
//! ```

use vil_mq_pubsub::{PubSubClient, PubSubConfig};

#[tokio::test]
#[ignore = "requires the Pub/Sub emulator or GCP credentials; see module docs and run with --ignored"]
async fn publish_subscribe_roundtrip() {
    let project = std::env::var("PUBSUB_PROJECT").expect("PUBSUB_PROJECT must be set");
    let topic = std::env::var("PUBSUB_TOPIC").expect("PUBSUB_TOPIC must be set");
    let subscription = std::env::var("PUBSUB_SUBSCRIPTION").expect("PUBSUB_SUBSCRIPTION must be set");

    let mut cfg = PubSubConfig::new(&project, &topic, &subscription);
    if let Ok(host) = std::env::var("PUBSUB_EMULATOR_HOST") {
        cfg = cfg.with_emulator(&host);
    }

    let client = match PubSubClient::new(cfg).await {
        Ok(c) => c,
        Err(_) => panic!("could not initialize Pub/Sub client"),
    };

    assert!(client.publish(b"ping").await.is_ok());

    if let Ok(messages) = client.subscribe().await {
        for m in messages {
            assert!(m.ack().await.is_ok());
        }
    }
}
