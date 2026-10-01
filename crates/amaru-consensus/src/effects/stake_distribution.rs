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

use std::sync::Arc;

use amaru_kernel::{Epoch, PoolId};
use amaru_ouroboros_traits::{PoolSummaries, PoolSummary};
use tokio::sync::watch;

/// Read-only stake-distribution snapshots shared by consensus effects.
///
/// Header validation takes an immediate snapshot. Consumers that cannot make
/// progress without a particular epoch, such as leader scheduling, wait for it.
#[derive(Clone, Debug)]
pub struct StakeDistributionSource {
    receiver: watch::Receiver<Arc<PoolSummaries>>,
}

/// Publishes projected stake distributions from the ledger worker.
#[derive(Clone, Debug)]
pub struct StakeDistributionPublisher {
    sender: watch::Sender<Arc<PoolSummaries>>,
}

impl StakeDistributionSource {
    /// Create a source and its ledger-side publisher from the initial snapshot.
    pub fn new(initial: PoolSummaries) -> (StakeDistributionPublisher, Self) {
        let (sender, receiver) = watch::channel(Arc::new(initial));
        (StakeDistributionPublisher { sender }, Self { receiver })
    }

    /// Return the latest complete snapshot without waiting for another update.
    pub fn snapshot(&self) -> Arc<PoolSummaries> {
        self.receiver.borrow().clone()
    }

    /// Wait for `epoch`, then return its summary for `pool`.
    ///
    /// `None` means that the distribution exists but contains no stake for the
    /// pool. A closed publisher also resolves to `None` during shutdown.
    pub async fn wait_for_epoch(&self, epoch: Epoch, pool: PoolId) -> Option<PoolSummary> {
        let mut receiver = self.receiver.clone();
        loop {
            let summaries = receiver.borrow_and_update().clone();
            if summaries.has_epoch(&epoch) {
                return summaries.get_pool_at_epoch(epoch, &pool);
            }
            if receiver.changed().await.is_err() {
                return None;
            }
        }
    }
}

impl StakeDistributionPublisher {
    /// Merge and publish a stake distribution computed by the ledger worker.
    pub fn publish(&self, update: PoolSummaries) {
        self.sender.send_modify(|current| *current = Arc::new(current.update(update)));
    }
}

/// Resource name retained for consumers that require stake-distribution data.
pub type ResourcePoolSummaries = StakeDistributionSource;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use amaru_kernel::Hash;

    use super::*;

    #[tokio::test]
    async fn waits_for_the_requested_epoch() {
        let pool = PoolId::new([0; 28]);
        let summary = PoolSummary { vrf: Hash::new([0; 32]), active_stake: 2, stake: 1 };
        let (publisher, source) = StakeDistributionSource::new(PoolSummaries::default());
        let waiting = tokio::spawn(async move { source.wait_for_epoch(Epoch::from(2), pool).await });

        tokio::task::yield_now().await;
        publisher.publish(PoolSummaries::new(Epoch::from(2), BTreeMap::from([(pool, summary)])));

        assert_eq!(waiting.await.expect("stake-distribution task"), Some(summary));
    }
}
