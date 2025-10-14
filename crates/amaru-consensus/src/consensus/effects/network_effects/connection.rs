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

use super::{BlockSender, ConnMsg};
use crate::consensus::events::ChainSyncEvent;
use acto::{AcTokioRuntime, ActoCell, ActoInput, variable::Reader};
use amaru_kernel::{Point, peer::Peer};
use amaru_network::{
    chain_sync_client::{ChainSyncClient, to_traverse},
    point::{from_network_point, to_network_point},
};
use amaru_ouroboros::BlockFetchClientError;
use futures_util::FutureExt;
use pallas_network::{
    facades::PeerClient,
    miniprotocols::{
        blockfetch,
        chainsync::{HeaderContent, NextResponse},
    },
};
use pure_stage::BoxFuture;
use std::{collections::VecDeque, future::pending, time::Duration};
use tokio::{select, sync::mpsc, task::JoinHandle, time::sleep};
use tracing::Span;

pub async fn actor(
    mut cell: ActoCell<ConnMsg, AcTokioRuntime>,
    peer: Peer,
    magic: u64,
    hd_tx: mpsc::Sender<ChainSyncEvent>,
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
    hd_tx: &mpsc::Sender<ChainSyncEvent>,
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
    hd_tx: &mpsc::Sender<ChainSyncEvent>,
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
            .fetch_single(to_network_point(point.clone()))
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
