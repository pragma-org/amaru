use crate::store::{
    columns::{accounts, dreps, pools, pots, proposals, slots, utxo},
    EpochTransitionProgress, HistoricalStores, ReadOnlyStore, Snapshot, Store, StoreError,
    TransactionalContext,
};
use amaru_kernel::{
    protocol_parameters::ProtocolParameters, Lovelace, Point, PoolId, ProposalId, Slot,
    StakeCredential, TransactionInput,
};
use slot_arithmetic::Epoch;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};

pub struct MemoryStore {
    utxos: RefCell<BTreeMap<TransactionInput, Option<utxo::Value>>>,
    accounts: RefCell<BTreeMap<StakeCredential, Option<accounts::Row>>>,
    pools: RefCell<BTreeMap<PoolId, Option<pools::Row>>>,
    pots: RefCell<pots::Row>,
    slots: RefCell<BTreeMap<Slot, Option<slots::Row>>>,
    dreps: RefCell<BTreeMap<StakeCredential, Option<dreps::Row>>>,
    proposals: RefCell<BTreeMap<ProposalId, Option<proposals::Row>>>,
    p_params: RefCell<BTreeMap<Epoch, ProtocolParameters>>,
}

impl Default for MemoryStore {
    fn default() -> Self {
        MemoryStore {
            utxos: RefCell::new(BTreeMap::new()),
            accounts: RefCell::new(BTreeMap::new()),
            pools: RefCell::new(BTreeMap::new()),
            pots: RefCell::new(pots::Row::default()),
            slots: RefCell::new(BTreeMap::new()),
            dreps: RefCell::new(BTreeMap::new()),
            proposals: RefCell::new(BTreeMap::new()),
            p_params: RefCell::new(BTreeMap::new()),
        }
    }
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Snapshot for MemoryStore {
    fn epoch(&self) -> Epoch {
        Epoch::from(10)
    }
}

impl ReadOnlyStore for MemoryStore {
    fn get_protocol_parameters_for(&self, epoch: &Epoch) -> Result<ProtocolParameters, StoreError> {
        let map = self.p_params.borrow();
        map.get(epoch).ok_or(StoreError::NotFound).cloned()
    }

    fn account(
        &self,
        credential: &amaru_kernel::StakeCredential,
    ) -> Result<Option<crate::store::columns::accounts::Row>, crate::store::StoreError> {
        Ok(self
            .accounts
            .borrow()
            .get(credential)
            .and_then(|opt| opt.clone()))
    }

    fn pool(
        &self,
        pool: &amaru_kernel::PoolId,
    ) -> Result<Option<crate::store::columns::pools::Row>, crate::store::StoreError> {
        Ok(self.pools.borrow().get(pool).and_then(|opt| opt.clone()))
    }

    fn utxo(
        &self,
        input: &amaru_kernel::TransactionInput,
    ) -> Result<Option<amaru_kernel::TransactionOutput>, crate::store::StoreError> {
        Ok(self.utxos.borrow().get(input).and_then(|opt| opt.clone()))
    }

    fn pots(&self) -> Result<crate::summary::Pots, crate::store::StoreError> {
        Ok((&*self.pots.borrow()).into())
    }

    #[allow(refining_impl_trait)]
    fn iter_utxos(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                crate::store::columns::utxo::Key,
                crate::store::columns::utxo::Value,
            ),
        >,
        crate::store::StoreError,
    > {
        let utxo_vec: Vec<_> = self
            .utxos
            .borrow()
            .iter()
            .filter_map(|(tx_input, opt_tx_output)| {
                opt_tx_output
                    .as_ref()
                    .map(|tx_output| (tx_input.clone(), tx_output.clone()))
            })
            .collect();
        Ok(utxo_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_block_issuers(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                crate::store::columns::slots::Key,
                crate::store::columns::slots::Value,
            ),
        >,
        StoreError,
    > {
        let block_issuer_vec: Vec<_> = self
            .slots
            .borrow()
            .iter()
            .filter_map(|(slot, opt_row)| opt_row.as_ref().map(|row| (*slot, row.clone())))
            .collect();

        Ok(block_issuer_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_pools(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                crate::store::columns::pools::Key,
                crate::store::columns::pools::Row,
            ),
        >,
        StoreError,
    > {
        let pool_vec: Vec<_> = self
            .pools
            .borrow()
            .iter()
            .filter_map(|(pool_id, opt_row)| opt_row.as_ref().map(|row| (*pool_id, row.clone())))
            .collect();

        Ok(pool_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_accounts(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                crate::store::columns::accounts::Key,
                crate::store::columns::accounts::Row,
            ),
        >,
        StoreError,
    > {
        let accounts_vec: Vec<_> = self
            .accounts
            .borrow()
            .iter()
            .filter_map(|(stake_credential, opt_row)| {
                opt_row
                    .as_ref()
                    .map(|row| (stake_credential.clone(), row.clone()))
            })
            .collect();

        Ok(accounts_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_dreps(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                crate::store::columns::dreps::Key,
                crate::store::columns::dreps::Row,
            ),
        >,
        StoreError,
    > {
        let dreps_vec: Vec<_> = self
            .dreps
            .borrow()
            .iter()
            .filter_map(|(stake_credential, opt_row)| {
                opt_row
                    .as_ref()
                    .map(|row| (stake_credential.clone(), row.clone()))
            })
            .collect();

        Ok(dreps_vec.into_iter())
    }

    #[allow(refining_impl_trait)]
    fn iter_proposals(
        &self,
    ) -> Result<
        impl Iterator<
            Item = (
                crate::store::columns::proposals::Key,
                crate::store::columns::proposals::Row,
            ),
        >,
        StoreError,
    > {
        let proposals_vec: Vec<_> = self
            .proposals
            .borrow()
            .iter()
            .filter_map(|(proposal_id, opt_row)| {
                opt_row
                    .as_ref()
                    .map(|row| (proposal_id.clone(), row.clone()))
            })
            .collect();

        Ok(proposals_vec.into_iter())
    }
}

pub struct MemoryTransactionalContext {}

impl<'a> TransactionalContext<'a> for MemoryTransactionalContext {
    fn commit(self) -> Result<(), StoreError> {
        Ok(())
    }

    fn rollback(self) -> Result<(), StoreError> {
        Ok(())
    }

    fn try_epoch_transition(
        &self,
        _from: Option<EpochTransitionProgress>,
        _to: Option<EpochTransitionProgress>,
    ) -> Result<bool, StoreError> {
        Ok(true)
    }

    fn refund(
        &self,
        _credential: &crate::store::columns::accounts::Key,
        _deposit: Lovelace,
    ) -> Result<Lovelace, StoreError> {
        Ok(0)
    }

    fn set_protocol_parameters(
        &self,
        _epoch: &Epoch,
        _protocol_parameters: &ProtocolParameters,
    ) -> Result<(), StoreError> {
        Ok(())
    }

    fn save(
        &self,
        _point: &Point,
        _issuer: Option<&crate::store::columns::pools::Key>,
        _add: crate::store::Columns<
            impl Iterator<
                Item = (
                    crate::store::columns::utxo::Key,
                    crate::store::columns::utxo::Value,
                ),
            >,
            impl Iterator<Item = crate::store::columns::pools::Value>,
            impl Iterator<
                Item = (
                    crate::store::columns::accounts::Key,
                    crate::store::columns::accounts::Value,
                ),
            >,
            impl Iterator<
                Item = (
                    crate::store::columns::dreps::Key,
                    crate::store::columns::dreps::Value,
                ),
            >,
            impl Iterator<
                Item = (
                    crate::store::columns::cc_members::Key,
                    crate::store::columns::cc_members::Value,
                ),
            >,
            impl Iterator<
                Item = (
                    crate::store::columns::proposals::Key,
                    crate::store::columns::proposals::Value,
                ),
            >,
        >,
        _remove: crate::store::Columns<
            impl Iterator<Item = crate::store::columns::utxo::Key>,
            impl Iterator<Item = (crate::store::columns::pools::Key, Epoch)>,
            impl Iterator<Item = crate::store::columns::accounts::Key>,
            impl Iterator<
                Item = (
                    crate::store::columns::dreps::Key,
                    amaru_kernel::CertificatePointer,
                ),
            >,
            impl Iterator<Item = crate::store::columns::cc_members::Key>,
            impl Iterator<Item = crate::store::columns::proposals::Key>,
        >,
        _withdrawals: impl Iterator<Item = crate::store::columns::accounts::Key>,
        _voting_dreps: BTreeSet<StakeCredential>,
    ) -> Result<(), crate::store::StoreError> {
        Ok(())
    }

    fn with_pots(
        &self,
        _with: impl FnMut(Box<dyn std::borrow::BorrowMut<crate::store::columns::pots::Row> + '_>),
    ) -> Result<(), crate::store::StoreError> {
        Ok(())
    }

    fn with_pools(
        &self,
        _with: impl FnMut(crate::store::columns::pools::Iter<'_, '_>),
    ) -> Result<(), crate::store::StoreError> {
        Ok(())
    }

    fn with_accounts(
        &self,
        _with: impl FnMut(crate::store::columns::accounts::Iter<'_, '_>),
    ) -> Result<(), crate::store::StoreError> {
        Ok(())
    }

    fn with_block_issuers(
        &self,
        _with: impl FnMut(crate::store::columns::slots::Iter<'_, '_>),
    ) -> Result<(), crate::store::StoreError> {
        Ok(())
    }

    fn with_utxo(
        &self,
        _with: impl FnMut(crate::store::columns::utxo::Iter<'_, '_>),
    ) -> Result<(), crate::store::StoreError> {
        Ok(())
    }

    fn with_dreps(
        &self,
        _with: impl FnMut(crate::store::columns::dreps::Iter<'_, '_>),
    ) -> Result<(), crate::store::StoreError> {
        Ok(())
    }

    fn with_proposals(
        &self,
        _with: impl FnMut(crate::store::columns::proposals::Iter<'_, '_>),
    ) -> Result<(), crate::store::StoreError> {
        Ok(())
    }
}

impl Store for MemoryStore {
    fn snapshots(&self) -> Result<Vec<Epoch>, StoreError> {
        Ok(vec![Epoch::from(3)])
    }
    fn next_snapshot(&self, _epoch: Epoch) -> Result<(), crate::store::StoreError> {
        Ok(())
    }
    fn create_transaction(&self) -> impl TransactionalContext<'_> {
        MemoryTransactionalContext {}
    }

    fn tip(&self) -> Result<Point, crate::store::StoreError> {
        Ok(Point::Origin)
    }
}

impl HistoricalStores for MemoryStore {
    fn for_epoch(&self, _epoch: Epoch) -> Result<impl Snapshot, crate::store::StoreError> {
        Ok(MemoryStore {
            utxos: RefCell::new(BTreeMap::new()),
            accounts: RefCell::new(BTreeMap::new()),
            pools: RefCell::new(BTreeMap::new()),
            pots: RefCell::new(pots::Row::default()),
            slots: RefCell::new(BTreeMap::new()),
            dreps: RefCell::new(BTreeMap::new()),
            proposals: RefCell::new(BTreeMap::new()),
            p_params: RefCell::new(BTreeMap::new()),
        })
    }
}

#[cfg(test)]
mod in_memory_tests {
    use super::*;
    use amaru_kernel::{
        protocol_parameters::{
            CostModels, DrepThresholds, PoolThresholds, Prices, ProtocolParametersThresholds,
        },
        Bytes, CertificatePointer, ExUnits, Hash, Nullable, PostAlonzoTransactionOutput,
        PseudoTransactionOutput, RationalNumber, TransactionInput, TransactionPointer, Value,
    };

    fn dummy_post_alonzo_output() -> PostAlonzoTransactionOutput {
        PostAlonzoTransactionOutput {
            address: Bytes::from(vec![0u8; 32]),
            value: Value::Coin(0),
            datum_option: None,
            script_ref: None,
        }
    }

    fn dummy_account_row() -> accounts::Row {
        accounts::Row {
            delegatee: None,
            deposit: 1000,
            drep: None,
            rewards: 500,
        }
    }

    fn dummy_account_row2() -> accounts::Row {
        accounts::Row {
            delegatee: None,
            deposit: 3000,
            drep: None,
            rewards: 200,
        }
    }

    fn dummy_pool_row() -> pools::Row {
        use amaru_kernel::{Nullable, PoolParams, Set, UnitInterval};
        let dummy_pool_params = PoolParams {
            id: Hash::new([0u8; 28]),
            vrf: Hash::new([0u8; 32]),
            pledge: 0,
            cost: 0,
            margin: UnitInterval {
                numerator: 0,
                denominator: 1,
            },
            reward_account: Bytes::from(vec![0u8; 32]),
            owners: Set::from(vec![Hash::from([0u8; 28])]),
            relays: Vec::new(),
            metadata: Nullable::Null,
        };

        pools::Row {
            current_params: dummy_pool_params,
            future_params: Vec::new(),
        }
    }

    fn dummy_pot() -> pots::Row {
        pots::Row {
            treasury: 1,
            reserves: 2,
            fees: 3,
        }
    }

    fn dummy_stake_credential() -> StakeCredential {
        StakeCredential::AddrKeyhash(Hash::new([0u8; 28]))
    }

    fn dummy_stake_credential2() -> StakeCredential {
        StakeCredential::AddrKeyhash(Hash::new([1u8; 28]))
    }

    fn dummy_drep_row() -> dreps::Row {
        dreps::Row {
            deposit: 500000000,
            anchor: None,
            registered_at: CertificatePointer {
                transaction: TransactionPointer {
                    slot: 100u64.into(),
                    transaction_index: 1,
                },
                certificate_index: 0,
            },
            last_interaction: None,
            previous_deregistration: None,
        }
    }

    fn dummy_drep_row2() -> dreps::Row {
        dreps::Row {
            deposit: 600000000,
            anchor: None,
            registered_at: CertificatePointer {
                transaction: TransactionPointer {
                    slot: 40u64.into(),
                    transaction_index: 5,
                },
                certificate_index: 1,
            },
            last_interaction: None,
            previous_deregistration: None,
        }
    }

    fn dummy_protocol_parameters1() -> ProtocolParameters {
        ProtocolParameters {
            max_block_body_size: 65536,
            max_tx_size: 16384,
            max_header_size: 512,
            max_tx_ex_units: ExUnits {
                mem: 1_000_000,
                steps: 1_000_000_000,
            },
            max_block_ex_units: ExUnits {
                mem: 50_000_000,
                steps: 5_000_000_000,
            },
            max_val_size: 10000,
            max_collateral_inputs: 3,

            // Economic group
            min_fee_a: 44u64,
            min_fee_b: 155381u64,
            stake_credential_deposit: 2000000u64,
            stake_pool_deposit: 500000000u64,
            monetary_expansion_rate: RationalNumber {
                numerator: 1u64,
                denominator: 100u64,
            },
            treasury_expansion_rate: RationalNumber {
                numerator: 1u64,
                denominator: 1000u64,
            },
            coins_per_utxo_byte: 4310u64,
            prices: Prices {
                mem: RationalNumber {
                    numerator: 1u64,
                    denominator: 10u64,
                },
                step: RationalNumber {
                    numerator: 1u64,
                    denominator: 100u64,
                },
            },
            min_fee_ref_script_coins_per_byte: RationalNumber {
                numerator: 1u64,
                denominator: 1000u64,
            },
            max_ref_script_size_per_tx: 10000,
            max_ref_script_size_per_block: 50000,
            ref_script_cost_stride: 500,
            ref_script_cost_multiplier: RationalNumber {
                numerator: 1u64,
                denominator: 1u64,
            },

            // Technical group
            max_epoch: 500u32,
            optimal_stake_pools_count: 500,
            pledge_influence: RationalNumber {
                numerator: 3u64,
                denominator: 10u64,
            },
            collateral_percentage: 150,
            cost_models: CostModels {
                plutus_v1: vec![1; 10],
                plutus_v2: vec![2; 10],
                plutus_v3: vec![3; 10],
            },

            // Governance group
            pool_thresholds: PoolThresholds {
                no_confidence: RationalNumber {
                    numerator: 1,
                    denominator: 5,
                },
                committee: RationalNumber {
                    numerator: 1,
                    denominator: 3,
                },
                committee_under_no_confidence: RationalNumber {
                    numerator: 2,
                    denominator: 5,
                },
                hard_fork: RationalNumber {
                    numerator: 3,
                    denominator: 4,
                },
                security_group: RationalNumber {
                    numerator: 1,
                    denominator: 2,
                },
            },
            drep_thresholds: DrepThresholds {
                no_confidence: RationalNumber {
                    numerator: 1,
                    denominator: 5,
                }, // 20%
                committee: RationalNumber {
                    numerator: 1,
                    denominator: 3,
                }, // 33%
                committee_under_no_confidence: RationalNumber {
                    numerator: 2,
                    denominator: 5,
                }, // 40%
                constitution: RationalNumber {
                    numerator: 3,
                    denominator: 4,
                }, // 75%
                hard_fork: RationalNumber {
                    numerator: 1,
                    denominator: 2,
                }, // 50%
                protocol_parameters: ProtocolParametersThresholds {
                    network_group: RationalNumber {
                        numerator: 1,
                        denominator: 5,
                    }, // 20%
                    economic_group: RationalNumber {
                        numerator: 1,
                        denominator: 4,
                    }, // 25%
                    technical_group: RationalNumber {
                        numerator: 1,
                        denominator: 3,
                    }, // 33%
                    governance_group: RationalNumber {
                        numerator: 1,
                        denominator: 2,
                    }, // 50%
                },
                treasury_withdrawal: RationalNumber {
                    numerator: 1,
                    denominator: 10,
                }, // 10%
            },
            cc_min_size: 3,
            cc_max_term_length: 20u32,
            gov_action_lifetime: 10u32,
            gov_action_deposit: 10000000u64,
            drep_deposit: 2000000u64,
            drep_expiry: 50u32,
        }
    }

    fn dummy_protocol_parameters2() -> ProtocolParameters {
        ProtocolParameters {
            max_block_body_size: 65546,
            max_tx_size: 16394,
            max_header_size: 522,
            max_tx_ex_units: ExUnits {
                mem: 1_000_010,
                steps: 1_000_000_010,
            },
            max_block_ex_units: ExUnits {
                mem: 50_000_010,
                steps: 5_000_000_010,
            },
            max_val_size: 10010,
            max_collateral_inputs: 13,

            min_fee_a: 54u64,
            min_fee_b: 155391u64,
            stake_credential_deposit: 2_000_010u64,
            stake_pool_deposit: 500_000_010u64,
            monetary_expansion_rate: RationalNumber {
                numerator: 11u64,
                denominator: 110u64,
            },
            treasury_expansion_rate: RationalNumber {
                numerator: 11u64,
                denominator: 1010u64,
            },
            coins_per_utxo_byte: 4320u64,
            prices: Prices {
                mem: RationalNumber {
                    numerator: 11u64,
                    denominator: 20u64,
                },
                step: RationalNumber {
                    numerator: 11u64,
                    denominator: 110u64,
                },
            },
            min_fee_ref_script_coins_per_byte: RationalNumber {
                numerator: 11u64,
                denominator: 1010u64,
            },
            max_ref_script_size_per_tx: 10010,
            max_ref_script_size_per_block: 50010,
            ref_script_cost_stride: 510,
            ref_script_cost_multiplier: RationalNumber {
                numerator: 11u64,
                denominator: 11u64,
            },

            max_epoch: 510u32,
            optimal_stake_pools_count: 510,
            pledge_influence: RationalNumber {
                numerator: 13u64,
                denominator: 20u64,
            },
            collateral_percentage: 160,
            cost_models: CostModels {
                plutus_v1: vec![1; 10],
                plutus_v2: vec![2; 10],
                plutus_v3: vec![3; 10],
            },

            pool_thresholds: PoolThresholds {
                no_confidence: RationalNumber {
                    numerator: 11,
                    denominator: 15,
                },
                committee: RationalNumber {
                    numerator: 11,
                    denominator: 13,
                },
                committee_under_no_confidence: RationalNumber {
                    numerator: 12,
                    denominator: 15,
                },
                hard_fork: RationalNumber {
                    numerator: 13,
                    denominator: 14,
                },
                security_group: RationalNumber {
                    numerator: 11,
                    denominator: 12,
                },
            },
            drep_thresholds: DrepThresholds {
                no_confidence: RationalNumber {
                    numerator: 11,
                    denominator: 15,
                },
                committee: RationalNumber {
                    numerator: 11,
                    denominator: 13,
                },
                committee_under_no_confidence: RationalNumber {
                    numerator: 12,
                    denominator: 15,
                },
                constitution: RationalNumber {
                    numerator: 13,
                    denominator: 14,
                },
                hard_fork: RationalNumber {
                    numerator: 11,
                    denominator: 12,
                },
                protocol_parameters: ProtocolParametersThresholds {
                    network_group: RationalNumber {
                        numerator: 11,
                        denominator: 15,
                    },
                    economic_group: RationalNumber {
                        numerator: 11,
                        denominator: 14,
                    },
                    technical_group: RationalNumber {
                        numerator: 11,
                        denominator: 13,
                    },
                    governance_group: RationalNumber {
                        numerator: 11,
                        denominator: 12,
                    },
                },
                treasury_withdrawal: RationalNumber {
                    numerator: 11,
                    denominator: 20,
                },
            },
            cc_min_size: 13,
            cc_max_term_length: 30u32,
            gov_action_lifetime: 20u32,
            gov_action_deposit: 10_000_010u64,
            drep_deposit: 2_000_010u64,
            drep_expiry: 60u32,
        }
    }

    #[test]
    fn test_utxo_returns_dummy_output() {
        let store = MemoryStore::new();

        let txin = TransactionInput {
            transaction_id: Hash::new([0u8; 32]),
            index: 0,
        };

        let output = PseudoTransactionOutput::PostAlonzo(dummy_post_alonzo_output());

        store.utxos.borrow_mut().insert(txin.clone(), Some(output));

        let result = store.utxo(&txin).unwrap();

        assert!(result.is_some(), "UTXO should be found");
    }

    #[test]
    fn test_account_returns_correct_row() {
        let store = MemoryStore::new();
        let credential = dummy_stake_credential();

        store
            .accounts
            .borrow_mut()
            .insert(credential.clone(), Some(dummy_account_row()));

        let result = store.account(&credential).unwrap();
        assert!(result.is_some(), "Expected Some(account row), got None");

        let row = result.unwrap();
        assert_eq!(row.deposit, 1000, "Deposit mismatch");
        assert_eq!(row.rewards, 500, "Rewards mismatch");
    }

    #[test]
    fn test_pool_returns_correct_row() {
        let store = MemoryStore::new();
        let pool_id = Hash::new([0u8; 28]);
        let dummy_row = dummy_pool_row();

        // Assuming pools map stores Option<pools::Row>, wrap in Some
        store
            .pools
            .borrow_mut()
            .insert(pool_id.clone(), Some(dummy_row.clone()));

        let result = store.pool(&pool_id).unwrap();
        assert!(result.is_some(), "Expected Some(pool row), got None");

        let row = result.unwrap();
        assert_eq!(row.current_params.id, pool_id, "Pool id mismatch");
        assert_eq!(row.current_params.pledge, 0, "Pool pledge mismatch");
        assert_eq!(row.current_params.cost, 0, "Pool cost mismatch");
        assert_eq!(
            row.current_params.margin.numerator, 0,
            "Margin numerator mismatch"
        );
        assert_eq!(
            row.current_params.margin.denominator, 1,
            "Margin denominator mismatch"
        );
        assert_eq!(
            row.current_params.owners.len(),
            1,
            "Pool owners length mismatch"
        );
        assert!(row.current_params.relays.is_empty(), "Expected no relays");
        assert!(
            matches!(row.current_params.metadata, Nullable::Null),
            "Expected metadata to be Null"
        );
        assert!(row.future_params.is_empty(), "Expected no future params");
    }

    #[test]
    fn test_pots_returns_correct_data() {
        let store = MemoryStore::new();
        *store.pots.borrow_mut() = dummy_pot();

        let result = store.pots().unwrap();
        assert_eq!(result.treasury, 1, "Treasury mismatch");
        assert_eq!(result.reserves, 2, "Reserves mismatch");
        assert_eq!(result.fees, 3, "Fees mismatch");
    }

    #[test]
    fn test_iter_utxos_returns_inserted_utxos() {
        let store = MemoryStore::new();

        let txin1 = TransactionInput {
            transaction_id: Hash::new([1u8; 32]),
            index: 0,
        };
        let txin2 = TransactionInput {
            transaction_id: Hash::new([2u8; 32]),
            index: 1,
        };

        let output1 = PseudoTransactionOutput::PostAlonzo(dummy_post_alonzo_output());
        let output2 = PseudoTransactionOutput::PostAlonzo(dummy_post_alonzo_output());

        {
            let mut utxos = store.utxos.borrow_mut();
            utxos.insert(txin1.clone(), Some(output1.clone()));
            utxos.insert(txin2.clone(), Some(output2.clone()));
        }

        let results: Vec<_> = store.iter_utxos().unwrap().collect();

        assert_eq!(results.len(), 2, "Expected 2 UTXOs returned");

        assert_eq!(
            results[0],
            (txin1.clone(), output1.clone()),
            "First UTXO mismatch"
        );
        assert_eq!(
            results[1],
            (txin2.clone(), output2.clone()),
            "Second UTXO mismatch"
        );
    }

    #[test]
    fn test_iter_accounts_returns_inserted_accounts() {
        let store = MemoryStore::new();

        let stake_credential1 = dummy_stake_credential();
        let stake_credential2 = dummy_stake_credential2();
        let row1 = dummy_account_row();
        let row2 = dummy_account_row2();

        {
            let mut accounts = store.accounts.borrow_mut();
            accounts.insert(stake_credential1.clone(), Some(row1.clone()));
            accounts.insert(stake_credential2.clone(), Some(row2.clone()));
        }

        let mut results = store.iter_accounts().unwrap().collect::<Vec<_>>();
        results.sort_by_key(|(k, _)| k.clone());

        let mut expected = vec![
            (stake_credential1.clone(), row1.clone()),
            (stake_credential2.clone(), row2.clone()),
        ];
        expected.sort_by_key(|(k, _)| k.clone());

        assert_eq!(
            results, expected,
            "Accounts iterator returned unexpected results"
        );
    }

    #[test]
    fn test_iter_dreps_returns_inserted_dreps() {
        let store = MemoryStore::new();

        let stake_credential1 = dummy_stake_credential();
        let stake_credential2 = dummy_stake_credential2();
        let row1 = dummy_drep_row();
        let row2 = dummy_drep_row2();

        {
            let mut dreps = store.dreps.borrow_mut();
            dreps.insert(stake_credential1.clone(), Some(row1.clone()));
            dreps.insert(stake_credential2.clone(), Some(row2.clone()));
        }

        let mut results = store.iter_dreps().unwrap().collect::<Vec<_>>();
        results.sort_by_key(|(k, _)| k.clone());

        let mut expected = vec![
            (stake_credential1.clone(), row1.clone()),
            (stake_credential2.clone(), row2.clone()),
        ];
        expected.sort_by_key(|(k, _)| k.clone());

        assert_eq!(
            results, expected,
            "Dreps iterator returned unexpected results"
        );
    }

    // TODO: Implement Ord for ProposalId
    //       to enable sorting of proposals in iter_proposals using BTreeMap keys

    #[test]
    fn test_get_protocol_params_returns_params_for_correct_epoch() {
        let store = MemoryStore::new();

        let epoch1: Epoch = 1u64.into();
        let epoch2: Epoch = 2u64.into();

        let protocol_parameters1 = dummy_protocol_parameters1();
        let protocol_parameters2 = dummy_protocol_parameters2();

        {
            let mut p_params = store.p_params.borrow_mut();
            p_params.insert(epoch1.clone(), protocol_parameters1.clone());
            p_params.insert(epoch2.clone(), protocol_parameters2.clone());
        }

        let result1 = store.get_protocol_parameters_for(&epoch1);
        assert_eq!(
            result1.unwrap(),
            protocol_parameters1,
            "Protocol parameters for epoch 1 mismatch"
        );

        let result2 = store.get_protocol_parameters_for(&epoch2);
        assert_eq!(
            result2.unwrap(),
            protocol_parameters2,
            "Protocol parameters for epoch 2 mismatch"
        );

        let epoch3: Epoch = 3u64.into();
        let result3 = store.get_protocol_parameters_for(&epoch3);
        assert!(
            matches!(result3, Err(StoreError::NotFound)),
            "Expected NotFound error for epoch 3"
        );
    }
}
