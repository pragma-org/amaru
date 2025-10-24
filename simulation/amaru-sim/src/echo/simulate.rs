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

use crate::echo::{EchoMessage, Envelope};
use crate::simulator::{
    Entry, GeneratedEntries, History, NodeHandle, generate_arrival_times, generate_u8,
    generate_vec, generate_zip_with,
};
use pure_stage::simulation::SimulationBuilder;
use pure_stage::{Instant, StageGraph, StageRef};
use rand::prelude::StdRng;
use std::time::Duration;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct State(u64, StageRef<Envelope<EchoMessage>>);

/// Start a node that has just one "echo" stage.
/// The regular echo behavior is to respond with the same message that was sent.
/// However we simulate a bug here where every 5th message is uppercased.
pub fn spawn_echo_node(_node_id: String) -> NodeHandle<EchoMessage> {
    let mut network = SimulationBuilder::default();
    let stage = network.stage(
        "echo",
        async |mut state: State, msg: Envelope<EchoMessage>, eff| {
            if let EchoMessage::Echo { msg_id, echo } = &msg.body {
                state.0 += 1;
                // Insert a bug every 5 messages.
                let echo_response = if state.0.is_multiple_of(5) {
                    echo.to_string().to_uppercase()
                } else {
                    echo.to_string()
                };
                let reply = Envelope {
                    src: msg.dest,
                    dest: msg.src,
                    body: EchoMessage::EchoOk {
                        msg_id: state.0,
                        in_reply_to: *msg_id,
                        echo: echo_response,
                    },
                };
                eff.send(&state.1, reply).await;
                state
            } else {
                panic!("Got a message that wasn't an echo: {:?}", msg.body)
            }
        },
    );
    let (output, rx) = network.output("output", 10);

    // This output is not used, but we need to wire it up to make the stage.
    let (_, init) = network.output("init", 10);
    let stage = network.wire_up(stage, State(0, output));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let running = network.run(rt.handle().clone());

    NodeHandle::from_pure_stage(stage.without_state(), init, rx, running).unwrap()
}

/// Generate some input echo messages at different arrival times.
pub fn echo_generator(rng: &mut StdRng) -> GeneratedEntries<EchoMessage, ()> {
    let now = Instant::at_offset(Duration::from_secs(0));
    let size = 20;
    let entries = generate_zip_with(
        size,
        generate_vec(generate_u8(0, 128)),
        generate_arrival_times(now, 200.0),
        |msg, arrival_time| Entry {
            arrival_time,
            envelope: Envelope {
                src: "c1".to_string(),
                dest: "n1".to_string(),
                body: EchoMessage::Echo {
                    msg_id: 0,
                    echo: format!("Please echo {}", msg),
                },
            },
        },
    )(rng);

    GeneratedEntries::new(entries, ())
}

/// Check that for every echo response from the node, there is a matching echo request that was sent to the node.
/// The response must have the same `in_reply_to` as the request's `msg_id`, and the echoed message must
/// match the original message.
pub fn echo_property(
    history: &History<EchoMessage>,
    _generation_context: &(),
) -> Result<(), String> {
    // TODO: Take response time into account.
    for (index, msg) in history
        .0
        .iter()
        .enumerate()
        .filter(|(_index, msg)| msg.src.starts_with("c"))
    {
        if let EchoMessage::Echo { msg_id, echo } = &msg.body {
            let response = history.0.split_at(index + 1).1.iter().find(|resp| {
                resp.dest == msg.src
                    && matches!(&resp.body, EchoMessage::EchoOk { in_reply_to, echo: resp_echo, .. }
                                if in_reply_to == msg_id && resp_echo == echo)
            });
            if response.is_none() {
                return Err(format!(
                    "No matching response found for echo request: {:?}",
                    msg
                ));
            }
        }
    }
    Ok(())
}
