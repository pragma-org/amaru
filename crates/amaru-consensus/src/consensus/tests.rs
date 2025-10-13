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

use amaru_kernel::{Bytes, HeaderBody, PseudoHeader};
use amaru_ouroboros::{OperationalCert, VrfCert};
use amaru_ouroboros_traits::fake::tests::any_header_hash;
use amaru_ouroboros_traits::{BlockHeader, IsHeader};
use pallas_crypto::hash::Hash;
use proptest::prelude::{Strategy, prop};

/// Create a list of arbitrary headers starting from a root, and where chain[i] is the parent of chain[i+1]
pub fn any_headers_chain(n: usize) -> impl Strategy<Value = Vec<BlockHeader>> {
    prop::collection::vec(any_header(), n).prop_map(make_header())
}

/// Given a list of headers, set their block_number, slot and parent fields to form a valid chain
fn make_header() -> impl Fn(Vec<BlockHeader>) -> Vec<BlockHeader> {
    |mut headers| {
        let mut parent = None;
        for (i, h) in headers.iter_mut().enumerate() {
            h.header_mut().header_body.block_number = i as u64;
            h.header_mut().header_body.slot = i as u64;
            h.header_mut().header_body.prev_hash = parent;
            parent = Some(h.hash())
        }
        headers
    }
}

/// Create an arbitrary BlockHeader, with an arbitrary parent, possibly set to None
pub fn any_header() -> impl Strategy<Value = BlockHeader> {
    (
        0u64..=1_000_000,
        0u64..=1_000_000,
        prop::option::weighted(0.01, any_header_hash()),
    )
        .prop_map(|(block_number, slot, prev_hash)| {
            BlockHeader::from(PseudoHeader {
                header_body: HeaderBody {
                    block_number,
                    slot,
                    prev_hash,
                    issuer_vkey: Bytes::from(vec![]),
                    vrf_vkey: Bytes::from(vec![]),
                    vrf_result: VrfCert(Bytes::from(vec![]), Bytes::from(vec![])),
                    block_body_size: 0,
                    block_body_hash: Hash::<32>::from([0u8; 32]),
                    operational_cert: OperationalCert {
                        operational_cert_hot_vkey: Bytes::from(vec![]),
                        operational_cert_sequence_number: 0,
                        operational_cert_kes_period: 0,
                        operational_cert_sigma: Bytes::from(vec![]),
                    },
                    protocol_version: (1, 2),
                },
                body_signature: Bytes::from(vec![]),
            })
        })
}
