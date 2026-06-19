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

use std::{
    collections::BTreeSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
};

use amaru_kernel::{Bytes, Nullable, Relay};
use amaru_ouroboros_traits::{HasPeersData, StoreError as ConsensusStoreError};
use async_trait::async_trait;
use tracing::field;

use crate::{
    state::State,
    store::{HistoricalStores, ReadStore, Store, StoreError as LedgerStoreError},
};

/// This data type accesses the ledger state to extract peers data.
pub struct PeersData<S, HS>
where
    S: Store + Send,
    HS: HistoricalStores + Send,
{
    pub state: State<S, HS>,
}

impl<S, HS> Clone for PeersData<S, HS>
where
    S: Store + Send,
    HS: HistoricalStores + Send,
{
    fn clone(&self) -> Self {
        Self { state: self.state.clone() }
    }
}

impl<S: Store + Send, HS: HistoricalStores + Send> PeersData<S, HS> {
    pub fn new(state: State<S, HS>) -> anyhow::Result<Self> {
        Ok(Self { state })
    }
}

// The 'LedgerState' trait materializes the interface required of the consensus layer in order to
// validate block headers. It allows to keep the ledger implementation rather abstract to the
// consensus in order to decouple both components.
#[async_trait]
impl<S, HS> HasPeersData for PeersData<S, HS>
where
    S: Store + Send + Sync,
    HS: HistoricalStores + Send + Sync,
{
    /// Get the registered relay socket addresses from the stable store.
    ///
    /// **NOTE:** This operation blocks the ledger for about 4ms (mainnet late
    /// 2025), so it should be called with care. Please cache the result, it
    /// only changes meaningfully once per epoch.
    async fn registered_relay_socket_addrs(&self) -> Result<BTreeSet<SocketAddr>, ConsensusStoreError> {
        self.state
            .load()
            .registered_relay_socket_addrs()
            .map_err(|error| ConsensusStoreError::ReadError { error: error.to_string() })
    }
}

pub fn collect_from_read_store(db: &impl ReadStore) -> Result<BTreeSet<SocketAddr>, LedgerStoreError> {
    let span = tracing::info_span!("collect relay addresses from ledger", addresses = field::Empty).entered();
    let mut set = BTreeSet::new();
    for (_, row) in db.iter_pools()? {
        for relay in &row.current_params.relays {
            relay_to_socket_addrs(relay, &mut set);
        }
    }
    span.record("addresses", set.len());
    Ok(set)
}

fn relay_to_socket_addrs(relay: &Relay, set: &mut BTreeSet<SocketAddr>) {
    match relay {
        Relay::SingleHostAddr(port, ipv4, ipv6) => {
            let Some(port) = nullable_to_port(port) else {
                return;
            };
            if let Some(ip) = nullable_ipv4_to_ip(ipv4)
                && !is_excluded_relay_ip(ip)
            {
                set.insert(SocketAddr::new(ip, port));
            }
            if let Some(ip) = nullable_ipv6_to_ip(ipv6)
                && !is_excluded_relay_ip(ip)
            {
                set.insert(SocketAddr::new(ip, port));
            }
        }
        Relay::SingleHostName(..) | Relay::MultiHostName(..) => {
            // DNS usage is very inconsistent, don't bother for now
        }
    }
}

fn nullable_to_port(port: &Nullable<u32>) -> Option<u16> {
    match port {
        Nullable::Some(p) => u16::try_from(*p).ok(),
        Nullable::Null | Nullable::Undefined => None,
    }
}

fn nullable_ipv4_to_ip(null: &Nullable<Bytes>) -> Option<IpAddr> {
    let bytes = match null {
        Nullable::Some(b) => <[u8; 4]>::try_from(b).ok()?,
        Nullable::Null | Nullable::Undefined => return None,
    };
    Some(IpAddr::V4(Ipv4Addr::from_octets(bytes)))
}

fn nullable_ipv6_to_ip(null: &Nullable<Bytes>) -> Option<IpAddr> {
    let mut bytes = match null {
        Nullable::Some(b) => <[u8; 16]>::try_from(b).ok()?,
        Nullable::Null | Nullable::Undefined => return None,
    };
    // manual inspection of the ledger contents has confirmed that Haskell node
    // serialization of IPv6 addresses is in “screwed-endian” byte order
    bytes[0..4].reverse();
    bytes[4..8].reverse();
    bytes[8..12].reverse();
    bytes[12..16].reverse();
    Some(IpAddr::V6(Ipv6Addr::from_octets(bytes)))
}

fn is_excluded_relay_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_documentation()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unicast_link_local() || is_unique_local_v6(&v6) || v6.is_unspecified()
        }
    }
}

fn is_unique_local_v6(v6: &Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xfe00) == 0xfc00
}
