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

use amaru_kernel::{Point, cbor};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Message {
    RequestRange { from: Point, through: Point },
    ClientDone,
    StartBatch,
    NoBlocks,
    Block { body: Vec<u8> },
    BatchDone,
}

impl Message {
    pub fn message_type(&self) -> &str {
        match self {
            Message::RequestRange { .. } => "RequestRange",
            Message::ClientDone => "ClientDone",
            Message::StartBatch => "StartBatch",
            Message::NoBlocks => "NoBlocks",
            Message::Block { .. } => "Block",
            Message::BatchDone => "BatchDone",
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
            Message::RequestRange { from, through } => {
                e.array(3)?.u16(0)?;
                e.encode(from)?;
                e.encode(through)?;
                Ok(())
            }
            Message::ClientDone => {
                e.array(1)?.u16(1)?;
                Ok(())
            }
            Message::StartBatch => {
                e.array(1)?.u16(2)?;
                Ok(())
            }
            Message::NoBlocks => {
                e.array(1)?.u16(3)?;
                Ok(())
            }
            Message::Block { body } => {
                e.array(2)?.u16(4)?;
                e.tag(cbor::IanaTag::Cbor)?;
                e.bytes(body)?;
                Ok(())
            }
            Message::BatchDone => {
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
                Ok(Message::RequestRange { from, through })
            }
            1 => {
                cbor::check_tagged_array_length(1, len, 1)?;
                Ok(Message::ClientDone)
            }
            2 => {
                cbor::check_tagged_array_length(2, len, 1)?;
                Ok(Message::StartBatch)
            }
            3 => {
                cbor::check_tagged_array_length(3, len, 1)?;
                Ok(Message::NoBlocks)
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

                let body = d.bytes()?;
                Ok(Message::Block {
                    body: Vec::from(body),
                })
            }
            5 => {
                cbor::check_tagged_array_length(5, len, 1)?;
                Ok(Message::BatchDone)
            }
            _ => Err(cbor::decode::Error::message(
                "unknown variant for blockfetch message",
            )),
        }
    }
}

/// Roundtrip property tests for blockfetch messages.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use amaru_kernel::{any_point, prop_cbor_roundtrip};
    use proptest::{prelude::*, prop_compose};

    prop_cbor_roundtrip!(Message, any_message());

    // HELPERS

    fn block_message() -> impl Strategy<Value = Message> {
        Just(Message::Block {
            body: vec![0u8; 128],
        })
    }

    fn no_blocks_message() -> impl Strategy<Value = Message> {
        Just(Message::NoBlocks)
    }

    fn batch_done_message() -> impl Strategy<Value = Message> {
        Just(Message::BatchDone)
    }

    fn start_batch_message() -> impl Strategy<Value = Message> {
        Just(Message::StartBatch)
    }

    fn client_done_message() -> impl Strategy<Value = Message> {
        Just(Message::ClientDone)
    }

    prop_compose! {
        fn request_range_message()(from in any_point(), through in any_point()) -> Message {
            Message::RequestRange {from, through}
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
