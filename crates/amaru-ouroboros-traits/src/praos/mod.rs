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

/*
Provides an interface around Praos nonces & operational certificates. Praos maintains an ever
evolving nonce to which every stake pool contributes when producing blocks using their VRF.

Hence, the VRF output of each block is combined with a rolling nonce. Once a certain point
within the epoch is reached, the nonce is anchored for the epoch. Nevertheless, the evolving
nonce keeps evolving until the epoch ends for the next epoch.

Summarizing:

```
             ┌ retain last block's                      ┌ compute [e+1] nonce from:
             │ header hash                   Randomness │   - [e-1] last block's ancestor header hash
             │                             Stabilization│   - [e] fixed candidate nonce
             │                                 Window   │
─────────────╵┼────────────⛶──────────────╷─ ─ ─ ─ ─ ─ ─┼───── ─ ─ ─ ─ ─ ─ ─
[e-1]          [e]         │              │              [e+1]
                           │              │
                           │              └─ candidate nonce for [e] is now fixed.
                           │
                           │
                           │          ┌─ 🔎 ────────────────────┐
                           └─────────>│   ┏━━━┓  ┏━━━┓  ┏━━━┓   │
                                      │..━┫ η ┣━━┫ η ┣━━┫ η ┣━..│
                                      │   ┗━╷━┛  ┗━╷━┛  ┗━╷━┛   │
                                      │...──┴─ <> ─┴─ <> ─┴──...│
                                      │                         │
                                      │ evolve nonce after each │
                                      │   block combining VRFs  │
                                      └─────────────────────────┘
*/

use crate::is_header::IsHeader;
use amaru_kernel::{Hash, Nonce, cbor, protocol_parameters::GlobalParameters};
use amaru_slot_arithmetic::Epoch;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, serde::Serialize, serde::Deserialize)]
pub struct Nonces {
    pub active: Nonce,
    pub evolving: Nonce,
    pub candidate: Nonce,
    pub tail: Hash<32>,
    pub epoch: Epoch,
}

impl<C> cbor::encode::Encode<C> for Nonces {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.begin_array()?;
        e.encode_with(self.active, ctx)?;
        e.encode_with(self.evolving, ctx)?;
        e.encode_with(self.candidate, ctx)?;
        e.encode_with(self.tail, ctx)?;
        e.encode_with(self.epoch, ctx)?;
        e.end()?;
        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for Nonces {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        Ok(Nonces {
            active: d.decode_with(ctx)?,
            evolving: d.decode_with(ctx)?,
            candidate: d.decode_with(ctx)?,
            tail: d.decode_with(ctx)?,
            epoch: d.decode_with(ctx)?,
        })
    }
}

pub trait Praos<H: IsHeader>: Send + Sync {
    type Error;

    /// Obtain a previously calculated nonce from a header ancestor. This API is meant to be
    /// concurrent-safe since we may need to keep track of multiple nonces at once from different
    /// chains.
    ///
    /// So, nonces aren't bound to epochs, but to headers.
    fn get_nonce(&self, header: &Hash<32>) -> Option<Nonce>;

    /// Evolve the given nonce by combining it in an arbitrary way with other data. When
    /// `within_stability_window` is false, this also modifies the candidate nonce for the next
    /// epoch.
    ///
    /// Once the stability window has been reached, the candidate is fixed for the epoch and will
    /// be used once crossing the epoch boundary to produce the next epoch nonce.
    fn evolve_nonce(
        &self,
        header: &H,
        global_parameters: &GlobalParameters,
    ) -> Result<Nonces, Self::Error>;
}
