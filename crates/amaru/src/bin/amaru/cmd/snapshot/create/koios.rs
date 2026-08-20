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

use std::{error::Error, str::FromStr, time::Duration};

use amaru_kernel::{Epoch, Hash, NetworkName, NetworkPoint, Slot};
use amaru_observability::info;
use serde::{Deserialize, de::DeserializeOwned};

use super::EpochTarget;

const KOIOS_RETRY_ATTEMPTS: u64 = 5;
const KOIOS_RETRY_BASE_DELAY: Duration = Duration::from_secs(2);

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

/// Perform a Koios GET request and decode its JSON body, retrying transient
/// failures (connection errors, timeouts, HTTP 408/429 and 5xx) with
/// exponential backoff.
async fn get_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    query: &[(&str, String)],
) -> Result<T, Box<dyn Error>> {
    let mut delay = KOIOS_RETRY_BASE_DELAY;
    for attempt in 1..=KOIOS_RETRY_ATTEMPTS {
        let result = async {
            client
                .get(url)
                .header(reqwest::header::ACCEPT, "application/json")
                .query(query)
                .send()
                .await?
                .error_for_status()?
                .json::<T>()
                .await
        }
        .await;

        match result {
            Ok(value) => return Ok(value),
            Err(err) if attempt < KOIOS_RETRY_ATTEMPTS && is_transient(&err) => {
                info!(cli::koios::RETRY, error = %err, attempt, delay_secs = delay.as_secs());
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
            Err(err) => return Err(err.into()),
        }
    }
    unreachable!("the final attempt either returned its value or its error")
}

fn is_transient(err: &reqwest::Error) -> bool {
    if err.is_connect() || err.is_timeout() {
        return true;
    }
    err.status().is_some_and(|code| {
        code.is_server_error()
            || code == reqwest::StatusCode::TOO_MANY_REQUESTS
            || code == reqwest::StatusCode::REQUEST_TIMEOUT
    })
}

pub(super) async fn fetch_current_epoch(
    client: &reqwest::Client,
    network: NetworkName,
) -> Result<Epoch, Box<dyn Error>> {
    let tip: Vec<KoiosTip> = get_json(client, &format!("{}/tip", koios_api_base(network)?), &[]).await?;
    let tip = tip.into_iter().next().ok_or("Koios returned empty tip response")?;

    info!(cli::current_epoch::RESOLVE, epoch = tip.epoch_no);

    Ok(Epoch::from(tip.epoch_no))
}

async fn fetch_block_by_hash(
    client: &reqwest::Client,
    network: NetworkName,
    hash: &str,
) -> Result<KoiosBlock, Box<dyn Error>> {
    let blocks: Vec<KoiosBlock> = get_json(
        client,
        &format!("{}/blocks", koios_api_base(network)?),
        &[("hash", format!("eq.{hash}")), ("limit", "1".to_owned())],
    )
    .await?;

    blocks.into_iter().next().ok_or_else(|| format!("Koios returned no block for hash {hash}").into())
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
    epoch: Epoch,
) -> Result<EpochTarget, Box<dyn Error>> {
    let blocks: Vec<KoiosBlock> = get_json(
        client,
        &format!("{}/blocks", koios_api_base(network)?),
        &[("epoch_no", format!("eq.{epoch}")), ("order", "abs_slot.desc".to_owned()), ("limit", "1".to_owned())],
    )
    .await?;

    let block = blocks.into_iter().next().ok_or_else(|| format!("Koios returned no blocks for epoch {epoch}"))?;

    let point = NetworkPoint::Specific(Slot::from(block.abs_slot), Hash::from_str(&block.hash)?);

    let parent_block = fetch_block_by_hash(client, network, &block.parent_hash).await?;
    let parent_point = NetworkPoint::Specific(Slot::from(parent_block.abs_slot), Hash::from_str(&parent_block.hash)?);

    info!(cli::last_block::RESOLVE, %epoch, %point);

    Ok(EpochTarget {
        epoch,
        slot: Slot::from(block.abs_slot),
        hash: Hash::from_str(&block.hash)?,
        parent_point: Some(parent_point),
    })
}
