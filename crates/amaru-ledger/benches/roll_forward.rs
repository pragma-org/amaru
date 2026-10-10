// Copyright 2026 PRAGMA
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

#![expect(clippy::expect_used, reason = "benchmark setup failures are fatal")]
#![expect(clippy::unwrap_used, reason = "benchmark setup failures are fatal")]

use std::{cell::RefCell, sync::LazyLock};

use amaru_kernel::{
    Address, Block, BlockHeight, Credential, Epoch, EraHistory, GlobalParameters, Hash, Hasher, Lovelace,
    MemoizedDatum, MemoizedTransactionOutput, Network, NetworkName, NonEmptyVec, Point, ProposalsRoots,
    ProtocolParameters, ShelleyAddress, Transaction, TransactionBody, TransactionInput, Value, VerificationKey,
    VerificationKeyWitness, WitnessSet, block,
    cbor::WithSize,
    ed25519::{Signer, SigningKey},
    make_header, to_cbor,
};
use amaru_ledger::{
    epoch_transition::GovernanceActivity,
    rules::block::BlockValidation,
    state::{State, volatile::VolatileFragment},
    store::{Columns, Store, TransactionalContext},
};
use amaru_plutus::arena_pool::ArenaPool;
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores, RocksDbConfig};
use divan::{Bencher, black_box};
use rand::{Rng, SeedableRng, rngs::StdRng};
use tempfile::TempDir;

fn main() {
    divan::main();
}

// Network constants
const NETWORK: NetworkName = NetworkName::Preprod;
static ERA_HISTORY: LazyLock<EraHistory> = LazyLock::new(|| NETWORK.as_era_history().unwrap().clone());
static PROTOCOL_PARAMETERS: LazyLock<ProtocolParameters> =
    LazyLock::new(|| NETWORK.as_protocol_parameters().unwrap().clone());
static GLOBAL_PARAMETERS: LazyLock<GlobalParameters> =
    LazyLock::new(|| NETWORK.as_global_parameters().unwrap().clone());

// Default Data
static DEFAULT_SIGNING_KEY: LazyLock<SigningKey> = LazyLock::new(|| SigningKey::from_bytes(&[42; 32]));

static DEFAULT_VERIFICATION_KEY: LazyLock<VerificationKey> =
    LazyLock::new(|| VerificationKey::from(DEFAULT_SIGNING_KEY.verifying_key().to_bytes()));

static DEFAULT_ADDRESS: LazyLock<Address> = LazyLock::new(|| {
    let payment = Credential::KeyHash(Hasher::<224>::hash(DEFAULT_VERIFICATION_KEY.as_slice()));
    Address::Shelley(ShelleyAddress::new(Network::Testnet, payment, None))
});

// Bench constants
const SAMPLE_COUNT: u32 = 1000;

/// Apply an empty block to a saturated ledger volatile window.
///
/// Fixture construction is deliberately outside Divan's timed section. It creates a real RocksDB
/// store, seeds 10,000 random UTxOs, and fills the volatile sequence to its security parameter.
///
/// The timed path is therefore the same one used by the ledger for a normal block after warm-up.
/// This benchmark is useful in order to subtract the overhead of block processing from raw
/// transaction processing. Thus avoiding the manual construction of a validation context
/// specifically for benchmarks. Instead, we can use the higher-level interface and, while at it,
/// get a sense of the overhead needed.
#[divan::bench(sample_count = SAMPLE_COUNT, sample_size = 1, args = [0, 10, 20, 50, 100, 200])]
fn simple_transactions(bencher: Bencher<'_, '_>, count: usize) {
    let mut bench = Bench::new();

    // Warm-up caches & what not.
    let block = bench.prepare(0);
    bench.run(&block);

    // NOTE: use of RefCell
    // This is okay only because we do not enable divan in parallel here. Necessary because both
    // .with_inputs and .bench_local_values need to borrow bench mutably. Yet, never at the same
    // time in the absence of threads.
    let bench = RefCell::new(bench);

    bencher
        .with_inputs(|| bench.borrow_mut().prepare(count))
        .bench_local_values(|block| bench.borrow_mut().run(&block));
}

struct Bench {
    state: State<RocksDB, RocksDBHistoricalStores>,
    pool: UtxoPool,
    current_height: BlockHeight,
    arena_pool: ArenaPool,
    // The state owns the RocksDB handles and must be dropped before its directory is removed.
    _directory: TempDir,
}

impl Bench {
    pub fn new() -> Self {
        let (_directory, store, snapshots) = Self::new_store();

        let pool = UtxoPool::default().save(&store);

        let mut state = State::new_with(
            store,
            snapshots,
            Epoch::default(),
            NETWORK,
            ERA_HISTORY.clone(),
            GLOBAL_PARAMETERS.clone(),
            PROTOCOL_PARAMETERS.clone(),
            GovernanceActivity::default(),
            None,
            Default::default(),
        );

        let current_height = Self::warm_volatile(&mut state);

        Self { state, pool, current_height, arena_pool: ArenaPool::default(), _directory }
    }

    pub fn prepare(&mut self, transaction_count: usize) -> Block {
        self.current_height = self.current_height + 1;
        self.pool.new_block_at(self.current_height, transaction_count)
    }

    pub fn run(&mut self, block: &Block) {
        assert!(matches!(black_box(self.state.roll_forward(block, &self.arena_pool)), BlockValidation::Valid(_)));
    }

    fn new_store() -> (TempDir, RocksDB, RocksDBHistoricalStores) {
        let directory = tempfile::tempdir().expect("temporary RocksDB directory");
        let config = RocksDbConfig::new(directory.path().to_path_buf());
        let store = RocksDB::empty(&config).expect("open temporary RocksDB store");
        let snapshots = RocksDBHistoricalStores::new(&config, 0);

        (directory, store, snapshots)
    }

    fn warm_volatile(state: &mut State<RocksDB, RocksDBHistoricalStores>) -> BlockHeight {
        let max_height = GLOBAL_PARAMETERS.consensus_security_param;

        for block_height in 1..=max_height {
            let point = Point::Specific(block_height.into(), Hash::new([block_height as u8; 32]), block_height.into());
            let result = state.push_fragment(VolatileFragment::default().anchor(point, Hash::new([0; 28])));
            assert!(matches!(result, Ok(None)), "warming must not flush a volatile fragment");
        }

        max_height.into()
    }
}

// -------------------------------------------------------------------------------------------------
// UtxoPool
// -------------------------------------------------------------------------------------------------

struct UtxoPool {
    entries: Vec<Utxo>,
}

impl Default for UtxoPool {
    fn default() -> Self {
        let mut rng = StdRng::seed_from_u64(Self::RNG_SEED);
        let mut entries = Vec::with_capacity(Self::SIZE);

        for index in 0..Self::SIZE {
            entries.push(Utxo::random(&mut rng, index as u64))
        }

        Self { entries }
    }
}

impl UtxoPool {
    const RNG_SEED: u64 = 0xA6A2_2026;
    const SIZE: usize = 10_000;

    /// Materialize a Block, replacing UTxOs that are consumed by the block for future selections.
    pub fn new_block_at(&mut self, height: BlockHeight, transaction_count: usize) -> Block {
        let selected = self.sample(transaction_count);
        let (block, replacements) = self.block_from_selection(selected, height);
        replacements.into_iter().for_each(|(index, utxo)| self.replace(index, utxo));
        block
    }

    /// Seed a given `store` with the UTxO list.
    fn save(self, store: &RocksDB) -> Self {
        store
            .with_transaction(|transaction| {
                transaction.set_protocol_parameters(&PROTOCOL_PARAMETERS)?;
                transaction.set_proposals_roots(&ProposalsRoots::default())?;
                transaction.save(
                    &ERA_HISTORY,
                    &PROTOCOL_PARAMETERS,
                    None,
                    &Point::Origin,
                    None,
                    Columns {
                        utxo: Box::new(self.entries.iter().map(From::from)) as Box<dyn Iterator<Item = _>>,
                        ..Default::default()
                    },
                    Columns::empty(),
                    std::iter::empty(),
                )
            })
            .expect("seed temporary RocksDB store");

        store.next_snapshot(Epoch::default()).expect("create initial ledger snapshot");

        self
    }

    fn get(&self, index: usize) -> Utxo {
        self.entries[index]
    }

    /// Spend a TxO and replace it with a new UTxO
    fn replace(&mut self, index: usize, utxo: Utxo) {
        self.entries[index] = utxo;
    }

    /// Get `size` distinct UTxO from the pool, by their indices.
    fn sample(&self, size: usize) -> Vec<usize> {
        let start = StdRng::seed_from_u64(Self::RNG_SEED).random_range(0..Self::SIZE);
        (0..size).map(|offset| (start + offset) % Self::SIZE).collect::<Vec<_>>()
    }

    fn block_from_selection(
        &self,
        inputs: impl IntoIterator<Item = usize>,
        height: BlockHeight,
    ) -> (Block, Vec<(usize, Utxo)>) {
        let mut replacements = Vec::new();
        let mut transactions = Vec::new();

        for ix in inputs {
            let (transaction, next) = Utxo::next(self.get(ix));
            replacements.push((ix, next));
            transactions.push(transaction);
        }

        let block_body =
            block::BodyParts::from_transactions(transactions).expect("failed to create block body from transactions");
        let (body_hash, body_size) = block_body.commitment();

        let mut header = make_header(height.into_u64(), height.into_u64(), None);
        header.body_mut().block_body_hash = body_hash;
        header.body_mut().block_body_size = body_size;

        (block_body.into_block(header).expect("failed to create block"), replacements)
    }
}

// -------------------------------------------------------------------------------------------------
// Utxo
// -------------------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Utxo {
    input: TransactionInput,
    lovelace: u64,
}

impl Utxo {
    const FEE: Lovelace = 1_000_000;
    const INPUT_LOVELACE_MIN: Lovelace = SAMPLE_COUNT as u64 * Self::FEE + 1_000_000;
    const INPUT_LOVELACE_MAX: Lovelace = 10 * Self::INPUT_LOVELACE_MIN;

    fn next(utxo: Utxo) -> (Transaction, Self) {
        let output_amount = utxo.lovelace - Utxo::FEE;

        let output = Self::simple_output(output_amount);

        let body = TransactionBody::new([utxo.input], [output], Utxo::FEE);

        let next = Utxo { input: TransactionInput { transaction_id: body.id(), index: 0 }, lovelace: output_amount };

        let witnesses = WitnessSet {
            verification_key_witness: Some(NonEmptyVec::singleton(VerificationKeyWitness {
                verification_key: *DEFAULT_VERIFICATION_KEY,
                signature: DEFAULT_SIGNING_KEY.sign(body.id().as_slice()).to_bytes().into(),
            })),
            ..WitnessSet::default()
        };

        let witnesses_size = to_cbor(&witnesses).len();

        let transaction = Transaction {
            body,
            witnesses: WithSize::new(witnesses, witnesses_size),
            is_expected_valid: true,
            auxiliary_data: None,
        };

        (transaction, next)
    }

    pub fn random(rng: &mut impl Rng, index: u64) -> Self {
        let mut transaction_id: [u8; 32] = rng.random();
        transaction_id[..8].copy_from_slice(&(index).to_be_bytes());
        Self {
            input: TransactionInput { transaction_id: Hash::new(transaction_id), index: 0 },
            lovelace: rng.random_range(Self::INPUT_LOVELACE_MIN..=Self::INPUT_LOVELACE_MAX),
        }
    }

    fn simple_output(output_amount: Lovelace) -> MemoizedTransactionOutput {
        MemoizedTransactionOutput::new(
            false,
            DEFAULT_ADDRESS.clone(),
            Value::Coin(output_amount),
            MemoizedDatum::None,
            None,
        )
    }
}

impl From<&Utxo> for (TransactionInput, MemoizedTransactionOutput) {
    fn from(utxo: &Utxo) -> Self {
        (utxo.input, Utxo::simple_output(utxo.lovelace))
    }
}
