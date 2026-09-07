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

use amaru_kernel::{NetworkPoint, cbor};
use amaru_pure_stage::define_messages;

define_messages! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
    pub enum Message {
        RequestRange { from: NetworkPoint, through: NetworkPoint },
        ClientDone,
        StartBatch,
        NoBlocks,
        Block { body: Vec<u8> },
        BatchDone,
    }
}

impl Message {
    pub fn message_type(&self) -> &str {
        match self {
            Message::RequestRange(_) => "RequestRange",
            Message::ClientDone(_) => "ClientDone",
            Message::StartBatch(_) => "StartBatch",
            Message::NoBlocks(_) => "NoBlocks",
            Message::Block(_) => "Block",
            Message::BatchDone(_) => "BatchDone",
        }
    }
}

impl cbor::Encode<()> for Message {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Message::RequestRange(RequestRange { from, through }) => {
                e.array(3)?.u16(0)?;
                e.encode(from)?;
                e.encode(through)?;
                Ok(())
            }
            Message::ClientDone(_) => {
                e.array(1)?.u16(1)?;
                Ok(())
            }
            Message::StartBatch(_) => {
                e.array(1)?.u16(2)?;
                Ok(())
            }
            Message::NoBlocks(_) => {
                e.array(1)?.u16(3)?;
                Ok(())
            }
            Message::Block(Block { body }) => {
                e.array(2)?.u16(4)?;
                e.tag(cbor::IanaTag::Cbor)?;
                e.bytes(body)?;
                Ok(())
            }
            Message::BatchDone(_) => {
                e.array(1)?.u16(5)?;
                Ok(())
            }
        }
    }
}

impl<'b> cbor::Decode<'b, ()> for Message {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut ()) -> Result<Self, cbor::decode::Error> {
        let len = d.array()?;
        let label = d.u16()?;

        match label {
            0 => {
                cbor::check_tagged_array_length(0, len, 3)?;
                let from = d.decode()?;
                let through = d.decode()?;
                Ok(RequestRange { from, through }.into())
            }
            1 => {
                cbor::check_tagged_array_length(1, len, 1)?;
                Ok(ClientDone.into())
            }
            2 => {
                cbor::check_tagged_array_length(2, len, 1)?;
                Ok(StartBatch.into())
            }
            3 => {
                cbor::check_tagged_array_length(3, len, 1)?;
                Ok(NoBlocks.into())
            }
            4 => {
                cbor::check_tagged_array_length(4, len, 2)?;
                let tag = d.tag()?;
                if tag != cbor::IanaTag::Cbor.tag() {
                    return Err(cbor::decode::Error::message(format!(
                        "unexpected tag for Block: expected {}, got {}",
                        cbor::IanaTag::Cbor.tag(),
                        tag
                    )));
                }

                // Conformance: the Haskell node unwraps CBOR-in-CBOR with cborg's `decodeBytes`,
                // which rejects indefinite-length byte strings, so we reject them too.
                #[allow(clippy::disallowed_methods)]
                let body = d.bytes()?;
                Ok(Block { body: Vec::from(body) }.into())
            }
            5 => {
                cbor::check_tagged_array_length(5, len, 1)?;
                Ok(BatchDone.into())
            }
            _ => Err(cbor::decode::Error::message("unknown variant for blockfetch message")),
        }
    }
}

/// Roundtrip property tests for blockfetch messages.
#[cfg(test)]
pub(crate) mod tests {
    use amaru_kernel::{any_network_point, prop_cbor_roundtrip};
    use proptest::{prelude::*, prop_compose};

    use super::*;

    prop_cbor_roundtrip!(Message, any_message());

    // HELPERS

    fn block_message() -> impl Strategy<Value = Message> {
        Just(Block { body: vec![0u8; 128] }.into())
    }

    fn no_blocks_message() -> impl Strategy<Value = Message> {
        Just(NoBlocks.into())
    }

    fn batch_done_message() -> impl Strategy<Value = Message> {
        Just(BatchDone.into())
    }

    fn start_batch_message() -> impl Strategy<Value = Message> {
        Just(StartBatch.into())
    }

    fn client_done_message() -> impl Strategy<Value = Message> {
        Just(ClientDone.into())
    }

    prop_compose! {
        fn request_range_message()(from in any_network_point(), through in any_network_point()) -> Message {
            RequestRange { from, through }.into()
        }
    }

    pub fn any_message() -> impl Strategy<Value = Message> {
        prop_oneof![
            1 => block_message(),
            3 => no_blocks_message(),
            3 => start_batch_message(),
            3 => batch_done_message(),
            3 => client_done_message(),
            3 => request_range_message(),
        ]
    }
}
