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

use amaru_kernel::{Epoch, EraHistory, EraHistoryError, Hasher, HeaderHash, IsHeader, Nonce, ORIGIN_HASH};
use amaru_ouroboros_traits::Nonces;

use crate::vrf;

/// Compute the nonces of a header from the nonces of its parent.
pub fn evolve_nonces<H: IsHeader>(
    header: &H,
    parent_nonces: &Nonces,
    epoch: Epoch,
    is_within_stability_window: bool,
    previous_epoch_tail_parent_hash: Option<HeaderHash>,
) -> Nonces {
    // Compute the new evolving nonce by combining it with the current one and the header's VRF
    // output.
    let evolving = evolve(header, &parent_nonces.evolving);

    // Unless we are within the randomness stability window, we also update the candidate. This
    // means that outside of the stability window, we always have:
    //
    //   evolving == candidate
    //
    // They only diverge for the last blocks of each epoch; The candidate remains stable while
    // the rolling nonce keeps evolving in preparation of the next epoch. Another way to look
    // at it is to think that there's always an entire epoch length contributing to the nonce
    // randomness, but it spans over two epochs.
    let candidate = if is_within_stability_window { evolving } else { parent_nonces.candidate };

    // The active nonce is either:
    //  1. the active parent nonce if there's no epoch change.
    //  2. the combination of the stable candidate and the previous epoch's last block's parent header hash.
    //
    // The tail is either
    //  1. the parent_nonce tail if there is no epoch change.
    //  2. the parent hash of the current header (the last header hash of the previous epoch).
    //
    let (active, tail) = match previous_epoch_tail_parent_hash {
        Some(previous_epoch_tail_parent_hash) => {
            (parent_nonces.next_active(previous_epoch_tail_parent_hash), header.parent().unwrap_or(ORIGIN_HASH))
        }
        None => (parent_nonces.active, parent_nonces.tail),
    };

    Nonces { epoch, evolving, candidate, active, tail }
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
            &Hasher::<256>::hash(&vrf::Derivation::Nonce.derive_tagged_vrf_output(header.vrf_output()))[..],
        ]
        .concat(),
    )
}

/// Determines if a header is within the randomness stability window of its epoch.
///
/// Returns the header's epoch and a boolean indicating whether the header is within
/// the stability window (i.e., far enough from the epoch boundary).
pub fn randomness_stability_window<H: IsHeader>(
    header: &H,
    era_history: &EraHistory,
    randomness_stabilization_window: u64,
) -> Result<(Epoch, bool), EraHistoryError> {
    let slot = header.slot();
    let tip = slot;
    let epoch = era_history.slot_to_epoch(tip, tip)?;

    let next_epoch_first_slot = era_history.next_epoch_first_slot(epoch, &tip)?;

    let is_within_stability_window = slot.as_u64() + randomness_stabilization_window < next_epoch_first_slot.as_u64();

    Ok((epoch, is_within_stability_window))
}
