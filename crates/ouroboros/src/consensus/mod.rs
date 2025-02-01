// Copyright 2024 PRAGMA
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

use crate::{
    kes::{KesPublicKey, KesSignature},
    ledger::{issuer_vkey_to_pool_id, LedgerState, PoolId},
    validator::{ValidationError, Validator},
    vrf::{VrfProof, VrfProofBytes, VrfProofHashBytes, VrfPublicKey, VrfPublicKeyBytes},
};
use pallas_crypto::{
    hash::{Hash, Hasher},
    key::ed25519::{PublicKey, Signature},
};
use pallas_math::math::{ExpOrdering, FixedDecimal, FixedPrecision};
use pallas_primitives::{
    babbage,
    babbage::{derive_tagged_vrf_output, VrfDerivation},
};
use rayon::prelude::*;
use std::{ops::Deref, sync::LazyLock};
use tracing::{span, trace};
pub use validator::*;

pub mod test;
pub mod validator;
