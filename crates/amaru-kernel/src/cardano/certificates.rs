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

use minicbor::{Decoder, decode::Error};

use crate::{Certificate, NonEmptyUniqueVec, cbor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Certificates(NonEmptyUniqueVec<Certificate>);

impl AsRef<[Certificate]> for Certificates {
    fn as_ref(&self) -> &[Certificate] {
        self.0.as_ref()
    }
}

impl IntoIterator for Certificates {
    type Item = Certificate;
    type IntoIter = std::vec::IntoIter<Certificate>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for Certificates {
    fn decode(d: &mut Decoder<'b>, ctx: &mut C) -> Result<Self, Error> {
        let certificates = NonEmptyUniqueVec::<Certificate>::decode(d, ctx)?;
        Ok(Self(certificates))
    }
}

impl<C: cbor::HasProtocolVersion> cbor::Encode<C> for Certificates {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        self.0.encode(e, ctx)
    }
}

impl Certificates {
    pub fn iter(&self) -> std::slice::Iter<'_, Certificate> {
        self.0.iter()
    }
}
