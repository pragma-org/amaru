use super::{
    kernel::{block_point, Hash, Hasher, MintedBlock, Point, TransactionInput, TransactionOutput},
    store::Store,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// The maximum depth of a rollback, also known as the security parameter 'k'.
/// This translates down to the length of our volatile storage, containing states of the ledger
/// which aren't yet considered final.
pub const MAX_ROLLBACK_DEPTH: usize = 2160;

/// The state of the ledger split into two sub-components:
///
/// - A _stable_ and persistent storage, which contains the part of the state which known to be
///   final. Fundamentally, this contains the aggregated state of the ledger that is at least 'k'
///   blocks old; where 'k' is the security parameter of the protocol.
///
/// - A _volatile_ state, which is maintained as a sequence of diff operations to be applied on
///   top of the _stable_ store. It contains at most 'MAX_ROLLBACK_DEPTH' entries; old entries
///   get persisted in the stable storage when they are popped out of the volatile state.
pub struct State<'a, E> {
    stable: StableDB<'a, E>,
    volatile: VolatileDB,
}

type StableDB<'a, E> = Box<dyn Store<Error = E> + 'a>;

impl<'a, E> State<'a, E> {
    pub fn new(stable: StableDB<'a, E>) -> Self {
        Self {
            stable,
            // NOTE: At this point, we always restart from an empty volatile state; which means
            // that there needs to be some form of synchronization between the consensus and the
            // ledger here. Few assumptions also stems from this:
            //
            // (1) The consensus must be storing its own state, and in particular, where it has
            //     left the synchronization.
            //
            // (2) Re-applying 2160 (already synchronized) blocks is _fast-enough_ that it can be
            //     done on restart easily. To be measured; if this turns out to be too slow, we
            //     will consider storing views of the volatile DB on-disk to be able to restore
            //     them quickly.
            volatile: VecDeque::new(),
        }
    }

    /// Roll the ledger forward with the given block by applying transactions one by one, in
    /// sequence. The update stops at the first invalid transaction, if any. Otherwise, it updates
    /// the internal state of the ledger.
    pub fn forward(&mut self, block: MintedBlock<'_>) -> Result<(), ForwardErr<E>> {
        let failed_transactions = FailedTransactions::from_block(&block);

        let point = block_point(&block);

        let mut diff = Diff::default();

        for (ix, transaction_body) in block.transaction_bodies.to_vec().into_iter().enumerate() {
            let transaction_id = Hasher::<256>::hash(transaction_body.raw_cbor());
            let transaction_body = transaction_body.unwrap();

            let (inputs, outputs) = if failed_transactions.has(ix as u32) {
                let inputs = transaction_body
                    .collateral
                    .map(|x| x.to_vec())
                    .unwrap_or_default()
                    .into_iter();

                let outputs = match transaction_body.collateral_return {
                    Some(output) => Box::new([output.into()].into_iter())
                        as Box<dyn Iterator<Item = TransactionOutput>>,
                    None => {
                        Box::new(std::iter::empty()) as Box<dyn Iterator<Item = TransactionOutput>>
                    }
                };

                (
                    Box::new(inputs) as Box<dyn Iterator<Item = TransactionInput>>,
                    outputs,
                )
            } else {
                let inputs = transaction_body.inputs.to_vec().into_iter();
                let outputs = transaction_body.outputs.into_iter().map(|x| x.into());
                (
                    Box::new(inputs) as Box<dyn Iterator<Item = TransactionInput>>,
                    Box::new(outputs) as Box<dyn Iterator<Item = TransactionOutput>>,
                )
            };

            diff.merge(self.apply_transaction(&transaction_id, inputs, outputs)?);
        }

        if self.volatile.len() >= MAX_ROLLBACK_DEPTH {
            let now_stable = self.volatile.pop_front().unwrap();
            self.stable
                .save(
                    &point,
                    Box::new(now_stable.produced.into_iter()),
                    Box::new(now_stable.consumed.into_iter()),
                )
                .map_err(ForwardErr::StorageErr)?;
        }

        self.volatile.push_back(diff.anchor(point));

        Ok(())
    }

    fn apply_transaction(
        &mut self,
        transaction_id: &Hash<32>,
        inputs: impl Iterator<Item = TransactionInput>,
        outputs: impl Iterator<Item = TransactionOutput>,
    ) -> Result<Diff<()>, ForwardErr<E>> {
        let mut consumed = BTreeSet::new();
        consumed.extend(inputs);

        let mut produced = BTreeMap::new();
        for (ix, output) in outputs.enumerate() {
            let input = TransactionInput {
                transaction_id: *transaction_id,
                index: ix as u64,
            };
            produced.insert(input, output);
        }

        Ok(Diff {
            point: (),
            consumed,
            produced,
        })
    }

    pub fn backward<'b>(&mut self, to: &'b Point) -> Result<(), BackwardErr<'b>> {
        if let Some(ix) = self.volatile.iter().position(|diff| &diff.point == to) {
            self.volatile.resize_with(ix + 1, || {
                unreachable!("ix is necessarly strictly smaller than the length")
            });
            Ok(())
        } else {
            Err(BackwardErr::UnknownRollbackPoint(to))
        }
    }
}

// FailedTransactions
// ----------------------------------------------------------------------------

struct FailedTransactions {
    inner: BTreeSet<u32>,
}

impl FailedTransactions {
    pub fn from_block(block: &MintedBlock<'_>) -> Self {
        FailedTransactions {
            inner: block
                .invalid_transactions
                .as_deref()
                .map(|indices| {
                    let mut tree = BTreeSet::new();
                    tree.extend(indices.to_vec().as_slice());
                    tree
                })
                .unwrap_or_default(),
        }
    }

    pub fn has(&self, ix: u32) -> bool {
        self.inner.contains(&ix)
    }
}

// VolatileDB
// ----------------------------------------------------------------------------

// NOTE: Once we implement ledger validation, we might want to maintain an aggregated version of
// this sequence of _Diff_, such that one can easily lookup the volatile database before reaching
// for the stable storage.
//
// Otherwise, we need to traverse the entire sequence for any query on the volatile state.
type VolatileDB = VecDeque<Diff<Point>>;

pub struct Diff<T> {
    pub point: T,
    pub consumed: BTreeSet<TransactionInput>,
    pub produced: BTreeMap<TransactionInput, TransactionOutput>,
}

impl Default for Diff<()> {
    fn default() -> Self {
        Self {
            point: (),
            consumed: Default::default(),
            produced: Default::default(),
        }
    }
}

impl Diff<()> {
    pub fn merge(&mut self, other: Diff<()>) {
        self.consumed.extend(other.consumed);
        self.produced.extend(other.produced);
        self.produced.retain(|k, _| !self.consumed.contains(k));
    }

    pub fn anchor(self, point: Point) -> Diff<Point> {
        Diff {
            point,
            consumed: self.consumed,
            produced: self.produced,
        }
    }
}

// Errors
// ----------------------------------------------------------------------------

#[derive(Debug)]
pub enum ForwardErr<E> {
    StorageErr(E),
}

#[derive(Debug)]
pub enum BackwardErr<'a> {
    /// The ledger has been instructed to rollback to an unknown point. This should be impossible
    /// if chain-sync messages (roll-forward and roll-backward) are all passed to the ledger.
    UnknownRollbackPoint(&'a Point),
}
