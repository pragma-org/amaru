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

use crate::stages::{common::adopt_current_span, PallasPoint};
use acto::{AcTokio, ActoCell, ActoMsgSuper, ActoRef, ActoRuntime};
use amaru_consensus::{consensus::store::ChainStore, IsHeader};
use amaru_kernel::{block::BlockValidationResult, Hash, Header};
use client_protocol::{client_protocols, ClientProtocolMsg};
use gasket::framework::*;
use pallas_network::{
    facades::PeerServer,
    miniprotocols::{chainsync::Tip, Point},
};
use std::{cell::RefCell, collections::BTreeMap, sync::Arc};
use tokio::{
    net::TcpListener,
    sync::{
        mpsc::{self, Receiver},
        Mutex,
    },
    task::JoinHandle,
};
use tracing::{error, info, instrument, trace, Level};

pub type UpstreamPort = gasket::messaging::InputPort<BlockValidationResult>;

pub const EVENT_TARGET: &str = "amaru::consensus::chain_forward";

/// Forwarding stage of the consensus where blocks are stored and made
/// available to downstream peers.

#[derive(Stage)]
#[stage(name = "consensus.forward", unit = "Unit", worker = "Worker")]
pub struct ForwardChainStage {
    pub store: Arc<Mutex<dyn ChainStore<Header>>>,
    pub upstream: UpstreamPort,
    pub network_magic: u64,
    pub runtime: AcTokio,
    pub listen_address: String,
    pub downstream: ActoRef<ForwardEvent>,
    pub max_peers: usize,
    pub our_tip: Tip,
}

#[derive(Debug, Clone)]
pub enum ForwardEvent {
    Listening(u16),
    Forward(Point),
    Backward(Point),
}

impl ForwardChainStage {
    pub fn new(
        downstream: Option<ActoRef<ForwardEvent>>,
        store: Arc<Mutex<dyn ChainStore<Header>>>,
        network_magic: u64,
        listen_address: &str,
        max_peers: usize,
        our_tip: Tip,
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
            downstream: downstream.unwrap_or_else(ActoRef::blackhole),
            max_peers,
            our_tip,
        }
    }
}

#[allow(clippy::large_enum_variant)]
pub enum Unit {
    Peer(RefCell<Option<PeerServer>>),
    Block(BlockValidationResult),
}

pub struct Worker {
    server: JoinHandle<()>,
    incoming_peers: Receiver<PeerServer>,
    our_tip: Tip,
    clients: ActoRef<ClientMsg>,
}

impl Worker {
    async fn handle_validation_result(
        &mut self,
        stage: &mut ForwardChainStage,
        result: &BlockValidationResult,
    ) -> Result<(), WorkerError> {
        match result {
            BlockValidationResult::BlockValidated { point, .. } => {
                // FIXME: block height should be part of BlockValidated message
                let store = stage.store.lock().await;
                if let Some(header) = store.load_header(&Hash::from(point)) {
                    // assert that the new tip is a direct successor of the old tip
                    assert_eq!(header.block_height(), self.our_tip.1 + 1);
                    match header.parent() {
                        Some(parent) => assert_eq!(
                            Point::new(self.our_tip.0.slot_or_default(), parent.as_ref().to_vec()),
                            self.our_tip.0
                        ),
                        None => assert_eq!(self.our_tip.0, Point::Origin),
                    }

                    self.our_tip = Tip(point.pallas_point(), header.block_height());

                    trace!(
                        target: EVENT_TARGET,
                        tip = %point,
                        "tip_changed"
                    );

                    self.clients.send(ClientMsg::Op(ClientOp::Forward(
                        header,
                        self.our_tip.clone(),
                    )));

                    stage
                        .downstream
                        .send(ForwardEvent::Forward(point.pallas_point()));
                }

                Ok(())
            }
            BlockValidationResult::RolledBackTo { rollback_point, .. } => {
                info!(
                    target: EVENT_TARGET,
                    point = %rollback_point,
                    "rolled_back_to"
                );

                // FIXME: block height should be part of BlockValidated message
                let store = stage.store.lock().await;
                if let Some(header) = store.load_header(&Hash::from(rollback_point)) {
                    self.our_tip = Tip(rollback_point.pallas_point(), header.block_height());
                    self.clients
                        .send(ClientMsg::Op(ClientOp::Backward(self.our_tip.clone())));

                    stage
                        .downstream
                        .send(ForwardEvent::Backward(rollback_point.pallas_point()));
                }

                Ok(())
            }
            BlockValidationResult::BlockValidationFailed { point, .. } => {
                error!(
                    target: EVENT_TARGET,
                    slot = ?point.slot_or_default(),
                    hash = ?Hash::<32>::from(point),
                    "block_validation_failed"
                );

                Ok(())
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
enum ClientOp {
    /// the tip to go back to
    Backward(Tip),
    /// the header to go forward to and the tip we will be at after sending this header
    Forward(Header, Tip),
}

impl std::fmt::Debug for ClientOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backward(tip) => f
                .debug_struct("Backward")
                .field("tip", &(tip.1, PrettyPoint(&tip.0)))
                .finish(),
            Self::Forward(header, tip) => f
                .debug_struct("Forward")
                .field(
                    "header",
                    &(header.block_height(), PrettyPoint(&header.pallas_point())),
                )
                .field("tip", &(tip.1, PrettyPoint(&tip.0)))
                .finish(),
        }
    }
}

struct PrettyPoint<'a>(&'a Point);

impl std::fmt::Debug for PrettyPoint<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "({}, {})",
            self.0.slot_or_default(),
            hex::encode(hash_point(self.0))
        )
    }
}

fn hash_point(point: &Point) -> Hash<32> {
    match point {
        Point::Origin => Hash::from([0; 32]),
        Point::Specific(_slot, hash) => Hash::from(hash.as_slice()),
    }
}

impl PartialEq for ClientOp {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Backward(l0), Self::Backward(r0)) => (&l0.0, l0.1) == (&r0.0, r0.1),
            (Self::Forward(l0, l1), Self::Forward(r0, r1)) => {
                l0 == r0 && (&l1.0, l1.1) == (&r1.0, r1.1)
            }
            _ => false,
        }
    }
}

impl Eq for ClientOp {}

impl ClientOp {
    pub fn tip(&self) -> Tip {
        match self {
            ClientOp::Backward(tip) => tip.clone(),
            ClientOp::Forward(_, tip) => tip.clone(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<ForwardChainStage> for Worker {
    async fn bootstrap(stage: &ForwardChainStage) -> Result<Self, WorkerError> {
        let server = TcpListener::bind(&stage.listen_address).await.or_panic()?;
        tracing::debug!("sending listening event");
        stage.downstream.send(ForwardEvent::Listening(
            server.local_addr().or_panic()?.port(),
        ));

        let (tx, incoming_peers) = mpsc::channel(10);

        let clients = stage
            .runtime
            .spawn_actor("chain_forward", |cell| {
                client_supervisor(cell, stage.store.clone(), stage.max_peers)
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
            our_tip: stage.our_tip.clone(),
            clients,
        })
    }

    async fn schedule(
        &mut self,
        stage: &mut ForwardChainStage,
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

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "stage.forward_chain",
    )]
    async fn execute(
        &mut self,
        unit: &Unit,
        stage: &mut ForwardChainStage,
    ) -> Result<(), WorkerError> {
        match unit {
            Unit::Block(result) => {
                adopt_current_span(result);
                self.handle_validation_result(stage, result).await
            }
            Unit::Peer(peer) => {
                // FIXME: gasket design bug that we only get &Unit and thus cannot take values from it without internal mutability
                let peer = peer.borrow_mut().take();
                if let Some(peer) = peer {
                    self.clients
                        .send(ClientMsg::Peer(peer, self.our_tip.clone()));
                } else {
                    error!(target: EVENT_TARGET, "Unit::Peer was empty in execute");
                }
                Ok(())
            }
        }
    }
}

#[allow(clippy::large_enum_variant)]
enum ClientMsg {
    /// A new peer has connected to us.
    ///
    /// Our tip is included to get the connection handlers started correctly.
    Peer(PeerServer, Tip),
    /// An operation to be executed on all clients.
    Op(ClientOp),
}

async fn client_supervisor(
    mut cell: ActoCell<ClientMsg, impl ActoRuntime, anyhow::Result<()>>,
    store: Arc<Mutex<dyn ChainStore<Header>>>,
    max_peers: usize,
) {
    let mut clients = BTreeMap::new();
    while let Some(msg) = cell.recv().await.has_senders() {
        match msg {
            ActoMsgSuper::Message(ClientMsg::Peer(peer, tip)) => {
                let addr = peer
                    .accepted_address()
                    .map(|a| a.to_string())
                    .unwrap_or_default();

                if clients.len() >= max_peers {
                    tracing::warn!(target: EVENT_TARGET, "max peers reached, dropping peer from {addr}");
                    continue;
                }

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

#[cfg(test)]
mod test_infra;
