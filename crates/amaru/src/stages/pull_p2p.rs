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

use crate::send;
use amaru_consensus::consensus::ChainSyncEvent;
use amaru_kernel::Point;
use gasket::framework::*;
use pallas_network2::{
    Manager, PeerId,
    behavior::{
        InitiatorBehavior, InitiatorCommand, InitiatorEvent, PromotionBehavior, PromotionConfig,
    },
};
use std::time::Duration;
use tokio::select;
use tracing::{Level, Span, instrument};

pub type BlockBodyData = Vec<u8>;
pub type RequestPort = gasket::messaging::InputPort<DownstreamRequest>;
pub type SyncDataPort = gasket::messaging::OutputPort<ChainSyncEvent>;
pub type BlockDataPort = gasket::messaging::OutputPort<BlockBodyData>;
pub type NetworkMessage = pallas_network2::behavior::AnyMessage;
pub type TcpInterface = pallas_network2::interface::TcpInterface<NetworkMessage>;
pub type BlockRange = (Point, Point);

const DEFAULT_HOUSEKEEPING_INTERVAL: Duration = Duration::from_secs(3);

// preferring conversion via ad-hoc functions instead of implementing `From` to
// avoid coupling other amaru crates with pallas-network2.

fn amaru_to_pallas_point(point: &Point) -> pallas_network2::protocol::Point {
    match point {
        Point::Origin => pallas_network2::protocol::Point::Origin,
        Point::Specific(slot, hash) => {
            pallas_network2::protocol::Point::Specific(*slot, hash.clone())
        }
    }
}

fn pallas_to_amaru_point(point: &pallas_network2::protocol::Point) -> Point {
    match point {
        pallas_network2::protocol::Point::Origin => Point::Origin,
        pallas_network2::protocol::Point::Specific(slot, hash) => {
            Point::Specific(*slot, hash.clone())
        }
    }
}

fn pallas_to_amaru_peer(peer: &PeerId) -> amaru_kernel::peer::Peer {
    amaru_kernel::peer::Peer::new(&peer.to_string())
}

#[derive(Default)]
pub struct WorkUnit {
    /// if set, instruct the network machinery to execute a particular command.
    pub network_cmd: Option<InitiatorCommand>,

    /// if set, dispatch a particular chain sync event to downstream stages.
    pub sync_data: Option<ChainSyncEvent>,

    /// if set, dispatch a particular block body data to downstream stages.
    pub block_data: Option<BlockBodyData>,
}

impl From<InitiatorCommand> for WorkUnit {
    fn from(command: InitiatorCommand) -> Self {
        Self {
            network_cmd: Some(command),
            sync_data: None,
            block_data: None,
        }
    }
}

/// A request from a downstream stage
#[derive(Debug, Clone)]
pub enum DownstreamRequest {
    /// A request to fetch a particular block range.
    FetchBlock(BlockRange),
}

pub struct Config {
    /// The chain-sync intersection to ask to every hot peer.
    pub intersection: Vec<Point>,

    /// To configure the "promotion" behavior (eg: hot, warm, cold peers).
    pub promotion: PromotionConfig,

    /// The initial set of peers to connect to (aka: bootstrap nodes).
    pub initial_peers: Vec<PeerId>,

    /// The interval at which the housekeeping task is run (defaults to 3 secs).
    pub housekeeping: Option<Duration>,
}

#[derive(Stage)]
// TODO: revisit stage name. I would prefer something like "network", "p2p", etc. I'll leave the
// decision for maintainers since it might affect tracing.
#[stage(name = "stage.chain_sync_client", unit = "WorkUnit", worker = "Worker")]
pub struct Stage {
    /// To receive downstream requests.
    pub requests: RequestPort,

    /// To output chain sync data.
    pub sync_data: SyncDataPort,

    /// To output block body data.
    pub block_data: BlockDataPort,

    config: Config,
}

impl Stage {
    pub fn new(config: Config) -> Self {
        Self {
            requests: Default::default(),
            sync_data: Default::default(),
            block_data: Default::default(),
            config,
        }
    }
}

pub struct Worker {
    pub network: Manager<TcpInterface, InitiatorBehavior, NetworkMessage>,
    pub housekeeping: tokio::time::Interval,
}

impl Worker {
    fn handle_network(&self, event: Option<InitiatorEvent>) -> WorkSchedule<WorkUnit> {
        let Some(event) = event else {
            return WorkSchedule::Idle;
        };

        match event {
            // This is triggered when a new peer has connected and initialized (aka: handshake
            // complete). We don't need to do anything here, just trace the event and potentially
            // update health stats.
            InitiatorEvent::PeerInitialized(pid, version) => {
                let (version, _) = version;
                tracing::info!(%pid, version, "peer initialized");
                WorkSchedule::Idle
            }

            // This is triggered when a particular peer has found an intersection point. Please note
            // that this could happen at any point in time, not just when bootstrapping the stage.
            // For example, ff a peer is promoted from warm -> hot, the network machinery will try
            // to intersect it.
            //
            // For now, we just take the naive behavior of keep progressing the chain-sync process.
            // A more sophisticated behavior could evaluate the actual progress of the node and
            // decide if the peer needs to move forward.
            //
            // This could also be a good place to notify downstream peers of newly found tip values.
            InitiatorEvent::IntersectionFound(pid, _, _) => {
                tracing::info!(%pid, "intersection found");
                WorkSchedule::Unit(InitiatorCommand::ContinueSync(pid).into())
            }

            // This is triggered when a block header has been received from a peer that is
            // performing chain-sync. Please note that peers might be going at different speeds or
            // just lagging behind because they connected after than others.
            //
            // For now, whenever we receive a block header, we just dispatch it downstream
            // and keep progressing the chain-sync protocol with this particular peer.
            //
            // More sophisticated behaviors might need to decide what to do with the peer depending
            // on back-pressure from downstream stages that need to process the header.
            InitiatorEvent::BlockHeaderReceived(pid, header, tip) => {
                tracing::info!(%pid, "block header received");

                let work = WorkUnit {
                    sync_data: Some(ChainSyncEvent::RollForward {
                        peer: pallas_to_amaru_peer(&pid),
                        raw_header: header.cbor,
                        point: todo!(),
                        span: Span::current(),
                    }),
                    network_cmd: Some(InitiatorCommand::ContinueSync(pid.clone())),
                    ..Default::default()
                };

                WorkSchedule::Unit(work)
            }

            // This is triggered when a rollback has been received from a peer that is performing
            // chain-sync. Comments from the block header received event apply here as well.
            InitiatorEvent::RollbackReceived(pid, point, _) => {
                tracing::info!(%pid, "rollback received");

                let work = WorkUnit {
                    sync_data: Some(ChainSyncEvent::Rollback {
                        peer: pallas_to_amaru_peer(&pid),
                        rollback_point: pallas_to_amaru_point(&point),
                        span: Span::current(),
                    }),
                    network_cmd: Some(InitiatorCommand::ContinueSync(pid.clone())),
                    ..Default::default()
                };

                WorkSchedule::Unit(work)
            }

            // This is triggered when a block body has been received from a peer that is handling a
            // block-fetch request. The network machinery explicitly avoids decoding the body for
            // performance reasons. It's up for us (amaru) to decide when / how to execute the
            // decoding.
            //
            // The downside of this approach is that we don't have much information about
            // the block at this point, just the payload. We can't infer which point in the chain or
            // to which downstream request it belongs to. The behavior here is to just dispatch the
            // block body downstream and let downstream stages decide how to handle it.
            //
            // There's no need for explicit commands back to the network machinery since the
            // continuation of the mini-protocol is unambiguous. If the fetch request involved more
            // blocks, the network machinery will keep receiving and notifying us of new
            // block bodies.
            InitiatorEvent::BlockBodyReceived(pid, body) => {
                tracing::info!(%pid, "block body received");

                let work = WorkUnit {
                    block_data: Some(body),
                    ..Default::default()
                };

                WorkSchedule::Unit(work)
            }

            // There're other events that the network can trigger, but we don't care about them for
            // now.
            _ => WorkSchedule::Idle,
        }
    }

    fn handle_request(&self, request: &DownstreamRequest) -> WorkSchedule<WorkUnit> {
        match request {
            DownstreamRequest::FetchBlock(range) => {
                let (start, end) = range;
                let range = (amaru_to_pallas_point(&start), amaru_to_pallas_point(&end));
                let cmd = InitiatorCommand::RequestBlocks(range);
                WorkSchedule::Unit(cmd.into())
            }
        }
    }
}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<Stage> for Worker {
    async fn bootstrap(stage: &Stage) -> Result<Self, WorkerError> {
        let behavior = InitiatorBehavior {
            promotion: PromotionBehavior::new(stage.config.promotion),
            // The network machinery supports configuration for other parts of the behavior. I
            // believe defaults are enough for now, maintainers should revisit the options available
            // that decide what needs to be "surfaced" to the root node configuration.
            ..Default::default()
        };

        let interface = TcpInterface::new();

        let mut network = Manager::new(interface, behavior);

        for peer in stage.config.initial_peers.iter() {
            network.execute(InitiatorCommand::IncludePeer(peer.clone()));
        }

        let intersection = stage
            .config
            .intersection
            .iter()
            .map(|p| amaru_to_pallas_point(p))
            .collect();

        network.execute(InitiatorCommand::StartSync(intersection));

        let interval = stage
            .config
            .housekeeping
            .unwrap_or(DEFAULT_HOUSEKEEPING_INTERVAL);

        let worker = Self {
            housekeeping: tokio::time::interval(interval),
            network,
        };

        Ok(worker)
    }

    async fn schedule(&mut self, stage: &mut Stage) -> Result<WorkSchedule<WorkUnit>, WorkerError> {
        // One of three things can happen here:
        // - housekeeping tick: this is triggered at approximate intervals to allow the
        //   network machinery to perform housekeeping tasks such as promoting peers,
        //   sending keep-alive messages, etc.
        // - network event: this is triggered when the network machinery has changed
        //   somehow. This could be be triggered by peers connecting / disconnecting,
        //   inbound messages arriving, etc.
        // - downstream request: this is triggered when a downstream stage sends a
        //   explicit request to this stage. For now, this only supports fetching block
        //   ranges.

        select! {
            _ = self.housekeeping.tick() => {
                tracing::trace!("housekeeping tick");
                Ok(WorkSchedule::Unit(InitiatorCommand::Housekeeping.into()))
            }
            evt = self.network.poll_next() => {
                let schedule = self.handle_network(evt);
                Ok(schedule)
            }
            req = stage.requests.recv() => {
                let req = req.or_panic()?;
                let schedule = self.handle_request(&req.payload);
                Ok(schedule)
            }
        }
    }

    #[instrument(
        level = Level::TRACE,
        name = "stage.pull",
        skip_all,
    )]
    async fn execute(&mut self, unit: &WorkUnit, stage: &mut Stage) -> Result<(), WorkerError> {
        if let Some(cmd) = unit.network_cmd.clone() {
            self.network.execute(cmd);
        }

        if let Some(data) = unit.sync_data.clone() {
            send!(stage.sync_data, data);
        }

        if let Some(data) = unit.block_data.clone() {
            send!(stage.block_data, data);
        }

        Ok(())
    }
}
