use std::sync::Arc;

use pallas_network::facades::PeerClient;
use tokio::sync::Mutex;

/// A single peer in the network, with a unique identifier.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct Peer {
    name: String,
}

impl Peer {
    pub fn new(name: &str) -> Peer {
        Peer {
            name: name.to_string(),
        }
    }
}

/// A session with a peer, including the peer itself and a client to communicate with it.
pub struct PeerSession {
    pub peer: Peer,
    pub peer_client: Arc<Mutex<PeerClient>>,
}

impl PeerSession {
    pub async fn lock(&mut self) -> tokio::sync::MutexGuard<'_, PeerClient> {
        self.peer_client.lock().await
    }
}
