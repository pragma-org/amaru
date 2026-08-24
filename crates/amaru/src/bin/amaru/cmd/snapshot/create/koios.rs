// Copyright 2026 PRAGMA
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

use std::str::FromStr;

use amaru_kernel::{Epoch, Hash, NetworkName, NetworkPoint, Slot};
use amaru_observability::info;
use anyhow::anyhow;
use serde::Deserialize;

use super::EpochTarget;

#[derive(Debug, Deserialize)]
struct KoiosBlock {
    abs_slot: u64,
    hash: String,
    parent_hash: String,
}

#[derive(Debug, Deserialize)]
struct KoiosTip {
    epoch_no: u64,
}

pub(super) async fn fetch_current_epoch(client: &reqwest::Client, network: NetworkName) -> anyhow::Result<Epoch> {
    let response = client
        .get(format!("{}/tip", koios_api_base(network)?))
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await?
        .error_for_status()?;

    let tip = response
        .json::<Vec<KoiosTip>>()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("Koios returned empty tip response"))?;

    info!(cli::current_epoch::RESOLVE, epoch = tip.epoch_no);

    Ok(Epoch::from(tip.epoch_no))
}

async fn fetch_block_by_hash(client: &reqwest::Client, network: NetworkName, hash: &str) -> anyhow::Result<KoiosBlock> {
    let response = client
        .get(format!("{}/blocks", koios_api_base(network)?))
        .header(reqwest::header::ACCEPT, "application/json")
        .query(&[("hash", format!("eq.{hash}")), ("limit", "1".to_owned())])
        .send()
        .await?
        .error_for_status()?;

    response
        .json::<Vec<KoiosBlock>>()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("Koios returned no block for hash {hash}"))
}

fn koios_api_base(network: NetworkName) -> anyhow::Result<&'static str> {
    match network {
        NetworkName::Mainnet => Ok("https://api.koios.rest/api/v1"),
        NetworkName::Preprod => Ok("https://preprod.koios.rest/api/v1"),
        NetworkName::Preview => Ok("https://preview.koios.rest/api/v1"),
        NetworkName::Testnet(_) => Err(anyhow!("Koios lookup is only supported on mainnet, preprod and preview")),
    }
}

pub(super) async fn fetch_last_block_for_epoch(
    client: &reqwest::Client,
    network: NetworkName,
    epoch: Epoch,
) -> anyhow::Result<EpochTarget> {
    let response = client
        .get(format!("{}/blocks", koios_api_base(network)?))
        .header(reqwest::header::ACCEPT, "application/json")
        .query(&[("epoch_no", format!("eq.{epoch}")), ("order", "abs_slot.desc".to_owned()), ("limit", "1".to_owned())])
        .send()
        .await?
        .error_for_status()?;

    let block = response
        .json::<Vec<KoiosBlock>>()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("Koios returned no blocks for epoch {epoch}"))?;

    let point = NetworkPoint::Specific(Slot::from(block.abs_slot), Hash::from_str(&block.hash)?);

    let parent_block = fetch_block_by_hash(client, network, &block.parent_hash).await?;
    let parent_point = NetworkPoint::Specific(Slot::from(parent_block.abs_slot), Hash::from_str(&parent_block.hash)?);

    info!(cli::last_block::RESOLVE, epoch, point);

    Ok(EpochTarget {
        epoch,
        slot: Slot::from(block.abs_slot),
        hash: Hash::from_str(&block.hash)?,
        parent_point: Some(parent_point),
    })
}
