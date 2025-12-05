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

use crate::tx_submission::{new_era_tx_body, new_era_tx_id, tx_id_from_era_tx_id};
use crate::{
    chain_sync_client::{ChainSyncClient, to_traverse},
    point::{from_network_point, to_network_point},
};
use acto::{AcTokioRuntime, ActoCell, ActoInput};
use amaru_kernel::tx_submission_events::TxServerRequest;
use amaru_kernel::{
    BlockHeader, IsHeader, Point, TxClientReply,
    connection::{BlockSender, ClientConnectionError, ConnMsg},
    consensus_events::{ChainSyncEvent, Tracked},
    peer::Peer,
};
use amaru_ouroboros::ChainStore;
use futures_util::FutureExt;
use pallas_network::miniprotocols::txsubmission::{Request, TxIdAndSize};
use pallas_network::{
    facades::PeerClient,
    miniprotocols::{
        blockfetch,
        chainsync::{HeaderContent, NextResponse},
    },
};
use pure_stage::BoxFuture;
use std::{collections::VecDeque, future::pending, sync::Arc, time::Duration};
use tokio::{sync::mpsc, task::JoinHandle, time::sleep};
use tracing::Span;

/// The actor that handles the connection to a peer.
///
/// NOTE: This is a stop-gap solution for using the pallas_network facilities
/// such that upstream connections may fail and be reestablished without tearing
/// down the node.
///
/// The actor basically oscillates between the two states:
/// - trying to establish a connection to the peer
/// - being connected to the peer and syncing headers / fetching blocks / replying to tx-requests
///
/// When a disconnect happens (either due to a network error or the ConnMsg::Disconnect message)
/// the actor will drop the connection, wait for 10 seconds, and then try to reconnect.
pub async fn actor(
    mut cell: ActoCell<ConnMsg, AcTokioRuntime>,
    peer: Peer,
    magic: u64,
    store: Arc<dyn ChainStore<BlockHeader>>,
    chain_sync_event_sender: mpsc::Sender<Tracked<ChainSyncEvent>>,
    tx_server_request_sender: mpsc::Sender<TxServerRequest>,
    mut tx_client_reply_receiver: mpsc::Receiver<TxClientReply>,
) -> anyhow::Result<()> {
    let mut req = VecDeque::new();

    loop {
        // connect to the peer (retry after 10sec if it fails)
        let Ok(PeerClient {
            plexer,
            chainsync,
            blockfetch,
            mut txsubmission,
            ..
        }) = PeerClient::connect(peer.name.as_str(), magic).await
        else {
            tracing::error!(peer = %peer.name, "connection_failed");
            sleep(Duration::from_secs(10)).await;
            continue;
        };

        enum State {
            Idle(blockfetch::Client),
            Running(BoxFuture<'static, blockfetch::Client>),
        }

        // spawn task for handling the chainsync protocol
        let mut fetch = State::Idle(blockfetch);
        let peer_clone = peer.clone();
        let chain_sync_event_tx_clone = chain_sync_event_sender.clone();
        let intersection = {
            let hashes = [store.get_anchor_hash(), store.get_best_chain_hash()];
            hashes
                .iter()
                .filter_map(|h| store.load_header(h).map(|h| h.point()))
                .collect()
        };

        let mut sync: JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
            let mut chainsync = ChainSyncClient::new(peer_clone.clone(), chainsync, intersection);
            let point = chainsync.find_intersection().await.inspect_err(|e| {
                tracing::error!(peer=%peer_clone, error=%e, "intersection.not_found");
            })?;
            tracing::debug!(peer=%peer_clone, %point, "intersection.found");
            loop {
                match chainsync.request_next().await? {
                    NextResponse::RollForward(hd, _tip) => {
                        roll_forward(&chain_sync_event_tx_clone, &peer_clone, hd).await?
                    }
                    NextResponse::RollBackward(point, _tip) => {
                        roll_back(&chain_sync_event_tx_clone, &peer_clone, point).await?
                    }
                    NextResponse::Await => {
                        chain_sync_event_tx_clone
                            .send(Tracked::CaughtUp {
                                peer: peer_clone.clone(),
                                span: Span::current(),
                            })
                            .await?;
                        match chainsync.await_next().await? {
                            NextResponse::RollForward(hd, _tip) => {
                                roll_forward(&chain_sync_event_tx_clone, &peer_clone, hd).await?
                            }
                            NextResponse::RollBackward(point, _tip) => {
                                roll_back(&chain_sync_event_tx_clone, &peer_clone, point).await?
                            }
                            NextResponse::Await => unreachable!(),
                        }
                    }
                }
            }
        });

        // init tx-submission mini-protocol
        if txsubmission.send_init().await.is_err() {
            tracing::error!(%peer, "disconnecting.network_error");
            plexer.abort().await;
            sleep(Duration::from_secs(10)).await;
            continue;
        };

        let tx_request_tx_clone = tx_server_request_sender.clone();
        let peer_clone = peer.clone();

        // main loop handling
        // - chainsync errors → disconnect
        // - tx_submission errors → disconnect
        // - blockfetch results → fetch next block if applicable
        // - commands to FetchBlock or Disconnect coming in via mailbox
        loop {
            // keep select!() below uniform, no matter whether currently fetching or not
            let mut may_fetch = if let State::Running(f) = &mut fetch {
                // left_future/right_future for merging to different Future types without boxing
                f.left_future()
            } else {
                // while not fetching, construct a Future that won't resolve
                pending().right_future()
            };
            let msg = tokio::select! {
                // chainsync task died -> disconnect
                res = &mut sync => {
                    tracing::error!(?res, %peer, "disconnecting.network_error");
                    plexer.abort().await;
                    sleep(Duration::from_secs(10)).await;
                    break;
                },

                // tx-submission: incoming requests from peer
                req = txsubmission.next_request() => {
                    let req = req?; // propagate error -> disconnect
                    match req {
                        Request::TxIds(ack, req) => {
                            tx_request_tx_clone
                                .send(TxServerRequest::TxIds {
                                    peer: peer_clone.clone(),
                                    ack,
                                    req,
                                    span: Span::current(),
                                })
                                .await?;
                        }
                        Request::TxIdsNonBlocking(ack, req) => {
                            tx_request_tx_clone
                                .send(TxServerRequest::TxIdsNonBlocking {
                                    peer: peer_clone.clone(),
                                    ack,
                                    req,
                                    span: Span::current(),
                                })
                                .await?;
                        }
                        Request::Txs(tx_ids) => {
                            tx_request_tx_clone
                                .send(TxServerRequest::Txs {
                                    peer: peer_clone.clone(),
                                    tx_ids: tx_ids.iter().map(tx_id_from_era_tx_id).collect(),
                                    span: Span::current(),
                                })
                                .await?;
                        }
                    }
                    continue;
                },

                // tx-submission: outgoing replies from node
                reply = tx_client_reply_receiver.recv() => {
                    match reply {
                        Some(TxClientReply::Init { .. }) => {
                            txsubmission
                                .send_init()
                                .await?;
                        }
                        Some(TxClientReply::TxIds { tx_ids, .. }) => {
                            txsubmission
                                .reply_tx_ids(
                                    tx_ids
                                        .into_iter()
                                        .map(|(tx_id, size)| TxIdAndSize(new_era_tx_id(tx_id), size))
                                        .collect(),
                                )
                                .await?;
                        }
                        Some(TxClientReply::Txs { txs, .. }) => {
                            txsubmission
                                .reply_txs(
                                    txs.iter()
                                        .map(new_era_tx_body)
                                        .collect(),
                                )
                                .await?;
                        }
                        None => {
                            // channel closed -> disconnect
                            return Err(anyhow::anyhow!("tx_reply channel closed"));
                        }
                    }
                    continue;
                },

                // existing blockfetch case
                blockfetch = &mut may_fetch => {
                    if let Some((point, tx)) = req.pop_front() {
                        fetch = State::Running(do_fetch(blockfetch, point, tx, peer.clone()))
                    } else {
                        fetch = State::Idle(blockfetch)
                    }
                    continue;
                },

                // mailbox input
                msg = cell.recv() => msg,
            };

            match msg {
                ActoInput::NoMoreSenders => {
                    // this won't actually happen with the current NetworkResource because that
                    // never drops the ActoRef, but who knows what the future holds...
                    plexer.abort().await;
                    sync.abort();
                    return Ok(());
                }
                ActoInput::Supervision { .. } => unreachable!(),
                ActoInput::Message(ConnMsg::FetchBlock(point, tx)) => match fetch {
                    State::Running(_) => req.push_back((point, tx)),
                    State::Idle(blockfetch) => {
                        fetch = State::Running(do_fetch(blockfetch, point, tx, peer.clone()))
                    }
                },
                ActoInput::Message(ConnMsg::Disconnect) => {
                    tracing::warn!(%peer, "disconnecting.node_policy");
                    plexer.abort().await;
                    sync.abort();
                    sleep(Duration::from_secs(10)).await;
                    break;
                }
            }
        }
    }
}

async fn roll_forward(
    chain_sync_event_tx: &mpsc::Sender<Tracked<ChainSyncEvent>>,
    peer: &Peer,
    hd: HeaderContent,
) -> anyhow::Result<()> {
    let hd = to_traverse(&hd)?;
    chain_sync_event_tx
        .send(Tracked::Wrapped(ChainSyncEvent::RollForward {
            peer: peer.clone(),
            point: Point::Specific(hd.slot(), hd.hash().to_vec()),
            raw_header: hd.cbor().to_vec(),
            span: Span::current(),
        }))
        .await?;
    Ok(())
}

async fn roll_back(
    chain_sync_event_tx: &mpsc::Sender<Tracked<ChainSyncEvent>>,
    peer: &Peer,
    point: pallas_network::miniprotocols::Point,
) -> anyhow::Result<()> {
    chain_sync_event_tx
        .send(Tracked::Wrapped(ChainSyncEvent::Rollback {
            peer: peer.clone(),
            rollback_point: from_network_point(&point),
            span: Span::current(),
        }))
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
                tracing::error!(%peer, %point, %err, "fetch_block.failed");
            });
        let tx = tx.lock().take();
        if let Some(tx) = tx {
            tx.send(body.map_err(|e| ClientConnectionError::new(e.into())))
                .ok();
        }
        blockfetch
    })
}
