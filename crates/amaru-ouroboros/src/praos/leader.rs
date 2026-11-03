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

//! Leader election for a block producer: which slots of an epoch a pool leads,
//! and the VRF certificate it must put in each header. The mirror of the
//! per-header checks in [`super::header`].

use std::{collections::BTreeMap, ops::Range};

use amaru_kernel::{Bytes, Hash, Nonce, Slot, VrfCert, cardano::fixed_bytes::FixedBytes, maths::FixedDecimal};

use super::header::is_leader;
use crate::vrf;

/// Per-epoch inputs shared by every slot's leadership test.
pub struct LeaderParams<'a> {
    pub nonce: &'a Nonce,
    pub relative_stake: &'a FixedDecimal,
    pub active_slot_coeff: &'a FixedDecimal,
}

/// The certificate to forge with if `slot` is led under `params`.
pub fn lead_slot(slot: Slot, params: &LeaderParams<'_>, vrf: &vrf::SecretKey) -> Option<VrfCert> {
    let proof = vrf.prove(&vrf::Input::new(slot, params.nonce));
    let output = Hash::<{ vrf::Proof::HASH_SIZE }>::from(&proof);
    is_leader(params.active_slot_coeff, params.relative_stake, output.as_ref()).then(|| VrfCert {
        output: Bytes::from(output.to_vec()),
        proof: FixedBytes::from(<[u8; vrf::Proof::SIZE]>::from(&proof)),
    })
}

/// Every led slot in `slots`, with its certificate.
pub fn lead_slots(slots: Range<Slot>, params: &LeaderParams<'_>, vrf: &vrf::SecretKey) -> BTreeMap<Slot, VrfCert> {
    (u64::from(slots.start)..u64::from(slots.end))
        .map(Slot::from)
        .filter_map(|slot| lead_slot(slot, params, vrf).map(|cert| (slot, cert)))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::praos::header::{AssertLeaderStakeError, AssertVrfProofError};

    fn secret() -> vrf::SecretKey {
        vrf::SecretKey::from(&[7u8; vrf::SecretKey::SIZE])
    }

    #[test]
    fn a_led_slot_passes_header_validation() {
        let nonce = Nonce::from([1u8; 32]);
        let stake = FixedDecimal::one();
        let coeff = FixedDecimal::one();
        let params = LeaderParams { nonce: &nonce, relative_stake: stake, active_slot_coeff: coeff };
        let slot = Slot::from(42);

        let cert = lead_slot(slot, &params, &secret()).unwrap();

        AssertVrfProofError::new(slot, &nonce, &vrf::PublicKey::from(&secret()), &cert).unwrap();
        let certified =
            FixedDecimal::from(vrf::Derivation::Leader.derive_tagged_vrf_output(cert.output.as_ref()).as_slice());
        AssertLeaderStakeError::new(coeff, stake, &certified).unwrap();
    }

    #[test]
    fn fractional_stake_producer_matches_node() {
        let nonce = Nonce::from([1u8; 32]);
        let stake = FixedDecimal::one() / &FixedDecimal::from(2u64);
        let coeff = FixedDecimal::one() / &FixedDecimal::from(20u64);
        let params = LeaderParams { nonce: &nonce, relative_stake: &stake, active_slot_coeff: &coeff };
        let secret = secret();

        // First winner/loser from slots 0..4096, classified by non-integral-1.0.0.0 and Pallas 99678214.
        let winning_slot = Slot::from(10);
        let winning_natural =
            FixedDecimal::from_str("1145313217582686085615033024436761286383631096638575333239487424905087535159")
                .unwrap();
        let cert = lead_slot(winning_slot, &params, &secret).unwrap();
        let certified =
            FixedDecimal::from(vrf::Derivation::Leader.derive_tagged_vrf_output(cert.output.as_ref()).as_slice());
        assert_eq!(certified, winning_natural);
        AssertVrfProofError::new(winning_slot, &nonce, &vrf::PublicKey::from(&secret), &cert).unwrap();
        AssertLeaderStakeError::new(&coeff, &stake, &certified).unwrap();

        let losing_slot = Slot::from(0);
        let losing_natural =
            FixedDecimal::from_str("62897518700883543014451624525454963495063668423356168144576691979841450957339")
                .unwrap();
        assert!(lead_slot(losing_slot, &params, &secret).is_none());
        let proof = secret.prove(&vrf::Input::new(losing_slot, &nonce));
        let output = Hash::<{ vrf::Proof::HASH_SIZE }>::from(&proof);
        let certified =
            FixedDecimal::from(vrf::Derivation::Leader.derive_tagged_vrf_output(output.as_ref()).as_slice());
        assert_eq!(certified, losing_natural);
        assert_eq!(
            AssertLeaderStakeError::new(&coeff, &stake, &certified),
            Err(AssertLeaderStakeError::InsufficientLeaderStake)
        );
    }

    #[test]
    fn zero_stake_leads_nothing() {
        let nonce = Nonce::from([1u8; 32]);
        let stake = FixedDecimal::ZERO;
        let coeff = &FixedDecimal::from(1u64) / &FixedDecimal::from(20u64);
        let params = LeaderParams { nonce: &nonce, relative_stake: &stake, active_slot_coeff: &coeff };

        assert!(lead_slots(Slot::from(0)..Slot::from(200), &params, &secret()).is_empty());
    }

    #[test]
    fn full_coefficient_leads_every_slot_in_range() {
        let nonce = Nonce::from([1u8; 32]);
        let stake = FixedDecimal::one();
        let params = LeaderParams { nonce: &nonce, relative_stake: stake, active_slot_coeff: stake };

        let led = lead_slots(Slot::from(10)..Slot::from(15), &params, &secret());

        assert_eq!(led.keys().map(|slot| u64::from(*slot)).collect::<Vec<_>>(), vec![10, 11, 12, 13, 14]);
    }
}
