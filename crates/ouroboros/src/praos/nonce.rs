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

use amaru_kernel::{
    network::EraHistory, next_epoch_first_slot, Epoch, Hasher, Nonce,
    RANDOMNESS_STABILIZATION_WINDOW,
};
use amaru_ouroboros_traits::IsHeader;
use slot_arithmetic::TimeHorizonError;

/// Obtain the final nonce at an epoch boundary for the epoch from the stable candidate and the
/// last block (header) of the previous epoch.
///
/// Return `None` if header has no parent (i.e. which never happens because all our blocks have
/// parents in Amaru).
pub fn from_candidate<H: IsHeader>(header: &H, candidate: &Nonce) -> Option<Nonce> {
    Some(Hasher::<256>::hash(
        &[&candidate[..], &header.parent()?[..]].concat(),
    ))
}

/// Evolve the current nonce by combining it with the current rolling nonce and the
/// range-extended tagged leader VRF output.
///
/// Specifically, we combine it with `η` (a.k.a eta), which is a blake2b-256 hash of the
/// tagged leader VRF output after a range extension. The range extension is, yet another
/// blake2b-256 hash.
pub fn evolve<H: IsHeader>(header: &H, current: &Nonce) -> Nonce {
    Hasher::<256>::hash(
        &[
            &current[..],
            &Hasher::<256>::hash(header.extended_vrf_nonce_output().as_slice())[..],
        ]
        .concat(),
    )
}

/// Check whether a header is within the stability of the current epoch, and return its epoch for
/// convenience.
pub fn randomness_stability_window<H: IsHeader>(
    header: &H,
    era_history: &EraHistory,
) -> Result<(Epoch, bool), TimeHorizonError> {
    let epoch = era_history.slot_to_epoch(header.slot())?;

    let is_within_stability_window =
        header.slot() + RANDOMNESS_STABILIZATION_WINDOW >= next_epoch_first_slot(epoch);
    Ok((epoch, !is_within_stability_window))
}
