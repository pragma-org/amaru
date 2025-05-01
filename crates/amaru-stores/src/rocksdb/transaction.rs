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

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tracing::error;

/// A wrapper to keep track of the status of an ongoing database transaction. This is handy to
/// ensure that we don't misuse db transactions while using the store by adding some invariant
/// around the tranasction lifecycle.
///
/// This is mostly useful during development and *could* be removed when building for production.
pub(crate) struct OngoingTransaction {
    value: Arc<AtomicBool>,
}

impl OngoingTransaction {
    pub(crate) fn new() -> Self {
        OngoingTransaction {
            value: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn get(&self) -> bool {
        self.value.load(Ordering::SeqCst)
    }

    pub(crate) fn set(&self, value: bool) {
        #[allow(clippy::panic)]
        if self.get() == value {
            // This is a bug, we should never set the same value twice. Crash the process.
            panic!("Ongoing transaction was already set to {}", value);
        }
        self.value.store(value, Ordering::SeqCst);
    }
}

impl Drop for OngoingTransaction {
    #[allow(clippy::panic)]
    fn drop(&mut self) {
        if self.get() {
            // This is a bug, no transaction should be left open. Crash the process.
            error!("Ongoing transaction was not closed before dropping RocksDB");
        }
    }
}
