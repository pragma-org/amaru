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

use amaru_ouroboros_traits::TxId;
use minicbor::encode::{Error, Write};
use minicbor::{CborLen, Decode, Decoder, Encode, Encoder};

/// Simple transaction data type for tests.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Tx {
    tx_body: String,
}

impl Tx {
    pub fn new(tx_body: impl Into<String>) -> Self {
        Self {
            tx_body: tx_body.into(),
        }
    }

    pub fn tx_id(&self) -> TxId {
        TxId::from(self.tx_body.as_str())
    }

    pub fn tx_body(&self) -> Vec<u8> {
        minicbor::to_vec(&self.tx_body).unwrap()
    }
}

impl CborLen<()> for Tx {
    fn cbor_len(&self, _ctx: &mut ()) -> usize {
        self.tx_body.len()
    }
}

impl Encode<()> for Tx {
    fn encode<W: Write>(&self, e: &mut Encoder<W>, _ctx: &mut ()) -> Result<(), Error<W::Error>> {
        e.encode(&self.tx_body)?;
        Ok(())
    }
}

impl<'a> Decode<'a, ()> for Tx {
    fn decode(d: &mut Decoder<'a>, _ctx: &mut ()) -> Result<Self, minicbor::decode::Error> {
        let tx_body: String = d.decode()?;
        Ok(Tx { tx_body })
    }
}
