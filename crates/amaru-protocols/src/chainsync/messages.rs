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

pub use amaru_kernel::HeaderContent;
use amaru_kernel::{NetworkPoint, NetworkTip, Point, cbor};
use amaru_pure_stage::DeserializerGuards;

/// Ouroboros Network's `smallByteLimit`, used by `byteLimitsChainSync`.
const MAX_FIND_INTERSECT_BYTES: usize = 0xffff;

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
        let start = d.position();
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
                let end = start.saturating_add(MAX_FIND_INTERSECT_BYTES).min(d.input().len());
                let mut bounded = cbor::Decoder::new(&d.input()[..end]);
                bounded.set_position(d.position());
                let points = bounded.decode().map_err(|error| {
                    if error.is_end_of_input() && end - start == MAX_FIND_INTERSECT_BYTES {
                        cbor::decode::Error::message("ChainSync intersection exceeds message size limit")
                    } else {
                        error
                    }
                })?;
                d.set_position(bounded.position());
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

/// Roundtrip property tests for chainsync messages.
#[cfg(test)]
mod tests {
    use amaru_kernel::{EraName, prop_cbor_roundtrip};
    use proptest::{prelude::*, prop_compose};

    use super::*;
    use crate::{chainsync::messages::Message::*, protocol_messages::handshake::tests::any_byron_prefix};

    mod header_content {
        use super::*;
        prop_cbor_roundtrip!(HeaderContent, any_header_content());

        #[test]
        fn cbor_field_json_is_hex_string() {
            let content = HeaderContent::with_bytes(vec![0xab, 0xcd], EraName::Conway);
            let json = amaru_kernel::json::to_value(&content).expect("json");
            assert_eq!(json["cbor"], "abcd");
        }
    }

    mod message {
        use super::*;
        prop_cbor_roundtrip!(Message, any_message());

        #[test]
        fn rejects_indefinite_length_array() {
            assert!(matches!(cbor::from_cbor_no_leftovers::<Message>(&[0x81, 0x00]), Ok(Message::RequestNext(_))));
            assert!(cbor::from_cbor_no_leftovers::<Message>(&[0x9f, 0x00, 0xff]).is_err());
        }

        #[test]
        fn bounds_intersection_messages_before_allocating_the_whole_point_list() {
            let points = vec![NetworkPoint::Origin; MAX_FIND_INTERSECT_BYTES - 5];
            let bytes = amaru_kernel::to_cbor(&FindIntersect(points.clone()));
            assert_eq!(bytes.len(), MAX_FIND_INTERSECT_BYTES);
            assert_eq!(cbor::from_cbor_no_leftovers::<Message>(&bytes).unwrap(), FindIntersect(points));

            let oversized =
                amaru_kernel::to_cbor(&FindIntersect(vec![NetworkPoint::Origin; MAX_FIND_INTERSECT_BYTES - 4]));
            assert!(cbor::from_cbor_no_leftovers::<Message>(&oversized).is_err());
            assert!(
                cbor::from_cbor_no_leftovers::<Message>(&[0x82, 4, 0x9b, 255, 255, 255, 255, 255, 255, 255, 255])
                    .is_err()
            );
        }

        #[test]
        fn intersection_limit_applies_per_message_when_pipelined() {
            let message = FindIntersect(vec![NetworkPoint::Origin; MAX_FIND_INTERSECT_BYTES - 5]);
            let mut bytes = amaru_kernel::to_cbor(&message);
            bytes.extend(amaru_kernel::to_cbor(&message));
            let mut decoder = cbor::Decoder::new(&bytes);
            assert_eq!(decoder.decode::<Message>().unwrap(), message);
            assert_eq!(decoder.decode::<Message>().unwrap(), message);
            assert_eq!(decoder.position(), bytes.len());
        }
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
        fn any_header_content()(variant in any::<EraName>(), byron_prefix in any_byron_prefix(), cbor in any_vec_u8()) -> HeaderContent {
            if variant == EraName::Byron {
                HeaderContent { variant, byron_prefix: Some(byron_prefix), cbor }
            } else {
                HeaderContent { variant, byron_prefix: None, cbor }
            }
        }
    }

    prop_compose! {
        fn roll_forward_message()(header_content in any_header_content(), tip in any::<Point>()) -> Message {
            RollForward(header_content, tip)
        }
    }

    prop_compose! {
        fn roll_backward_message()(point in any::<NetworkPoint>(), tip in any::<Point>()) -> Message {
            RollBackward(point, tip)
        }
    }

    prop_compose! {
        fn find_intersect_message()(points in proptest::collection::vec(any::<NetworkPoint>(), 0..3)) -> Message {
            FindIntersect(points)
        }
    }

    prop_compose! {
        fn intersect_found_message()(point in any::<NetworkPoint>(), tip in any::<Point>()) -> Message {
            IntersectFound(point, tip)
        }
    }

    prop_compose! {
        fn intersect_not_found_message()(tip in any::<Point>()) -> Message {
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
