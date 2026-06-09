#!/usr/bin/env bash
# Opt-in external infrastructure compatibility matrix skeleton.
# Deterministic H5 smokes remain in scripts/vflow-compat-live-smoke.sh.
set -euo pipefail

if [ "${VIL_EXTERNAL_LIVE:-0}" != "1" ]; then
  cat <<'MSG'
RESULT: SKIP — external live infrastructure suite is opt-in.
Set VIL_EXTERNAL_LIVE=1 to run the external matrix skeleton.
No brokers, databases, cloud stores, or protocol servers were contacted.
MSG
  exit 0
fi

printf '%s\n' "==> VIL external live infra matrix (opt-in)"
printf '%-28s %-10s %s\n' "Category" "Status" "Probe / next action"
printf '%-28s %-10s %s\n' "NATS / JetStream / KV" "SKIP" "Set VIL_NATS_URL and provide ephemeral stream/bucket fixtures"
printf '%-28s %-10s %s\n' "Kafka / MQTT / RabbitMQ" "SKIP" "Set VIL_KAFKA_BROKERS / VIL_MQTT_URL / VIL_RABBITMQ_URL"
printf '%-28s %-10s %s\n' "Postgres / Cassandra / CH / Mongo / Redis" "SKIP" "Set per-service URLs and fixture schemas"
printf '%-28s %-10s %s\n' "S3 / GCS / Azure" "SKIP" "Set test bucket/container env and credentials"
printf '%-28s %-10s %s\n' "gRPC typed-body server" "SKIP" "Start fixture server and set VIL_GRPC_TEST_ADDR"
printf '%-28s %-10s %s\n' "Codec / protocol adapters" "SKIP" "Wire self-contained adapter fixtures"
printf '%s\n' "RESULT: SKIP — VIL_EXTERNAL_LIVE=1 acknowledged, but no external fixture env is configured."
exit 0
