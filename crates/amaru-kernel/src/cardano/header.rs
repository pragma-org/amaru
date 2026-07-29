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

use crate::{Bytes, HeaderBody, cbor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode, cbor::Decode)]
pub struct Header {
    #[n(0)]
    pub header_body: HeaderBody,

    #[n(1)]
    pub body_signature: Bytes,
}

impl Header {
    pub fn vrf_output(&self) -> &[u8] {
        &self.header_body.vrf_result.output
    }

    pub fn vrf_proof(&self) -> &[u8] {
        &self.header_body.vrf_result.proof
    }
}
