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

use crate::{Point, protocol_messages::block_height::BlockHeight};
use minicbor::{Decode, Decoder, Encode, Encoder, decode, encode};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct Tip(pub Point, pub BlockHeight);

impl Encode<()> for Tip {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), encode::Error<W::Error>> {
        e.array(2)?;
        e.encode(&self.0)?;
        e.encode(&self.1)?;

        Ok(())
    }
}

impl<'b> Decode<'b, ()> for Tip {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut ()) -> Result<Self, decode::Error> {
        d.array()?;
        let point = d.decode()?;
        let block_num = d.decode()?;

        Ok(Tip(point, block_num))
    }
}
