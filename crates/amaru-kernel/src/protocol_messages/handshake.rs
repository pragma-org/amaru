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

use crate::protocol_messages::{
    version_data::VersionData, version_number::VersionNumber, version_table::VersionTable,
};
use minicbor::{Decode, Decoder, Encode, Encoder, decode, encode};

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum HandshakeResult {
    Accepted(VersionNumber, VersionData),
    Refused(RefuseReason),
    Query(VersionTable<VersionData>),
}

#[derive(Debug, PartialEq, Eq, Clone, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum RefuseReason {
    VersionMismatch(Vec<VersionNumber>),
    HandshakeDecodeError(VersionNumber, String),
    Refused(VersionNumber, String),
}

impl Encode<()> for RefuseReason {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), encode::Error<W::Error>> {
        match self {
            RefuseReason::VersionMismatch(versions) => {
                e.array(2)?;
                e.u16(0)?;
                e.array(versions.len() as u64)?;
                for v in versions.iter() {
                    e.encode(v)?;
                }

                Ok(())
            }
            RefuseReason::HandshakeDecodeError(version, msg) => {
                e.array(3)?;
                e.u16(1)?;
                e.encode(version)?;
                e.str(msg)?;

                Ok(())
            }
            RefuseReason::Refused(version, msg) => {
                e.array(3)?;
                e.u16(2)?;
                e.encode(version)?;
                e.str(msg)?;

                Ok(())
            }
        }
    }
}

impl<'b> Decode<'b, ()> for RefuseReason {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut ()) -> Result<Self, decode::Error> {
        let len = d.array()?;

        match d.u16()? {
            0 => {
                check_length(0, len, 2)?;
                let versions = d.array_iter::<VersionNumber>()?;
                let versions = versions.collect::<Result<_, _>>()?;
                Ok(RefuseReason::VersionMismatch(versions))
            }
            1 => {
                check_length(1, len, 3)?;
                let version = d.decode()?;
                let msg = d.str()?;

                Ok(RefuseReason::HandshakeDecodeError(version, msg.to_string()))
            }
            2 => {
                check_length(2, len, 3)?;
                let version = d.decode()?;
                let msg = d.str()?;

                Ok(RefuseReason::Refused(version, msg.to_string()))
            }
            _ => Err(decode::Error::message("unknown variant for refusereason")),
        }
    }
}

/// This function checks that the actual length of a CBOR array matches the expected length for
/// a message variant with a given label.
pub fn check_length(label: usize, actual: Option<u64>, expected: u64) -> Result<(), decode::Error> {
    if actual != Some(expected) {
        Err(decode::Error::message(format!(
            "expected array length {expected} for label {label}, got: {actual:?}"
        )))
    } else {
        Ok(())
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::*;
    use crate::prop_cbor_roundtrip;
    use crate::protocol_messages::version_number::tests::any_version_number;
    use proptest::prelude::*;
    use proptest::prop_compose;

    prop_cbor_roundtrip!(RefuseReason, any_refuse_reason());

    prop_compose! {
        pub fn any_handshake_decode_error_reason()(version_number in any_version_number(), message in any::<String>()) -> RefuseReason {
            RefuseReason::HandshakeDecodeError(version_number, message)
        }
    }

    prop_compose! {
        pub fn any_refused_reason()(version_number in any_version_number(), message in any::<String>()) -> RefuseReason {
            RefuseReason::Refused(version_number, message)
        }
    }

    prop_compose! {
        pub fn any_version_mismatch_reason()(versions in proptest::collection::vec(any_version_number(), 1..3)) -> RefuseReason {
            RefuseReason::VersionMismatch(versions)
        }
    }

    pub fn any_refuse_reason() -> impl Strategy<Value = RefuseReason> {
        prop_oneof![
            1 => any_version_mismatch_reason(),
            1 => any_handshake_decode_error_reason(),
            1 => any_refused_reason(),
        ]
    }

    prop_compose! {
        pub fn any_byron_prefix()(b1 in any::<u8>(), b2 in any::<u64>()) -> (u8, u64) {
            (b1, b2)
        }
    }
}
