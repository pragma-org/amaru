use amaru_kernel::{Certificate, HasKeyHash, Hash, StakeAddress, StakePayload, Voter};

pub trait RequiresVkeyWitness {
    /// `RequiresVkeyWitness for T` returns the key hash of the vkey witness that is required for transaction validation
    fn requires_vkey_witness(&self) -> Option<Hash<28>>;
}

impl RequiresVkeyWitness for StakeAddress {
    fn requires_vkey_witness(&self) -> Option<Hash<28>> {
        match self.payload() {
            StakePayload::Stake(hash) => Some(*hash),
            StakePayload::Script(_) => None,
        }
    }
}

impl RequiresVkeyWitness for Voter {
    fn requires_vkey_witness(&self) -> Option<Hash<28>> {
        match self {
            Voter::ConstitutionalCommitteeKey(hash)
            | Voter::DRepKey(hash)
            | Voter::StakePoolKey(hash) => Some(*hash),
            Voter::ConstitutionalCommitteeScript(_) | Voter::DRepScript(_) => None,
        }
    }
}

impl RequiresVkeyWitness for Certificate {
    fn requires_vkey_witness(&self) -> Option<Hash<28>> {
        match self {
            Certificate::StakeRegistration(_) => None,
            Certificate::PoolRegistration {
                operator: pool_id,
                vrf_keyhash: _,
                pledge: _,
                cost: _,
                margin: _,
                reward_account: _,
                pool_owners: _,
                relays: _,
                pool_metadata: _,
            }
            | Certificate::PoolRetirement(pool_id, _) => Some(*pool_id),
            Certificate::Reg(stake_credential, coin) => {
                // The "old behavior of not requiring a witness for staking credential registration"
                //  is mantained:
                // - Only during the "transitional period of Conway"
                // - Only for staking credential registration certificates without a deposit
                // (see https://github.com/IntersectMBO/cardano-ledger/blob/81637a1c2250225fef47399dd56f80d87384df32/eras/conway/impl/src/Cardano/Ledger/Conway/TxCert.hs#L698)
                if coin == &0 {
                    None
                } else {
                    stake_credential.key_hash()
                }
            }
            Certificate::StakeDeregistration(stake_credential)
            | Certificate::StakeDelegation(stake_credential, _)
            | Certificate::UnReg(stake_credential, _)
            | Certificate::VoteDeleg(stake_credential, _)
            | Certificate::StakeVoteDeleg(stake_credential, _, _)
            | Certificate::StakeRegDeleg(stake_credential, _, _)
            | Certificate::VoteRegDeleg(stake_credential, _, _)
            | Certificate::StakeVoteRegDeleg(stake_credential, _, _, _)
            | Certificate::AuthCommitteeHot(stake_credential, _)
            | Certificate::ResignCommitteeCold(stake_credential, _)
            | Certificate::RegDRepCert(stake_credential, _, _)
            | Certificate::UnRegDRepCert(stake_credential, _)
            | Certificate::UpdateDRepCert(stake_credential, _) => stake_credential.key_hash(),
        }
    }
}
