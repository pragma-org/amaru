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

use acto::{AcTokio, ActoCell, ActoMsgSuper, ActoRef, ActoRuntime};
use amaru_consensus::{consensus::store::ChainStore, IsHeader};
use amaru_kernel::{Hash, Header};
use amaru_ledger::BlockValidationResult;
use client_protocol::{client_protocols, ClientProtocolMsg};
use client_state::{to_pallas_point, ClientOp};
use gasket::framework::*;
use pallas_network::{
    facades::PeerServer,
    miniprotocols::{chainsync::Tip, Point},
};
use std::{cell::RefCell, collections::HashMap, sync::Arc};
use tokio::{
    net::TcpListener,
    sync::{
        mpsc::{self, Receiver},
        Mutex,
    },
    task::JoinHandle,
};
use tracing::{error, info, trace_span};

pub type UpstreamPort = gasket::messaging::InputPort<BlockValidationResult>;

pub const EVENT_TARGET: &str = "amaru::consensus::chain_forward";

/// Forwarding stage of the consensus where blocks are stored and made
/// available to downstream peers.
///
/// TODO: currently does nothing, should store block, update chain state, and
/// forward new chain downstream

#[derive(Stage)]
#[stage(name = "consensus.forward", unit = "Unit", worker = "Worker")]
pub struct ForwardStage {
    pub store: Arc<Mutex<dyn ChainStore<Header>>>,
    pub upstream: UpstreamPort,
    pub network_magic: u64,
    pub runtime: AcTokio,
    pub listen_address: String,
    pub downstream: Option<ActoRef<ForwardEvent>>,
}

#[derive(Debug, Clone)]
pub enum ForwardEvent {
    Listening(u16),
    Forward(Point),
}

impl ForwardStage {
    pub fn new(
        downstream: Option<ActoRef<ForwardEvent>>,
        store: Arc<Mutex<dyn ChainStore<Header>>>,
        network_magic: u64,
        listen_address: &str,
    ) -> Self {
        #[allow(clippy::expect_used)]
        let runtime =
            AcTokio::new("consensus.forward", 1).expect("failed to create AcTokio runtime");
        Self {
            store,
            upstream: Default::default(),
            network_magic,
            runtime,
            listen_address: listen_address.to_string(),
            downstream,
        }
    }
}

pub enum Unit {
    Peer(RefCell<Option<PeerServer>>),
    Block(BlockValidationResult),
}

pub struct Worker {
    server: JoinHandle<()>,
    incoming_peers: Receiver<PeerServer>,
    tip: Tip,
    clients: ActoRef<ClientMsg>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<ForwardStage> for Worker {
    async fn bootstrap(stage: &ForwardStage) -> Result<Self, WorkerError> {
        let server = TcpListener::bind(&stage.listen_address).await.or_panic()?;
        if let Some(downstream) = &stage.downstream {
            tracing::debug!("sending listening event");
            downstream.send(ForwardEvent::Listening(
                server.local_addr().or_panic()?.port(),
            ));
        }

        let (tx, incoming_peers) = mpsc::channel(10);

        let clients = stage
            .runtime
            .spawn_actor("chain_forward", |cell| {
                client_supervisor(cell, stage.store.clone())
            })
            .me;

        let network_magic = stage.network_magic;
        let server = tokio::spawn(async move {
            loop {
                // due to the signature of TcpListener::accept, this is the only way to use this API
                // in particular, it isn’t possible to poll for new peers within the `schedule` method
                let peer = match PeerServer::accept(&server, network_magic).await {
                    Ok(peer) => peer,
                    Err(e) => {
                        tracing::warn!(
                            target: EVENT_TARGET,
                            "error accepting peer: {e}",
                        );
                        continue;
                    }
                };

                match tx.send(peer).await {
                    Ok(_) => {}
                    Err(e) => {
                        tracing::info!(
                            target: EVENT_TARGET,
                            "dropping incoming connection: {e}"
                        );
                    }
                }
            }
        });

        Ok(Self {
            server,
            incoming_peers,
            tip: Tip(Point::Origin, 0),
            clients,
        })
    }

    async fn schedule(
        &mut self,
        stage: &mut ForwardStage,
    ) -> Result<WorkSchedule<Unit>, WorkerError> {
        tokio::select! {
            block = stage.upstream.recv() => Ok(WorkSchedule::Unit(Unit::Block(block.or_panic()?.payload))),
            peer = self.incoming_peers.recv() => {
                match peer {
                    Some(peer) => Ok(WorkSchedule::Unit(Unit::Peer(RefCell::new(Some(peer))))),
                    None => Err(WorkerError::Panic),
                }
            }
        }
    }

    async fn execute(&mut self, unit: &Unit, stage: &mut ForwardStage) -> Result<(), WorkerError> {
        match unit {
            Unit::Block(BlockValidationResult::BlockValidated(point, span)) => {
                // FIXME: this span is just a placeholder to hold a link to t
                // the parent, it will be filled once we had the storage and
                // forwarding logic.
                let _span = trace_span!(
                    target: EVENT_TARGET,
                    parent: span,
                    "forward.block_validated",
                    slot = ?point.slot_or_default(),
                    hash = %Hash::<32>::from(point),
                );

                // FIXME: block height should be part of BlockValidated message
                let store = stage.store.lock().await;
                if let Some(header) = store.load_header(&Hash::from(point)) {
                    self.tip = Tip(to_pallas_point(point), header.block_height());
                    self.clients.send(ClientMsg::Op(ClientOp::Forward(header)));
                }

                if let Some(downstream) = &stage.downstream {
                    downstream.send(ForwardEvent::Forward(to_pallas_point(point)));
                }

                Ok(())
            }
            Unit::Block(BlockValidationResult::RolledBackTo(point, span)) => {
                info!(
                    target: EVENT_TARGET,
                    parent: span,
                    slot = ?point.slot_or_default(),
                    hash = %Hash::<32>::from(point),
                    "rolled_back_to"
                );

                // FIXME: block height should be part of BlockValidated message
                let store = stage.store.lock().await;
                if let Some(header) = store.load_header(&Hash::from(point)) {
                    self.tip = Tip(to_pallas_point(point), header.block_height());
                    self.clients
                        .send(ClientMsg::Op(ClientOp::Backward(to_pallas_point(point))));
                }

                Ok(())
            }
            Unit::Block(BlockValidationResult::BlockValidationFailed(point, span)) => {
                error!(
                    target: EVENT_TARGET,
                    parent: span,
                    slot = ?point.slot_or_default(),
                    hash = %Hash::<32>::from(point),
                    "block_validation_failed"
                );

                Ok(())
            }
            Unit::Peer(peer) => {
                // FIXME: gasket design bug that we only get &Unit and thus cannot take values from it without internal mutability
                let peer = peer.borrow_mut().take();
                if let Some(peer) = peer {
                    self.clients.send(ClientMsg::Peer(peer, self.tip.clone()));
                } else {
                    tracing::error!(target: EVENT_TARGET, "Unit::Peer was empty in execute");
                }
                Ok(())
            }
        }
    }
}

#[allow(clippy::large_enum_variant)]
enum ClientMsg {
    Peer(PeerServer, Tip),
    Op(ClientOp),
}

async fn client_supervisor(
    mut cell: ActoCell<ClientMsg, impl ActoRuntime, Result<(), client_protocol::ClientError>>,
    store: Arc<Mutex<dyn ChainStore<Header>>>,
) {
    let mut clients = HashMap::new();
    while let Some(msg) = cell.recv().await.has_senders() {
        match msg {
            ActoMsgSuper::Message(ClientMsg::Peer(peer, tip)) => {
                let addr = peer
                    .accepted_address()
                    .map(|a| a.to_string())
                    .unwrap_or_default();

                let client = cell.spawn_supervised(&addr, {
                    let store = store.clone();
                    move |cell| client_protocols(cell, peer, store, tip)
                });
                clients.insert(client.id(), client);
            }
            ActoMsgSuper::Message(ClientMsg::Op(op)) => {
                for client in clients.values() {
                    client.send(ClientProtocolMsg::Op(op.clone()));
                }
            }
            ActoMsgSuper::Supervision { id, name, result } => {
                tracing::info!(target: EVENT_TARGET, "client {} terminated: {:?}", name, result);
                clients.remove(&id);
            }
        }
    }
}

mod client_protocol;
mod client_state;

#[cfg(test)]
mod tests;
