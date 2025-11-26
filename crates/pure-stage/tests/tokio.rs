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
use pure_stage::{StageGraph, StageRef, tokio::TokioBuilder};
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

    let mut graph = TokioBuilder::default();
    let double = graph.stage("double", async |to, msg: u32, eff| {
        eff.send(&to, msg * 2).await;
        to
    });
    let (out_ref, mut out_rx) = graph.output("output", 10);
    let double = graph.wire_up(double, out_ref);

    let send_double = graph.input(&double);

    let handle = rt.handle().clone();
    rt.block_on(async move {
        // running needs `tokio::spawn` to work
        let graph = graph.run(handle);

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
                timeout(Duration::from_secs(1), out_rx.next())
                    .await
                    .unwrap(),
                Some(i * 2)
            );
        }

        graph.abort();
    });
}

#[test]
fn add_stage() {
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct ParentState {
        child_ref: Option<StageRef<u32>>,
        output: StageRef<u32>,
    }

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct ChildState {
        value: u32,
        output: StageRef<u32>,
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut network = TokioBuilder::default();

    // Parent stage that creates a child stage
    let parent = network.stage("parent", async |mut state: ParentState, msg: u32, eff| {
        if state.child_ref.is_none() {
            // Create a child stage within the parent stage
            let child = eff
                .stage("child", async |mut state: ChildState, msg: u32, eff| {
                    state.value += msg;
                    eff.send(&state.output, state.value).await;
                    state
                })
                .await;

            // Wire up the child stage with initial state that includes the output reference
            let child_ref = eff
                .wire_up(
                    child,
                    ChildState {
                        value: 0u32,
                        output: state.output.clone(),
                    },
                )
                .await;
            state.child_ref = Some(child_ref);
        }

        // Send a message to the child stage
        if let Some(ref child) = state.child_ref {
            eff.send(child, msg).await;
        }

        state
    });

    let (output, mut rx) = network.output("output", 10);
    let parent = network.wire_up(
        parent,
        ParentState {
            child_ref: None,
            output: output.clone(),
        },
    );
    let input = network.input(&parent);

    let running = network.run(rt.handle().clone());

    rt.block_on(async move {
        input.send(42).await.unwrap();
        assert_eq!(rx.next().await.unwrap(), 42);
        input.send(42).await.unwrap();
        assert_eq!(rx.next().await.unwrap(), 84);
    });

    running.abort();
}
