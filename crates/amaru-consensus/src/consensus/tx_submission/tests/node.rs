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

use crate::tx_submission::tests::sized_mempool::SizedMempool;
use crate::tx_submission::tests::{MockTransport, Tx};
use crate::tx_submission::tx_submission_client::TxSubmissionClient;
use crate::tx_submission::tx_submission_server::TxSubmissionServer;
use crate::tx_submission::{Blocking, ServerParams};
use amaru_kernel::peer::Peer;
use amaru_mempool::strategies::InMemoryMempool;
use amaru_ouroboros_traits::can_validate_transactions::mock::MockCanValidateTransactions;
use amaru_ouroboros_traits::{CanValidateTransactions, Mempool};
use pallas_network::miniprotocols::txsubmission::{EraTxBody, EraTxId, Message};
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;
use tokio::task::JoinHandle;

/// A test node consisting of a TxSubmissionClient and TxSubmissionServer,
/// their respective mempools, connected via a MockTransport.
pub struct Node {
    pub client_mempool: Arc<dyn Mempool<Tx>>,
    pub server_mempool: Arc<dyn Mempool<Tx>>,
    pub client: TxSubmissionClient<Tx>,
    pub server: TxSubmissionServer<Tx>,
    pub transport: MockTransport,
}

impl Node {
    /// Starts the client and server asynchronously.
    pub fn start(self) -> NodeHandle {
        let Node {
            client_mempool,
            server_mempool,
            mut client,
            mut server,
            transport,
        } = self;
        let MockTransport {
            client_transport,
            server_transport,
            rx_observe_messages,
        } = transport;
        let server_handle =
            tokio::spawn(async move { server.start_server_with_transport(server_transport).await });

        let client_handle =
            tokio::spawn(async move { client.start_client_with_transport(client_transport).await });

        NodeHandle {
            client_mempool,
            server_mempool,
            rx_observe_messages,
            _client_handle: client_handle,
            _server_handle: server_handle,
        }
    }

    pub fn insert_client_transactions(&self, txs: &[Tx]) {
        for tx in txs.iter() {
            self.client_mempool
                .insert(
                    tx.clone(),
                    amaru_ouroboros_traits::TxOrigin::Remote(Peer::new("upstream")),
                )
                .unwrap();
        }
    }
}

/// Creates a test node with default server options.
pub fn create_node() -> Node {
    create_node_with(ServerOptions::default())
}

/// Creates a test node with the specified server options.
pub fn create_node_with(server_options: ServerOptions) -> Node {
    let client_mempool = Arc::new(SizedMempool::with_tx_validator(
        server_options.mempool_capacity,
        server_options.tx_validator,
    ));
    let client = TxSubmissionClient::new(client_mempool.clone(), &Peer::new("server_peer"));
    let server_mempool = server_options.mempool;
    let server = TxSubmissionServer::new(
        server_options.params,
        server_mempool.clone(),
        Peer::new("client_peer"),
    );

    let transport = MockTransport::with_channel_size(10);

    Node {
        client_mempool,
        server_mempool,
        client,
        server,
        transport,
    }
}

/// A handle to a running test node, allowing access to its mempools and transport.
/// It can be aborted to stop the client and server tasks.
pub struct NodeHandle {
    pub server_mempool: Arc<dyn Mempool<Tx>>,
    pub client_mempool: Arc<dyn Mempool<Tx>>,
    pub rx_observe_messages: Receiver<Message<EraTxId, EraTxBody>>,
    _client_handle: JoinHandle<anyhow::Result<()>>,
    _server_handle: JoinHandle<anyhow::Result<()>>,
}

impl Drop for NodeHandle {
    fn drop(&mut self) {
        self.abort()
    }
}

impl NodeHandle {
    pub fn abort(&self) {
        self._client_handle.abort();
        self._server_handle.abort();
    }

    pub async fn observe_messages(&mut self) -> Vec<Message<EraTxId, EraTxBody>> {
        let mut messages = Vec::new();
        while let Ok(message) = self.rx_observe_messages.try_recv() {
            messages.push(message);
        }
        messages
    }
}

pub struct ServerOptions {
    params: ServerParams,
    mempool: Arc<dyn Mempool<Tx>>,
    mempool_capacity: u64,
    tx_validator: Arc<dyn CanValidateTransactions<Tx>>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        ServerOptions {
            params: ServerParams::new(4, 2, Blocking::Yes),
            mempool: Arc::new(InMemoryMempool::default()),
            mempool_capacity: 4,
            tx_validator: Arc::new(MockCanValidateTransactions),
        }
    }
}

impl ServerOptions {
    #[expect(dead_code)]
    pub fn with_params(mut self, params: ServerParams) -> Self {
        self.params = params;
        self
    }

    pub fn with_blocking(mut self, blocking: Blocking) -> Self {
        self.params.blocking = blocking;
        self
    }

    pub fn with_max_window(mut self, size: usize) -> Self {
        self.params.max_window = size;
        self
    }

    pub fn with_fetch_batch(mut self, size: usize) -> Self {
        self.params.fetch_batch = size;
        self
    }

    #[expect(dead_code)]
    pub fn with_mempool(mut self, mempool: Arc<dyn Mempool<Tx>>) -> Self {
        self.mempool = mempool;
        self
    }

    pub fn with_mempool_capacity(mut self, size: u64) -> Self {
        self.mempool_capacity = size;
        self
    }

    pub fn with_tx_validator(mut self, tx_validator: Arc<dyn CanValidateTransactions<Tx>>) -> Self {
        self.tx_validator = tx_validator;
        self
    }
}
