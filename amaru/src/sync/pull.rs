use gasket::framework::*;
use miette::miette;
use pallas_network::{
    facades::PeerClient,
    miniprotocols::{
        chainsync::{HeaderContent, NextResponse, Tip},
        Point,
    },
};
use pallas_traverse::MultiEraHeader;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::timeout;
use tracing::{debug, info};

use super::{PullEvent, RawHeader};

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
    peer_session: Arc<Mutex<PeerClient>>,
    intersection: Vec<Point>,

    pub downstream: DownstreamPort,

    #[metric]
    chain_tip: gasket::metrics::Gauge,
}

impl Stage {
    pub fn new(peer_session: Arc<Mutex<PeerClient>>, intersection: Vec<Point>) -> Self {
        Self {
            peer_session,
            intersection,
            downstream: Default::default(),
            chain_tip: Default::default(),
        }
    }

    fn track_tip(&self, tip: &Tip) {
        self.chain_tip.set(tip.0.slot_or_default() as i64);
    }
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<Stage> for Worker {
    async fn bootstrap(stage: &Stage) -> Result<Self, WorkerError> {
        let mut peer_session = stage.peer_session.lock().unwrap();
        let client = (*peer_session).chainsync();

        debug!("finding intersect");

        let (point, _) = client
            .find_intersect(stage.intersection.clone())
            .await
            .or_restart()?;

        let intersection = point.ok_or(miette!("couldn't find intersect")).or_panic()?;

        info!(?intersection, "found intersection");

        let worker = Self {};

        Ok(worker)
    }

    async fn schedule(&mut self, stage: &mut Stage) -> Result<WorkSchedule<WorkUnit>, WorkerError> {
        let mut peer_session = stage.peer_session.lock().unwrap();
        let client = (*peer_session).chainsync();

        if client.has_agency() {
            // should request next block
            Ok(WorkSchedule::Unit(WorkUnit::Pull))
        } else {
            // should await for next block
            Ok(WorkSchedule::Unit(WorkUnit::Await))
        }
    }

    async fn execute(&mut self, unit: &WorkUnit, stage: &mut Stage) -> Result<(), WorkerError> {
        let next = {
            let mut peer_session = stage.peer_session.lock().unwrap();
            let client = (*peer_session).chainsync();

            match unit {
                WorkUnit::Pull => {
                    info!("pulling block batch from upstream peer");

                    client.request_next().await.or_restart()?
                }
                WorkUnit::Await => {
                    info!("awaiting for new block");
                    //FIXME: This isn't ideal to use a timeout because we won't see the block the second
                    // it arrives. Ideally, we could just recv_while_must_reply().await forever, but that
                    // causes worker starvation downstream because they cannot lock() the peer_session.
                    match timeout(Duration::from_secs(1), client.recv_while_must_reply()).await {
                        Ok(result) => result.or_restart()?,
                        Err(_) => Err(WorkerError::Retry)?,
                    }
                }
            }
        };

        match next {
            NextResponse::RollForward(header, tip) => {
                let header = to_traverse(&header).or_panic()?;
                let point = Point::Specific(header.slot(), header.hash().to_vec());

                info!(?point, "new block received from upstream peer");

                let raw_header: RawHeader = header.cbor().to_vec();

                stage
                    .downstream
                    .send(PullEvent::RollForward(point, raw_header).into())
                    .await
                    .or_panic()?;

                info!("block sent downstream");

                stage.track_tip(&tip);
            }
            NextResponse::RollBackward(point, tip) => {
                info!(?point, "rollback sent by upstream peer");

                stage
                    .downstream
                    .send(PullEvent::Rollback(point).into())
                    .await
                    .or_panic()?;

                info!("rollback sent downstream");

                stage.track_tip(&tip);
            }
            NextResponse::Await => {
                info!("reached tip of the chain");
            }
        };

        Ok(())
    }
}
