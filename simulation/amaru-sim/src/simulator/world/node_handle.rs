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

use crate::simulator::Envelope;
use anyhow::anyhow;
use pure_stage::simulation::running::SimulationRunning;
use serde::{Deserialize, Serialize};
use std::{
    fmt::{Debug, Display},
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
};
use tokio::runtime::Handle;
use tracing::info_span;

/// A `NodeHandle` is an async function that sends an Envelope<Msg> to a node and returns a list of Envelope<Msg>.
/// as the result of processing that message (Envelope holds source/destination values representing node ids).
///
/// If no message is provided (i.e., None), the node is given a chance to make progress.
/// If the node can made progress and is not blocked, it returns None, indicating that further progress can be made later.
/// Otherwise, it returns Some(Vec<Envelope<Msg>>), representing the outgoing messages produced by the node so far.
///
/// Additionally, it provides an async function to shutdown the node gracefully.
///
pub struct NodeHandle<Msg> {
    handle:
        Box<dyn FnMut(Option<Envelope<Msg>>) -> Result<Option<Vec<Envelope<Msg>>>, anyhow::Error>>,
    pub(crate) close: Box<dyn FnMut()>,
}

impl<Msg> NodeHandle<Msg> {
    pub fn new<
        F: FnMut(Option<Envelope<Msg>>) -> Result<Option<Vec<Envelope<Msg>>>, anyhow::Error> + 'static,
        G: FnMut() + 'static,
    >(
        handle: F,
        close: G,
    ) -> Self {
        Self {
            handle: Box::new(handle),
            close: Box::new(close),
        }
    }

    pub fn handle_msg(
        &mut self,
        msg: Option<Envelope<Msg>>,
    ) -> Result<Option<Vec<Envelope<Msg>>>, anyhow::Error> {
        info_span!("handle_msg").in_scope(|| (self.handle)(msg))
    }

    /// Make some progress in the node execution.
    /// If the node cannot make any progress return its outgoing messages.
    pub fn step(&mut self) -> Result<StepResult<Msg>, anyhow::Error> {
        match (self.handle)(None) {
            Ok(Some(messages)) => Ok(StepResult::Finished(messages)),
            Ok(None) => Ok(StepResult::Continue),
            Err(e) => Err(e),
        }
    }

    /// Create a stateful function that can be used to send messages to node and receive messages from it.
    ///
    ///  * `input` is a handle used to send messages to the node.
    ///  * `init_message` is a handle used to receive the chain sync initialization message.
    ///  * `output` is a handle used to receive messages from the node.
    ///  * `running` is the simulated node, waiting for messages to arrive.
    ///
    pub fn from_pure_stage(
        mut _running: SimulationRunning,
        _rt: Handle,
    ) -> anyhow::Result<NodeHandle<Msg>>
    where
        Msg: PartialEq
            + Send
            + Debug
            + Display
            + serde::Serialize
            + serde::de::DeserializeOwned
            + 'static,
    {
        // TODO: make different nodes for upstream peer, node under test and downstream peer
        let handle = Box::new(
            move |_msg: Option<Envelope<Msg>>| Ok(None),
            // Some(msg) => {
            //     trace!(msg = %msg, "enqueuing");
            //     running.enqueue_msg(&input, [msg]);
            //     match running.run_one_step(&rt) {
            //         Some(_blocked) => {
            //             let mut result = init_messages.drain().collect::<Vec<_>>();
            //             result.extend(output.drain().collect::<Vec<_>>());
            //             Ok(Some(result))
            //         }
            //         None => Ok(None),
            //     }
            // }
            // None => match running.run_one_step(&rt) {
            //     Some(_) => Ok(Some(output.drain().collect::<Vec<_>>())),
            //     None => Ok(None),
            // },
        );

        let close = Box::new(move || ());

        Ok(NodeHandle::new(handle, close))
    }

    /// Start a node executable and create a node handle that communicates with that node via stdin/stdout.
    ///
    /// * `filepath` is the path to the executable.
    /// * `args` are the arguments to pass to the executable.
    ///
    pub fn from_executable(filepath: &Path, args: &[&str]) -> anyhow::Result<NodeHandle<Msg>>
    where
        Msg: Serialize + for<'a> Deserialize<'a>,
    {
        let mut child = Command::new(filepath)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| anyhow!("Failed to create process: {}", e))?;
        let mut stdin = child.stdin.take().ok_or(anyhow!("Failed to take stdin"))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or(anyhow!("Failed to take stdout"))?;

        let handle = Box::new(move |msg: Option<Envelope<Msg>>| {
            let json =
                serde_json::to_string(&msg).map_err(|e| anyhow!("Failed to encode JSON: {}", e))?;
            println!("About to write: {}", json);
            writeln!(stdin, "{}", json)
                .map_err(|e| anyhow!("Failed to write to child's stdin: {}", e))?;
            stdin
                .flush()
                .map_err(|e| anyhow!("Failed to flush child's stdin: {}", e))?;

            let mut reader = BufReader::new(&mut stdout);
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .map_err(|e| anyhow!("Failed to read from child's stdout: {}", e))?;

            println!("Just read: {}", &line);
            Ok(Some(
                serde_json::from_str(&line)
                    // TODO: Read more than one message? Either make SUT send one message
                    // per line and end by a termination token, or make write a JSON array
                    // of messages?
                    .map(|msg: Envelope<Msg>| vec![msg])
                    .map_err(|e| anyhow!("Failed to decode JSON: {}", e))?,
            ))
        });

        let close = Box::new(move || {
            child
                .kill()
                .map_err(|e| anyhow!("Failed to terminate process: {}", e))
                .ok();
        });

        Ok(NodeHandle::new(handle, close))
    }
}

pub enum StepResult<Msg> {
    Finished(Vec<Envelope<Msg>>),
    Continue,
}
