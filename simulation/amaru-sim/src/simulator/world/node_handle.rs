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

use crate::echo::Envelope;
use anyhow::anyhow;
use pure_stage::simulation::SimulationRunning;
use pure_stage::{Receiver, StageRef};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use tracing::info;

/// A `NodeHandle` is an async function that sends an Envelope<Msg> to a node and returns a list of Envelope<Msg>.
/// as the result of processing that message (Envelope holds source/destination values representing node ids).
///
/// Additionally, it provides  n async function to shutdown the node gracefully.
///
pub struct NodeHandle<Msg> {
    handle: Box<dyn FnMut(Envelope<Msg>) -> Result<Vec<Envelope<Msg>>, anyhow::Error>>,
    pub(crate) close: Box<dyn FnMut()>,
}

impl<Msg> NodeHandle<Msg> {
    pub fn new<
        F: FnMut(Envelope<Msg>) -> Result<Vec<Envelope<Msg>>, anyhow::Error> + 'static,
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

    pub fn handle_msg(&mut self, msg: Envelope<Msg>) -> Result<Vec<Envelope<Msg>>, anyhow::Error> {
        (self.handle)(msg)
    }

    /// Create a stateful function that can be used to send messages to node and receive messages from it.
    ///
    ///  * `input` is a handle used to send messages to the node.
    ///  * `output` is a handle used to receive messages from the node.
    ///  * `running` is the simulated node, waiting for messages to arrive.
    ///
    pub fn from_pure_stage<St>(
        input: StageRef<Envelope<Msg>, St>,
        mut output: Receiver<Envelope<Msg>>,
        mut running: SimulationRunning,
    ) -> anyhow::Result<NodeHandle<Msg>>
    where
        Msg: PartialEq + Send + Debug + serde::Serialize + serde::de::DeserializeOwned + 'static,
        St: 'static,
    {
        let handle = Box::new(move |msg: Envelope<Msg>| {
            info!(msg = ?msg, "enqueuing");
            running.enqueue_msg(&input, [msg]);
            running.run_until_blocked().assert_idle();
            Ok(output.drain().collect::<Vec<_>>())
        });

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

        let handle = Box::new(move |msg: Envelope<Msg>| {
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
            serde_json::from_str(&line)
                // TODO: Read more than one message? Either make SUT send one message
                // per line and end by a termination token, or make write a JSON array
                // of messages?
                .map(|msg: Envelope<Msg>| vec![msg])
                .map_err(|e| anyhow!("Failed to decode JSON: {}", e))
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
