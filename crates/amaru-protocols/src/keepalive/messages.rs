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

use amaru_kernel::protocol_messages::handshake::check_length;
use minicbor::{Decode, Decoder, Encode, Encoder, decode, encode};

#[derive(
    Debug, PartialEq, Eq, Clone, Copy, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct Cookie(u16);

impl Default for Cookie {
    fn default() -> Self {
        Self::new()
    }
}

impl Cookie {
    pub fn new() -> Self {
        Self(0)
    }

    pub fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

impl From<u16> for Cookie {
    fn from(value: u16) -> Self {
        Self(value)
    }
}

impl From<Cookie> for u16 {
    fn from(value: Cookie) -> Self {
        value.0
    }
}

impl<T> Encode<T> for Cookie {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut T,
    ) -> Result<(), encode::Error<W::Error>> {
        e.u16(self.0)?;
        Ok(())
    }
}

impl<'b, T> Decode<'b, T> for Cookie {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut T) -> Result<Self, decode::Error> {
        Ok(Self(d.u16()?))
    }
}

#[derive(
    Debug, PartialEq, Eq, Clone, Copy, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum Message {
    KeepAlive(Cookie),
    ResponseKeepAlive(Cookie),
    Done,
}

impl<T> Encode<T> for Message {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut T,
    ) -> Result<(), encode::Error<W::Error>> {
        match self {
            Message::KeepAlive(cookie) => {
                e.array(2)?.u16(0)?;
                e.encode(cookie)?;
            }
            Message::ResponseKeepAlive(cookie) => {
                e.array(2)?.u16(1)?;
                e.encode(cookie)?;
            }
            Message::Done => {
                e.array(1)?.u16(2)?;
            }
        }

        Ok(())
    }
}

impl<'b, T> Decode<'b, T> for Message {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut T) -> Result<Self, decode::Error> {
        let len = d.array()?;
        let label = d.u16()?;

        match label {
            0 => {
                check_length(0, len, 2)?;
                let cookie = d.decode()?;
                Ok(Message::KeepAlive(cookie))
            }
            1 => {
                check_length(1, len, 2)?;
                let cookie = d.decode()?;
                Ok(Message::ResponseKeepAlive(cookie))
            }
            2 => {
                check_length(2, len, 1)?;
                Ok(Message::Done)
            }
            _ => Err(decode::Error::message("can't decode Message")),
        }
    }
}

/// Roundtrip property tests for handshake messages.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::keepalive::messages::Message::*;
    use amaru_kernel::prop_cbor_roundtrip;
    use proptest::prelude::*;
    use proptest::prop_compose;

    prop_cbor_roundtrip!(Message, any_message());

    // HELPERS

    prop_compose! {
        fn any_cookie()(n in any::<u16>()) -> Cookie {
            Cookie(n)
        }
    }

    prop_compose! {
        fn any_keep_alive_message()(cookie in any_cookie()) -> Message {
            KeepAlive(cookie)
        }
    }

    prop_compose! {
        fn any_response_keep_alive_message()(cookie in any_cookie()) -> Message {
            ResponseKeepAlive(cookie)
        }
    }

    pub fn done_message() -> impl Strategy<Value = Message> {
        Just(Done)
    }

    pub fn any_message() -> impl Strategy<Value = Message> {
        prop_oneof![
            1 => done_message(),
            1 => any_keep_alive_message(),
            1 => any_response_keep_alive_message(),
        ]
    }
}
