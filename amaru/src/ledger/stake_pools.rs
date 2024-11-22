use ouroboros::ledger::PoolId;
use pallas_crypto::hash::Hash;
use std::collections::HashMap;

pub fn from_csv(csv: &str) -> HashMap<PoolId, Hash<32>> {
    let mut pool_id_to_vrf = HashMap::new();

    for line in csv.lines() {
        let mut parts = line.split(',');
        let pool_id: Hash<28> = parts.next().unwrap().parse().unwrap();
        let vrf: Hash<32> = parts.next().unwrap().parse().unwrap();
        pool_id_to_vrf.insert(pool_id, vrf);
    }

    pool_id_to_vrf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprod_172() {
        let stake_pools = from_csv(include_str!("../../data/preprod/stake_pools/172.csv"));

        let hit = |pool_id, vrf| {
            assert_eq!(
                stake_pools.get(&hex::decode(pool_id).unwrap().as_slice().into()),
                Some(&hex::decode(vrf).unwrap().as_slice().into())
            );
        };

        assert_eq!(stake_pools.len(), 405);
        hit(
            "10caff4f6980296efccd94b91a966e4692f8fc89de2490605454935a",
            "ed6b09ed118264f29bad9af7b785a48d562998924e2d0a98e9356fcf0a5a01f1",
        );
        hit(
            "7df614e44ae175d5c13aa9cd7c9e4caf9e03027a9562a1114f0762a5",
            "74511e297e8d8670729af5a4eb08ff8b49f0247f1100f28ce5599b44f07b57b4",
        );
    }
}
