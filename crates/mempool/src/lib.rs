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

use std::collections::HashSet;
use pallas_primitives::conway::{TransactionInput, Tx};
use amaru_ledger::store::{Snapshot};

pub trait Mempool {
    type Tx;
    type Block;
    fn add_tx<S: Snapshot>(&mut self, snapshot: &S, tx: Self::Tx) -> bool;
    fn make_block<S: Snapshot>(&mut self, snapshot: &S) -> Option<Self::Block>;
    fn invalidate_utxos(&mut self, txins: HashSet<TransactionInput>);
}

#[derive(Clone, Debug)]
pub struct SimpleMempool {
    transactions: Vec<Tx>,
}

impl SimpleMempool {
    // Just check that each tx input exists
    fn validate_tx<S: Snapshot>(&self, snapshot: &S, tx: &Tx) -> bool {
        for input in &tx.transaction_body.inputs {
            let u = snapshot.utxo(input);
            if !u.is_ok() {
                return false
            }
        }
        return true
    }
}

impl Mempool for SimpleMempool {
    type Tx = Tx;
    type Block = Vec<Tx>;

    fn add_tx<S: Snapshot>(&mut self, snapshot: &S, tx: Tx) -> bool {
        if !self.validate_tx(snapshot, &tx) {
            return false
        }
        self.transactions.push(tx);
        return true
    }

    fn make_block<S: Snapshot>(&mut self, _snapshot: &S) -> Option<Vec<Tx>> {
        Some(self.transactions.clone())
    }

    fn invalidate_utxos(&mut self, txins: HashSet<TransactionInput>) {
        let mut new_transactions = vec![];
        for tx in &self.transactions {
            let mut valid = true;
            for input in &tx.transaction_body.inputs {
                if txins.contains(input) {
                    valid = false;
                }
            }
            if valid {
                new_transactions.push(tx.clone());
            }
        }
        self.transactions = new_transactions;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pallas_primitives::{TransactionInput};
    use pallas_primitives::conway::{TransactionOutput, Tx};
    use amaru_ledger::store::StoreError;
    use amaru_ledger::store::columns::accounts;
    use amaru_ledger::store::columns::pools;
    use amaru_ledger::store::columns::slots;

    struct TestSnapshot {
        utxos: HashMap<TransactionInput, TransactionOutput>,
    }

    #[derive(Debug)]
    struct TestStoreError {
    }

    impl std::fmt::Display for TestStoreError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
            write!(f, "TestStoreError")
        }
    }

    impl std::error::Error for TestStoreError {
    }

    impl Snapshot for TestSnapshot {
        fn utxo(&self, input: &TransactionInput) -> Result<Option<TransactionOutput>, StoreError> {
            match self.utxos.get(input) {
                Some(ok) => Ok(Some(ok.clone())),
                None => Err(StoreError::Internal(Box::new(TestStoreError{}))),
            }
        }
        fn most_recent_snapshot(&self) -> u64 {
            todo!()
        }
        fn pool(&self, _: &pallas_primitives::Hash<28>) -> Result<Option<pools::Row>, StoreError> {
            todo!()
        }
        fn pots(&self) -> Result<Pots, StoreError> {
            todo!()
        }
        fn iter_utxos(&self) -> Result<impl Iterator<Item = (TransactionInput, TransactionOutput)>, StoreError> {
            todo!() as Result<Cloned<Iter<_>>, _>
        }
        fn iter_block_issuers(&self) -> Result<impl Iterator<Item = (u64, slots::Row)>, StoreError> {
            todo!() as Result<Cloned<Iter<_>>, _>
        }
        fn iter_pools(&self) -> Result<impl Iterator<Item = (pallas_primitives::Hash<28>, pools::Row)>, StoreError> {
            todo!() as Result<Cloned<Iter<_>>, _>
        }
        fn iter_accounts(&self) -> Result<impl Iterator<Item = (StakeCredential, accounts::Row)>, StoreError> {
            todo!() as Result<Cloned<Iter<_>>, _>
        }
    }

    fn make_empty_tx() -> Tx {
        Tx {
            transaction_body: pallas_primitives::conway::TransactionBody {
                inputs: vec![].into(),
                outputs: vec![].into(),
                fee: 0,
                ttl: None,
                validity_interval_start: None,
                certificates: None,
                withdrawals: None,
                auxiliary_data_hash: None,
                mint: None,
                script_data_hash: None,
                collateral: None,
                required_signers: None,
                network_id: None,
                collateral_return: None,
                reference_inputs: None,
                total_collateral: None,
                voting_procedures: None,
                proposal_procedures: None,
                treasury_value: None,
                donation: None,
            },
            transaction_witness_set: pallas_primitives::conway::WitnessSet {
                vkeywitness: None,
                native_script: None,
                bootstrap_witness: None,
                plutus_v1_script: None,
                plutus_data: None,
                redeemer: None,
                plutus_v2_script: None,
                plutus_v3_script: None,
            },
            success: true,
            auxiliary_data: None.into(),
        }
    }

    fn simple_output(amount: u64) -> TransactionOutput {
        pallas_primitives::conway::PseudoTransactionOutput::PostAlonzo(
             pallas_primitives::conway::PostAlonzoTransactionOutput {
                 address: vec![].into(),
                 value: pallas_primitives::conway::Value::Coin(amount),
                 datum_option: None,
                 script_ref: None,
             },
         )
    }

    fn tx_add_input(tx: &mut Tx, input: TransactionInput) {
        let mut inputs: Vec<TransactionInput> = tx.transaction_body.inputs.clone().to_vec();
        inputs.push(input);
        tx.transaction_body.inputs = inputs.into();
    }

    fn tx_add_output(tx: &mut Tx, output: TransactionOutput) {
        tx.transaction_body.outputs.push(output);
    }

    #[test]
    fn add_tx_success() {
        let mut mempool = SimpleMempool {
            transactions: vec![],
        };
        let id = [0; 32];
        let mut utxos = HashMap::new();
        utxos.insert(
            TransactionInput {
                transaction_id: pallas_primitives::Hash::new(id),
                index: 0,
            },
            simple_output(1_000_000),
        );
        let snapshot = TestSnapshot {
            utxos: utxos,
        };
        let mut basic_tx = make_empty_tx();
        let success = mempool.add_tx(&snapshot, basic_tx);
        assert_eq!(success, true);
    }

    #[test]
    fn add_tx_failure() {
        let mut mempool = SimpleMempool {
            transactions: vec![],
        };
        let id = [0; 32];
        let mut utxos = HashMap::new();
        utxos.insert(
            TransactionInput {
                transaction_id: pallas_primitives::Hash::new(id),
                index: 0,
            },
            simple_output(1_000_000),
        );
        let snapshot = TestSnapshot {
            utxos: utxos,
        };
        let mut basic_tx = make_empty_tx();
        tx_add_input(&mut basic_tx, TransactionInput {
            transaction_id: pallas_primitives::Hash::new(id),
            index: 1,
        });
        let success = mempool.add_tx(&snapshot, basic_tx);
        assert_eq!(success, false);
    }

    #[test]
    fn invalidate_utxos() {
        let mut mempool = SimpleMempool {
            transactions: vec![],
        };
        let id = [0; 32];
        let mut utxos = HashMap::new();
        utxos.insert(
            TransactionInput {
                transaction_id: pallas_primitives::Hash::new(id),
                index: 0,
            },
            simple_output(1_000_000),
        );
        let snapshot = TestSnapshot {
            utxos: utxos,
        };
        let mut basic_tx = make_empty_tx();
        tx_add_input(&mut basic_tx, TransactionInput {
            transaction_id: pallas_primitives::Hash::new(id),
            index: 0,
        });
        mempool.add_tx(&snapshot, basic_tx);
        let mut set = HashSet::new();
        set.insert(TransactionInput {
            transaction_id: pallas_primitives::Hash::new(id),
            index: 0,
        });
        mempool.invalidate_utxos(set);
        assert_eq!(mempool.transactions.len(), 0);
    }
}
