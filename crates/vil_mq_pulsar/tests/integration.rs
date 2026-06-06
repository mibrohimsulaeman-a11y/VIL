//! Broker integration tests for `vil_mq_pulsar` (require a live Pulsar broker).
//!
//! `#[ignore]`d by default. Run manually:
//! ```bash
//! docker run -d --rm -p 6650:6650 -p 8080:8080 apachepulsar/pulsar:3.2.0 bin/pulsar standalone
//! export PULSAR_URL="pulsar://localhost:6650"
//! cargo test -p vil_mq_pulsar --test integration -- --ignored
//! ```

use vil_mq_pulsar::{PulsarClient, PulsarConfig};

#[tokio::test]
#[ignore = "requires a live Pulsar broker; see module docs and run with --ignored"]
async fn connect_smoke() {
    let url = std::env::var("PULSAR_URL").unwrap_or_else(|_| "pulsar://localhost:6650".to_string());
    let cfg = PulsarConfig::new(&url, "public", "default");

    match PulsarClient::connect(cfg).await {
        Ok(_) => {}
        Err(_) => panic!("could not connect to Pulsar broker at {url}"),
    }
}
