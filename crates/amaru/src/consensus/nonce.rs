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

use pallas_crypto::hash::Hash;
use pallas_primitives::conway::Epoch;

use std::collections::HashMap;

// TODO: Move to pallas_primitives
pub type Nonce = Hash<32>;

pub fn from_csv(csv: &str) -> HashMap<Epoch, Nonce> {
    let mut epoch_to_nonce = HashMap::new();
    for line in csv.lines() {
        let mut parts = line.split(',');
        let epoch: u64 = parts.next().unwrap().parse().unwrap();
        let nonce: Hash<32> = parts.next().unwrap().parse().unwrap();
        epoch_to_nonce.insert(epoch, nonce);
    }
    epoch_to_nonce
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprod_4_174() {
        let nonces = from_csv(include_str!("../../../../data/preprod/nonces.csv"));

        let hit = |epoch, hash| {
            assert_eq!(
                nonces.get(&epoch).map(|h| h.as_slice()),
                Some(hex::decode(hash).unwrap().as_slice())
            );
        };

        assert_eq!(nonces.len(), 171);
        assert_eq!(nonces.get(&3), None);
        hit(
            4,
            "162d29c4e1cf6b8a84f2d692e67a3ac6bc7851bc3e6e4afe64d15778bed8bd86",
        );
        hit(
            174,
            "39fdfcb9de873d4937930473359383194f9fd1e0ecc49aae21e9ac7f3e80a7b4",
        );
        assert_eq!(nonces.get(&175), None);
    }
}
