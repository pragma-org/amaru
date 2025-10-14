use crate::{point::from_network_point, stages::to_pallas_point};
use acto::{AcTokioRuntime, ActoCell, ActoInput, ActoRef, ActoRuntime, variable::Reader};
use amaru_consensus::{BlockFetchClientError, consensus::events::ChainSyncEvent};
use amaru_kernel::{Point, peer::Peer};
use amaru_network::chain_sync_client::{ChainSyncClient, to_traverse};
use futures_util::FutureExt;
use pallas_network::{
    facades::PeerClient,
    miniprotocols::{
        blockfetch,
        chainsync::{HeaderContent, NextResponse},
    },
};
use parking_lot::Mutex;
use pure_stage::{BoxFuture, ExternalEffect, ExternalEffectAPI, Resources, SendData};
use rand::{rng, seq::IteratorRandom};
use std::{
    collections::{HashMap, VecDeque},
    future::pending,
    ops::Deref,
    sync::Arc,
    time::Duration,
};
use tokio::{
    select,
    sync::{
        Mutex as AsyncMutex,
        mpsc::{Receiver, Sender, channel},
        oneshot,
    },
    task::JoinHandle,
    time::sleep,
};
use tracing::Span;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChainSyncEffect {}

impl ExternalEffectAPI for ChainSyncEffect {
    type Response = ChainSyncEvent;
}

impl ExternalEffect for ChainSyncEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            let network = resources
                .get::<NetworkResource>()
                .expect("ChainSyncEffect requires a NetworkResource")
                .clone();
            network.next_sync().await
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BlockFetchEffect {
    peer: Peer,
    point: Point,
}

impl ExternalEffectAPI for BlockFetchEffect {
    type Response = Result<Vec<u8>, BlockFetchClientError>;
}

impl BlockFetchEffect {
    pub fn new(peer: &Peer, point: &Point) -> Self {
        Self {
            peer: peer.clone(),
            point: point.clone(),
        }
    }
}

impl ExternalEffect for BlockFetchEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            let network = resources
                .get::<NetworkResource>()
                .expect("BlockFetchEffect requires a NetworkResource")
                .clone();
            network.fetch_block(&self.peer, self.point).await
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DisconnectEffect {
    peer: Peer,
}

impl ExternalEffectAPI for DisconnectEffect {
    type Response = ();
}

impl DisconnectEffect {
    pub fn new(peer: &Peer) -> Self {
        Self { peer: peer.clone() }
    }
}

impl ExternalEffect for DisconnectEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            let network = resources
                .get::<NetworkResource>()
                .expect("DisconnectEffect requires a NetworkResource")
                .clone();
            network.disconnect(&self.peer).await
        })
    }
}

#[derive(Clone)]
pub struct NetworkResource {
    inner: Arc<NetworkInner>,
}

impl Deref for NetworkResource {
    type Target = NetworkInner;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl NetworkResource {
    pub fn new(
        peers: impl IntoIterator<Item = Peer>,
        rt: &AcTokioRuntime,
        magic: u64,
        intersection: Reader<Vec<Point>>,
    ) -> Self {
        let (hd_tx, hd_rx) = channel(100);
        let connections = peers
            .into_iter()
            .map(|peer| {
                (
                    peer.clone(),
                    rt.spawn_actor(&format!("conn-{}", peer), |cell| {
                        connection(cell, peer, magic, hd_tx.clone(), intersection.clone())
                    })
                    .me,
                )
            })
            .collect();
        Self {
            inner: Arc::new(NetworkInner {
                connections,
                hd_rx: AsyncMutex::new(hd_rx),
            }),
        }
    }
}

pub struct NetworkInner {
    connections: HashMap<Peer, ActoRef<ConnMsg>>,
    hd_rx: AsyncMutex<Receiver<ChainSyncEvent>>,
}

impl NetworkInner {
    pub async fn next_sync(&self) -> ChainSyncEvent {
        self.hd_rx
            .lock()
            .await
            .recv()
            .await
            .expect("upstream funnel will never stop")
    }

    pub async fn fetch_block(
        &self,
        peer: &Peer,
        point: Point,
    ) -> Result<Vec<u8>, BlockFetchClientError> {
        let (tx, rx) = oneshot::channel();
        let tx = Arc::new(Mutex::new(Some(tx)));
        if let Some(peer) = self.connections.get(peer) {
            peer.send(ConnMsg::FetchBlock(point.clone(), tx.clone()));
        }
        for p in self.connections.keys().choose_multiple(&mut rng(), 3) {
            if peer != p {
                self.connections[p].send(ConnMsg::FetchBlock(point.clone(), tx.clone()));
            }
        }
        drop(tx);
        // if no sends were made then the drop above ensures that the below errors instead of deadlock
        rx.await
            .map_err(|e| BlockFetchClientError::new(e.into()))
            .flatten()
    }

    pub async fn disconnect(&self, peer: &Peer) {
        if let Some(p) = self.connections.get(peer) {
            p.send_wait(ConnMsg::Disconnect).await;
        }
    }
}

type BlockSender = Arc<Mutex<Option<oneshot::Sender<Result<Vec<u8>, BlockFetchClientError>>>>>;

pub enum ConnMsg {
    FetchBlock(Point, BlockSender),
    Disconnect,
}

async fn connection(
    mut cell: ActoCell<ConnMsg, AcTokioRuntime>,
    peer: Peer,
    magic: u64,
    hd_tx: Sender<ChainSyncEvent>,
    intersection: Reader<Vec<Point>>,
) {
    let mut req = VecDeque::new();

    loop {
        let Ok(PeerClient {
            plexer,
            chainsync,
            blockfetch,
            ..
        }) = PeerClient::connect(peer.name.as_str(), magic).await
        else {
            tracing::error!(peer = %peer.name, "failed to connect to peer");
            sleep(Duration::from_secs(10)).await;
            continue;
        };

        enum State {
            Idle(blockfetch::Client),
            Running(BoxFuture<'static, blockfetch::Client>),
        }

        let mut fetch = State::Idle(blockfetch);
        let peer2 = peer.clone();
        let hd_tx = hd_tx.clone();
        let intersection = intersection.get_cloned();
        let mut sync: JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
            let mut chainsync = ChainSyncClient::new(peer2.clone(), chainsync, intersection);
            let point = chainsync.find_intersection().await.inspect_err(|e| {
                tracing::error!("no intersection found with {}: {}", peer2, e);
            })?;
            tracing::debug!("Intersection found with {}: {:?}", peer2, point);
            loop {
                match chainsync.request_next().await? {
                    NextResponse::RollForward(hd, _tip) => roll_forward(&hd_tx, &peer2, hd).await?,
                    NextResponse::RollBackward(point, _tip) => {
                        roll_back(&hd_tx, &peer2, point).await?
                    }
                    NextResponse::Await => {
                        hd_tx
                            .send(ChainSyncEvent::CaughtUp {
                                peer: peer2.clone(),
                                span: Span::current(),
                            })
                            .await?;
                        match chainsync.await_next().await? {
                            NextResponse::RollForward(hd, _tip) => {
                                roll_forward(&hd_tx, &peer2, hd).await?
                            }
                            NextResponse::RollBackward(point, _tip) => {
                                roll_back(&hd_tx, &peer2, point).await?
                            }
                            NextResponse::Await => unreachable!(),
                        }
                    }
                }
            }
        });

        loop {
            let mut may_fetch = if let State::Running(f) = &mut fetch {
                f.left_future()
            } else {
                pending().right_future()
            };
            let msg = select! {
                res = &mut sync => {
                    tracing::error!(?res, %peer, "disconnecting due to network error");
                    plexer.abort().await;
                    sleep(Duration::from_secs(10)).await;
                    break;
                },
                blockfetch = &mut may_fetch => {
                    if let Some((point, tx)) = req.pop_front() {
                        fetch = State::Running(do_fetch(blockfetch, point, tx, peer.clone()))
                    } else {
                        fetch = State::Idle(blockfetch)
                    }
                    continue;
                }
                msg = cell.recv() => msg,
            };
            match msg {
                ActoInput::NoMoreSenders => return,
                ActoInput::Supervision { .. } => unreachable!(),
                ActoInput::Message(ConnMsg::FetchBlock(point, tx)) => match fetch {
                    State::Running(_) => req.push_back((point, tx)),
                    State::Idle(blockfetch) => {
                        fetch = State::Running(do_fetch(blockfetch, point, tx, peer.clone()))
                    }
                },
                ActoInput::Message(ConnMsg::Disconnect) => {
                    tracing::warn!(%peer, "disconnecting due to node policy");
                    plexer.abort().await;
                    sleep(Duration::from_secs(10)).await;
                    break;
                }
            }
        }
    }
}

async fn roll_forward(
    hd_tx: &Sender<ChainSyncEvent>,
    peer: &Peer,
    hd: HeaderContent,
) -> anyhow::Result<()> {
    let hd = to_traverse(&hd)?;
    hd_tx
        .send(ChainSyncEvent::RollForward {
            peer: peer.clone(),
            point: Point::Specific(hd.slot(), hd.hash().to_vec()),
            raw_header: hd.cbor().to_vec(),
            span: Span::current(),
        })
        .await?;
    Ok(())
}

async fn roll_back(
    hd_tx: &Sender<ChainSyncEvent>,
    peer: &Peer,
    point: pallas_network::miniprotocols::Point,
) -> anyhow::Result<()> {
    hd_tx
        .send(ChainSyncEvent::Rollback {
            peer: peer.clone(),
            rollback_point: from_network_point(&point),
            span: Span::current(),
        })
        .await?;
    Ok(())
}

fn do_fetch(
    mut blockfetch: blockfetch::Client,
    point: Point,
    tx: BlockSender,
    peer: Peer,
) -> BoxFuture<'static, blockfetch::Client> {
    Box::pin(async move {
        let body = blockfetch
            .fetch_single(to_pallas_point(&point))
            .await
            .inspect_err(|err| {
                tracing::error!(%peer, %point, %err, "fetch block failed");
            });
        let tx = tx.lock().take();
        if let Some(tx) = tx {
            tx.send(body.map_err(|e| BlockFetchClientError::new(e.into())))
                .ok();
        }
        blockfetch
    })
}
