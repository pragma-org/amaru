// Copyright 2026 PRAGMA
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

use amaru_kernel::{ProtocolVersion, cbor};
use amaru_minicbor_extra::{from_cbor_no_leftovers, from_cbor_no_leftovers_with, to_cbor, to_cbor_with};

/// Decode a sample and re-encode it, yielding the re-encoded bytes.
pub type RoundTrip = fn(&[u8], ProtocolVersion) -> Result<Vec<u8>, cbor::decode::Error>;

pub fn round_trip<T>(bytes: &[u8], version: ProtocolVersion) -> Result<Vec<u8>, cbor::decode::Error>
where
    T: for<'b> cbor::Decode<'b, ProtocolVersion> + cbor::Encode<ProtocolVersion>,
{
    let mut version = version;
    let value: T = from_cbor_no_leftovers_with(bytes, &mut version)?;
    Ok(to_cbor_with(&value, &mut version))
}

/// Decode a sample and re-encode it, for a type whose codec takes no context.
pub fn round_trip_without_context<T>(bytes: &[u8], _version: ProtocolVersion) -> Result<Vec<u8>, cbor::decode::Error>
where
    T: for<'b> cbor::Decode<'b, ()> + cbor::Encode<()>,
{
    let value: T = from_cbor_no_leftovers(bytes)?;
    Ok(to_cbor(&value))
}
