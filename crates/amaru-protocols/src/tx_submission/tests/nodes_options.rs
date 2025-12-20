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

use crate::tx_submission::ResponderParams;
use crate::tx_submission::tests::SizedMempool;
use amaru_mempool::InMemoryMempool;
use amaru_ouroboros_traits::can_validate_transactions::mock::MockCanValidateTransactions;
use amaru_ouroboros_traits::{CanValidateTransactions, Mempool};
use pallas_primitives::conway::Tx;
use std::sync::Arc;

/// Options for setting up the tx submission nodes.
pub struct NodesOptions {
    pub responder_params: ResponderParams,
    pub initiator_mempool: Arc<dyn Mempool<Tx>>,
    pub responder_mempool: Arc<dyn Mempool<Tx>>,
}

impl Default for NodesOptions {
    fn default() -> Self {
        NodesOptions {
            responder_params: ResponderParams::new(4, 2),
            initiator_mempool: Arc::new(SizedMempool::with_tx_validator(
                4,
                Arc::new(MockCanValidateTransactions),
            )),
            responder_mempool: Arc::new(InMemoryMempool::default()),
        }
    }
}

impl NodesOptions {
    pub fn with_params(mut self, params: ResponderParams) -> Self {
        self.responder_params = params;
        self
    }

    pub fn with_max_window(mut self, size: u16) -> Self {
        self.responder_params.max_window = size;
        self
    }

    pub fn with_fetch_batch(mut self, size: u16) -> Self {
        self.responder_params.fetch_batch = size;
        self
    }

    pub fn with_initiator_mempool(mut self, mempool: Arc<dyn Mempool<Tx>>) -> Self {
        self.initiator_mempool = mempool;
        self
    }

    pub fn with_responder_mempool(mut self, mempool: Arc<dyn Mempool<Tx>>) -> Self {
        self.responder_mempool = mempool;
        self
    }

    pub fn with_initiator_mempool_capacity(self, size: u64) -> Self {
        self.with_initiator_mempool(Arc::new(SizedMempool::with_capacity(size)))
    }

    pub fn with_responder_tx_validator(
        self,
        tx_validator: Arc<dyn CanValidateTransactions<Tx>>,
    ) -> Self {
        self.with_responder_mempool(Arc::new(SizedMempool::with_tx_validator(4, tx_validator)))
    }
}
