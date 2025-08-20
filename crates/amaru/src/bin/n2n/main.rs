use amaru_kernel::network::NetworkName;
use amaru_network::chain_sync_client::{new_with_peer, PullResult};
use amaru_network::connect_to_peer;

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

    let peer = connect_to_peer(peer_address, &network_name).await?;

    let mut chain_sync = new_with_peer(peer, &vec![point]);

    chain_sync
        .find_intersection()
        .await
        .expect("failed to find intersection");

    let mut batch = vec![];
    while batch.len() < 200 {
        match chain_sync.pull_batch().await.expect("failed to pull batch") {
            PullResult::ForwardBatch(mut header_contents) => batch.append(&mut header_contents),
            PullResult::RollBack(_point) => continue, // FIXME: should trim already accumulated headers to this point
            PullResult::Nothing => break,
        }
    }

    println!("batched: {} headers", batch.len());

    Ok(())
}
