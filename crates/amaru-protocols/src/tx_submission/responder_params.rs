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

use minicbor::decode::Error;
use minicbor::{Decode, Decoder, Encode};
use serde::{Deserialize, Serialize};

/// Parameters used to control the behavior of the transaction submission responder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponderParams {
    pub max_window: u16,  // how many tx ids we keep in “window”
    pub fetch_batch: u16, // how many txs we request per round
}

const MAX_OUTSTANDING_TX_IDS: u16 = 3;
const MAX_OUTSTANDING_TXS: u16 = 2;

impl Default for ResponderParams {
    fn default() -> Self {
        Self {
            max_window: MAX_OUTSTANDING_TX_IDS,
            fetch_batch: MAX_OUTSTANDING_TXS,
        }
    }
}

impl ResponderParams {
    pub fn new(max_window: u16, fetch_batch: u16) -> Self {
        Self {
            max_window,
            fetch_batch,
        }
    }
}

/// Whether the transaction submission initiator should block until it can fulfill a server request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Blocking {
    Yes,
    No,
}

impl Encode<()> for Blocking {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        match self {
            Blocking::Yes => e.encode(true)?,
            Blocking::No => e.encode(false)?,
        };
        Ok(())
    }
}

impl<'b> Decode<'b, ()> for Blocking {
    fn decode(d: &mut Decoder<'b>, ctx: &mut ()) -> Result<Self, Error> {
        let value = bool::decode(d, ctx)?;
        match value {
            true => Ok(Blocking::Yes),
            false => Ok(Blocking::No),
        }
    }
}

impl From<Blocking> for bool {
    fn from(value: Blocking) -> Self {
        match value {
            Blocking::Yes => true,
            Blocking::No => false,
        }
    }
}
