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

use crate::point::to_network_point;
use crate::server::client_protocol::{ChainSyncOp, ClientMsg, ClientProtocolMsg, client_protocols};
use acto::{AcTokio, ActoCell, ActoMsgSuper, ActoRef, ActoRuntime, MailboxSize};
use amaru_kernel::BlockHeader;
use amaru_kernel::connection::ClientConnectionError;
use amaru_kernel::is_header::{AsHeaderTip, HeaderTip};
use amaru_kernel::peer::Peer;
use amaru_ouroboros_traits::network_operations::ForwardEvent;
use amaru_ouroboros_traits::{ChainStore, TxClientReply, TxServerRequest};
use anyhow::anyhow;
use pallas_network::{facades::PeerServer, miniprotocols::chainsync::Tip};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::sync::mpsc::Sender;
use tokio::sync::{Mutex, mpsc};

pub const EVENT_TARGET: &str = "amaru::consensus::forward_chain";

/// The DownstreamServer listens for incoming TCP connections from peers
/// and spawns a client protocol handler for each accepted connection.
/// It also implements the ForwardEventListener trait to receive chain selection forward events
/// and forward them to all connected peers.
pub struct DownstreamServer {
    _runtime: AcTokio,
    our_tip: Arc<Mutex<HeaderTip>>,
    clients: ActoRef<ClientMsg>,
    tx_client_reply_receiver: Mutex<mpsc::Receiver<TxClientReply>>,
}

impl DownstreamServer {
    /// Creates a new DownstreamServer instance:
    ///
    ///  - Start an Acto runtime
    ///  - Bind a TCP listener to the given address
    ///  - Spawn the client supervisor actor
    pub async fn new(
        store: Arc<dyn ChainStore<BlockHeader>>,
        listen_address: String,
        network_magic: u64,
        max_peers: usize,
        our_tip: HeaderTip,
    ) -> anyhow::Result<Self> {
        let tcp_listener = TcpListener::bind(&listen_address).await?;
        DownstreamServer::create(store, tcp_listener, network_magic, max_peers, our_tip)
    }

    /// Creates a new DownstreamServer instance with a provided Acto runtime and TcpListener.
    pub fn create(
        store: Arc<dyn ChainStore<BlockHeader>>,
        tcp_listener: TcpListener,
        network_magic: u64,
        max_peers: usize,
        our_tip: HeaderTip,
    ) -> anyhow::Result<Self> {
        let runtime = AcTokio::from_handle("consensus.forward", Handle::current());
        let (tx_client_reply_sender, tx_client_reply_receiver) = mpsc::channel(100);

        let clients = runtime
            // FIXME: This is a temporary stop gap solution while we wait
            // to refactor to use pure-stage. Acto library has as a capped
            // size for the mailbox and drops incoming messages when it's
            // full.  This should not be a problem in real life, but while
            // we are syncing _and_ forwarding at the same time for demo
            // purpose, this is problematic.
            .with_mailbox_size(1_000_000)
            .spawn_actor("chain_forward", |cell| {
                client_supervisor(cell, store.clone(), max_peers, tx_client_reply_sender)
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
                        let our_tip = our_tip_clone.lock().await.clone();

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
            tx_client_reply_receiver: Mutex::new(tx_client_reply_receiver),
        })
    }

    pub async fn next_tx_reply(&self) -> Result<TxClientReply, ClientConnectionError> {
        if let Some(tx_reply) = self.tx_client_reply_receiver.lock().await.recv().await {
            Ok(tx_reply)
        } else {
            Err(anyhow!("tx reply channel closed").into())
        }
    }

    pub async fn send_tx_request(
        &self,
        request: TxServerRequest,
    ) -> Result<(), ClientConnectionError> {
        self.clients.send(ClientMsg::TxSubmission(request));
        Ok(())
    }

    pub async fn send(&self, event: ForwardEvent) -> anyhow::Result<()> {
        match event {
            ForwardEvent::Forward(header) => {
                {
                    let mut our_tip = self.our_tip.lock().await;
                    *our_tip = header.as_header_tip();
                };

                self.clients
                    .send(ClientMsg::ChainSync(ChainSyncOp::Forward(header)));
                Ok(())
            }
            ForwardEvent::Backward(tip) => {
                let mut our_tip = self.our_tip.lock().await;
                *our_tip = tip.clone();
                self.clients
                    .send(ClientMsg::ChainSync(ChainSyncOp::Backward(Tip(
                        to_network_point(tip.point()),
                        tip.block_height(),
                    ))));
                Ok(())
            }
        }
    }
}

async fn client_supervisor(
    mut cell: ActoCell<ClientMsg, impl ActoRuntime, anyhow::Result<()>>,
    store: Arc<dyn ChainStore<BlockHeader>>,
    max_peers: usize,
    tx_client_reply_sender: Sender<TxClientReply>,
) {
    let mut clients = BTreeMap::new();
    while let Some(msg) = cell.recv().await.has_senders() {
        match msg {
            ActoMsgSuper::Message(ClientMsg::Peer(peer_server, tip)) => {
                let addr = peer_server
                    .accepted_address()
                    .map(|a| a.to_string())
                    .unwrap_or_default();

                if clients.len() >= max_peers {
                    tracing::warn!(target: EVENT_TARGET, "max peers reached, dropping peer from {addr}");
                    continue;
                }

                let tx_client_reply_sender_clone = tx_client_reply_sender.clone();
                let client = cell.spawn_supervised(&addr, {
                    let store = store.clone();
                    let peer = Peer::new(&addr);
                    move |cell| {
                        client_protocols(
                            cell,
                            peer_server,
                            peer,
                            store,
                            tip,
                            tx_client_reply_sender_clone,
                        )
                    }
                });
                clients.insert(Peer::new(&addr), client);
            }
            ActoMsgSuper::Message(ClientMsg::ChainSync(op)) => {
                for client in clients.values() {
                    client.send(ClientProtocolMsg::ChainSync(op.clone()));
                }
            }
            ActoMsgSuper::Message(ClientMsg::TxSubmission(request)) => {
                if let Some(sender) = clients.get(request.peer()) {
                    let peer = request.peer().clone();
                    if !sender.send(ClientProtocolMsg::TxSubmission(request)) {
                        tracing::warn!(target: EVENT_TARGET, "failed to send tx request to peer: {}", peer);
                    }
                } else {
                    tracing::warn!(target: EVENT_TARGET, "no connected peers to send tx request");
                }
            }
            ActoMsgSuper::Supervision { id, name, result } => {
                tracing::info!(target: EVENT_TARGET, "client {} terminated: {:?}", name, result);
                clients.retain(|_, c| c.id() != id);
            }
        }
    }
}
