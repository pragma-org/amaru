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

use std::error::Error;

use amaru_kernel::NetworkName;
use serde::Deserialize;
use tracing::info;

use super::EpochTarget;

#[derive(Debug, Deserialize)]
struct KoiosBlock {
    abs_slot: u64,
    hash: String,
}

fn koios_api_base(network: NetworkName) -> Result<&'static str, Box<dyn Error>> {
    match network {
        NetworkName::Mainnet => Ok("https://api.koios.rest/api/v1"),
        NetworkName::Preprod => Ok("https://preprod.koios.rest/api/v1"),
        NetworkName::Preview => Ok("https://preview.koios.rest/api/v1"),
        NetworkName::Testnet(_) => Err("Koios lookup is only supported on mainnet, preprod and preview".into()),
    }
}

pub(super) async fn fetch_last_block_for_epoch(
    client: &reqwest::Client,
    network: NetworkName,
    epoch: u64,
) -> Result<EpochTarget, Box<dyn Error>> {
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
        .ok_or_else(|| format!("Koios returned no blocks for epoch {epoch}"))?;

    info!(epoch, slot = block.abs_slot, hash = %block.hash, "resolved last produced block for epoch");

    Ok(EpochTarget { epoch, slot: block.abs_slot, hash: block.hash, archive_path: None, snapshot_path: None })
}
