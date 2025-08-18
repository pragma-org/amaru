use std::sync::Arc;

use amaru_kernel::network::NetworkName;
use amaru_kernel::Point;
use amaru_network::chain_sync_client::ChainSyncClient;
use amaru_network::connect_to_peer;
use tokio::sync::Mutex;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let peer_address = "clermont:3001";
    let network_name = NetworkName::Preprod;
    let point =
        "69638365.4ec0f5a78431fdcc594eab7db91aff7dfd91c13cc93e9fbfe70cd15a86fadfb2".try_into()?;

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .json()
        .init();

    let mut peer = connect_to_peer(peer_address, &network_name).await?;

    let client = peer.chainsync();

    let mut chain_sync = ChainSyncClient::new1(Box::new(client), &vec![point]);

    chain_sync
        .find_intersection()
        .await
        .expect("failed to find intersection");

    let batch = chain_sync.pull_batch().await.expect("failed to pull batch");

    println!("batch: {:?}", batch);

    Ok(())
}
