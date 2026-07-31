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

use crate::cbor;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Skip {
    pub skipped: Vec<u8>,
}

impl Deref for Skip {
    type Target = Vec<u8>;

    fn deref(&self) -> &Self::Target {
        &self.skipped
    }
}

impl<'b, C> cbor::Decode<'b, C> for Skip {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let all = d.input();
        let start = d.position();
        d.skip()?;
        let end = d.position();

        Ok(Self { skipped: Vec::from(&all[start..end]) })
    }
}

impl<C> cbor::Encode<C> for Skip {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.writer_mut().write_all(self.deref()).map_err(cbor::encode::Error::write)
    }
}
