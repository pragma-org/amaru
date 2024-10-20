use ouroboros::ledger::{PoolId, PoolSigma};
use pallas_crypto::hash::Hash;
use std::collections::HashMap;

pub fn from_csv(csv: &str) -> HashMap<PoolId, PoolSigma> {
    let mut pool_id_to_sigma = HashMap::new();

    for line in csv.lines() {
        let mut parts = line.split(',');
        let pool_id: Hash<28> = parts.next().unwrap().parse().unwrap();
        let numerator: u64 = parts.next().unwrap().parse().unwrap();
        let denominator: u64 = parts.next().unwrap().parse().unwrap();
        pool_id_to_sigma.insert(
            pool_id,
            PoolSigma {
                numerator,
                denominator,
            },
        );
    }

    pool_id_to_sigma
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprod_172() {
        let stake_distribution = from_csv(include_str!(
            "../../data/preprod/stake_distribution/172.csv"
        ));

        let hit = |pool_id, numerator, denominator| {
            assert_eq!(
                stake_distribution.get(&hex::decode(pool_id).unwrap().as_slice().into()),
                Some(&PoolSigma {
                    numerator,
                    denominator
                })
            );
        };

        assert_eq!(stake_distribution.len(), 368);
        hit(
            "097ed33e2485fe2955bfbffa66d0b3e58909363bfb707c7194ee4e5f",
            1199079694297,
            336456263477551,
        );
        hit(
            "7df614e44ae175d5c13aa9cd7c9e4caf9e03027a9562a1114f0762a5",
            0,
            336456263477551,
        );
    }
}
