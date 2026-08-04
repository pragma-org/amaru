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
```
*/
use amaru_kernel::{HeaderHash, IsHeader, Nonce};

use crate::Nonces;

pub trait Praos<H: IsHeader>: Send + Sync {
    type Error;

    /// Obtain a previously calculated nonce from a header ancestor. This API is meant to be
    /// concurrent-safe since we may need to keep track of multiple nonces at once from different
    /// chains.
    ///
    /// So, nonces aren't bound to epochs, but to headers.
    fn get_nonce(&self, header: &HeaderHash) -> Option<Nonce>;

    /// Evolve the given nonce by combining it in an arbitrary way with other data.
    fn evolve_nonce(&self, header: &H) -> Result<Nonces, Self::Error>;
}
