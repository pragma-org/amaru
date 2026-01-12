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

use amaru_kernel::check_tagged_array_length;
use amaru_kernel::protocol_messages::{
    handshake::RefuseReason, version_number::VersionNumber, version_table::VersionTable,
};
use minicbor::{Decode, Decoder, Encode, Encoder, decode, encode};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Message<D>
where
    D: fmt::Debug + Clone,
{
    Propose(VersionTable<D>),
    Accept(VersionNumber, D),
    Refuse(RefuseReason),
    QueryReply(VersionTable<D>),
}

impl<D> Encode<()> for Message<D>
where
    D: fmt::Debug + Clone + Encode<VersionNumber>,
    VersionTable<D>: Encode<()>,
{
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), encode::Error<W::Error>> {
        match self {
            Message::Propose(version_table) => {
                e.array(2)?.u16(0)?;
                e.encode(version_table)?;
            }
            Message::Accept(version_number, version_data) => {
                e.array(3)?.u16(1)?;
                e.encode(version_number)?;
                let mut ctx = *version_number;
                e.encode_with(version_data, &mut ctx)?;
            }
            Message::Refuse(reason) => {
                e.array(2)?.u16(2)?;
                e.encode(reason)?;
            }
            Message::QueryReply(version_table) => {
                e.array(2)?.u16(3)?;
                e.encode(version_table)?;
            }
        };

        Ok(())
    }
}

impl<'b, D> Decode<'b, ()> for Message<D>
where
    D: Decode<'b, VersionNumber> + fmt::Debug + Clone,
    VersionTable<D>: Decode<'b, ()>,
{
    fn decode(d: &mut Decoder<'b>, _ctx: &mut ()) -> Result<Self, decode::Error> {
        let len = d.array()?;

        match d.u16()? {
            0 => {
                check_tagged_array_length(0, len, 2)?;
                let version_table = d.decode()?;
                Ok(Message::Propose(version_table))
            }
            1 => {
                check_tagged_array_length(1, len, 3)?;
                let version_number = d.decode()?;
                let mut ctx = version_number;
                let version_data = d.decode_with(&mut ctx)?;
                Ok(Message::Accept(version_number, version_data))
            }
            2 => {
                check_tagged_array_length(2, len, 2)?;
                let reason: RefuseReason = d.decode()?;
                Ok(Message::Refuse(reason))
            }
            3 => {
                check_tagged_array_length(3, len, 2)?;
                let version_table = d.decode()?;
                Ok(Message::QueryReply(version_table))
            }
            n => Err(decode::Error::message(format!(
                "unknown variant for handshake message: {}",
                n,
            ))),
        }
    }
}

/// Roundtrip property tests for handshake messages.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::handshake::messages::Message::*;
    use amaru_kernel::prop_cbor_roundtrip;
    use amaru_kernel::protocol_messages::handshake::tests::any_refuse_reason;
    use amaru_kernel::protocol_messages::version_data::VersionData;
    use amaru_kernel::protocol_messages::version_data::tests::any_version_data;
    use amaru_kernel::protocol_messages::version_number::tests::any_version_number;
    use amaru_kernel::protocol_messages::version_table::tests::any_version_table;
    use proptest::prelude::*;
    use proptest::prop_compose;

    prop_cbor_roundtrip!(Message<VersionData>, any_message());

    // HELPERS
    prop_compose! {
        fn any_propose_message()(version_table in any_version_table()) -> Message<VersionData> {
            Propose(version_table)
        }
    }

    prop_compose! {
        fn any_query_reply_message()(version_table in any_version_table()) -> Message<VersionData> {
            QueryReply(version_table)
        }
    }

    prop_compose! {
        fn any_accept_message()(version_number in any_version_number(), version_data in any_version_data()) -> Message<VersionData> {
            Accept(version_number, version_data)
        }
    }

    prop_compose! {
        fn any_refuse_message()(reason in any_refuse_reason()) -> Message<VersionData> {
            Refuse(reason)
        }
    }

    pub fn any_message() -> impl Strategy<Value = Message<VersionData>> {
        prop_oneof![
            1 => any_query_reply_message(),
            1 => any_propose_message(),
            1 => any_accept_message(),
            1 => any_refuse_message(),
        ]
    }
}
