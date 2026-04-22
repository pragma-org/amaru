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

use std::ops::Deref;

pub use pallas_primitives::conway::PlutusData;

use crate::{Bytes, MemoizedPlutusData, NonEmptyVec, cbor, empty_bytes};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct PlutusDataSet {
    #[serde(skip, default = "empty_bytes")]
    original_bytes: Bytes,
    inner: NonEmptyVec<MemoizedPlutusData>,
}

impl<C> cbor::Encode<C> for PlutusDataSet {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        self.inner.encode(e, ctx)
    }
}

impl<'b, C> cbor::Decode<'b, C> for PlutusDataSet {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let (inner, bytes) = cbor::tee(d, |d| NonEmptyVec::<MemoizedPlutusData>::decode(d, ctx))?;
        Ok(Self { original_bytes: Bytes::from(bytes.to_vec()), inner })
    }
}

impl PlutusDataSet {
    pub fn original_bytes(&self) -> &[u8] {
        &self.original_bytes
    }
}

impl Deref for PlutusDataSet {
    type Target = NonEmptyVec<MemoizedPlutusData>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
