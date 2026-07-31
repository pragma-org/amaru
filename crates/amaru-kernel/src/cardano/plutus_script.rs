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

use crate::{Bytes, ToBytes, cbor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode, cbor::Decode)]
#[cbor(transparent)]
pub struct PlutusScript<const VERSION: usize>(#[n(0)] pub Bytes);

/// Unwrap the CBOR bytestring envelope to get the raw flat-encoded program bytes.
impl<const V: usize> ToBytes for PlutusScript<V> {
    fn to_bytes(&self) -> Result<&[u8], cbor::decode::Error> {
        cbor::Decoder::new(&self.0).bytes()
    }
}

impl<const VERSION: usize> AsRef<[u8]> for PlutusScript<VERSION> {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}
