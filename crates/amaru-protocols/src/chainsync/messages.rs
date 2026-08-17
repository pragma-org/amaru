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

use amaru_kernel::{EraName, Header, NetworkPoint, NetworkTip, Point, cbor, to_cbor, utils::debug_bytes};
use amaru_pure_stage::DeserializerGuards;

pub fn register_deserializers() -> DeserializerGuards {
    vec![amaru_pure_stage::register_data_deserializer::<Message>().boxed()]
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Ord, PartialOrd)]
pub enum Message {
    RequestNext(u8),
    AwaitReply,
    RollForward(HeaderContent, Point),
    RollBackward(NetworkPoint, Point),
    FindIntersect(Vec<NetworkPoint>),
    IntersectFound(NetworkPoint, Point),
    IntersectNotFound(Point),
    Done,
}

impl Message {
    pub fn message_type(&self) -> &str {
        match self {
            Message::RequestNext(_) => "RequestNext",
            Message::AwaitReply => "AwaitReply",
            Message::RollForward(_, _) => "RollForward",
            Message::RollBackward(_, _) => "RollBackward",
            Message::FindIntersect(_) => "FindIntersect",
            Message::IntersectFound(_, _) => "IntersectFound",
            Message::IntersectNotFound(_) => "IntersectNotFound",
            Message::Done => "Done",
        }
    }
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Ord, PartialOrd)]
pub struct HeaderContent {
    pub variant: EraName,
    pub byron_prefix: Option<(u8, u64)>,
    pub cbor: Vec<u8>,
}

impl std::fmt::Debug for HeaderContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeaderContent")
            .field("variant", &self.variant)
            .field("byron_prefix", &self.byron_prefix)
            .field("cbor", &debug_bytes(&self.cbor, 300))
            .field("cbor_len", &self.cbor.len())
            .finish()
    }
}

impl HeaderContent {
    pub fn new(header: &Header, era: EraName) -> Self {
        Self { variant: era, byron_prefix: None, cbor: to_cbor(header) }
    }

    pub fn with_bytes(bytes: Vec<u8>, variant: EraName) -> Self {
        Self { variant, byron_prefix: None, cbor: bytes }
    }
}

impl cbor::Encode<()> for Message {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Message::RequestNext(n) => {
                for _ in 0..*n {
                    e.array(1)?.u16(0)?;
                }
                Ok(())
            }
            Message::AwaitReply => {
                e.array(1)?.u16(1)?;
                Ok(())
            }
            Message::RollForward(content, tip) => {
                e.array(3)?.u16(2)?;
                e.encode(content)?;
                e.encode(NetworkTip::from(tip))?;
                Ok(())
            }
            Message::RollBackward(point, tip) => {
                e.array(3)?.u16(3)?;
                e.encode(point)?;
                e.encode(NetworkTip::from(tip))?;
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
                e.encode(NetworkTip::from(tip))?;
                Ok(())
            }
            Message::IntersectNotFound(tip) => {
                e.array(2)?.u16(6)?;
                e.encode(NetworkTip::from(tip))?;
                Ok(())
            }
            Message::Done => {
                e.array(1)?.u16(7)?;
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
                cbor::check_tagged_array_length(0, len, 1)?;
                Ok(Message::RequestNext(1))
            }
            1 => {
                cbor::check_tagged_array_length(1, len, 1)?;
                Ok(Message::AwaitReply)
            }
            2 => {
                cbor::check_tagged_array_length(2, len, 3)?;
                let content = d.decode()?;
                let tip = Point::from(d.decode::<NetworkTip>()?);
                Ok(Message::RollForward(content, tip))
            }
            3 => {
                cbor::check_tagged_array_length(3, len, 3)?;
                let point = d.decode()?;
                let tip = Point::from(d.decode::<NetworkTip>()?);
                Ok(Message::RollBackward(point, tip))
            }
            4 => {
                cbor::check_tagged_array_length(4, len, 2)?;
                let points = d.decode()?;
                Ok(Message::FindIntersect(points))
            }
            5 => {
                cbor::check_tagged_array_length(5, len, 3)?;
                let point = d.decode()?;
                let tip = Point::from(d.decode::<NetworkTip>()?);
                Ok(Message::IntersectFound(point, tip))
            }
            6 => {
                cbor::check_tagged_array_length(6, len, 2)?;
                let tip = Point::from(d.decode::<NetworkTip>()?);
                Ok(Message::IntersectNotFound(tip))
            }
            7 => {
                cbor::check_tagged_array_length(7, len, 1)?;
                Ok(Message::Done)
            }
            _ => Err(cbor::decode::Error::message("unknown variant for chainsync message")),
        }
    }
}

impl<'b> cbor::Decode<'b, ()> for HeaderContent {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut ()) -> Result<Self, cbor::decode::Error> {
        let len = d.array()?;
        let variant = EraName::from_header_variant(d.u8()?).map_err(cbor::decode::Error::custom)?;

        match variant {
            EraName::Byron => {
                cbor::check_tagged_array_length(0, len, 2)?;
                let len = d.array()?;
                cbor::check_tagged_array_length(0, len, 2)?;

                // can't find a reference anywhere about the structure of these values, but they
                // seem to provide the Byron-specific variant of the header
                let (a, b): (u8, u64) = d.decode()?;

                d.tag()?;
                // Conformance: the Haskell node unwraps CBOR-in-CBOR with cborg's `decodeBytes`,
                // which rejects indefinite-length byte strings, so we reject them too.
                #[allow(clippy::disallowed_methods)]
                let bytes = d.bytes()?;

                Ok(HeaderContent { variant, byron_prefix: Some((a, b)), cbor: Vec::from(bytes) })
            }
            EraName::Shelley
            | EraName::Allegra
            | EraName::Mary
            | EraName::Alonzo
            | EraName::Babbage
            | EraName::Conway
            | EraName::Dijkstra => {
                cbor::check_tagged_array_length(variant.header_variant().into(), len, 2)?;
                d.tag()?;
                // Conformance: the Haskell node unwraps CBOR-in-CBOR with cborg's `decodeBytes`,
                // which rejects indefinite-length byte strings, so we reject them too.
                #[allow(clippy::disallowed_methods)]
                let bytes = d.bytes()?;
                Ok(HeaderContent { variant, byron_prefix: None, cbor: Vec::from(bytes) })
            }
        }
    }
}

impl cbor::Encode<()> for HeaderContent {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.u8(self.variant.header_variant())?;

        if self.variant == EraName::Byron {
            e.array(2)?;

            if let Some((a, b)) = self.byron_prefix {
                e.array(2)?;
                e.u8(a)?;
                e.u64(b)?;
            } else {
                return Err(cbor::encode::Error::message("header variant 0 but no byron prefix"));
            }

            e.tag(cbor::IanaTag::Cbor)?;
            e.bytes(&self.cbor)?;
        } else {
            e.tag(cbor::IanaTag::Cbor)?;
            e.bytes(&self.cbor)?;
        }

        Ok(())
    }
}

/// Roundtrip property tests for chainsync messages.
#[cfg(test)]
mod tests {
    use amaru_kernel::{any_era_name, any_network_point, any_point, prop_cbor_roundtrip};
    use proptest::{prelude::*, prop_compose};

    use super::*;
    use crate::{chainsync::messages::Message::*, protocol_messages::handshake::tests::any_byron_prefix};

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
        Just(Message::RequestNext(1))
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
        fn any_header_content()(variant in any_era_name(), byron_prefix in any_byron_prefix(), cbor in any_vec_u8()) -> HeaderContent {
            if variant == EraName::Byron {
                HeaderContent { variant, byron_prefix: Some(byron_prefix), cbor }
            } else {
                HeaderContent { variant, byron_prefix: None, cbor }
            }
        }
    }

    prop_compose! {
        fn roll_forward_message()(header_content in any_header_content(), tip in any_point()) -> Message {
            RollForward(header_content, tip)
        }
    }

    prop_compose! {
        fn roll_backward_message()(point in any_network_point(), tip in any_point()) -> Message {
            RollBackward(point, tip)
        }
    }

    prop_compose! {
        fn find_intersect_message()(points in proptest::collection::vec(any_network_point(), 0..3)) -> Message {
            FindIntersect(points)
        }
    }

    prop_compose! {
        fn intersect_found_message()(point in any_network_point(), tip in any_point()) -> Message {
            IntersectFound(point, tip)
        }
    }

    prop_compose! {
        fn intersect_not_found_message()(tip in any_point()) -> Message {
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
