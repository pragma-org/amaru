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

use crate::stages::consensus::forward_chain::client_protocol::{
    ClientMsg, ClientOp, ClientProtocolMsg, client_protocols,
};
use acto::{AcTokio, ActoCell, ActoMsgSuper, ActoRef, ActoRuntime, MailboxSize};
use amaru_consensus::consensus::effects::{ForwardEvent, ForwardEventListener};
use amaru_kernel::HeaderTip;
use amaru_kernel::{AsHeaderTip, BlockHeader, IsHeader};
use amaru_network::point::to_network_point;
use amaru_ouroboros_traits::ChainStore;
use async_trait::async_trait;
use pallas_network::{facades::PeerServer, miniprotocols::chainsync::Tip};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::net::TcpListener;
use tokio::runtime::Handle;

pub const EVENT_TARGET: &str = "amaru::consensus::forward_chain";

/// The TcpForwardChainServer listens for incoming TCP connections from peers
/// and spawns a client protocol handler for each accepted connection.
/// It also implements the ForwardEventListener trait to receive forward events
/// and forward them to all connected peers.
pub struct TcpForwardChainServer<H> {
    our_tip: Arc<Mutex<HeaderTip>>,
    clients: ActoRef<ClientMsg<H>>,
    _runtime: AcTokio,
}

impl<H: IsHeader + 'static + Clone + Send> TcpForwardChainServer<H> {
    /// Creates a new TcpForwardChainServer instance:
    ///
    ///  - Start an Acto runtime
    ///  - Bind a TCP listener to the given address
    ///  - Spawn the client supervisor actor
    pub async fn new(
        store: Arc<dyn ChainStore<H>>,
        listen_address: String,
        network_magic: u64,
        max_peers: usize,
        our_tip: HeaderTip,
    ) -> anyhow::Result<Self> {
        let tcp_listener = TcpListener::bind(&listen_address).await?;
        TcpForwardChainServer::create(store, tcp_listener, network_magic, max_peers, our_tip)
    }

    /// Creates a new TcpForwardChainServer instance with a provided Acto runtime and TcpListener.
    #[expect(clippy::expect_used)]
    pub fn create(
        store: Arc<dyn ChainStore<H>>,
        tcp_listener: TcpListener,
        network_magic: u64,
        max_peers: usize,
        our_tip: HeaderTip,
    ) -> anyhow::Result<Self> {
        let runtime = AcTokio::from_handle("consensus.forward", Handle::current());

        let clients = runtime
            // FIXME: This is a temporary stop gap solution while we wait
            // to refactor to use pure-stage. Acto library has as a capped
            // size for the mailbox and drops incoming messages when it's
            // full.  This should not be a problem in real life, but while
            // we are syncing _and_ forwarding at the same time for demo
            // purpose, this is problematic.
            .with_mailbox_size(1_000_000)
            .spawn_actor("chain_forward", |cell| {
                client_supervisor(cell, store.clone(), max_peers)
            })
            .me;

        let our_tip = Arc::new(Mutex::new(our_tip.clone()));

        let our_tip_clone = our_tip.clone();
        let clients_clone = clients.clone();
        tokio::spawn(async move {
            loop {
                // due to the signature of TcpListener::accept, this is the only way to use this API
                // in particular, it isn’t possible to poll for new peers within the `schedule` method
                match PeerServer::accept(&tcp_listener, network_magic).await {
                    Ok(peer) => {
                        let our_tip = our_tip_clone
                            .lock()
                            .expect("poisoned lock for our tip")
                            .clone();
                        clients_clone.send(ClientMsg::Peer(
                            peer,
                            Tip(to_network_point(our_tip.point()), our_tip.block_height()),
                        ));
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: EVENT_TARGET,
                            "error accepting peer: {e}",
                        );
                        continue;
                    }
                };
            }
        });

        Ok(Self {
            _runtime: runtime,
            clients: clients.clone(),
            our_tip: our_tip.clone(),
        })
    }
}

/// This implementation of ForwardEventListener sends the received events to all connected clients.
#[async_trait]
impl ForwardEventListener for TcpForwardChainServer<BlockHeader> {
    async fn send(&self, event: ForwardEvent) -> anyhow::Result<()> {
        match event {
            ForwardEvent::Forward(header) => {
                {
                    let mut our_tip = self
                        .our_tip
                        .lock()
                        .map_err(|e| anyhow::anyhow!("Mutex poisoned: {}", e))?;
                    *our_tip = header.as_header_tip();
                };

                self.clients.send(ClientMsg::Op(ClientOp::Forward(header)));
                Ok(())
            }
            ForwardEvent::Backward(tip) => {
                let mut our_tip = self
                    .our_tip
                    .lock()
                    .map_err(|e| anyhow::anyhow!("Mutex poisoned: {}", e))?;
                *our_tip = tip.clone();
                self.clients.send(ClientMsg::Op(ClientOp::Backward(Tip(
                    to_network_point(tip.point()),
                    tip.block_height(),
                ))));
                Ok(())
            }
        }
    }
}

async fn client_supervisor<H: IsHeader + 'static + Send + Clone>(
    mut cell: ActoCell<ClientMsg<H>, impl ActoRuntime, anyhow::Result<()>>,
    store: Arc<dyn ChainStore<H>>,
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
