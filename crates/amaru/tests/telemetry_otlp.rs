// Copyright 2026 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! A single-process OTLP contract test. The custom harness configures the environment before starting threads.
//! Expected exports were captured before the telemetry refactor at bd286c86357333144b0c565ad68f95a539c40d37.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "custom test harness; malformed output or setup errors must fail the test"
)]

#[expect(dead_code, reason = "the local-output tests use the shared normalization helpers")]
#[path = "../src/observability/telemetry_fixture.rs"]
mod telemetry_fixture;

use std::{collections::BTreeMap, env, sync::mpsc};

use amaru::observability::{ObservabilityHints, try_setup_observability};
use amaru_metrics::{LedgerMetrics, MetricRecorder};
use amaru_node::telemetry::OtelSignals;
use opentelemetry_proto::tonic::collector::{
    logs::v1::{
        ExportLogsServiceRequest, ExportLogsServiceResponse,
        logs_service_server::{LogsService, LogsServiceServer},
    },
    metrics::v1::{
        ExportMetricsServiceRequest, ExportMetricsServiceResponse,
        metrics_service_server::{MetricsService, MetricsServiceServer},
    },
    trace::v1::{
        ExportTraceServiceRequest, ExportTraceServiceResponse,
        trace_service_server::{TraceService, TraceServiceServer},
    },
};
use serde_json::{Value, json};
use telemetry_fixture::{FILTER, run_ledger_fixture};
use tokio::{net::TcpListener, runtime::Builder};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{Request, Response, Status, transport::Server};

struct NoListenAddress;

impl ObservabilityHints for NoListenAddress {
    fn listen_address(&self) -> Option<&str> {
        None
    }
}

#[derive(Clone)]
struct Collector(mpsc::Sender<(&'static str, Value)>);

#[tonic::async_trait]
impl TraceService for Collector {
    async fn export(
        &self,
        request: Request<ExportTraceServiceRequest>,
    ) -> Result<Response<ExportTraceServiceResponse>, Status> {
        self.0.send(("traces", serde_json::to_value(request.into_inner()).unwrap())).unwrap();
        Ok(Response::new(ExportTraceServiceResponse::default()))
    }
}

#[tonic::async_trait]
impl LogsService for Collector {
    async fn export(
        &self,
        request: Request<ExportLogsServiceRequest>,
    ) -> Result<Response<ExportLogsServiceResponse>, Status> {
        self.0.send(("logs", serde_json::to_value(request.into_inner()).unwrap())).unwrap();
        Ok(Response::new(ExportLogsServiceResponse::default()))
    }
}

#[tonic::async_trait]
impl MetricsService for Collector {
    async fn export(
        &self,
        request: Request<ExportMetricsServiceRequest>,
    ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
        self.0.send(("metrics", serde_json::to_value(request.into_inner()).unwrap())).unwrap();
        Ok(Response::new(ExportMetricsServiceResponse::default()))
    }
}

fn main() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let inherited: Vec<_> =
        env::vars_os().map(|(key, _)| key).filter(|key| key.to_string_lossy().starts_with("OTEL_")).collect();
    // SAFETY: harness=false runs this main directly; no test harness or runtime threads have started.
    unsafe {
        for key in inherited {
            env::remove_var(key);
        }
        for (key, value) in [
            ("AMARU_LOG", "off"),
            ("AMARU_TRACE", FILTER),
            ("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint.as_str()),
            ("OTEL_SERVICE_NAME", "amaru-telemetry-test"),
            ("OTEL_SERVICE_INSTANCE_ID", "ledger-fixture"),
            ("OTEL_RESOURCE_ATTRIBUTES", "test.fixture=ledger"),
            ("OTEL_BSP_SCHEDULE_DELAY", "60000"),
            ("OTEL_BLRP_SCHEDULE_DELAY", "60000"),
            ("OTEL_METRIC_EXPORT_INTERVAL", "60000"),
        ] {
            env::set_var(key, value);
        }
    }

    let runtime = Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let _entered = runtime.enter();
    let listener = TcpListener::from_std(listener).unwrap();
    let (sender, receiver) = mpsc::channel();
    let collector = Collector(sender);
    let server = runtime.spawn(
        Server::builder()
            .add_service(TraceServiceServer::new(collector.clone()))
            .add_service(LogsServiceServer::new(collector.clone()))
            .add_service(MetricsServiceServer::new(collector))
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );
    let handle =
        try_setup_observability(true, OtelSignals::default(), false, None, false, &NoListenAddress, &[]).unwrap();
    run_ledger_fixture();
    LedgerMetrics {
        block_height: 42,
        slot: 86400,
        epoch: 200,
        slot_in_epoch: 123,
        density: 0.75,
        current_kes_period: 7,
        remaining_kes_periods: 8,
        block_header_hash: "ab".repeat(32),
        parent_block_header_hash: "cd".repeat(32),
        issuer_verification_key_hash: "ef".repeat(28),
        ..LedgerMetrics::default()
    }
    .record_to_meter(&handle.meter);
    assert!(
        matches!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "records must remain buffered until shutdown"
    );
    handle.teardown.shutdown().unwrap();
    server.abort();

    let mut actual = json!({"traces": [], "logs": [], "metrics": []});
    for (signal, request) in receiver.try_iter() {
        actual[signal].as_array_mut().unwrap().push(request);
    }
    normalize(&mut actual, &mut BTreeMap::new());
    let expected: Value =
        serde_json::from_str(include_str!("../src/observability/fixtures/telemetry/otlp.json")).unwrap();
    assert_eq!(actual, expected, "OTLP exports changed");
}

/// Keep record contents and ID relationships, removing only run-specific values and attribute ordering.
fn normalize(value: &mut Value, ids: &mut BTreeMap<String, usize>) {
    match value {
        Value::Object(fields) => {
            if let (Some(start), Some(end)) = (fields.get("startTimeUnixNano"), fields.get("endTimeUnixNano")) {
                assert!(unsigned(end) >= unsigned(start));
            }
            if matches!(
                fields.get("key").and_then(Value::as_str),
                Some("busy_ns" | "idle_ns" | "thread.id" | "thread.name")
            ) {
                let attribute = fields.get_mut("value").unwrap().as_object_mut().unwrap();
                if let Some(integer) = attribute.get("intValue") {
                    unsigned(integer);
                }
                for scalar in attribute.values_mut() {
                    assert!(scalar.is_string());
                    *scalar = json!("<variable>");
                }
            }
            for (key, value) in fields {
                match key.as_str() {
                    "traceId" | "spanId" | "parentSpanId" if value != "" => {
                        let id = value.as_str().unwrap();
                        let bytes = hex::decode(id).unwrap();
                        assert_eq!(bytes.len(), if key == "traceId" { 16 } else { 8 });
                        assert!(bytes.iter().any(|byte| *byte != 0));
                        let next = ids.len() + 1;
                        *value = json!(ids.entry(id.to_string()).or_insert(next));
                    }
                    key if key.ends_with("UnixNano") && unsigned(value) != 0 => *value = json!("<timestamp>"),
                    "attributes" | "metrics" => value.as_array_mut().unwrap().sort_by_key(Value::to_string),
                    _ => {}
                }
                normalize(value, ids);
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize(item, ids);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn unsigned(value: &Value) -> u64 {
    value.as_str().unwrap().parse().unwrap()
}
