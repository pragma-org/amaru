use pallas_codec::utils::Nullable;
use pallas_crypto::hash::{Hash, Hasher};
use pallas_primitives::conway::{MintedBlock, MintedTx, TransactionInput, TransactionOutput};
use std::collections::{BTreeSet, HashMap};
use tracing::debug;
// use uplc::tx::{eval_phase_two, script_context::ResolvedInput, SlotConfig};

pub type Point = pallas_network::miniprotocols::Point;

const MAX_ROLLBACK_DEPTH: u64 = 129600;
const GC_DEFAULT_COUNTER: usize = 1000;

pub(crate) mod worker;

/// A 'dummy' in-memory ledger state focused solely on UTxO state management.
pub struct LedgerState {
    /// Keep track of the current UTxO state.
    utxo_set: HashMap<TransactionInput, TransactionOutput>,

    /// Recently created UTxO, needed on chain switches. Should ideally be bounded.
    /// We maintain references to the recently created UTxO so that we know which one to
    /// remove from our ongoing set when rolling back.
    recently_created: Vec<(Point, TransactionInput)>,

    /// Recently consumed UTxO, needed on chain switches. Should ideally be bounded.
    /// We stash UTxO that have been recently consumed here so we can "put them back"
    /// if we rollback beyond the point they were created.
    recently_consumed: Vec<(Point, TransactionInput, TransactionOutput)>,

    /// Some internal counter which ensures that recently_created & recently_consumed do not get
    /// too big.
    gc_counter: usize,
}

#[derive(Debug)]
pub enum ForwardErr {
    UnknownTransactionInput(TransactionInput),
    PhaseTwoValidations(uplc::tx::error::Error),
}

#[derive(Debug)]
pub enum BackwardErr<'a> {
    /// The ledger has been instructed to rollback to an unknown point. This should be impossible
    /// if chain-sync messages (roll-forward and roll-backward) are all passed to the ledger.
    UnknownRollbackPoint(&'a Point),
}

impl LedgerState {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        LedgerState {
            utxo_set: HashMap::new(),
            recently_created: Vec::new(),
            recently_consumed: Vec::new(),
            gc_counter: GC_DEFAULT_COUNTER,
        }
    }

    pub fn resolve(&self, input: &TransactionInput) -> Option<&TransactionOutput> {
        self.utxo_set.get(input)
    }

    pub fn backward<'b>(&mut self, to: &'b Point) -> Result<(), BackwardErr<'b>> {
        self.recently_created.retain(|(created_at, input)| {
            if slot(created_at) <= slot(to) {
                return true;
            }

            self.utxo_set.remove(input);

            false
        });

        self.recently_consumed
            .retain(|(consumed_at, input, output)| {
                if slot(consumed_at) <= slot(to) {
                    return true;
                }

                self.utxo_set.insert(input.to_owned(), output.to_owned());

                false
            });

        Ok(())
    }

    pub fn forward(&mut self, block: MintedBlock<'_>) -> Result<(), ForwardErr> {
        let failed_transactions: BTreeSet<u32> = block
            .invalid_transactions
            .map(|indices| {
                let mut tree = BTreeSet::new();
                tree.extend(indices.to_vec().as_slice());
                tree
            })
            .unwrap_or_default();

        let point = Point::Specific(
            block.header.header_body.slot,
            // TODO: Move hashing to pallas.
            // It should be straightforward to request the hash of any `KeepRaw structure.
            Hasher::<256>::hash(block.header.raw_cbor()).to_vec(),
        );

        self.garbage_collect(&point);

        for (ix, transaction_body) in Vec::from(block.transaction_bodies).into_iter().enumerate() {
            // TODO: Move hashing to pallas.
            // It should be straightforward to request the hash of any `KeepRaw structure.
            let transaction_id = Hasher::<256>::hash(transaction_body.raw_cbor());

            let transaction_witness_set = block.transaction_witness_sets[ix].clone();

            if failed_transactions.contains(&(ix as u32)) {
                // NOTE: 'unwrap' is a misnommer from pallas here, and does nothing unsafe.
                let transaction_body = transaction_body.unwrap();

                let inputs = transaction_body
                    .collateral
                    .map(|x| x.to_vec())
                    .unwrap_or_default()
                    .into_iter();

                if let Some(output) = transaction_body.collateral_return.map(|x| x.into()) {
                    let outputs = [output].into_iter();
                    self.apply_transaction(&point, &transaction_id, inputs, outputs)?;
                }
            } else {
                let minted_tx = MintedTx {
                    transaction_body,
                    transaction_witness_set,
                    success: true,
                    // FIXME: ignored and unneeded for now.
                    auxiliary_data: Nullable::Null,
                };

                // FIXME: Disabling phase-2 validation for now.
                // self.eval_phase_two(&transaction_id, &minted_tx)?;

                // NOTE: 'unwrap' is a misnommer from pallas here, and does nothing unsafe.
                let transaction_body = minted_tx.transaction_body.unwrap();

                // TODO:
                // We shouldn't need 'to_vec', pallas ought to provide iterator instances
                // directly on those.
                let inputs = transaction_body.inputs.to_vec().into_iter();
                let outputs = transaction_body.outputs.into_iter().map(|x| x.into());
                self.apply_transaction(&point, &transaction_id, inputs, outputs)?;
            }
        }

        Ok(())
    }

    // FIXME: Disabling phase-2 validation for now until we can point to an upstream uplc version
    // that uses the latest version of pallas that we need for consensus.
    //
    // fn eval_phase_two(
    //     &self,
    //     transaction_id: &Hash<32>,
    //     transaction: &MintedTx<'_>,
    // ) -> Result<(), ForwardErr> {
    //     let mut resolved_inputs = Vec::new();
    //
    //     for input in transaction.transaction_body.inputs.iter() {
    //         if let Some(output) = self.utxo_set.get(input) {
    //             resolved_inputs.push(ResolvedInput {
    //                 input: input.clone(),
    //                 output: output.clone(),
    //             })
    //         } else {
    //             warn!("[context: {transaction_id:?}] unknown input in transaction, skipping phase-2 validations");
    //             return Ok(());
    //         }
    //     }
    //
    //     let redeemers = eval_phase_two(
    //         transaction,
    //         &resolved_inputs[..],
    //         None,
    //         None,
    //         &SlotConfig::default(),
    //         false,
    //         |_| (),
    //     )
    //     .map_err(ForwardErr::PhaseTwoValidations)?;
    //
    //     if !redeemers.is_empty() {
    //         info!("[context: {transaction_id:?}] phase-2 evaluation results: {redeemers:?}");
    //     } else {
    //         info!("[context: {transaction_id:?}] no Plutus scripts found in transaction, skipping phase-2 validations");
    //     }
    //
    //     Ok(())
    // }

    fn apply_transaction(
        &mut self,
        point: &Point,
        transaction_id: &Hash<32>,
        inputs: impl Iterator<Item = TransactionInput>,
        outputs: impl Iterator<Item = TransactionOutput>,
    ) -> Result<(), ForwardErr> {
        // Remove consumed outputs
        for input in inputs.into_iter() {
            if let Some(output) = self.utxo_set.remove(&input) {
                self.recently_consumed.push((point.clone(), input, output));
            } else {
                debug!(
                    "ignoring unknown consumed transaction input : {:?}#{:?}",
                    input.index, input.transaction_id
                );
            }
        }

        // Insert newly created outputs
        for (ix, output) in outputs.enumerate() {
            let input = TransactionInput {
                transaction_id: *transaction_id,
                index: ix as u64,
            };

            self.recently_created.push((point.clone(), input.clone()));
            self.utxo_set.insert(input, output);
        }

        Ok(())
    }

    fn garbage_collect(&mut self, point: &Point) {
        let predicate = |at: &Point| slot(point).abs_diff(slot(at)) <= MAX_ROLLBACK_DEPTH;
        if self.gc_counter == 0 {
            self.gc_counter = GC_DEFAULT_COUNTER;
            self.recently_created.retain(|(p, _)| predicate(p));
            self.recently_consumed.retain(|(p, _, _)| predicate(p));
        } else {
            self.gc_counter -= 1;
        }
    }
}

fn slot(point: &Point) -> u64 {
    match point {
        Point::Origin => 0,
        Point::Specific(s, _) => *s,
    }
}
