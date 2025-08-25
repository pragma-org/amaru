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

use super::{EchoMessage, EchoMessage::*, Envelope};

pub struct EchoService {
    node_id: String,
    count: u64,
}

pub enum EchoError {
    UnexpectedMessage(EchoMessage),
}

impl Default for EchoService {
    fn default() -> Self {
        Self::new()
    }
}

impl EchoService {
    pub fn new() -> Self {
        Self {
            node_id: "".to_string(),
            count: 0,
        }
    }

    pub fn handle_echo(
        &mut self,
        msg: Envelope<EchoMessage>,
    ) -> Result<Envelope<EchoMessage>, EchoError> {
        match msg.body {
            Init {
                msg_id,
                node_id,
                node_ids,
            } => {
                let body = self.init(msg_id, node_id, node_ids);
                Ok(Envelope {
                    src: self.node_id.clone(),
                    dest: msg.src,
                    body,
                })
            }
            Echo { msg_id, echo } => Ok(Envelope {
                src: self.node_id.clone(),
                dest: msg.src,
                body: self.echo(msg_id, echo),
            }),
            other => Err(EchoError::UnexpectedMessage(other)),
        }
    }

    fn init(&mut self, msg_id: u64, node_id: String, _node_ids: Vec<String>) -> EchoMessage {
        self.node_id = node_id;
        InitOk {
            in_reply_to: msg_id,
        }
    }

    fn echo(&mut self, msg_id: u64, echo: String) -> EchoMessage {
        self.count += 1;
        if self.count.is_multiple_of(5) {
            EchoOk {
                msg_id: self.count,
                in_reply_to: msg_id,
                echo: echo.to_uppercase(),
            }
        } else {
            EchoOk {
                msg_id: self.count,
                in_reply_to: msg_id,
                echo,
            }
        }
    }
}
