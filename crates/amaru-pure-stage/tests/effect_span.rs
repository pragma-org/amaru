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

//! The Tokio runtime records a debug span around each external effect.

#![expect(clippy::expect_used)]

use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
    time::Duration,
};

use amaru_pure_stage::{
    BoxFuture, EFFECT_SPAN_TARGET, ExternalEffectAPI, Resources, SendData, StageGraph, tokio::TokioBuilder,
};
use futures_util::StreamExt;
use tokio::time::timeout;
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{self, MakeWriter, format::FmtSpan},
    layer::SubscriberExt,
};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct MeasuredEffect;

impl ExternalEffectAPI for MeasuredEffect {
    type Response = u32;

    fn run(self: Box<Self>, _resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync(7)
    }
}

#[derive(Clone, Default)]
struct CaptureWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl CaptureWriter {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.buf.lock().expect("writer lock")).into_owned()
    }
}

impl Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.lock().expect("writer lock").write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CaptureWriter {
    type Writer = CaptureWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn run_once(filter: &str) -> String {
    let writer = CaptureWriter::default();
    let subscriber = tracing_subscriber::registry().with(
        fmt::layer()
            .with_writer(writer.clone())
            .with_ansi(false)
            .json()
            .with_span_events(FmtSpan::CLOSE)
            .with_filter(EnvFilter::new(filter)),
    );

    let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().expect("runtime");
    let handle = rt.handle().clone();
    tracing::subscriber::with_default(subscriber, || {
        rt.block_on(async move {
            let mut graph = TokioBuilder::default();
            let (out_ref, mut out_rx) = graph.output("output", 4);
            let stage = graph.stage("worker", async |out_ref, _msg: u32, eff| {
                let value: u32 = eff.external(MeasuredEffect).await;
                eff.send(&out_ref, value).await;
                out_ref
            });
            let stage = graph.wire_up(stage, out_ref);
            let send = graph.input(&stage);
            let running = graph.run(handle);
            timeout(Duration::from_secs(2), send.send(1)).await.expect("send").expect("queued");
            let got = timeout(Duration::from_secs(2), out_rx.next()).await.expect("recv");
            assert_eq!(got, Some(7));
            running.abort();
        });
    });
    writer.contents()
}

#[test]
fn tokio_effect_span_is_a_json_close_with_the_type_name() {
    let text = run_once("amaru_pure_stage::effect=debug");
    let line = text.lines().find(|line| line.contains("MeasuredEffect")).expect(&text);
    let value: serde_json::Value = serde_json::from_str(line).expect(line);
    let fields = value.get("fields").expect(line);
    assert_eq!(fields.get("message").and_then(serde_json::Value::as_str), Some("close"), "{text}");
    assert!(fields.get("time.busy").is_some(), "{text}");
    assert!(fields.get("time.idle").is_some(), "{text}");
    assert!(line.contains(EFFECT_SPAN_TARGET), "{text}");
}

#[test]
fn disabled_effect_span_writes_nothing() {
    let text = run_once("warn");
    assert!(text.is_empty(), "{text}");
}
