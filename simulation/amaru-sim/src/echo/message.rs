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

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Envelope<T> {
    pub src: String,
    pub dest: String,
    pub body: T,
}

impl<T> Envelope<T> {
    pub fn new(src: String, dest: String, body: T) -> Self {
        Self { src, dest, body }
    }

    pub fn is_client_message(&self) -> bool {
        self.src.starts_with("c")
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EchoMessage {
    Init {
        msg_id: u64,
        node_id: String,
        node_ids: Vec<String>,
    },
    InitOk {
        in_reply_to: u64,
    },
    Echo {
        msg_id: u64,
        echo: String,
    },
    EchoOk {
        msg_id: u64,
        in_reply_to: u64,
        echo: String,
    },
}

#[cfg(test)]
mod test {
    use proptest::{prelude::BoxedStrategy, proptest};
    use pure_stage::SendData;

    use super::EchoMessage;

    #[test]
    fn typetag_name() {
        let env = super::Envelope::<EchoMessage> {
            src: "src".to_string(),
            dest: "dest".to_string(),
            body: EchoMessage::Init {
                msg_id: 0,
                node_id: "node_id".to_string(),
                node_ids: vec!["node_id".to_string()],
            },
        };
        assert_eq!(
            env.typetag_name(),
            "amaru_sim::echo::message::Envelope<amaru_sim::echo::message::EchoMessage>"
        );
    }

    fn arbitrary_message() -> BoxedStrategy<EchoMessage> {
        use super::EchoMessage;
        use proptest::{collection::vec, prelude::*};

        prop_oneof![
            (any::<u64>(), any::<String>(), vec(any::<String>(), 0..10)).prop_map(
                |(msg_id, node_id, node_ids)| EchoMessage::Init {
                    msg_id,
                    node_id,
                    node_ids
                }
            ),
            (any::<u64>()).prop_map(|msg_id| EchoMessage::InitOk {
                in_reply_to: msg_id
            }),
            (any::<u64>(), any::<String>())
                .prop_map(|(msg_id, echo)| EchoMessage::Echo { msg_id, echo }),
            (any::<u64>(), any::<u64>(), any::<String>()).prop_map(
                |(msg_id, in_reply_to, echo)| EchoMessage::EchoOk {
                    msg_id,
                    in_reply_to,
                    echo
                }
            ),
        ]
        .boxed()
    }

    proptest! {
        #[test]
        fn roundtrip_echo_messages(message in arbitrary_message()) {
            let encoded = serde_json::to_string(&message).unwrap();
            let decoded = serde_json::from_str(&encoded).unwrap();
            assert_eq!(message, decoded);
        }
    }
}
