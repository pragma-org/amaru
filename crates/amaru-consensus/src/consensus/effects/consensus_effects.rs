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

use crate::consensus::effects::base_effects::{Base, BaseOps};
use crate::consensus::effects::ledger_effects::{Ledger, LedgerOps};
use crate::consensus::effects::network_effects::{Network, NetworkOps};
use crate::consensus::effects::store_effects::{Store, StoreOps};
use pure_stage::{Effects, SendData};

pub struct ConsensusEffects<T> {
    effects: Effects<T>,
}

impl<T: SendData + Sync> ConsensusEffects<T> {
    pub fn new(effects: Effects<T>) -> ConsensusEffects<T> {
        ConsensusEffects { effects }
    }

    pub fn store(&mut self) -> impl StoreOps {
        Store::new(&mut self.effects)
    }

    pub fn network(&self) -> impl NetworkOps {
        Network::new(&self.effects)
    }

    pub fn ledger(&self) -> impl LedgerOps {
        Ledger::new(&self.effects)
    }

    pub fn base(&self) -> impl BaseOps {
        Base::new(&self.effects)
    }
}

pub trait ConsensusOps {
    fn store(&mut self) -> impl StoreOps;
    fn network(&self) -> impl NetworkOps;
    fn ledger(&self) -> impl LedgerOps;
    fn base(&self) -> impl BaseOps;
}

impl<T: SendData + Sync> ConsensusOps for ConsensusEffects<T> {
    fn store(&mut self) -> impl StoreOps {
        self.store()
    }

    fn network(&self) -> impl NetworkOps {
        self.network()
    }

    fn ledger(&self) -> impl LedgerOps {
        self.ledger()
    }

    fn base(&self) -> impl BaseOps {
        self.base()
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::consensus::errors::{ConsensusError, ProcessingFailed};
    use amaru_kernel::peer::Peer;
    use amaru_kernel::{Header, Point, RawBlock};
    use amaru_ouroboros_traits::{BlockValidationError, Nonces};
    use amaru_slot_arithmetic::Epoch;
    use pallas_crypto::hash::Hash;
    use pure_stage::{BoxFuture, Instant, StageRef};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    pub struct MockConsensusOps {
        mock_store: MockStoreOps,
        mock_network: MockNetworkOps,
        mock_ledger: MockLedgerOps,
        mock_base: MockBaseOps,
    }

    #[allow(refining_impl_trait)]
    impl ConsensusOps for &mut MockConsensusOps {
        fn store(&mut self) -> &mut MockStoreOps {
            &mut self.mock_store
        }

        fn network(&self) -> &MockNetworkOps {
            &self.mock_network
        }

        fn ledger(&self) -> &MockLedgerOps {
            &self.mock_ledger
        }

        fn base(&self) -> &MockBaseOps {
            &self.mock_base
        }
    }

    #[allow(refining_impl_trait)]
    impl ConsensusOps for MockConsensusOps {
        fn store(&mut self) -> &mut MockStoreOps {
            &mut self.mock_store
        }

        fn network(&self) -> &MockNetworkOps {
            &self.mock_network
        }

        fn ledger(&self) -> &MockLedgerOps {
            &self.mock_ledger
        }

        fn base(&self) -> &MockBaseOps {
            &self.mock_base
        }
    }

    pub struct MockStoreOps;

    impl StoreOps for &mut MockStoreOps {
        async fn store_header(
            &mut self,
            _peer: &Peer,
            _header: &Header,
        ) -> Result<(), ProcessingFailed> {
            Ok(())
        }

        async fn store_block(
            &mut self,
            _peer: &Peer,
            _point: &Point,
            _block: &RawBlock,
        ) -> Result<(), ProcessingFailed> {
            Ok(())
        }

        async fn evolve_nonce(
            &mut self,
            _peer: &Peer,
            _header: &Header,
        ) -> Result<Nonces, ConsensusError> {
            Ok(Nonces {
                active: Hash::new([0; 32]),
                evolving: Hash::new([0; 32]),
                candidate: Hash::new([0; 32]),
                tail: Hash::new([0; 32]),
                epoch: Epoch::from(0),
            })
        }
    }

    pub struct MockNetworkOps {
        block_to_return: Arc<Mutex<Result<Vec<u8>, ConsensusError>>>,
    }

    impl Default for MockNetworkOps {
        fn default() -> Self {
            Self {
                block_to_return: Arc::new(Mutex::new(Ok(vec![]))),
            }
        }
    }

    impl MockNetworkOps {
        pub fn return_block(&self, block: Result<Vec<u8>, ConsensusError>) -> &Self {
            let mut self_block_to_return = self.block_to_return.lock().unwrap();
            *self_block_to_return = block;
            self
        }
    }

    impl NetworkOps for &MockNetworkOps {
        async fn fetch_block(
            &self,
            _peer: &Peer,
            point: &Point,
        ) -> Result<Vec<u8>, ConsensusError> {
            let self_block_to_return = self.block_to_return.lock().unwrap();
            match *self_block_to_return {
                Ok(ref block) => Ok(block.clone()),
                Err(_) => Err(ConsensusError::FetchBlockFailed(point.clone())),
            }
        }
    }

    pub struct MockLedgerOps;

    impl LedgerOps for &MockLedgerOps {
        async fn validate(
            &self,
            _peer: &Peer,
            _point: &Point,
            _block: RawBlock,
        ) -> Result<Result<u64, BlockValidationError>, BlockValidationError> {
            Ok(Ok(0))
        }

        async fn rollback(
            &self,
            _peer: &Peer,
            _point: &Point,
        ) -> anyhow::Result<(), ProcessingFailed> {
            Ok(())
        }
    }

    #[derive(Default)]
    pub struct MockBaseOps {
        messages: Arc<Mutex<BTreeMap<String, String>>>,
    }

    impl MockBaseOps {
        pub fn received(&self) -> BTreeMap<String, String> {
            self.messages.lock().unwrap().clone()
        }
    }

    impl BaseOps for &MockBaseOps {
        fn send<Msg: SendData + Sync>(
            &self,
            target: &StageRef<Msg>,
            msg: Msg,
        ) -> BoxFuture<'static, ()> {
            let mut messages = self.messages.lock().unwrap();
            messages.insert(target.name().to_string(), format!("{msg:?}"));
            Box::pin(async move {})
        }

        fn clock(&self) -> BoxFuture<'static, Instant> {
            Box::pin(async { Instant::at_offset(Duration::from_millis(0)) })
        }

        fn wait(&self, duration: Duration) -> BoxFuture<'static, Instant> {
            Box::pin(async move { Instant::at_offset(duration) })
        }

        fn terminate(&self) -> BoxFuture<'static, ()> {
            Box::pin(async {})
        }
    }

    pub fn mock_consensus_ops() -> MockConsensusOps {
        MockConsensusOps {
            mock_store: MockStoreOps,
            mock_network: MockNetworkOps::default(),
            mock_ledger: MockLedgerOps,
            mock_base: MockBaseOps::default(),
        }
    }
}
