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

//! Message throughput of the Tokio runtime's stage-to-stage send path, with roomy
//! mailboxes (every send takes the uncontended path) and with mailbox size 1 (sends
//! regularly hit a full mailbox and take the blocked-send bookkeeping path).
//!
//! Each round measures the whole `input -> first -> second -> output` pipeline, and the
//! input sender and output receiver have performance characteristics of their own, so the
//! result does not isolate the cost of a single stage-to-stage send. It is meant for
//! comparing two revisions of the runtime on the same pipeline, not for absolute numbers.

#![expect(clippy::unwrap_used)]

use std::time::Duration;

use amaru_pure_stage::{
    Effects, Receiver, Sender, StageGraph, StageRef,
    tokio::{TokioBuilder, TokioRunning},
};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};

const MESSAGES: usize = 1_000;

async fn forward(next: StageRef<u64>, msg: u64, eff: Effects<u64>) -> StageRef<u64> {
    eff.send(&next, msg).await;
    next
}

fn pipeline(mailbox_size: usize) -> (tokio::runtime::Runtime, TokioRunning, Sender<u64>, Receiver<u64>) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut network = TokioBuilder::default().with_mailbox_size(mailbox_size);
    let first = network.stage("first", forward);
    let second = network.stage("second", forward);
    let (out, rx) = network.output::<u64>("out", MESSAGES);
    let first_ref = first.sender();
    let second_ref = second.sender();
    network.wire_up(first, second_ref.clone());
    network.wire_up(second, out);
    let input = network.input(&first_ref);
    let running = network.run(rt.handle().clone());
    (rt, running, input, rx)
}

fn run_round(rt: &tokio::runtime::Runtime, input: &Sender<u64>, rx: &mut Receiver<u64>) {
    rt.block_on(async {
        for msg in 0..MESSAGES {
            input.send(msg as u64).await.unwrap();
        }
        let mut received = 0;
        while received < MESSAGES {
            received += rx.drain().count();
            tokio::task::yield_now().await;
        }
    });
}

fn messaging(c: &mut Criterion) {
    let mut group = c.benchmark_group("Tokio stage messaging");
    group.measurement_time(Duration::from_secs(10));
    group.throughput(Throughput::Elements(MESSAGES as u64));

    let (rt, running, input, mut rx) = pipeline(1_024);
    group.bench_function("roomy mailboxes", |b| b.iter(|| run_round(&rt, &input, &mut rx)));
    running.abort();
    drop((rt, input, rx));

    let (rt, running, input, mut rx) = pipeline(1);
    group.bench_function("mailbox size 1", |b| b.iter(|| run_round(&rt, &input, &mut rx)));
    running.abort();
    drop((rt, input, rx));

    group.finish();
}

criterion_group!(benches, messaging);
criterion_main!(benches);
