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

use amaru_kernel::Point;
use minicbor::{Decode, Decoder, Encode, Encoder, data::IanaTag, decode, encode};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Message {
    RequestRange { from: Point, through: Point },
    ClientDone,
    StartBatch,
    NoBlocks,
    Block { body: Vec<u8> },
    BatchDone,
}

impl Encode<()> for Message {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), encode::Error<W::Error>> {
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
                e.tag(IanaTag::Cbor)?;
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

impl<'b> Decode<'b, ()> for Message {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut ()) -> Result<Self, decode::Error> {
        let len = d.array()?;
        let label = d.u16()?;

        match label {
            0 => {
                check_length(0, len, 3)?;
                let from = d.decode()?;
                let through = d.decode()?;
                Ok(Message::RequestRange { from, through })
            }
            1 => {
                check_length(1, len, 1)?;
                Ok(Message::ClientDone)
            }
            2 => {
                check_length(2, len, 1)?;
                Ok(Message::StartBatch)
            }
            3 => {
                check_length(3, len, 1)?;
                Ok(Message::NoBlocks)
            }
            4 => {
                check_length(4, len, 2)?;
                d.tag()?;
                let body = d.bytes()?;
                Ok(Message::Block {
                    body: Vec::from(body),
                })
            }
            5 => {
                check_length(5, len, 1)?;
                Ok(Message::BatchDone)
            }
            _ => Err(decode::Error::message(
                "unknown variant for blockfetch message",
            )),
        }
    }
}

/// This function checks that the actual length of a CBOR array matches the expected length for
/// a message variant with a given label.
pub(crate) fn check_length(
    label: usize,
    actual: Option<u64>,
    expected: u64,
) -> Result<(), decode::Error> {
    if actual != Some(expected) {
        Err(decode::Error::message(format!(
            "expected array length {expected} for label {label}, got: {actual:?}"
        )))
    } else {
        Ok(())
    }
}

/// Roundtrip property tests for blockfetch messages.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use amaru_kernel::{Hash, HeaderHash, Point, Slot, prop_cbor_roundtrip};
    use proptest::prelude::*;
    use proptest::prop_compose;
    use std::str::FromStr;

    mod message {
        use super::*;
        prop_cbor_roundtrip!(Message, any_message());
    }

    // HELPERS

    prop_compose! {
        pub fn any_specific_point()(slot in any_slot(), header_hash in any_header_hash()) -> Point {
            Point::Specific(slot, header_hash)
        }
    }

    fn any_header_hash() -> impl Strategy<Value = HeaderHash> {
        Just(
            Hash::from_str("4df4505d862586f9e2c533c5fbb659f04402664db1b095aba969728abfb77301")
                .unwrap(),
        )
    }

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
        fn any_slot()(n in 0u64..=1000) -> Slot {
            Slot::from(n)
        }
    }

    pub fn any_point() -> impl Strategy<Value = Point> {
        prop_oneof![
            1 => Just(Point::Origin),
            3 => any_specific_point(),
        ]
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
