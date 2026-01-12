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

use amaru_kernel::{
    BlockHeader, Point, check_tagged_array_length, protocol_messages::tip::Tip, to_cbor,
};
use minicbor::{Decode, Decoder, Encode, Encoder, decode, encode};
use pure_stage::DeserializerGuards;

pub fn register_deserializers() -> DeserializerGuards {
    vec![pure_stage::register_data_deserializer::<Message>().boxed()]
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Ord, PartialOrd)]
pub enum Message {
    RequestNext,
    AwaitReply,
    RollForward(HeaderContent, Tip),
    RollBackward(Point, Tip),
    FindIntersect(Vec<Point>),
    IntersectFound(Point, Tip),
    IntersectNotFound(Tip),
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Ord, PartialOrd)]
pub struct HeaderContent {
    pub variant: u8,
    pub byron_prefix: Option<(u8, u64)>,
    pub cbor: Vec<u8>,
}

impl HeaderContent {
    pub fn v6(header: &BlockHeader) -> Self {
        Self::make_v6(to_cbor(header))
    }

    pub fn make_v6(cbor: Vec<u8>) -> Self {
        Self {
            variant: 6,
            byron_prefix: None,
            cbor,
        }
    }
}

impl Encode<()> for Message {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), encode::Error<W::Error>> {
        match self {
            Message::RequestNext => {
                e.array(1)?.u16(0)?;
                Ok(())
            }
            Message::AwaitReply => {
                e.array(1)?.u16(1)?;
                Ok(())
            }
            Message::RollForward(content, tip) => {
                e.array(3)?.u16(2)?;
                e.encode(content)?;
                e.encode(tip)?;
                Ok(())
            }
            Message::RollBackward(point, tip) => {
                e.array(3)?.u16(3)?;
                e.encode(point)?;
                e.encode(tip)?;
                Ok(())
            }
            Message::FindIntersect(points) => {
                e.array(2)?.u16(4)?;
                e.array(points.len() as u64)?;
                for point in points.iter() {
                    e.encode(point)?;
                }
                Ok(())
            }
            Message::IntersectFound(point, tip) => {
                e.array(3)?.u16(5)?;
                e.encode(point)?;
                e.encode(tip)?;
                Ok(())
            }
            Message::IntersectNotFound(tip) => {
                e.array(2)?.u16(6)?;
                e.encode(tip)?;
                Ok(())
            }
            Message::Done => {
                e.array(1)?.u16(7)?;
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
                check_tagged_array_length(0, len, 1)?;
                Ok(Message::RequestNext)
            }
            1 => {
                check_tagged_array_length(1, len, 1)?;
                Ok(Message::AwaitReply)
            }
            2 => {
                check_tagged_array_length(2, len, 3)?;
                let content = d.decode()?;
                let tip = d.decode()?;
                Ok(Message::RollForward(content, tip))
            }
            3 => {
                check_tagged_array_length(3, len, 3)?;
                let point = d.decode()?;
                let tip = d.decode()?;
                Ok(Message::RollBackward(point, tip))
            }
            4 => {
                check_tagged_array_length(4, len, 2)?;
                let points = d.decode()?;
                Ok(Message::FindIntersect(points))
            }
            5 => {
                check_tagged_array_length(5, len, 3)?;
                let point = d.decode()?;
                let tip = d.decode()?;
                Ok(Message::IntersectFound(point, tip))
            }
            6 => {
                check_tagged_array_length(6, len, 2)?;
                let tip = d.decode()?;
                Ok(Message::IntersectNotFound(tip))
            }
            7 => {
                check_tagged_array_length(7, len, 1)?;
                Ok(Message::Done)
            }
            _ => Err(decode::Error::message(
                "unknown variant for chainsync message",
            )),
        }
    }
}

impl<'b> Decode<'b, ()> for HeaderContent {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut ()) -> Result<Self, decode::Error> {
        let len = d.array()?;
        let variant = d.u8()?; // era variant

        match variant {
            // byron
            0 => {
                check_tagged_array_length(0, len, 2)?;
                let len = d.array()?;
                check_tagged_array_length(0, len, 2)?;

                // can't find a reference anywhere about the structure of these values, but they
                // seem to provide the Byron-specific variant of the header
                let (a, b): (u8, u64) = d.decode()?;

                d.tag()?;
                let bytes = d.bytes()?;

                Ok(HeaderContent {
                    variant,
                    byron_prefix: Some((a, b)),
                    cbor: Vec::from(bytes),
                })
            }
            // shelley and beyond
            v => {
                check_tagged_array_length(v as usize, len, 2)?;
                d.tag()?;
                let bytes = d.bytes()?;
                Ok(HeaderContent {
                    variant,
                    byron_prefix: None,
                    cbor: Vec::from(bytes),
                })
            }
        }
    }
}

impl Encode<()> for HeaderContent {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), encode::Error<W::Error>> {
        e.array(2)?;
        e.u8(self.variant)?;

        // variant 0 is byron
        if self.variant == 0 {
            e.array(2)?;

            if let Some((a, b)) = self.byron_prefix {
                e.array(2)?;
                e.u8(a)?;
                e.u64(b)?;
            } else {
                return Err(encode::Error::message(
                    "header variant 0 but no byron prefix",
                ));
            }

            e.tag(minicbor::data::IanaTag::Cbor)?;
            e.bytes(&self.cbor)?;
        } else {
            e.tag(minicbor::data::IanaTag::Cbor)?;
            e.bytes(&self.cbor)?;
        }

        Ok(())
    }
}

/// Roundtrip property tests for chainsync messages.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::chainsync::messages::Message::*;
    use amaru_kernel::prop_cbor_roundtrip;
    use amaru_kernel::protocol_messages::{
        handshake::tests::any_byron_prefix, point::tests::any_point, tip::tests::any_tip,
    };
    use proptest::prelude::*;
    use proptest::prop_compose;

    mod header_content {
        use super::*;
        prop_cbor_roundtrip!(HeaderContent, any_header_content());
    }

    mod message {
        use super::*;
        prop_cbor_roundtrip!(Message, any_message());
    }

    // HELPERS

    fn done_message() -> impl Strategy<Value = Message> {
        Just(Message::Done)
    }

    fn request_next_message() -> impl Strategy<Value = Message> {
        Just(Message::RequestNext)
    }

    fn await_reply_message() -> impl Strategy<Value = Message> {
        Just(Message::AwaitReply)
    }

    prop_compose! {
        fn any_vec_u8()(elems in proptest::collection::vec(any::<u8>(), 0..10)) -> Vec<u8> {
            elems
        }
    }

    prop_compose! {
        fn any_header_content()(variant in 0..8u8, byron_prefix in any_byron_prefix(), cbor in any_vec_u8()) -> HeaderContent {
            if variant == 0 {
                HeaderContent { variant, byron_prefix: Some(byron_prefix), cbor }
            } else {
                HeaderContent { variant, byron_prefix: None, cbor }
            }
        }
    }

    prop_compose! {
        fn roll_forward_message()(header_content in any_header_content(), tip in any_tip()) -> Message {
            RollForward(header_content, tip)
        }
    }

    prop_compose! {
        fn roll_backward_message()(point in any_point(), tip in any_tip()) -> Message {
            RollBackward(point, tip)
        }
    }

    prop_compose! {
        fn find_intersect_message()(points in proptest::collection::vec(any_point(), 0..3)) -> Message {
            FindIntersect(points)
        }
    }

    prop_compose! {
        fn intersect_found_message()(point in any_point(), tip in any_tip()) -> Message {
            IntersectFound(point, tip)
        }
    }

    prop_compose! {
        fn intersect_not_found_message()(tip in any_tip()) -> Message {
            IntersectNotFound(tip)
        }
    }

    pub fn any_message() -> impl Strategy<Value = Message> {
        prop_oneof![
            1 => done_message(),
            3 => request_next_message(),
            3 => await_reply_message(),
            3 => roll_forward_message(),
            3 => roll_backward_message(),
            3 => find_intersect_message(),
            3 => intersect_found_message(),
            3 => intersect_not_found_message(),
        ]
    }
}
