// Copyright 2025 PRAGMA
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

use futures_util::StreamExt;
use pure_stage::{StageGraph, tokio::TokioBuilder};
use std::time::Duration;
use tokio::time::timeout;

#[test]
fn basic() {
    tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut network = TokioBuilder::default();
    let output = network.make_stage("output");
    let mut rx = network.output(&output, 10);

    let double = network.stage(
        "double",
        async |to, msg: u32, eff| {
            eff.send(&to, msg * 2).await;
            to
        },
        output,
    );

    let send_double = network.input(&double);

    let handle = rt.handle().clone();
    rt.block_on(async move {
        // running needs `tokio::spawn` to work
        let graph = network.run(handle);

        for i in 0..10 {
            tracing::info!("sending {}", i);
            timeout(Duration::from_secs(1), send_double.send(i))
                .await
                .unwrap()
                .unwrap();
        }

        for i in 0..10 {
            tracing::info!("receiving {}", i);
            assert_eq!(
                timeout(Duration::from_secs(1), rx.next()).await.unwrap(),
                Some(i * 2)
            );
        }

        graph.abort();
    });
}
