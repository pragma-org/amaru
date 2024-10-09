use gasket::framework::*;
use miette::miette;
use pallas_network::facades::PeerClient;
use pallas_network::miniprotocols::chainsync::{
    HeaderContent, NextResponse, RollbackBuffer, RollbackEffect, Tip,
};
use pallas_network::miniprotocols::Point;
use pallas_traverse::{MultiEraBlock, MultiEraHeader};
use tracing::{debug, info};

use super::PullEvent;

fn to_traverse(header: &HeaderContent) -> Result<MultiEraHeader<'_>, WorkerError> {
    let out = match header.byron_prefix {
        Some((subtag, _)) => MultiEraHeader::decode(header.variant, Some(subtag), &header.cbor),
        None => MultiEraHeader::decode(header.variant, None, &header.cbor),
    };

    out.or_panic()
}

pub type DownstreamPort = gasket::messaging::OutputPort<PullEvent>;

pub enum WorkUnit {
    Pull,
    Await,
}

#[derive(Stage)]
#[stage(name = "pull", unit = "WorkUnit", worker = "Worker")]
pub struct Stage {
    peer_address: String,
    network_magic: u32,
    intersection: Vec<Point>,

    pub downstream: DownstreamPort,

    #[metric]
    block_count: gasket::metrics::Counter,

    #[metric]
    chain_tip: gasket::metrics::Gauge,
}

impl Stage {
    pub fn new(peer_address: String, network_magic: u32, intersection: Vec<Point>) -> Self {
        Self {
            peer_address,
            network_magic,
            intersection,
            downstream: Default::default(),
            block_count: Default::default(),
            chain_tip: Default::default(),
        }
    }

    fn track_tip(&self, tip: &Tip) {
        self.chain_tip.set(tip.0.slot_or_default() as i64);
    }
}

pub struct Worker {
    peer_session: PeerClient,
}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<Stage> for Worker {
    async fn bootstrap(stage: &Stage) -> Result<Self, WorkerError> {
        debug!("connecting to peer");

        let mut peer_session = PeerClient::connect(&stage.peer_address, stage.network_magic as u64)
            .await
            .or_retry()?;

        info!(
            address = stage.peer_address,
            magic = stage.network_magic,
            "connected to peer"
        );

        debug!("finding intersect");

        let (point, _) = peer_session
            .chainsync()
            .find_intersect(stage.intersection.clone())
            .await
            .or_restart()?;

        let intersection = point.ok_or(miette!("couldn't find intersect")).or_panic()?;

        info!(?intersection, "found intersection");

        let worker = Self { peer_session };

        Ok(worker)
    }

    async fn schedule(
        &mut self,
        _stage: &mut Stage,
    ) -> Result<WorkSchedule<WorkUnit>, WorkerError> {
        let client = self.peer_session.chainsync();

        if client.has_agency() {
            // should request next block
            Ok(WorkSchedule::Unit(WorkUnit::Pull))
        } else {
            // should await for next block
            Ok(WorkSchedule::Unit(WorkUnit::Await))
        }
    }

    async fn execute(&mut self, unit: &WorkUnit, stage: &mut Stage) -> Result<(), WorkerError> {
        let client = self.peer_session.chainsync();

        let next = match unit {
            WorkUnit::Pull => {
                info!("pulling block batch from upstream peer");

                client.request_next().await.or_restart()?
            }
            WorkUnit::Await => {
                info!("awaiting for new block");

                self.peer_session
                    .chainsync()
                    .recv_while_must_reply()
                    .await
                    .or_restart()?
            }
        };

        match next {
            NextResponse::RollForward(header, tip) => {
                let header = to_traverse(&header).or_panic()?;
                let point = Point::Specific(header.slot(), header.hash().to_vec());

                info!(?point, "new block received from upstream peer");

                let block = self
                    .peer_session
                    .blockfetch()
                    .fetch_single(point.clone())
                    .await
                    .or_restart()?;

                stage
                    .downstream
                    .send(PullEvent::RollForward(point, block).into())
                    .await
                    .or_panic()?;

                stage.track_tip(&tip);
            }
            NextResponse::RollBackward(point, tip) => {
                info!(?point, "rollback sent by upstream peer");

                stage
                    .downstream
                    .send(PullEvent::Rollback(point).into())
                    .await
                    .or_panic()?;

                stage.track_tip(&tip);
            }
            NextResponse::Await => {
                info!("reached tip of the chain");
            }
        };

        Ok(())
    }
}
