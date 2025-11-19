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
use crate::sync::ChainSyncMessage;
use futures_util::SinkExt;
use tokio::io::{Stdout, stdout};
use tokio_util::codec::{FramedWrite, LinesCodec};

pub struct OutputWriter {
    pub writer: FramedWrite<Stdout, LinesCodec>,
}

impl Default for OutputWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputWriter {
    pub fn new() -> Self {
        let writer = FramedWrite::new(stdout(), LinesCodec::new());
        Self { writer }
    }

    pub async fn write(&mut self, messages: Vec<Envelope<ChainSyncMessage>>) {
        for msg in messages {
            let line = serde_json::to_string(&msg).unwrap();
            self.writer.send(line).await.unwrap();
        }
    }
}
