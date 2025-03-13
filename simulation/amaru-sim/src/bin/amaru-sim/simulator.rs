// Copyright 2025 PRAGMA
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

use std::{path::PathBuf, sync::Arc, time::Duration};

use crate::ledger::{FakeLedgerStage, FakeStakeDistribution};
use amaru_consensus::peer::Peer;
use amaru_consensus::{
    chain_forward,
    consensus::{
        chain_selection::{ChainSelector, ChainSelectorBuilder},
        header_validation::Consensus,
        store::{rocksdb::RocksDBStore, ChainStore},
        wiring::HeaderStage,
    },
};
use amaru_kernel::{
    Header,
    Point::{self, *},
};
use clap::{ArgAction, Parser};
use gasket::runtime::Tether;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::trace;

#[derive(Debug, Parser)]
#[clap(name = "Amaru Simulator")]
#[clap(bin_name = "amaru-sim")]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    /// Upstream peer "addresses" to synchronize from.
    ///
    /// These are not actual network addresses but rather identifiers for the peers.
    /// This option can be specified multiple times to connect to multiple peers.
    /// At least one peer address must be specified.
    #[arg(long, action = ArgAction::Append, required = true)]
    peer_address: Vec<String>,

    /// Path of JSON-formatted stake distribution file.
    #[arg(long, default_value = "./stake_distribution.json")]
    stake_distribution_file: PathBuf,

    /// Path of the chain on-disk storage.
    #[arg(long, default_value = "./chain.db")]
    chain_dir: PathBuf,

    /// Path to the directory containing blockchain data such as epoch nonces.
    #[arg(long, default_value = "./data")]
    data_dir: PathBuf,
}

pub async fn run(args: Args) {
    let sync = bootstrap(args);

    let exit = amaru::exit::hook_exit_token();

    run_pipeline(gasket::daemon::Daemon::new(sync), exit.clone()).await;
}

fn define_gasket_policy() -> gasket::runtime::Policy {
    let retries = gasket::retries::Policy {
        max_retries: 20,
        backoff_unit: std::time::Duration::from_secs(1),
        backoff_factor: 2,
        max_backoff: std::time::Duration::from_secs(60),
        dismissible: false,
    };

    gasket::runtime::Policy {
        //be generous with tick timeout to avoid timeout during block awaits
        tick_timeout: std::time::Duration::from_secs(600).into(),
        bootstrap_retry: retries.clone(),
        work_retry: retries.clone(),
        teardown_retry: retries.clone(),
    }
}

pub fn bootstrap(args: Args) -> Vec<Tether> {
    let stake_distribution: FakeStakeDistribution =
        FakeStakeDistribution::from_file(&args.stake_distribution_file).unwrap();

    let mut ledger = FakeLedgerStage::new();

    let mut sync_from_peers = crate::sync::Stage::new();

    let chain_store =
        RocksDBStore::new(args.chain_dir.clone()).expect("unable to open chain store");
    let chain_selector = make_chain_selector(
        Origin,
        &chain_store,
        &args
            .peer_address
            .iter()
            .map(|a| Peer::new(&a.clone()))
            .collect::<Vec<_>>(),
    );
    let chain_ref = Arc::new(Mutex::new(chain_store));
    let consensus = Consensus::new(
        vec![],
        Box::new(stake_distribution),
        chain_ref.clone(),
        chain_selector,
    );

    let mut consensus_stage = HeaderStage::new(consensus);

    let mut block_forward = chain_forward::ForwardStage::new(chain_ref.clone());

    let (to_consensus, from_peers) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_ledger, from_header_validation) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_block_forward, from_ledger) = gasket::messaging::tokio::mpsc_channel(50);

    sync_from_peers.downstream.connect(to_consensus);
    consensus_stage.upstream.connect(from_peers);
    consensus_stage.downstream.connect(to_ledger);
    ledger.upstream.connect(from_header_validation);
    ledger.downstream.connect(to_block_forward);
    block_forward.upstream.connect(from_ledger);

    let policy = define_gasket_policy();

    let chain_sync = gasket::runtime::spawn_stage(sync_from_peers, policy.clone());
    let header_validation = gasket::runtime::spawn_stage(consensus_stage, policy.clone());
    let ledger = gasket::runtime::spawn_stage(ledger, policy.clone());
    let block_forward = gasket::runtime::spawn_stage(block_forward, policy.clone());

    vec![chain_sync, header_validation, ledger, block_forward]
}

fn make_chain_selector(
    tip: Point,
    chain_store: &impl ChainStore<Header>,
    peers: &Vec<Peer>,
) -> Arc<Mutex<ChainSelector<Header>>> {
    let mut builder = ChainSelectorBuilder::new();

    load_tip_from_store(chain_store, tip, &mut builder);

    for peer in peers {
        builder.add_peer(peer);
    }

    match builder.build() {
        Ok(chain_selector) => Arc::new(Mutex::new(chain_selector)),
        Err(e) => panic!("unable to build chain selector: {:?}", e),
    }
}

fn load_tip_from_store<'a>(
    chain_store: &impl ChainStore<Header>,
    tip: Point,
    builder: &'a mut ChainSelectorBuilder<Header>,
) -> &'a mut ChainSelectorBuilder<Header> {
    match tip {
        Origin => builder,
        Specific(..) => match chain_store.load_header(&From::from(&tip)) {
            None => panic!("Tip {:?} not found in chain store", tip),
            Some(header) => builder.set_tip(&header),
        },
    }
}

pub async fn run_pipeline(pipeline: gasket::daemon::Daemon, exit: CancellationToken) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5000)) => {
                if pipeline.should_stop() {
                    break;
                }
            }
            _ = exit.cancelled() => {
                trace!("exit requested");
                break;
            }
        }
    }

    trace!("shutting down pipeline");

    pipeline.teardown();
}
