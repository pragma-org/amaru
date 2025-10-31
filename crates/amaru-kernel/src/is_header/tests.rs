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

use super::*;
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::{Config, RngSeed, TestRunner};

/// Make a mostly empty Header with the given block_number, slot and previous hash
pub fn make_header(block_number: u64, slot: u64, prev_hash: Option<HeaderHash>) -> Header {
    use crate::Bytes;
    use pallas_primitives::VrfCert;
    use pallas_primitives::babbage::PseudoHeader;
    use pallas_primitives::conway::OperationalCert;

    PseudoHeader {
        header_body: HeaderBody {
            block_number,
            slot,
            prev_hash,
            issuer_vkey: Bytes::from(vec![]),
            vrf_vkey: Bytes::from(vec![]),
            vrf_result: VrfCert(Bytes::from(vec![]), Bytes::from(vec![])),
            block_body_size: 0,
            block_body_hash: HeaderHash::from([0u8; 32]),
            operational_cert: OperationalCert {
                operational_cert_hot_vkey: Bytes::from(vec![]),
                operational_cert_sequence_number: 0,
                operational_cert_kes_period: 0,
                operational_cert_sigma: Bytes::from(vec![]),
            },
            protocol_version: (1, 2),
        },
        body_signature: Bytes::from(vec![]),
    }
}

/// Create a list of arbitrary headers starting from a root, and where chain[i] is the parent of chain[i+1]
pub fn any_headers_chain(n: usize) -> impl Strategy<Value = Vec<BlockHeader>> {
    prop::collection::vec(any_header(), n).prop_map(make_headers())
}

/// Given a list of headers, set their block_number, slot and parent fields to form a valid chain
fn make_headers() -> impl Fn(Vec<BlockHeader>) -> Vec<BlockHeader> {
    |headers| {
        let mut parent = None;
        headers
            .into_iter()
            .enumerate()
            .map({
                |(i, h)| {
                    let mut header = h.header().clone();
                    // NOTE: by convention, chain numbering starts at 1. There can't be a block 0
                    // nor a block forged at slot 0
                    header.header_body.block_number = (i + 1) as u64;
                    header.header_body.slot = (i + 1) as u64;
                    header.header_body.prev_hash = parent;
                    let block_header = BlockHeader::from(header);
                    parent = Some(block_header.hash());
                    block_header
                }
            })
            .collect()
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
            BlockHeader::from(make_header(block_number, slot, prev_hash))
        })
}

/// Create an arbitrary BlockHeader, with an arbitrary parent
pub fn any_header_with_parent(parent: HeaderHash) -> impl Strategy<Value = BlockHeader> {
    (0u64..=1_000_000, 0u64..=1_000_000).prop_map(move |(block_number, slot)| {
        BlockHeader::from(make_header(block_number, slot, Some(parent)))
    })
}

/// Create an arbitrary BlockHeader, with an arbitrary parent that is guaranteed to be Some
pub fn any_header_with_some_parent() -> impl Strategy<Value = BlockHeader> {
    any_header().prop_flat_map(|h| any_header_with_parent(h.hash()))
}

/// Create an arbitrary header hash with the right number of bytes
pub fn any_header_hash() -> impl Strategy<Value = HeaderHash> {
    any::<[u8; HEADER_HASH_SIZE]>().prop_map(HeaderHash::from)
}

/// Create an arbitrary FakeHeader
pub fn any_fake_header() -> impl Strategy<Value = BlockHeader> {
    (
        0u64..=1_000_000,
        0u64..=1_000_000,
        prop::option::weighted(0.01, any_header_hash()),
    )
        .prop_map(|(block_number, slot, parent)| {
            let header = make_header(block_number, slot, parent);
            BlockHeader::from(header)
        })
}

/// Run a strategy and return the generated value, panicking if generation fails.
#[expect(clippy::unwrap_used)]
pub fn run<T>(s: impl Strategy<Value = T>) -> T {
    let mut runner = TestRunner::default();
    s.new_tree(&mut runner).unwrap().current()
}

/// Run a strategy with a seed provided by a random generator
/// and return the generated value, panicking if generation fails.
#[expect(clippy::unwrap_used)]
pub fn run_with_rng<T, RNG: Rng>(rng: &mut RNG, s: impl Strategy<Value = T>) -> T {
    let config = Config {
        rng_seed: RngSeed::Fixed(rng.random()),
        ..Default::default()
    };
    let mut runner = TestRunner::new(config);
    s.new_tree(&mut runner).unwrap().current()
}
