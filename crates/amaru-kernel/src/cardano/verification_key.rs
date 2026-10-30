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

use crate::cardano::fixed_bytes::FixedBytes;

const KEY_SIZE: usize = 32;

/// A raw Ed25519 verification key.
///
/// VRF verification keys are the same length but are a different kind of key; they have
/// their own type, [`crate::VrfVerificationKey`].
pub type VerificationKey = FixedBytes<KEY_SIZE>;
