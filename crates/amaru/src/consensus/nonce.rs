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
